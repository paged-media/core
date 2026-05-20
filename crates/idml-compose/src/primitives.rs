//! Vector primitives (rectangles, ellipses, generic paths).
//!
//! These emit `FillPath` commands just like the text path does, so the
//! display list stays a single flat stream. Shapes that are trivially
//! representable as transforms of a unit path (axis-aligned rect,
//! circle, unit Bézier ellipse) share interned path data via dedicated
//! cache keys — memory-efficient for documents with many frames.

use crate::display_list::{
    DisplayCommand, DisplayList, DropShadow, ImageId, Paint, PathData, PathSegment, Rect, Stroke,
    Transform,
};

/// Cache key for the unit rectangle `[0, 0, 1, 1]`. Any interned-path
/// consumer should treat this as reserved.
pub const UNIT_RECT_KEY: u64 = 0xD001_0001_0000_0001;

/// Emit a `FillPath` command for an axis-aligned rectangle in page
/// space. The unit-rect path is interned so a document with N frames
/// only stores one copy of the path data.
pub fn emit_rect(rect: Rect, paint: Paint, list: &mut DisplayList) {
    emit_rect_transformed(rect, Transform::IDENTITY, paint, list);
}

/// Filled rectangle with an arbitrary affine applied on top of the
/// rect-to-page mapping. Used by callers that need IDML's
/// `ItemTransform` to compose into the final transform without
/// throwing the unit-rect path interning away.
pub fn emit_rect_transformed(rect: Rect, outer: Transform, paint: Paint, list: &mut DisplayList) {
    let (path_id, _) = list.paths.intern(UNIT_RECT_KEY, unit_rect());
    let transform = Transform::for_rect_in(rect, outer);
    list.push(DisplayCommand::FillPath {
        path_id,
        paint,
        transform,
    });
}

/// Same as [`emit_rect_transformed`] but composites with a non-Normal
/// blend mode. `BlendMode::Normal` falls through to a regular
/// `FillPath` so the fast path stays single-allocation.
pub fn emit_rect_transformed_blend(
    rect: Rect,
    outer: Transform,
    paint: Paint,
    blend_mode: crate::display_list::BlendMode,
    list: &mut DisplayList,
) {
    let (path_id, _) = list.paths.intern(UNIT_RECT_KEY, unit_rect());
    let transform = Transform::for_rect_in(rect, outer);
    if matches!(blend_mode, crate::display_list::BlendMode::Normal) {
        list.push(DisplayCommand::FillPath {
            path_id,
            paint,
            transform,
        });
    } else {
        list.push(DisplayCommand::FillPathBlend {
            path_id,
            paint,
            transform,
            blend_mode,
        });
    }
}

/// Emit a `StrokePath` command for an axis-aligned rectangle. Reuses
/// the same interned unit-rect path as [`emit_rect`], so a document
/// with N stroked frames still stores exactly one rect outline.
pub fn emit_stroke_rect(rect: Rect, stroke: Stroke, paint: Paint, list: &mut DisplayList) {
    emit_stroke_rect_transformed(rect, Transform::IDENTITY, stroke, paint, list);
}

/// Stroked rectangle with an outer affine. See [`emit_rect_transformed`].
pub fn emit_stroke_rect_transformed(
    rect: Rect,
    outer: Transform,
    stroke: Stroke,
    paint: Paint,
    list: &mut DisplayList,
) {
    let (path_id, _) = list.paths.intern(UNIT_RECT_KEY, unit_rect());
    let transform = Transform::for_rect_in(rect, outer);
    list.push(DisplayCommand::StrokePath {
        path_id,
        paint,
        stroke,
        transform,
    });
}

/// Cache key for the unit ellipse — a four-cubic Bézier approximation
/// inscribed in the `[0, 0, 1, 1]` square (centred at `(0.5, 0.5)`,
/// radius `0.5`). Reserved for any interned-path consumer.
pub const UNIT_ELLIPSE_KEY: u64 = 0xD001_0001_0000_0002;

/// Emit a filled ellipse inscribed in `rect`. Like [`emit_rect`], the
/// unit-ellipse path is interned once per `DisplayList`, so a
/// document with N ovals only stores one outline.
pub fn emit_ellipse(rect: Rect, paint: Paint, list: &mut DisplayList) {
    emit_ellipse_transformed(rect, Transform::IDENTITY, paint, list);
}

/// Filled ellipse with an outer affine. See [`emit_rect_transformed`].
pub fn emit_ellipse_transformed(
    rect: Rect,
    outer: Transform,
    paint: Paint,
    list: &mut DisplayList,
) {
    let (path_id, _) = list.paths.intern(UNIT_ELLIPSE_KEY, unit_ellipse());
    let transform = Transform::for_rect_in(rect, outer);
    list.push(DisplayCommand::FillPath {
        path_id,
        paint,
        transform,
    });
}

/// Like [`emit_ellipse_transformed`] but composites with the given
/// blend mode. `BlendMode::Normal` falls through to a regular
/// `FillPath` so the fast path stays single-allocation.
pub fn emit_ellipse_transformed_blend(
    rect: Rect,
    outer: Transform,
    paint: Paint,
    blend_mode: crate::display_list::BlendMode,
    list: &mut DisplayList,
) {
    let (path_id, _) = list.paths.intern(UNIT_ELLIPSE_KEY, unit_ellipse());
    let transform = Transform::for_rect_in(rect, outer);
    if matches!(blend_mode, crate::display_list::BlendMode::Normal) {
        list.push(DisplayCommand::FillPath {
            path_id,
            paint,
            transform,
        });
    } else {
        list.push(DisplayCommand::FillPathBlend {
            path_id,
            paint,
            transform,
            blend_mode,
        });
    }
}

/// Stroked variant of [`emit_ellipse`].
pub fn emit_stroke_ellipse(rect: Rect, stroke: Stroke, paint: Paint, list: &mut DisplayList) {
    emit_stroke_ellipse_transformed(rect, Transform::IDENTITY, stroke, paint, list);
}

pub fn emit_stroke_ellipse_transformed(
    rect: Rect,
    outer: Transform,
    stroke: Stroke,
    paint: Paint,
    list: &mut DisplayList,
) {
    let (path_id, _) = list.paths.intern(UNIT_ELLIPSE_KEY, unit_ellipse());
    let transform = Transform::for_rect_in(rect, outer);
    list.push(DisplayCommand::StrokePath {
        path_id,
        paint,
        stroke,
        transform,
    });
}

/// Emit a drop-shadow stamp for an axis-aligned rectangle. Shares
/// the unit-rect interned path so a document with N shadowed frames
/// only stores one outline. Conventionally emitted *before* the
/// matching `emit_rect` so the shadow lands behind the fill.
pub fn emit_drop_shadow_rect(rect: Rect, shadow: DropShadow, list: &mut DisplayList) {
    emit_drop_shadow_rect_transformed(rect, Transform::IDENTITY, shadow, list);
}

pub fn emit_drop_shadow_rect_transformed(
    rect: Rect,
    outer: Transform,
    shadow: DropShadow,
    list: &mut DisplayList,
) {
    let (path_id, _) = list.paths.intern(UNIT_RECT_KEY, unit_rect());
    let transform = Transform::for_rect_in(rect, outer);
    list.push(DisplayCommand::DropShadow {
        path_id,
        transform,
        shadow,
    });
}

/// Emit a placed image covering `rect` (in local coords) with an
/// outer affine on top — same composition story as the rect / ellipse
/// `*_transformed` family. The rasterizer maps the image's full
/// pixel grid into `rect` then through `outer` into page coords.
pub fn emit_image_at(rect: Rect, outer: Transform, image_id: ImageId, list: &mut DisplayList) {
    let transform = Transform::for_rect_in(rect, outer);
    list.push(DisplayCommand::Image {
        image_id,
        transform,
    });
}

/// Emit a stroked straight line from `(x1, y1)` to `(x2, y2)` in page
/// coords. Lines have no fill; this only emits a `StrokePath`.
pub fn emit_line(
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    stroke: Stroke,
    paint: Paint,
    list: &mut DisplayList,
) {
    // Lines aren't naturally interned (their geometry depends on the
    // endpoints), so push an anonymous path each time.
    let path = PathData {
        segments: vec![
            PathSegment::MoveTo { x: x1, y: y1 },
            PathSegment::LineTo { x: x2, y: y2 },
        ],
    };
    let path_id = list.paths.push_anon(path);
    list.push(DisplayCommand::StrokePath {
        path_id,
        paint,
        stroke,
        transform: Transform::IDENTITY,
    });
}

/// Magic constant for cubic Bézier ellipse approximation: the cubic
/// control-point distance that turns four arcs into a near-perfect
/// circle. Standard value: 4·(√2 − 1) / 3.
const ELLIPSE_KAPPA: f32 = 0.552_284_8;

pub fn unit_ellipse() -> PathData {
    // A cubic Bézier approximation of a unit-square inscribed circle
    // (centre 0.5, radius 0.5). Four arcs, each cubic.
    let r = 0.5_f32;
    let k = r * ELLIPSE_KAPPA;
    let cx = 0.5_f32;
    let cy = 0.5_f32;
    PathData {
        segments: vec![
            PathSegment::MoveTo { x: cx + r, y: cy },
            PathSegment::CubicTo {
                cx1: cx + r,
                cy1: cy + k,
                cx2: cx + k,
                cy2: cy + r,
                x: cx,
                y: cy + r,
            },
            PathSegment::CubicTo {
                cx1: cx - k,
                cy1: cy + r,
                cx2: cx - r,
                cy2: cy + k,
                x: cx - r,
                y: cy,
            },
            PathSegment::CubicTo {
                cx1: cx - r,
                cy1: cy - k,
                cx2: cx - k,
                cy2: cy - r,
                x: cx,
                y: cy - r,
            },
            PathSegment::CubicTo {
                cx1: cx + k,
                cy1: cy - r,
                cx2: cx + r,
                cy2: cy - k,
                x: cx + r,
                y: cy,
            },
            PathSegment::Close,
        ],
    }
}

fn unit_rect() -> PathData {
    PathData {
        segments: vec![
            PathSegment::MoveTo { x: 0.0, y: 0.0 },
            PathSegment::LineTo { x: 1.0, y: 0.0 },
            PathSegment::LineTo { x: 1.0, y: 1.0 },
            PathSegment::LineTo { x: 0.0, y: 1.0 },
            PathSegment::Close,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_list::Color;

    #[test]
    fn emits_one_fillpath_per_rect() {
        let mut list = DisplayList::new();
        emit_rect(
            Rect {
                x: 10.0,
                y: 20.0,
                w: 100.0,
                h: 50.0,
            },
            Paint::Solid(Color::BLACK),
            &mut list,
        );
        assert_eq!(list.commands.len(), 1);
    }

    #[test]
    fn rects_share_interned_unit_path() {
        let mut list = DisplayList::new();
        for i in 0..5 {
            emit_rect(
                Rect {
                    x: i as f32 * 10.0,
                    y: 0.0,
                    w: 8.0,
                    h: 8.0,
                },
                Paint::Solid(Color::BLACK),
                &mut list,
            );
        }
        assert_eq!(list.commands.len(), 5);
        assert_eq!(list.paths.len(), 1, "unit rect should be interned once");
    }

    #[test]
    fn transform_maps_unit_rect_onto_target() {
        let mut list = DisplayList::new();
        emit_rect(
            Rect {
                x: 100.0,
                y: 200.0,
                w: 300.0,
                h: 400.0,
            },
            Paint::Solid(Color::WHITE),
            &mut list,
        );
        let t = match &list.commands[0] {
            DisplayCommand::FillPath { transform, .. } => *transform,
            DisplayCommand::FillPathBlend { transform, .. } => *transform,
            DisplayCommand::StrokePath { transform, .. } => *transform,
            DisplayCommand::DropShadow { transform, .. } => *transform,
            DisplayCommand::PathShadow { transform, .. } => *transform,
            DisplayCommand::Image { transform, .. } => *transform,
            DisplayCommand::PushClip { transform, .. } => *transform,
            DisplayCommand::PopClip(transform) => *transform,
            DisplayCommand::BeginBlendGroup { transform, .. } => *transform,
            DisplayCommand::EndBlendGroup(transform) => *transform,
            DisplayCommand::InnerShadow { transform, .. } => *transform,
            DisplayCommand::OuterGlow { transform, .. } => *transform,
            DisplayCommand::InnerGlow { transform, .. } => *transform,
            DisplayCommand::BevelEmboss { transform, .. } => *transform,
            DisplayCommand::Satin { transform, .. } => *transform,
            DisplayCommand::Feather { transform, .. } => *transform,
            DisplayCommand::DirectionalFeather { transform, .. } => *transform,
            DisplayCommand::GradientFeather { transform, .. } => *transform,
            DisplayCommand::PushLayer { transform, .. } => *transform,
            DisplayCommand::PopLayer(transform) => *transform,
            DisplayCommand::FillPathOverprint { transform, .. } => *transform,
            DisplayCommand::StrokePathOverprint { transform, .. } => *transform,
        };
        // Unit rect corners: (0,0), (1,0), (1,1), (0,1).
        assert_eq!(t.apply(0.0, 0.0), (100.0, 200.0));
        assert_eq!(t.apply(1.0, 0.0), (400.0, 200.0));
        assert_eq!(t.apply(1.0, 1.0), (400.0, 600.0));
        assert_eq!(t.apply(0.0, 1.0), (100.0, 600.0));
    }

    #[test]
    fn stroke_rect_emits_stroke_command() {
        let mut list = DisplayList::new();
        emit_stroke_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 50.0,
            },
            Stroke::new(2.0),
            Paint::Solid(Color::BLACK),
            &mut list,
        );
        assert_eq!(list.commands.len(), 1);
        match &list.commands[0] {
            DisplayCommand::StrokePath { stroke, .. } => {
                assert_eq!(stroke.width, 2.0);
            }
            _ => panic!("expected StrokePath"),
        }
    }

    #[test]
    fn fill_and_stroke_rect_share_one_interned_path() {
        let mut list = DisplayList::new();
        let r = Rect {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
        };
        emit_rect(r, Paint::Solid(Color::WHITE), &mut list);
        emit_stroke_rect(r, Stroke::new(1.0), Paint::Solid(Color::BLACK), &mut list);
        assert_eq!(list.commands.len(), 2);
        assert_eq!(
            list.paths.len(),
            1,
            "fill + stroke should share the unit rect"
        );
    }

    #[test]
    fn emit_ellipse_interns_unit_path_once() {
        let mut list = DisplayList::new();
        for i in 0..4 {
            emit_ellipse(
                Rect {
                    x: i as f32 * 10.0,
                    y: 0.0,
                    w: 8.0,
                    h: 8.0,
                },
                Paint::Solid(Color::WHITE),
                &mut list,
            );
        }
        assert_eq!(list.commands.len(), 4);
        assert_eq!(list.paths.len(), 1);
    }

    #[test]
    fn ellipse_and_rect_use_distinct_unit_paths() {
        let mut list = DisplayList::new();
        let r = Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        };
        emit_rect(r, Paint::Solid(Color::WHITE), &mut list);
        emit_ellipse(r, Paint::Solid(Color::BLACK), &mut list);
        assert_eq!(list.commands.len(), 2);
        assert_eq!(list.paths.len(), 2, "rect + ellipse keys differ");
    }

    #[test]
    fn drop_shadow_rect_emits_drop_shadow_command() {
        use crate::DropShadow;
        let mut list = DisplayList::new();
        emit_drop_shadow_rect(
            Rect {
                x: 10.0,
                y: 20.0,
                w: 100.0,
                h: 50.0,
            },
            DropShadow::default_soft(),
            &mut list,
        );
        assert_eq!(list.commands.len(), 1);
        match &list.commands[0] {
            DisplayCommand::DropShadow { shadow, .. } => {
                assert_eq!(shadow.offset_x, 4.0);
                assert!(shadow.opacity > 0.0 && shadow.opacity < 1.0);
            }
            other => panic!("expected DropShadow, got {other:?}"),
        }
    }

    #[test]
    fn line_emits_a_stroke_path() {
        let mut list = DisplayList::new();
        emit_line(
            0.0,
            0.0,
            100.0,
            50.0,
            Stroke::new(1.0),
            Paint::Solid(Color::BLACK),
            &mut list,
        );
        assert_eq!(list.commands.len(), 1);
        match &list.commands[0] {
            DisplayCommand::StrokePath { stroke, .. } => assert_eq!(stroke.width, 1.0),
            other => panic!("expected StrokePath, got {other:?}"),
        }
    }
}
