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
//! `list-markers.idml` and `list-overrides.idml`, against InDesign
//! 20.0.1's PDF export of the same files (`corpus/generated/*.pdf`,
//! 2026-10-01).
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

/// `list-overrides`: per case and paragraph, (the tag's pen x, the word's
/// pen x, the marker's width), frame-local, read off InDesign's export the
/// same way as [`INDESIGN`]. The marker width is the marker word's
/// `xMax - xMin` — it tells "1." from "5." from "I." (Inter's digits are
/// proportional), which is how the counter cases are checked. `None`
/// where the marker glues to the tag (no separator) or the word could not
/// be told apart.
#[allow(clippy::type_complexity)]
const INDESIGN_OVERRIDES: [[(Option<f32>, f32, Option<f32>); 2]; 22] = [
    [
        (Some(18.000), 39.430, Some(5.630)),
        (Some(18.000), 39.430, Some(5.630)),
    ],
    [
        (Some(8.438), 27.627, Some(5.630)),
        (Some(8.438), 27.627, Some(5.630)),
    ],
    [(None, 28.168, None), (None, 30.200, None)],
    [
        (Some(24.000), 45.300, Some(11.370)),
        (Some(24.000), 45.300, Some(13.400)),
    ],
    [
        (Some(18.000), 39.580, Some(5.630)),
        (Some(18.000), 39.580, Some(5.630)),
    ],
    [
        (Some(8.438), 29.488, Some(5.630)),
        (Some(8.438), 29.488, Some(5.630)),
    ],
    [
        (Some(18.000), 39.320, Some(6.950)),
        (Some(18.000), 39.320, Some(8.980)),
    ],
    [(None, 31.945, None), (None, 33.977, None)],
    [(None, 33.551, None), (None, 33.551, None)],
    [
        (Some(18.000), 39.320, Some(8.540)),
        (Some(18.000), 39.320, Some(8.740)),
    ],
    [
        (Some(18.000), 36.849, Some(8.540)),
        (Some(18.000), 36.849, Some(8.740)),
    ],
    [
        (Some(18.000), 34.609, Some(6.950)),
        (Some(18.000), 34.609, Some(6.950)),
    ],
    [
        (Some(18.000), 36.639, Some(5.565)),
        (Some(18.000), 36.639, Some(8.255)),
    ],
    [
        (Some(18.000), 36.719, Some(6.950)),
        (Some(18.000), 36.719, Some(8.980)),
    ],
    [
        (Some(16.875), 35.874, None),
        (Some(16.875), 35.874, Some(11.260)),
    ],
    [
        (Some(8.438), 26.907, Some(5.630)),
        (Some(8.438), 26.907, Some(5.630)),
    ],
    [
        (Some(13.897), 32.636, None),
        (Some(17.959), 36.698, Some(17.960)),
    ],
    [(None, 25.147, None), (None, 27.178, None)],
    [
        (Some(18.000), 36.729, Some(8.540)),
        (Some(18.000), 36.729, Some(8.740)),
    ],
    [
        (Some(18.000), 36.739, Some(6.950)),
        (Some(18.000), 36.739, Some(8.980)),
    ],
    [
        (Some(18.000), 39.220, Some(8.540)),
        (Some(18.000), 39.220, Some(8.740)),
    ],
    [
        (Some(18.000), 36.980, Some(6.950)),
        (Some(18.000), 36.980, Some(8.980)),
    ],
];

/// Cases InDesign renders with the marker's character style at ITS size
/// (20 pt): the engine applies a marker character style's colour only,
/// not its size or face, so these two still differ.
const MARKER_SIZE_NOT_MODELLED: [&str; 2] = ["o14", "o16"];

/// Marker widths agree to well under the 0.2 pt between "2." and "6.".
const W_TOLERANCE: f32 = 0.1;

#[test]
fn local_list_overrides_render_where_indesign_puts_them() {
    use paged_gen::samples::list_overrides as lo;
    let bytes = paged_gen::write_idml(&lo::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");

    let mut report = Vec::new();
    let mut ok = true;
    for (i, case) in lo::cases().iter().enumerate() {
        let (fx, _) = lo::frame_origin(i as u32);
        let lines = built.story_layout(&lo::body_story_id(i as u32));
        assert_eq!(lines.len(), 2, "{}: one line per paragraph", case.name);
        let gap = MARKER_SIZE_NOT_MODELLED.contains(&case.tag);
        for (line, want) in lines.iter().zip(INDESIGN_OVERRIDES[i]) {
            let (start, end) = (line.byte_range.start, line.byte_range.end);
            let x_at = |byte: u32| {
                line.clusters
                    .iter()
                    .find(|c| c.byte == byte)
                    .map(|c| c.x_pt - fx)
                    .unwrap_or(f32::NAN)
            };
            let tag = x_at(end - 7);
            let word = x_at(end - 3);
            // The marker is everything before the tag; its last byte is
            // the separator (a tab or a space) whenever it is a word of
            // its own.
            let marker_w = x_at(end - 8) - x_at(start);
            let same = (word - want.1).abs() <= X_TOLERANCE
                && want.0.is_none_or(|t| (tag - t).abs() <= X_TOLERANCE)
                && want.2.is_none_or(|w| (marker_w - w).abs() <= W_TOLERANCE);
            if !gap {
                ok &= same;
            }
            report.push(format!(
                "{} {:34} {:12} engine ({tag:.3}, {word:.3}, w {marker_w:.3})  indesign ({:?}, {:.3}, w {:?})",
                case.tag,
                case.name,
                match (same, gap) {
                    (true, _) => "ok",
                    (false, true) => "known gap",
                    (false, false) => "DIFFERS",
                },
                want.0,
                want.1,
                want.2
            ));
        }
    }
    assert!(ok, "\n{}", report.join("\n"));
    eprintln!("{}", report.join("\n"));
}
