//! Vector primitives (rectangles, ellipses, generic paths).
//!
//! These emit `FillPath` commands just like the text path does, so the
//! display list stays a single flat stream. Shapes that are trivially
//! representable as transforms of a unit path (axis-aligned rect,
//! circle, unit Bézier ellipse) share interned path data via dedicated
//! cache keys — memory-efficient for documents with many frames.

use crate::display_list::{
    DisplayCommand, DisplayList, Paint, PathData, PathSegment, Rect, Stroke, Transform,
};

/// Cache key for the unit rectangle `[0, 0, 1, 1]`. Any interned-path
/// consumer should treat this as reserved.
pub const UNIT_RECT_KEY: u64 = 0xD001_0001_0000_0001;

/// Emit a `FillPath` command for an axis-aligned rectangle in page
/// space. The unit-rect path is interned so a document with N frames
/// only stores one copy of the path data.
pub fn emit_rect(rect: Rect, paint: Paint, list: &mut DisplayList) {
    let (path_id, _) = list.paths.intern(UNIT_RECT_KEY, unit_rect());
    // Map unit-rect [0,0,1,1] → [rect.x, rect.y, rect.x+rect.w, rect.y+rect.h]:
    // scale by (rect.w, rect.h), translate to (rect.x, rect.y).
    let transform = Transform([rect.w, 0.0, 0.0, rect.h, rect.x, rect.y]);
    list.push(DisplayCommand::FillPath {
        path_id,
        paint,
        transform,
    });
}

/// Emit a `StrokePath` command for an axis-aligned rectangle. Reuses
/// the same interned unit-rect path as [`emit_rect`], so a document
/// with N stroked frames still stores exactly one rect outline.
pub fn emit_stroke_rect(rect: Rect, stroke: Stroke, paint: Paint, list: &mut DisplayList) {
    let (path_id, _) = list.paths.intern(UNIT_RECT_KEY, unit_rect());
    let transform = Transform([rect.w, 0.0, 0.0, rect.h, rect.x, rect.y]);
    list.push(DisplayCommand::StrokePath {
        path_id,
        paint,
        stroke,
        transform,
    });
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
            DisplayCommand::StrokePath { transform, .. } => *transform,
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
}
