//! Convert a laid-out paragraph into display-list commands.
//!
//! Produces one `FillPath` command per glyph. Glyph outlines are
//! interned in the list's `PathBuffer` via a `(font_id, glyph_id)`
//! cache key so repeated glyphs (the common case) share tessellated
//! data.
//!
//! Coordinate system:
//! - Font outlines are y-up with the baseline at y=0.
//! - IDML pages are y-down with the top-left at (0, 0).
//! - The per-glyph transform scales by `point_size / units_per_em`
//!   and flips y, then translates to the glyph's (x, y) position on
//!   the page (in pt).
//!
//! All text input positions are in 1/64 pt, as produced by
//! `idml_text::layout`; we divide by 64 at the emit boundary.

use idml_text::layout::LaidOutParagraph;

use crate::display_list::{DisplayCommand, DisplayList, GlyphCacheKey, Paint, PathId, Transform};
use crate::glyph::GlyphOutliner;

/// Advance precision used by `idml_text::layout`: positions are in
/// 1/64 pt. We divide by this when converting to float pt.
const ADVANCE_PRECISION: f32 = 64.0;

/// Emit `FillPath` commands for every glyph in `laid_out`.
///
/// - `font_id` identifies the font for glyph caching; callers pick a
///   scheme (hash of the font bytes, index into a font table, etc.)
///   and keep it stable for a single render.
/// - `point_size` is the em size the glyphs were shaped at.
/// - `paint_for(cluster)` returns the paint for the glyph at `cluster`
///   (byte offset into the source paragraph). Single-colour callers
///   pass `|_| Paint::Solid(my_color)`.
/// - `frame_origin_pt` is the page-space position of the frame's
///   top-left corner. Glyph positions are offset by it so the
///   commands live in page coordinates.
pub fn emit_paragraph<O, F>(
    laid_out: &LaidOutParagraph,
    font_id: u32,
    point_size: f32,
    paint_for: F,
    frame_origin_pt: (f32, f32),
    outliner: &O,
    list: &mut DisplayList,
) where
    O: GlyphOutliner,
    F: Fn(u32) -> Paint,
{
    let upem = outliner.units_per_em();
    let scale = point_size / upem;
    let (ox, oy) = frame_origin_pt;

    for line in &laid_out.lines {
        for g in &line.glyphs {
            let Some(path_id) = get_or_intern_glyph_outline(font_id, g.glyph_id, outliner, list)
            else {
                continue;
            };
            let gx = ox + g.x as f32 / ADVANCE_PRECISION;
            let gy = oy + g.y as f32 / ADVANCE_PRECISION;
            let paint = paint_for(g.cluster);
            // Column-major 2×3 as `[a b c d tx ty]`: scale by (scale,
            // scale) and flip y by negating the y-axis scale. Then
            // translate to (gx, gy).
            let transform = Transform([scale, 0.0, 0.0, -scale, gx, gy]);
            list.push(DisplayCommand::FillPath {
                path_id,
                paint,
                transform,
            });
        }
    }
}

fn get_or_intern_glyph_outline(
    font_id: u32,
    glyph_id: u32,
    outliner: &impl GlyphOutliner,
    list: &mut DisplayList,
) -> Option<PathId> {
    let key = GlyphCacheKey { font_id, glyph_id }.to_u64();
    // `PathBuffer::intern` already treats a repeated key as a cache
    // hit and does not store a second copy. Build the outline only on
    // a miss by probing the cache first.
    if let Some(existing) = list.paths.find_by_key(key) {
        return Some(existing);
    }
    let outline = outliner.outline(glyph_id)?;
    let (id, _fresh) = list.paths.intern(key, outline);
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_list::Color;
    use crate::glyph::UnitSquareOutliner;
    use idml_text::compose::{ComposeOptions, MonospaceMeasurer};
    use idml_text::layout::{layout_paragraph, LayoutOptions};

    fn laid_out(text: &str) -> LaidOutParagraph {
        let shaper = MonospaceMeasurer::new(500, 500);
        let opts = LayoutOptions {
            compose: ComposeOptions {
                column_width: 500 * 8,
                tolerance: 10.0,
                stretch_ratio: 1.0,
                shrink_ratio: 0.5,
                looseness: 0,
            },
            line_height: 64 * 14,
            first_baseline: 64 * 10,
        };
        layout_paragraph(text, &shaper, &opts)
    }

    #[test]
    fn emit_produces_one_command_per_glyph() {
        let p = laid_out("hello world foo bar");
        let mut list = DisplayList::new();
        emit_paragraph(
            &p,
            1,
            12.0,
            |_| Paint::Solid(Color::BLACK),
            (0.0, 0.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        let total_glyphs: usize = p.lines.iter().map(|l| l.glyphs.len()).sum();
        assert_eq!(list.commands.len(), total_glyphs);
    }

    #[test]
    fn repeated_glyph_shares_path_id() {
        // "aaaa" — every glyph id identical → every FillPath reuses
        // the same interned path.
        let p = laid_out("aaaa aaaa");
        let mut list = DisplayList::new();
        emit_paragraph(
            &p,
            1,
            12.0,
            |_| Paint::Solid(Color::BLACK),
            (0.0, 0.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        // Path buffer holds one outline for 'a' plus one for ' '.
        // (MonospaceMeasurer issues a real glyph per space too.)
        assert!(
            list.paths.len() <= 2,
            "expected ≤ 2 unique paths, got {}",
            list.paths.len()
        );
    }

    #[test]
    fn glyph_positions_are_offset_by_frame_origin() {
        let p = laid_out("abc");
        let mut list = DisplayList::new();
        emit_paragraph(
            &p,
            1,
            12.0,
            |_| Paint::Solid(Color::BLACK),
            (100.0, 200.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        let first = match &list.commands[0] {
            DisplayCommand::FillPath { transform, .. } => transform.0,
            other => panic!("expected FillPath, got {other:?}"),
        };
        // tx = 100 + glyph.x/64 = 100 + 0 = 100 (first glyph at x=0).
        assert!((first[4] - 100.0).abs() < 1e-4, "tx = {}", first[4]);
        // ty should be 200 + baseline_y/64 = 200 + 10 = 210.
        assert!((first[5] - 210.0).abs() < 1e-4, "ty = {}", first[5]);
    }

    #[test]
    fn paint_picker_receives_cluster_byte_offset() {
        // "ab" with MonospaceMeasurer → 2 glyphs at clusters 0 and 1.
        let p = laid_out("ab");
        let mut list = DisplayList::new();
        let red = Paint::Solid(Color::rgba(1.0, 0.0, 0.0, 1.0));
        let blue = Paint::Solid(Color::rgba(0.0, 0.0, 1.0, 1.0));
        emit_paragraph(
            &p,
            1,
            12.0,
            |c| if c == 0 { red } else { blue },
            (0.0, 0.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        assert_eq!(list.commands.len(), 2);
        let paints: Vec<Paint> = list
            .commands
            .iter()
            .map(|c| match c {
                DisplayCommand::FillPath { paint, .. } => *paint,
                other => panic!("expected FillPath, got {other:?}"),
            })
            .collect();
        assert_eq!(paints[0], red);
        assert_eq!(paints[1], blue);
    }

    #[test]
    fn y_axis_is_flipped_by_transform_matrix() {
        let p = laid_out("x");
        let mut list = DisplayList::new();
        emit_paragraph(
            &p,
            1,
            12.0,
            |_| Paint::Solid(Color::BLACK),
            (0.0, 0.0),
            &UnitSquareOutliner::default(),
            &mut list,
        );
        let m = match &list.commands[0] {
            DisplayCommand::FillPath { transform, .. } => transform.0,
            other => panic!("expected FillPath, got {other:?}"),
        };
        // d (y-scale) must be negative — fonts are y-up, pages y-down.
        assert!(m[3] < 0.0, "y-scale not flipped: {:?}", m);
    }
}
