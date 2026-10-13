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

//! Paragraph `Justification` over the generated `justification.idml`,
//! against where InDesign 2025 set each line in its PDF export of the
//! same file (`corpus/generated/justification.pdf`, 2026-10-12, measured
//! with `pdftotext -bbox-layout`).
//!
//! Every case frame is 126 pt wide, unstroked, with zero insets: the
//! measure is the frame.

use paged_gen::samples::justification::{body_story_id, frame_origin, FRAME_W};
use paged_renderer::{pipeline, PipelineOptions};

/// The engine sums glyph advances in 1/64 pt, so a 27-glyph line's
/// natural width drifts up to ~0.06 pt from InDesign's.
const TOLERANCE: f32 = 0.1;

/// Rows of the fixture's matrix.
const CENTER_JUSTIFIED: usize = 4;
const RIGHT_JUSTIFIED: usize = 5;
const FULLY_JUSTIFIED: usize = 6;
const TO_BINDING: usize = 7;
const AWAY_FROM_BINDING: usize = 8;
/// Columns: the last-line shapes.
const SHORT_LAST_LINE: usize = 0;
const ONE_WORD_LAST_LINE: usize = 1;
const FORCED_BREAK: usize = 2;
const SINGLE_WORD: usize = 3;
/// Pages: 0 a recto alone in its spread, 1 the verso and 2 the recto
/// of the reader spread.
const VERSO: usize = 1;
const RECTOS: [usize; 2] = [0, 2];

fn built() -> pipeline::BuiltDocument {
    let bytes = paged_gen::write_idml(&paged_gen::samples::justification::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus/fonts/OpenSans.ttf"),
    )
    .expect("read OpenSans.ttf");
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    pipeline::build_document(&doc, &opts).expect("build")
}

/// Each line of a case as (gap to the measure's left edge, gap to its
/// right edge), pt — the shape `pdftotext` gives InDesign's lines.
fn gaps(
    built: &pipeline::BuiltDocument,
    page: usize,
    row: usize,
    column: usize,
) -> Vec<(f32, f32)> {
    let (x, _) = frame_origin(row, column);
    let (left, right) = (x, x + FRAME_W);
    built
        .story_layout(&body_story_id(page, row, column))
        .iter()
        .filter(|l| !l.clusters.is_empty())
        .map(|l| {
            let first = l.clusters.iter().map(|c| c.x_pt).fold(f32::MAX, f32::min);
            let last = l
                .clusters
                .iter()
                .map(|c| c.x_pt + c.advance_pt)
                .fold(f32::MIN, f32::max);
            (first - left, right - last)
        })
        .collect()
}

fn close(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() <= TOLERANCE && (a.1 - b.1).abs() <= TOLERANCE
}

/// CenterJustified (`justification` 1.17): the body lines reach both
/// edges, the last line ("at dawn") is centred, 46.23 / 46.22 pt; a
/// one-word paragraph (1.20) is centred too, 38.26 / 38.25 pt.
#[test]
fn center_justified_last_line_centred() {
    let built = built();
    let lines = gaps(&built, 0, CENTER_JUSTIFIED, SHORT_LAST_LINE);
    let (last, body) = lines.split_last().expect("lines");
    assert_eq!(body.len(), 3, "{lines:?}");
    for l in body {
        assert!(
            close(*l, (0.0, 0.0)),
            "a body line fills the measure: {l:?}"
        );
    }
    assert!(
        close(*last, (46.23, 46.22)),
        "the last line is centred: {last:?}"
    );
    let word = gaps(&built, 0, CENTER_JUSTIFIED, SINGLE_WORD);
    assert!(close(word[0], (38.26, 38.25)), "{word:?}");
}

/// RightJustified (1.21 / 1.23): the last line sits flush right, 92.45
/// pt in; the line a forced break ends fills the measure.
#[test]
fn right_justified_last_line_flush_right() {
    let built = built();
    let lines = gaps(&built, 0, RIGHT_JUSTIFIED, SHORT_LAST_LINE);
    assert!(close(*lines.last().unwrap(), (92.45, 0.0)), "{lines:?}");
    let broken = gaps(&built, 0, RIGHT_JUSTIFIED, FORCED_BREAK);
    assert!(close(broken[0], (0.0, 0.0)), "{broken:?}");
    assert!(close(*broken.last().unwrap(), (54.76, 0.0)), "{broken:?}");
}

/// FullyJustified: every line reaches both edges — the last one too
/// (1.25), and a one-word last line (1.26, "extraordinarily") or
/// paragraph (1.28, "Typography") by its letters.
///
/// InDesign also BREAKS such a paragraph differently: it composes the
/// last line into the fit ("Quick zephyrs blow," / "vexing daft Jim,
/// and the" / "five boxing wizards jump" / "over the fence at dawn"),
/// where the engine breaks it like LeftJustified. Only where each line
/// ends is asserted here.
#[test]
fn fully_justified_last_line_spread() {
    let built = built();
    for column in [
        SHORT_LAST_LINE,
        ONE_WORD_LAST_LINE,
        FORCED_BREAK,
        SINGLE_WORD,
    ] {
        let lines = gaps(&built, 0, FULLY_JUSTIFIED, column);
        for l in &lines {
            assert!(close(*l, (0.0, 0.0)), "column {column}: {lines:?}");
        }
    }
    // The letters of the one-word paragraph spread evenly: InDesign set
    // them 8.50 pt apart (76.52 pt of slack over 9 gaps).
    let story = body_story_id(0, FULLY_JUSTIFIED, SINGLE_WORD);
    let layout = built.story_layout(&story);
    let xs: Vec<f32> = layout[0].clusters.iter().map(|c| c.x_pt).collect();
    let advances: Vec<f32> = layout[0].clusters.iter().map(|c| c.advance_pt).collect();
    assert_eq!(xs.len(), 10, "Typography");
    for i in 1..xs.len() {
        let gap = xs[i] - (xs[i - 1] + advances[i - 1]);
        assert!((gap - 8.50).abs() <= TOLERANCE, "gap {i}: {gap}");
    }
}

/// AwayFromBindingSide is flush LEFT on the verso (2.33: every line
/// 0.00 pt from the left edge, "at dawn" 92.45 pt short of the right),
/// flush right on a recto; ToBindingSide the opposite.
#[test]
fn away_from_binding_on_verso_is_left() {
    let built = built();
    let ragged_left = [(0.0, 12.95), (0.0, 9.25), (0.0, 7.68), (0.0, 92.45)];
    let ragged_right = [(12.95, 0.0), (9.25, 0.0), (7.69, 0.0), (92.45, 0.0)];
    let check = |page: usize, row: usize, want: &[(f32, f32)]| {
        let got = gaps(&built, page, row, SHORT_LAST_LINE);
        assert!(
            got.len() == want.len() && got.iter().zip(want).all(|(g, w)| close(*g, *w)),
            "page {page} row {row}\n  engine   {got:?}\n  indesign {want:?}"
        );
    };
    check(VERSO, AWAY_FROM_BINDING, &ragged_left);
    check(VERSO, TO_BINDING, &ragged_right);
    for page in RECTOS {
        check(page, AWAY_FROM_BINDING, &ragged_right);
        check(page, TO_BINDING, &ragged_left);
    }
}
