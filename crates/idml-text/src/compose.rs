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
    /// Column width in 1/64 pt. Used as the only width when
    /// `column_widths` is `None`, and as the fallback for lines past
    /// the end of `column_widths`.
    pub column_width: i32,
    /// Per-line column widths in 1/64 pt. Index `i` is the width
    /// available for the `i`-th composed line; lines past the end
    /// of the slice fall back to `column_width`. `None` means "use
    /// `column_width` for every line" (the legacy single-width
    /// shape). Drives text-wrap-around-objects and other layouts
    /// where a wrap rectangle carves a hole out of specific lines.
    pub column_widths: Option<Vec<i32>>,
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
            column_widths: None,
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

    // Per-item metadata kept in lockstep with the items vector via
    // `push_item`. paragraph-breaker takes `&[Item<()>]`, so we keep
    // the items in their own contiguous Vec — but every push goes
    // through the helper so the byte_end / is_hyphen side-data can
    // never drift out of sync.
    let item_capacity = if options.hyphenator.is_some() {
        words.len() * 4 + 2
    } else {
        words.len() * 2 + 2
    };
    let mut items: Vec<Item<()>> = Vec::with_capacity(item_capacity);
    let mut meta: Vec<ItemMeta> = Vec::with_capacity(item_capacity);
    let push = |items: &mut Vec<_>, meta: &mut Vec<ItemMeta>, item, byte_end, is_hyphen| {
        items.push(item);
        meta.push(ItemMeta {
            byte_end,
            is_hyphen,
        });
    };

    for (i, w) in words.iter().enumerate() {
        let word_text = &text[w.start..w.end];
        // No hyphenator → emit a single Box for the whole word and
        // skip the per-word break-vec construction entirely.
        match options.hyphenator {
            None => {
                push(
                    &mut items,
                    &mut meta,
                    Item::Box {
                        width: measurer.measure_word(word_text),
                        data: (),
                    },
                    w.end,
                    false,
                );
            }
            Some(h) => {
                let mut seg_start = 0usize;
                for offset in h.opportunities(word_text) {
                    if offset <= seg_start || offset >= word_text.len() {
                        continue;
                    }
                    push(
                        &mut items,
                        &mut meta,
                        Item::Box {
                            width: measurer.measure_word(&word_text[seg_start..offset]),
                            data: (),
                        },
                        w.start + offset,
                        false,
                    );
                    push(
                        &mut items,
                        &mut meta,
                        Item::Penalty {
                            width: hyphen_width,
                            penalty: options.hyphen_penalty,
                            flagged: true,
                        },
                        w.start + offset,
                        true,
                    );
                    seg_start = offset;
                }
                push(
                    &mut items,
                    &mut meta,
                    Item::Box {
                        width: measurer.measure_word(&word_text[seg_start..]),
                        data: (),
                    },
                    w.end,
                    false,
                );
            }
        }

        if i + 1 < words.len() {
            push(
                &mut items,
                &mut meta,
                Item::Glue {
                    width: space_width,
                    stretch,
                    shrink,
                },
                // A break at this glue trims the trailing space, so
                // the byte_end is the previous word's end.
                w.end,
                false,
            );
        }
    }
    // Paragraph end: infinite stretch + forced break (TeX convention).
    push(
        &mut items,
        &mut meta,
        Item::Glue {
            width: 0,
            stretch: paragraph_breaker::INFINITE_PENALTY,
            shrink: 0,
        },
        text.len(),
        false,
    );
    push(
        &mut items,
        &mut meta,
        Item::Penalty {
            width: 0,
            penalty: -paragraph_breaker::INFINITE_PENALTY,
            flagged: true,
        },
        text.len(),
        false,
    );

    // Per-line widths drive text-wrap-around-objects: each line's
    // available width is computed from any wrap rectangles
    // overlapping that line's predicted y-range. Without an
    // explicit `column_widths`, every line uses `column_width`.
    let single_width = [options.column_width];
    let lengths: &[i32] = options
        .column_widths
        .as_deref()
        .filter(|v| !v.is_empty())
        .unwrap_or(&single_width);
    let breaks: Vec<Breakpoint> =
        paragraph_breaker::total_fit(&items, lengths, options.tolerance, options.looseness);

    // Translate Breakpoints (item indices) into byte ranges. A break
    // at a flagged hyphenation penalty marks the line for hyphen
    // rendering at the layout pass.
    let mut lines = Vec::with_capacity(breaks.len());
    let mut byte_cursor = 0usize;
    let bytes = text.as_bytes();
    for bp in &breaks {
        let Some(m) = meta.get(bp.index) else {
            continue;
        };
        // Skip whitespace at the line's left edge (after a glue
        // break) so byte_range tracks visible content.
        let mut start = byte_cursor;
        while start < m.byte_end && bytes[start].is_ascii_whitespace() {
            start += 1;
        }
        if start >= m.byte_end {
            continue;
        }
        lines.push(ComposedLine {
            byte_range: start..m.byte_end,
            width: bp.width,
            ratio: bp.ratio,
            ends_with_hyphen: m.is_hyphen,
        });
        byte_cursor = m.byte_end;
    }
    lines
}

struct ItemMeta {
    byte_end: usize,
    is_hyphen: bool,
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
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor > start {
            out.push(WordSpan { start, end: cursor });
        }
    }
    out
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
