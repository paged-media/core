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
use vello::wgpu;
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions};

use crate::vello_rs::{build_scene_for_surface, linear_to_peniko};

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
    #[error("render_to_surface failed: {0}")]
    RenderToSurface(String),
}

/// Long-lived presenter bound to one canvas. Constructed once at
/// editor mount; reused for every frame.
pub struct SurfacePresenter {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    surface_format: wgpu::TextureFormat,
    width: u32,
    height: u32,
}

impl SurfacePresenter {
    /// Create a presenter bound to `canvas`. Async because adapter
    /// + device requests are Promise-based on wasm.
    ///
    /// `width` / `height` are device-pixel dimensions — pass
    /// `canvas.width()` / `canvas.height()`, not the CSS size.
    pub async fn new(
        canvas: web_sys::HtmlCanvasElement,
        width: u32,
        height: u32,
    ) -> Result<Self, SurfaceError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| SurfaceError::CreateSurface(format!("{e:?}")))?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or(SurfaceError::NoAdapter)?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("idml-gpu surface device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            )
            .await
            .map_err(|e| SurfaceError::RequestDevice(format!("{e:?}")))?;

        // Pick the canvas's preferred format; fall back to a sensible
        // default if the capabilities query returns nothing.
        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);

        let renderer = Renderer::new(
            &device,
            RendererOptions {
                surface_format: Some(surface_format),
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: std::num::NonZeroUsize::new(1),
            },
        )
        .map_err(|e| SurfaceError::RendererInit(format!("{e:?}")))?;

        let presenter = Self {
            surface,
            device,
            queue,
            renderer,
            surface_format,
            width,
            height,
        };
        presenter.configure_surface();
        Ok(presenter)
    }

    /// Re-configure the surface after a resize. The editor calls this
    /// in a ResizeObserver and after dpr changes.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.configure_surface();
    }

    fn configure_surface(&self) {
        self.surface.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
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

    /// Render `list` and present onto the bound canvas. The viewport
    /// composes the page → surface transform; `background` is the
    /// canvas clear color (linear RGB, per display-list convention).
    pub fn present(
        &mut self,
        list: &DisplayList,
        viewport: Viewport,
        background: Color,
    ) -> Result<(), SurfaceError> {
        let frame = self
            .surface
            .get_current_texture()
            .map_err(|e| SurfaceError::GetTexture(format!("{e:?}")))?;

        let scene = build_scene_for_surface(list, viewport);

        self.renderer
            .render_to_surface(
                &self.device,
                &self.queue,
                &scene,
                &frame,
                &RenderParams {
                    base_color: linear_to_peniko(background),
                    width: self.width,
                    height: self.height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| SurfaceError::RenderToSurface(format!("{e:?}")))?;

        frame.present();
        Ok(())
    }
}
