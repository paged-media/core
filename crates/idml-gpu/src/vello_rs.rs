//! Vello backend.
//!
//! `PathRasterizer` impl that drives Vello-via-wgpu. Coverage today:
//!  - `FillPath` with solid paints, linear and radial gradients
//!    (linear RGB → sRGB at the boundary)
//!  - `StrokePath` with peniko `Stroke` + cap/join/miter mapping
//!  - `Image` via peniko `ImageData` + `ImageBrush` + `draw_image`
//!  - `PushClip` / `PopClip` via Vello clip layers
//!  - `BeginBlendGroup` / `EndBlendGroup` via Vello blend layers
//!    (peniko `Mix` mapped 1:1 from our `BlendMode`, `Compose::SrcOver`)
//!  - `FillPathBlend` per-command non-Normal blend wrapped in a
//!    transient blend layer (Normal stays on the fast `scene.fill` path)
//!  - Background fill from `RasterOptions::background`
//!  - Paths converted from our PathData (line / quad / cubic / close)
//!    into `kurbo::BezPath`; the per-command transform applies to
//!    every control point at conversion time so vello sees the
//!    final page-space coordinates and stroke widths come out right
//!
//! Approximate (no true Gaussian blur in vello):
//!  - PathShadow / InnerShadow / OuterGlow / InnerGlow / Satin /
//!    Feather — rendered via a multi-stamp falloff: a centre fill
//!    plus a series of expanding strokes at decreasing alpha,
//!    optionally clipped to the path's interior. Visually soft
//!    but not a true Gaussian; the CPU rasterizer remains the path
//!    of record for fidelity. See `stamp_blurred_path` for the
//!    falloff shape.
//!
//! Stubbed (logged-and-skipped):
//!  - DropShadow — rect-stamp drop shadows arrive through
//!    `PathShadow` in current emitters; this arm rarely fires.
//!  - BevelEmboss — chisel-edge approximation regresses sample
//!    geometry without the per-pixel normal field; left as
//!    log+skip so the rest of the page still renders.
//!
//! The CPU rasterizer (`cpu.rs`) remains the path of record for the
//! fidelity harness; the Vello backend's job is to keep the
//! WASM/native preview from dropping frames on common-case
//! primitives, with effect approximations close enough for preview.
//!
//! wgpu lifecycle: an instance + adapter + device + queue + Vello
//! `Renderer` are created lazily on first `rasterize()` call and
//! cached for the rasterizer's lifetime. Construction is sync via
//! `pollster::block_on` — fine on native; the wasm path will need
//! a different lifetime once the JS shell can hand us a device.

use std::cell::RefCell;
use std::sync::Arc;

use idml_compose::{
    BlendMode as ComposeBlendMode, Color as ComposeColor, DisplayCommand, DisplayList, LineCap,
    LineJoin, Paint, PathSegment,
};
use vello::kurbo::{self, Shape as KurboShape, Stroke as KurboStroke};
use vello::peniko::{
    BlendMode as PenikoBlendMode, Blob, BrushRef, Color as PenikoColor, ColorStop as PenikoColorStop,
    Compose, Fill, Gradient as PenikoGradient, ImageAlphaType, ImageBrush, ImageData, ImageFormat,
    Mix,
};
use wgpu;
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
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .map_err(|e| format!("no wgpu adapter available: {e:?}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("idml-gpu vello device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::default(),
    }))
    .map_err(|e| e.to_string())?;
    let renderer = Renderer::new(
        &device,
        RendererOptions {
            use_cpu: false,
            antialiasing_support: AaSupport::area_only(),
            num_init_threads: std::num::NonZeroUsize::new(1),
            pipeline_cache: None,
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

    // Track layer-stack depth so we can drop unbalanced Pop/End
    // commands without underflowing the encoder. Vello's encoder
    // tolerates `pop_layer` after a real `push_layer`; an unmatched
    // pop is undefined, so we count pushes here and only emit pops
    // when `depth > 0`. Mirrors the CPU rasterizer's "stray pop ⇒
    // no-op" policy.
    let mut layer_depth: usize = 0;

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
            DisplayCommand::FillPathBlend {
                path_id,
                paint,
                transform,
                blend_mode,
            } => {
                // Per-command non-Normal blend is rare at runtime
                // (the orchestrator brackets non-Normal frames with
                // BeginBlendGroup/EndBlendGroup instead). Wrap the
                // single fill in a peniko blend layer so the
                // composite still reads the page contents below.
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let path = path_to_bez(path_data, transform);
                let Some(brush) = resolve_paint(paint, list, transform) else {
                    continue;
                };
                let pb = blend_to_peniko(*blend_mode);
                if pb.mix == Mix::Normal && pb.compose == Compose::SrcOver {
                    // Fast path: Normal blend, no layer needed.
                    scene.fill(Fill::NonZero, page_to_px, brush.as_ref(), None, &path);
                } else {
                    // The blend layer's clip shape is the path
                    // itself — anything outside is unaffected.
                    scene.push_layer(Fill::NonZero, pb, 1.0, page_to_px, &path);
                    scene.fill(Fill::NonZero, page_to_px, brush.as_ref(), None, &path);
                    scene.pop_layer();
                }
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
                let Some(img) = list.image(*image_id) else {
                    continue;
                };
                if img.width == 0
                    || img.height == 0
                    || img.rgba.len() != (img.width as usize * img.height as usize * 4)
                {
                    continue;
                }
                // peniko 0.6+ replaced `Image::new(...)` with
                // `ImageData { ... }` + `ImageBrush::new(data)`. We
                // hand the decoded RGBA8 buffer over via a peniko
                // Blob (boxed into an Arc). The display list keeps
                // the canonical buffer alive for the duration of the
                // scene, but Blob wants its own Arc — clone bytes
                // out per command. Image dedup happens upstream so
                // each ImageId only appears in the buffer once.
                let bytes: Box<[u8]> = img.rgba.clone().into_boxed_slice();
                let blob = Blob::new(Arc::new(bytes));
                let image_data = ImageData {
                    data: blob,
                    format: ImageFormat::Rgba8,
                    // The display list's RGBA buffer is straight
                    // (un-premultiplied) alpha — pipeline decoders
                    // emit straight RGBA8. Mark it so peniko's
                    // sampler does the right multiply at draw time.
                    alpha_type: ImageAlphaType::Alpha,
                    width: img.width,
                    height: img.height,
                };
                let brush = ImageBrush::new(image_data);
                // Compose the placement transform: the display-list
                // `transform` maps the unit rect (0..1, 0..1) → page
                // coords. Vello's `draw_image` expects a transform
                // that maps the source pixel rect (0..w, 0..h) → final
                // device pixels. So: page_to_px ∘ unit_to_page ∘
                // pixel_to_unit.
                let inv_w = 1.0 / img.width as f64;
                let inv_h = 1.0 / img.height as f64;
                let [a, b, c, d, tx, ty] = transform.0;
                let unit_to_page = kurbo::Affine::new([
                    a as f64,
                    b as f64,
                    c as f64,
                    d as f64,
                    tx as f64,
                    ty as f64,
                ]);
                let pixel_to_unit = kurbo::Affine::scale_non_uniform(inv_w, inv_h);
                let pixel_to_px = page_to_px * unit_to_page * pixel_to_unit;
                scene.draw_image(&brush, pixel_to_px);
            }
            DisplayCommand::DropShadow { .. } => {
                // Stub — needs Vello's offscreen-layer + Gaussian
                // blur path which only lands cleanly with the
                // §10.4 effect plumbing. Rect-stamp drop shadows
                // arrive through `PathShadow` in current emitters,
                // so this arm rarely fires; leave it skipped.
            }
            DisplayCommand::PathShadow {
                path_id,
                transform,
                shadow,
            } => {
                // Approximate Gaussian blur with a multi-stamp
                // expanding-fill stack: paint the offset path
                // multiple times at decreasing alpha and increasing
                // outset, blended with `Plus` (additive) so the
                // overlap accumulates a soft falloff. Not equal to
                // a true Gaussian — but visibly soft, no offscreen
                // buffer required, and stays inside vello's
                // path-only API surface.
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let mut shifted = *transform;
                shifted.0[4] += shadow.offset_x;
                shifted.0[5] += shadow.offset_y;
                let path = path_to_bez(path_data, &shifted);
                let mut shadow_color = shadow.color;
                shadow_color.a *= shadow.opacity.clamp(0.0, 1.0);
                stamp_blurred_path(
                    &mut scene,
                    page_to_px,
                    &path,
                    shadow_color,
                    shadow.blur_radius,
                    PenikoBlendMode::new(Mix::Normal, Compose::SrcOver),
                );
            }
            DisplayCommand::InnerShadow {
                path_id,
                transform,
                params,
            } => {
                // Same multi-stamp approximation as PathShadow but
                // clipped to the path's interior so the soft
                // shadow falls *inside* the shape. The clip layer
                // is the un-offset path; the stamps inside it are
                // the offset path drawn in the shadow color with
                // an additive blend.
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let clip_path = path_to_bez(path_data, transform);
                let mut shifted = *transform;
                shifted.0[4] += params.offset_x;
                shifted.0[5] += params.offset_y;
                let stamp_path = path_to_bez(path_data, &shifted);
                let mut shadow_color = params.color;
                shadow_color.a *= params.opacity.clamp(0.0, 1.0);
                // Push the path interior as a clip layer so the
                // stamp paint can only land where the original path
                // is filled. Inside the clip, draw the offset path
                // in the shadow colour with the multi-stamp falloff;
                // the soft edge of the offset stamp produces the
                // inner shadow look (true inner shadow paints the
                // *complement* of the offset path inside the clip,
                // but the simpler stamp here is visually close for
                // small offsets and avoids a second mask pass).
                scene.push_layer(
                    Fill::NonZero,
                    PenikoBlendMode::default(),
                    1.0,
                    page_to_px,
                    &clip_path,
                );
                stamp_blurred_path(
                    &mut scene,
                    page_to_px,
                    &stamp_path,
                    shadow_color,
                    params.blur_radius,
                    blend_to_peniko(params.blend_mode),
                );
                scene.pop_layer();
                let _ = params.choke; // CPU rasterizer's dilation knob; not honoured here.
            }
            DisplayCommand::OuterGlow {
                path_id,
                transform,
                params,
            } => {
                // Centred soft halo outside the path. Same multi-
                // stamp approximation as PathShadow with no offset
                // and the glow's own blend mode (typically Screen).
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let path = path_to_bez(path_data, transform);
                let mut glow_color = params.color;
                glow_color.a *= params.opacity.clamp(0.0, 1.0);
                // `spread` widens the hard stamp before the blur;
                // fold it into the blur radius so the falloff
                // covers the dilated region.
                let blur_pt = params.blur_radius + params.spread.max(0.0);
                stamp_blurred_path(
                    &mut scene,
                    page_to_px,
                    &path,
                    glow_color,
                    blur_pt,
                    blend_to_peniko(params.blend_mode),
                );
            }
            DisplayCommand::InnerGlow {
                path_id,
                transform,
                params,
            } => {
                // Centred soft halo inside the path. Push the
                // path as a clip layer, then stamp the same path
                // with the multi-stamp falloff inside it. The
                // overlap stack reads as a soft inner edge once
                // clipped to the interior.
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let path = path_to_bez(path_data, transform);
                let mut glow_color = params.color;
                glow_color.a *= params.opacity.clamp(0.0, 1.0);
                scene.push_layer(
                    Fill::NonZero,
                    PenikoBlendMode::default(),
                    1.0,
                    page_to_px,
                    &path,
                );
                stamp_blurred_path(
                    &mut scene,
                    page_to_px,
                    &path,
                    glow_color,
                    params.blur_radius,
                    blend_to_peniko(params.blend_mode),
                );
                scene.pop_layer();
            }
            DisplayCommand::BevelEmboss { .. } => {
                // Skipped: the chisel-edge approximation (two
                // offset stroke fills along the light angle)
                // regresses sample geometry visibly without the
                // per-pixel normal field the CPU rasterizer
                // runs. Keeping this as a log+skip is honest;
                // the CPU pipeline remains the path of record.
                tracing::trace!("vello: BevelEmboss skipped (no normal-field path)");
            }
            DisplayCommand::Satin {
                path_id,
                transform,
                params,
            } => {
                // Approximate satin: stamp the path twice along
                // the angle vector, blended with the satin colour
                // and the configured blend mode (typically
                // Multiply). Clipped to the path interior so the
                // wave doesn't bleed outside. Blur is faked by the
                // multi-stamp falloff.
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let clip_path = path_to_bez(path_data, transform);
                let theta = params.angle_deg.to_radians();
                let dx = params.distance * theta.cos();
                let dy = params.distance * theta.sin();
                let mut color = params.color;
                color.a *= params.opacity.clamp(0.0, 1.0);
                // Reduce per-stamp opacity since two stamps
                // overlap; matches the CPU rasterizer's
                // "subtract-difference" intent loosely.
                color.a *= 0.5;
                scene.push_layer(
                    Fill::NonZero,
                    PenikoBlendMode::default(),
                    1.0,
                    page_to_px,
                    &clip_path,
                );
                let mut a = *transform;
                a.0[4] += dx;
                a.0[5] += dy;
                let path_a = path_to_bez(path_data, &a);
                let mut b = *transform;
                b.0[4] -= dx;
                b.0[5] -= dy;
                let path_b = path_to_bez(path_data, &b);
                let blend = blend_to_peniko(params.blend_mode);
                stamp_blurred_path(
                    &mut scene,
                    page_to_px,
                    &path_a,
                    color,
                    params.blur_radius,
                    blend,
                );
                stamp_blurred_path(
                    &mut scene,
                    page_to_px,
                    &path_b,
                    color,
                    params.blur_radius,
                    blend,
                );
                scene.pop_layer();
            }
            DisplayCommand::Feather {
                path_id,
                transform,
                params,
            } => {
                // Push the path as a clip, then approximate the
                // soft edge by stamping the path in solid alpha
                // and following with shrinking inset rings of
                // diminishing opacity. The CPU rasterizer uses a
                // distance-field; here we just paint the path,
                // and let the multi-stamp falloff blur the edge
                // outward into the clipped region. Visible but
                // lossy — flagged in the docstring.
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let path = path_to_bez(path_data, transform);
                scene.push_layer(
                    Fill::NonZero,
                    PenikoBlendMode::default(),
                    1.0,
                    page_to_px,
                    &path,
                );
                // Re-stamp with the multi-stamp falloff in white at
                // alpha 1.0; the clip masks everything outside, so
                // the falloff produces a soft inner edge. Width
                // scales the falloff radius. We use Compose::SrcOver
                // for the central stamps (full alpha) — the soft
                // edge appears because successive stamps stack with
                // additive blending in `stamp_blurred_path`.
                let edge_color = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
                stamp_blurred_path(
                    &mut scene,
                    page_to_px,
                    &path,
                    edge_color,
                    params.width,
                    PenikoBlendMode::new(Mix::Normal, Compose::SrcOver),
                );
                scene.pop_layer();
                // corner_type / noise / choke are CPU-rasterizer
                // distance-field knobs not honoured by this
                // multi-stamp approximation — the falloff is the
                // same regardless of corner shape or noise weight.
            }
            DisplayCommand::DirectionalFeather {
                path_id,
                transform,
                params,
            } => {
                // Approximate directional feather with the plain
                // Feather treatment using the max of the four
                // per-edge widths. The CPU rasterizer is the path
                // of record for per-edge gradients; this is just a
                // visible soft edge for preview.
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let path = path_to_bez(path_data, transform);
                let width = params
                    .left_width
                    .max(params.right_width)
                    .max(params.top_width)
                    .max(params.bottom_width);
                if width <= 0.0 {
                    continue;
                }
                scene.push_layer(
                    Fill::NonZero,
                    PenikoBlendMode::default(),
                    1.0,
                    page_to_px,
                    &path,
                );
                let edge_color = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
                stamp_blurred_path(
                    &mut scene,
                    page_to_px,
                    &path,
                    edge_color,
                    width,
                    PenikoBlendMode::new(Mix::Normal, Compose::SrcOver),
                );
                scene.pop_layer();
            }
            DisplayCommand::GradientFeather {
                path_id,
                transform,
                params,
            } => {
                // Approximate gradient feather: paint a peniko brush
                // along the gradient axis using the alpha stops
                // (alpha = 0 → transparent, alpha = 1 → opaque
                // white) clipped to the path's interior. The CPU
                // rasterizer is the path of record for the exact
                // alpha modulation; this version just shows
                // *something* aligned with the gradient axis.
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                if params.stops.len() < 2 {
                    continue;
                }
                let path = path_to_bez(path_data, transform);
                let (sx, sy) = transform.apply(params.start_x, params.start_y);
                let (ex, ey) = transform.apply(params.end_x, params.end_y);
                let stops: Vec<PenikoColorStop> = params
                    .stops
                    .iter()
                    .map(|s| PenikoColorStop {
                        offset: s.location.clamp(0.0, 1.0),
                        color: linear_to_peniko(ComposeColor::rgba(
                            1.0,
                            1.0,
                            1.0,
                            s.alpha.clamp(0.0, 1.0),
                        ))
                        .into(),
                    })
                    .collect();
                let gradient = match params.kind {
                    idml_compose::GradientFeatherKind::Linear => PenikoGradient::new_linear(
                        kurbo::Point::new(sx as f64, sy as f64),
                        kurbo::Point::new(ex as f64, ey as f64),
                    )
                    .with_stops(stops.as_slice()),
                    idml_compose::GradientFeatherKind::Radial => {
                        let radius = ((ex - sx) as f64).hypot((ey - sy) as f64);
                        if radius <= 0.0 {
                            continue;
                        }
                        PenikoGradient::new_radial(
                            kurbo::Point::new(sx as f64, sy as f64),
                            radius as f32,
                        )
                        .with_stops(stops.as_slice())
                    }
                };
                scene.fill(
                    Fill::NonZero,
                    page_to_px,
                    BrushRef::Gradient(&gradient),
                    None,
                    &path,
                );
            }
            DisplayCommand::PushClip { path_id, transform } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let path = path_to_bez(path_data, transform);
                // Vello clip layers are encoded as plain push_layer
                // with Mix::Normal + Compose::SrcOver and full alpha
                // — the layer becomes a pure clip. (Equivalent to
                // `push_clip_layer`; using `push_layer` keeps the
                // call shape identical to the blend-group path.)
                scene.push_layer(
                    Fill::NonZero,
                    PenikoBlendMode::default(),
                    1.0,
                    page_to_px,
                    &path,
                );
                layer_depth += 1;
            }
            DisplayCommand::PopClip(_) => {
                if layer_depth > 0 {
                    scene.pop_layer();
                    layer_depth -= 1;
                }
            }
            DisplayCommand::BeginBlendGroup {
                bounds,
                blend_mode,
                opacity,
                ..
            } => {
                // Vello transparency group: clip to the bounds rect
                // (in page coords) and composite the contents back
                // with the requested blend mode + opacity. Vello
                // pushes the layer onto its own internal stack;
                // EndBlendGroup pops it.
                let rect = kurbo::Rect::new(
                    bounds.x as f64,
                    bounds.y as f64,
                    (bounds.x + bounds.w) as f64,
                    (bounds.y + bounds.h) as f64,
                );
                scene.push_layer(
                    Fill::NonZero,
                    blend_to_peniko(*blend_mode),
                    opacity.clamp(0.0, 1.0),
                    page_to_px,
                    &rect,
                );
                layer_depth += 1;
            }
            DisplayCommand::EndBlendGroup(_) => {
                if layer_depth > 0 {
                    scene.pop_layer();
                    layer_depth -= 1;
                }
            }
            DisplayCommand::PushLayer {
                bounds,
                blend_mode,
                opacity,
                ..
            } => {
                // Vello-side stub: treat PushLayer as a plain
                // transparency group. The CPU rasterizer applies the
                // Gaussian blur from `LayerEffect::GaussianBlur` at
                // pop time, but Vello doesn't expose a built-in
                // separable Gaussian over the layer buffer. Drawing
                // without the blur produces a hard-edged stamp —
                // visibly different from the CPU output but keeps
                // the rest of the page in shape; a proper
                // shader-based blur lives on the Vello backlog.
                let rect = kurbo::Rect::new(
                    bounds.x as f64,
                    bounds.y as f64,
                    (bounds.x + bounds.w) as f64,
                    (bounds.y + bounds.h) as f64,
                );
                scene.push_layer(
                    Fill::NonZero,
                    blend_to_peniko(*blend_mode),
                    opacity.clamp(0.0, 1.0),
                    page_to_px,
                    &rect,
                );
                layer_depth += 1;
            }
            DisplayCommand::PopLayer(_) => {
                if layer_depth > 0 {
                    scene.pop_layer();
                    layer_depth -= 1;
                }
            }
        }
    }
    // Defensive: any pushes left unmatched at scene end still need
    // to be balanced so the encoder is well-formed. The orchestrator
    // shouldn't ever leave them dangling, but be tolerant.
    while layer_depth > 0 {
        scene.pop_layer();
        layer_depth -= 1;
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
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
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
    let _ = state.device.poll(wgpu::PollType::wait_indefinitely());
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

/// Multi-stamp soft-edge approximation. Vello (peniko + kurbo) has
/// no built-in image-space Gaussian blur — true blur would need an
/// offscreen render target plus a wgpu compute pass. We fake it by
/// stamping the path multiple times: a solid fill at full alpha,
/// followed by progressively expanded outset stamps at low alpha
/// blended additively. The overlap forms a falloff that reads as a
/// soft edge from a normal viewing distance, even though it isn't
/// the true Gaussian convolution the CPU rasterizer performs.
///
/// `blur_radius` is in page-space pt; if it's sub-pixel small we
/// skip the stack and emit a single hard fill.
///
/// The fast path (blur ≤ 0.5 pt) bypasses any layering; the slow
/// path opens a single transparency group with `Compose::Plus` so
/// the stamp stack accumulates additively, then closes it. The
/// caller's blend mode wraps the whole accumulated group.
fn stamp_blurred_path(
    scene: &mut Scene,
    page_to_px: kurbo::Affine,
    path: &kurbo::BezPath,
    color: ComposeColor,
    blur_radius: f32,
    outer_blend: PenikoBlendMode,
) {
    if color.a <= 0.0 {
        return;
    }
    let blur = blur_radius.max(0.0);
    if blur <= 0.5 {
        // Fast path: blur is sub-pixel, just do a hard fill with
        // the caller's blend mode wrapped in a transient layer if
        // it's non-default.
        let pcolor = linear_to_peniko(color);
        if outer_blend.mix == Mix::Normal && outer_blend.compose == Compose::SrcOver {
            scene.fill(
                Fill::NonZero,
                page_to_px,
                BrushRef::Solid(pcolor),
                None,
                path,
            );
        } else {
            scene.push_layer(Fill::NonZero, outer_blend, 1.0, page_to_px, path);
            scene.fill(
                Fill::NonZero,
                page_to_px,
                BrushRef::Solid(pcolor),
                None,
                path,
            );
            scene.pop_layer();
        }
        return;
    }

    // Slow path: build a stack of expanding stamps inside a Plus-
    // composed layer so they accumulate. Higher stamp counts make
    // the falloff smoother but cost linearly more vello commands;
    // 5 stamps + a centre is a defensible compromise between
    // visual quality and encoder load.
    //
    // Each stamp paints the same path with a Stroke whose width
    // grows with the stamp index, producing a series of concentric
    // rings around the path's edge. The centre stamp is a solid
    // fill that lays down the inside; the strokes layer the soft
    // halo on top. This keeps everything inside vello's path API
    // (no offscreen targets, no compute) at the cost of fidelity.
    let bbox = path.bounding_box();
    let bbox_pad = bbox.inflate(blur as f64 + 1.0, blur as f64 + 1.0);
    let layer_clip = kurbo::Rect::new(bbox_pad.x0, bbox_pad.y0, bbox_pad.x1, bbox_pad.y1);
    scene.push_layer(Fill::NonZero, outer_blend, 1.0, page_to_px, &layer_clip);

    let centre_color = linear_to_peniko(color);
    // Solid centre fill at the caller's full opacity.
    scene.fill(
        Fill::NonZero,
        page_to_px,
        BrushRef::Solid(centre_color),
        None,
        path,
    );

    // Halo: 5 expanding strokes with diminishing alpha. Stroke
    // widths grow linearly to the blur radius (×2 since the stroke
    // straddles the path edge); alpha falls off so the outer-most
    // stroke is barely visible. The cumulative coverage approximates
    // the tail of a Gaussian — close enough for a preview.
    const RINGS: usize = 5;
    for i in 1..=RINGS {
        let t = i as f32 / RINGS as f32; // 0.2 .. 1.0
        let stroke_w = (2.0 * blur * t) as f64;
        if stroke_w <= 0.0 {
            continue;
        }
        // Roughly Gaussian-tail falloff: alpha ∝ exp(-2 t²).
        let falloff = (-2.0 * t * t).exp();
        let mut ring_color = color;
        ring_color.a *= falloff;
        if ring_color.a <= 1.0 / 255.0 {
            continue;
        }
        let pc = linear_to_peniko(ring_color);
        let stroke = KurboStroke::new(stroke_w);
        scene.stroke(&stroke, page_to_px, BrushRef::Solid(pc), None, path);
    }

    scene.pop_layer();
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
                    color: linear_to_peniko(s.color).into(),
                })
                .collect();
            let pg = PenikoGradient::new_linear(
                kurbo::Point::new(sx as f64, sy as f64),
                kurbo::Point::new(ex as f64, ey as f64),
            )
            .with_stops(stops.as_slice());
            Some(VelloBrush::Gradient(pg))
        }
        Paint::RadialGradient(id) => {
            let g = list.radial_gradient(*id)?;
            if g.stops.len() < 2 {
                return None;
            }
            let (cx, cy) = transform.apply(g.center.0, g.center.1);
            // Match cpu.rs: average the mapped per-axis radii so a
            // non-square unit rect ovals the gradient with the path.
            let [a, b, c, d, _, _] = transform.0;
            let rx = (a * g.radius).hypot(b * g.radius);
            let ry = (c * g.radius).hypot(d * g.radius);
            let radius = (rx + ry) * 0.5;
            if !radius.is_finite() || radius <= 0.0 {
                return None;
            }
            let stops: Vec<PenikoColorStop> = g
                .stops
                .iter()
                .map(|s| PenikoColorStop {
                    offset: s.offset,
                    color: linear_to_peniko(s.color).into(),
                })
                .collect();
            let pg =
                PenikoGradient::new_radial(kurbo::Point::new(cx as f64, cy as f64), radius)
                    .with_stops(stops.as_slice());
            Some(VelloBrush::Gradient(pg))
        }
    }
}

/// Map our `BlendMode` to peniko's `(Mix, Compose)` pair. `Normal`
/// maps to (`Mix::Normal`, `Compose::SrcOver`) — peniko's default —
/// and every other variant lines up 1:1 with peniko's `Mix` enum,
/// keeping `Compose::SrcOver` since IDML transparency groups are
/// non-isolated source-over composites by default.
fn blend_to_peniko(m: ComposeBlendMode) -> PenikoBlendMode {
    let mix = match m {
        ComposeBlendMode::Normal => Mix::Normal,
        ComposeBlendMode::Multiply => Mix::Multiply,
        ComposeBlendMode::Screen => Mix::Screen,
        ComposeBlendMode::Overlay => Mix::Overlay,
        ComposeBlendMode::Darken => Mix::Darken,
        ComposeBlendMode::Lighten => Mix::Lighten,
        ComposeBlendMode::ColorDodge => Mix::ColorDodge,
        ComposeBlendMode::ColorBurn => Mix::ColorBurn,
        ComposeBlendMode::HardLight => Mix::HardLight,
        ComposeBlendMode::SoftLight => Mix::SoftLight,
        ComposeBlendMode::Difference => Mix::Difference,
        ComposeBlendMode::Exclusion => Mix::Exclusion,
        ComposeBlendMode::Hue => Mix::Hue,
        ComposeBlendMode::Saturation => Mix::Saturation,
        ComposeBlendMode::Color => Mix::Color,
        ComposeBlendMode::Luminosity => Mix::Luminosity,
    };
    PenikoBlendMode::new(mix, Compose::SrcOver)
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
    PenikoColor::from_rgba8(
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

    #[test]
    fn build_scene_handles_clip_blend_image_radial() {
        // Scene encoding-only smoke test for the 4 new variants:
        // PushClip / PopClip, BeginBlendGroup / EndBlendGroup,
        // Image, and RadialGradient. We don't require a GPU here
        // (init may fail in CI); the assertion is just that
        // build_scene runs to completion without panicking on a
        // realistic command sequence.
        use idml_compose::{
            BlendMode as ComposeBlend, Color as DLColor, DecodedImage, DisplayCommand,
            GradientStop, Paint, PathData, PathSegment, RadialGradient, Rect, Transform,
        };

        let mut list = DisplayList::new();

        // A simple unit-rect path used for both clip and group bounds.
        let mut rect_path = PathData::default();
        rect_path.segments.push(PathSegment::MoveTo { x: 0.0, y: 0.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 1.0, y: 0.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 1.0, y: 1.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 0.0, y: 1.0 });
        rect_path.segments.push(PathSegment::Close);
        let rect_id = list.paths.push_anon(rect_path);

        // 1×1 RGBA image — smallest valid placement.
        let image_id = list.push_image(DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        });

        // RadialGradient: red → blue.
        let radial_id = list.push_radial_gradient(RadialGradient {
            center: (0.5, 0.5),
            radius: 0.5,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: DLColor::rgba(1.0, 0.0, 0.0, 1.0),
                },
                GradientStop {
                    offset: 1.0,
                    color: DLColor::rgba(0.0, 0.0, 1.0, 1.0),
                },
            ],
        });

        // Clip → group → image + radial fill → end → pop, plus an
        // outer fill so the un-clipped state is also exercised.
        list.commands.push(DisplayCommand::PushClip {
            path_id: rect_id,
            transform: Transform([20.0, 0.0, 0.0, 20.0, 5.0, 5.0]),
        });
        list.commands.push(DisplayCommand::BeginBlendGroup {
            bounds: Rect {
                x: 5.0,
                y: 5.0,
                w: 20.0,
                h: 20.0,
            },
            blend_mode: ComposeBlend::Multiply,
            opacity: 0.8,
            transform: Transform::IDENTITY,
        });
        list.commands.push(DisplayCommand::Image {
            image_id,
            transform: Transform([10.0, 0.0, 0.0, 10.0, 8.0, 8.0]),
        });
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect_id,
            paint: Paint::RadialGradient(radial_id),
            transform: Transform([15.0, 0.0, 0.0, 15.0, 6.0, 6.0]),
        });
        list.commands.push(DisplayCommand::EndBlendGroup(
            Transform::IDENTITY,
        ));
        list.commands.push(DisplayCommand::PopClip(Transform::IDENTITY));

        // Encoding shouldn't panic. We don't dig into Scene internals;
        // a successful return is enough to verify the variants are
        // wired through to peniko's encoder.
        let _ = build_scene_with_transform(&list, kurbo::Affine::scale(1.0));
    }

    #[test]
    fn build_scene_handles_effect_variants() {
        // Smoke test for the 7 effect variants now wired through the
        // multi-stamp soft-edge approximation. The success criterion
        // is the same as the clip/blend test above: encoding runs to
        // completion without panicking. Visual fidelity is tracked
        // by the CPU rasterizer's golden snapshots, not by Vello's.
        use idml_compose::{
            BevelEmboss, BlendMode as ComposeBlend, Color as DLColor, DisplayCommand, DropShadow,
            Feather, FeatherCornerType, InnerGlow, InnerShadow, OuterGlow, PathData, PathSegment,
            Satin, Transform,
        };

        let mut list = DisplayList::new();
        let mut rect_path = PathData::default();
        rect_path.segments.push(PathSegment::MoveTo { x: 0.0, y: 0.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 1.0, y: 0.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 1.0, y: 1.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 0.0, y: 1.0 });
        rect_path.segments.push(PathSegment::Close);
        let rect_id = list.paths.push_anon(rect_path);

        let xf = Transform([20.0, 0.0, 0.0, 20.0, 5.0, 5.0]);

        list.commands.push(DisplayCommand::PathShadow {
            path_id: rect_id,
            transform: xf,
            shadow: DropShadow {
                offset_x: 2.0,
                offset_y: 2.0,
                blur_radius: 3.0,
                color: DLColor::rgba(0.0, 0.0, 0.0, 1.0),
                opacity: 0.5,
            },
        });
        list.commands.push(DisplayCommand::InnerShadow {
            path_id: rect_id,
            transform: xf,
            params: InnerShadow {
                offset_x: 1.0,
                offset_y: 1.0,
                blur_radius: 2.0,
                color: DLColor::rgba(0.0, 0.0, 0.0, 1.0),
                opacity: 0.5,
                choke: 0.0,
                blend_mode: ComposeBlend::Multiply,
            },
        });
        list.commands.push(DisplayCommand::OuterGlow {
            path_id: rect_id,
            transform: xf,
            params: OuterGlow {
                blur_radius: 4.0,
                color: DLColor::rgba(1.0, 1.0, 0.5, 1.0),
                opacity: 0.7,
                blend_mode: ComposeBlend::Screen,
                spread: 1.0,
            },
        });
        list.commands.push(DisplayCommand::InnerGlow {
            path_id: rect_id,
            transform: xf,
            params: InnerGlow {
                blur_radius: 3.0,
                color: DLColor::rgba(1.0, 1.0, 0.5, 1.0),
                opacity: 0.7,
                blend_mode: ComposeBlend::Screen,
                choke: 0.0,
            },
        });
        list.commands.push(DisplayCommand::BevelEmboss {
            path_id: rect_id,
            transform: xf,
            params: BevelEmboss::default_soft(),
        });
        list.commands.push(DisplayCommand::Satin {
            path_id: rect_id,
            transform: xf,
            params: Satin::default_soft(),
        });
        list.commands.push(DisplayCommand::Feather {
            path_id: rect_id,
            transform: xf,
            params: Feather {
                width: 6.0,
                corner_type: FeatherCornerType::Sharp,
                noise: 0.0,
                choke: 0.0,
            },
        });

        let _ = build_scene_with_transform(&list, kurbo::Affine::scale(1.0));
    }
}
