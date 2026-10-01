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

//! The forced line break (U+2028) over the generated
//! `forced-line-break.idml`, against where InDesign 2025 put each line in
//! its PDF export of the same file (`corpus/generated/forced-line-break.pdf`,
//! 2026-10-01).
//!
//! The IDML importer (plugin-publish `idml-import`) keeps U+2028 in the
//! run, as InDesign spells it inside `<Content>`, so the fixture goes
//! through the real import path: no body story may carry a `\n`, which
//! the engine would read as a paragraph mark.

use paged_gen::samples::forced_line_break::{body_story_id, cases, frame_origin, FRAME_W};
use paged_renderer::{pipeline, PipelineOptions};

/// First word of each laid-out line, per case, as InDesign placed it:
/// (left edge x, top of the word box y), from `pdftotext -bbox`. Empty
/// lines carry no word, so they show only as the gap they leave.
const INDESIGN: [&[(f32, f32)]; 7] = [
    // Alpha (first-line indent) / Bravo / Charlie / Delta: no indent or
    // spacing on the broken lines; space after (10) + before (8) once.
    &[
        (96.125, 73.045),
        (72.125, 85.045),
        (72.125, 97.045),
        (96.125, 127.045),
    ],
    // • Echo / Foxtrot (no bullet, at the left indent) / • Golf.
    &[(312.125, 73.045), (330.125, 85.045), (312.125, 97.045)],
    // 1. Hotel / India (no number) / 2. Juliet.
    &[(72.125, 263.045), (90.125, 275.045), (72.125, 287.045)],
    // Kilo / (empty) / Lima / Mike.
    &[(312.125, 263.045), (312.125, 287.045), (312.125, 299.045)],
    // November / (the line the trailing break leaves) / Oscar.
    &[(72.125, 453.045), (72.125, 477.045)],
    // Papa quebec romeo (JUSTIFIED: romeo ends at 511.875) / Sierra / Uniform.
    &[(312.125, 453.045), (312.125, 465.045), (312.125, 477.045)],
    // (empty first line) / Victor / Whiskey.
    &[(72.125, 655.045), (72.125, 667.045)],
];

/// InDesign's word box top sits this far above the baseline (Inter 10 pt;
/// poppler's box is the font descriptor's ascent): the body frames carry a
/// 0.25 pt centre stroke, which insets the text area by 0.125 pt
/// (`stroke-inset`), so the first baseline of a frame at y=72 is 84.125
/// and its words' tops 73.045. The word box starts at the pen, so the x
/// values are the pen's too (72.125 at a frame-edge).
const WORD_TOP_TO_BASELINE: f32 = 11.08;
const X_TOLERANCE: f32 = 0.05;
const Y_TOLERANCE: f32 = 0.05;

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

#[test]
fn forced_line_breaks_land_where_indesign_puts_them() {
    let bytes =
        paged_gen::write_idml(&paged_gen::samples::forced_line_break::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let bodies: Vec<String> = (0..cases().len() as u32).map(body_story_id).collect();
    let mut breaks = 0;
    for s in doc.stories.iter().filter(|s| bodies.contains(&s.self_id)) {
        for run in s.story.paragraphs.iter().flat_map(|p| &p.runs) {
            assert!(
                !run.text.contains('\n'),
                "{}: the importer turned a forced line break into a paragraph mark: {:?}",
                s.self_id,
                run.text
            );
            breaks += run.text.matches('\u{2028}').count();
        }
    }
    assert!(
        breaks >= 7,
        "the fixture's forced line breaks reach the model ({breaks})"
    );
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");

    let mut report = Vec::new();
    let mut ok = true;
    for (i, case) in cases().iter().enumerate() {
        let lines = built.story_layout(&body_story_id(i as u32));
        let engine: Vec<(f32, f32)> = lines
            .iter()
            .map(|l| {
                let x = l.clusters.first().map(|c| c.x_pt).unwrap_or(f32::NAN);
                (x, l.baseline_y_pt - WORD_TOP_TO_BASELINE)
            })
            .collect();
        let want = INDESIGN[i];
        let same = engine.len() == want.len()
            && engine
                .iter()
                .zip(want)
                .all(|(e, w)| (e.0 - w.0).abs() <= X_TOLERANCE && (e.1 - w.1).abs() <= Y_TOLERANCE);
        ok &= same;
        report.push(format!(
            "{:28} {}\n    engine   {engine:?}\n    indesign {want:?}",
            case.name,
            if same { "ok" } else { "DIFFERS" }
        ));
    }
    assert!(ok, "\n{}", report.join("\n"));

    // One paragraph per fixture paragraph: the broken lines share it.
    let indent = built.story_layout(&body_story_id(0));
    let paras: Vec<u32> = indent.iter().map(|l| l.paragraph_idx).collect();
    assert_eq!(paras, [0, 0, 0, 1]);
    let idx: Vec<u32> = indent.iter().map(|l| l.line_idx).collect();
    assert_eq!(idx, [0, 1, 2, 0], "line indices run across the paragraph");
    // Bytes are the paragraph's: "Alpha one" 0..9, U+2028 (3 bytes),
    // "Bravo two" from 12.
    assert_eq!(indent[1].byte_range.start, 12);

    // The justified case: InDesign sets the line before the forced break
    // to full measure (its last word ends at 511.875 = the text area's
    // right edge, half the stroke inside the frame's), and the
    // paragraph's real last line ragged.
    let (fx, _) = frame_origin(5);
    let just = built.story_layout(&body_story_id(5));
    let right = |l: &pipeline::LineLayout| {
        l.clusters
            .iter()
            .map(|c| c.x_pt + c.advance_pt)
            .fold(f32::MIN, f32::max)
    };
    assert!(
        (right(just[0]) - (fx + FRAME_W - 0.125)).abs() <= X_TOLERANCE,
        "the line before the break is justified: ends at {}",
        right(just[0])
    );
    assert!(
        right(just[1]) < fx + FRAME_W - 100.0,
        "the paragraph's last line stays ragged: ends at {}",
        right(just[1])
    );
}
