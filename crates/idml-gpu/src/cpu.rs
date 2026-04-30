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
    // pushed clip up to and including that level. `None` ⇒ no clipping
    // (the common case). Push: clone the current top (or the first
    // mask is built from a fresh white pixmap and intersected with the
    // pushed path), then `intersect_path` it. Pop: pop the top.
    //
    // tiny-skia masks live in pixel space; we transform paths through
    // `page_to_px` when filling. For Push, intersect at pixel
    // resolution to inherit anti-alias behaviour.
    let mut clip_stack: Vec<TsMask> = Vec::new();
    fn active_mask(stack: &[TsMask]) -> Option<&TsMask> {
        stack.last()
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
                let ts_paint = paint_to_ts(paint, list, transform, page_to_px);
                pixmap.fill_path(
                    &path,
                    &ts_paint,
                    FillRule::Winding,
                    page_to_px,
                    active_mask(&clip_stack),
                );
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
                let ts_paint = paint_to_ts(paint, list, transform, page_to_px);
                let ts_mode = blend_mode_to_ts(*blend_mode);
                if matches!(ts_mode, TsBlendMode::SourceOver) {
                    // Normal blend ⇒ same fast path as FillPath.
                    pixmap.fill_path(
                        &path,
                        &ts_paint,
                        FillRule::Winding,
                        page_to_px,
                        active_mask(&clip_stack),
                    );
                } else {
                    // Non-Normal: render the fill into a scratch
                    // pixmap covering the path's pixel bounds, then
                    // composite the stamp onto the page with the
                    // requested blend mode. Blend modes are
                    // pixel-local so the scratch only needs the path
                    // bbox + 1px anti-alias slack.
                    let bbox = path.bounds();
                    let pad_pt = 1.0;
                    let min_x_pt = bbox.left() - pad_pt;
                    let min_y_pt = bbox.top() - pad_pt;
                    let max_x_pt = bbox.right() + pad_pt;
                    let max_y_pt = bbox.bottom() + pad_pt;
                    let off_x_px = (min_x_pt * scale).floor() as i32;
                    let off_y_px = (min_y_pt * scale).floor() as i32;
                    let max_x_px = (max_x_pt * scale).ceil() as i32;
                    let max_y_px = (max_y_pt * scale).ceil() as i32;
                    let w_px = (max_x_px - off_x_px).max(1) as u32;
                    let h_px = (max_y_px - off_y_px).max(1) as u32;
                    if let Some(mut scratch) = Pixmap::new(w_px, h_px) {
                        let scratch_xform = TsTransform::from_translate(
                            -off_x_px as f32,
                            -off_y_px as f32,
                        )
                        .pre_concat(TsTransform::from_scale(scale, scale));
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
                        pixmap.draw_pixmap(
                            off_x_px,
                            off_y_px,
                            scratch.as_ref(),
                            &composite,
                            TsTransform::identity(),
                            active_mask(&clip_stack),
                        );
                    } else {
                        pixmap.fill_path(
                            &path,
                            &ts_paint,
                            FillRule::Winding,
                            page_to_px,
                            active_mask(&clip_stack),
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
                let ts_paint = paint_to_ts(paint, list, transform, page_to_px);
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
                pixmap.stroke_path(
                    &path,
                    &ts_paint,
                    &ts_stroke,
                    page_to_px,
                    active_mask(&clip_stack),
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
                    pixmap.fill_path(
                        &path,
                        &p,
                        FillRule::Winding,
                        page_to_px,
                        active_mask(&clip_stack),
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
                    let min_x_pt = bbox.left() - pad_pt;
                    let min_y_pt = bbox.top() - pad_pt;
                    let max_x_pt = bbox.right() + pad_pt;
                    let max_y_pt = bbox.bottom() + pad_pt;
                    // Snap top-left to whole pixels so draw_pixmap
                    // (integer offsets) is pixel-aligned and the
                    // composite isn't bilinearly resampled.
                    let off_x_px = (min_x_pt * scale).floor() as i32;
                    let off_y_px = (min_y_pt * scale).floor() as i32;
                    let max_x_px = (max_x_pt * scale).ceil() as i32;
                    let max_y_px = (max_y_pt * scale).ceil() as i32;
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
                        .pre_concat(TsTransform::from_scale(scale, scale));
                        scratch.fill_path(&path, &p, FillRule::Winding, scratch_xform, None);
                        // tiny-skia stores RGBA8 premultiplied — the
                        // Gaussian blurs each channel independently
                        // over premultiplied alpha, which is the
                        // correct convolution for a glow/shadow stamp
                        // (blurring straight alpha would brighten the
                        // edges into a halo).
                        let kernel = gaussian_kernel(sigma_px);
                        gaussian_blur_premul(scratch.data_mut(), w_px, h_px, &kernel);
                        pixmap.draw_pixmap(
                            off_x_px,
                            off_y_px,
                            scratch.as_ref(),
                            &PixmapPaint::default(),
                            TsTransform::identity(),
                            active_mask(&clip_stack),
                        );
                    } else {
                        // Allocation failed (pathological size) —
                        // fall back to the hard-edge fill rather
                        // than skipping the shadow entirely.
                        pixmap.fill_path(
                            &path,
                            &p,
                            FillRule::Winding,
                            page_to_px,
                            active_mask(&clip_stack),
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
                // Compose the placement transform: the display-list
                // transform maps (0..1, 0..1) → page coords, and
                // page_to_px scales those to device pixels. Source
                // pixmap pixels live in (0..w, 0..h), so divide by
                // (w, h) before the existing transform.
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
                let pixel_to_px = page_to_px
                    .pre_concat(unit_to_page)
                    .pre_concat(pixel_to_unit);
                pixmap.draw_pixmap(
                    0,
                    0,
                    src.as_ref(),
                    &PixmapPaint::default(),
                    pixel_to_px,
                    active_mask(&clip_stack),
                );
            }
            DisplayCommand::PushClip { path_id, transform } => {
                // Build the clip path in page-space pt; the mask
                // rasteriser then applies the same `page_to_px`
                // scale used for fills/strokes so the clip lives in
                // pixel space.
                let Some(path_data) = list.paths.get(*path_id) else {
                    // Push an empty mask so the matching pop balances
                    // the stack — then drawing is unaffected (an empty
                    // clip would mean "draw nothing", but the missing
                    // path is more likely a renderer bug than a real
                    // empty clip; keep the un-clipped behaviour).
                    if let Some(top) = clip_stack.last().cloned() {
                        clip_stack.push(top);
                    } else if let Some(m) = TsMask::new(px_w, px_h) {
                        // Fresh white mask = no clipping.
                        let mut m = m;
                        // tiny_skia::Mask::new returns a black (zero)
                        // mask; flip it via invert so "no clip"
                        // semantics match (white = drawable).
                        m.invert();
                        clip_stack.push(m);
                    }
                    continue;
                };
                let Some(path) = build_path_transformed(path_data, transform) else {
                    if let Some(top) = clip_stack.last().cloned() {
                        clip_stack.push(top);
                    }
                    continue;
                };
                if let Some(parent) = clip_stack.last() {
                    let mut child = parent.clone();
                    child.intersect_path(&path, FillRule::Winding, true, page_to_px);
                    clip_stack.push(child);
                } else if let Some(mut fresh) = TsMask::new(px_w, px_h) {
                    // First clip on the stack: build from a fresh
                    // (transparent) mask filled with the path. Any
                    // subsequent push intersects against this base.
                    fresh.fill_path(&path, FillRule::Winding, true, page_to_px);
                    clip_stack.push(fresh);
                }
            }
            DisplayCommand::PopClip(_) => {
                clip_stack.pop();
            }
        }
    }

    let data = pixmap.take();
    RgbaImage::from_raw(px_w, px_h, data)
        .unwrap_or_else(|| RgbaImage::from_pixel(px_w, px_h, Rgba([0, 0, 0, 0])))
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
}
