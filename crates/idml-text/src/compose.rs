//! Paragraph composition: Knuth-Plass line breaking with calibrated
//! penalty weights.
//!
//! The composer works in 1/64 pt integer arithmetic (matching
//! `shape_run`) so results are stable and reproducible. A
//! `AdvanceMeasurer` trait abstracts width measurement so:
//!
//! * Production code plugs in `RustybuzzMeasurer` (real shaping).
//! * Tests and tooling can use `MonospaceMeasurer` (every char N units,
//!   every space M units) — fast, deterministic, no font required.
//!
//! Calibration of the penalty knobs against InDesign's Paragraph
//! Composer happens in `spikes/composer-calibration`; this module owns
//! the shape of the calibration surface (tolerance, looseness, glue
//! stretch/shrink ratios).

use paragraph_breaker::{Breakpoint, Item};
use rustybuzz::Face;

use crate::shape::{shape_run, ShapedGlyph, ShapedRun, ADVANCE_PRECISION};

/// Abstraction over width measurement. Returns advances in 1/64 pt.
pub trait AdvanceMeasurer {
    /// Advance width of `text`. `text` must not contain whitespace.
    fn measure_word(&self, text: &str) -> i32;
    /// Advance width of a single inter-word break opportunity.
    fn space_width(&self) -> i32;
}

/// Produces per-glyph data the layout pass needs to position text.
///
/// This sits *above* [`AdvanceMeasurer`]: every shaper is also a
/// measurer (measurement is just `shape(text).total_advance`). The
/// two traits are separate because the composer only needs widths —
/// keeping the cheaper path allocation-free where possible.
pub trait TextShaper: AdvanceMeasurer {
    /// Shape `text` into glyph ids + advances at the shaper's point
    /// size. Units are 1/64 pt.
    fn shape(&self, text: &str) -> ShapedRun;
}

/// Knobs the calibration spike tunes against InDesign.
#[derive(Debug, Clone)]
pub struct ComposeOptions {
    /// Column width in 1/64 pt.
    pub column_width: i32,
    /// paragraph-breaker tolerance. Higher = more permissive fits.
    pub tolerance: f32,
    /// Looseness bias: >0 prefers longer paragraphs, <0 shorter.
    pub looseness: i32,
    /// Inter-word glue: stretch as a fraction of `space_width`.
    pub stretch_ratio: f32,
    /// Inter-word glue: shrink as a fraction of `space_width`.
    pub shrink_ratio: f32,
}

impl ComposeOptions {
    /// Defaults chosen as a reasonable starting point; the composer
    /// spike calibrates these against InDesign before the text engine
    /// takes them as fixed.
    pub fn new(column_width_pt: f32) -> Self {
        Self {
            column_width: (column_width_pt * ADVANCE_PRECISION).round() as i32,
            tolerance: 4.0,
            looseness: 0,
            stretch_ratio: 1.0,
            shrink_ratio: 0.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComposedLine {
    /// Byte range of the source paragraph this line covers (after
    /// trailing space is trimmed).
    pub byte_range: std::ops::Range<usize>,
    /// Content width of the line, 1/64 pt.
    pub width: i32,
    /// Justification ratio: 0 = natural width, >0 = stretched, <0 =
    /// shrunk. Matches paragraph-breaker's convention.
    pub ratio: f32,
}

/// Compose one paragraph.
///
/// Splits `text` into words by ASCII whitespace, measures each with
/// `measurer`, builds a Knuth-Plass item stream, and invokes
/// `paragraph_breaker::total_fit`. Returns the resulting line sequence.
pub fn compose_paragraph(
    text: &str,
    measurer: &dyn AdvanceMeasurer,
    options: &ComposeOptions,
) -> Vec<ComposedLine> {
    let words = segment(text);
    if words.is_empty() {
        return Vec::new();
    }
    let space_width = measurer.space_width();
    let stretch = (space_width as f32 * options.stretch_ratio).round() as i32;
    let shrink = (space_width as f32 * options.shrink_ratio).round() as i32;

    let mut items: Vec<Item<usize>> = Vec::with_capacity(words.len() * 2 + 2);
    for (i, w) in words.iter().enumerate() {
        let width = measurer.measure_word(&text[w.start..w.end]);
        items.push(Item::Box { width, data: i });
        if i + 1 < words.len() {
            items.push(Item::Glue {
                width: space_width,
                stretch,
                shrink,
            });
        }
    }
    // Paragraph end: infinite stretch + forced break (TeX convention).
    items.push(Item::Glue {
        width: 0,
        stretch: paragraph_breaker::INFINITE_PENALTY,
        shrink: 0,
    });
    items.push(Item::Penalty {
        width: 0,
        penalty: -paragraph_breaker::INFINITE_PENALTY,
        flagged: true,
    });

    let breaks: Vec<Breakpoint> = paragraph_breaker::total_fit(
        &items,
        &[options.column_width],
        options.tolerance,
        options.looseness,
    );

    // Translate Breakpoints (item indices) back into word-end byte offsets.
    let mut lines = Vec::with_capacity(breaks.len());
    let mut start_word = 0usize;
    for bp in &breaks {
        // Find the last Box whose item index is ≤ bp.index.
        let last_box = (0..=bp.index).rev().find_map(|i| match items.get(i) {
            Some(Item::Box { data, .. }) => Some(*data),
            _ => None,
        });
        let Some(end_word) = last_box else {
            continue;
        };
        let byte_start = words[start_word].start;
        let byte_end = words[end_word].end;
        lines.push(ComposedLine {
            byte_range: byte_start..byte_end,
            width: bp.width,
            ratio: bp.ratio,
        });
        start_word = end_word + 1;
    }
    lines
}

#[derive(Debug, Clone, Copy)]
struct WordSpan {
    start: usize,
    end: usize,
}

fn segment(text: &str) -> Vec<WordSpan> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        while cursor < bytes.len() && is_ws(bytes[cursor]) {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && !is_ws(bytes[cursor]) {
            cursor += 1;
        }
        if cursor > start {
            out.push(WordSpan { start, end: cursor });
        }
    }
    out
}

#[inline]
fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// Production measurer: shapes each word via `rustybuzz` at the given
/// point size and reads back advance widths.
pub struct RustybuzzMeasurer<'a> {
    pub face: &'a Face<'a>,
    pub point_size: f32,
}

impl<'a> RustybuzzMeasurer<'a> {
    pub fn new(face: &'a Face<'a>, point_size: f32) -> Self {
        Self { face, point_size }
    }
}

impl AdvanceMeasurer for RustybuzzMeasurer<'_> {
    fn measure_word(&self, text: &str) -> i32 {
        shape_run(self.face, text, self.point_size).total_advance
    }

    fn space_width(&self) -> i32 {
        shape_run(self.face, " ", self.point_size).total_advance
    }
}

impl TextShaper for RustybuzzMeasurer<'_> {
    fn shape(&self, text: &str) -> ShapedRun {
        shape_run(self.face, text, self.point_size)
    }
}

/// Deterministic measurer used in tests and by tooling that doesn't want
/// to ship a TTF. Treats each Unicode scalar as having a fixed width.
pub struct MonospaceMeasurer {
    pub char_width: i32,
    pub space_width: i32,
}

impl MonospaceMeasurer {
    pub fn new(char_width: i32, space_width: i32) -> Self {
        Self {
            char_width,
            space_width,
        }
    }
}

impl AdvanceMeasurer for MonospaceMeasurer {
    fn measure_word(&self, text: &str) -> i32 {
        text.chars().count() as i32 * self.char_width
    }

    fn space_width(&self) -> i32 {
        self.space_width
    }
}

impl TextShaper for MonospaceMeasurer {
    /// Produces a synthetic `ShapedRun` — one glyph per Unicode scalar.
    /// Useful for layout-pass tests without shipping a test font.
    fn shape(&self, text: &str) -> ShapedRun {
        let mut byte_cursor = 0u32;
        let mut total = 0i32;
        let glyphs: Vec<ShapedGlyph> = text
            .chars()
            .map(|c| {
                let cluster = byte_cursor;
                byte_cursor += c.len_utf8() as u32;
                let advance = if c.is_whitespace() {
                    self.space_width
                } else {
                    self.char_width
                };
                total += advance;
                ShapedGlyph {
                    glyph_id: c as u32,
                    cluster,
                    x_advance: advance,
                    y_offset: 0,
                    x_offset: 0,
                }
            })
            .collect();
        ShapedRun {
            glyphs,
            total_advance: total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str, column_chars: i32) -> Vec<String> {
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: column_chars * 10,
            tolerance: 10.0,
            stretch_ratio: 1.0,
            shrink_ratio: 0.5,
            ..ComposeOptions::new(0.0)
        };
        compose_paragraph(text, &m, &opts)
            .into_iter()
            .map(|l| text[l.byte_range].to_string())
            .collect()
    }

    #[test]
    fn single_word_paragraph() {
        let ls = lines("hello", 20);
        assert_eq!(ls, vec!["hello"]);
    }

    #[test]
    fn paragraph_wraps_to_multiple_lines() {
        // "lorem ipsum dolor sit amet" — 26 chars total with spaces.
        // column_chars = 12 forces a break after "lorem ipsum" (11 chars).
        let ls = lines("lorem ipsum dolor sit amet", 12);
        assert!(ls.len() >= 2, "expected >=2 lines, got {:?}", ls);
        // First line ends at or before the 12th char boundary.
        assert!(ls[0].len() <= 12, "first line too long: {:?}", ls);
        // Round-trip: joining lines with spaces reproduces the input.
        assert_eq!(ls.join(" "), "lorem ipsum dolor sit amet");
    }

    #[test]
    fn empty_paragraph_returns_no_lines() {
        let ls = lines("", 40);
        assert!(ls.is_empty());
    }

    #[test]
    fn line_widths_are_populated() {
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: 120,
            ..ComposeOptions::new(0.0)
        };
        let out = compose_paragraph("one two three four five", &m, &opts);
        for line in &out {
            assert!(line.width > 0, "width not set: {:?}", line);
        }
    }
}
