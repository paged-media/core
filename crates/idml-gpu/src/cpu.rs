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
    Color as CComposeColor, DisplayCommand, DisplayList, LineCap, LineJoin, Paint, PathData,
    PathSegment, Transform as CTransform,
};
use image::{Rgba, RgbaImage};
use tiny_skia::{
    FillRule, GradientStop as TsGradientStop, LineCap as TsLineCap, LineJoin as TsLineJoin,
    LinearGradient as TsLinearGradient, Paint as TsPaint, PathBuilder, Pixmap, PixmapPaint,
    Point as TsPoint, Shader, SpreadMode, Stroke as TsStroke, Transform as TsTransform,
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
                pixmap.fill_path(&path, &ts_paint, FillRule::Winding, page_to_px, None);
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
                pixmap.stroke_path(&path, &ts_paint, &ts_stroke, page_to_px, None);
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
                // the shadow offset. Hard-edge today; idea.md §10.4's
                // separable Gaussian blur lands once the offscreen
                // layer pipeline is in place.
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
                pixmap.fill_path(&path, &p, FillRule::Winding, page_to_px, None);
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
                    None,
                );
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

    TsLinearGradient::new(start, end, stops, SpreadMode::Pad, page_to_px)
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
