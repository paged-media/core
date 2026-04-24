//! Glyph outline extraction for the display-list compositor.
//!
//! Production uses `ttf-parser` via [`TtfOutliner`]. Tests use
//! [`UnitSquareOutliner`], which emits a deterministic unit-square
//! outline for every glyph id — enough to exercise path caching,
//! transforms, and command emission without shipping a test font.

use crate::display_list::{PathData, PathSegment};

/// Extracts a glyph outline in font-design units (y-up, baseline at
/// y=0). `units_per_em` is the font's em box, used by the compositor
/// to scale to pt.
pub trait GlyphOutliner {
    fn outline(&self, glyph_id: u32) -> Option<PathData>;
    fn units_per_em(&self) -> f32;
}

/// Production outliner backed by `ttf_parser::Face`.
pub struct TtfOutliner<'a> {
    pub face: &'a ttf_parser::Face<'a>,
}

impl<'a> TtfOutliner<'a> {
    pub fn new(face: &'a ttf_parser::Face<'a>) -> Self {
        Self { face }
    }
}

impl GlyphOutliner for TtfOutliner<'_> {
    fn outline(&self, glyph_id: u32) -> Option<PathData> {
        if glyph_id > u16::MAX as u32 {
            return None;
        }
        let mut builder = PathBuilder::default();
        self.face
            .outline_glyph(ttf_parser::GlyphId(glyph_id as u16), &mut builder)?;
        Some(PathData {
            segments: builder.segments,
        })
    }

    fn units_per_em(&self) -> f32 {
        self.face.units_per_em() as f32
    }
}

#[derive(Default)]
struct PathBuilder {
    segments: Vec<PathSegment>,
}

impl ttf_parser::OutlineBuilder for PathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.segments.push(PathSegment::MoveTo { x, y });
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.segments.push(PathSegment::LineTo { x, y });
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.segments.push(PathSegment::QuadTo { cx, cy, x, y });
    }
    fn curve_to(&mut self, cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32) {
        self.segments.push(PathSegment::CubicTo {
            cx1,
            cy1,
            cx2,
            cy2,
            x,
            y,
        });
    }
    fn close(&mut self) {
        self.segments.push(PathSegment::Close);
    }
}

/// Test-only outliner. Returns a 1000-unit square for every glyph id,
/// with `units_per_em = 1000`. Deterministic and cheap.
#[derive(Debug, Clone, Copy)]
pub struct UnitSquareOutliner {
    pub units_per_em: f32,
}

impl Default for UnitSquareOutliner {
    fn default() -> Self {
        Self {
            units_per_em: 1000.0,
        }
    }
}

impl GlyphOutliner for UnitSquareOutliner {
    fn outline(&self, _glyph_id: u32) -> Option<PathData> {
        let e = self.units_per_em;
        Some(PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::LineTo { x: e, y: 0.0 },
                PathSegment::LineTo { x: e, y: e },
                PathSegment::LineTo { x: 0.0, y: e },
                PathSegment::Close,
            ],
        })
    }

    fn units_per_em(&self) -> f32 {
        self.units_per_em
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_square_has_five_segments() {
        let o = UnitSquareOutliner::default();
        let path = o.outline(42).unwrap();
        assert_eq!(path.segments.len(), 5);
        assert!(matches!(path.segments[0], PathSegment::MoveTo { .. }));
        assert!(matches!(path.segments[4], PathSegment::Close));
    }
}
