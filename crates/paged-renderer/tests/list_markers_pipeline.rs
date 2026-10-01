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

//! Where the text after a list marker starts, over the generated
//! `list-markers.idml`, against InDesign 20.0.1's PDF export of the same
//! file (`corpus/generated/list-markers.pdf`, 2026-10-01).
//!
//! The rule InDesign follows (and `paragraph_tab_stops` implements):
//! an undeclared `BulletsTextAfter` is a tab; a tab goes to the first
//! stop past the pen, where the stops are the `<TabList>` plus, on a
//! hanging first line, the left indent; past all of them it goes to the
//! next 36 pt default stop, counted from the frame's edge.

use paged_gen::samples::list_markers::{body_story_id, cases, frame_origin};
use paged_renderer::{pipeline, PipelineOptions};

/// Frame-local pen x of (the case's tag, the word after it) in each of
/// its two paragraphs, read off `pdftotext -bbox`: word xMin minus the
/// 0.125 pt every word of this export sits right of its pen (the markers
/// at x=0 come back at 0.125, the frame-edge words at 72.125). `None` where the tag is not a word of its own:
/// `^#.` puts no gap after the number, so InDesign's word is `1.c09`.
const INDESIGN: [[(Option<f32>, f32); 2]; 14] = [
    // c00 bullet, BulletsTextAfter undeclared, hanging 18 → a tab, to 18.
    [(Some(18.0), 39.14), (Some(18.0), 39.14)],
    // c01 ^t, hanging 18.
    [(Some(18.0), 36.9), (Some(18.0), 36.9)],
    // c02 a space: the bullet's advance plus one space.
    [(Some(8.437), 29.368), (Some(8.437), 29.368)],
    // c03 ^t, no indent: the first default stop.
    [(Some(36.0), 57.01), (Some(36.0), 57.01)],
    // c04 ^t, left indent 18 with no first-line indent: bullet at 18,
    // text at the default stop 36 (frame-relative, not indent + 36).
    [(Some(36.0), 57.29), (Some(36.0), 57.29)],
    // c05 ^t, left 50 first -20: bullet at 30, text at the indent.
    [(Some(50.0), 70.76), (Some(50.0), 70.76)],
    // c06 ^t, hanging 50, explicit stop 30: the stop before the indent wins.
    [(Some(30.0), 51.03), (Some(30.0), 51.03)],
    // c07 ^t, hanging 30, explicit stop 60: the indent comes first.
    [(Some(30.0), 50.293), (Some(30.0), 50.293)],
    // c08 ^#.^t, hanging 18.
    [(Some(18.0), 39.02), (Some(18.0), 39.02)],
    // c09 ^#. — no tab; "1." / "2." differ in width.
    [(None, 27.978), (None, 30.01)],
    // c10 ^#.^t, hanging 50, explicit stop 10: the stop wins.
    [(Some(10.0), 28.566), (Some(10.0), 28.566)],
    // c11 ^#.^t, hanging 6, number wider than the indent: default 36.
    [(Some(36.0), 52.326), (Some(36.0), 52.326)],
    // c12 no list, "Tab\t", hanging 40: a plain tab stops at the indent too.
    [(Some(40.0), 58.356), (Some(40.0), 58.356)],
    // c13 ^t, hanging 18, explicit stop 2 under the bullet: skipped, 18.
    [(Some(18.0), 36.436), (Some(18.0), 36.436)],
];

const X_TOLERANCE: f32 = 0.5;

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

#[test]
fn text_after_a_list_marker_starts_where_indesign_puts_it() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::list_markers::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");

    let mut report = Vec::new();
    let mut ok = true;
    for (i, case) in cases().iter().enumerate() {
        let (fx, _) = frame_origin(i as u32);
        let lines = built.story_layout(&body_story_id(i as u32));
        assert_eq!(lines.len(), 2, "{}: one line per paragraph", case.name);
        for (line, want) in lines.iter().zip(INDESIGN[i]) {
            // Each paragraph is "<marker><tag> one|two" on one line, so
            // the tag and the word after it sit at fixed distances from
            // the line's end, whatever the marker's length.
            let end = line.byte_range.end;
            let x_at = |byte: u32| {
                line.clusters
                    .iter()
                    .find(|c| c.byte == byte)
                    .map(|c| c.x_pt - fx)
                    .unwrap_or(f32::NAN)
            };
            let tag = x_at(end - 7);
            let word = x_at(end - 3);
            let same = (word - want.1).abs() <= X_TOLERANCE
                && want.0.is_none_or(|t| (tag - t).abs() <= X_TOLERANCE);
            ok &= same;
            report.push(format!(
                "{:40} {:8} engine ({tag:.3}, {word:.3})  indesign ({:?}, {:.3})",
                case.name,
                if same { "ok" } else { "DIFFERS" },
                want.0,
                want.1
            ));
        }
    }
    assert!(ok, "\n{}", report.join("\n"));
}
