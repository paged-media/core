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

use crate::hyphenate::Hyphenator;
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
///
/// `'a` is the lifetime of the optional `&Hyphenator` borrow. Building
/// `ComposeOptions` without a hyphenator imposes no lifetime obligation
/// on callers — the lifetime is inferred to `'static`.
#[derive(Debug, Clone)]
pub struct ComposeOptions<'a> {
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
    /// Optional hyphenation engine. When set, the composer emits
    /// flagged Penalty items at every TeX-pattern break opportunity
    /// inside each word; paragraph-breaker decides whether to take
    /// them based on `tolerance` and `hyphen_penalty`.
    pub hyphenator: Option<&'a Hyphenator>,
    /// Penalty cost paid when a line is broken at a hyphenation
    /// opportunity. Knuth-Plass convention: 50 = mildly penalised,
    /// 100 = costly. Only consulted when `hyphenator` is set.
    pub hyphen_penalty: i32,
}

impl ComposeOptions<'_> {
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
            hyphenator: None,
            hyphen_penalty: 50,
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
    /// True when the line break landed on a flagged hyphenation
    /// penalty mid-word. The layout pass appends a `-` glyph after
    /// the line's last glyph so the rendered output reads correctly.
    pub ends_with_hyphen: bool,
}

/// Compose one paragraph.
///
/// Splits `text` into words by ASCII whitespace, measures each with
/// `measurer`, builds a Knuth-Plass item stream, and invokes
/// `paragraph_breaker::total_fit`. Returns the resulting line sequence.
///
/// When `options.hyphenator` is set, each word is split into segments
/// at every TeX-pattern break point and a flagged Penalty is emitted
/// between segments so paragraph-breaker can hyphenate mid-word.
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
    let hyphen_width = if options.hyphenator.is_some() {
        measurer.measure_word("-")
    } else {
        0
    };

    // `items` is what paragraph-breaker sees. `byte_ends` and
    // `is_hyphen_break` are parallel arrays indexed the same way so we
    // can map any chosen break back to a source byte offset and learn
    // whether the break needs a trailing hyphen glyph.
    let mut items: Vec<Item<()>> = Vec::with_capacity(words.len() * 4 + 2);
    let mut byte_ends: Vec<usize> = Vec::with_capacity(items.capacity());
    let mut is_hyphen_break: Vec<bool> = Vec::with_capacity(items.capacity());

    for (i, w) in words.iter().enumerate() {
        let word_text = &text[w.start..w.end];
        // Hyphenation breaks are byte offsets inside the word; filter
        // out any sentinel 0/len entries the dictionary might produce.
        let breaks: Vec<usize> = options
            .hyphenator
            .map(|h| h.opportunities(word_text))
            .unwrap_or_default()
            .into_iter()
            .filter(|&b| b > 0 && b < word_text.len())
            .collect();

        let mut seg_start = 0usize;
        for &offset in &breaks {
            if offset <= seg_start {
                continue;
            }
            let seg = &word_text[seg_start..offset];
            let width = measurer.measure_word(seg);
            items.push(Item::Box { width, data: () });
            byte_ends.push(w.start + offset);
            is_hyphen_break.push(false);
            items.push(Item::Penalty {
                width: hyphen_width,
                penalty: options.hyphen_penalty,
                flagged: true,
            });
            byte_ends.push(w.start + offset);
            is_hyphen_break.push(true);
            seg_start = offset;
        }
        let final_seg = &word_text[seg_start..];
        let final_w = measurer.measure_word(final_seg);
        items.push(Item::Box {
            width: final_w,
            data: (),
        });
        byte_ends.push(w.end);
        is_hyphen_break.push(false);

        if i + 1 < words.len() {
            items.push(Item::Glue {
                width: space_width,
                stretch,
                shrink,
            });
            // Glue between words: a break here trims the trailing
            // space, so byte_end is the previous word's end.
            byte_ends.push(w.end);
            is_hyphen_break.push(false);
        }
    }
    // Paragraph end: infinite stretch + forced break (TeX convention).
    items.push(Item::Glue {
        width: 0,
        stretch: paragraph_breaker::INFINITE_PENALTY,
        shrink: 0,
    });
    byte_ends.push(text.len());
    is_hyphen_break.push(false);
    items.push(Item::Penalty {
        width: 0,
        penalty: -paragraph_breaker::INFINITE_PENALTY,
        flagged: true,
    });
    byte_ends.push(text.len());
    is_hyphen_break.push(false);

    let breaks: Vec<Breakpoint> = paragraph_breaker::total_fit(
        &items,
        &[options.column_width],
        options.tolerance,
        options.looseness,
    );

    // Translate Breakpoints (item indices) into byte ranges. A break
    // at a flagged hyphenation penalty marks the line for hyphen
    // rendering at the layout pass.
    let mut lines = Vec::with_capacity(breaks.len());
    let mut byte_cursor = 0usize;
    for bp in &breaks {
        let Some(&end) = byte_ends.get(bp.index) else {
            continue;
        };
        let hyphen = is_hyphen_break.get(bp.index).copied().unwrap_or(false);
        // Skip whitespace at the line's left edge (after a glue
        // break) so byte_range tracks the visible content.
        let mut start = byte_cursor;
        let bytes = text.as_bytes();
        while start < end && is_ws(bytes[start]) {
            start += 1;
        }
        if start >= end {
            continue;
        }
        lines.push(ComposedLine {
            byte_range: start..end,
            width: bp.width,
            ratio: bp.ratio,
            ends_with_hyphen: hyphen,
        });
        byte_cursor = end;
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
    fn hyphenation_inserts_mid_word_breaks_when_needed() {
        // A line that fits exactly when broken at a hyphenation
        // penalty inside the second word, and can't fit without one
        // (a single-word line has no inner glue to absorb slack).
        let m = MonospaceMeasurer::new(10, 10);
        let h = crate::Hyphenator::for_language(crate::Language::EnglishUS);
        let mut opts = ComposeOptions::new(0.0);
        opts.column_width = 80;
        opts.tolerance = 20.0;
        opts.hyphenator = Some(&h);
        let out = compose_paragraph("the elephants", &m, &opts);
        assert!(
            out.iter().any(|l| l.ends_with_hyphen),
            "expected at least one hyphenated line: {:?}",
            out
        );
    }

    #[test]
    fn no_hyphenator_means_no_hyphen_flag() {
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: 200,
            ..ComposeOptions::new(0.0)
        };
        let out = compose_paragraph("optimisation works well", &m, &opts);
        for l in &out {
            assert!(!l.ends_with_hyphen, "no hyphenator → no flag");
        }
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
