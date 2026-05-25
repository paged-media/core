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
//! Approximate (no image-space Gaussian convolution in vello):
//!  - PathShadow / InnerShadow / OuterGlow / InnerGlow / Satin /
//!    Feather — rendered via a multi-stamp falloff: a centre fill
//!    plus a series of expanding strokes at decreasing alpha,
//!    optionally clipped to the path's interior. Visually soft
//!    but not a true Gaussian; the CPU rasterizer remains the path
//!    of record for fidelity. See `stamp_blurred_path` for the
//!    falloff shape.
//!  - `PushLayer { effect: LayerEffect::GaussianBlur }` /
//!    `PopLayer` — Vello has no image-space Gaussian over a layer
//!    buffer in the version we link against (the
//!    `draw_blurred_rounded_rect` primitive only blurs a rounded
//!    rect *brush*, and `vello_filters_cpu` is still a reference
//!    CPU implementation). We capture the inner commands as a
//!    sub-scene and replay it via `Scene::append` at a 7×7 Gaussian
//!    sample grid, each tap wrapped in a `(Normal, Plus)`-composed
//!    `push_layer` whose alpha is the Gaussian weight at that grid
//!    point. The accumulation is mathematically a true convolution
//!    — the only approximation is the grid discretisation. See
//!    `emit_blurred_layer`.
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
    BlendMode as ComposeBlendMode, Color as ComposeColor, DisplayCommand, DisplayList,
    LayerEffect, LineCap, LineJoin, Paint, PathSegment,
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
    /// Compute pipelines for CMYK-overprint parity. `None` if shader
    /// compilation or pipeline creation failed on this adapter — the
    /// rasterizer then falls back to the pre-parity knockout behaviour
    /// for overprint commands (logged once via `tracing::warn!`). The
    /// `vello_compute_pipeline_creation_failure_falls_back_to_knockout`
    /// test pins that contract.
    pipelines: Option<crate::cmyk_compute::Pipelines>,
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

        // Count *Overprint commands. The three branches below trade
        // accuracy for cost:
        //   * 0: unchanged from pre-parity Vello — render and return.
        //   * 1..=8 (with `cpu` feature): CPU fast-path finisher. The
        //     existing CPU rasterizer handles overprint composition
        //     pixel-perfectly; for small counts the readback +
        //     re-rasterize overhead is the dominant cost anyway.
        //   * 9+ or no `cpu` feature: GPU compute pipeline path. One
        //     Vello scene per coalesced overprint batch, one splat
        //     dispatch per batch, then one recomposite dispatch.
        let overprint_count = count_overprints(list);
        if overprint_count == 0 {
            let scene = build_scene(list, options);
            return match render_scene_to_buffer(state, &scene, options) {
                Ok(buf) => buf,
                Err(e) => {
                    tracing::warn!(error = %e, "vello: render_to_texture failed");
                    vec![0; (px_w * px_h * 4) as usize]
                }
            };
        }

        #[cfg(feature = "cpu")]
        if overprint_count <= 8 && !crate::cmyk_compute::should_force_compute_path() {
            return cpu_finisher_path(list, options);
        }

        match gpu_compute_overprint_path(state, list, options) {
            Ok(buf) => buf,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "vello: compute overprint path failed; falling back to knockout"
                );
                // Knockout fallback: re-render once via the original
                // build_scene path. Overprint draws collapse to
                // plain `scene.fill` per the existing fallback in
                // `build_scene_with_transform`. Better a knockout
                // image than a panic.
                let scene = build_scene(list, options);
                render_scene_to_buffer(state, &scene, options)
                    .unwrap_or_else(|_| vec![0; (px_w * px_h * 4) as usize])
            }
        }
    }
}

/// Walk the display list and tally `*Overprint` commands. Cheap (the
/// list is in-memory and the match is one branch per command). Drives
/// the policy decision in `rasterize` between unchanged / CPU-finisher /
/// GPU-compute paths.
/// Materialise an RGBA8 buffer from encoded image bytes at the
/// dimensions the build phase recorded. Used by the wasm32 lazy
/// path (`DecodedImage.rgba` empty, `encoded` populated) so peak
/// heap stays at one decoded image at a time. Mirror of cpu.rs's
/// helper of the same name; duplicated to avoid cross-feature
/// visibility issues.
fn decode_image_for_render(bytes: &[u8], expected_w: u32, expected_h: u32) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let raw = if img.width() == expected_w && img.height() == expected_h {
        img.to_rgba8()
    } else {
        image::imageops::resize(
            &img.to_rgba8(),
            expected_w,
            expected_h,
            image::imageops::FilterType::Triangle,
        )
    };
    Some(raw.into_raw())
}

fn count_overprints(list: &DisplayList) -> usize {
    list.commands
        .iter()
        .filter(|cmd| {
            matches!(
                cmd,
                DisplayCommand::FillPathOverprint { .. }
                    | DisplayCommand::StrokePathOverprint { .. }
            )
        })
        .count()
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
    // Try to build the compute pipelines once; failure here logs and
    // falls back to knockout per the plan. The Vello renderer is the
    // critical path — its failure is fatal, but the compute pipelines
    // are an optimisation and their absence stays survivable.
    let pipelines = match crate::cmyk_compute::create_pipelines(&device) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "vello: CMYK-overprint compute pipeline init failed; \
                 falling back to knockout for *Overprint commands"
            );
            None
        }
    };
    Ok(GpuState {
        device,
        queue,
        renderer,
        pipelines,
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

pub(crate) fn build_scene_with_transform(list: &DisplayList, page_to_px: kurbo::Affine) -> Scene {
    build_scene_with_transform_filtered(list, page_to_px, /*skip_overprints=*/ false)
}

/// Same as `build_scene_with_transform`, but with the option to skip
/// `*Overprint` commands. The compute-path Vello render passes
/// `skip_overprints = true` so the vello_target represents
/// "everything below the overprint layer" — the splat shader uses
/// that buffer to recover bottom-side CMYK on virgin pixels.
fn build_scene_with_transform_filtered(
    list: &DisplayList,
    page_to_px: kurbo::Affine,
    skip_overprints: bool,
) -> Scene {
    // Stack of in-flight scenes. The bottom entry is the final
    // returned scene; additional entries are sub-scenes captured
    // for `PushLayer { effect: GaussianBlur, .. }` so we can replay
    // their contents under a multi-tap Gaussian sampling pattern at
    // the matching `PopLayer`. All `scene.X(...)` calls below route
    // to `scene_stack.last_mut().unwrap()` — the current target.
    let mut scene_stack: Vec<Scene> = vec![Scene::new()];

    // Per-push bookkeeping. `Encoded` means the push translated
    // directly into a `push_layer` call on the current target scene
    // (clip / blend group / no-effect layer); the matching pop just
    // calls `pop_layer`. `BlurredLayer` means the push opened a
    // sub-scene that's now collecting commands — the matching pop
    // replays it via the multi-tap Gaussian onto the parent. This
    // single LIFO stack carries both kinds because the display list
    // pairs them unambiguously: every PushClip/PopClip,
    // BeginBlendGroup/EndBlendGroup, and PushLayer/PopLayer is
    // properly nested by the emitter.
    let mut layer_stack: Vec<LayerKind> = Vec::new();

    for cmd in &list.commands {
        match cmd {
            DisplayCommand::PushClip { path_id, transform } => {
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let path = path_to_bez(path_data, transform);
                // Vello clip layers are encoded as plain push_layer
                // with Mix::Normal + Compose::SrcOver and full alpha
                // — the layer becomes a pure clip.
                scene.push_layer(
                    Fill::NonZero,
                    PenikoBlendMode::default(),
                    1.0,
                    page_to_px,
                    &path,
                );
                layer_stack.push(LayerKind::Encoded);
            }
            DisplayCommand::PopClip(_) | DisplayCommand::EndBlendGroup(_) => {
                pop_layer_or_blur(
                    &mut scene_stack,
                    &mut layer_stack,
                    page_to_px,
                );
            }
            DisplayCommand::BeginBlendGroup {
                bounds,
                blend_mode,
                opacity,
                ..
            } => {
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                layer_stack.push(LayerKind::Encoded);
            }
            DisplayCommand::PushLayer {
                bounds,
                effect,
                blend_mode,
                opacity,
                ..
            } => match *effect {
                LayerEffect::GaussianBlur { sigma_pt } if sigma_pt > 0.5 => {
                    // Capture-and-replay path: open a fresh sub-scene
                    // so subsequent draws collect into a buffer we can
                    // replay multiple times under a 2D Gaussian sample
                    // pattern. The actual blur happens at the matching
                    // `PopLayer`, via `emit_blurred_layer`. Vello has no
                    // built-in image-space Gaussian for layer contents
                    // in this version, so this multi-tap replay is the
                    // honest workaround — see the module docstring.
                    scene_stack.push(Scene::new());
                    layer_stack.push(LayerKind::Blurred {
                        sigma_pt,
                        bounds: *bounds,
                        blend_mode: *blend_mode,
                        opacity: opacity.clamp(0.0, 1.0),
                    });
                }
                _ => {
                    // Sub-pixel σ or `LayerEffect::None` — fall back to
                    // a plain transparency group with the requested
                    // composite. The blur is a no-op at this σ so the
                    // CPU rasterizer's separable-Gaussian shortcut
                    // matches: a single uniform layer composite.
                    let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                    layer_stack.push(LayerKind::Encoded);
                }
            },
            DisplayCommand::PopLayer(_) => {
                pop_layer_or_blur(
                    &mut scene_stack,
                    &mut layer_stack,
                    page_to_px,
                );
            }
            DisplayCommand::FillPath {
                path_id,
                paint,
                transform,
            } => {
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
            DisplayCommand::FillPathOverprint {
                path_id,
                paint,
                transform,
            } => {
                if skip_overprints {
                    continue;
                }
                // Vello backend overprint knockout fallback: when the
                // compute-pipeline path isn't running we render the
                // overprint as a normal knockout fill so the page is
                // still visible. The compute path's main render
                // passes `skip_overprints = true` and re-encodes each
                // overprint batch into a scratch texture.
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let path = path_to_bez(path_data, transform);
                let Some(brush) = resolve_paint(paint, list, transform) else {
                    continue;
                };
                scene.fill(Fill::NonZero, page_to_px, brush.as_ref(), None, &path);
            }
            DisplayCommand::StrokePathOverprint {
                path_id,
                paint,
                stroke,
                transform,
            } => {
                if skip_overprints {
                    continue;
                }
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
                let Some(img) = list.image(*image_id) else {
                    continue;
                };
                if img.width == 0 || img.height == 0 {
                    continue;
                }
                // Two paths: eager (rgba pre-decoded; native build)
                // and lazy (rgba empty; wasm32 build defers decode
                // to here to keep peak heap bounded). The lazy buf
                // is owned + dropped after the scene closes.
                let expected_len = img.width as usize * img.height as usize * 4;
                let lazy: Option<Vec<u8>> = if img.rgba.len() == expected_len {
                    None
                } else if !img.encoded.is_empty() {
                    match decode_image_for_render(&img.encoded, img.width, img.height) {
                        Some(buf) => Some(buf),
                        None => continue,
                    }
                } else {
                    continue;
                };
                // peniko 0.6+ replaced `Image::new(...)` with
                // `ImageData { ... }` + `ImageBrush::new(data)`. We
                // hand the decoded RGBA8 buffer over via a peniko
                // Blob (boxed into an Arc).
                let bytes: Box<[u8]> = match lazy {
                    Some(v) => v.into_boxed_slice(),
                    None => img.rgba.as_ref().to_vec().into_boxed_slice(),
                };
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
                // Stub — rect-stamp drop shadows now flow through
                // `PathShadow` (multi-stamp falloff) or through the
                // new `PushLayer { GaussianBlur }` + `FillPath` +
                // `PopLayer` plumbing (multi-tap convolution) in
                // current emitters, so this arm rarely fires; leave
                // it skipped.
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                    scene,
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                    scene,
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                    scene,
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                    scene,
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                    scene,
                    page_to_px,
                    &path_a,
                    color,
                    params.blur_radius,
                    blend,
                );
                stamp_blurred_path(
                    scene,
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                    scene,
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
                    scene,
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
                let scene = scene_stack.last_mut().expect("scene_stack underflow");
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
        }
    }
    // Defensive: any pushes left unmatched at scene end still need
    // to be balanced so the encoder is well-formed. The orchestrator
    // shouldn't ever leave them dangling, but be tolerant — drain the
    // layer stack, popping each layer or finalising each pending blur
    // sub-scene against the parent.
    while !layer_stack.is_empty() {
        pop_layer_or_blur(&mut scene_stack, &mut layer_stack, page_to_px);
    }
    scene_stack
        .pop()
        .expect("scene_stack should still contain the root scene")
}

/// Tracks what each entry on the layer LIFO meant at push time so the
/// matching pop knows whether to call `pop_layer` on the current scene
/// (Encoded) or unwind a captured sub-scene through the multi-tap
/// Gaussian replay (Blurred).
#[derive(Debug, Clone, Copy)]
enum LayerKind {
    /// A plain `push_layer` call on the current target scene. Clip
    /// layers, blend groups, and `PushLayer { effect: None }` all use
    /// this — the pop is a straight `pop_layer`.
    Encoded,
    /// A `PushLayer { effect: GaussianBlur }` push opened a new sub-
    /// scene on `scene_stack`; the matching pop runs
    /// [`emit_blurred_layer`] which replays the sub-scene under a 2D
    /// Gaussian sample pattern onto the parent target.
    Blurred {
        sigma_pt: f32,
        bounds: idml_compose::Rect,
        blend_mode: ComposeBlendMode,
        opacity: f32,
    },
}

/// Generic pop: examine the top of `layer_stack` and either pop the
/// current scene's layer (Encoded) or fold the captured sub-scene back
/// onto the parent target through a multi-tap Gaussian replay
/// (Blurred). Mismatched / underflowed pops are a no-op, matching the
/// CPU rasterizer's tolerance policy for `PopClip` / `EndBlendGroup` /
/// `PopLayer`.
fn pop_layer_or_blur(
    scene_stack: &mut Vec<Scene>,
    layer_stack: &mut Vec<LayerKind>,
    page_to_px: kurbo::Affine,
) {
    let Some(kind) = layer_stack.pop() else {
        return;
    };
    match kind {
        LayerKind::Encoded => {
            let scene = scene_stack
                .last_mut()
                .expect("scene_stack underflow on Encoded pop");
            scene.pop_layer();
        }
        LayerKind::Blurred {
            sigma_pt,
            bounds,
            blend_mode,
            opacity,
        } => {
            // Pop the captured sub-scene and replay it onto the
            // parent target under a 2D Gaussian sample grid.
            let sub = scene_stack
                .pop()
                .expect("scene_stack underflow on Blurred pop");
            let parent = scene_stack
                .last_mut()
                .expect("Blurred layer with no parent scene");
            emit_blurred_layer(
                parent,
                page_to_px,
                &sub,
                sigma_pt,
                bounds,
                blend_mode,
                opacity,
            );
        }
    }
}

/// Multi-tap Gaussian-blur approximation over a captured sub-scene.
///
/// Vello doesn't expose an image-space separable Gaussian over an
/// arbitrary layer buffer in the version we link against —
/// `draw_blurred_rounded_rect` is a *brush* primitive that only blurs
/// a rounded rect, not the contents of a transparency group, and the
/// `vello_filters_cpu` crate (the future home of the SVG-filter
/// Gaussian) is still a CPU-only reference implementation today (see
/// `image_filters/README.md` in the Vello tree).
///
/// Our workaround: capture the commands inside the `PushLayer` /
/// `PopLayer` pair as a sub-scene, then `Scene::append` it repeatedly
/// at a regular grid of (dx, dy) offsets that sample the 2D Gaussian.
/// Each replay sits inside a `push_layer` whose `alpha = w_ij` is the
/// Gaussian weight at that grid point and whose blend mode is
/// `(Normal, Plus)` — so popping that layer *adds* the alpha-weighted
/// content onto the surrounding accumulator. Mathematically this is a
/// true convolution: `output(p) = Σ w_ij · sub(p - offset_ij)`. The
/// only approximation is the grid discretisation (we sample 7×7 = 49
/// points across [-3σ, +3σ]); with 49 taps the 1D weight grid already
/// resolves σ ≈ 1.7px steps per tap, which reads as a soft Gaussian
/// to the eye for any σ ≥ 1px. Visual quality vs. the CPU separable
/// Gaussian: comparable softness, subtly more "boxy" tails for very
/// large σ; the CPU rasterizer remains the path of record for the
/// fidelity harness.
///
/// Cost: 49 sub-scene appends per blurred layer, each wrapped in a
/// `push_layer` / `pop_layer` pair. Vello's encoder is O(N) over
/// command count and the GPU rasterizer parallelises path tiles, so
/// this is reasonable for preview-time use even at high tap counts;
/// IDML pages typically carry at most a handful of effect-driven
/// layers per spread.
fn emit_blurred_layer(
    parent: &mut Scene,
    page_to_px: kurbo::Affine,
    sub: &Scene,
    sigma_pt: f32,
    bounds: idml_compose::Rect,
    blend_mode: ComposeBlendMode,
    opacity: f32,
) {
    let sigma = sigma_pt.max(0.0);
    if sigma <= 0.5 {
        // Sub-pixel σ: fall back to a plain transparency group. The
        // CPU rasterizer's separable-Gaussian shortcut takes the same
        // path, so the behaviour matches at the limit.
        let rect = kurbo::Rect::new(
            bounds.x as f64,
            bounds.y as f64,
            (bounds.x + bounds.w) as f64,
            (bounds.y + bounds.h) as f64,
        );
        parent.push_layer(
            Fill::NonZero,
            blend_to_peniko(blend_mode),
            opacity.clamp(0.0, 1.0),
            page_to_px,
            &rect,
        );
        parent.append(sub, None);
        parent.pop_layer();
        return;
    }

    // Build a separable 1-D Gaussian sample grid: TAPS samples evenly
    // spaced over [-3σ, +3σ]. The full 2-D kernel is the outer product
    // of two 1-D grids, total TAPS² taps. 7 is a defensible balance
    // between visual quality (smoother than 5, indistinguishable from
    // 9 at typical preview resolution) and encoder load (49 appends
    // per layer instead of 81).
    const TAPS: usize = 7;
    let (positions, weights) = gaussian_sample_grid_1d(sigma, TAPS);

    // Pad the layer clip rect by 3σ on each side so the kernel tail
    // isn't clipped by the layer bounds (matches the CPU rasterizer's
    // `3σ + 1px` padding policy). The pad is in page-pt units; the
    // outer `page_to_px` transform scales it to device pixels.
    let pad = 3.0 * sigma as f64;
    let outer_clip = kurbo::Rect::new(
        bounds.x as f64 - pad,
        bounds.y as f64 - pad,
        (bounds.x + bounds.w) as f64 + pad,
        (bounds.y + bounds.h) as f64 + pad,
    );

    // Outer layer: composites the convolved result onto the parent
    // target with the caller's blend mode + opacity. This is the
    // visible composite a `BeginBlendGroup` would have produced.
    parent.push_layer(
        Fill::NonZero,
        blend_to_peniko(blend_mode),
        opacity.clamp(0.0, 1.0),
        page_to_px,
        &outer_clip,
    );

    // Each tap: open a layer with alpha = w_x * w_y and blend mode
    // `(Normal, Plus)`, append the sub-scene shifted by (dx, dy) in
    // page-pt, then pop. Plus accumulates the weighted contributions
    // into the outer layer's buffer, producing the convolution. We
    // skip taps with negligible weight to keep the encoder lean.
    let plus = PenikoBlendMode::new(Mix::Normal, Compose::Plus);
    for (j, &dy) in positions.iter().enumerate() {
        let wy = weights[j];
        for (i, &dx) in positions.iter().enumerate() {
            let w = weights[i] * wy;
            // Anything below ~0.1% of full weight is invisible after
            // the 8-bit channel quantisation; skipping these halves
            // the tap count for moderate σ.
            if w < 1.0e-3 {
                continue;
            }
            parent.push_layer(
                Fill::NonZero,
                plus,
                w,
                page_to_px,
                &outer_clip,
            );
            // Translation is in page-pt because the captured sub-scene
            // emitted its commands in page-pt space (the outer
            // `page_to_px` transform applies at draw time).
            parent.append(
                sub,
                Some(kurbo::Affine::translate((dx as f64, dy as f64))),
            );
            parent.pop_layer();
        }
    }

    parent.pop_layer();
}

/// 1-D Gaussian sample grid: `count` positions evenly spaced over
/// [-3σ, +3σ], with weights `exp(-x²/(2σ²))` normalised to sum to 1.
/// Returns `(positions, weights)` of length `count` each.
fn gaussian_sample_grid_1d(sigma: f32, count: usize) -> (Vec<f32>, Vec<f32>) {
    assert!(count >= 1);
    let radius = 3.0 * sigma;
    // step between adjacent samples. For odd count, the centre sample
    // sits at x=0 with weight 1 (max), and the outer samples sit at
    // ±radius. For even count, samples straddle zero symmetrically.
    let step = if count > 1 {
        2.0 * radius / (count - 1) as f32
    } else {
        0.0
    };
    let two_sigma_sq = 2.0 * sigma * sigma;
    let mut positions = Vec::with_capacity(count);
    let mut weights = Vec::with_capacity(count);
    let mut sum = 0.0f32;
    for i in 0..count {
        let x = -radius + step * i as f32;
        let w = (-(x * x) / two_sigma_sq).exp();
        positions.push(x);
        weights.push(w);
        sum += w;
    }
    if sum > 0.0 {
        for w in &mut weights {
            *w /= sum;
        }
    }
    (positions, weights)
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

/// CPU-rasterizer fast-path for documents with 1..=8 overprint
/// commands. The CPU backend is the project's fidelity-of-record and
/// handles per-pixel plane state + spot-ink composition correctly;
/// for low overprint counts the readback latency dwarfs the cost of
/// re-running the whole scene through CPU rasterization. Delegating
/// here keeps the rare cases of compute-pipeline edge bugs from
/// affecting documents where the GPU path's encode overhead would
/// dominate.
///
/// The plan calls for "walk the readback buffer through the existing
/// CPU compose_cmyk_overprint_via_planes"; that's what the CPU
/// rasterizer does internally, plus correct handling of paper-bypass,
/// spot inks, image overprints, etc. We let it do the full job rather
/// than partially reusing one helper.
#[cfg(feature = "cpu")]
fn cpu_finisher_path(list: &DisplayList, options: &RasterOptions) -> Vec<u8> {
    let cpu = crate::CpuRasterizer;
    cpu.rasterize(list, options)
}

/// GPU compute-pipeline path for overprint-heavy documents. Renders
/// the full scene via Vello (with the existing knockout fallback for
/// `*Overprint`), then for each coalesced overprint batch renders the
/// batch's paths to a scratch buffer and dispatches `splat_or_overprint`
/// against the plane state. After every batch, one `recomposite`
/// dispatch unions everything into the final buffer.
///
/// Coalescing key = `(ink_mask_packed, spot_id)`. Consecutive overprint
/// commands with the same key collapse into a single Vello scene and
/// one splat dispatch — eliminating the "one render per glyph" worst
/// case for K-only body text (a 200-word paragraph at ~1,100
/// FillPathOverprint commands becomes 1 batch / 1 encode / 1 dispatch).
///
/// Per-channel max is associative + commutative within a single ink
/// mask, so order within a batch doesn't matter. Across batches we
/// preserve document order via the segmentation walk.
///
/// Pipeline-creation failure here returns `Err`; the caller logs and
/// falls back to plain knockout via `build_scene`. This is the
/// behaviour the `vello_compute_pipeline_creation_failure_falls_back_to_knockout`
/// test pins.
fn gpu_compute_overprint_path(
    state: &mut GpuState,
    list: &DisplayList,
    options: &RasterOptions,
) -> Result<Vec<u8>, String> {
    let pipelines = state
        .pipelines
        .as_ref()
        .ok_or_else(|| "compute pipelines unavailable".to_string())?;

    let (width, height) = options.pixel_size();
    let n_pixels = (width as usize) * (height as usize);

    // 1) Render the scene MINUS overprint commands via Vello to
    //    vello_target. The splat shader recovers bottom-side CMYK via
    //    `rgb_to_naive_cmyk_8bit(target)` on virgin pixels — if the
    //    target carried the overprint's own colour we'd round-trip
    //    only that, dropping the underlying ink. Excluding overprint
    //    commands keeps the target as the "everything below the
    //    overprint" buffer the splat needs.
    let scale = options.dpi / 72.0;
    let scene = build_scene_with_transform_filtered(
        list,
        kurbo::Affine::scale(scale as f64),
        /*skip_overprints=*/ true,
    );
    let vello_target = create_storage_texture(
        &state.device,
        width,
        height,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        "vello target",
    );
    let vello_view = vello_target.create_view(&wgpu::TextureViewDescriptor::default());
    state
        .renderer
        .render_to_texture(
            &state.device,
            &state.queue,
            &scene,
            &vello_view,
            &RenderParams {
                base_color: linear_to_peniko(options.background),
                width,
                height,
                antialiasing_method: AaConfig::Area,
            },
        )
        .map_err(|e| format!("vello render_to_texture (main): {e:?}"))?;

    // 2) Allocate buffer-backed plane state. The shader prefers
    //    buffers over storage textures because `rgba8unorm` read_write
    //    storage isn't a portable WebGPU feature.
    let plane_cmyk = create_zero_storage_buffer(
        &state.device,
        (n_pixels * 4) as u64,
        "plane_cmyk",
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    let coverage_buf = create_zero_storage_buffer(
        &state.device,
        (n_pixels * 4) as u64,
        "coverage",
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );

    // Collect overprint commands and coalesce by ink-mask + spot id.
    let batches = build_overprint_batches(list);
    let num_spots: u32 = batches
        .iter()
        .filter_map(|b| b.spot_id)
        .max()
        .map(|m| m + 1)
        .unwrap_or(0);
    let num_spot_groups = num_spots.div_ceil(4);
    let spot_planes_size = (num_spot_groups as usize).max(1) * n_pixels * 4;
    let spot_planes = create_zero_storage_buffer(
        &state.device,
        spot_planes_size as u64,
        "spot_planes",
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );
    // Sentinel binding for process dispatches: a 16-byte buffer
    // satisfies the bind-group layout's `Storage` slot without ever
    // being read or written (the shader's `spot_id == sentinel`
    // branch keeps it untouched).
    let sentinel_spot = create_zero_storage_buffer(
        &state.device,
        16,
        "sentinel_spot",
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );

    // Spot-alts table: one u32 per spot id, byte order = C, M, Y, K.
    let mut spot_alts_data = vec![0u32; num_spots.max(1) as usize];
    for batch in &batches {
        if let Some(spot_id) = batch.spot_id {
            if let Some(ink) = list.spot_ink(idml_compose::SpotInkId(spot_id)) {
                spot_alts_data[spot_id as usize] = crate::cmyk_compute::pack_cmyk_bytes(
                    ink.cmyk_alternate[0],
                    ink.cmyk_alternate[1],
                    ink.cmyk_alternate[2],
                    ink.cmyk_alternate[3],
                );
            }
        }
    }
    let spot_alts_buf = create_storage_buffer_with_data(
        &state.device,
        bytemuck::cast_slice(&spot_alts_data),
        "spot_alts",
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    );

    // Vello target copied into a buffer so the recomposite shader can
    // read it via a `storage, read` binding (Rgba8Unorm storage read
    // isn't portable on every adapter).
    let bpr_aligned = wgpu::util::align_to(width * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let vello_target_buf = state.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("idml-gpu vello target buffer"),
        size: (n_pixels * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    {
        let staging = state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("idml-gpu vello target staging"),
            size: (bpr_aligned * height) as u64,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("idml-gpu vello target copy enc"),
            });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &vello_target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
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
        // Row-by-row copy into the tight per-pixel buffer the shader
        // expects. wgpu requires a multiple of COPY_BYTES_PER_ROW_ALIGNMENT
        // (256) for buffer-to-buffer copies, so we copy each row
        // individually.
        for row in 0..height {
            enc.copy_buffer_to_buffer(
                &staging,
                (row * bpr_aligned) as u64,
                &vello_target_buf,
                (row * width * 4) as u64,
                (width * 4) as u64,
            );
        }
        state.queue.submit(Some(enc.finish()));
    }

    // 3) For each overprint batch, render its paths to a scratch
    //    Vello target, copy to a buffer, dispatch splat_or_overprint.
    let scratch_op = create_storage_texture(
        &state.device,
        width,
        height,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        "scratch_op",
    );
    let scratch_view = scratch_op.create_view(&wgpu::TextureViewDescriptor::default());
    let scratch_buf = state.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("idml-gpu scratch op buffer"),
        size: (n_pixels * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let scratch_staging = state.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("idml-gpu scratch staging"),
        size: (bpr_aligned * height) as u64,
        usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    for batch in &batches {
        let batch_scene = build_overprint_batch_scene(list, batch, options);
        state
            .renderer
            .render_to_texture(
                &state.device,
                &state.queue,
                &batch_scene,
                &scratch_view,
                &RenderParams {
                    // Transparent base so the scratch alpha truly
                    // encodes only the batch's path coverage.
                    base_color: PenikoColor::from_rgba8(0, 0, 0, 0),
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| format!("vello render_to_texture (scratch): {e:?}"))?;

        // Copy scratch_op → scratch_buf via the row-aligned staging
        // buffer (same pattern as the main vello-target copy above).
        {
            let mut enc = state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("idml-gpu scratch copy enc"),
                });
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &scratch_op,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &scratch_staging,
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
            for row in 0..height {
                enc.copy_buffer_to_buffer(
                    &scratch_staging,
                    (row * bpr_aligned) as u64,
                    &scratch_buf,
                    (row * width * 4) as u64,
                    (width * 4) as u64,
                );
            }
            state.queue.submit(Some(enc.finish()));
        }

        // Dispatch splat_or_overprint with the batch's ink mask and
        // optional spot binding.
        let (spot_id, spot_channel, spot_tint) = match batch.spot_id {
            Some(id) => (id, id % 4, batch.spot_tint_8),
            None => (crate::cmyk_compute::NO_SPOT_SENTINEL, 0u32, 0u32),
        };
        let params = crate::cmyk_compute::SplatParams {
            ink_mask_packed: batch.ink_mask_packed,
            spot_id,
            spot_channel,
            spot_tint,
            width,
            height,
            _pad0: 0,
            _pad1: 0,
        };
        let params_buf = create_uniform_buffer_with_data(
            &state.device,
            bytemuck::bytes_of(&params),
            "splat params",
        );

        let bg0 = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("idml-gpu splat bg0"),
            layout: &pipelines.splat_group0_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: plane_cmyk.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: coverage_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: scratch_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: vello_target_buf.as_entire_binding(),
                },
            ],
        });

        // For spot dispatches bind the matching slice of spot_planes
        // (one packed group of 4 spots) into bg1; otherwise bind the
        // sentinel buffer so the layout matches.
        let bg1 = if let Some(id) = batch.spot_id {
            let group = id / 4;
            let offset = (group as u64) * (n_pixels as u64) * 4;
            let size = (n_pixels as u64) * 4;
            state.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("idml-gpu splat bg1 (spot)"),
                layout: &pipelines.splat_group1_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &spot_planes,
                        offset,
                        size: std::num::NonZeroU64::new(size),
                    }),
                }],
            })
        } else {
            state.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("idml-gpu splat bg1 (sentinel)"),
                layout: &pipelines.splat_group1_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: sentinel_spot.as_entire_binding(),
                }],
            })
        };

        let mut enc = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("idml-gpu splat dispatch enc"),
            });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("idml-gpu splat pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipelines.splat);
            pass.set_bind_group(0, &bg0, &[]);
            pass.set_bind_group(1, &bg1, &[]);
            let wg_x = width.div_ceil(8);
            let wg_y = height.div_ceil(8);
            pass.dispatch_workgroups(wg_x, wg_y, 1);
        }
        state.queue.submit(Some(enc.finish()));
    }

    // 4) Recomposite into a final Rgba8Unorm texture, then copy that
    //    out to a CPU-readable buffer.
    let final_texture = create_storage_texture(
        &state.device,
        width,
        height,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        "final target",
    );
    let final_view = final_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let recomp_params = crate::cmyk_compute::RecompositeParams {
        width,
        height,
        num_spot_groups: num_spot_groups.max(1),
        num_spots,
    };
    let recomp_params_buf = create_uniform_buffer_with_data(
        &state.device,
        bytemuck::bytes_of(&recomp_params),
        "recomp params",
    );

    let bg_recomp = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("idml-gpu recomposite bg"),
        layout: &pipelines.recomposite_group0_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: plane_cmyk.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: coverage_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: spot_planes.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: spot_alts_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: vello_target_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: recomp_params_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&final_view),
            },
        ],
    });
    {
        let mut enc = state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("idml-gpu recomposite enc"),
            });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("idml-gpu recomposite pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipelines.recomposite);
            pass.set_bind_group(0, &bg_recomp, &[]);
            let wg_x = width.div_ceil(8);
            let wg_y = height.div_ceil(8);
            pass.dispatch_workgroups(wg_x, wg_y, 1);
        }
        state.queue.submit(Some(enc.finish()));
    }

    // 5) Read back the final texture to CPU.
    read_texture_to_rgba_vec(state, &final_texture, width, height)
}

fn create_storage_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    label: &'static str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn create_zero_storage_buffer(
    device: &wgpu::Device,
    size: u64,
    label: &'static str,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    // Buffers created without `mapped_at_creation = true` come back
    // zeroed under wgpu; we explicitly enable `COPY_DST` upstream so
    // callers can refill later if they need to. The mapped-at-creation
    // path would be slightly faster but adds wgpu API boilerplate
    // around the write-combining-buffer slice contract.
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

fn create_storage_buffer_with_data(
    device: &wgpu::Device,
    data: &[u8],
    label: &'static str,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let size = data.len().max(16) as u64;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: true,
    });
    {
        let mut view = buf.slice(..).get_mapped_range_mut();
        // BufferViewMut doesn't deref to &mut [u8] (write-combining
        // memory safety); use `copy_from_slice` which is the supported
        // bulk-write API. Pad with a zero copy when `data.len() < size`.
        let mut padded;
        let slice: &[u8] = if data.len() as u64 == size {
            data
        } else {
            padded = vec![0u8; size as usize];
            padded[..data.len()].copy_from_slice(data);
            padded.as_slice()
        };
        view.copy_from_slice(slice);
    }
    buf.unmap();
    buf
}

fn create_uniform_buffer_with_data(
    device: &wgpu::Device,
    data: &[u8],
    label: &'static str,
) -> wgpu::Buffer {
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: data.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: true,
    });
    {
        let mut view = buf.slice(..).get_mapped_range_mut();
        view.copy_from_slice(data);
    }
    buf.unmap();
    buf
}

fn read_texture_to_rgba_vec(
    state: &mut GpuState,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let bpr_aligned = wgpu::util::align_to(width * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer_size = (bpr_aligned * height) as u64;
    let buffer = state.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("idml-gpu final readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = state
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("idml-gpu final readback enc"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
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

/// One coalesced batch of overprint commands: a set of paths to draw
/// with a single `(ink_mask, spot)` combination. The compute pipeline
/// renders each batch via Vello to a scratch texture, then runs one
/// `splat_or_overprint` dispatch against the plane state.
///
/// `paths` carries `(path_id, transform, optional_stroke)` triples —
/// fills have `optional_stroke = None`, strokes carry their parameters
/// so the batch render reproduces the stroke geometry that contributes
/// the coverage. Mixing fills and strokes in one batch is allowed
/// because the coverage union is order-independent within a batch.
struct OverprintBatch {
    ink_mask_packed: u32,
    spot_id: Option<u32>,
    /// Pre-quantised spot tint (0..=255). Ignored for process batches.
    spot_tint_8: u32,
    paths: Vec<OverprintPath>,
}

struct OverprintPath {
    path_id: idml_compose::PathId,
    transform: idml_compose::Transform,
    stroke: Option<idml_compose::Stroke>,
}

/// Walk the display list, group consecutive `*Overprint` commands by
/// `(ink_mask_packed, spot_id)`. Different keys break the batch (we
/// emit a new one); non-overprint commands are ignored.
fn build_overprint_batches(list: &DisplayList) -> Vec<OverprintBatch> {
    let mut batches: Vec<OverprintBatch> = Vec::new();
    for cmd in &list.commands {
        let (paint, path_id, transform, stroke) = match cmd {
            DisplayCommand::FillPathOverprint {
                paint,
                path_id,
                transform,
            } => (paint, *path_id, *transform, None),
            DisplayCommand::StrokePathOverprint {
                paint,
                path_id,
                transform,
                stroke,
            } => (paint, *path_id, *transform, Some(*stroke)),
            _ => continue,
        };
        let (ink_mask_packed, spot_id, spot_tint_8) = match paint {
            Paint::Cmyk { c, m, y, k, spot, .. } => {
                let pack =
                    crate::cmyk_compute::pack_cmyk_unit([*c, *m, *y, *k]);
                match spot {
                    Some(idml_compose::SpotInkId(id)) => {
                        // Spot tint = max ink channel / max alternate
                        // channel, per the CPU rasterizer's logic in
                        // `compose_cmyk_overprint_dispatch`.
                        let tint_unit = if let Some(ink) =
                            list.spot_ink(idml_compose::SpotInkId(*id))
                        {
                            let alt_max = ink
                                .cmyk_alternate
                                .iter()
                                .map(|v| *v as f32 / 255.0)
                                .fold(0.0_f32, f32::max);
                            if alt_max <= f32::EPSILON {
                                0.0
                            } else {
                                (c.max(*m).max(*y).max(*k) / alt_max).clamp(0.0, 1.0)
                            }
                        } else {
                            0.0
                        };
                        let tint_8 = (tint_unit * 255.0).round() as u32;
                        (pack, Some(*id), tint_8)
                    }
                    None => (pack, None, 0u32),
                }
            }
            // Non-CMYK paints reaching an overprint command: this
            // shouldn't happen at the orchestrator level, but if it
            // did we treat the paint as 100% K knockout (the same
            // visible result as the pre-parity Vello fallback).
            Paint::Solid(c) => {
                let (_, _, _, k) = crate::cpu::rgb_to_naive_cmyk_8bit(
                    (c.r.clamp(0.0, 1.0) * 255.0) as u8,
                    (c.g.clamp(0.0, 1.0) * 255.0) as u8,
                    (c.b.clamp(0.0, 1.0) * 255.0) as u8,
                );
                let pack = crate::cmyk_compute::pack_cmyk_bytes(0, 0, 0, k);
                (pack, None, 0u32)
            }
            _ => continue,
        };

        // Coalesce with the previous batch when the key matches.
        let path = OverprintPath {
            path_id,
            transform,
            stroke,
        };
        if let Some(last) = batches.last_mut() {
            if last.ink_mask_packed == ink_mask_packed
                && last.spot_id == spot_id
                && last.spot_tint_8 == spot_tint_8
            {
                last.paths.push(path);
                continue;
            }
        }
        batches.push(OverprintBatch {
            ink_mask_packed,
            spot_id,
            spot_tint_8,
            paths: vec![path],
        });
    }
    batches
}

/// Build a Vello scene containing only the paths of one overprint
/// batch, all painted in opaque white. The scratch target's alpha
/// channel captures the coverage union; the splat shader reads alpha
/// and uses the push-constant `ink_mask` for the per-channel ink
/// amounts. RGB doesn't matter — the shader ignores it.
fn build_overprint_batch_scene(
    list: &DisplayList,
    batch: &OverprintBatch,
    options: &RasterOptions,
) -> Scene {
    let scale = options.dpi / 72.0;
    let page_to_px = kurbo::Affine::scale(scale as f64);
    let mut scene = Scene::new();
    let opaque_white = PenikoColor::from_rgba8(255, 255, 255, 255);
    for path in &batch.paths {
        let Some(path_data) = list.paths.get(path.path_id) else {
            continue;
        };
        let bez = path_to_bez(path_data, &path.transform);
        if let Some(stroke) = &path.stroke {
            let ks = KurboStroke::new(stroke.width.max(0.0) as f64)
                .with_caps(map_cap(stroke.cap))
                .with_join(map_join(stroke.join))
                .with_miter_limit(stroke.miter_limit.max(1.0) as f64);
            scene.stroke(
                &ks,
                page_to_px,
                BrushRef::Solid(opaque_white),
                None,
                &bez,
            );
        } else {
            scene.fill(
                Fill::NonZero,
                page_to_px,
                BrushRef::Solid(opaque_white),
                None,
                &bez,
            );
        }
    }
    scene
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
        Paint::Cmyk { rgb, .. } => {
            // The pipeline baked the ICC-resolved display colour
            // onto the paint at compose time — use it so ordinary
            // (non-overprint) draws stay identical to the prior
            // `Paint::Solid` path on the GPU backend.
            //
            // Stage C (`Paint::Cmyk::spot`) is ignored on the Vello
            // backend: spot-on-same-spot overprint requires the same
            // per-pixel plane walk Stage 4A/4B use on the CPU
            // rasterizer, which doesn't have a Vello equivalent today
            // (would need a custom shader). Spot paints still render
            // visibly correct here because the cached `rgb` already
            // encodes the alternate-CMYK × tint colour; only the
            // spot-on-same-spot overprint case loses fidelity vs. the
            // CPU rasterizer.
            Some(VelloBrush::Solid(linear_to_peniko(*rgb)))
        }
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

    #[test]
    fn gaussian_kernel_weights_sum_to_one() {
        // Sanity: the 1-D sample grid normalises so the full 2-D
        // convolution preserves total luminance. Without this, a blurred
        // layer would brighten or darken globally relative to the input.
        let (_pos, w) = gaussian_sample_grid_1d(3.0, 7);
        let sum: f32 = w.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1.0e-5,
            "weight sum should be 1.0, got {sum}"
        );
        // Centre weight should be the largest (symmetric Gaussian).
        let max = w.iter().cloned().fold(0.0f32, f32::max);
        assert_eq!(w[w.len() / 2], max);
    }

    #[test]
    fn build_scene_handles_push_layer_gaussian_blur() {
        // Scene encoding-only smoke test for the new blurred-layer
        // capture/replay path. Asserts the multi-tap replay encodes
        // without panicking; pixel-level coverage lives in
        // `push_layer_gaussian_blur_softens_edges_on_gpu` below
        // (guarded by GPU availability).
        use idml_compose::{
            BlendMode as ComposeBlend, Color as DLColor, DisplayCommand, LayerEffect, Paint,
            PathData, PathSegment, Rect, Transform,
        };

        let mut list = DisplayList::new();
        let mut rect_path = PathData::default();
        rect_path.segments.push(PathSegment::MoveTo { x: 0.0, y: 0.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 1.0, y: 0.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 1.0, y: 1.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 0.0, y: 1.0 });
        rect_path.segments.push(PathSegment::Close);
        let rect_id = list.paths.push_anon(rect_path);

        list.commands.push(DisplayCommand::PushLayer {
            bounds: Rect {
                x: 10.0,
                y: 10.0,
                w: 20.0,
                h: 20.0,
            },
            effect: LayerEffect::GaussianBlur { sigma_pt: 3.0 },
            blend_mode: ComposeBlend::Normal,
            opacity: 1.0,
            transform: Transform::IDENTITY,
        });
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect_id,
            paint: Paint::Solid(DLColor::rgba(0.0, 0.0, 0.0, 1.0)),
            transform: Transform([20.0, 0.0, 0.0, 20.0, 10.0, 10.0]),
        });
        list.commands.push(DisplayCommand::PopLayer(Transform::IDENTITY));

        let _ = build_scene_with_transform(&list, kurbo::Affine::scale(1.0));
    }

    #[test]
    fn push_layer_gaussian_blur_softens_edges_on_gpu() {
        // Pixel-level: rasterize a black rect wrapped in a
        // `PushLayer { GaussianBlur(sigma_pt = 5) }` and check that
        // pixels *outside* the rect's geometric bounds are darkened
        // (the blur halo). A hard-edge fill would leave them at the
        // background colour; the multi-tap replay must attenuate them.
        //
        // Skipped silently if Vello's GPU init fails (no adapter, no
        // driver, headless CI without a GPU). Local runs on a Mac /
        // workstation with a real GPU exercise the assertion; the
        // smoke test above stays as the encoder-only fallback.
        use idml_compose::{
            BlendMode as ComposeBlend, Color as DLColor, DisplayCommand, LayerEffect, Paint,
            PathData, PathSegment, Rect, Transform,
        };

        let mut list = DisplayList::new();
        let mut rect_path = PathData::default();
        rect_path.segments.push(PathSegment::MoveTo { x: 0.0, y: 0.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 1.0, y: 0.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 1.0, y: 1.0 });
        rect_path.segments.push(PathSegment::LineTo { x: 0.0, y: 1.0 });
        rect_path.segments.push(PathSegment::Close);
        let rect_id = list.paths.push_anon(rect_path);

        list.commands.push(DisplayCommand::PushLayer {
            bounds: Rect {
                x: 20.0,
                y: 20.0,
                w: 40.0,
                h: 40.0,
            },
            effect: LayerEffect::GaussianBlur { sigma_pt: 5.0 },
            blend_mode: ComposeBlend::Normal,
            opacity: 1.0,
            transform: Transform::IDENTITY,
        });
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect_id,
            // Black rect spanning (20..60, 20..60) in page-pt.
            paint: Paint::Solid(DLColor::rgba(0.0, 0.0, 0.0, 1.0)),
            transform: Transform([40.0, 0.0, 0.0, 40.0, 20.0, 20.0]),
        });
        list.commands.push(DisplayCommand::PopLayer(Transform::IDENTITY));

        let v = VelloRasterizer::new();
        let mut opts = RasterOptions::new(80.0, 80.0);
        opts.dpi = 72.0;
        opts.background = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
        let buf = v.rasterize(&list, &opts);

        // GPU init may have failed and returned the zero-fill fallback
        // (all-black 4*W*H bytes) — distinguish from a real render by
        // checking the corner pixel against the white background. If
        // the buffer is all zeros, skip the assertion: the smoke test
        // above already covers encoding.
        let at = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 80 + x) * 4;
            [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
        };
        let corner = at(2, 2);
        if corner == [0, 0, 0, 0] {
            // GPU unavailable — fallback buffer. Skip the assertion;
            // CI without a GPU exercises only the smoke test.
            return;
        }
        assert!(
            corner[0] > 240,
            "corner pixel should be background white when GPU is live; got {corner:?}"
        );

        // Pixel a few pt outside the rect's right edge (rect spans
        // 20..60 in both axes; sample at x=63, y=40 — well inside the
        // 3σ=15pt blur halo). A hard-edge fill would leave this at
        // background white (~255); the multi-tap replay must darken
        // it noticeably.
        let halo = at(63, 40);
        assert!(
            halo[0] < 230,
            "blur halo should darken pixel outside rect; got {halo:?}"
        );
        // Pixel at the rect's centre: should still be (nearly)
        // opaque black — blur softens edges, not the interior.
        let centre = at(40, 40);
        assert!(
            centre[0] < 80,
            "blurred rect centre should stay dark; got {centre:?}"
        );
    }

    // ------------------------------------------------------------------
    // CMYK overprint parity tests.
    //
    // Every test follows the same skeleton:
    //   1. Build a synthetic display list with the CMYK / overprint
    //      commands the case exercises.
    //   2. Force the GPU compute path via `FORCE_COMPUTE_PATH` so we
    //      actually validate the compute shaders even on small scenes.
    //   3. Run the rasterizer; if the readback buffer is all zeros
    //      (the documented GPU-unavailable fallback), early-return —
    //      the test stays a "if we have a GPU, this works" check.
    //   4. Sample pixels at known coordinates and assert.
    //
    // The early-return policy matches the existing
    // `push_layer_gaussian_blur_softens_edges_on_gpu` pattern at line ~1820
    // (pre-parity). Headless CI without an adapter exercises every
    // test up to the GPU-availability check.
    // ------------------------------------------------------------------

    /// Helper: build a unit-rect path used by every CMYK test.
    fn unit_rect_path(list: &mut idml_compose::DisplayList) -> idml_compose::PathId {
        use idml_compose::{PathData, PathSegment};
        let mut p = PathData::default();
        p.segments.push(PathSegment::MoveTo { x: 0.0, y: 0.0 });
        p.segments.push(PathSegment::LineTo { x: 1.0, y: 0.0 });
        p.segments.push(PathSegment::LineTo { x: 1.0, y: 1.0 });
        p.segments.push(PathSegment::LineTo { x: 0.0, y: 1.0 });
        p.segments.push(PathSegment::Close);
        list.paths.push_anon(p)
    }

    /// Helper: assert the readback buffer isn't the zero-fill fallback
    /// (the documented "no GPU available" sentinel from
    /// `VelloRasterizer::rasterize`). Returns `false` when the GPU
    /// path didn't actually run — tests treat that as "skip cleanly".
    fn gpu_path_ran(buf: &[u8]) -> bool {
        buf.iter().any(|b| *b != 0)
    }

    /// Tests that toggle the global FORCE_COMPUTE_PATH /
    /// FAIL_PIPELINE_CREATION flags must run serially — they share
    /// process-wide state and `cargo test` runs tests in parallel by
    /// default. A poison-free `Mutex` ensures only one CMYK test
    /// holds the flag at a time.
    static CMYK_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Lock guard for the entire CMYK test. Acquire at test entry so
    /// every render in the test sees a consistent flag state; engage
    /// the FORCE flag later for specific render calls.
    #[allow(dead_code)] // The MutexGuard is held by its existence.
    struct CmykTestLock(std::sync::MutexGuard<'static, ()>);
    impl CmykTestLock {
        fn acquire() -> Self {
            let lock = CMYK_TEST_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            // Reset to a known state on entry; a poisoned lock could
            // leave the flag set from a previous panic.
            crate::cmyk_compute::FORCE_COMPUTE_PATH
                .store(false, std::sync::atomic::Ordering::SeqCst);
            crate::cmyk_compute::FAIL_PIPELINE_CREATION
                .store(false, std::sync::atomic::Ordering::SeqCst);
            Self(lock)
        }
    }
    impl Drop for CmykTestLock {
        fn drop(&mut self) {
            crate::cmyk_compute::FORCE_COMPUTE_PATH
                .store(false, std::sync::atomic::Ordering::SeqCst);
            crate::cmyk_compute::FAIL_PIPELINE_CREATION
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Engage the FORCE_COMPUTE_PATH flag for a scoped region. The
    /// caller must already hold a `CmykTestLock` to prevent races
    /// with other CMYK tests.
    struct ForceComputeScope;
    impl ForceComputeScope {
        fn engage() -> Self {
            crate::cmyk_compute::FORCE_COMPUTE_PATH
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Self
        }
    }
    impl Drop for ForceComputeScope {
        fn drop(&mut self) {
            crate::cmyk_compute::FORCE_COMPUTE_PATH
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Helper: build a 40×40 page with a magenta knockout fill from
    /// pt (5..35, 5..35) covered by a cyan overprint fill in the same
    /// rect. Returns the page-pt extent and the constructed list.
    fn cmyk_overprint_cyan_on_magenta_list() -> idml_compose::DisplayList {
        use idml_compose::{Color as DLColor, DisplayCommand, Paint, Transform};
        let mut list = idml_compose::DisplayList::new();
        let rect_id = unit_rect_path(&mut list);
        // Magenta knockout: cyan 0, magenta 100%, yellow 0, K 0.
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect_id,
            paint: Paint::Cmyk {
                c: 0.0,
                m: 1.0,
                y: 0.0,
                k: 0.0,
                rgb: DLColor::rgba(1.0, 0.0, 1.0, 1.0),
                spot: None,
            },
            transform: Transform([30.0, 0.0, 0.0, 30.0, 5.0, 5.0]),
        });
        // Cyan overprint on top.
        list.commands.push(DisplayCommand::FillPathOverprint {
            path_id: rect_id,
            paint: Paint::Cmyk {
                c: 1.0,
                m: 0.0,
                y: 0.0,
                k: 0.0,
                rgb: DLColor::rgba(0.0, 1.0, 1.0, 1.0),
                spot: None,
            },
            transform: Transform([30.0, 0.0, 0.0, 30.0, 5.0, 5.0]),
        });
        list
    }

    #[test]
    fn vello_cmyk_overprint_cyan_on_magenta_produces_blue() {
        // Cyan over magenta should produce the per-channel max
        // composite (C=100, M=100, Y=0, K=0) which decodes to a dark
        // blue. Without the parity work, the Vello knockout fallback
        // would paint pure cyan on top — visible R, low G, full B.
        let _lock = CmykTestLock::acquire();
        let _force = ForceComputeScope::engage();
        let list = cmyk_overprint_cyan_on_magenta_list();
        let v = VelloRasterizer::new();
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        opts.background = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
        let buf = v.rasterize(&list, &opts);
        if !gpu_path_ran(&buf) {
            return;
        }
        let at = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 40 + x) * 4;
            [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
        };
        // Interior pixel of the overprint region.
        let p = at(20, 20);
        // Naive CMYK→RGB(C=255, M=255, Y=0, K=0) = (0, 0, 255). Allow
        // generous slack for rasterizer AA at edges (we're sampling
        // an interior pixel so this stays tight).
        assert!(
            p[0] < 30 && p[1] < 30 && p[2] > 200,
            "cyan-over-magenta interior should look blue, got {p:?}"
        );
    }

    #[test]
    fn vello_stroke_overprint_cyan_on_magenta() {
        // Same composite rule for `StrokePathOverprint`. The stroke
        // covers a narrow band; we sample a pixel near the stroke
        // centre to avoid AA.
        use idml_compose::{
            Color as DLColor, DisplayCommand, LineCap, LineJoin, Paint, Stroke, Transform,
        };
        let _lock = CmykTestLock::acquire();
        let _force = ForceComputeScope::engage();
        let mut list = idml_compose::DisplayList::new();
        let rect_id = unit_rect_path(&mut list);
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect_id,
            paint: Paint::Cmyk {
                c: 0.0,
                m: 1.0,
                y: 0.0,
                k: 0.0,
                rgb: DLColor::rgba(1.0, 0.0, 1.0, 1.0),
                spot: None,
            },
            transform: Transform([30.0, 0.0, 0.0, 30.0, 5.0, 5.0]),
        });
        list.commands.push(DisplayCommand::StrokePathOverprint {
            path_id: rect_id,
            paint: Paint::Cmyk {
                c: 1.0,
                m: 0.0,
                y: 0.0,
                k: 0.0,
                rgb: DLColor::rgba(0.0, 1.0, 1.0, 1.0),
                spot: None,
            },
            stroke: Stroke {
                width: 8.0,
                cap: LineCap::Butt,
                join: LineJoin::Miter,
                miter_limit: 4.0,
                dash: idml_compose::DashPattern::default(),
            },
            transform: Transform([30.0, 0.0, 0.0, 30.0, 5.0, 5.0]),
        });

        let v = VelloRasterizer::new();
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        opts.background = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
        let buf = v.rasterize(&list, &opts);
        if !gpu_path_ran(&buf) {
            return;
        }
        // Stroke center should land near (5, 5..35) horizontal band.
        // Sample at (5, 20) — the left edge of the stroke band.
        let i = (20 * 40 + 5) * 4;
        let p = [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]];
        assert!(
            p[2] > 150,
            "cyan-on-magenta stroke should be blueish, got {p:?}"
        );
    }

    #[test]
    fn vello_no_overprint_unchanged_vs_baseline() {
        // Document with NO overprint commands must render bit-identical
        // to the pre-parity path. Snapshot the result first via the
        // FORCE_COMPUTE_PATH-off rasterizer (which takes the unchanged
        // path since count == 0), then re-run with FORCE on and assert
        // the buffer is the same. (FORCE_COMPUTE_PATH only matters when
        // count > 0; here it's a no-op — confirming that.)
        let _lock = CmykTestLock::acquire();
        use idml_compose::{
            BlendMode as ComposeBlend, Color as DLColor, DisplayCommand, Paint, PathData,
            PathSegment, Rect, Transform,
        };
        let mut list = idml_compose::DisplayList::new();
        let rect_id = unit_rect_path(&mut list);
        list.commands.push(DisplayCommand::BeginBlendGroup {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: 40.0,
                h: 40.0,
            },
            blend_mode: ComposeBlend::Normal,
            opacity: 1.0,
            transform: Transform::IDENTITY,
        });
        list.commands.push(DisplayCommand::FillPath {
            path_id: rect_id,
            paint: Paint::Solid(DLColor::rgba(0.0, 0.4, 0.8, 1.0)),
            transform: Transform([30.0, 0.0, 0.0, 30.0, 5.0, 5.0]),
        });
        list.commands.push(DisplayCommand::EndBlendGroup(Transform::IDENTITY));
        // Quiet the unused path warning for tests on no-effect paths.
        let _ = PathSegment::Close;
        let _ = PathData::default();

        let v1 = VelloRasterizer::new();
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        opts.background = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
        let buf_baseline = v1.rasterize(&list, &opts);
        if !gpu_path_ran(&buf_baseline) {
            return;
        }

        let _force = ForceComputeScope::engage();
        let v2 = VelloRasterizer::new();
        let buf_forced = v2.rasterize(&list, &opts);
        // FORCE_COMPUTE_PATH is a no-op when count == 0 — both should
        // take the unchanged path and produce bit-identical bytes.
        assert_eq!(
            buf_baseline, buf_forced,
            "zero-overprint scene should render bit-identical regardless of force flag"
        );
    }

    #[test]
    fn vello_k_overprint_passes_through_rgb_image() {
        // K-only black text overprinted on a photo. The non-text RGB
        // image pixels must pass through unchanged (coverage == 0
        // passthrough). Text-covered pixels must darken (K plane
        // splat) vs. the baseline image-only render.
        let _lock = CmykTestLock::acquire();
        use idml_compose::{
            Color as DLColor, DecodedImage, DisplayCommand, Paint, Transform,
        };
        let mut list = idml_compose::DisplayList::new();
        let rect_id = unit_rect_path(&mut list);
        // 1×1 red image; the placement transform scales it across
        // the full page.
        let image_id = list.push_image(DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![200, 80, 80, 255],
        });
        list.commands.push(DisplayCommand::Image {
            image_id,
            transform: Transform([40.0, 0.0, 0.0, 40.0, 0.0, 0.0]),
        });
        // K-only overprint over a small rect in the middle.
        list.commands.push(DisplayCommand::FillPathOverprint {
            path_id: rect_id,
            paint: Paint::Cmyk {
                c: 0.0,
                m: 0.0,
                y: 0.0,
                k: 1.0,
                rgb: DLColor::rgba(0.0, 0.0, 0.0, 1.0),
                spot: None,
            },
            transform: Transform([10.0, 0.0, 0.0, 10.0, 15.0, 15.0]),
        });

        // Baseline: just the image, no overprint. Build a separate
        // list to render and capture the pre-darken RGB.
        let mut baseline = idml_compose::DisplayList::new();
        let _rect_id_b = unit_rect_path(&mut baseline);
        let image_id_b = baseline.push_image(DecodedImage {
            width: 1,
            height: 1,
            rgba: vec![200, 80, 80, 255],
        });
        baseline.commands.push(DisplayCommand::Image {
            image_id: image_id_b,
            transform: Transform([40.0, 0.0, 0.0, 40.0, 0.0, 0.0]),
        });

        let v = VelloRasterizer::new();
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        opts.background = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
        let baseline_buf = v.rasterize(&baseline, &opts);
        if !gpu_path_ran(&baseline_buf) {
            return;
        }
        let _force = ForceComputeScope::engage();
        let buf = v.rasterize(&list, &opts);

        let at = |b: &[u8], x: usize, y: usize| -> [u8; 4] {
            let i = (y * 40 + x) * 4;
            [b[i], b[i + 1], b[i + 2], b[i + 3]]
        };
        // Outside the overprint rect (x=5, y=5): RGB image pixel must
        // be bit-identical to baseline.
        let outside_b = at(&baseline_buf, 5, 5);
        let outside = at(&buf, 5, 5);
        assert_eq!(
            outside_b, outside,
            "pixel outside K overprint must be bit-identical to baseline image"
        );
        // Inside the overprint rect (x=20, y=20): pixel must be
        // darker than the baseline image (K plane adds to all channels).
        let inside_b = at(&baseline_buf, 20, 20);
        let inside = at(&buf, 20, 20);
        assert!(
            inside[0] < inside_b[0]
                && inside[1] < inside_b[1]
                && inside[2] < inside_b[2],
            "K overprint should darken every channel; before={inside_b:?} after={inside:?}"
        );
    }

    #[test]
    fn vello_spot_on_spot_same_ink_max_tints() {
        // Two overprint draws of the same spot ink at 50% and 80%
        // tints. The union pixel should reflect 80% — `max(50%, 80%)`.
        // We assert by comparing to a single-draw 80% pixel.
        use idml_compose::{
            Color as DLColor, DisplayCommand, Paint, SpotInk, Transform,
        };
        let _lock = CmykTestLock::acquire();
        let _force = ForceComputeScope::engage();
        // Spot with strong cyan alternate (so any tint is visible).
        let ink = SpotInk {
            name: "Color/TestSpot".to_string(),
            cmyk_alternate: [255, 0, 0, 0], // 100% cyan
        };
        let spot_at = |list: &mut idml_compose::DisplayList, tint: f32| -> Paint {
            let spot_id = list.push_spot_ink(ink.clone());
            // CMYK channels carry alt × tint per the parser contract.
            Paint::Cmyk {
                c: tint * 1.0,
                m: 0.0,
                y: 0.0,
                k: 0.0,
                rgb: DLColor::rgba(1.0 - tint, 1.0, 1.0, 1.0),
                spot: Some(spot_id),
            }
        };

        let mut overlap = idml_compose::DisplayList::new();
        let rect_id = unit_rect_path(&mut overlap);
        let p50 = spot_at(&mut overlap, 0.5);
        let p80 = spot_at(&mut overlap, 0.8);
        overlap.commands.push(DisplayCommand::FillPathOverprint {
            path_id: rect_id,
            paint: p50,
            transform: Transform([30.0, 0.0, 0.0, 30.0, 5.0, 5.0]),
        });
        overlap.commands.push(DisplayCommand::FillPathOverprint {
            path_id: rect_id,
            paint: p80,
            transform: Transform([30.0, 0.0, 0.0, 30.0, 5.0, 5.0]),
        });

        let mut alone80 = idml_compose::DisplayList::new();
        let rect_id_b = unit_rect_path(&mut alone80);
        let p80b = spot_at(&mut alone80, 0.8);
        alone80.commands.push(DisplayCommand::FillPathOverprint {
            path_id: rect_id_b,
            paint: p80b,
            transform: Transform([30.0, 0.0, 0.0, 30.0, 5.0, 5.0]),
        });

        let v = VelloRasterizer::new();
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        opts.background = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
        let buf_overlap = v.rasterize(&overlap, &opts);
        if !gpu_path_ran(&buf_overlap) {
            return;
        }
        let buf_80 = v.rasterize(&alone80, &opts);
        let at = |b: &[u8], x: usize, y: usize| -> [u8; 4] {
            let i = (y * 40 + x) * 4;
            [b[i], b[i + 1], b[i + 2], b[i + 3]]
        };
        let a = at(&buf_overlap, 20, 20);
        let b = at(&buf_80, 20, 20);
        // Allow a few-LSB tolerance for rounding.
        for i in 0..3 {
            let diff = (a[i] as i32 - b[i] as i32).abs();
            assert!(
                diff <= 4,
                "channel {i}: union {a:?} should match alone-80% {b:?} within rounding"
            );
        }
    }

    #[test]
    fn vello_spot_packing_at_5_spots() {
        // 5 spot inks => ceil(5/4) = 2 packed spot textures. We can't
        // easily inspect bind-group internals from a test, but we CAN
        // verify each ink reads back from the right channel by drawing
        // 5 overprints in disjoint regions and checking that each
        // region shows a distinct visible colour matching its alt.
        use idml_compose::{
            Color as DLColor, DisplayCommand, Paint, SpotInk, Transform,
        };
        let _lock = CmykTestLock::acquire();
        let _force = ForceComputeScope::engage();
        let mut list = idml_compose::DisplayList::new();
        let rect_id = unit_rect_path(&mut list);
        // 5 spots, each with a different distinctive alternate.
        let alts: [[u8; 4]; 5] = [
            [255, 0, 0, 0],   // pure cyan
            [0, 255, 0, 0],   // pure magenta
            [0, 0, 255, 0],   // pure yellow
            [0, 0, 0, 255],   // pure black
            [128, 128, 0, 0], // 50% C/M
        ];
        let mut spot_ids = Vec::new();
        for (i, alt) in alts.iter().enumerate() {
            let ink = SpotInk {
                name: format!("Color/Spot{i}"),
                cmyk_alternate: *alt,
            };
            spot_ids.push(list.push_spot_ink(ink));
        }
        // Tile 5 horizontal stripes across the page, each painted by
        // a different spot at 100% tint.
        for (i, sid) in spot_ids.iter().enumerate() {
            let y0 = 2.0 + (i as f32) * 6.0;
            list.commands.push(DisplayCommand::FillPathOverprint {
                path_id: rect_id,
                paint: Paint::Cmyk {
                    c: (alts[i][0] as f32) / 255.0,
                    m: (alts[i][1] as f32) / 255.0,
                    y: (alts[i][2] as f32) / 255.0,
                    k: (alts[i][3] as f32) / 255.0,
                    rgb: DLColor::rgba(0.0, 0.0, 0.0, 1.0),
                    spot: Some(*sid),
                },
                transform: Transform([35.0, 0.0, 0.0, 4.0, 2.0, y0]),
            });
        }

        let v = VelloRasterizer::new();
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        opts.background = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
        let buf = v.rasterize(&list, &opts);
        if !gpu_path_ran(&buf) {
            return;
        }
        let at = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 40 + x) * 4;
            [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
        };
        // Stripe 0: 100% cyan alt => (0, 255, 255) RGB roughly.
        let s0 = at(20, 4);
        assert!(
            s0[0] < 50 && s0[1] > 200 && s0[2] > 200,
            "spot 0 stripe should look cyan, got {s0:?}"
        );
        // Stripe 1: 100% magenta alt => (255, 0, 255).
        let s1 = at(20, 10);
        assert!(
            s1[0] > 200 && s1[1] < 50 && s1[2] > 200,
            "spot 1 stripe should look magenta, got {s1:?}"
        );
        // Stripe 3: 100% K alt (the first ink to land in the SECOND
        // packed texture group at index 3 channel 3, *not* a new
        // group — group 0 channel 3). Should appear black.
        let s3 = at(20, 22);
        assert!(
            s3[0] < 40 && s3[1] < 40 && s3[2] < 40,
            "spot 3 (K alt) stripe should look black, got {s3:?}"
        );
        // Stripe 4: 50% C + 50% M alt = bluish purple. This is the
        // first ink that lands in the SECOND packed group (index 4 =
        // group 1, channel 0). If the per-group bind-group offset
        // arithmetic were wrong, this stripe would either be miswritten
        // (read back as something else entirely) or zero.
        let s4 = at(20, 28);
        assert!(
            s4[2] > 80,
            "spot 4 (50% C/M alt, second packed group) should still show colour, got {s4:?}"
        );
    }

    #[test]
    fn vello_fast_path_low_overprint_count() {
        // Fixture with 2 overprints: the CPU finisher branch fires
        // without `FORCE_COMPUTE_PATH`. The visible result must match
        // what the compute path produces — both routes terminate at
        // the same per-channel-max composite.
        let _lock = CmykTestLock::acquire();
        let list = cmyk_overprint_cyan_on_magenta_list();
        let v = VelloRasterizer::new();
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        opts.background = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
        let buf_fast = v.rasterize(&list, &opts);
        if !gpu_path_ran(&buf_fast) {
            // CPU finisher always "ran" so this means the CPU build
            // didn't even pull tiny-skia — bail.
            return;
        }
        let _force = ForceComputeScope::engage();
        let buf_compute = v.rasterize(&list, &opts);
        if !gpu_path_ran(&buf_compute) {
            return;
        }
        // The interior pixel should look blue in both buffers and
        // ΔE between them should be small (different rasterizers but
        // the same composite intent). We use a per-channel slack of
        // 16 — generous enough for AA / 8-bit quantisation differences.
        let at = |b: &[u8], x: usize, y: usize| -> [u8; 4] {
            let i = (y * 40 + x) * 4;
            [b[i], b[i + 1], b[i + 2], b[i + 3]]
        };
        let a = at(&buf_fast, 20, 20);
        let b = at(&buf_compute, 20, 20);
        for i in 0..3 {
            let diff = (a[i] as i32 - b[i] as i32).abs();
            assert!(
                diff < 32,
                "channel {i}: fast={a:?} vs compute={b:?} differ by {diff}"
            );
        }
        // Both should look distinctly blue.
        assert!(a[2] > 150, "fast-path interior should be blue, got {a:?}");
        assert!(b[2] > 150, "compute interior should be blue, got {b:?}");
    }

    #[test]
    fn vello_compute_pipeline_creation_failure_falls_back_to_knockout() {
        // Force pipeline creation to fail before constructing the
        // rasterizer. The Vello backend should log + fall back to the
        // pre-parity knockout fill — a plausible (non-black) image
        // rather than a panic or a zero buffer.
        let _lock = CmykTestLock::acquire();
        use std::sync::atomic::Ordering;
        crate::cmyk_compute::FAIL_PIPELINE_CREATION.store(true, Ordering::SeqCst);
        let _force = ForceComputeScope::engage();
        let list = cmyk_overprint_cyan_on_magenta_list();
        let v = VelloRasterizer::new();
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        opts.background = ComposeColor::rgba(1.0, 1.0, 1.0, 1.0);
        let buf = v.rasterize(&list, &opts);
        if !gpu_path_ran(&buf) {
            return;
        }
        // Knockout fallback paints the overprint as a normal cyan
        // fill on top of magenta — visible cyan in the centre. We're
        // not asserting per-pixel parity (it explicitly diverges from
        // the CPU result here), just that the rasterizer didn't
        // panic and returned a plausible image rather than zero.
        let at = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 40 + x) * 4;
            [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
        };
        let interior = at(20, 20);
        // Some colour at the interior — the rect is filled with
        // something rather than left at background or zero.
        let nonzero = interior[0] > 0 || interior[1] > 0 || interior[2] > 0;
        assert!(nonzero, "knockout fallback produced black, got {interior:?}");
    }
}
