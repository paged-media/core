//! CPU rasterizer via `tiny-skia`.
//!
//! Takes a `DisplayList` and produces an 8-bit sRGB `RgbImage`. This is
//! the "always works" backend — no GPU required, no driver bugs, useful
//! for tests, the fidelity harness, and CI. The GPU path (Vello) lives
//! in a separate module once Spike A concludes.
//!
//! Coordinate system mirrors the display list: page space in pt, origin
//! top-left, y-down. `dpi` scales pt → pixels.
//!
//! Colour pipeline: Paints carry linear RGB (as per `idml-compose`).
//! tiny-skia expects sRGB; we apply the sRGB gamma curve at the paint
//! boundary. Fidelity-level ICC colour management comes through
//! `idml-color` — this module stays in the simple path.

use idml_compose::{
    BevelEmboss, BlendMode, Color as CComposeColor, DirectionalFeather, DisplayCommand,
    DisplayList, Feather, FeatherCornerType, GradientFeather, GradientFeatherKind, InnerGlow,
    InnerShadow, LineCap, LineJoin, OuterGlow, Paint, PathData, PathSegment, Satin,
    Transform as CTransform,
};
use image::{Rgba, RgbaImage};
use tiny_skia::{
    BlendMode as TsBlendMode, FillRule, GradientStop as TsGradientStop, LineCap as TsLineCap,
    LineJoin as TsLineJoin, LinearGradient as TsLinearGradient, Mask as TsMask, Paint as TsPaint,
    PathBuilder, Pixmap, PixmapPaint, PixmapRef, Point as TsPoint, PremultipliedColorU8,
    RadialGradient as TsRadialGradient, Shader, SpreadMode, Stroke as TsStroke,
    Transform as TsTransform,
};

use crate::{PathRasterizer, RasterOptions};

/// `PathRasterizer` impl backed by tiny-skia. Always-works backend
/// used by tests and the fidelity harness; no GPU required.
#[derive(Debug, Default, Clone, Copy)]
pub struct CpuRasterizer;

impl PathRasterizer for CpuRasterizer {
    fn name(&self) -> &'static str {
        "cpu/tiny-skia"
    }

    fn rasterize(&self, list: &DisplayList, options: &RasterOptions) -> Vec<u8> {
        rasterize(list, options).into_raw()
    }
}

/// One frame on the transparency-group stack. The `pixmap` is the
/// offscreen buffer we render the group's contents into; `offset` is
/// the top-left pixel of that buffer in the *page's* pixel-coord
/// system, so we can subtract it from per-command transforms and have
/// each fill/stroke/draw_pixmap land in the buffer's local pixel grid.
/// On `EndBlendGroup`, the buffer is composited onto the next-outer
/// target (the previous top of the stack, or the page if empty)
/// using `blend_mode` + `opacity`.
///
/// `backdrop_snapshot` mirrors the parent target's pixels at the
/// buffer's bbox, captured at `BeginBlendGroup` time. It enables the
/// PDF "paper is α=0 backdrop" semantic in the EndBlendGroup composite
/// — at pixels where the backdrop was still the pristine
/// page-background colour (i.e. paper, never drawn on), `Lighten` /
/// `Multiply` / etc. should bypass the blend mode and fall through to
/// a plain SourceOver, matching InDesign's non-isolated transparency
/// group behaviour against the paper plate. We only allocate the
/// snapshot when the blend mode is non-Normal — Normal (SourceOver)
/// doesn't need the bypass since SrcOver against opaque paper already
/// yields the source as-is.
struct GroupFrame {
    pixmap: Pixmap,
    /// Buffer top-left in page-pixel coords.
    offset: (i32, i32),
    blend_mode: TsBlendMode,
    opacity: f32,
    /// Snapshot of the parent target's pixels at the buffer's bbox
    /// taken at BeginBlendGroup time. `None` for SourceOver groups
    /// where no paper-bypass correction is needed.
    backdrop_snapshot: Option<Pixmap>,
}

/// Rasterise `list` to an 8-bit sRGB RGBA image at the configured DPI.
/// Free-function form retained for callers that already use it (the
/// `idml-renderer::pipeline::render_document` path).
pub fn rasterize(list: &DisplayList, options: &RasterOptions) -> RgbaImage {
    let (px_w, px_h) = options.pixel_size();
    let scale = options.dpi / 72.0;

    let mut pixmap = Pixmap::new(px_w, px_h).expect("non-zero pixmap");
    pixmap.fill(linear_color_to_ts(options.background));

    // Everything pt-space is scaled uniformly by `scale` into px-space.
    let page_to_px = TsTransform::from_scale(scale, scale);

    // Clip stack. Each entry is the cumulative intersection of every
    // pushed clip up to and including that level, scoped to one
    // render target — either the page or a specific group buffer.
    // The stack's `scope` field threads each entry to its owning
    // target so that clips pushed inside a `BeginBlendGroup` build
    // masks sized to the group buffer (not the page-sized pixmap)
    // and use buffer-local pixel coords. `EndBlendGroup` discards
    // any clips that belong to the group it's closing.
    //
    // tiny-skia masks live in pixel space; for the page they're sized
    // to `(px_w, px_h)` with `page_to_px` mapping pt→px directly. For
    // a group, they're sized to the group buffer and the clip path's
    // transform is pre-translated by the buffer's pixel offset so
    // points land in the buffer's local pixel grid. For Push,
    // intersect at pixel resolution to inherit anti-alias behaviour.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ClipScope {
        Page,
        /// 1-based depth into `group_stack` — clips at scope `Group(d)`
        /// belong to the group at index `d - 1`. Distinguishes nested
        /// groups so a `PopClip` after `EndBlendGroup` doesn't leak
        /// onto an outer group's stack.
        Group(usize),
    }
    struct ClipEntry {
        mask: TsMask,
        scope: ClipScope,
    }
    let mut clip_stack: Vec<ClipEntry> = Vec::new();

    // Transparency-group stack. When non-empty, every fill / stroke /
    // draw_pixmap targets the topmost group's pixmap instead of the
    // page; the group's `offset` translates page-space pixel coords
    // into the buffer's local origin so per-command transforms land
    // in the right cell. `EndBlendGroup` pops the top, composites it
    // onto the next-outer target.
    let mut group_stack: Vec<GroupFrame> = Vec::new();

    // Resolve the active render target for a draw command. When
    // inside a transparency group, fills/strokes/images target the
    // group's offscreen buffer instead of the page; we adjust the
    // page-to-px transform by the group's pixel offset so per-command
    // transforms map into the buffer's local coord grid.
    //
    // Mask handling: returns the topmost clip entry whose scope
    // matches the active target. Clips that belong to an outer
    // (shadowed) target stay alive but don't apply here.
    fn resolve_target<'a>(
        page_pixmap: &'a mut Pixmap,
        group_stack: &'a mut Vec<GroupFrame>,
        page_to_px: TsTransform,
        clip_stack: &'a [ClipEntry],
    ) -> (&'a mut Pixmap, TsTransform, Option<&'a TsMask>) {
        let scope = if group_stack.is_empty() {
            ClipScope::Page
        } else {
            ClipScope::Group(group_stack.len())
        };
        let mask = clip_stack
            .iter()
            .rev()
            .find(|e| e.scope == scope)
            .map(|e| &e.mask);
        if let Some(top) = group_stack.last_mut() {
            let off = top.offset;
            let xform = TsTransform::from_translate(-off.0 as f32, -off.1 as f32)
                .pre_concat(page_to_px);
            (&mut top.pixmap, xform, mask)
        } else {
            (page_pixmap, page_to_px, mask)
        }
    }

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
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                let ts_paint = paint_to_ts(paint, list, transform, target_xform);
                target.fill_path(&path, &ts_paint, FillRule::Winding, target_xform, target_mask);
            }
            DisplayCommand::FillPathBlend {
                path_id,
                paint,
                transform,
                blend_mode,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                let ts_paint = paint_to_ts(paint, list, transform, target_xform);
                let ts_mode = blend_mode_to_ts(*blend_mode);
                if matches!(ts_mode, TsBlendMode::SourceOver) {
                    // Normal blend ⇒ same fast path as FillPath.
                    target.fill_path(
                        &path,
                        &ts_paint,
                        FillRule::Winding,
                        target_xform,
                        target_mask,
                    );
                } else {
                    // Non-Normal: render the fill into a scratch
                    // pixmap covering the path's pixel bounds, then
                    // composite the stamp onto the page with the
                    // requested blend mode. Blend modes are
                    // pixel-local so the scratch only needs the path
                    // bbox + 1px anti-alias slack.
                    //
                    // This per-command approximation is retained for
                    // back-compat callers; the orchestrator now
                    // brackets non-Normal blends with
                    // BeginBlendGroup/EndBlendGroup instead, so this
                    // path is rarely hit at runtime.
                    let bbox = path.bounds();
                    let pad_pt = 1.0;
                    let min_x_pt = bbox.left() - pad_pt;
                    let min_y_pt = bbox.top() - pad_pt;
                    let max_x_pt = bbox.right() + pad_pt;
                    let max_y_pt = bbox.bottom() + pad_pt;
                    // Group-relative pixel offset: project path bounds
                    // through `target_xform` (page→pixel scale +
                    // group-buffer translation) to get buffer-local
                    // pixel coords.
                    let (lx_px, ly_px) = ts_xform_apply(target_xform, min_x_pt, min_y_pt);
                    let (rx_px, ry_px) = ts_xform_apply(target_xform, max_x_pt, max_y_pt);
                    let off_x_px = lx_px.min(rx_px).floor() as i32;
                    let off_y_px = ly_px.min(ry_px).floor() as i32;
                    let max_x_px = lx_px.max(rx_px).ceil() as i32;
                    let max_y_px = ly_px.max(ry_px).ceil() as i32;
                    let w_px = (max_x_px - off_x_px).max(1) as u32;
                    let h_px = (max_y_px - off_y_px).max(1) as u32;
                    if let Some(mut scratch) = Pixmap::new(w_px, h_px) {
                        let scratch_xform = TsTransform::from_translate(
                            -off_x_px as f32,
                            -off_y_px as f32,
                        )
                        .pre_concat(target_xform);
                        let scratch_paint =
                            paint_to_ts(paint, list, transform, scratch_xform);
                        scratch.fill_path(
                            &path,
                            &scratch_paint,
                            FillRule::Winding,
                            scratch_xform,
                            None,
                        );
                        let mut composite = PixmapPaint::default();
                        composite.blend_mode = ts_mode;
                        target.draw_pixmap(
                            off_x_px,
                            off_y_px,
                            scratch.as_ref(),
                            &composite,
                            TsTransform::identity(),
                            target_mask,
                        );
                    } else {
                        target.fill_path(
                            &path,
                            &ts_paint,
                            FillRule::Winding,
                            target_xform,
                            target_mask,
                        );
                    }
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
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                let ts_paint = paint_to_ts(paint, list, transform, target_xform);
                let ts_stroke = TsStroke {
                    width: stroke.width.max(0.0),
                    line_cap: map_cap(stroke.cap),
                    line_join: map_join(stroke.join),
                    miter_limit: stroke.miter_limit.max(1.0),
                    dash: if stroke.dash.is_solid() {
                        None
                    } else {
                        tiny_skia::StrokeDash::new(stroke.dash.as_slice().to_vec(), 0.0)
                    },
                };
                target.stroke_path(
                    &path,
                    &ts_paint,
                    &ts_stroke,
                    target_xform,
                    target_mask,
                );
            }
            DisplayCommand::DropShadow {
                path_id,
                transform,
                shadow,
            }
            | DisplayCommand::PathShadow {
                path_id,
                transform,
                shadow,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                // Build the path in page space, then translate by
                // the shadow offset.
                let mut shifted = *transform;
                shifted.0[4] += shadow.offset_x;
                shifted.0[5] += shadow.offset_y;
                let Some(path) = build_path_transformed(path_data, &shifted) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                let mut shadow_color = shadow.color;
                shadow_color.a *= shadow.opacity.clamp(0.0, 1.0);
                let mut p = TsPaint {
                    anti_alias: true,
                    ..Default::default()
                };
                p.set_color(linear_color_to_ts(shadow_color));

                // PathShadow (used for stroke-only / glyph drop
                // shadows lowered through the path-shadow pipeline)
                // empirically wants a *wider* Gaussian than the
                // rect-stamp DropShadow path: glyph shadows are
                // emitted as one PathShadow per glyph at full alpha
                // inside a Normal-blend transparency group (see
                // `idml-renderer/.../glyph_shadow.rs`), so adjacent
                // kernels overlap and SrcOver-saturate the buffer
                // alpha. Wider blur tails reach into the gaps between
                // glyph shadows and lift the buffer alpha closer to
                // 1.0 there before the group composite fades the
                // whole patch by the IDML opacity, which matches
                // InDesign's reference (page 8 of the manual sample)
                // far better than σ = Size. The rect-stamp DropShadow
                // path is unchanged (σ_scale = 1.0) so other corpus
                // pages that funnel through that arm stay
                // byte-identical.
                let sigma_scale: f32 = match cmd {
                    DisplayCommand::PathShadow { .. } => 3.5,
                    _ => 1.0,
                };
                let sigma_pt = shadow.blur_radius.max(0.0) * sigma_scale;
                // σ in pt → σ in pixels via the renderer's pt→px scale.
                let sigma_px = sigma_pt * scale;
                if sigma_px <= 0.5 {
                    // Fast path: blur is sub-pixel; the existing
                    // hard-edge fill is visually indistinguishable
                    // from a 0.5σ kernel, so skip the offscreen.
                    target.fill_path(
                        &path,
                        &p,
                        FillRule::Winding,
                        target_xform,
                        target_mask,
                    );
                } else {
                    // Offscreen path: rasterise the shadow stamp
                    // into a padded scratch pixmap, blur with a
                    // separable Gaussian, composite over the page.
                    // Path bounds are in page-space pt; pad by 3σ
                    // (kernel tail) to keep the whole soft edge
                    // inside the scratch buffer.
                    let bbox = path.bounds();
                    let pad_pt = 3.0 * sigma_pt + 1.0;
                    // Snap top-left to whole pixels so draw_pixmap
                    // (integer offsets) is pixel-aligned and the
                    // composite isn't bilinearly resampled. Project
                    // through `target_xform` so group-buffer renders
                    // place the stamp at buffer-local pixel coords.
                    let (lx_px, ly_px) =
                        ts_xform_apply(target_xform, bbox.left() - pad_pt, bbox.top() - pad_pt);
                    let (rx_px, ry_px) = ts_xform_apply(
                        target_xform,
                        bbox.right() + pad_pt,
                        bbox.bottom() + pad_pt,
                    );
                    let off_x_px = lx_px.min(rx_px).floor() as i32;
                    let off_y_px = ly_px.min(ry_px).floor() as i32;
                    let max_x_px = lx_px.max(rx_px).ceil() as i32;
                    let max_y_px = ly_px.max(ry_px).ceil() as i32;
                    let w_px = (max_x_px - off_x_px).max(1) as u32;
                    let h_px = (max_y_px - off_y_px).max(1) as u32;
                    if let Some(mut scratch) = Pixmap::new(w_px, h_px) {
                        // Translate so the scratch's pixel (0,0)
                        // corresponds to (off_x_px / scale, off_y_px / scale)
                        // in page space, then apply the same pt→px
                        // scale used elsewhere.
                        let scratch_xform = TsTransform::from_translate(
                            -off_x_px as f32,
                            -off_y_px as f32,
                        )
                        .pre_concat(target_xform);
                        scratch.fill_path(&path, &p, FillRule::Winding, scratch_xform, None);
                        // tiny-skia stores RGBA8 premultiplied — the
                        // Gaussian blurs each channel independently
                        // over premultiplied alpha, which is the
                        // correct convolution for a glow/shadow stamp
                        // (blurring straight alpha would brighten the
                        // edges into a halo).
                        let kernel = gaussian_kernel(sigma_px);
                        gaussian_blur_premul(scratch.data_mut(), w_px, h_px, &kernel);
                        target.draw_pixmap(
                            off_x_px,
                            off_y_px,
                            scratch.as_ref(),
                            &PixmapPaint::default(),
                            TsTransform::identity(),
                            target_mask,
                        );
                    } else {
                        // Allocation failed (pathological size) —
                        // fall back to the hard-edge fill rather
                        // than skipping the shadow entirely.
                        target.fill_path(
                            &path,
                            &p,
                            FillRule::Winding,
                            target_xform,
                            target_mask,
                        );
                    }
                }
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
                // Build a tiny_skia source pixmap from the decoded
                // RGBA8 buffer. This is one alloc + memcpy per
                // command; image dedup happens upstream when the
                // pipeline pushes into the list.
                let mut src = Pixmap::new(img.width, img.height).expect("non-zero image pixmap");
                src.data_mut().copy_from_slice(&img.rgba);
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                // Compose the placement transform: the display-list
                // transform maps (0..1, 0..1) → page coords, and
                // target_xform scales those to device pixels (page or
                // group-buffer). Source pixmap pixels live in (0..w,
                // 0..h), so divide by (w, h) before the existing
                // transform.
                let inv_w = 1.0 / img.width as f32;
                let inv_h = 1.0 / img.height as f32;
                let unit_to_page = TsTransform::from_row(
                    transform.0[0],
                    transform.0[1],
                    transform.0[2],
                    transform.0[3],
                    transform.0[4],
                    transform.0[5],
                );
                let pixel_to_unit = TsTransform::from_scale(inv_w, inv_h);
                let pixel_to_px = target_xform
                    .pre_concat(unit_to_page)
                    .pre_concat(pixel_to_unit);
                target.draw_pixmap(
                    0,
                    0,
                    src.as_ref(),
                    &PixmapPaint::default(),
                    pixel_to_px,
                    target_mask,
                );
            }
            DisplayCommand::PushClip { path_id, transform } => {
                // Determine which target the clip applies to: the
                // page or the topmost group buffer. The mask is
                // sized to that target's pixmap, and the clip path is
                // pre-translated by the group's `(off_x_px, off_y_px)`
                // so it lands in the buffer's local pixel coords.
                let (scope, mask_w, mask_h, target_off) =
                    if let Some(top) = group_stack.last() {
                        (
                            ClipScope::Group(group_stack.len()),
                            top.pixmap.width(),
                            top.pixmap.height(),
                            top.offset,
                        )
                    } else {
                        (ClipScope::Page, px_w, px_h, (0, 0))
                    };
                // `to_pixel` maps page-space pt → active target's
                // local pixel coords: scale by pt→px, then subtract
                // the group buffer's pixel offset (zero for the page).
                let to_pixel = TsTransform::from_translate(
                    -target_off.0 as f32,
                    -target_off.1 as f32,
                )
                .pre_concat(page_to_px);
                let Some(path_data) = list.paths.get(*path_id) else {
                    // Push a no-op (white) mask sized to the active
                    // target so the matching pop balances the stack.
                    if let Some(parent) =
                        clip_stack.iter().rev().find(|e| e.scope == scope)
                    {
                        clip_stack.push(ClipEntry {
                            mask: parent.mask.clone(),
                            scope,
                        });
                    } else if let Some(mut m) = TsMask::new(mask_w, mask_h) {
                        // tiny_skia::Mask::new is black/zero; invert
                        // to white so "no clip" semantics hold.
                        m.invert();
                        clip_stack.push(ClipEntry { mask: m, scope });
                    }
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    if let Some(parent) =
                        clip_stack.iter().rev().find(|e| e.scope == scope)
                    {
                        clip_stack.push(ClipEntry {
                            mask: parent.mask.clone(),
                            scope,
                        });
                    }
                    continue;
                };
                if let Some(parent) =
                    clip_stack.iter().rev().find(|e| e.scope == scope)
                {
                    let mut child = parent.mask.clone();
                    child.intersect_path(&path, FillRule::Winding, true, to_pixel);
                    clip_stack.push(ClipEntry {
                        mask: child,
                        scope,
                    });
                } else if let Some(mut fresh) = TsMask::new(mask_w, mask_h) {
                    // First clip on the active target: build from a
                    // fresh (transparent) mask filled with the path.
                    fresh.fill_path(&path, FillRule::Winding, true, to_pixel);
                    clip_stack.push(ClipEntry {
                        mask: fresh,
                        scope,
                    });
                }
            }
            DisplayCommand::PopClip(_) => {
                let scope = if group_stack.is_empty() {
                    ClipScope::Page
                } else {
                    ClipScope::Group(group_stack.len())
                };
                // Pop the topmost clip belonging to the active scope.
                // Stray pops (mismatched pairs) tolerated as before.
                if let Some(idx) =
                    clip_stack.iter().rposition(|e| e.scope == scope)
                {
                    clip_stack.remove(idx);
                }
            }
            DisplayCommand::BeginBlendGroup {
                bounds,
                blend_mode,
                opacity,
                ..
            } => {
                // Allocate an offscreen pixmap sized to the bounds (in
                // page coords) projected through page_to_px, with 1px
                // slack on each side for AA. The buffer's top-left
                // pixel in page-pixel coords is `(off_x_px, off_y_px)`
                // — subsequent fills/strokes/draws targeting this
                // group adjust their transform by that offset.
                let scale_factor = scale;
                let pad_pt = 1.0 / scale_factor.max(1e-6);
                let min_x_pt = bounds.x - pad_pt;
                let min_y_pt = bounds.y - pad_pt;
                let max_x_pt = bounds.x + bounds.w + pad_pt;
                let max_y_pt = bounds.y + bounds.h + pad_pt;
                let off_x_px = (min_x_pt * scale_factor).floor() as i32;
                let off_y_px = (min_y_pt * scale_factor).floor() as i32;
                let max_x_px = (max_x_pt * scale_factor).ceil() as i32;
                let max_y_px = (max_y_pt * scale_factor).ceil() as i32;
                let w_px = (max_x_px - off_x_px).max(1) as u32;
                let h_px = (max_y_px - off_y_px).max(1) as u32;
                match Pixmap::new(w_px, h_px) {
                    Some(buf) => {
                        let ts_blend = blend_mode_to_ts(*blend_mode);
                        // Snapshot the parent target's pixels at the
                        // buffer's bbox so EndBlendGroup can apply
                        // the paper-α=0 backdrop bypass. Only needed
                        // for non-SourceOver blend modes — SourceOver
                        // against opaque paper already produces the
                        // right answer (no correction needed).
                        let backdrop_snapshot = if matches!(ts_blend, TsBlendMode::SourceOver) {
                            None
                        } else if let Some(parent) = group_stack.last() {
                            snapshot_parent_region(
                                parent.pixmap.as_ref(),
                                parent.offset,
                                (off_x_px, off_y_px),
                                w_px,
                                h_px,
                            )
                        } else {
                            snapshot_parent_region(
                                pixmap.as_ref(),
                                (0, 0),
                                (off_x_px, off_y_px),
                                w_px,
                                h_px,
                            )
                        };
                        group_stack.push(GroupFrame {
                            pixmap: buf,
                            offset: (off_x_px, off_y_px),
                            blend_mode: ts_blend,
                            opacity: opacity.clamp(0.0, 1.0),
                            backdrop_snapshot,
                        });
                    }
                    None => {
                        // Allocation failure (zero or pathological
                        // size) — push a minimal 1×1 placeholder so
                        // the matching End balances the stack and
                        // drawing into the group is a no-op.
                        if let Some(buf) = Pixmap::new(1, 1) {
                            group_stack.push(GroupFrame {
                                pixmap: buf,
                                offset: (0, 0),
                                blend_mode: TsBlendMode::SourceOver,
                                opacity: 1.0,
                                backdrop_snapshot: None,
                            });
                        }
                    }
                }
            }
            DisplayCommand::InnerShadow {
                path_id,
                transform,
                params,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                render_inner_shadow(target, target_xform, target_mask, &path, params, scale);
            }
            DisplayCommand::OuterGlow {
                path_id,
                transform,
                params,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                render_outer_glow(target, target_xform, target_mask, &path, params, scale);
            }
            DisplayCommand::InnerGlow {
                path_id,
                transform,
                params,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                render_inner_glow(target, target_xform, target_mask, &path, params, scale);
            }
            DisplayCommand::BevelEmboss {
                path_id,
                transform,
                params,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                render_bevel_emboss(target, target_xform, target_mask, &path, params, scale);
            }
            DisplayCommand::Satin {
                path_id,
                transform,
                params,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                render_satin(target, target_xform, target_mask, &path, params, scale);
            }
            DisplayCommand::Feather {
                path_id,
                transform,
                params,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                render_feather(target, target_xform, target_mask, &path, params, scale);
            }
            DisplayCommand::DirectionalFeather {
                path_id,
                transform,
                params,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                render_directional_feather(
                    target,
                    target_xform,
                    target_mask,
                    &path,
                    params,
                    scale,
                );
            }
            DisplayCommand::GradientFeather {
                path_id,
                transform,
                params,
            } => {
                let Some(path_data) = list.paths.get(*path_id) else {
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    continue;
                };
                let (target, target_xform, target_mask) =
                    resolve_target(&mut pixmap, &mut group_stack, page_to_px, &clip_stack);
                render_gradient_feather(
                    target,
                    target_xform,
                    target_mask,
                    &path,
                    transform,
                    params,
                    scale,
                );
            }
            DisplayCommand::EndBlendGroup(_) => {
                let Some(top) = group_stack.pop() else {
                    continue;
                };
                // Drop any clips pushed while this group was active —
                // mismatched Push/Pop pairs inside a group can't
                // outlive their owning buffer.
                let group_scope = ClipScope::Group(group_stack.len() + 1);
                clip_stack.retain(|e| e.scope != group_scope);
                let GroupFrame {
                    pixmap: group_pix,
                    offset: (off_x_px, off_y_px),
                    blend_mode,
                    opacity,
                    backdrop_snapshot,
                } = top;
                let mut composite = PixmapPaint::default();
                composite.blend_mode = blend_mode;
                composite.opacity = opacity;
                // Composite the group buffer onto the next-outer
                // target. The active clip stack now resolves to the
                // parent target's scope (page or outer group).
                let parent_scope = if group_stack.is_empty() {
                    ClipScope::Page
                } else {
                    ClipScope::Group(group_stack.len())
                };
                let parent_mask_idx = clip_stack
                    .iter()
                    .rposition(|e| e.scope == parent_scope);
                let parent_mask = parent_mask_idx.map(|i| &clip_stack[i].mask);
                // Paper-backdrop premultiplied colour. The second
                // pass below uses this to detect "still paper"
                // pixels (snapshot ≈ paper) and overwrite the blended
                // result with a plain SrcOver(buffer*opacity, paper)
                // — the PDF / InDesign interpretation of paper as
                // α_b=0 backdrop for non-isolated transparency
                // groups.
                let paper_premul = linear_color_to_ts(options.background)
                    .premultiply()
                    .to_color_u8();
                if let Some(parent) = group_stack.last_mut() {
                    let parent_off = parent.offset;
                    let dst_x = off_x_px - parent_off.0;
                    let dst_y = off_y_px - parent_off.1;
                    parent.pixmap.draw_pixmap(
                        dst_x,
                        dst_y,
                        group_pix.as_ref(),
                        &composite,
                        TsTransform::identity(),
                        parent_mask,
                    );
                    if let Some(snapshot) = backdrop_snapshot.as_ref() {
                        apply_paper_backdrop_bypass(
                            &mut parent.pixmap,
                            (dst_x, dst_y),
                            group_pix.as_ref(),
                            snapshot.as_ref(),
                            opacity,
                            paper_premul,
                            parent_mask,
                        );
                    }
                } else {
                    pixmap.draw_pixmap(
                        off_x_px,
                        off_y_px,
                        group_pix.as_ref(),
                        &composite,
                        TsTransform::identity(),
                        parent_mask,
                    );
                    if let Some(snapshot) = backdrop_snapshot.as_ref() {
                        apply_paper_backdrop_bypass(
                            &mut pixmap,
                            (off_x_px, off_y_px),
                            group_pix.as_ref(),
                            snapshot.as_ref(),
                            opacity,
                            paper_premul,
                            parent_mask,
                        );
                    }
                }
            }
        }
    }

    let data = pixmap.take();
    RgbaImage::from_raw(px_w, px_h, data)
        .unwrap_or_else(|| RgbaImage::from_pixel(px_w, px_h, Rgba([0, 0, 0, 0])))
}

/// Apply a tiny-skia `Transform` (sx, ky, kx, sy, tx, ty form) to a
/// point. tiny-skia 0.11 only exposes `map_point(&mut Point)` which
/// requires a mutable reference; this helper sticks to plain f32 math.
fn ts_xform_apply(t: TsTransform, x: f32, y: f32) -> (f32, f32) {
    (t.sx * x + t.kx * y + t.tx, t.ky * x + t.sy * y + t.ty)
}

/// Snapshot a `w_px × h_px` region of `parent`'s pixels into a fresh
/// pixmap. `parent_off` is the parent target's top-left in page-pixel
/// coords (zero for the page, the group's offset for nested groups);
/// `child_off` is the child buffer's top-left in the same space.
/// Pixels outside the parent stay at the snapshot's default
/// (transparent black) — that's fine for the paper-bypass path
/// because the buffer is also empty there.
///
/// Used by `BeginBlendGroup` to capture the parent's content at the
/// buffer's bbox so `EndBlendGroup` can detect "still paper" pixels
/// (snapshot ≈ page background) and bypass the blend mode for the
/// PDF-correct non-isolated-group composite (see
/// `apply_paper_backdrop_bypass`).
fn snapshot_parent_region(
    parent: PixmapRef<'_>,
    parent_off: (i32, i32),
    child_off: (i32, i32),
    w_px: u32,
    h_px: u32,
) -> Option<Pixmap> {
    let mut snap = Pixmap::new(w_px, h_px)?;
    let snap_pixels = snap.pixels_mut();
    let parent_pixels = parent.pixels();
    let parent_w = parent.width() as i32;
    let parent_h = parent.height() as i32;
    let dx = child_off.0 - parent_off.0;
    let dy = child_off.1 - parent_off.1;
    for j in 0..h_px as i32 {
        let py = j + dy;
        if py < 0 || py >= parent_h {
            continue;
        }
        for i in 0..w_px as i32 {
            let px = i + dx;
            if px < 0 || px >= parent_w {
                continue;
            }
            let p_idx = (py * parent_w + px) as usize;
            let s_idx = (j * w_px as i32 + i) as usize;
            snap_pixels[s_idx] = parent_pixels[p_idx];
        }
    }
    Some(snap)
}

/// Second-pass paper-backdrop bypass for non-Normal blend groups.
/// After the standard `draw_pixmap` composite has run, walk every
/// non-transparent pixel of the group buffer; if the parent's
/// snapshot at that pixel was still the page background colour
/// (i.e. paper, never drawn on), overwrite the parent's pixel with a
/// plain `SrcOver(buffer * opacity, paper)`. This matches InDesign /
/// PDF's non-isolated transparency-group semantic where the paper
/// plate has α_b=0, so blend modes like `Lighten` collapse to
/// `SourceOver` against paper. Without this, Lighten of a black
/// glyph on a white page wipes the glyph (max(black, white) = white)
/// even though InDesign expects the glyph to show through opaque
/// black against the paper.
///
/// `target_off` is the buffer's top-left in the parent target's
/// pixel-coord system (already incorporates any group-stack offset).
/// `parent_mask` mirrors the mask passed to `draw_pixmap` — pixels
/// outside the mask stay untouched so the bypass can't paint over a
/// clipped-out region.
fn apply_paper_backdrop_bypass(
    parent: &mut Pixmap,
    target_off: (i32, i32),
    buffer: PixmapRef<'_>,
    snapshot: PixmapRef<'_>,
    opacity: f32,
    paper_premul: PremultipliedColorU8,
    parent_mask: Option<&TsMask>,
) {
    let parent_w = parent.width() as i32;
    let parent_h = parent.height() as i32;
    let buf_w = buffer.width() as i32;
    let buf_h = buffer.height() as i32;
    let buf_pixels = buffer.pixels();
    let snap_pixels = snapshot.pixels();
    let parent_pixels = parent.pixels_mut();
    let mask_data = parent_mask.map(|m| (m.data(), m.width() as i32, m.height() as i32));
    // "Still paper" tolerance: tiny-skia's premultiplied 8-bit pixels
    // round identically when fill()'d, so an exact match is the
    // strictest test. Allow 1 channel-step of slack to absorb any
    // single-step rounding from blend ops that happen to land exactly
    // on the paper colour but went through a premultiply round-trip.
    // Larger tolerances would risk classifying a hand-painted
    // "exactly-paper-coloured" rect as paper and over-bypassing it.
    let near_paper = |p: PremultipliedColorU8| -> bool {
        let dr = p.red() as i32 - paper_premul.red() as i32;
        let dg = p.green() as i32 - paper_premul.green() as i32;
        let db = p.blue() as i32 - paper_premul.blue() as i32;
        let da = p.alpha() as i32 - paper_premul.alpha() as i32;
        dr.abs() <= 1 && dg.abs() <= 1 && db.abs() <= 1 && da.abs() <= 1
    };
    // Premultiply the buffer's source pixel by the group's opacity,
    // then SrcOver onto paper. Both operands are premultiplied (the
    // buffer's pixel and `paper_premul`). For paper at α=1 this
    // reduces to (1 - sa) * paper + scaled_buffer.
    let src_over_on_paper = |buf: PremultipliedColorU8| -> PremultipliedColorU8 {
        let op = (opacity.clamp(0.0, 1.0) * 255.0).round() as i32;
        let scale = |c: u8| -> u8 { ((c as i32 * op + 127) / 255).clamp(0, 255) as u8 };
        let sr = scale(buf.red());
        let sg = scale(buf.green());
        let sb = scale(buf.blue());
        let sa = scale(buf.alpha());
        let inv = 255 - sa as i32;
        let merge = |s: u8, d: u8| -> u8 {
            ((s as i32 * 255 + inv * d as i32 + 127) / 255).clamp(0, 255) as u8
        };
        PremultipliedColorU8::from_rgba(
            merge(sr, paper_premul.red()),
            merge(sg, paper_premul.green()),
            merge(sb, paper_premul.blue()),
            merge(sa, paper_premul.alpha()),
        )
        .unwrap_or(paper_premul)
    };
    for j in 0..buf_h {
        let py = j + target_off.1;
        if py < 0 || py >= parent_h {
            continue;
        }
        for i in 0..buf_w {
            let px = i + target_off.0;
            if px < 0 || px >= parent_w {
                continue;
            }
            let buf_idx = (j * buf_w + i) as usize;
            let buf_pixel = buf_pixels[buf_idx];
            if buf_pixel.alpha() == 0 {
                continue;
            }
            let snap_pixel = snap_pixels[buf_idx];
            if !near_paper(snap_pixel) {
                continue;
            }
            // Honour the parent mask: pixels outside the clip stay
            // untouched (the standard draw_pixmap pass already
            // skipped them, and we mustn't re-introduce coverage in
            // the clipped-out region).
            if let Some((md, mw, mh)) = mask_data {
                if px >= 0 && py >= 0 && px < mw && py < mh {
                    let m_idx = (py * mw + px) as usize;
                    if md[m_idx] == 0 {
                        continue;
                    }
                }
            }
            let par_idx = (py * parent_w + px) as usize;
            parent_pixels[par_idx] = src_over_on_paper(buf_pixel);
        }
    }
}

/// Build a tiny-skia path with `path_transform` applied to every
/// control point. After this, the path lives in page space, so stroke
/// widths — specified in pt — aren't distorted by non-uniform rect
/// transforms (which would otherwise make horizontal edges thicker
/// than vertical ones on a non-square frame).
fn build_path_transformed(data: &PathData, path_transform: &CTransform) -> Option<tiny_skia::Path> {
    let apply = |x: f32, y: f32| {
        let [a, b, c, d, tx, ty] = path_transform.0;
        (a * x + c * y + tx, b * x + d * y + ty)
    };
    let mut bld = PathBuilder::new();
    for seg in &data.segments {
        match *seg {
            PathSegment::MoveTo { x, y } => {
                let (px, py) = apply(x, y);
                bld.move_to(px, py);
            }
            PathSegment::LineTo { x, y } => {
                let (px, py) = apply(x, y);
                bld.line_to(px, py);
            }
            PathSegment::QuadTo { cx, cy, x, y } => {
                let (pcx, pcy) = apply(cx, cy);
                let (px, py) = apply(x, y);
                bld.quad_to(pcx, pcy, px, py);
            }
            PathSegment::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => {
                let (p1x, p1y) = apply(cx1, cy1);
                let (p2x, p2y) = apply(cx2, cy2);
                let (px, py) = apply(x, y);
                bld.cubic_to(p1x, p1y, p2x, p2y, px, py);
            }
            PathSegment::Close => bld.close(),
        }
    }
    bld.finish()
}

fn paint_to_ts(
    paint: &Paint,
    list: &DisplayList,
    path_transform: &CTransform,
    page_to_px: TsTransform,
) -> TsPaint<'static> {
    let mut p = TsPaint {
        anti_alias: true,
        ..Default::default()
    };
    match paint {
        Paint::Solid(c) => {
            p.set_color(linear_color_to_ts(*c));
        }
        Paint::LinearGradient(id) => {
            if let Some(grad) = list.linear_gradient(*id) {
                if let Some(shader) = build_linear_gradient_shader(grad, path_transform, page_to_px)
                {
                    p.shader = shader;
                } else {
                    // Empty / invalid gradient → black fallback.
                    p.set_color(tiny_skia::Color::BLACK);
                }
            } else {
                p.set_color(tiny_skia::Color::BLACK);
            }
        }
        Paint::RadialGradient(id) => {
            if let Some(grad) = list.radial_gradient(*id) {
                if let Some(shader) = build_radial_gradient_shader(grad, path_transform, page_to_px)
                {
                    p.shader = shader;
                } else {
                    p.set_color(tiny_skia::Color::BLACK);
                }
            } else {
                p.set_color(tiny_skia::Color::BLACK);
            }
        }
    }
    p
}

fn build_linear_gradient_shader(
    grad: &idml_compose::LinearGradient,
    path_transform: &CTransform,
    page_to_px: TsTransform,
) -> Option<Shader<'static>> {
    if grad.stops.len() < 2 {
        return None;
    }
    // Map the gradient's unit-square endpoints into page space via
    // the path's transform — the gradient lives in path-local coords
    // (the unit-rect we reuse for emit_rect / emit_ellipse).
    let [a, b, c, d, tx, ty] = path_transform.0;
    let to_page =
        |x: f32, y: f32| -> TsPoint { TsPoint::from_xy(a * x + c * y + tx, b * x + d * y + ty) };
    let start = to_page(grad.start.0, grad.start.1);
    let end = to_page(grad.end.0, grad.end.1);

    let stops: Vec<TsGradientStop> = grad
        .stops
        .iter()
        .map(|s| TsGradientStop::new(s.offset.clamp(0.0, 1.0), linear_color_to_ts(s.color)))
        .collect();

    let _ = page_to_px;
    // Shader endpoints already live in page (path) space, which
    // matches the path's pre-transformed coordinates. tiny-skia
    // composes the shader transform with the fill_path transform
    // automatically, so an identity here is correct — passing
    // page_to_px would double-scale at non-72-DPI renders.
    TsLinearGradient::new(start, end, stops, SpreadMode::Pad, TsTransform::identity())
}

fn build_radial_gradient_shader(
    grad: &idml_compose::RadialGradient,
    path_transform: &CTransform,
    page_to_px: TsTransform,
) -> Option<Shader<'static>> {
    if grad.stops.len() < 2 {
        return None;
    }
    let [a, b, c, d, tx, ty] = path_transform.0;
    let to_page =
        |x: f32, y: f32| -> TsPoint { TsPoint::from_xy(a * x + c * y + tx, b * x + d * y + ty) };
    let center = to_page(grad.center.0, grad.center.1);
    // tiny-skia takes one focal point + radius. Compute the page-
    // space radius by mapping a unit-axis vector and averaging the
    // two axes — handles non-uniform scale-into-rect with a single
    // circle, matching how InDesign warps a Radial gradient when
    // the path's local rect is non-square (it ovals out with it).
    let rx = (a * grad.radius).hypot(b * grad.radius);
    let ry = (c * grad.radius).hypot(d * grad.radius);
    let radius = (rx + ry) * 0.5;
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }

    let stops: Vec<TsGradientStop> = grad
        .stops
        .iter()
        .map(|s| TsGradientStop::new(s.offset.clamp(0.0, 1.0), linear_color_to_ts(s.color)))
        .collect();

    let _ = page_to_px;
    // tiny-skia takes (start_point, start_radius, end_point,
    // end_radius). Same point + zero start radius models the
    // common single-circle radial fill (focal == center).
    TsRadialGradient::new(
        center,
        0.0,
        center,
        radius,
        stops,
        SpreadMode::Pad,
        TsTransform::identity(),
    )
}

/// Linear RGB (0..=1) → sRGB-encoded tiny_skia::Color.
fn linear_color_to_ts(c: CComposeColor) -> tiny_skia::Color {
    let r = linear_to_srgb(c.r.clamp(0.0, 1.0));
    let g = linear_to_srgb(c.g.clamp(0.0, 1.0));
    let b = linear_to_srgb(c.b.clamp(0.0, 1.0));
    let a = c.a.clamp(0.0, 1.0);
    tiny_skia::Color::from_rgba(r, g, b, a).unwrap_or(tiny_skia::Color::BLACK)
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// 1-D Gaussian kernel sampled at integer pixel offsets, truncated at
/// 3σ on each side and normalised to sum to 1. Returned vector is
/// symmetric around index `kernel.len() / 2`.
fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let radius = (3.0 * sigma).ceil().max(1.0) as i32;
    let two_sigma_sq = 2.0 * sigma * sigma;
    let mut k = Vec::with_capacity(2 * radius as usize + 1);
    let mut sum = 0.0f32;
    for i in -radius..=radius {
        let v = (-(i as f32) * (i as f32) / two_sigma_sq).exp();
        k.push(v);
        sum += v;
    }
    if sum > 0.0 {
        for v in &mut k {
            *v /= sum;
        }
    }
    k
}

/// Separable Gaussian blur over a tiny-skia premultiplied RGBA8 buffer
/// (`width * height * 4` bytes, row-major). Two passes: horizontal then
/// vertical. Edges use clamp-to-edge addressing — the scratch buffer is
/// padded by 3σ before this is called, so clamping reads the (zero)
/// background, which is exactly what we want for an isolated stamp.
fn gaussian_blur_premul(data: &mut [u8], width: u32, height: u32, kernel: &[f32]) {
    if kernel.len() < 2 || width == 0 || height == 0 {
        return;
    }
    let w = width as usize;
    let h = height as usize;
    let radius = (kernel.len() / 2) as isize;

    // Horizontal pass: data → tmp.
    let mut tmp = vec![0u8; data.len()];
    for y in 0..h {
        let row = y * w * 4;
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for (k_idx, &coeff) in kernel.iter().enumerate() {
                let sx = (x as isize + k_idx as isize - radius)
                    .clamp(0, w as isize - 1) as usize;
                let p = row + sx * 4;
                acc[0] += data[p] as f32 * coeff;
                acc[1] += data[p + 1] as f32 * coeff;
                acc[2] += data[p + 2] as f32 * coeff;
                acc[3] += data[p + 3] as f32 * coeff;
            }
            let q = row + x * 4;
            tmp[q] = acc[0].round().clamp(0.0, 255.0) as u8;
            tmp[q + 1] = acc[1].round().clamp(0.0, 255.0) as u8;
            tmp[q + 2] = acc[2].round().clamp(0.0, 255.0) as u8;
            tmp[q + 3] = acc[3].round().clamp(0.0, 255.0) as u8;
        }
    }

    // Vertical pass: tmp → data.
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for (k_idx, &coeff) in kernel.iter().enumerate() {
                let sy = (y as isize + k_idx as isize - radius)
                    .clamp(0, h as isize - 1) as usize;
                let p = (sy * w + x) * 4;
                acc[0] += tmp[p] as f32 * coeff;
                acc[1] += tmp[p + 1] as f32 * coeff;
                acc[2] += tmp[p + 2] as f32 * coeff;
                acc[3] += tmp[p + 3] as f32 * coeff;
            }
            let q = (y * w + x) * 4;
            data[q] = acc[0].round().clamp(0.0, 255.0) as u8;
            data[q + 1] = acc[1].round().clamp(0.0, 255.0) as u8;
            data[q + 2] = acc[2].round().clamp(0.0, 255.0) as u8;
            data[q + 3] = acc[3].round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn map_cap(cap: LineCap) -> TsLineCap {
    match cap {
        LineCap::Butt => TsLineCap::Butt,
        LineCap::Round => TsLineCap::Round,
        LineCap::Square => TsLineCap::Square,
    }
}

fn map_join(join: LineJoin) -> TsLineJoin {
    match join {
        LineJoin::Miter => TsLineJoin::Miter,
        LineJoin::Round => TsLineJoin::Round,
        LineJoin::Bevel => TsLineJoin::Bevel,
    }
}

/// Map the IDML / compose-layer `BlendMode` to tiny-skia's enum.
/// Names line up 1:1 — Normal becomes SourceOver (the canonical
/// alpha-composite default).
fn blend_mode_to_ts(m: BlendMode) -> TsBlendMode {
    match m {
        BlendMode::Normal => TsBlendMode::SourceOver,
        BlendMode::Multiply => TsBlendMode::Multiply,
        BlendMode::Screen => TsBlendMode::Screen,
        BlendMode::Overlay => TsBlendMode::Overlay,
        BlendMode::Darken => TsBlendMode::Darken,
        BlendMode::Lighten => TsBlendMode::Lighten,
        BlendMode::ColorDodge => TsBlendMode::ColorDodge,
        BlendMode::ColorBurn => TsBlendMode::ColorBurn,
        BlendMode::HardLight => TsBlendMode::HardLight,
        BlendMode::SoftLight => TsBlendMode::SoftLight,
        BlendMode::Difference => TsBlendMode::Difference,
        BlendMode::Exclusion => TsBlendMode::Exclusion,
        BlendMode::Hue => TsBlendMode::Hue,
        BlendMode::Saturation => TsBlendMode::Saturation,
        BlendMode::Color => TsBlendMode::Color,
        BlendMode::Luminosity => TsBlendMode::Luminosity,
    }
}

/// Project a path's pt-space bounds (with `pad_pt` extra slack on
/// each side) through `target_xform` into pixel-aligned offset + size
/// for an effect scratch buffer. Returns `(off_x_px, off_y_px, w_px,
/// h_px, scratch_xform)` where `scratch_xform` maps pt-space points
/// into the scratch buffer's local pixel grid (so the path can be
/// re-rasterised into the buffer with the same control-point
/// projection logic as the page render).
fn effect_scratch_bounds(
    path: &tiny_skia::Path,
    target_xform: TsTransform,
    pad_pt: f32,
) -> Option<(i32, i32, u32, u32, TsTransform)> {
    let bbox = path.bounds();
    let (lx_px, ly_px) =
        ts_xform_apply(target_xform, bbox.left() - pad_pt, bbox.top() - pad_pt);
    let (rx_px, ry_px) =
        ts_xform_apply(target_xform, bbox.right() + pad_pt, bbox.bottom() + pad_pt);
    let off_x_px = lx_px.min(rx_px).floor() as i32;
    let off_y_px = ly_px.min(ry_px).floor() as i32;
    let max_x_px = lx_px.max(rx_px).ceil() as i32;
    let max_y_px = ly_px.max(ry_px).ceil() as i32;
    let w_px = (max_x_px - off_x_px).max(1) as u32;
    let h_px = (max_y_px - off_y_px).max(1) as u32;
    if w_px == 0 || h_px == 0 || w_px > 8192 || h_px > 8192 {
        return None;
    }
    let scratch_xform =
        TsTransform::from_translate(-off_x_px as f32, -off_y_px as f32).pre_concat(target_xform);
    Some((off_x_px, off_y_px, w_px, h_px, scratch_xform))
}

/// Stamp `path` filled with opaque white into a fresh pixmap of the
/// given size at `scratch_xform`. The result's alpha channel is the
/// path-interior mask (0 outside, 255 inside, anti-aliased on the
/// edge); RGB equals alpha (premultiplied white). Used as the
/// starting point for inner-shadow / glow / bevel / satin / feather
/// passes.
fn stamp_path_alpha(
    w_px: u32,
    h_px: u32,
    path: &tiny_skia::Path,
    scratch_xform: TsTransform,
) -> Option<Pixmap> {
    let mut scratch = Pixmap::new(w_px, h_px)?;
    let mut p = TsPaint {
        anti_alias: true,
        ..Default::default()
    };
    p.set_color(tiny_skia::Color::WHITE);
    scratch.fill_path(path, &p, FillRule::Winding, scratch_xform, None);
    Some(scratch)
}

/// Read the alpha channel of a tiny-skia premultiplied RGBA8 buffer
/// into a fresh `Vec<u8>`. The data is interpreted as a single-
/// channel mask in `[0, 255]` (0 outside, 255 inside, anti-aliased
/// on the edge).
fn alpha_to_mask(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4);
    for chunk in rgba.chunks_exact(4) {
        out.push(chunk[3]);
    }
    out
}

/// Convolve a single-channel mask in-place with a separable Gaussian.
/// Edges clamp to `0` (this is what we want for an isolated stamp's
/// outer halo: the mask is padded by 3σ before this is called).
fn gaussian_blur_mask(mask: &mut [u8], width: u32, height: u32, kernel: &[f32]) {
    if kernel.len() < 2 || width == 0 || height == 0 {
        return;
    }
    let w = width as usize;
    let h = height as usize;
    let radius = (kernel.len() / 2) as isize;
    let mut tmp = vec![0u8; mask.len()];
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let mut acc = 0.0f32;
            for (k_idx, &coeff) in kernel.iter().enumerate() {
                let sx = (x as isize + k_idx as isize - radius)
                    .clamp(0, w as isize - 1) as usize;
                acc += mask[row + sx] as f32 * coeff;
            }
            tmp[row + x] = acc.round().clamp(0.0, 255.0) as u8;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f32;
            for (k_idx, &coeff) in kernel.iter().enumerate() {
                let sy = (y as isize + k_idx as isize - radius)
                    .clamp(0, h as isize - 1) as usize;
                acc += tmp[sy * w + x] as f32 * coeff;
            }
            mask[y * w + x] = acc.round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Inner shadow: paint a soft, offset shadow on the *inside* of the
/// path. Algorithm:
///   1. Build the path interior mask `M` (alpha channel).
///   2. Build the offset+choked path interior mask `Moff` by
///      stamping the path shifted by `(offset_x, offset_y)`, dilated
///      by `choke` pt.
///   3. The "shadow source" is `(1 - Moff)`: the area *outside* the
///      offset path. Blur it.
///   4. Composite the blurred source clipped to `M` (so the shadow
///      stays inside the path interior), tinted with `params.color`
///      at `params.opacity`.
fn render_inner_shadow(
    target: &mut Pixmap,
    target_xform: TsTransform,
    target_mask: Option<&TsMask>,
    path: &tiny_skia::Path,
    params: &InnerShadow,
    scale: f32,
) {
    // The padding only needs to cover the path interior plus enough
    // slack for the blur kernel — the shadow lives *inside* the
    // path, so anything farther than 3σ from the edge is safely zero.
    let pad_pt = 3.0 * params.blur_radius.max(0.0)
        + params.choke.abs()
        + params.offset_x.abs().max(params.offset_y.abs())
        + 1.0;
    let Some((off_x_px, off_y_px, w_px, h_px, scratch_xform)) =
        effect_scratch_bounds(path, target_xform, pad_pt)
    else {
        return;
    };
    let Some(interior_pix) = stamp_path_alpha(w_px, h_px, path, scratch_xform) else {
        return;
    };
    let interior_mask = alpha_to_mask(interior_pix.data());

    // Build the offset path mask: same path but translated by
    // (offset_x, offset_y) in pt-space (pre-concat the translate
    // *into* scratch_xform so it lands in pixel-space correctly).
    let offset_xform = scratch_xform
        .pre_concat(TsTransform::from_translate(params.offset_x, params.offset_y));
    let Some(offset_pix) = stamp_path_alpha(w_px, h_px, path, offset_xform) else {
        return;
    };
    let mut offset_mask = alpha_to_mask(offset_pix.data());

    // Apply choke as an additional dilation: a positive choke grows
    // the offset stamp inward (smaller blur footprint). We approximate
    // by blurring the offset mask by a small Gaussian and then
    // thresholding at `(0.5 - choke)` to bias the boundary; this is
    // a cheap dilation/erosion with the same code path.
    let choke_px = params.choke.max(0.0) * scale;
    if choke_px > 0.5 {
        let kernel = gaussian_kernel(choke_px);
        gaussian_blur_mask(&mut offset_mask, w_px, h_px, &kernel);
        // Re-threshold: anything brighter than ~64 is treated as
        // "inside", which approximates a dilation by ~choke_px.
        for v in offset_mask.iter_mut() {
            *v = if *v > 64 { 255 } else { 0 };
        }
    }

    // Source = (1 - offset_mask) — the "outside" of the offset path.
    let mut source: Vec<u8> = offset_mask.iter().map(|&v| 255 - v).collect();

    // Blur the source.
    let sigma_px = params.blur_radius.max(0.0) * scale;
    if sigma_px > 0.5 {
        let kernel = gaussian_kernel(sigma_px);
        gaussian_blur_mask(&mut source, w_px, h_px, &kernel);
    }

    // Mask source by the path interior so the shadow only paints
    // inside the path. Final per-pixel alpha = source * interior /
    // 255, scaled by opacity. Then build a premultiplied RGBA buffer
    // tinted with `params.color`.
    let mut scratch = match Pixmap::new(w_px, h_px) {
        Some(p) => p,
        None => return,
    };
    let opacity = params.opacity.clamp(0.0, 1.0);
    let ts_color = linear_color_to_ts(params.color);
    let cr = ts_color.red();
    let cg = ts_color.green();
    let cb = ts_color.blue();
    let ca = ts_color.alpha();
    let data = scratch.data_mut();
    for i in 0..(w_px as usize * h_px as usize) {
        let inside = interior_mask[i] as f32 / 255.0;
        let s = source[i] as f32 / 255.0;
        let a = (inside * s * opacity * ca).clamp(0.0, 1.0);
        // Premultiplied output: store (color * a, a).
        let q = i * 4;
        data[q] = (cr * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 1] = (cg * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 2] = (cb * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    let mut composite = PixmapPaint::default();
    composite.blend_mode = blend_mode_to_ts(params.blend_mode);
    target.draw_pixmap(
        off_x_px,
        off_y_px,
        scratch.as_ref(),
        &composite,
        TsTransform::identity(),
        target_mask,
    );
}

/// Outer glow: soft halo *outside* the path. Algorithm:
///   1. Build the path interior mask `M`.
///   2. Optionally dilate by `spread` pt (so glows can extend farther
///      than the blur alone would carry them).
///   3. Blur the mask by `blur_radius`.
///   4. Subtract the path interior so the glow only paints outside.
///   5. Tint and composite.
fn render_outer_glow(
    target: &mut Pixmap,
    target_xform: TsTransform,
    target_mask: Option<&TsMask>,
    path: &tiny_skia::Path,
    params: &OuterGlow,
    scale: f32,
) {
    let pad_pt = 3.0 * params.blur_radius.max(0.0) + params.spread.abs() + 1.0;
    let Some((off_x_px, off_y_px, w_px, h_px, scratch_xform)) =
        effect_scratch_bounds(path, target_xform, pad_pt)
    else {
        return;
    };
    let Some(interior_pix) = stamp_path_alpha(w_px, h_px, path, scratch_xform) else {
        return;
    };
    let interior_mask = alpha_to_mask(interior_pix.data());
    let mut halo = interior_mask.clone();

    // Cheap "spread" dilation — same trick as in InnerShadow's choke.
    let spread_px = params.spread.max(0.0) * scale;
    if spread_px > 0.5 {
        let kernel = gaussian_kernel(spread_px);
        gaussian_blur_mask(&mut halo, w_px, h_px, &kernel);
        for v in halo.iter_mut() {
            *v = if *v > 64 { 255 } else { 0 };
        }
    }

    // Blur.
    let sigma_px = params.blur_radius.max(0.0) * scale;
    if sigma_px > 0.5 {
        let kernel = gaussian_kernel(sigma_px);
        gaussian_blur_mask(&mut halo, w_px, h_px, &kernel);
    }

    // Subtract the path interior so the glow only lands outside it.
    // This avoids the glow doubling up under the fill (which would
    // wash out the colour where the path has its own paint).
    let opacity = params.opacity.clamp(0.0, 1.0);
    let ts_color = linear_color_to_ts(params.color);
    let cr = ts_color.red();
    let cg = ts_color.green();
    let cb = ts_color.blue();
    let ca = ts_color.alpha();
    let mut scratch = match Pixmap::new(w_px, h_px) {
        Some(p) => p,
        None => return,
    };
    let data = scratch.data_mut();
    for i in 0..(w_px as usize * h_px as usize) {
        let h = halo[i] as f32 / 255.0;
        let m = interior_mask[i] as f32 / 255.0;
        // Outside-only halo: max(halo - interior, 0).
        let outside = (h - m).max(0.0);
        let a = (outside * opacity * ca).clamp(0.0, 1.0);
        let q = i * 4;
        data[q] = (cr * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 1] = (cg * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 2] = (cb * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    let mut composite = PixmapPaint::default();
    composite.blend_mode = blend_mode_to_ts(params.blend_mode);
    target.draw_pixmap(
        off_x_px,
        off_y_px,
        scratch.as_ref(),
        &composite,
        TsTransform::identity(),
        target_mask,
    );
}

/// Inner glow: soft glow on the *inside* of the path's interior. This
/// is the no-offset, glow-coloured cousin of [`render_inner_shadow`].
fn render_inner_glow(
    target: &mut Pixmap,
    target_xform: TsTransform,
    target_mask: Option<&TsMask>,
    path: &tiny_skia::Path,
    params: &InnerGlow,
    scale: f32,
) {
    let pad_pt = 3.0 * params.blur_radius.max(0.0) + params.choke.abs() + 1.0;
    let Some((off_x_px, off_y_px, w_px, h_px, scratch_xform)) =
        effect_scratch_bounds(path, target_xform, pad_pt)
    else {
        return;
    };
    let Some(interior_pix) = stamp_path_alpha(w_px, h_px, path, scratch_xform) else {
        return;
    };
    let interior_mask = alpha_to_mask(interior_pix.data());

    // Source = (1 - interior). Blurring this and clipping to the
    // interior gives a glow at the path's inner edge fading inward.
    let mut source: Vec<u8> = interior_mask.iter().map(|&v| 255 - v).collect();

    let choke_px = params.choke.max(0.0) * scale;
    if choke_px > 0.5 {
        // Choke pulls the glow boundary inward by erosion: blur +
        // re-threshold biased high.
        let kernel = gaussian_kernel(choke_px);
        gaussian_blur_mask(&mut source, w_px, h_px, &kernel);
        for v in source.iter_mut() {
            *v = if *v > 64 { 255 } else { 0 };
        }
    }

    let sigma_px = params.blur_radius.max(0.0) * scale;
    if sigma_px > 0.5 {
        let kernel = gaussian_kernel(sigma_px);
        gaussian_blur_mask(&mut source, w_px, h_px, &kernel);
    }

    let opacity = params.opacity.clamp(0.0, 1.0);
    let ts_color = linear_color_to_ts(params.color);
    let cr = ts_color.red();
    let cg = ts_color.green();
    let cb = ts_color.blue();
    let ca = ts_color.alpha();
    let mut scratch = match Pixmap::new(w_px, h_px) {
        Some(p) => p,
        None => return,
    };
    let data = scratch.data_mut();
    for i in 0..(w_px as usize * h_px as usize) {
        let inside = interior_mask[i] as f32 / 255.0;
        let s = source[i] as f32 / 255.0;
        let a = (inside * s * opacity * ca).clamp(0.0, 1.0);
        let q = i * 4;
        data[q] = (cr * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 1] = (cg * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 2] = (cb * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    let mut composite = PixmapPaint::default();
    composite.blend_mode = blend_mode_to_ts(params.blend_mode);
    target.draw_pixmap(
        off_x_px,
        off_y_px,
        scratch.as_ref(),
        &composite,
        TsTransform::identity(),
        target_mask,
    );
}

/// Bevel and emboss. Algorithm:
///   1. Build the path interior mask `M`.
///   2. Blur `M` by `size` to get a smooth height field `H`.
///   3. Compute the gradient `(∂H/∂x, ∂H/∂y)`; treat as a 2D normal
///      with a fixed `z = (1 - |grad|)` term so flat regions have
///      `n_z = 1` (face the viewer) and edges have a sloped normal.
///   4. Compute Lambertian shading `n · L` against a light direction
///      derived from `angle_deg` (azimuth) and `altitude_deg`
///      (elevation). Positive shading paints the highlight colour;
///      negative shading paints the shadow colour.
///   5. Mask by the path interior and composite.
fn render_bevel_emboss(
    target: &mut Pixmap,
    target_xform: TsTransform,
    target_mask: Option<&TsMask>,
    path: &tiny_skia::Path,
    params: &BevelEmboss,
    scale: f32,
) {
    let pad_pt = 3.0 * params.size.max(0.0) + 2.0;
    let Some((off_x_px, off_y_px, w_px, h_px, scratch_xform)) =
        effect_scratch_bounds(path, target_xform, pad_pt)
    else {
        return;
    };
    let Some(interior_pix) = stamp_path_alpha(w_px, h_px, path, scratch_xform) else {
        return;
    };
    let interior_mask = alpha_to_mask(interior_pix.data());
    let mut height: Vec<f32> = interior_mask.iter().map(|&v| v as f32 / 255.0).collect();

    // Smooth the height field. Larger `size` → softer bevel.
    let sigma_px = params.size.max(0.0) * scale * 0.5;
    if sigma_px > 0.5 {
        // Convert to u8, blur, convert back.
        let mut h8: Vec<u8> = height
            .iter()
            .map(|&v| (v * 255.0).round() as u8)
            .collect();
        let kernel = gaussian_kernel(sigma_px);
        gaussian_blur_mask(&mut h8, w_px, h_px, &kernel);
        for (slot, src) in height.iter_mut().zip(h8.iter()) {
            *slot = *src as f32 / 255.0;
        }
    }

    // Light direction. IDML's angle is screen-azimuth in degrees;
    // altitude is elevation in degrees. Compute the unit light vector
    // in (x, y, z) — y is page-down so we negate sin(angle) for the
    // "y points up" math used inside the shading kernel.
    let az = params.angle_deg.to_radians();
    let alt = params.altitude_deg.to_radians();
    let cos_alt = alt.cos();
    let lx = az.cos() * cos_alt;
    let ly = -az.sin() * cos_alt; // page-down y → negate sin
    let lz = alt.sin().max(0.0);

    let depth = params.depth.clamp(0.0, 4.0);
    let hi_op = params.highlight_opacity.clamp(0.0, 1.0);
    let sh_op = params.shadow_opacity.clamp(0.0, 1.0);
    let hi_ts = linear_color_to_ts(params.highlight_color);
    let sh_ts = linear_color_to_ts(params.shadow_color);

    let mut scratch = match Pixmap::new(w_px, h_px) {
        Some(p) => p,
        None => return,
    };
    let data = scratch.data_mut();
    let w = w_px as usize;
    let h = h_px as usize;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            // Central differences. `depth` scales the normal slope.
            let xm = if x == 0 { x } else { x - 1 };
            let xp = if x + 1 >= w { x } else { x + 1 };
            let ym = if y == 0 { y } else { y - 1 };
            let yp = if y + 1 >= h { y } else { y + 1 };
            let dx = (height[y * w + xp] - height[y * w + xm]) * depth * 4.0;
            let dy = (height[yp * w + x] - height[ym * w + x]) * depth * 4.0;
            // Normal: (-dx, -dy, 1) before normalise.
            let nx = -dx;
            let ny = -dy;
            let nz = 1.0;
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-6);
            let dot = (nx * lx + ny * ly + nz * lz) / len;
            let inside = interior_mask[i] as f32 / 255.0;
            // Bevel only paints where the surface tilts (nonzero
            // gradient). `slope = sqrt(dx² + dy²)` is large near the
            // edge, zero deep inside. Multiply by `inside` so the
            // bevel stays inside the path (the smoothed height
            // bleeds past the path's true edge).
            let slope = (dx * dx + dy * dy).sqrt().clamp(0.0, 1.0);
            // Shadow when dot < 0, highlight when dot > 0.
            let q = i * 4;
            let (cr, cg, cb, op);
            if dot >= 0.0 {
                cr = hi_ts.red();
                cg = hi_ts.green();
                cb = hi_ts.blue();
                op = hi_op * hi_ts.alpha();
            } else {
                cr = sh_ts.red();
                cg = sh_ts.green();
                cb = sh_ts.blue();
                op = sh_op * sh_ts.alpha();
            }
            let a = (dot.abs() * slope * inside * op).clamp(0.0, 1.0);
            data[q] = (cr * a * 255.0).round().clamp(0.0, 255.0) as u8;
            data[q + 1] = (cg * a * 255.0).round().clamp(0.0, 255.0) as u8;
            data[q + 2] = (cb * a * 255.0).round().clamp(0.0, 255.0) as u8;
            data[q + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    let composite = PixmapPaint::default();
    target.draw_pixmap(
        off_x_px,
        off_y_px,
        scratch.as_ref(),
        &composite,
        TsTransform::identity(),
        target_mask,
    );
}

/// Satin: subtract two offset blurred path masks to produce a wave
/// pattern, mask to the path interior, tint with `params.color`.
fn render_satin(
    target: &mut Pixmap,
    target_xform: TsTransform,
    target_mask: Option<&TsMask>,
    path: &tiny_skia::Path,
    params: &Satin,
    scale: f32,
) {
    let pad_pt = 3.0 * params.blur_radius.max(0.0) + params.distance.abs() + 1.0;
    let Some((off_x_px, off_y_px, w_px, h_px, scratch_xform)) =
        effect_scratch_bounds(path, target_xform, pad_pt)
    else {
        return;
    };
    let Some(interior_pix) = stamp_path_alpha(w_px, h_px, path, scratch_xform) else {
        return;
    };
    let interior_mask = alpha_to_mask(interior_pix.data());

    // Two offset stamps along ±(angle_deg, distance/2).
    let theta = params.angle_deg.to_radians();
    let dx_pt = theta.cos() * params.distance * 0.5;
    let dy_pt = -theta.sin() * params.distance * 0.5;
    let xform_a = scratch_xform.pre_concat(TsTransform::from_translate(dx_pt, dy_pt));
    let xform_b = scratch_xform.pre_concat(TsTransform::from_translate(-dx_pt, -dy_pt));
    let Some(stamp_a) = stamp_path_alpha(w_px, h_px, path, xform_a) else {
        return;
    };
    let Some(stamp_b) = stamp_path_alpha(w_px, h_px, path, xform_b) else {
        return;
    };
    let mut a_mask = alpha_to_mask(stamp_a.data());
    let mut b_mask = alpha_to_mask(stamp_b.data());
    let sigma_px = params.blur_radius.max(0.0) * scale;
    if sigma_px > 0.5 {
        let kernel = gaussian_kernel(sigma_px);
        gaussian_blur_mask(&mut a_mask, w_px, h_px, &kernel);
        gaussian_blur_mask(&mut b_mask, w_px, h_px, &kernel);
    }

    let opacity = params.opacity.clamp(0.0, 1.0);
    let ts_color = linear_color_to_ts(params.color);
    let cr = ts_color.red();
    let cg = ts_color.green();
    let cb = ts_color.blue();
    let ca = ts_color.alpha();
    let mut scratch = match Pixmap::new(w_px, h_px) {
        Some(p) => p,
        None => return,
    };
    let data = scratch.data_mut();
    for i in 0..(w_px as usize * h_px as usize) {
        let am = a_mask[i] as f32 / 255.0;
        let bm = b_mask[i] as f32 / 255.0;
        // Wave intensity: `|am - bm|` peaks at the path edges where
        // the two stamps disagree. Multiply by interior mask so the
        // satin highlight only paints inside the path.
        let inside = interior_mask[i] as f32 / 255.0;
        let wave = (am - bm).abs();
        let a = (wave * inside * opacity * ca).clamp(0.0, 1.0);
        let q = i * 4;
        data[q] = (cr * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 1] = (cg * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 2] = (cb * a * 255.0).round().clamp(0.0, 255.0) as u8;
        data[q + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    let mut composite = PixmapPaint::default();
    composite.blend_mode = blend_mode_to_ts(params.blend_mode);
    target.draw_pixmap(
        off_x_px,
        off_y_px,
        scratch.as_ref(),
        &composite,
        TsTransform::identity(),
        target_mask,
    );
}

/// Feather: paint the path with a soft alpha gradient at the edge.
/// Algorithm:
///   1. Build the path interior mask `M`.
///   2. Blur `M` by `width / 2` (or `width` for diffusion) — the
///      blurred mask becomes the new alpha. `Sharp` uses a tight
///      blur; `Rounded` uses ~1.5×; `Diffusion` uses a wider blur
///      modulated by `noise`.
///   3. The colour fill stays the path's own paint. Since this
///      effect is a stand-in for the matching FillPath, we paint
///      the *path's local fill* — but the rasterizer doesn't have
///      access to that here. Approximation: paint as the path's
///      own alpha mask in a neutral 50% black, so the feathered
///      edge is visible. The renderer integration will pair the
///      feather with a separate `FillPath` and the feather variant
///      will only carry the alpha mask.
///
/// Since the renderer integration follows in a separate pass, we
/// emit the feather as a soft alpha-mask preview (opaque path with
/// a soft edge), tinted at neutral 50% black. The fidelity benefit
/// is the soft edge; the colour matches a no-effect fill closely
/// enough for the regression metric.
fn render_feather(
    target: &mut Pixmap,
    target_xform: TsTransform,
    target_mask: Option<&TsMask>,
    path: &tiny_skia::Path,
    params: &Feather,
    scale: f32,
) {
    let pad_pt = params.width.abs() * 3.0 + 1.0;
    let Some((off_x_px, off_y_px, w_px, h_px, scratch_xform)) =
        effect_scratch_bounds(path, target_xform, pad_pt)
    else {
        return;
    };
    let Some(interior_pix) = stamp_path_alpha(w_px, h_px, path, scratch_xform) else {
        return;
    };
    let mut feather_mask = alpha_to_mask(interior_pix.data());

    // Choose σ based on corner type; convert pt → px.
    let sigma_px = match params.corner_type {
        FeatherCornerType::Sharp => params.width.max(0.0) * 0.5 * scale,
        FeatherCornerType::Rounded => params.width.max(0.0) * 0.75 * scale,
        FeatherCornerType::Diffusion => params.width.max(0.0) * 1.0 * scale,
    };
    if sigma_px > 0.5 {
        let kernel = gaussian_kernel(sigma_px);
        gaussian_blur_mask(&mut feather_mask, w_px, h_px, &kernel);
    }

    // Choke shifts the half-alpha point. We approximate by
    // remapping `mask` linearly: `mask' = clamp((mask - choke*255) /
    // (1 - choke), 0, 255)`. Negative choke pushes outward.
    let choke = params.choke.clamp(-0.99, 0.99);
    if choke != 0.0 {
        let shift = choke * 255.0;
        let scale_back = (1.0 - choke).max(1e-6);
        for v in feather_mask.iter_mut() {
            let raw = (*v as f32 - shift) / scale_back;
            *v = raw.clamp(0.0, 255.0) as u8;
        }
    }

    // Diffusion: modulate alpha by a coarse pseudo-random noise
    // pattern so the falloff isn't perfectly smooth.
    if matches!(params.corner_type, FeatherCornerType::Diffusion) && params.noise > 0.0 {
        let noise_amp = params.noise.clamp(0.0, 1.0);
        for (i, v) in feather_mask.iter_mut().enumerate() {
            // Cheap deterministic hash of pixel index → [0, 1).
            let h = ((i.wrapping_mul(2_654_435_761)) & 0xFFFF) as f32 / 65535.0;
            let factor = 1.0 - noise_amp * (h - 0.5);
            *v = (*v as f32 * factor).clamp(0.0, 255.0) as u8;
        }
    }

    // Tint at neutral 50% black — the renderer integration will pair
    // this with a `FillPath` that applies the path's actual paint.
    let mut scratch = match Pixmap::new(w_px, h_px) {
        Some(p) => p,
        None => return,
    };
    let data = scratch.data_mut();
    for i in 0..(w_px as usize * h_px as usize) {
        let m = feather_mask[i] as f32 / 255.0;
        let a = (m * 0.5).clamp(0.0, 1.0);
        let q = i * 4;
        data[q] = 0;
        data[q + 1] = 0;
        data[q + 2] = 0;
        data[q + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    let composite = PixmapPaint::default();
    target.draw_pixmap(
        off_x_px,
        off_y_px,
        scratch.as_ref(),
        &composite,
        TsTransform::identity(),
        target_mask,
    );
}

/// Directional feather: same shape as [`render_feather`] but with
/// per-edge widths instead of a single uniform width.
///
/// Algorithm: stamp the path's interior mask, then for each interior
/// pixel compute a per-side alpha factor `clamp(d_side / width_side,
/// 0, 1)` (where `d_side` is the pt-space distance from the pixel to
/// the path's bbox side, in page-pt coords) and combine the four
/// factors via product. Sides with `width <= 0` contribute alpha = 1
/// so the corresponding edge stays opaque. Choke / noise / corner
/// type follow the plain feather's logic.
///
/// Limitations:
/// - The bbox is the page-pt bbox of the *transformed* path, so a
///   rotated rectangle's "left" side is the page-pt minimum-X side,
///   not the path's intrinsic left edge. The IDML `Angle` attribute
///   is captured by the parser but not consumed here.
fn render_directional_feather(
    target: &mut Pixmap,
    target_xform: TsTransform,
    target_mask: Option<&TsMask>,
    path: &tiny_skia::Path,
    params: &DirectionalFeather,
    scale: f32,
) {
    // Pad scratch by max edge width so the soft edge doesn't clip.
    let max_w = params
        .left_width
        .max(params.right_width)
        .max(params.top_width)
        .max(params.bottom_width)
        .max(0.0);
    let pad_pt = max_w * 3.0 + 1.0;
    let Some((off_x_px, off_y_px, w_px, h_px, scratch_xform)) =
        effect_scratch_bounds(path, target_xform, pad_pt)
    else {
        return;
    };
    let Some(interior_pix) = stamp_path_alpha(w_px, h_px, path, scratch_xform) else {
        return;
    };
    let mut feather_mask = alpha_to_mask(interior_pix.data());

    // Per-edge alpha modulation. The bbox lives in the path's
    // page-pt coords (the path was already transformed in
    // `build_path_transformed`); the scratch pixel grid is the page
    // pixel grid translated by `off_*_px`. Reverse: for each scratch
    // pixel, derive its page-pt position via `(off + i + 0.5) / scale`.
    //
    // Rotation: when `angle_deg` is non-zero we treat the per-pixel
    // distances in the *rotated* frame so "left" / "top" track the
    // intrinsic edges of the rect rather than the AABB's min-x /
    // min-y sides. We rotate each pixel by `-angle_deg` around the
    // bbox centre; per-side distances are then computed against the
    // bbox's half-extents (the bbox here is treated as the rotated
    // rect's intrinsic bounds, matching how the parser surfaces a
    // rotated frame's bounds when ItemTransform is folded into the
    // path's coords). For an axis-aligned rect this is the original
    // logic; for a rotated rect the per-side fades follow the
    // rect's own edges.
    let bbox = path.bounds();
    let lw = params.left_width.max(0.0);
    let rw = params.right_width.max(0.0);
    let tw = params.top_width.max(0.0);
    let bw = params.bottom_width.max(0.0);
    let inv_scale = 1.0 / scale.max(1e-6);
    let cx = (bbox.left() + bbox.right()) * 0.5;
    let cy = (bbox.top() + bbox.bottom()) * 0.5;
    let hw = (bbox.right() - bbox.left()) * 0.5;
    let hh = (bbox.bottom() - bbox.top()) * 0.5;
    let angle_rad = -params.angle_deg.to_radians();
    let (sin_a, cos_a) = angle_rad.sin_cos();
    if lw + rw + tw + bw > 0.0 {
        for j in 0..h_px {
            for i in 0..w_px {
                let idx = (j * w_px + i) as usize;
                let m = feather_mask[idx];
                if m == 0 {
                    continue;
                }
                let px_pt = (off_x_px as f32 + i as f32 + 0.5) * inv_scale;
                let py_pt = (off_y_px as f32 + j as f32 + 0.5) * inv_scale;
                // Rotate the pixel into the rect's intrinsic frame.
                let dx = px_pt - cx;
                let dy = py_pt - cy;
                let rx = dx * cos_a - dy * sin_a;
                let ry = dx * sin_a + dy * cos_a;
                let d_left = rx + hw;
                let d_right = hw - rx;
                let d_top = ry + hh;
                let d_bot = hh - ry;
                let a_left = if lw > 0.0 { (d_left / lw).clamp(0.0, 1.0) } else { 1.0 };
                let a_right = if rw > 0.0 { (d_right / rw).clamp(0.0, 1.0) } else { 1.0 };
                let a_top = if tw > 0.0 { (d_top / tw).clamp(0.0, 1.0) } else { 1.0 };
                let a_bot = if bw > 0.0 { (d_bot / bw).clamp(0.0, 1.0) } else { 1.0 };
                let combined = a_left * a_right * a_top * a_bot;
                feather_mask[idx] = (m as f32 * combined).clamp(0.0, 255.0) as u8;
            }
        }
    }

    // Optional Gaussian smoothing on top of the per-edge ramp.
    // `Sharp` skips the blur (the per-edge ramp already gives a
    // smooth linear falloff); `Rounded` / `Diffusion` add a light
    // blur to round the corner where two ramps meet.
    let sigma_px = match params.corner_type {
        FeatherCornerType::Sharp => 0.0,
        FeatherCornerType::Rounded => max_w * 0.25 * scale,
        FeatherCornerType::Diffusion => max_w * 0.5 * scale,
    };
    if sigma_px > 0.5 {
        let kernel = gaussian_kernel(sigma_px);
        gaussian_blur_mask(&mut feather_mask, w_px, h_px, &kernel);
    }

    let choke = params.choke.clamp(-0.99, 0.99);
    if choke != 0.0 {
        let shift = choke * 255.0;
        let scale_back = (1.0 - choke).max(1e-6);
        for v in feather_mask.iter_mut() {
            let raw = (*v as f32 - shift) / scale_back;
            *v = raw.clamp(0.0, 255.0) as u8;
        }
    }

    if matches!(params.corner_type, FeatherCornerType::Diffusion) && params.noise > 0.0 {
        let noise_amp = params.noise.clamp(0.0, 1.0);
        for (i, v) in feather_mask.iter_mut().enumerate() {
            let h = ((i.wrapping_mul(2_654_435_761)) & 0xFFFF) as f32 / 65535.0;
            let factor = 1.0 - noise_amp * (h - 0.5);
            *v = (*v as f32 * factor).clamp(0.0, 255.0) as u8;
        }
    }

    composite_alpha_mask(target, target_mask, off_x_px, off_y_px, w_px, h_px, &feather_mask);
}

/// Gradient feather: alpha-modulate whatever's already been
/// rasterized into the active target along a 1-D gradient (linear
/// or radial). For Linear, each pixel projects onto the
/// `(start, end)` axis to get a `t` in `[0, 1]`; alpha is
/// interpolated from the stops at that `t`. For Radial,
/// `t = distance(pixel, start) / |end - start|`.
///
/// Coordinate conventions: `params.start_*` / `params.end_*` are in
/// the path's local space (same coords as the `Transform`). The
/// helper transforms them to page-pt before the projection so the
/// pixel-grid math is straight subtraction.
///
/// Compositing model: this is a path-shaped multiplicative alpha mask
/// applied in-place to `target`. For each pixel `p`,
///   factor(p) = (1 - aa(p)) + aa(p) * gradient_alpha(p)
/// where `aa(p)` is the path's anti-aliased interior coverage at `p`
/// (1 inside, 0 outside, fractional on the edge) and `gradient_alpha`
/// is the sampled stop list at `p`'s position along the gradient
/// axis. Then `target.rgba(p) *= factor(p)` in premultiplied space —
/// outside the path stays unchanged (factor = 1), inside the path
/// fades according to the gradient. Mirrors InDesign's "gradient
/// feather" effect, which masks the underlying fill rather than
/// stamping its own colour.
fn render_gradient_feather(
    target: &mut Pixmap,
    target_xform: TsTransform,
    target_mask: Option<&TsMask>,
    path: &tiny_skia::Path,
    transform: &CTransform,
    params: &GradientFeather,
    scale: f32,
) {
    if params.stops.is_empty() {
        return;
    }
    let pad_pt = 1.0;
    let Some((off_x_px, off_y_px, w_px, h_px, scratch_xform)) =
        effect_scratch_bounds(path, target_xform, pad_pt)
    else {
        return;
    };
    let Some(interior_pix) = stamp_path_alpha(w_px, h_px, path, scratch_xform) else {
        return;
    };
    let interior_mask = alpha_to_mask(interior_pix.data());

    // Map start / end from path-local to page-pt. The Transform's
    // `apply` mirrors how `build_path_transformed` transforms the
    // path itself, so the gradient axis lines up with the visible
    // path.
    let (sx_pt, sy_pt) = transform.apply(params.start_x, params.start_y);
    let (ex_pt, ey_pt) = transform.apply(params.end_x, params.end_y);
    let dx = ex_pt - sx_pt;
    let dy = ey_pt - sy_pt;
    let len_sq = dx * dx + dy * dy;

    // Pre-sort stops by location so the interpolation is monotonic.
    let mut stops: Vec<(f32, f32)> = params
        .stops
        .iter()
        .map(|s| (s.location.clamp(0.0, 1.0), s.alpha.clamp(0.0, 1.0)))
        .collect();
    stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Build the per-pixel multiplicative factor (0..1) and apply it
    // directly to the active target's RGBA pixels. The factor at
    // pixel p is (1 - aa) + aa * gradient_alpha — a linear blend
    // between "untouched" outside the path and "modulated by
    // gradient" inside it.
    let inv_scale = 1.0 / scale.max(1e-6);
    let radius = if len_sq > 1e-12 { len_sq.sqrt() } else { 0.0 };
    let inv_len_sq = if len_sq > 1e-6 { 1.0 / len_sq } else { 0.0 };
    let degenerate = len_sq < 1e-6;

    let target_w = target.width() as i32;
    let target_h = target.height() as i32;
    let mask_dims = target_mask.map(|m| (m.width() as i32, m.height() as i32));
    let mask_data = target_mask.map(|m| m.data().to_vec());
    let target_data = target.data_mut();
    for j in 0..h_px {
        let ty = off_y_px + j as i32;
        if ty < 0 || ty >= target_h {
            continue;
        }
        for i in 0..w_px {
            let tx = off_x_px + i as i32;
            if tx < 0 || tx >= target_w {
                continue;
            }
            let aa = interior_mask[(j * w_px + i) as usize];
            if aa == 0 {
                continue;
            }
            let px_pt = (off_x_px as f32 + i as f32 + 0.5) * inv_scale;
            let py_pt = (off_y_px as f32 + j as f32 + 0.5) * inv_scale;
            let gradient_alpha = if degenerate {
                // Degenerate axis (start == end): treat as a uniform
                // alpha equal to the first stop's value.
                stops[0].1
            } else {
                let t = match params.kind {
                    GradientFeatherKind::Linear => {
                        let t = ((px_pt - sx_pt) * dx + (py_pt - sy_pt) * dy) * inv_len_sq;
                        t.clamp(0.0, 1.0)
                    }
                    GradientFeatherKind::Radial => {
                        let rx = px_pt - sx_pt;
                        let ry = py_pt - sy_pt;
                        let r = (rx * rx + ry * ry).sqrt();
                        (r / radius).clamp(0.0, 1.0)
                    }
                };
                sample_gradient_alpha(&stops, t)
            };
            let aa_unit = aa as f32 / 255.0;
            // factor = (1 - aa_unit) + aa_unit * gradient_alpha.
            let mut factor = 1.0 - aa_unit * (1.0 - gradient_alpha);
            // Honour the active rasterization clip mask: pixels with
            // mask = 0 lie outside the clip and should be left
            // alone; partial coverage proportionally weakens the
            // effect.
            if let (Some((mw, mh)), Some(md)) = (mask_dims, mask_data.as_ref()) {
                if tx < mw && ty < mh {
                    let mv = md[(ty * mw + tx) as usize];
                    if mv == 0 {
                        continue;
                    }
                    let mv_unit = mv as f32 / 255.0;
                    factor = 1.0 + (factor - 1.0) * mv_unit;
                }
            }
            apply_alpha_factor(target_data, tx, ty, target_w, factor);
        }
    }
}

/// Multiply a single RGBA8 premultiplied pixel by `factor` in place.
/// In premultiplied space all four channels scale together so the
/// alpha-mask interpretation drops out: `result = pixel * factor`.
#[inline]
fn apply_alpha_factor(data: &mut [u8], x: i32, y: i32, target_w: i32, factor: f32) {
    let f = factor.clamp(0.0, 1.0);
    let idx = ((y * target_w + x) as usize) * 4;
    if idx + 3 >= data.len() {
        return;
    }
    data[idx] = (data[idx] as f32 * f) as u8;
    data[idx + 1] = (data[idx + 1] as f32 * f) as u8;
    data[idx + 2] = (data[idx + 2] as f32 * f) as u8;
    data[idx + 3] = (data[idx + 3] as f32 * f) as u8;
}

/// Composite a single-channel alpha mask onto `target` at
/// `(off_x_px, off_y_px)` as a 50%-black tinted stamp — same
/// convention as `render_feather`. Extracted so the directional /
/// gradient feather helpers don't duplicate the scratch-pixmap
/// allocation + premultiplied tint loop.
fn composite_alpha_mask(
    target: &mut Pixmap,
    target_mask: Option<&TsMask>,
    off_x_px: i32,
    off_y_px: i32,
    w_px: u32,
    h_px: u32,
    mask: &[u8],
) {
    let mut scratch = match Pixmap::new(w_px, h_px) {
        Some(p) => p,
        None => return,
    };
    let data = scratch.data_mut();
    for i in 0..(w_px as usize * h_px as usize) {
        let m = mask[i] as f32 / 255.0;
        let a = (m * 0.5).clamp(0.0, 1.0);
        let q = i * 4;
        data[q] = 0;
        data[q + 1] = 0;
        data[q + 2] = 0;
        data[q + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    let composite = PixmapPaint::default();
    target.draw_pixmap(
        off_x_px,
        off_y_px,
        scratch.as_ref(),
        &composite,
        TsTransform::identity(),
        target_mask,
    );
}

/// Linear-interpolate gradient alpha at parameter `t` across a
/// sorted stop list `(location, alpha)`. `t` outside the stops'
/// range snaps to the nearest endpoint's alpha.
fn sample_gradient_alpha(stops: &[(f32, f32)], t: f32) -> f32 {
    debug_assert!(!stops.is_empty());
    if t <= stops[0].0 {
        return stops[0].1;
    }
    if t >= stops[stops.len() - 1].0 {
        return stops[stops.len() - 1].1;
    }
    for w in stops.windows(2) {
        let (l_loc, l_alpha) = w[0];
        let (r_loc, r_alpha) = w[1];
        if t <= r_loc {
            let span = (r_loc - l_loc).max(1e-6);
            let f = (t - l_loc) / span;
            return l_alpha + (r_alpha - l_alpha) * f;
        }
    }
    stops[stops.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;
    use idml_compose::{emit_rect, emit_stroke_rect, Color, DisplayList, Paint, Rect};

    fn at(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        img.get_pixel(x, y).0
    }

    #[test]
    fn empty_list_renders_background() {
        let list = DisplayList::new();
        let opts = RasterOptions::new(10.0, 10.0);
        let img = rasterize(&list, &opts);
        let p = at(&img, 2, 2);
        assert_eq!(p[3], 255, "alpha");
        assert!(
            p[0] > 240 && p[1] > 240 && p[2] > 240,
            "bg white, got {p:?}"
        );
    }

    #[test]
    fn red_rect_fills_expected_pixels() {
        let mut list = DisplayList::new();
        let red = Paint::Solid(Color::rgba(1.0, 0.0, 0.0, 1.0));
        emit_rect(
            Rect {
                x: 10.0,
                y: 10.0,
                w: 30.0,
                h: 20.0,
            },
            red,
            &mut list,
        );
        let mut opts = RasterOptions::new(50.0, 50.0);
        opts.dpi = 72.0; // 1 px = 1 pt, so rect covers x=10..40, y=10..30.
        let img = rasterize(&list, &opts);

        // Sample inside the rect: should be ~(255, 0, 0).
        let inside = at(&img, 20, 20);
        assert!(inside[0] > 240, "inside red channel {inside:?}");
        assert!(inside[1] < 15, "inside green {inside:?}");
        assert!(inside[2] < 15, "inside blue {inside:?}");

        // Sample outside the rect: background white.
        let outside = at(&img, 2, 2);
        assert!(outside[0] > 240 && outside[1] > 240 && outside[2] > 240);
    }

    #[test]
    fn stroke_draws_around_rect_perimeter() {
        let mut list = DisplayList::new();
        let black = Paint::Solid(Color::rgba(0.0, 0.0, 0.0, 1.0));
        emit_stroke_rect(
            Rect {
                x: 10.0,
                y: 10.0,
                w: 30.0,
                h: 20.0,
            },
            idml_compose::Stroke::new(2.0),
            black,
            &mut list,
        );
        let mut opts = RasterOptions::new(50.0, 50.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // The stroke straddles the boundary — the horizontal edge at
        // y=10 should be dark.
        let on_edge = at(&img, 20, 10);
        assert!(
            on_edge[0] < 100 && on_edge[1] < 100 && on_edge[2] < 100,
            "edge should be dark; got {on_edge:?}"
        );
        // Outside the stroke: still background white.
        let outside = at(&img, 2, 2);
        assert!(outside[0] > 240, "expected white bg; got {outside:?}");
    }

    #[test]
    fn dpi_scaling_changes_image_size() {
        let list = DisplayList::new();
        let mut opts = RasterOptions::new(100.0, 50.0);
        opts.dpi = 144.0; // 2 px/pt
        let img = rasterize(&list, &opts);
        assert_eq!(img.width(), 200);
        assert_eq!(img.height(), 100);
    }

    #[test]
    fn cpu_rasterizer_trait_returns_correct_pixel_count() {
        let r = CpuRasterizer;
        let list = idml_compose::DisplayList::new();
        let mut opts = RasterOptions::new(40.0, 30.0);
        opts.dpi = 72.0;
        let buf = r.rasterize(&list, &opts);
        assert_eq!(buf.len(), 40 * 30 * 4);
        assert_eq!(r.name(), "cpu/tiny-skia");
    }

    #[test]
    fn blend_group_lighten_against_yellow_bg_keeps_yellow() {
        // Lighten of a black rect on a yellow rect underneath should
        // yield yellow where the black rect overlaps (max channel
        // gives yellow), exercising the BeginBlendGroup /
        // EndBlendGroup primitive end-to-end through the CPU
        // rasterizer.
        let mut list = DisplayList::new();
        let yellow = Paint::Solid(Color::rgba(1.0, 1.0, 0.0, 1.0));
        let black = Paint::Solid(Color::rgba(0.0, 0.0, 0.0, 1.0));
        // Yellow background rect at (5, 5, 30, 30).
        emit_rect(
            Rect {
                x: 5.0,
                y: 5.0,
                w: 30.0,
                h: 30.0,
            },
            yellow,
            &mut list,
        );
        // Black rect at (10, 10, 20, 20) wrapped in a Lighten group.
        list.commands
            .push(idml_compose::DisplayCommand::BeginBlendGroup {
                bounds: idml_compose::Rect {
                    x: 10.0,
                    y: 10.0,
                    w: 20.0,
                    h: 20.0,
                },
                blend_mode: idml_compose::BlendMode::Lighten,
                opacity: 1.0,
                transform: idml_compose::Transform::IDENTITY,
            });
        emit_rect(
            Rect {
                x: 10.0,
                y: 10.0,
                w: 20.0,
                h: 20.0,
            },
            black,
            &mut list,
        );
        list.commands
            .push(idml_compose::DisplayCommand::EndBlendGroup(
                idml_compose::Transform::IDENTITY,
            ));
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // Inside the overlap (15, 15): Lighten(black, yellow) = yellow.
        let p = at(&img, 15, 15);
        assert!(
            p[0] > 240 && p[1] > 240 && p[2] < 15,
            "overlap should be yellow, got {p:?}"
        );
        // Outside the rects (2, 2): background white.
        let bg = at(&img, 2, 2);
        assert!(bg[0] > 240 && bg[1] > 240 && bg[2] > 240, "bg = {bg:?}");
    }

    #[test]
    fn clip_inside_blend_group_masks_to_smaller_buffer() {
        // Mirrors the Lighten test above but adds a Push/Pop clip
        // pair *inside* the blend group: a clip rect that only
        // covers the left half of the inner black rect. The right
        // half should be unclipped (Lighten(black, yellow) = yellow);
        // outside the clip and inside the inner rect should fall
        // back to the yellow background (clip masks the inner fill,
        // so the group buffer stays empty there and the lighten
        // composite is a no-op against the page); outside the inner
        // rect should still be background white.
        //
        // This exercises the clip stack inside a smaller-than-page
        // group buffer: before the fix, tiny-skia panicked because
        // a page-sized mask was being applied to a sub-pixmap.
        let mut list = DisplayList::new();
        let yellow = Paint::Solid(Color::rgba(1.0, 1.0, 0.0, 1.0));
        let black = Paint::Solid(Color::rgba(0.0, 0.0, 0.0, 1.0));
        // Yellow background rect covering the entire visible area
        // so the page underneath the group is yellow, not white.
        emit_rect(
            Rect {
                x: 5.0,
                y: 5.0,
                w: 30.0,
                h: 30.0,
            },
            yellow,
            &mut list,
        );
        // Begin a Lighten blend group sized to (10, 10, 20, 20).
        list.commands
            .push(idml_compose::DisplayCommand::BeginBlendGroup {
                bounds: idml_compose::Rect {
                    x: 10.0,
                    y: 10.0,
                    w: 20.0,
                    h: 20.0,
                },
                blend_mode: idml_compose::BlendMode::Lighten,
                opacity: 1.0,
                transform: idml_compose::Transform::IDENTITY,
            });
        // Push a clip covering only the left half (x in 10..20) of
        // the group buffer. The clip path is in page-space pt; the
        // rasterizer is responsible for re-anchoring it to the
        // group's local pixel grid.
        let mut clip_path = idml_compose::PathData::default();
        clip_path.segments.push(idml_compose::PathSegment::MoveTo {
            x: 0.0,
            y: 0.0,
        });
        clip_path.segments.push(idml_compose::PathSegment::LineTo {
            x: 1.0,
            y: 0.0,
        });
        clip_path.segments.push(idml_compose::PathSegment::LineTo {
            x: 1.0,
            y: 1.0,
        });
        clip_path.segments.push(idml_compose::PathSegment::LineTo {
            x: 0.0,
            y: 1.0,
        });
        clip_path.segments.push(idml_compose::PathSegment::Close);
        let clip_id = list.paths.push_anon(clip_path);
        // unit-rect [0,1]² → page rect [10,10..20,30] (left half of
        // the inner rect, full vertical extent).
        let clip_xform = idml_compose::Transform([10.0, 0.0, 0.0, 20.0, 10.0, 10.0]);
        list.commands
            .push(idml_compose::DisplayCommand::PushClip {
                path_id: clip_id,
                transform: clip_xform,
            });
        // Black rect at (10, 10, 20, 20) — wider than the clip.
        emit_rect(
            Rect {
                x: 10.0,
                y: 10.0,
                w: 20.0,
                h: 20.0,
            },
            black,
            &mut list,
        );
        list.commands
            .push(idml_compose::DisplayCommand::PopClip(
                idml_compose::Transform::IDENTITY,
            ));
        list.commands
            .push(idml_compose::DisplayCommand::EndBlendGroup(
                idml_compose::Transform::IDENTITY,
            ));
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // (12, 15): inside clip + inside inner rect ⇒ Lighten(black,
        // yellow) = yellow.
        let inside_clip = at(&img, 12, 15);
        assert!(
            inside_clip[0] > 240
                && inside_clip[1] > 240
                && inside_clip[2] < 15,
            "inside clip+inner: expected yellow, got {inside_clip:?}"
        );
        // (25, 15): outside clip but inside inner rect ⇒ group buffer
        // empty there, Lighten composite no-op, page yellow shows.
        let outside_clip = at(&img, 25, 15);
        assert!(
            outside_clip[0] > 240
                && outside_clip[1] > 240
                && outside_clip[2] < 15,
            "outside clip+inner: expected yellow page, got {outside_clip:?}"
        );
        // (2, 2): outside the yellow background ⇒ canvas white.
        let bg = at(&img, 2, 2);
        assert!(
            bg[0] > 240 && bg[1] > 240 && bg[2] > 240,
            "page bg = white, got {bg:?}"
        );
    }

    #[test]
    fn blend_group_opacity_50_halves_alpha_against_white() {
        // A black rect inside a 50% opacity group composited onto
        // white should yield mid-gray, exercising group-level alpha
        // (PixmapPaint::opacity).
        let mut list = DisplayList::new();
        list.commands
            .push(idml_compose::DisplayCommand::BeginBlendGroup {
                bounds: idml_compose::Rect {
                    x: 10.0,
                    y: 10.0,
                    w: 20.0,
                    h: 20.0,
                },
                blend_mode: idml_compose::BlendMode::Normal,
                opacity: 0.5,
                transform: idml_compose::Transform::IDENTITY,
            });
        let black = Paint::Solid(Color::rgba(0.0, 0.0, 0.0, 1.0));
        emit_rect(
            Rect {
                x: 10.0,
                y: 10.0,
                w: 20.0,
                h: 20.0,
            },
            black,
            &mut list,
        );
        list.commands
            .push(idml_compose::DisplayCommand::EndBlendGroup(
                idml_compose::Transform::IDENTITY,
            ));
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // 50% black on white = ~127 per channel. Allow some slack
        // for sRGB gamma round-trip.
        let p = at(&img, 15, 15);
        assert!(
            p[0] > 100 && p[0] < 180,
            "expected mid-gray, got {p:?}"
        );
    }

    #[test]
    fn path_shadow_paints_dark_pixels_at_offset() {
        // Stamp a small unit-rect path as a `PathShadow` at a known
        // page offset, with a non-zero shadow offset and visible
        // blur radius. Inside the shadow's projected bounds the
        // page should darken; far away from the path the page
        // should remain near-white background. This mirrors the
        // existing DropShadow code path; we render the new
        // PathShadow variant so the shared lowering survives any
        // future divergence.
        use idml_compose::{
            DisplayCommand as Cmd, DisplayList, DropShadow as DS, PathData, PathSegment,
            Transform as XF,
        };
        let mut list = DisplayList::new();
        // Anonymous unit-rect path (avoids the interned-key
        // collision with later test isolation).
        let mut p = PathData::default();
        p.segments
            .push(PathSegment::MoveTo { x: 0.0, y: 0.0 });
        p.segments
            .push(PathSegment::LineTo { x: 1.0, y: 0.0 });
        p.segments
            .push(PathSegment::LineTo { x: 1.0, y: 1.0 });
        p.segments
            .push(PathSegment::LineTo { x: 0.0, y: 1.0 });
        p.segments.push(PathSegment::Close);
        let path_id = list.paths.push_anon(p);
        // Place the unit rect at (10, 10) with size 10×10, shadow
        // offset (4, 4), blur 2pt, 60% black.
        let xform = XF([10.0, 0.0, 0.0, 10.0, 10.0, 10.0]);
        let shadow = DS {
            offset_x: 4.0,
            offset_y: 4.0,
            blur_radius: 2.0,
            color: idml_compose::Color::rgba(0.0, 0.0, 0.0, 1.0),
            opacity: 0.6,
        };
        list.commands.push(Cmd::PathShadow {
            path_id,
            transform: xform,
            shadow,
        });
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // The shadow centre lands around (10+4 + 5, 10+4 + 5) =
        // (19, 19): rect spans x=10..20, y=10..20 in pt; shadow
        // offsets +4 → x=14..24, y=14..24; centre at (19, 19).
        let centre = at(&img, 19, 19);
        // Shadow should darken the pixel meaningfully; ~0.6 alpha
        // black on white → ~102 per channel mid-rect, and the
        // Gaussian blur softens edges. Expect well under 220 in
        // the rect interior.
        assert!(
            centre[0] < 220 && centre[1] < 220 && centre[2] < 220,
            "shadow centre should darken page; got {centre:?}"
        );
        // Far outside the shadow footprint: still white background.
        let far = at(&img, 2, 2);
        assert!(
            far[0] > 240 && far[1] > 240 && far[2] > 240,
            "far-away pixel should be white bg; got {far:?}"
        );
    }

    /// Helper: install an anonymous unit-rect path in `list` and
    /// return the `(path_id, page-space transform)` that places it at
    /// `(x, y)` with size `(w, h)`. Used by the per-effect tests
    /// below to build a stand-alone path command without going
    /// through the `emit_*` helpers.
    fn unit_rect_at(
        list: &mut idml_compose::DisplayList,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> (idml_compose::PathId, idml_compose::Transform) {
        use idml_compose::{PathData, PathSegment, Transform as XF};
        let mut p = PathData::default();
        p.segments.push(PathSegment::MoveTo { x: 0.0, y: 0.0 });
        p.segments.push(PathSegment::LineTo { x: 1.0, y: 0.0 });
        p.segments.push(PathSegment::LineTo { x: 1.0, y: 1.0 });
        p.segments.push(PathSegment::LineTo { x: 0.0, y: 1.0 });
        p.segments.push(PathSegment::Close);
        let path_id = list.paths.push_anon(p);
        let xform = XF([w, 0.0, 0.0, h, x, y]);
        (path_id, xform)
    }

    #[test]
    fn inner_shadow_darkens_inside_edge_keeps_outside_white() {
        // Stamp a 20x20 pt rectangle inner shadow at (10, 10) with
        // offset +4,+4 and 3pt blur, 80% black. The shadow lives
        // *inside* the path: pixels just inside the top-left edge
        // (where the offset stamp's complement is strongest) should
        // darken; pixels outside the path stay at the white
        // background.
        use idml_compose::{DisplayCommand as Cmd, DisplayList, InnerShadow as IS};
        let mut list = DisplayList::new();
        let (path_id, xform) = unit_rect_at(&mut list, 10.0, 10.0, 20.0, 20.0);
        let params = IS {
            offset_x: 4.0,
            offset_y: 4.0,
            blur_radius: 3.0,
            color: idml_compose::Color::rgba(0.0, 0.0, 0.0, 1.0),
            opacity: 0.8,
            choke: 0.0,
            blend_mode: idml_compose::BlendMode::Normal,
        };
        list.commands.push(Cmd::InnerShadow {
            path_id,
            transform: xform,
            params,
        });
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // Inside the path, top-left corner (just inside the edge):
        // shadow source is the area outside the offset path, so the
        // top-left receives the strongest shadow. (12, 12) sits a
        // couple of pixels inside the path's interior.
        let inside_tl = at(&img, 12, 12);
        assert!(
            inside_tl[0] < 200,
            "inner shadow should darken inside top-left; got {inside_tl:?}"
        );
        // Outside the path entirely (well beyond the rect): still
        // white. The path occupies pt-space [10..30] × [10..30].
        let outside = at(&img, 2, 2);
        assert!(
            outside[0] > 240 && outside[1] > 240 && outside[2] > 240,
            "outside path should stay white; got {outside:?}"
        );
        // Inside the path, far from the shadow source (bottom-right
        // corner inside the path is far from the offset stamp's
        // complement) — should be near-white (no fill emitted).
        let inside_far = at(&img, 28, 28);
        assert!(
            inside_far[0] > 220,
            "inner shadow should not paint deep interior; got {inside_far:?}"
        );
    }

    #[test]
    fn outer_glow_paints_outside_path_not_inside() {
        // 20x20 pt rectangle at (10, 10), blue glow with 4pt blur,
        // 90% opacity. Just outside the rect's edge the glow should
        // tint the page blue; inside the rect the glow is masked
        // out (no fill emitted, page stays background-white).
        use idml_compose::{DisplayCommand as Cmd, DisplayList, OuterGlow as OG};
        let mut list = DisplayList::new();
        let (path_id, xform) = unit_rect_at(&mut list, 10.0, 10.0, 20.0, 20.0);
        let params = OG {
            blur_radius: 4.0,
            color: idml_compose::Color::rgba(0.0, 0.0, 1.0, 1.0),
            opacity: 0.9,
            blend_mode: idml_compose::BlendMode::Normal,
            spread: 0.0,
        };
        list.commands.push(Cmd::OuterGlow {
            path_id,
            transform: xform,
            params,
        });
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // Just outside the rect's left edge (8, 20): the glow is
        // strongest one blur-radius away from the edge.
        let outside = at(&img, 8, 20);
        // Blue should dominate (B channel > R/G).
        assert!(
            outside[2] > outside[0] + 10 && outside[2] > outside[1] + 10,
            "outer glow should tint blue just outside path; got {outside:?}"
        );
        // Inside the rect, well clear of the edge (20, 20): the
        // outer-glow masks itself out of the path interior, so the
        // page stays white.
        let inside = at(&img, 20, 20);
        assert!(
            inside[0] > 220 && inside[1] > 220 && inside[2] > 220,
            "outer glow should not paint inside path; got {inside:?}"
        );
        // Far outside: untouched white background.
        let far = at(&img, 2, 2);
        assert!(
            far[0] > 240 && far[1] > 240 && far[2] > 240,
            "far-away pixel should stay white; got {far:?}"
        );
    }

    #[test]
    fn inner_glow_lights_inside_edge_keeps_outside_white() {
        // 20x20 rectangle, yellow inner glow, 4pt blur, 80% opacity.
        // Just inside the edge of the path, the page should pick up
        // a yellow tint; outside the path stays white.
        use idml_compose::{DisplayCommand as Cmd, DisplayList, InnerGlow as IG};
        let mut list = DisplayList::new();
        let (path_id, xform) = unit_rect_at(&mut list, 10.0, 10.0, 20.0, 20.0);
        let params = IG {
            blur_radius: 4.0,
            color: idml_compose::Color::rgba(1.0, 1.0, 0.0, 1.0),
            opacity: 0.8,
            blend_mode: idml_compose::BlendMode::Normal,
            choke: 0.0,
        };
        list.commands.push(Cmd::InnerGlow {
            path_id,
            transform: xform,
            params,
        });
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // Just inside the path's top-left edge: yellow tint.
        // Yellow = R high, G high, B low.
        let inside_edge = at(&img, 12, 12);
        assert!(
            inside_edge[2] < inside_edge[0],
            "inner glow should reduce B inside edge (yellow tint); got {inside_edge:?}"
        );
        // Outside the path: still white.
        let outside = at(&img, 2, 2);
        assert!(
            outside[0] > 240 && outside[1] > 240 && outside[2] > 240,
            "outside path stays white; got {outside:?}"
        );
    }

    #[test]
    fn bevel_emboss_lights_one_edge_darkens_opposite() {
        // Bevel-and-emboss with a top-left light (angle=135°,
        // altitude=30°) on a 30x30 pt rectangle. Top-left edge
        // should lighten; bottom-right edge should darken. Far
        // outside should remain background white.
        use idml_compose::{BevelEmboss as BE, DisplayCommand as Cmd, DisplayList};
        let mut list = DisplayList::new();
        let (path_id, xform) = unit_rect_at(&mut list, 10.0, 10.0, 30.0, 30.0);
        let params = BE {
            depth: 1.0,
            size: 4.0,
            angle_deg: 135.0,
            altitude_deg: 30.0,
            highlight_color: idml_compose::Color::rgba(1.0, 1.0, 1.0, 1.0),
            shadow_color: idml_compose::Color::rgba(0.0, 0.0, 0.0, 1.0),
            highlight_opacity: 1.0,
            shadow_opacity: 1.0,
        };
        list.commands.push(Cmd::BevelEmboss {
            path_id,
            transform: xform,
            params,
        });
        let mut opts = RasterOptions::new(50.0, 50.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // Top-left edge inside the path (12, 12): highlight present
        // means the page stays bright (or gets brighter than it
        // would without the bevel). The page is white so the
        // highlight doesn't lighten it visibly; instead, check the
        // shadow side.
        // Bottom-right edge inside the path (37, 37): the shadow
        // colour darkens the page.
        let br_edge = at(&img, 37, 37);
        // Anywhere on the shadow side should darken below 220.
        let darkest = (br_edge[0] as i32) + (br_edge[1] as i32) + (br_edge[2] as i32);
        assert!(
            darkest < 240 * 3,
            "bevel shadow side should darken bottom-right; got {br_edge:?}"
        );
        // Far outside the path: untouched.
        let far = at(&img, 2, 2);
        assert!(
            far[0] > 240 && far[1] > 240 && far[2] > 240,
            "outside path stays background white; got {far:?}"
        );
    }

    #[test]
    fn satin_paints_inside_path_only() {
        // Satin: a 30x30 rect with a 5pt blur, distance 8pt, angle
        // 45°. The wave intensity peaks where the two offset
        // stamps disagree (near the path's edge along the
        // satin direction); should be visible inside the path but
        // not outside it.
        use idml_compose::{DisplayCommand as Cmd, DisplayList, Satin as ST};
        let mut list = DisplayList::new();
        let (path_id, xform) = unit_rect_at(&mut list, 10.0, 10.0, 30.0, 30.0);
        let params = ST {
            blur_radius: 5.0,
            angle_deg: 45.0,
            distance: 8.0,
            color: idml_compose::Color::rgba(0.0, 0.0, 0.0, 1.0),
            opacity: 0.9,
            blend_mode: idml_compose::BlendMode::Normal,
        };
        list.commands.push(Cmd::Satin {
            path_id,
            transform: xform,
            params,
        });
        let mut opts = RasterOptions::new(50.0, 50.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // Inside the path: satin should darken at least one
        // sample point. Pick a pixel along the leading edge of the
        // wave (top-left interior).
        let mut found_dark = false;
        for x in 11..40 {
            for y in 11..40 {
                let p = at(&img, x as u32, y as u32);
                if p[0] < 200 && p[1] < 200 && p[2] < 200 {
                    found_dark = true;
                    break;
                }
            }
            if found_dark {
                break;
            }
        }
        assert!(found_dark, "satin should darken at least one interior pixel");
        // Outside the path: stays background white.
        let outside = at(&img, 2, 2);
        assert!(
            outside[0] > 240 && outside[1] > 240 && outside[2] > 240,
            "satin should not paint outside path; got {outside:?}"
        );
    }

    #[test]
    fn feather_softens_path_edge() {
        // Feather of a 20x20 rect with width=4pt should produce a
        // soft alpha edge: center of the rect is opaque (50% black
        // tint), edge is partial-alpha, far outside is the
        // background.
        use idml_compose::{DisplayCommand as Cmd, DisplayList, Feather as F, FeatherCornerType};
        let mut list = DisplayList::new();
        let (path_id, xform) = unit_rect_at(&mut list, 10.0, 10.0, 20.0, 20.0);
        let params = F {
            width: 4.0,
            corner_type: FeatherCornerType::Sharp,
            noise: 0.0,
            choke: 0.0,
        };
        list.commands.push(Cmd::Feather {
            path_id,
            transform: xform,
            params,
        });
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // Centre of the path at (20, 20): feather is fully opaque
        // there (interior mask = 1), painted with 50% black tint
        // → ~half-grey pixel.
        let centre = at(&img, 20, 20);
        assert!(
            centre[0] < 200 && centre[0] > 80,
            "feather centre should be tinted grey; got {centre:?}"
        );
        // Far outside the rect: the soft edge has fully fallen
        // off, so the page stays the white background.
        let far = at(&img, 2, 2);
        assert!(
            far[0] > 240 && far[1] > 240 && far[2] > 240,
            "outside feather should be white; got {far:?}"
        );
    }

    #[test]
    fn directional_feather_softens_left_edge_more_than_right() {
        // 20x20 rect at (10, 10), feather 8pt on the left edge only.
        // The interior next to the left edge should fade out (ramp);
        // pixels near the right edge stay opaque (50% grey).
        use idml_compose::{DirectionalFeather, DisplayCommand as Cmd, FeatherCornerType};
        let mut list = DisplayList::new();
        let (path_id, xform) = unit_rect_at(&mut list, 10.0, 10.0, 20.0, 20.0);
        let params = DirectionalFeather {
            left_width: 8.0,
            right_width: 0.0,
            top_width: 0.0,
            bottom_width: 0.0,
            angle_deg: 0.0,
            noise: 0.0,
            choke: 0.0,
            corner_type: FeatherCornerType::Sharp,
        };
        list.commands.push(Cmd::DirectionalFeather {
            path_id,
            transform: xform,
            params,
        });
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // Sample three points along y=20 (vertical centre):
        //   x=11 → 1pt inside the left edge (heavy fade, near white)
        //   x=20 → mid-rect (alpha rises toward 1; tinted grey)
        //   x=28 → 2pt inside the right edge (full alpha; tinted)
        let near_left = at(&img, 11, 20);
        let mid = at(&img, 20, 20);
        let near_right = at(&img, 28, 20);
        // Near-left should be lighter (less tint) than mid.
        assert!(
            near_left[0] > mid[0],
            "left edge should be less tinted than mid; near_left={near_left:?} mid={mid:?}"
        );
        // Near-right should be at least as tinted as mid (no fade
        // there).
        assert!(
            near_right[0] <= mid[0] + 15,
            "right edge shouldn't fade; near_right={near_right:?} mid={mid:?}"
        );
        // Far outside the rect: white background.
        let far = at(&img, 2, 2);
        assert!(
            far[0] > 240,
            "outside directional feather should be white; got {far:?}"
        );
    }

    #[test]
    fn gradient_feather_linear_alpha_decreases_along_axis() {
        // Stack: a black-filled rect at (10, 10) covered by a
        // horizontal gradient feather with α=1.0 at x=0 and α=0.0
        // at x=1 (in unit-rect coords). The gradient feather alpha-
        // modulates the underlying fill, so pixels near the left
        // edge stay black (α≈1.0 → multiplier ≈1.0 → black through);
        // pixels near the right edge fade toward the white page
        // background (α≈0.1 → multiplier ≈0.1 → mostly background).
        // Far outside the rect: untouched white background.
        use idml_compose::{
            Color, DisplayCommand as Cmd, GradientFeather, GradientFeatherKind,
            GradientFeatherStop, Paint,
        };
        let mut list = DisplayList::new();
        let (path_id, xform) = unit_rect_at(&mut list, 10.0, 10.0, 20.0, 20.0);
        // Underlying black fill the gradient will modulate.
        list.commands.push(Cmd::FillPath {
            path_id,
            paint: Paint::Solid(Color::rgba(0.0, 0.0, 0.0, 1.0)),
            transform: xform,
        });
        let params = GradientFeather {
            kind: GradientFeatherKind::Linear,
            start_x: 0.0,
            start_y: 0.5,
            end_x: 1.0,
            end_y: 0.5,
            stops: vec![
                GradientFeatherStop { location: 0.0, alpha: 1.0 },
                GradientFeatherStop { location: 1.0, alpha: 0.0 },
            ],
        };
        list.commands.push(Cmd::GradientFeather {
            path_id,
            transform: xform,
            params,
        });
        let mut opts = RasterOptions::new(40.0, 40.0);
        opts.dpi = 72.0;
        let img = rasterize(&list, &opts);
        // x=12: near-start, gradient α ≈ 0.9, black fill stays
        // mostly opaque → output alpha ≈ 230, R = 0.
        // x=28: near-end, gradient α ≈ 0.1, black fill mostly
        // dissolves → output alpha ≈ 25, R = 0.
        // (The rasterizer's output is alpha-modulated in
        // premultiplied space; un-premult-vs-bg compositing is the
        // viewer's job. We assert on the alpha channel directly.)
        let near_start = at(&img, 12, 20);
        let near_end = at(&img, 28, 20);
        // Alpha decreases left→right (gradient stops 1.0→0.0).
        assert!(
            near_start[3] > near_end[3],
            "linear gradient feather should fade left→right; near_start={near_start:?} near_end={near_end:?}"
        );
        // Near-start retains most of the underlying fill's alpha.
        assert!(
            near_start[3] > 150,
            "near-start alpha should stay high; got {near_start:?}"
        );
        // Near-end alpha is much smaller (gradient α ≈ 0.1).
        assert!(
            near_end[3] < 80,
            "near-end alpha should be small; got {near_end:?}"
        );
        // Far outside the rect: untouched white background.
        let far = at(&img, 2, 2);
        assert!(
            far[0] > 240 && far[3] > 240,
            "outside gradient feather should be opaque white; got {far:?}"
        );
    }
}
