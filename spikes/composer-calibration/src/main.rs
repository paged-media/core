//! Spike B: Paragraph Composer calibration harness.
//!
//! Takes a JSON paragraph spec (font path, point size, column width, text),
//! shapes runs via `rustybuzz`, drives `paragraph-breaker`, and emits the
//! break positions. A companion script (not in this repo) compares these
//! against InDesign's line breaks to tune penalty weights.
//!
//! Pass criterion: ≥ 95% line-break parity on a 30-paragraph calibration
//! corpus. Below that, idea.md §4 fidelity contract needs renegotiation.

use anyhow::Result;
use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(name = "composer-calibration", version, about)]
struct Args {
    /// JSON file with a ParagraphSpec.
    spec: std::path::PathBuf,
    /// Emit JSON to stdout suitable for diffing against InDesign output.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ParagraphSpec {
    font: std::path::PathBuf,
    point_size: f32,
    column_width_pt: f32,
    text: String,
    /// Optional override for penalty weights under calibration.
    #[serde(default)]
    penalties: Penalties,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Penalties {
    #[serde(default)]
    hyphen: Option<f32>,
    #[serde(default)]
    adjacent_hyphen: Option<f32>,
    #[serde(default)]
    widow: Option<f32>,
    #[serde(default)]
    orphan: Option<f32>,
}

#[derive(Debug, Serialize)]
struct BreakReport {
    line_count: usize,
    breaks_char_index: Vec<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let spec_bytes = std::fs::read(&args.spec)?;
    let _spec: ParagraphSpec = serde_json::from_slice(&spec_bytes)?;

    // TODO: load font via ttf-parser, shape runs via rustybuzz,
    // construct Knuth-Plass items, run paragraph-breaker, emit break
    // positions. Throwaway code — full implementation in the spike
    // execution phase.
    let report = BreakReport {
        line_count: 0,
        breaks_char_index: vec![],
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        eprintln!("line_count = {}", report.line_count);
        eprintln!("breaks    = {:?}", report.breaks_char_index);
    }
    Ok(())
}
