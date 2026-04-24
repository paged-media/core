//! Spike B: Paragraph Composer calibration harness.
//!
//! Takes a JSON paragraph spec (font path, point size, column width, text),
//! shapes each whitespace-separated word with `rustybuzz`, builds a
//! Knuth-Plass item stream, runs `paragraph-breaker::total_fit`, and emits
//! the chosen break positions.
//!
//! A companion script (not in this repo) compares these against
//! InDesign-exported break positions to iterate on penalty weights and
//! glue parameters.
//!
//! Pass criterion: ≥ 95% line-break parity on a 30-paragraph calibration
//! corpus. Below that, idea.md §4 fidelity contract needs renegotiation.

use anyhow::{Context, Result};
use clap::Parser;
use paragraph_breaker::{Breakpoint, Item};
use rustybuzz::{Face, UnicodeBuffer};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "composer-calibration", version, about)]
struct Args {
    /// JSON file with a ParagraphSpec.
    spec: PathBuf,
    /// Emit JSON to stdout suitable for diffing against InDesign output.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ParagraphSpec {
    font: PathBuf,
    point_size: f32,
    column_width_pt: f32,
    text: String,
    /// Optional overrides for calibration knobs.
    #[serde(default)]
    penalties: Penalties,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Penalties {
    /// Tolerance passed to `total_fit`. Higher = more permissive fitting.
    #[serde(default = "default_tolerance")]
    tolerance: f32,
    /// Looseness bias: >0 prefers longer paragraphs, <0 prefers shorter.
    #[serde(default)]
    looseness: i32,
}

fn default_tolerance() -> f32 {
    2.0
}

#[derive(Debug, Serialize)]
struct BreakReport {
    line_count: usize,
    /// Break positions expressed as byte offsets into the original text.
    byte_offsets: Vec<usize>,
    /// Ratios reported by paragraph-breaker (loose > 0, tight < 0).
    ratios: Vec<f32>,
    /// Column width used for the solve, in font-design units scaled to pt.
    column_width_pt: f32,
}

/// Minimal "word + trailing space" item carrying its byte-range metadata.
struct Word {
    /// Byte offset of the first character of this word. (Retained for
    /// future cross-referencing against InDesign break positions even
    /// though the current report only emits `end`.)
    #[allow(dead_code)]
    start: usize,
    /// Byte offset just past the end of this word.
    end: usize,
    /// Advance width in 64ths of a font-unit (to match shaping resolution).
    width: i32,
    /// Width of the trailing space (glue), if any.
    space: Option<i32>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let spec_bytes =
        std::fs::read(&args.spec).with_context(|| format!("read {}", args.spec.display()))?;
    let spec: ParagraphSpec = serde_json::from_slice(&spec_bytes)?;

    let font_bytes =
        std::fs::read(&spec.font).with_context(|| format!("read font {}", spec.font.display()))?;
    let face = Face::from_slice(&font_bytes, 0).context("not a valid TTF/OTF")?;

    let units_per_em = face.units_per_em() as f32;
    // Scale factor from design units to integer-pt * 64 (paragraph-breaker
    // works in i32; 64× gives us sub-pt precision without floats).
    let scale =
        |u: i32| -> i32 { ((u as f32) * spec.point_size * 64.0 / units_per_em).round() as i32 };

    let words = segment_and_shape(&spec.text, &face, scale);
    let items = build_items(&words);
    let line_length = (spec.column_width_pt * 64.0).round() as i32;

    let breaks: Vec<Breakpoint> = paragraph_breaker::total_fit(
        &items,
        &[line_length],
        spec.penalties.tolerance,
        spec.penalties.looseness,
    );

    let byte_offsets = breaks
        .iter()
        .filter_map(|b| words.get(b.index / 2).map(|w| w.end))
        .collect::<Vec<_>>();
    let ratios = breaks.iter().map(|b| b.ratio).collect::<Vec<_>>();

    let report = BreakReport {
        line_count: breaks.len(),
        byte_offsets,
        ratios,
        column_width_pt: spec.column_width_pt,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        eprintln!("line_count = {}", report.line_count);
        eprintln!("byte_offsets = {:?}", report.byte_offsets);
        eprintln!("ratios = {:?}", report.ratios);
    }
    Ok(())
}

fn segment_and_shape(text: &str, face: &Face, scale: impl Fn(i32) -> i32) -> Vec<Word> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let bytes = text.as_bytes();

    while cursor < bytes.len() {
        // Skip any leading whitespace (treated as part of the *previous* word
        // as trailing glue, so nothing to emit here at the paragraph start).
        let word_start = cursor;
        while cursor < bytes.len() && !is_ws(bytes[cursor]) {
            cursor += 1;
        }
        let word_end = cursor;
        // Collect trailing whitespace run (single glue per break opportunity).
        let ws_start = cursor;
        while cursor < bytes.len() && is_ws(bytes[cursor]) {
            cursor += 1;
        }
        let has_space = cursor > ws_start;

        if word_start == word_end {
            // Leading whitespace at paragraph start — skip without emitting.
            continue;
        }

        let word_text = &text[word_start..word_end];
        let width = shape_width(word_text, face, &scale);
        let space_width = if has_space {
            Some(shape_width(&text[ws_start..cursor], face, &scale))
        } else {
            None
        };

        out.push(Word {
            start: word_start,
            end: word_end,
            width,
            space: space_width,
        });
    }
    out
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn shape_width(text: &str, face: &Face, scale: impl Fn(i32) -> i32) -> i32 {
    let mut buf = UnicodeBuffer::new();
    buf.push_str(text);
    let shaped = rustybuzz::shape(face, &[], buf);
    shaped
        .glyph_positions()
        .iter()
        .map(|p| scale(p.x_advance))
        .sum::<i32>()
}

fn build_items(words: &[Word]) -> Vec<Item<usize>> {
    // Pattern: Box (word) [Glue (space)]  repeated, closed by a forced break.
    // `data` field carries the word index so we can map Breakpoint -> Word.
    let mut items = Vec::with_capacity(words.len() * 2 + 2);
    for (i, w) in words.iter().enumerate() {
        items.push(Item::Box {
            width: w.width,
            data: i,
        });
        if let Some(space) = w.space {
            items.push(Item::Glue {
                width: space,
                stretch: space / 2,
                shrink: space / 3,
            });
        }
    }
    // Force a line break at paragraph end (TeX convention).
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
    items
}
