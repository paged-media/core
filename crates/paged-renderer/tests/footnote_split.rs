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

//! A note too tall for the room under its reference (ADR 161), pinned
//! against InDesign 20.0.1's PDFs of `footnote-split.idml` and
//! `footnote-nosplit.idml`: it keeps the whole lines that fit and
//! continues at the head of the next frame's area, or, with
//! `NoSplitting`, its reference line moves to the next frame.

use std::path::PathBuf;

use paged_gen::samples::footnote_split::{self as sample, Variant};
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

/// The baselines of note `n`'s lines.
fn note(b: &[BreakRecord], n: u32) -> Vec<f32> {
    b.iter()
        .filter(|r| r.story_id.ends_with(&format!("#footnote{n}")))
        .map(|r| r.baseline_y_pt)
        .collect()
}

/// The body line holding the first note's reference: its frame (0 or
/// 1) and its baseline below that frame's top.
fn reference_line(b: &[BreakRecord]) -> (u32, f32) {
    let r = b
        .iter()
        .find(|r| !r.story_id.contains("#footnote") && r.source_text.starts_with("line full"))
        .expect("the reference line");
    (r.frame_idx, r.baseline_y_pt)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.05
}

#[test]
fn a_note_too_tall_continues_in_the_next_frame() {
    let b = breaks(&sample::SPLIT);
    let n1 = note(&b, 1);
    // Nine lines on frame 1's foot (330), two at the head of frame 2's
    // area, which ends on frame 2's foot (770) after notes 2 and 3.
    assert_eq!(n1.len(), 11, "{n1:?}");
    assert!(close(n1[8], 330.0), "{n1:?}");
    assert!(close(n1[9], 741.2) && close(n1[10], 750.8), "{n1:?}");
    assert!(close(note(&b, 2)[0], 760.4) && close(note(&b, 3)[0], 770.0));
    // The reference line stays in frame 1; the next paragraph opens
    // frame 2 (its first baseline 9.69 below the frame's top).
    assert_eq!(reference_line(&b).0, 0);
    let p2 = b
        .iter()
        .find(|r| r.source_text.starts_with("P2 sentence 1"))
        .unwrap();
    assert_eq!(p2.frame_idx, 1);
    assert!(close(p2.baseline_y_pt, 9.69), "{}", p2.baseline_y_pt);
}

#[test]
fn with_no_splitting_the_reference_line_moves_on() {
    let b = breaks(&sample::NOSPLIT);
    // The reference line opens frame 2, and the whole note goes with it.
    let (frame, baseline) = reference_line(&b);
    assert!(frame == 1 && close(baseline, 9.69), "{frame} {baseline}");
    let n1 = note(&b, 1);
    assert_eq!(n1.len(), 11, "{n1:?}");
    assert!(n1.iter().all(|&y| y > 440.0), "{n1:?}");
    assert!(close(note(&b, 3)[0], 770.0));
}
