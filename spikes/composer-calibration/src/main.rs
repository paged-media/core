//! Spike B: Paragraph Composer calibration harness.
//!
//! Thin CLI wrapper around `paged_text::compose_paragraph`. Takes a JSON
//! paragraph spec (font path, point size, column width, text, penalty
//! knobs) and emits the chosen break positions.
//!
//! A companion script (not in this repo) compares these against
//! InDesign-exported break positions to iterate on tolerance /
//! stretch-ratio / looseness until we hit the 95% line-break parity
//! pass criterion.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use paged_text::{compose_paragraph, ComposeOptions, RustybuzzMeasurer};
use rustybuzz::Face;
use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    penalties: Penalties,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Penalties {
    #[serde(default)]
    tolerance: Option<f32>,
    #[serde(default)]
    looseness: Option<i32>,
    #[serde(default)]
    stretch_ratio: Option<f32>,
    #[serde(default)]
    shrink_ratio: Option<f32>,
}

#[derive(Debug, Serialize)]
struct BreakReport {
    line_count: usize,
    /// Byte ranges for each composed line.
    lines: Vec<LineReport>,
    column_width_pt: f32,
}

#[derive(Debug, Serialize)]
struct LineReport {
    start: usize,
    end: usize,
    /// Paragraph-breaker ratio. 0 = natural, >0 = stretched, <0 = shrunk.
    ratio: f32,
    /// Rendered line width in pt.
    width_pt: f32,
    /// Preview of the line text (bounded).
    preview: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let spec_bytes =
        std::fs::read(&args.spec).with_context(|| format!("read {}", args.spec.display()))?;
    let spec: ParagraphSpec = serde_json::from_slice(&spec_bytes)?;

    let font_bytes =
        std::fs::read(&spec.font).with_context(|| format!("read font {}", spec.font.display()))?;
    let face = Face::from_slice(&font_bytes, 0).context("not a valid TTF/OTF")?;

    let measurer = RustybuzzMeasurer::new(&face, spec.point_size);
    let mut opts = ComposeOptions::new(spec.column_width_pt);
    if let Some(t) = spec.penalties.tolerance {
        opts.tolerance = t;
    }
    if let Some(l) = spec.penalties.looseness {
        opts.looseness = l;
    }
    if let Some(s) = spec.penalties.stretch_ratio {
        opts.stretch_ratio = s;
    }
    if let Some(s) = spec.penalties.shrink_ratio {
        opts.shrink_ratio = s;
    }

    let lines = compose_paragraph(&spec.text, &measurer, &opts);

    let report = BreakReport {
        line_count: lines.len(),
        column_width_pt: spec.column_width_pt,
        lines: lines
            .into_iter()
            .map(|l| {
                let preview_bytes = &spec.text[l.byte_range.clone()];
                LineReport {
                    start: l.byte_range.start,
                    end: l.byte_range.end,
                    ratio: l.ratio,
                    width_pt: l.width as f32 / 64.0,
                    preview: truncate(preview_bytes, 72),
                }
            })
            .collect(),
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        eprintln!(
            "column = {:.2} pt   lines = {}",
            report.column_width_pt, report.line_count
        );
        for (i, line) in report.lines.iter().enumerate() {
            eprintln!(
                "  L{i:02}  {:>5}..{:<5}  ratio={:+.2}  w={:>6.2}pt  {}",
                line.start, line.end, line.ratio, line.width_pt, line.preview
            );
        }
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let mut out: String = chars.into_iter().take(max).collect();
        out.push('…');
        out
    }
}
