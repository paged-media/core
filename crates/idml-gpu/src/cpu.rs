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
    BlendMode, Color as CComposeColor, DisplayCommand, DisplayList, LineCap, LineJoin, Paint,
    PathData, PathSegment, Transform as CTransform,
};
use image::{Rgba, RgbaImage};
use tiny_skia::{
    BlendMode as TsBlendMode, FillRule, GradientStop as TsGradientStop, LineCap as TsLineCap,
    LineJoin as TsLineJoin, LinearGradient as TsLinearGradient, Mask as TsMask, Paint as TsPaint,
    PathBuilder, Pixmap, PixmapPaint, Point as TsPoint, RadialGradient as TsRadialGradient, Shader,
    SpreadMode, Stroke as TsStroke, Transform as TsTransform,
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
struct GroupFrame {
    pixmap: Pixmap,
    /// Buffer top-left in page-pixel coords.
    offset: (i32, i32),
    blend_mode: TsBlendMode,
    opacity: f32,
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

                // σ in pt → σ in pixels via the renderer's pt→px scale.
                let sigma_px = shadow.blur_radius.max(0.0) * scale;
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
                    let pad_pt = 3.0 * shadow.blur_radius.max(0.0) + 1.0;
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
                        group_stack.push(GroupFrame {
                            pixmap: buf,
                            offset: (off_x_px, off_y_px),
                            blend_mode: blend_mode_to_ts(*blend_mode),
                            opacity: opacity.clamp(0.0, 1.0),
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
                            });
                        }
                    }
                }
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
                let parent_mask = clip_stack
                    .iter()
                    .rev()
                    .find(|e| e.scope == parent_scope)
                    .map(|e| &e.mask);
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
                } else {
                    pixmap.draw_pixmap(
                        off_x_px,
                        off_y_px,
                        group_pix.as_ref(),
                        &composite,
                        TsTransform::identity(),
                        parent_mask,
                    );
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
}
