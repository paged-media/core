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

use idml_compose::{
    Color as ComposeColor, DisplayCommand, DisplayList, LineCap, LineJoin, Paint, PathSegment,
};
use vello::peniko::{
    kurbo::{self, Stroke as KurboStroke},
    Blob, BrushRef, Color as PenikoColor, ColorStop as PenikoColorStop, Fill,
    Format as PenikoFormat, Gradient as PenikoGradient, Image as PenikoImage,
};
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
    let scale = options.dpi / 72.0;
    let page_to_px = kurbo::Affine::scale(scale as f64);
    build_scene_with_transform(list, page_to_px)
}

/// Surface-presenter entrypoint. Same scene-building as `build_scene`
/// but parameterised by the editor's `Viewport` (page → device-pixel
/// transform). Lives behind the wasm32 cfg because `Viewport` is in
/// `surface.rs` which only compiles for browser builds.
#[cfg(target_arch = "wasm32")]
pub(crate) fn build_scene_for_surface(
    list: &DisplayList,
    viewport: crate::surface::Viewport,
) -> Scene {
    let scale = (viewport.base_scale * viewport.zoom * viewport.dpr) as f64;
    let pan_x = (viewport.pan_x * viewport.dpr) as f64;
    let pan_y = (viewport.pan_y * viewport.dpr) as f64;
    let page_to_surface = kurbo::Affine::translate((pan_x, pan_y)) * kurbo::Affine::scale(scale);
    build_scene_with_transform(list, page_to_surface)
}

fn build_scene_with_transform(list: &DisplayList, page_to_px: kurbo::Affine) -> Scene {
    let mut scene = Scene::new();

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
                let Some(brush) = resolve_paint(paint, list, transform) else {
                    continue;
                };
                scene.fill(Fill::NonZero, page_to_px, brush.as_ref(), None, &path);
            }
            DisplayCommand::StrokePath {
                path_id,
                paint,
                stroke,
                transform,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let path = path_to_bez(path_data, transform);
                let Some(brush) = resolve_paint(paint, list, transform) else {
                    continue;
                };
                let ks = KurboStroke::new(stroke.width.max(0.0) as f64)
                    .with_caps(map_cap(stroke.cap))
                    .with_join(map_join(stroke.join))
                    .with_miter_limit(stroke.miter_limit.max(1.0) as f64);
                scene.stroke(&ks, page_to_px, brush.as_ref(), None, &path);
            }
            DisplayCommand::Image {
                image_id,
                transform,
            } => {
                let Some(decoded) = list.image(*image_id) else {
                    continue;
                };
                if decoded.width == 0
                    || decoded.height == 0
                    || decoded.rgba.len() != (decoded.width as usize * decoded.height as usize * 4)
                {
                    continue;
                }
                let blob = Blob::new(std::sync::Arc::new(decoded.rgba.clone()));
                let img =
                    PenikoImage::new(blob, PenikoFormat::Rgba8, decoded.width, decoded.height);
                // Display-list transform maps unit-rect (0..1, 0..1)
                // → page coords. The image's pixel grid lives in
                // (0..w, 0..h), so divide by (w, h) before composing
                // with the unit-rect transform and the page→px scale.
                let inv_w = 1.0 / decoded.width as f64;
                let inv_h = 1.0 / decoded.height as f64;
                let pixel_to_unit = kurbo::Affine::scale_non_uniform(inv_w, inv_h);
                let unit_to_page = kurbo::Affine::new([
                    transform.0[0] as f64,
                    transform.0[1] as f64,
                    transform.0[2] as f64,
                    transform.0[3] as f64,
                    transform.0[4] as f64,
                    transform.0[5] as f64,
                ]);
                let pixel_to_px = page_to_px * unit_to_page * pixel_to_unit;
                scene.draw_image(&img, pixel_to_px);
            }
            DisplayCommand::DropShadow { .. } => {
                // Stub — needs Vello's offscreen-layer + Gaussian
                // blur path which only lands cleanly with the
                // §10.4 effect plumbing.
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

/// Owned brush variant to keep both arms of `BrushRef` alive long
/// enough for the `scene.fill` / `scene.stroke` call. `BrushRef`
/// borrows from a `peniko::Color` or a `peniko::Gradient`; we hold
/// whichever one we built and convert via `as_ref()` at the call
/// site.
enum VelloBrush {
    Solid(PenikoColor),
    Gradient(PenikoGradient),
}

impl VelloBrush {
    fn as_ref(&self) -> BrushRef<'_> {
        match self {
            VelloBrush::Solid(c) => BrushRef::Solid(*c),
            VelloBrush::Gradient(g) => BrushRef::Gradient(g),
        }
    }
}

/// Convert a display-list `Paint` (solid or gradient id) into a
/// vello brush. Gradient endpoints in the display list live in
/// the path's unit-rect local coordinates; we apply the path's
/// `transform` to them so the brush's coordinates land in the
/// same page-space the shape lives in (vello's `brush_transform`
/// is `None` here, meaning "brush in shape's local coords").
fn resolve_paint(
    paint: &Paint,
    list: &DisplayList,
    transform: &idml_compose::Transform,
) -> Option<VelloBrush> {
    match paint {
        Paint::Solid(c) => Some(VelloBrush::Solid(linear_to_peniko(*c))),
        Paint::LinearGradient(id) => {
            let g = list.linear_gradient(*id)?;
            if g.stops.len() < 2 {
                return None;
            }
            let (sx, sy) = transform.apply(g.start.0, g.start.1);
            let (ex, ey) = transform.apply(g.end.0, g.end.1);
            let stops: Vec<PenikoColorStop> = g
                .stops
                .iter()
                .map(|s| PenikoColorStop {
                    offset: s.offset,
                    color: linear_to_peniko(s.color),
                })
                .collect();
            let pg = PenikoGradient::new_linear(
                kurbo::Point::new(sx as f64, sy as f64),
                kurbo::Point::new(ex as f64, ey as f64),
            )
            .with_stops(stops.as_slice());
            Some(VelloBrush::Gradient(pg))
        }
    }
}

fn map_cap(c: LineCap) -> kurbo::Cap {
    match c {
        LineCap::Butt => kurbo::Cap::Butt,
        LineCap::Round => kurbo::Cap::Round,
        LineCap::Square => kurbo::Cap::Square,
    }
}

fn map_join(j: LineJoin) -> kurbo::Join {
    match j {
        LineJoin::Miter => kurbo::Join::Miter,
        LineJoin::Round => kurbo::Join::Round,
        LineJoin::Bevel => kurbo::Join::Bevel,
    }
}

/// Linear RGB (0..1) → sRGB-encoded peniko Color (the format
/// vello expects). Mirrors cpu.rs's `linear_color_to_ts`.
pub(crate) fn linear_to_peniko(c: ComposeColor) -> PenikoColor {
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
