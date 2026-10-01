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

//! A text frame's STROKE moves its text, over the generated
//! `stroke-inset.idml`, line by line against where InDesign 20.0.1 put
//! every line of the same file (2026-10-01; the fixture is also in the
//! fidelity gate with InDesign's PDF export as reference).
//!
//! The rule InDesign follows (`text_area_frame` implements it): a stroke
//! the frame PAINTS insets the text area on all four sides — by half its
//! weight when centred, all of it inside, none outside — on top of
//! `InsetSpacing`. The first baseline moves down by it, the last line
//! has that much less room, a multi-column frame keeps its gutter and
//! narrows its columns, and a shaped frame's outline is eroded by the
//! sum. A weight without a colour moves nothing.

use paged_gen::samples::stroke_inset::{body_story_id, cases, is_right};
use paged_renderer::{pipeline, PipelineOptions};

/// Every line InDesign set, per case: (case, text, x, end x, baseline),
/// page points — `line.horizontalOffset`, `line.endHorizontalOffset`
/// and `line.baseline` read from InDesign's DOM after opening the
/// generated IDML; they agree with its PDF export (`pdftotext -bbox`).
const INDESIGN: &[(usize, &str, f32, f32, f32)] = &[
    (0, "One left", 72.125, 112.018, 72.125),
    (0, "Two right", 227.632, 271.875, 84.125),
    (0, "Three left", 72.125, 120.26, 96.125),
    (0, "Four right", 226.279, 271.875, 108.125),
    (0, "Five left", 72.125, 112.213, 120.125),
    (1, "One left", 322.25, 362.143, 72.25),
    (1, "Two right", 477.507, 521.75, 84.25),
    (1, "Three left", 322.25, 370.385, 96.25),
    (1, "Four right", 476.154, 521.75, 108.25),
    (1, "Five left", 322.25, 362.338, 120.25),
    (2, "One left", 72.0, 111.893, 168.0),
    (2, "Two right", 227.757, 272.0, 180.0),
    (2, "Three left", 72.0, 120.135, 192.0),
    (2, "Four right", 226.404, 272.0, 204.0),
    (2, "Five left", 72.0, 112.088, 216.0),
    (3, "One left", 322.5, 362.393, 168.5),
    (3, "Two right", 477.257, 521.5, 180.5),
    (3, "Three left", 322.5, 370.635, 192.5),
    (3, "Four right", 475.904, 521.5, 204.5),
    (3, "Five left", 322.5, 362.588, 216.5),
    (4, "One left", 73.0, 112.893, 265.0),
    (4, "Two right", 226.757, 271.0, 277.0),
    (4, "Three left", 73.0, 121.135, 289.0),
    (4, "Four right", 225.404, 271.0, 301.0),
    (4, "Five left", 73.0, 113.088, 313.0),
    (5, "One left", 322.0, 361.893, 264.0),
    (5, "Two right", 477.757, 522.0, 276.0),
    (5, "Three left", 322.0, 370.135, 288.0),
    (5, "Four right", 476.404, 522.0, 300.0),
    (5, "Five left", 322.0, 362.088, 312.0),
    (6, "One left", 75.0, 114.893, 363.0),
    (6, "Two right", 224.757, 269.0, 375.0),
    (6, "Three left", 75.0, 123.135, 387.0),
    (6, "Four right", 223.404, 269.0, 399.0),
    (6, "Five left", 75.0, 115.088, 411.0),
    (7, "One left", 328.0, 367.893, 366.0),
    (7, "Two right", 471.757, 516.0, 378.0),
    (7, "Three left", 328.0, 376.135, 390.0),
    (7, "Four right", 470.404, 516.0, 402.0),
    (8, "One left", 72.0, 111.893, 456.0),
    (8, "Two right", 227.757, 272.0, 468.0),
    (8, "Three left", 72.0, 120.135, 480.0),
    (8, "Four right", 226.404, 272.0, 492.0),
    (8, "Five left", 72.0, 112.088, 504.0),
    (9, "One left", 328.0, 367.893, 462.0),
    (9, "Two right", 471.757, 516.0, 474.0),
    (9, "Three left", 328.0, 376.135, 486.0),
    (9, "Four right", 470.404, 516.0, 498.0),
    (10, "One left", 84.0, 123.893, 564.0),
    (10, "Two right", 215.757, 260.0, 576.0),
    (10, "Three left", 84.0, 132.135, 588.0),
    (11, "One left", 322.0, 361.893, 552.0),
    (11, "Two right", 477.757, 522.0, 564.0),
    (11, "Three left", 322.0, 370.135, 576.0),
    (11, "Four right", 476.404, 522.0, 588.0),
    (11, "Five left", 322.0, 362.088, 600.0),
    (12, "One left", 72.0, 111.893, 648.0),
    (12, "Two right", 227.757, 272.0, 660.0),
    (12, "Three left", 72.0, 120.135, 672.0),
    (12, "Four right", 226.404, 272.0, 684.0),
    (12, "Five left", 72.0, 112.088, 696.0),
    (13, "One left", 322.0, 361.893, 648.0),
    (13, "Two right", 477.757, 522.0, 660.0),
    (13, "Three left", 322.0, 370.135, 672.0),
    (13, "Four right", 476.404, 522.0, 684.0),
    (13, "Five left", 322.0, 362.088, 696.0),
    (14, "One left", 72.0, 111.893, 744.0),
    (14, "Two right", 227.757, 272.0, 756.0),
    (14, "Three left", 72.0, 120.135, 768.0),
    (14, "Four right", 226.404, 272.0, 780.0),
    (14, "Five left", 72.0, 112.088, 792.0),
    (15, "One left", 329.0, 368.893, 751.0),
    (15, "Two right", 470.757, 515.0, 763.0),
    (15, "Three left", 329.0, 377.135, 775.0),
    (15, "Four right", 469.404, 515.0, 787.0),
    (16, "One left", 82.0, 121.893, 82.0),
    (16, "Two right", 217.757, 262.0, 94.0),
    (16, "Three left", 82.0, 130.135, 106.0),
    (17, "One left", 326.0, 365.893, 76.0),
    (17, "Two right", 473.757, 518.0, 88.0),
    (17, "Three left", 326.0, 374.135, 100.0),
    (17, "Four right", 472.404, 518.0, 112.0),
    (18, "One left", 80.5, 120.393, 170.5),
    (18, "Two right", 217.257, 261.5, 182.5),
    (18, "Three left", 80.5, 128.635, 194.5),
    (18, "Four right", 215.904, 261.5, 206.5),
    (19, "One left", 322.0, 361.893, 168.0),
    (19, "Two right", 477.757, 522.0, 180.0),
    (19, "Three left", 322.0, 370.135, 192.0),
    (19, "Four right", 476.404, 522.0, 204.0),
    (19, "Five left", 322.0, 362.088, 216.0),
    (20, "One left", 72.125, 112.018, 264.125),
    (20, "Two right", 227.632, 271.875, 276.125),
    (20, "Three left", 72.125, 120.26, 288.125),
    (20, "Four right", 226.279, 271.875, 300.125),
    (20, "Five left", 72.125, 112.213, 312.125),
    (21, "One left", 322.5, 362.393, 264.5),
    (21, "Two right", 477.257, 521.5, 276.5),
    (21, "Three left", 322.5, 370.635, 288.5),
    (21, "Four right", 475.904, 521.5, 300.5),
    (22, "One left", 72.25, 112.143, 360.25),
    (22, "Two right", 227.507, 271.75, 372.25),
    (22, "Three left", 72.25, 120.385, 384.25),
    (22, "Four right", 226.154, 271.75, 396.25),
    (23, "One left", 325.0, 364.893, 363.0),
    (23, "Two right", 367.757, 412.0, 375.0),
    (23, "Three left", 325.0, 373.135, 387.0),
    (23, "Four right", 473.404, 519.0, 363.0),
    (23, "Five left", 432.0, 472.088, 375.0),
    (23, "Six right", 480.289, 519.0, 387.0),
    (24, "One left", 78.0, 117.893, 462.0),
    (24, "Two right", 117.757, 162.0, 474.0),
    (24, "Three left", 182.0, 230.135, 462.0),
    (24, "Four right", 220.404, 266.0, 474.0),
    (25, "One left", 334.0, 373.893, 459.0),
    (25, "Two right", 474.757, 519.0, 471.0),
    (25, "Three left", 325.0, 373.135, 483.0),
    (25, "Four right", 473.404, 519.0, 495.0),
    (25, "Five left", 331.0, 371.088, 507.0),
    (26, "One left", 84.0, 123.893, 552.0),
    (26, "Two right", 227.757, 272.0, 564.0),
    (26, "Three left", 72.0, 120.135, 576.0),
    (26, "Four right", 226.404, 272.0, 588.0),
    (26, "Five left", 73.0, 113.088, 600.0),
    (27, "One left", 353.0, 392.893, 555.0),
    (27, "Two right", 474.757, 519.0, 567.0),
    (27, "Three left", 329.0, 377.135, 579.0),
    (27, "Four right", 473.404, 519.0, 591.0),
    (27, "Five left", 325.0, 365.088, 603.0),
    (28, "One left", 102.0, 141.893, 648.0),
    (28, "Two right", 227.757, 272.0, 660.0),
    (28, "Three left", 78.0, 126.135, 672.0),
    (28, "Four right", 226.404, 272.0, 684.0),
    (28, "Five left", 72.0, 112.088, 696.0),
    (29, "One left", 354.0, 393.893, 655.0),
    (29, "Two right", 470.757, 515.0, 667.0),
    (29, "Three left", 330.0, 378.135, 679.0),
    (29, "Four right", 469.404, 515.0, 691.0),
    (30, "One left", 103.0, 142.893, 748.0),
    (30, "Two right", 223.757, 268.0, 760.0),
    (30, "Three left", 79.0, 127.135, 772.0),
    (30, "Four right", 222.404, 268.0, 784.0),
];

/// Cases the engine cannot match yet, with the reason. Everything else
/// must land within [`TOLERANCE`].
///
/// A text frame's `StrokeAlignment` does not reach the model: the IDML
/// importer (plugin-publish `idml-import`) reads it on rectangles, ovals
/// and polygons but not on text frames, so these frames are painted AND
/// inset as centred. Real documents do not hit it — 0 of the 13,727 text
/// frames in the corpus packs spell `StrokeAlignment`.
const KNOWN: &[(usize, &str)] = &[
    (1, "inside alignment not in the model"),
    (2, "outside alignment not in the model"),
    (4, "inside alignment not in the model"),
    (5, "outside alignment not in the model"),
    (7, "inside alignment not in the model"),
    (8, "outside alignment not in the model"),
    (10, "inside alignment not in the model"),
    (11, "outside alignment not in the model"),
    (16, "inside alignment not in the model"),
    (17, "outside alignment not in the model"),
    (22, "inside alignment not in the model"),
    (24, "inside alignment not in the model"),
    // The engine composes a rounded-corner text frame as its rectangle
    // (the corner effect is painted, not laid out): line 1 and line 5 sit
    // in the corners. The same residue with no stroke at all (case 26).
    (25, "text does not follow rounded corners"),
    (26, "text does not follow rounded corners"),
    // A shaped frame: the first line of a later paragraph is measured
    // from its own ascent where InDesign measures from the previous
    // baseline, and a band edge on a whole point is dropped on the right.
    // Both appear unchanged without a stroke (28, 30). And a shaped
    // frame is not inset by its stroke yet (`text_area_frame`); what
    // InDesign does there is pinned by `a_stroke_erodes_a_shaped_frame`.
    (27, "shaped-frame slug and right-edge residue"),
    (28, "shaped-frame slug and right-edge residue"),
    (29, "shaped-frame slug and right-edge residue"),
    (30, "shaped-frame slug and right-edge residue"),
];

const TOLERANCE: f32 = 0.05;

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

#[test]
fn every_line_lands_where_indesign_puts_it() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::stroke_inset::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");

    let mut report = Vec::new();
    let mut failures = 0;
    let mut checked = 0;
    for (i, case) in cases().iter().enumerate() {
        let known = KNOWN.iter().find(|k| k.0 == i).map(|k| k.1);
        let want: Vec<_> = INDESIGN.iter().filter(|r| r.0 == i).collect();
        let lines = built.story_layout(&body_story_id(i as u32));
        report.push(format!(
            "── {i:2} {} ({} lines, InDesign {}){}",
            case.name,
            lines.len(),
            want.len(),
            known.map(|r| format!("  [known: {r}]")).unwrap_or_default()
        ));
        let mut bad = lines.len() != want.len();
        for (k, w) in want.iter().enumerate() {
            let Some(line) = lines.get(k) else {
                report.push(format!("   {:12} missing", w.1));
                continue;
            };
            let left = line
                .clusters
                .iter()
                .map(|c| c.x_pt)
                .fold(f32::INFINITY, f32::min);
            let right = line
                .clusters
                .iter()
                .map(|c| c.x_pt + c.advance_pt)
                .fold(f32::NEG_INFINITY, f32::max);
            // A left-aligned line is read by where it starts, a
            // right-aligned one by where it ends: that is the edge the
            // text area puts it against.
            let (x, want_x) = if is_right(line.paragraph_idx as usize) {
                (right, w.3)
            } else {
                (left, w.2)
            };
            let y = line.baseline_y_pt;
            let ok = (x - want_x).abs() <= TOLERANCE && (y - w.4).abs() <= TOLERANCE;
            bad |= !ok;
            report.push(format!(
                "   {} {:12} engine x {x:8.3} y {y:8.3}   indesign x {want_x:8.3} y {:8.3}",
                if ok { " " } else { "✗" },
                w.1,
                w.4
            ));
        }
        match (bad, known) {
            (true, None) => failures += 1,
            (false, Some(_)) => {
                report.push("   ^ matches now: drop it from KNOWN".to_string());
                failures += 1;
            }
            _ => {}
        }
        if known.is_none() {
            checked += 1;
        }
    }
    if std::env::var_os("STROKE_INSET_REPORT").is_some() {
        eprintln!(
            "{}",
            report.join(
                "
"
            )
        );
    }
    assert!(checked >= 13, "only {checked} cases checked");
    assert_eq!(
        failures,
        0,
        "
{}",
        report.join(
            "
"
        )
    );
}

/// What a stroke does to a SHAPED frame, independent of the shaped-frame
/// residues above: a chamfered frame with a 6 pt centre stroke against
/// the same frame unstroked, with no inset (cases 27 / 28) and with a
/// 4 pt inset (29 / 30). InDesign erodes the outline by the stroke's
/// share exactly as by an inset, and on TOP of one: the first baseline
/// drops 3 pt either way, the chamfer's first line moves right by 1 pt
/// (3 pt across a 45° edge, then the whole-point floor), and every
/// line's change from the unstroked frame must be the engine's too.
///
/// Read on every baseline and on the left edge where it is the frame's
/// straight side. The right edge is the residue named in `KNOWN`:
/// InDesign keeps a band edge that lands exactly on a whole point (200 →
/// 272 here), the engine drops it (`ceil - 1`), and an eroded straight
/// edge comes back a hair either side of 197, so its delta is -2 or -3
/// by rounding alone.
#[test]
#[ignore = "shaped frames are not inset by their stroke yet: folding it moves text-in-shape's \
            donut onto InDesign's bands, where the composer breaks the paragraph shorter than \
            InDesign does (see text_area_frame)"]
fn a_stroke_erodes_a_shaped_frame() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::stroke_inset::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    let origin = |i: usize| paged_gen::samples::stroke_inset::frame_origin(i);
    let edges = |i: usize| -> Vec<(f32, f32)> {
        let (ox, oy) = origin(i);
        built
            .story_layout(&body_story_id(i as u32))
            .iter()
            .map(|line| {
                let x = if is_right(line.paragraph_idx as usize) {
                    line.clusters
                        .iter()
                        .map(|c| c.x_pt + c.advance_pt)
                        .fold(f32::NEG_INFINITY, f32::max)
                } else {
                    line.clusters
                        .iter()
                        .map(|c| c.x_pt)
                        .fold(f32::INFINITY, f32::min)
                };
                (x - ox, line.baseline_y_pt - oy)
            })
            .collect()
    };
    let indesign = |i: usize| -> Vec<(f32, f32)> {
        let (ox, oy) = origin(i);
        INDESIGN
            .iter()
            .filter(|r| r.0 == i)
            .enumerate()
            .map(|(k, r)| (if is_right(k) { r.3 } else { r.2 } - ox, r.4 - oy))
            .collect()
    };
    for (stroked, plain, inset) in [(27usize, 28usize, 0.0_f32), (29, 30, 4.0)] {
        let (es, ep, is, ip) = (
            edges(stroked),
            edges(plain),
            indesign(stroked),
            indesign(plain),
        );
        assert_eq!(es.len(), is.len(), "case {stroked}: line count");
        assert_eq!(ep.len(), ip.len(), "case {plain}: line count");
        for k in 0..is.len() {
            let dy = (es[k].1 - ep[k].1, is[k].1 - ip[k].1);
            assert!(
                (dy.0 - dy.1).abs() <= TOLERANCE,
                "case {stroked} line {k}: the stroke moved the baseline {} in the engine, {} in InDesign",
                dy.0,
                dy.1
            );
            // The left edge where it is the frame's straight side: a line
            // under the chamfer is floored to whole points from a slug the
            // engine measures from elsewhere (the residue above), so its
            // change is rounding, not the stroke.
            if is_right(k) || ip[k].0 > inset + 0.5 {
                continue;
            }
            let dx = (es[k].0 - ep[k].0, is[k].0 - ip[k].0);
            assert!(
                (dx.0 - dx.1).abs() <= TOLERANCE,
                "case {stroked} line {k}: the stroke moved the left edge {} in the engine, {} in InDesign",
                dx.0,
                dx.1
            );
        }
    }
}
