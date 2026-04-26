//! Vello backend.
//!
//! `PathRasterizer` impl that drives Vello-via-wgpu. Coverage today:
//!  - `FillPath` with solid paints (linear RGB → sRGB at the boundary)
//!  - Background fill from `RasterOptions::background`
//!  - Paths converted from our PathData (line / quad / cubic / close)
//!    into `kurbo::BezPath`; the per-command transform applies to
//!    every control point at conversion time so vello sees the
//!    final page-space coordinates and stroke widths come out right
//!
//! Stubbed (logged-and-skipped):
//!  - StrokePath (path conversion is wired; just needs the
//!    `scene.stroke` call with peniko Stroke + brush)
//!  - DropShadow (vello 0.3 has limited shadow primitives; the
//!    plan is a Gaussian-blur layer once §10.4 lands)
//!  - Image (decoded RGBA buffers → `peniko::Image` + draw_image)
//!  - LinearGradient paint resolution (peniko::Gradient stops)
//!
//! wgpu lifecycle: an instance + adapter + device + queue + Vello
//! `Renderer` are created lazily on first `rasterize()` call and
//! cached for the rasterizer's lifetime. Construction is sync via
//! `pollster::block_on` — fine on native; the wasm path will need
//! a different lifetime once the JS shell can hand us a device.

use std::cell::RefCell;

use idml_compose::{Color as ComposeColor, DisplayCommand, DisplayList, Paint, PathSegment};
use vello::peniko::{kurbo, Color as PenikoColor, Fill};
use vello::wgpu;
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

use crate::{PathRasterizer, RasterOptions};

pub struct VelloRasterizer {
    /// Lazily initialised on first rasterize call so constructing
    /// the rasterizer doesn't require a GPU + adapter probe.
    state: RefCell<Option<GpuState>>,
}

impl Default for VelloRasterizer {
    fn default() -> Self {
        Self {
            state: RefCell::new(None),
        }
    }
}

impl VelloRasterizer {
    pub fn new() -> Self {
        Self::default()
    }
}

struct GpuState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
}

impl PathRasterizer for VelloRasterizer {
    fn name(&self) -> &'static str {
        "vello/wgpu"
    }

    fn rasterize(&self, list: &DisplayList, options: &RasterOptions) -> Vec<u8> {
        let (px_w, px_h) = options.pixel_size();
        let mut state_borrow = self.state.borrow_mut();
        if state_borrow.is_none() {
            *state_borrow = match init_gpu() {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!(error = %e, "vello: GPU init failed; returning empty image");
                    return vec![0; (px_w * px_h * 4) as usize];
                }
            };
        }
        let state = state_borrow.as_mut().unwrap();

        let scene = build_scene(list, options);
        match render_scene_to_buffer(state, &scene, options) {
            Ok(buf) => buf,
            Err(e) => {
                tracing::warn!(error = %e, "vello: render_to_texture failed");
                vec![0; (px_w * px_h * 4) as usize]
            }
        }
    }
}

fn init_gpu() -> Result<GpuState, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok_or_else(|| "no wgpu adapter available".to_string())?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("idml-gpu vello device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    ))
    .map_err(|e| e.to_string())?;
    let renderer = Renderer::new(
        &device,
        RendererOptions {
            surface_format: None,
            use_cpu: false,
            antialiasing_support: AaSupport::area_only(),
            num_init_threads: std::num::NonZeroUsize::new(1),
        },
    )
    .map_err(|e| format!("Renderer::new: {e:?}"))?;
    Ok(GpuState {
        device,
        queue,
        renderer,
    })
}

/// Walk the display list and build a vello `Scene`. Commands we
/// don't yet handle log + skip; the scene still renders the parts
/// we do understand so the result isn't all-or-nothing.
fn build_scene(list: &DisplayList, options: &RasterOptions) -> Scene {
    let mut scene = Scene::new();
    let scale = options.dpi / 72.0;
    let page_to_px = kurbo::Affine::scale(scale as f64);

    for cmd in &list.commands {
        match cmd {
            DisplayCommand::FillPath {
                path_id,
                paint,
                transform,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let path = path_to_bez(path_data, transform);
                let color = match paint_solid_or_skip(paint) {
                    Some(c) => c,
                    None => {
                        // TODO: gradients via peniko::Gradient
                        continue;
                    }
                };
                scene.fill(Fill::NonZero, page_to_px, color, None, &path);
            }
            DisplayCommand::StrokePath { .. }
            | DisplayCommand::DropShadow { .. }
            | DisplayCommand::Image { .. } => {
                // Stubs — see module docs.
            }
        }
    }
    scene
}

fn render_scene_to_buffer(
    state: &mut GpuState,
    scene: &Scene,
    options: &RasterOptions,
) -> Result<Vec<u8>, String> {
    let (width, height) = options.pixel_size();
    let texture = state.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("idml-gpu vello target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    state
        .renderer
        .render_to_texture(
            &state.device,
            &state.queue,
            scene,
            &view,
            &RenderParams {
                base_color: linear_to_peniko(options.background),
                width,
                height,
                antialiasing_method: AaConfig::Area,
            },
        )
        .map_err(|e| format!("render_to_texture: {e:?}"))?;

    // Copy texture → buffer → CPU. Row pitch must be a multiple of
    // wgpu::COPY_BYTES_PER_ROW_ALIGNMENT (256); we copy with that
    // alignment then tighten on the CPU side.
    let bpr_aligned = wgpu::util::align_to(width * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer_size = (bpr_aligned * height) as u64;
    let buffer = state.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("idml-gpu vello readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = state
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("idml-gpu vello readback enc"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bpr_aligned),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    state.queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    slice.map_async(wgpu::MapMode::Read, move |res| {
        tx.send(res).ok();
    });
    state.device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .map_err(|e| format!("map_async recv: {e}"))?
        .map_err(|e| format!("buffer map: {e:?}"))?;
    let mapped = slice.get_mapped_range();
    let bpr_tight = (width * 4) as usize;
    let mut out = Vec::with_capacity(bpr_tight * height as usize);
    for row in 0..height as usize {
        let start = row * bpr_aligned as usize;
        out.extend_from_slice(&mapped[start..start + bpr_tight]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(out)
}

fn path_to_bez(
    data: &idml_compose::PathData,
    transform: &idml_compose::Transform,
) -> kurbo::BezPath {
    let apply = |x: f32, y: f32| -> kurbo::Point {
        let [a, b, c, d, tx, ty] = transform.0;
        kurbo::Point::new((a * x + c * y + tx) as f64, (b * x + d * y + ty) as f64)
    };
    let mut p = kurbo::BezPath::new();
    for seg in &data.segments {
        match seg {
            PathSegment::MoveTo { x, y } => p.move_to(apply(*x, *y)),
            PathSegment::LineTo { x, y } => p.line_to(apply(*x, *y)),
            PathSegment::QuadTo { cx, cy, x, y } => {
                p.quad_to(apply(*cx, *cy), apply(*x, *y));
            }
            PathSegment::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => {
                p.curve_to(apply(*cx1, *cy1), apply(*cx2, *cy2), apply(*x, *y));
            }
            PathSegment::Close => p.close_path(),
        }
    }
    p
}

fn paint_solid_or_skip(paint: &Paint) -> Option<PenikoColor> {
    match paint {
        Paint::Solid(c) => Some(linear_to_peniko(*c)),
        Paint::LinearGradient(_) => None,
    }
}

/// Linear RGB (0..1) → sRGB-encoded peniko Color (the format
/// vello expects). Mirrors cpu.rs's `linear_color_to_ts`.
fn linear_to_peniko(c: ComposeColor) -> PenikoColor {
    let to_srgb = |v: f32| -> u8 {
        let s = if v <= 0.0031308 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (s.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    PenikoColor::rgba8(
        to_srgb(c.r),
        to_srgb(c.g),
        to_srgb(c.b),
        (c.a.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizer_constructs_without_panicking() {
        // GPU init is lazy; constructing should be cheap and never
        // probe the system.
        let _ = VelloRasterizer::new();
    }

    #[test]
    fn empty_list_produces_pixel_count_buffer() {
        // GPU init may or may not succeed in the test environment.
        // The rasterizer should always return a buffer of the right
        // size — either rendered output or the zero-fill fallback.
        let v = VelloRasterizer::new();
        let mut opts = RasterOptions::new(20.0, 10.0);
        opts.dpi = 72.0;
        let buf = v.rasterize(&DisplayList::new(), &opts);
        assert_eq!(buf.len(), 20 * 10 * 4);
    }
}
