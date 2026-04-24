//! Positioned glyphs — the handoff format to the GPU rasterizer.
//!
//! Composes a paragraph into lines, shapes each line, and walks the
//! glyphs to turn per-glyph advances into absolute (x, y) coordinates
//! in 1/64 pt, frame-origin-relative.
//!
//! Justification is deliberately out of scope for this pass: every
//! line sits at its natural shaped width. Ragged-right is fine for
//! the fidelity corpus's first seed entries; full justification
//! (distributing the composer's ratio across inter-word glue) lands
//! alongside §8.5's justification features.

use std::ops::Range;

use crate::compose::{compose_paragraph, ComposeOptions, TextShaper};
use crate::shape::{ShapedRun, ADVANCE_PRECISION};

/// A glyph positioned in frame space, ready for rasterization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub glyph_id: u32,
    /// Byte offset within the source paragraph.
    pub cluster: u32,
    /// Frame-origin-relative x, 1/64 pt.
    pub x: i32,
    /// Frame-origin-relative y (baseline + per-glyph y_offset), 1/64 pt.
    pub y: i32,
}

#[derive(Debug, Clone)]
pub struct LaidOutLine {
    pub byte_range: Range<usize>,
    /// Baseline y, 1/64 pt, frame-origin-relative.
    pub baseline_y: i32,
    /// Natural (unjustified) width of the line, 1/64 pt.
    pub width: i32,
    /// Paragraph-breaker ratio. 0 = natural, >0 = stretched (would be
    /// justified), <0 = shrunk.
    pub ratio: f32,
    pub glyphs: Vec<PositionedGlyph>,
}

#[derive(Debug, Clone)]
pub struct LaidOutParagraph {
    pub lines: Vec<LaidOutLine>,
}

#[derive(Debug, Clone)]
pub struct LayoutOptions {
    pub compose: ComposeOptions,
    /// Distance between baselines, 1/64 pt.
    pub line_height: i32,
    /// Offset of the first baseline from the top of the paragraph box,
    /// 1/64 pt.
    pub first_baseline: i32,
}

impl LayoutOptions {
    /// Convenience constructor from point-unit inputs. Uses 1.2×
    /// point_size as the default line height (common InDesign default
    /// for Auto leading) and `0.8 × point_size` for the first baseline.
    pub fn new(column_width_pt: f32, point_size: f32) -> Self {
        let line_height = (point_size * 1.2 * ADVANCE_PRECISION).round() as i32;
        let first_baseline = (point_size * 0.8 * ADVANCE_PRECISION).round() as i32;
        Self {
            compose: ComposeOptions::new(column_width_pt),
            line_height,
            first_baseline,
        }
    }
}

/// Lay out `text` through `shaper` (which provides both widths for the
/// composer and glyph outlines for positioning).
pub fn layout_paragraph<S: TextShaper>(
    text: &str,
    shaper: &S,
    options: &LayoutOptions,
) -> LaidOutParagraph {
    let composed = compose_paragraph(text, shaper, &options.compose);
    let mut lines = Vec::with_capacity(composed.len());
    let mut baseline = options.first_baseline;

    for line in composed {
        let slice = &text[line.byte_range.clone()];
        let shaped = shaper.shape(slice);
        let glyphs = position_line(&shaped, 0, baseline, line.byte_range.start as u32);
        lines.push(LaidOutLine {
            byte_range: line.byte_range,
            baseline_y: baseline,
            width: shaped.total_advance,
            ratio: line.ratio,
            glyphs,
        });
        baseline += options.line_height;
    }

    LaidOutParagraph { lines }
}

/// Walk a `ShapedRun`'s advances and turn them into absolute positions.
///
/// `start_x` and `baseline_y` are in 1/64 pt, frame-origin-relative.
/// `cluster_base` is added to each glyph's intra-slice cluster so the
/// output carries byte offsets back into the source paragraph.
pub fn position_line(
    shaped: &ShapedRun,
    start_x: i32,
    baseline_y: i32,
    cluster_base: u32,
) -> Vec<PositionedGlyph> {
    let mut out = Vec::with_capacity(shaped.glyphs.len());
    let mut pen_x = start_x;
    for g in &shaped.glyphs {
        out.push(PositionedGlyph {
            glyph_id: g.glyph_id,
            cluster: cluster_base + g.cluster,
            x: pen_x + g.x_offset,
            y: baseline_y + g.y_offset,
        });
        pen_x += g.x_advance;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::MonospaceMeasurer;
    use crate::shape::ShapedGlyph;

    fn fake_run(advances: &[i32]) -> ShapedRun {
        let glyphs: Vec<ShapedGlyph> = advances
            .iter()
            .enumerate()
            .map(|(i, &adv)| ShapedGlyph {
                glyph_id: 100 + i as u32,
                cluster: i as u32,
                x_advance: adv,
                y_offset: 0,
                x_offset: 0,
            })
            .collect();
        ShapedRun {
            glyphs,
            total_advance: advances.iter().sum(),
        }
    }

    #[test]
    fn position_line_accumulates_advances() {
        let run = fake_run(&[100, 80, 120]);
        let out = position_line(&run, 50, 200, 0);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].x, 50);
        assert_eq!(out[1].x, 150);
        assert_eq!(out[2].x, 230);
        for g in &out {
            assert_eq!(g.y, 200);
        }
    }

    #[test]
    fn position_line_applies_offsets() {
        let run = ShapedRun {
            glyphs: vec![ShapedGlyph {
                glyph_id: 1,
                cluster: 0,
                x_advance: 100,
                x_offset: 5,
                y_offset: -7,
            }],
            total_advance: 100,
        };
        let out = position_line(&run, 10, 50, 0);
        assert_eq!(out[0].x, 15); // 10 + x_offset 5
        assert_eq!(out[0].y, 43); // 50 + y_offset -7
    }

    #[test]
    fn position_line_offsets_cluster_by_base() {
        let run = fake_run(&[10, 10]);
        let out = position_line(&run, 0, 0, 42);
        assert_eq!(out[0].cluster, 42);
        assert_eq!(out[1].cluster, 43);
    }

    #[test]
    fn layout_paragraph_uses_monospace_shaper_end_to_end() {
        let shaper = MonospaceMeasurer::new(10, 10);
        let opts = LayoutOptions {
            compose: ComposeOptions {
                column_width: 120, // 12 glyph widths
                tolerance: 10.0,
                stretch_ratio: 1.0,
                shrink_ratio: 0.5,
                looseness: 0,
            },
            line_height: 20,
            first_baseline: 15,
        };
        let out = layout_paragraph("lorem ipsum dolor sit amet", &shaper, &opts);

        assert!(!out.lines.is_empty(), "no lines emitted");
        // Baselines advance by line_height.
        for w in out.lines.windows(2) {
            assert_eq!(w[1].baseline_y - w[0].baseline_y, 20);
        }
        // First baseline is at first_baseline.
        assert_eq!(out.lines[0].baseline_y, 15);
        // All glyphs on line 0 start at x >= 0 and increase monotonically.
        let line0 = &out.lines[0];
        for pair in line0.glyphs.windows(2) {
            assert!(pair[0].x <= pair[1].x);
        }
        // Line width matches sum of advances.
        let expected_width: i32 = line0.glyphs.iter().map(|_| 10).sum::<i32>();
        assert_eq!(line0.width, expected_width);
    }
}
