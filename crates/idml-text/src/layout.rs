//! Positioned glyphs — the handoff format to the GPU rasterizer.
//!
//! Composes a paragraph into lines, shapes each line, and walks the
//! glyphs to turn per-glyph advances into absolute (x, y) coordinates
//! in 1/64 pt, frame-origin-relative.
//!
//! Alignment is a post-shape pass. Left/right/center shift each line's
//! glyphs by a constant. Justify distributes the leftover width across
//! the line's inter-word glue (glyphs whose cluster points at a
//! whitespace byte in the source paragraph).

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

/// Paragraph-level horizontal alignment.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    #[default]
    Left,
    Right,
    Center,
    /// Fully justified — the last line of a paragraph stays
    /// left-aligned (common typographic convention). Intermediate
    /// lines distribute extra width across inter-word glue.
    Justify,
}

#[derive(Debug, Clone)]
pub struct LayoutOptions {
    pub compose: ComposeOptions,
    /// Distance between baselines, 1/64 pt.
    pub line_height: i32,
    /// Offset of the first baseline from the top of the paragraph box,
    /// 1/64 pt.
    pub first_baseline: i32,
    /// Horizontal alignment. Left by default.
    pub alignment: Alignment,
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
            alignment: Alignment::Left,
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
    let last_index = composed.len().saturating_sub(1);
    let mut lines = Vec::with_capacity(composed.len());
    let mut baseline = options.first_baseline;

    for (i, line) in composed.iter().enumerate() {
        let slice = &text[line.byte_range.clone()];
        let shaped = shaper.shape(slice);
        let mut glyphs = position_line(&shaped, 0, baseline, line.byte_range.start as u32);
        let is_last = i == last_index;
        apply_alignment(
            &mut glyphs,
            shaped.total_advance,
            options.column_width(),
            options.alignment,
            is_last,
            text.as_bytes(),
        );
        lines.push(LaidOutLine {
            byte_range: line.byte_range.clone(),
            baseline_y: baseline,
            width: shaped.total_advance,
            ratio: line.ratio,
            glyphs,
        });
        baseline += options.line_height;
    }

    LaidOutParagraph { lines }
}

impl LayoutOptions {
    /// Column width in 1/64 pt (convenience for layout passes).
    pub fn column_width(&self) -> i32 {
        self.compose.column_width
    }
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

/// Shift / justify a line's glyphs in-place.
///
/// `natural_width` is the sum of advances (= `ShapedRun::total_advance`).
/// `column_width` is the target column width. Both in 1/64 pt.
///
/// For `Justify`, the last line of a paragraph stays left-aligned
/// (indicated by `is_last_line`) to avoid stretching a short tail line.
fn apply_alignment(
    glyphs: &mut [PositionedGlyph],
    natural_width: i32,
    column_width: i32,
    alignment: Alignment,
    is_last_line: bool,
    paragraph_bytes: &[u8],
) {
    if glyphs.is_empty() || column_width <= 0 {
        return;
    }
    let extra = column_width - natural_width;
    match alignment {
        Alignment::Left => {}
        Alignment::Right => {
            for g in glyphs.iter_mut() {
                g.x += extra;
            }
        }
        Alignment::Center => {
            let shift = extra / 2;
            for g in glyphs.iter_mut() {
                g.x += shift;
            }
        }
        Alignment::Justify => {
            if is_last_line || extra <= 0 {
                return;
            }
            // Count glyphs whose cluster points at a whitespace byte
            // (skipping the first glyph so we don't indent the line).
            let space_count = glyphs
                .iter()
                .skip(1)
                .filter(|g| is_ws_at(paragraph_bytes, g.cluster as usize))
                .count() as i32;
            if space_count == 0 {
                return;
            }
            let per_space = extra / space_count;
            let remainder = extra - per_space * space_count;
            // Walk glyphs left-to-right, accumulating a shift as each
            // space is encountered. Integer division leaves a small
            // remainder which we bleed into the first few spaces so
            // the last glyph lands exactly on the column edge.
            let mut shift = 0i32;
            let mut spaces_seen = 0i32;
            for (i, g) in glyphs.iter_mut().enumerate() {
                if i > 0 && is_ws_at(paragraph_bytes, g.cluster as usize) {
                    let bleed = if spaces_seen < remainder { 1 } else { 0 };
                    shift += per_space + bleed;
                    spaces_seen += 1;
                }
                g.x += shift;
            }
        }
    }
}

fn is_ws_at(bytes: &[u8], i: usize) -> bool {
    matches!(
        bytes.get(i),
        Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
    )
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

    fn opts(column_chars: i32, alignment: Alignment) -> LayoutOptions {
        LayoutOptions {
            compose: ComposeOptions {
                column_width: column_chars * 10,
                tolerance: 10.0,
                stretch_ratio: 1.0,
                shrink_ratio: 0.5,
                looseness: 0,
            },
            line_height: 20,
            first_baseline: 15,
            alignment,
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
    fn left_alignment_leaves_glyphs_at_zero() {
        let shaper = MonospaceMeasurer::new(10, 10);
        let out = layout_paragraph("ab", &shaper, &opts(20, Alignment::Left));
        let first = &out.lines[0].glyphs[0];
        assert_eq!(first.x, 0);
    }

    #[test]
    fn right_alignment_pushes_line_to_column_edge() {
        let shaper = MonospaceMeasurer::new(10, 10);
        // "ab" = 20 units, column = 200, expected shift = 180.
        let out = layout_paragraph("ab", &shaper, &opts(20, Alignment::Right));
        let first = &out.lines[0].glyphs[0];
        assert_eq!(first.x, 180);
    }

    #[test]
    fn center_alignment_halves_the_gap() {
        let shaper = MonospaceMeasurer::new(10, 10);
        // "ab" = 20, column = 200, gap = 180, shift = 90.
        let out = layout_paragraph("ab", &shaper, &opts(20, Alignment::Center));
        let first = &out.lines[0].glyphs[0];
        assert_eq!(first.x, 90);
    }

    #[test]
    fn justify_last_line_stays_left_aligned() {
        let shaper = MonospaceMeasurer::new(10, 10);
        // Only one line — it IS the last — so justify stays at 0.
        let out = layout_paragraph("ab cd", &shaper, &opts(20, Alignment::Justify));
        let first = &out.lines[0].glyphs[0];
        assert_eq!(first.x, 0);
    }

    #[test]
    fn justify_stretches_intermediate_lines_to_column() {
        let shaper = MonospaceMeasurer::new(10, 10);
        // Column = 80, paragraph "ab cd ef gh ij kl" → multiple lines.
        // Intermediate lines should land the last glyph exactly on the
        // right column edge.
        let out = layout_paragraph("ab cd ef gh ij kl", &shaper, &opts(8, Alignment::Justify));
        assert!(out.lines.len() >= 2, "need ≥ 2 lines to exercise justify");
        let non_last: Vec<_> = out.lines.iter().take(out.lines.len() - 1).collect();
        for line in non_last {
            let last_glyph = line.glyphs.last().unwrap();
            // Last glyph sits at column_edge - last_glyph_advance.
            // advance = 10, column = 80 → last glyph x ≥ 70.
            assert!(
                last_glyph.x >= 70 - 2 && last_glyph.x <= 70 + 2,
                "expected last glyph near 70, got {}",
                last_glyph.x
            );
        }
    }

    #[test]
    fn layout_paragraph_uses_monospace_shaper_end_to_end() {
        let shaper = MonospaceMeasurer::new(10, 10);
        let o = opts(12, Alignment::Left);
        let out = layout_paragraph("lorem ipsum dolor sit amet", &shaper, &o);

        assert!(!out.lines.is_empty(), "no lines emitted");
        for w in out.lines.windows(2) {
            assert_eq!(w[1].baseline_y - w[0].baseline_y, 20);
        }
        assert_eq!(out.lines[0].baseline_y, 15);
        let line0 = &out.lines[0];
        for pair in line0.glyphs.windows(2) {
            assert!(pair[0].x <= pair[1].x);
        }
        let expected_width: i32 = line0.glyphs.iter().map(|_| 10).sum::<i32>();
        assert_eq!(line0.width, expected_width);
    }
}
