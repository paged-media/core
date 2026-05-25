//! Live wgpu Surface presentation for the editor.
//!
//! The existing `VelloRasterizer` renders offscreen → reads back →
//! returns RGBA bytes. That's right for export and the fidelity
//! harness, but wrong for an interactive editor: we want the GPU
//! frame to land directly on the browser canvas with no readback.
//!
//! `SurfacePresenter` owns its own `wgpu::Surface` bound to an
//! `HtmlCanvasElement` and a long-lived device + queue + Vello
//! `Renderer`. Construction is async (the browser's GPU adapter
//! request is a Promise); subsequent presents are synchronous.
//!
//! M0 scope: present a single `DisplayList` at a given viewport
//! transform. Multi-page culling, dirty-region presents, and
//! incremental scene reuse arrive with the editor milestones that
//! actually need them.
//!
//! Native surface presentation (winit window, raw window handle) is
//! a future concern; this module is wasm32-only by design.

#![cfg(target_arch = "wasm32")]

use idml_compose::{Color, DisplayList};
use image::ImageEncoder;
use vello::kurbo;
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

use crate::vello_rs::{build_scene_for_surface, build_scene_with_transform, linear_to_peniko};

/// Viewport transform applied on top of the page → px scale before
/// presenting. The editor uses this for zoom/pan/dpr — the renderer
/// is happy as long as the final transform leaves coordinates in
/// surface-pixel space.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    /// Page-pt → CSS-px scale before zoom (typically `dpi / 72.0`).
    pub base_scale: f32,
    /// User-controlled zoom factor; 1.0 == 100%.
    pub zoom: f32,
    /// Pan in CSS-px, applied after scale.
    pub pan_x: f32,
    pub pan_y: f32,
    /// Device-pixel ratio. Surface is sized in device pixels; CSS
    /// coordinates are scaled by this factor before being baked into
    /// the affine.
    pub dpr: f32,
}

impl Viewport {
    /// Identity-ish viewport at 1:1 with no pan, dpr=1.
    pub fn identity() -> Self {
        Self {
            base_scale: 1.0,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            dpr: 1.0,
        }
    }

    /// Surface-space transform a point picked up from `dpr`-CSS-px
    /// would need to land in the painted page. The presenter uses
    /// the inverse internally; exposing the forward form here keeps
    /// the math testable in isolation.
    pub fn page_to_surface_scale(&self) -> f32 {
        self.base_scale * self.zoom * self.dpr
    }
}

/// Errors that can come out of surface construction or presentation.
#[derive(Debug, thiserror::Error)]
pub enum SurfaceError {
    #[error("create_surface failed: {0}")]
    CreateSurface(String),
    #[error("no compatible wgpu adapter")]
    NoAdapter,
    #[error("request_device failed: {0}")]
    RequestDevice(String),
    #[error("Renderer::new failed: {0}")]
    RendererInit(String),
    #[error("get_current_texture failed: {0}")]
    GetTexture(String),
    #[error("render failed: {0}")]
    Render(String),
    #[error("texture readback failed: {0}")]
    Readback(String),
    #[error("PNG encode failed: {0}")]
    PngEncode(String),
}

/// Long-lived presenter bound to one canvas. Constructed once at
/// editor mount; reused for every frame.
pub struct SurfacePresenter {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    surface_format: wgpu::TextureFormat,
    /// Intermediate Rgba8Unorm storage texture Vello renders into.
    /// Vello's compute pipeline cannot bind the surface texture
    /// directly on most GPUs (Apple Metal in particular silently
    /// drops the writes), so we render here and then blit.
    target_texture: wgpu::Texture,
    target_view: wgpu::TextureView,
    blitter: wgpu::util::TextureBlitter,
    width: u32,
    height: u32,
}

impl SurfacePresenter {
    /// Create a presenter bound to a main-thread `HtmlCanvasElement`.
    /// Async because adapter + device requests are Promise-based on
    /// wasm.
    ///
    /// `width` / `height` are device-pixel dimensions — pass
    /// `canvas.width()` / `canvas.height()`, not the CSS size.
    pub async fn new(
        canvas: web_sys::HtmlCanvasElement,
        width: u32,
        height: u32,
    ) -> Result<Self, SurfaceError> {
        Self::new_inner(wgpu::SurfaceTarget::Canvas(canvas), width, height).await
    }

    /// Worker-side variant: takes an `OffscreenCanvas` that has been
    /// transferred from the main thread. The canvas worker uses this
    /// path because it owns the OffscreenCanvas, not the main thread.
    pub async fn new_offscreen(
        canvas: web_sys::OffscreenCanvas,
        width: u32,
        height: u32,
    ) -> Result<Self, SurfaceError> {
        Self::new_inner(wgpu::SurfaceTarget::OffscreenCanvas(canvas), width, height).await
    }

    async fn new_inner(
        target: wgpu::SurfaceTarget<'static>,
        width: u32,
        height: u32,
    ) -> Result<Self, SurfaceError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(target)
            .map_err(|e| SurfaceError::CreateSurface(format!("{e:?}")))?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|_| SurfaceError::NoAdapter)?;

        // Chrome's canvas typically only exposes `Bgra8Unorm` /
        // `Bgra8UnormSrgb`. Vello renders into a STORAGE_BINDING
        // texture; writing to a Bgra8Unorm storage texture requires
        // the `BGRA8UNORM_STORAGE` device feature. Request it when
        // the adapter advertises it; gracefully fall back if not.
        let mut required_features = wgpu::Features::empty();
        if adapter
            .features()
            .contains(wgpu::Features::BGRA8UNORM_STORAGE)
        {
            required_features |= wgpu::Features::BGRA8UNORM_STORAGE;
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("idml-gpu surface device"),
                required_features,
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::default(),
            })
            .await
            .map_err(|e| SurfaceError::RequestDevice(format!("{e:?}")))?;

        // Vello renders into an intermediate Rgba8Unorm storage
        // texture; `TextureBlitter` then copies it onto the surface
        // (which keeps RENDER_ATTACHMENT-only usage). This matches
        // vello/util's recommended pattern.
        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
                )
            })
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
            .or_else(|| caps.formats.first().copied())
            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);

        let renderer = Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: std::num::NonZeroUsize::new(1),
                pipeline_cache: None,
            },
        )
        .map_err(|e| SurfaceError::RendererInit(format!("{e:?}")))?;

        let (target_texture, target_view) = create_target(width, height, &device);
        let blitter = wgpu::util::TextureBlitter::new(&device, surface_format);

        let presenter = Self {
            surface,
            device,
            queue,
            renderer,
            surface_format,
            target_texture,
            target_view,
            blitter,
            width,
            height,
        };
        presenter.configure_surface();
        Ok(presenter)
    }

    /// Surface dimensions in device pixels. Read at any time by the
    /// worker to drive visibility-culling math.
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Re-configure the surface after a resize. The editor calls this
    /// in a ResizeObserver and after dpr changes.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        let (t, v) = create_target(self.width, self.height, &self.device);
        self.target_texture = t;
        self.target_view = v;
        self.configure_surface();
    }

    fn configure_surface(&self) {
        self.surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                // RENDER_ATTACHMENT only — Vello's compute pipeline
                // cannot bind the surface texture directly on most
                // GPUs. The blit happens through `TextureBlitter`.
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.surface_format,
                width: self.width,
                height: self.height,
                present_mode: wgpu::PresentMode::AutoVsync,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            },
        );
    }

    /// Build a Vello `Scene` from a display list in page-document
    /// space (identity transform). The result can be cached and
    /// composed via `present_scenes` each frame, avoiding the
    /// per-frame display-list traversal.
    ///
    /// `width_pt` / `height_pt` are the page's dimensions in points;
    /// the scene is prefixed with a white background rect + 1pt
    /// grey border so the page chrome stays consistent across the
    /// CPU and GPU paths.
    pub fn build_page_scene(list: &DisplayList, width_pt: f32, height_pt: f32) -> Scene {
        use vello::peniko;

        let mut scene = Scene::new();
        // White page body.
        scene.fill(
            peniko::Fill::NonZero,
            kurbo::Affine::IDENTITY,
            peniko::Color::from_rgba8(255, 255, 255, 255),
            None,
            &kurbo::Rect::new(0.0, 0.0, width_pt as f64, height_pt as f64),
        );
        // 1pt grey border around the page edge.
        scene.stroke(
            &kurbo::Stroke::new(1.0),
            kurbo::Affine::IDENTITY,
            peniko::Color::from_rgba8(180, 180, 180, 255),
            None,
            &kurbo::Rect::new(0.0, 0.0, width_pt as f64, height_pt as f64),
        );
        // Append the page's content on top.
        let content = build_scene_with_transform(list, kurbo::Affine::IDENTITY);
        scene.append(&content, None);
        scene
    }

    /// Compose previously-built per-page scenes onto the surface,
    /// each at its own affine transform. The transform parallels
    /// `present_multi`'s `[a, b, c, d, e, f]` convention.
    ///
    /// This is the sub-phase D hot path: cached scenes mean per-frame
    /// cost is just `Scene::append` (a memcpy + transform composition)
    /// per visible page, not a full display-list walk.
    pub fn present_scenes(
        &mut self,
        pages: &[(&Scene, [f32; 6])],
        background: Color,
    ) -> Result<(), SurfaceError> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            other => {
                return Err(SurfaceError::GetTexture(format!("{other:?}")));
            }
        };

        let mut combined = Scene::new();
        for (scene, t) in pages {
            let affine = kurbo::Affine::new([
                t[0] as f64,
                t[1] as f64,
                t[2] as f64,
                t[3] as f64,
                t[4] as f64,
                t[5] as f64,
            ]);
            combined.append(scene, Some(affine));
        }

        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                &combined,
                &self.target_view,
                &RenderParams {
                    base_color: linear_to_peniko(background),
                    width: self.width,
                    height: self.height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| SurfaceError::Render(format!("{e:?}")))?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("idml-gpu surface blit scenes"),
            });
        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.blitter.copy(
            &self.device,
            &mut encoder,
            &self.target_view,
            &surface_view,
        );
        self.queue.submit([encoder.finish()]);
        frame.present();
        Ok(())
    }

    /// Present multiple display lists in one frame, each with its
    /// own page-to-surface transform (the `[a, b, c, d, e, f]` affine
    /// matrix from `idml_compose::Transform`). The canvas worker
    /// uses this to lay out all visible pages under the camera
    /// transform without merging per-page resource pools.
    pub fn present_multi(
        &mut self,
        pages: &[(&DisplayList, [f32; 6])],
        background: Color,
    ) -> Result<(), SurfaceError> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            other => {
                return Err(SurfaceError::GetTexture(format!("{other:?}")));
            }
        };

        // Compose one scene from many per-page scenes. `Scene::append`
        // copies the appended scene's draws into the target with the
        // supplied transform — Vello handles resource id rewriting
        // internally so we don't have to merge path / gradient pools
        // by hand.
        let mut combined = Scene::new();
        for (list, t) in pages {
            // Per-page transform comes in as the row-major affine
            // [a, b, c, d, e, f]; kurbo::Affine takes [a, b, c, d, e, f]
            // in the same order, so the cast is direct.
            let affine = kurbo::Affine::new([
                t[0] as f64,
                t[1] as f64,
                t[2] as f64,
                t[3] as f64,
                t[4] as f64,
                t[5] as f64,
            ]);
            let scene = build_scene_with_transform(list, kurbo::Affine::IDENTITY);
            combined.append(&scene, Some(affine));
        }

        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                &combined,
                &self.target_view,
                &RenderParams {
                    base_color: linear_to_peniko(background),
                    width: self.width,
                    height: self.height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| SurfaceError::Render(format!("{e:?}")))?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("idml-gpu surface blit multi"),
            });
        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.blitter.copy(
            &self.device,
            &mut encoder,
            &self.target_view,
            &surface_view,
        );
        self.queue.submit([encoder.finish()]);
        frame.present();
        Ok(())
    }

    /// Render `list` and present onto the bound canvas. The viewport
    /// composes the page → surface transform; `background` is the
    /// canvas clear color (linear RGB, per display-list convention).
    ///
    /// Vello main no longer exposes `render_to_surface`; we render to
    /// an intermediate offscreen texture and blit it onto the surface.
    /// The editor's hot path stays GPU-only — no CPU readback.
    pub fn present(
        &mut self,
        list: &DisplayList,
        viewport: Viewport,
        background: Color,
    ) -> Result<(), SurfaceError> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            other => {
                return Err(SurfaceError::GetTexture(format!("{other:?}")));
            }
        };

        let scene = build_scene_for_surface(list, viewport);

        // Vello renders into the intermediate Rgba8Unorm storage
        // texture (`target_view`); we then blit it onto the surface.
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                &scene,
                &self.target_view,
                &RenderParams {
                    base_color: linear_to_peniko(background),
                    width: self.width,
                    height: self.height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| SurfaceError::Render(format!("{e:?}")))?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("idml-gpu surface blit"),
            });
        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.blitter.copy(
            &self.device,
            &mut encoder,
            &self.target_view,
            &surface_view,
        );
        self.queue.submit([encoder.finish()]);

        frame.present();
        Ok(())
    }

    /// Sub-phase D — render a pre-built Vello `Scene` off-surface to
    /// a PNG. The caller is responsible for building the scene
    /// (typically via `SurfacePresenter::build_page_scene` + an
    /// optional pt→px scale transform), so this method can run with
    /// only a mutable borrow on the presenter — no second borrow on
    /// the document model.
    ///
    /// Reuses the presenter's device + queue + renderer; the wgpu
    /// adapter init isn't paid per call.
    pub async fn render_scene_to_png(
        &mut self,
        scene: &Scene,
        width_px: u32,
        height_px: u32,
    ) -> Result<Vec<u8>, SurfaceError> {
        let width_px = width_px.max(1);
        let height_px = height_px.max(1);

        // Render target: separate from the presenter's surface-bound
        // intermediate so concurrent surface presents (live editor) and
        // offscreen renders (fidelity test) don't trample each other.
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("idml-gpu vello readback target"),
            size: wgpu::Extent3d {
                width: width_px,
                height: height_px,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            format: wgpu::TextureFormat::Rgba8Unorm,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                scene,
                &target_view,
                &RenderParams {
                    base_color: linear_to_peniko(Color::WHITE),
                    width: width_px,
                    height: height_px,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| SurfaceError::Render(format!("{e:?}")))?;

        // `copy_texture_to_buffer` requires the row stride to be a
        // multiple of 256 bytes. Allocate a padded buffer, then strip
        // the padding row-by-row when we copy out the RGBA bytes.
        let bytes_per_pixel: u32 = 4;
        let row_bytes = width_px * bytes_per_pixel;
        let padded_row_bytes = row_bytes.div_ceil(256) * 256;
        let buffer_size = (padded_row_bytes as u64) * (height_px as u64);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("idml-gpu vello readback buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("idml-gpu vello readback encoder"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(height_px),
                },
            },
            wgpu::Extent3d {
                width: width_px,
                height: height_px,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        // Async map: on wasm32 we use a oneshot channel + a poll loop.
        let slice = readback.slice(..);
        let (tx, rx) = futures_channel::oneshot::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        // Trigger the map by polling — on web targets, polling is a
        // no-op (the browser drives the GPU queue), but we still need
        // to flush our submit and wait for the callback.
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.await
            .map_err(|_| SurfaceError::Readback("map callback dropped".into()))?
            .map_err(|e| SurfaceError::Readback(format!("map_async: {e:?}")))?;

        let mut rgba = Vec::with_capacity((row_bytes as usize) * (height_px as usize));
        {
            let data = slice.get_mapped_range();
            for row in 0..height_px {
                let start = (row as usize) * (padded_row_bytes as usize);
                let end = start + (row_bytes as usize);
                rgba.extend_from_slice(&data[start..end]);
            }
        }
        readback.unmap();

        let mut png_bytes = Vec::with_capacity(rgba.len() / 4);
        image::codecs::png::PngEncoder::new(&mut png_bytes)
            .write_image(&rgba, width_px, height_px, image::ExtendedColorType::Rgba8)
            .map_err(|e| SurfaceError::PngEncode(e.to_string()))?;
        Ok(png_bytes)
    }
}

fn create_target(
    width: u32,
    height: u32,
    device: &wgpu::Device,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("idml-gpu vello target"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        format: wgpu::TextureFormat::Rgba8Unorm,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}
