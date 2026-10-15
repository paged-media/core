/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! Where InDesign sets the footnotes of a frame with columns (ADR 160),
//! pinned against its PDFs of `footnote-columns.idml` and
//! `footnote-straddle.idml`: under each column at the column's width, or
//! across the frame when the document (`EnableStraddling`) or the frame's
//! override (`FootnotesSpanAcrossColumns`) says so; an override with no
//! spacing of its own puts 6 pt between notes.

use std::path::PathBuf;

use paged_gen::samples::footnote_columns::{self as sample, Variant};
use paged_renderer::pipeline::BreakRecord;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

fn breaks(v: &Variant) -> Vec<BreakRecord> {
    let bytes = paged_gen::write_idml(&sample::build(v)).expect("write_idml");
    let document = idml_import::import_idml_doc(&bytes).expect("open");
    let mut resolver = BytesResolver::new();
    let inter = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    resolver.add_font("Inter", None, std::fs::read(inter).expect("Inter"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        collect_breaks: true,
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts)
        .expect("build")
        .breaks
}

/// A note's lines, as `(baseline, width)`, keyed by the paragraph it
/// annotates (`A1` …).
fn note_lines(b: &[BreakRecord], on: &str) -> Vec<(f32, f32)> {
    let mut lines: Vec<(f32, f32)> = Vec::new();
    let mut inside = false;
    for r in b {
        if r.source_text.contains("The note on") {
            inside = r.source_text.contains(&format!("note on {on},"));
        } else if !inside {
            continue;
        }
        if inside {
            lines.push((r.baseline_y_pt, r.width_pt));
            if r.source_text.ends_with("width.") {
                inside = false;
            }
        }
    }
    lines
}

/// A column is 204 pt wide; each note, set at a column's width, wraps
/// once. Set across the frame (420 pt) it takes one line.
const COLUMN_W: f32 = (sample::FRAME_W - sample::GUTTER) / 2.0;

fn assert_per_column(b: &[BreakRecord], notes: &[&str]) {
    for n in notes {
        let lines = note_lines(b, n);
        assert_eq!(lines.len(), 2, "{n} wraps at its column: {lines:?}");
        assert!(
            lines.iter().all(|(_, w)| *w <= COLUMN_W + 0.01),
            "{n}: {lines:?}"
        );
    }
}

fn assert_spanning(b: &[BreakRecord], notes: &[&str]) {
    for n in notes {
        let lines = note_lines(b, n);
        assert_eq!(lines.len(), 1, "{n} spans the frame: {lines:?}");
        assert!(lines[0].1 > COLUMN_W, "{n}: {lines:?}");
    }
}

#[test]
fn notes_stand_under_their_column_unless_they_span() {
    let b = breaks(&sample::COLUMNS);
    assert_per_column(&b, &["A1", "A2", "A3", "A4", "A5", "A6"]);
    assert_spanning(&b, &["B1", "B2", "B3", "B4"]);
    // Frame B overrides with no spacing of its own: 6 pt between notes
    // on top of the 9.6 pt leading (InDesign: 695.8, 711.4, 727.0, 742.6).
    let baselines: Vec<f32> = ["B1", "B2", "B3", "B4"]
        .iter()
        .map(|n| note_lines(&b, n)[0].0)
        .collect();
    for w in baselines.windows(2) {
        assert!((w[1] - w[0] - 15.6).abs() < 0.05, "{baselines:?}");
    }
}

#[test]
fn a_document_that_straddles_spans_every_frame_that_does_not_override() {
    let b = breaks(&sample::STRADDLE);
    assert_spanning(&b, &["A1", "A2", "A3", "A4", "A5", "A6"]);
    // In reference order, one under the other, with the document's
    // spacing (0): 9.6 pt apart.
    let baselines: Vec<f32> = ["A1", "A2", "A3", "A4", "A5", "A6"]
        .iter()
        .map(|n| note_lines(&b, n)[0].0)
        .collect();
    for w in baselines.windows(2) {
        assert!((w[1] - w[0] - 9.6).abs() < 0.05, "{baselines:?}");
    }
    assert_per_column(&b, &["B1", "B2", "B3", "B4"]);
}
