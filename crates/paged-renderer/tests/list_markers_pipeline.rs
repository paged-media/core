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

//! Where the text after a list marker starts, and how the marker itself
//! is set, over the generated `list-markers.idml`, `list-overrides.idml`
//! and `list-marker-styles.idml`, against InDesign
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
/// its two paragraphs, read off `pdftotext -bbox` (word xMin minus the
/// frame's left edge). The body frames carry a 0.25 pt centre stroke,
/// which insets the text area by 0.125 pt (`stroke-inset`), so every
/// position is 0.125 right of where the tab rule alone puts it: a marker
/// at the text area's left edge comes back at 0.125. `None` where the
/// tag is not a word of its own: `^#.` puts no gap after the number, so
/// InDesign's word is `1.c09`.
const INDESIGN: [[(Option<f32>, f32); 2]; 14] = [
    // c00 bullet, BulletsTextAfter undeclared, hanging 18 → a tab, to 18.
    [(Some(18.125), 39.265), (Some(18.125), 39.265)],
    // c01 ^t, hanging 18.
    [(Some(18.125), 37.025), (Some(18.125), 37.025)],
    // c02 a space: the bullet's advance plus one space.
    [(Some(8.562), 29.493), (Some(8.562), 29.493)],
    // c03 ^t, no indent: the first default stop.
    [(Some(36.125), 57.135), (Some(36.125), 57.135)],
    // c04 ^t, left indent 18 with no first-line indent: bullet at 18,
    // text at the default stop 36 (frame-relative, not indent + 36).
    [(Some(36.125), 57.415), (Some(36.125), 57.415)],
    // c05 ^t, left 50 first -20: bullet at 30, text at the indent.
    [(Some(50.125), 70.885), (Some(50.125), 70.885)],
    // c06 ^t, hanging 50, explicit stop 30: the stop before the indent wins.
    [(Some(30.125), 51.155), (Some(30.125), 51.155)],
    // c07 ^t, hanging 30, explicit stop 60: the indent comes first.
    [(Some(30.125), 50.418), (Some(30.125), 50.418)],
    // c08 ^#.^t, hanging 18.
    [(Some(18.125), 39.145), (Some(18.125), 39.145)],
    // c09 ^#. — no tab; "1." / "2." differ in width.
    [(None, 28.103), (None, 30.135)],
    // c10 ^#.^t, hanging 50, explicit stop 10: the stop wins.
    [(Some(10.125), 28.691), (Some(10.125), 28.691)],
    // c11 ^#.^t, hanging 6, number wider than the indent: default 36.
    [(Some(36.125), 52.451), (Some(36.125), 52.451)],
    // c12 no list, "Tab\t", hanging 40: a plain tab stops at the indent too.
    [(Some(40.125), 58.481), (Some(40.125), 58.481)],
    // c13 ^t, hanging 18, explicit stop 2 under the bullet: skipped, 18.
    [(Some(18.125), 36.561), (Some(18.125), 36.561)],
];

const X_TOLERANCE: f32 = 0.05;

/// Where the `list-overrides` and `list-marker-styles` tables are
/// measured from: their frames' text area, which half the frames' 0.25 pt
/// centre stroke insets on every side (`stroke-inset`).
const TEXT_AREA_INSET: f32 = 0.125;

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
/// pen x, the marker's width), measured from the frame's text area
/// ([`TEXT_AREA_INSET`] inside its edge), read off InDesign's export the
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
        let fx = fx + TEXT_AREA_INSET;
        let lines = built.story_layout(&lo::body_story_id(i as u32));
        assert_eq!(lines.len(), 2, "{}: one line per paragraph", case.name);
        // A marker that ends in neither a tab nor a space glues to the
        // tag; InDesign's word is then the whole marker where the font
        // size splits it off (o16, "2." at 20 pt), so it is measured up
        // to the tag rather than to the separator.
        let glued = case
            .attrs
            .iter()
            .map(|&(k, v)| (k, v))
            .chain(case.props.iter().map(|&(k, _, v)| (k, v)))
            .any(|(k, v)| {
                matches!(k, "NumberingExpression" | "BulletsTextAfter")
                    && !v.ends_with("^t")
                    && !v.ends_with(' ')
            });
        for (line, want) in lines.iter().zip(INDESIGN_OVERRIDES[i]) {
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
            // The marker is everything before the tag; its last byte is
            // the separator (a tab or a space) whenever it is a word of
            // its own.
            // its own. The marker's glyphs are not characters of the
            // line; the line records where the marker starts.
            let marker_x = line.marker.first().map_or(f32::NAN, |c| c.x_pt - fx);
            let marker_w = if glued {
                x_at(end - 7) - marker_x
            } else {
                // The separator is the marker's last cluster.
                line.marker.last().map_or(f32::NAN, |c| c.x_pt - fx) - marker_x
            };
            let same = (word - want.1).abs() <= X_TOLERANCE
                && want.0.is_none_or(|t| (tag - t).abs() <= X_TOLERANCE)
                && want.2.is_none_or(|w| (marker_w - w).abs() <= W_TOLERANCE);
            ok &= same;
            report.push(format!(
                "{} {:34} {:8} engine ({tag:.3}, {word:.3}, w {marker_w:.3})  indesign ({:?}, {:.3}, w {:?})",
                case.tag,
                case.name,
                if same { "ok" } else { "DIFFERS" },
                want.0,
                want.1,
                want.2
            ));
        }
    }
    assert!(ok, "\n{}", report.join("\n"));
    eprintln!("{}", report.join("\n"));
}

/// One line of `list-marker-styles` as InDesign 20.0.1 exported it
/// (`corpus/generated/list-marker-styles.pdf`, 2026-10-01), frame-local
/// pt measured from the TEXT AREA's corner ([`TEXT_AREA_INSET`] inside
/// the frame's). `marker` lists the marker's visible glyphs as (char, pen x,
/// point size, baseline y); the separator is a space or a tab and has no
/// outline.
struct Line {
    baseline: f32,
    tag: f32,
    word: f32,
    red: bool,
    marker: &'static [(char, f32, f32, f32)],
}

/// The rule these pin: InDesign sets the WHOLE marker — number, literal
/// text and separator — in its character style laid over the first
/// character's formatting (size, family, bold, colour, baseline shift and
/// tracking each apply; a style that sets nothing changes nothing; the
/// style wins over the run's own local values, m20/m21), and the marker
/// never raises the line: a 20 pt marker on auto-leaded 10 pt text keeps
/// the 12 pt leading (m14/m15).
const INDESIGN_MARKER_STYLES: [[Line; 2]; 22] = [
    // m00
    [
        Line {
            baseline: 12.000,
            tag: 16.875,
            word: 41.065,
            red: false,
            marker: &[('•', 0.000, 20.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 16.875,
            word: 41.065,
            red: false,
            marker: &[('•', 0.000, 20.0, 24.000)],
        },
    ],
    // m01
    [
        Line {
            baseline: 12.000,
            tag: 19.522,
            word: 41.471,
            red: false,
            marker: &[('1', 0.000, 20.0, 12.000), ('.', 8.140, 20.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 23.584,
            word: 45.534,
            red: false,
            marker: &[('2', 0.000, 20.0, 24.000), ('.', 12.200, 20.0, 24.000)],
        },
    ],
    // m02
    [
        Line {
            baseline: 12.000,
            tag: 7.109,
            word: 31.089,
            red: false,
            marker: &[('•', 0.000, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 7.109,
            word: 31.089,
            red: false,
            marker: &[('•', 0.000, 10.0, 24.000)],
        },
    ],
    // m03
    [
        Line {
            baseline: 12.000,
            tag: 9.712,
            word: 33.772,
            red: false,
            marker: &[('1', 0.000, 10.0, 12.000), ('.', 4.312, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 11.699,
            word: 35.759,
            red: false,
            marker: &[('2', 0.000, 10.0, 24.000), ('.', 6.299, 10.0, 24.000)],
        },
    ],
    // m04
    [
        Line {
            baseline: 12.000,
            tag: 12.000,
            word: 36.340,
            red: false,
            marker: &[('•', 0.000, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 12.000,
            word: 36.340,
            red: false,
            marker: &[('•', 0.000, 10.0, 24.000)],
        },
    ],
    // m05
    [
        Line {
            baseline: 12.000,
            tag: 18.000,
            word: 41.810,
            red: false,
            marker: &[('1', 0.000, 10.0, 12.000), ('.', 6.000, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 18.000,
            word: 41.810,
            red: false,
            marker: &[('2', 0.000, 10.0, 24.000), ('.', 6.000, 10.0, 24.000)],
        },
    ],
    // m06
    [
        Line {
            baseline: 12.000,
            tag: 8.437,
            word: 32.518,
            red: true,
            marker: &[('•', 0.000, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 8.437,
            word: 32.518,
            red: true,
            marker: &[('•', 0.000, 10.0, 24.000)],
        },
    ],
    // m07
    [
        Line {
            baseline: 12.000,
            tag: 9.761,
            word: 33.104,
            red: true,
            marker: &[('1', 0.000, 10.0, 12.000), ('.', 4.070, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 11.792,
            word: 35.135,
            red: true,
            marker: &[('2', 0.000, 10.0, 24.000), ('.', 6.100, 10.0, 24.000)],
        },
    ],
    // m08
    [
        Line {
            baseline: 12.000,
            tag: 8.437,
            word: 32.507,
            red: false,
            marker: &[('•', 0.000, 10.0, 8.000)],
        },
        Line {
            baseline: 24.000,
            tag: 8.437,
            word: 32.507,
            red: false,
            marker: &[('•', 0.000, 10.0, 20.000)],
        },
    ],
    // m09
    [
        Line {
            baseline: 12.000,
            tag: 9.761,
            word: 33.841,
            red: false,
            marker: &[('1', 0.000, 10.0, 8.000), ('.', 4.070, 10.0, 8.000)],
        },
        Line {
            baseline: 24.000,
            tag: 11.792,
            word: 35.872,
            red: false,
            marker: &[('2', 0.000, 10.0, 20.000), ('.', 6.100, 10.0, 20.000)],
        },
    ],
    // m10
    [
        Line {
            baseline: 12.000,
            tag: 12.437,
            word: 33.992,
            red: false,
            marker: &[('•', 0.000, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 12.437,
            word: 33.992,
            red: false,
            marker: &[('•', 0.000, 10.0, 24.000)],
        },
    ],
    // m11
    [
        Line {
            baseline: 12.000,
            tag: 15.761,
            word: 35.075,
            red: false,
            marker: &[('1', 0.000, 10.0, 12.000), ('.', 6.070, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 17.792,
            word: 37.106,
            red: false,
            marker: &[('2', 0.000, 10.0, 24.000), ('.', 8.100, 10.0, 24.000)],
        },
    ],
    // m12
    [
        Line {
            baseline: 12.000,
            tag: 8.437,
            word: 29.781,
            red: false,
            marker: &[('•', 0.000, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 8.437,
            word: 29.781,
            red: false,
            marker: &[('•', 0.000, 10.0, 24.000)],
        },
    ],
    // m13
    [
        Line {
            baseline: 12.000,
            tag: 9.761,
            word: 31.185,
            red: false,
            marker: &[('1', 0.000, 10.0, 12.000), ('.', 4.070, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 11.792,
            word: 33.216,
            red: false,
            marker: &[('2', 0.000, 10.0, 24.000), ('.', 6.100, 10.0, 24.000)],
        },
    ],
    // m14
    [
        Line {
            baseline: 12.000,
            tag: 16.875,
            word: 38.579,
            red: false,
            marker: &[('•', 0.000, 20.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 16.875,
            word: 38.579,
            red: false,
            marker: &[('•', 0.000, 20.0, 24.000)],
        },
    ],
    // m15
    [
        Line {
            baseline: 12.000,
            tag: 19.522,
            word: 40.696,
            red: false,
            marker: &[('1', 0.000, 20.0, 12.000), ('.', 8.140, 20.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 23.584,
            word: 44.758,
            red: false,
            marker: &[('2', 0.000, 20.0, 24.000), ('.', 12.200, 20.0, 24.000)],
        },
    ],
    // m16
    [
        Line {
            baseline: 12.000,
            tag: 18.000,
            word: 39.444,
            red: false,
            marker: &[('•', 0.000, 20.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 18.000,
            word: 39.444,
            red: false,
            marker: &[('•', 0.000, 20.0, 24.000)],
        },
    ],
    // m17
    [
        Line {
            baseline: 12.000,
            tag: 30.000,
            word: 50.904,
            red: false,
            marker: &[('1', 0.000, 20.0, 12.000), ('.', 8.140, 20.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 30.000,
            word: 50.904,
            red: false,
            marker: &[('2', 0.000, 20.0, 24.000), ('.', 12.200, 20.0, 24.000)],
        },
    ],
    // m18
    [
        Line {
            baseline: 12.000,
            tag: 19.522,
            word: 40.956,
            red: false,
            marker: &[('1', 0.000, 20.0, 12.000), ('.', 8.140, 20.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 23.584,
            word: 45.018,
            red: false,
            marker: &[('2', 0.000, 20.0, 24.000), ('.', 12.200, 20.0, 24.000)],
        },
    ],
    // m19
    [
        Line {
            baseline: 12.000,
            tag: 7.109,
            word: 29.009,
            red: true,
            marker: &[('•', 0.000, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 7.109,
            word: 29.009,
            red: true,
            marker: &[('•', 0.000, 10.0, 24.000)],
        },
    ],
    // m20
    [
        Line {
            baseline: 12.000,
            tag: 19.424,
            word: 43.964,
            red: false,
            marker: &[('1', 0.000, 20.0, 12.000), ('.', 8.624, 20.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 23.398,
            word: 47.938,
            red: false,
            marker: &[('2', 0.000, 20.0, 24.000), ('.', 12.598, 20.0, 24.000)],
        },
    ],
    // m21
    [
        Line {
            baseline: 12.000,
            tag: 9.712,
            word: 31.452,
            red: true,
            marker: &[('1', 0.000, 10.0, 12.000), ('.', 4.312, 10.0, 12.000)],
        },
        Line {
            baseline: 24.000,
            tag: 11.699,
            word: 33.439,
            red: true,
            marker: &[('2', 0.000, 10.0, 24.000), ('.', 6.299, 10.0, 24.000)],
        },
    ],
];

/// The marker's glyph sizes agree to well under the 0.1 pt asked.
const SIZE_TOLERANCE: f32 = 0.1;

#[test]
fn list_marker_character_styles_render_as_indesign_does() {
    use paged_gen::samples::list_marker_styles as lms;
    let bytes = paged_gen::write_idml(&lms::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let mono = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus/fonts/JetBrainsMono-VF.ttf"),
    )
    .expect("read JetBrainsMono-VF.ttf");
    let mut assets = paged_renderer::BytesResolver::new();
    assets.add_font("Inter", None, font.clone());
    assets.add_font("JetBrains Mono", None, mono);
    let opts = PipelineOptions {
        font: Some(&font),
        assets: Some(&assets),
        collect_glyph_runs: true,
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    let page = &built.pages[0];
    let glyphs = &page.list.glyph_runs.as_ref().expect("glyph runs").entries;

    let near = |a: f32, b: f32, tol: f32| (a - b).abs() <= tol;
    let mut report = Vec::new();
    let mut ok = true;
    for (i, case) in lms::cases().iter().enumerate() {
        let (fx, fy) = lms::frame_origin(i as u32);
        let (fx, fy) = (fx + TEXT_AREA_INSET, fy + TEXT_AREA_INSET);
        let lines = built.story_layout(&lms::body_story_id(i as u32));
        assert_eq!(lines.len(), 2, "{}: one line per paragraph", case.name);
        for (line, want) in lines.iter().zip(&INDESIGN_MARKER_STYLES[i]) {
            let end = line.byte_range.end;
            let x_at = |byte: u32| {
                line.clusters
                    .iter()
                    .find(|c| c.byte == byte)
                    .map(|c| c.x_pt - fx)
                    .unwrap_or(f32::NAN)
            };
            let baseline = line.baseline_y_pt - fy;
            let tag = x_at(end - 7);
            let word = x_at(end - 3);
            let marker_left = line.marker.first().map_or(f32::NAN, |c| c.x_pt);
            let marker_right = x_at(end - 7) + fx;
            // The marker's visible glyphs: the outlines inside this
            // line's band left of the tag.
            let mut marker: Vec<_> = glyphs
                .iter()
                .filter(|g| !g.is_stroke)
                .filter(|g| {
                    let (x, y) = (g.transform.0[4], g.transform.0[5]);
                    x >= marker_left - 0.01
                        && x < marker_right - 0.01
                        && (y - line.baseline_y_pt).abs() < 6.0
                })
                .collect();
            marker.sort_by(|a, b| a.transform.0[4].total_cmp(&b.transform.0[4]));
            let red = marker.iter().any(
                |g| matches!(g.paint, paged_compose::Paint::Solid(c) if c.r > 0.5 && c.g < 0.2),
            );
            let mut same = near(baseline, want.baseline, X_TOLERANCE)
                && near(tag, want.tag, X_TOLERANCE)
                && near(word, want.word, X_TOLERANCE)
                && red == want.red
                && marker.len() == want.marker.len();
            // The text after the marker keeps its own 10 pt (every case's
            // `m` is 10 pt in InDesign's export): a slice drawn at the
            // marker's size would squash it.
            let tag_size = glyphs
                .iter()
                .filter(|g| !g.is_stroke)
                .find(|g| {
                    (g.transform.0[4] - marker_right).abs() < 0.01
                        && (g.transform.0[5] - line.baseline_y_pt).abs() < 6.0
                })
                .map_or(f32::NAN, |g| g.font_size);
            same &= near(tag_size, 10.0, SIZE_TOLERANCE);
            let mut got = vec![format!("tag {tag_size:.1}pt")];
            for (g, w) in marker.iter().zip(want.marker) {
                let (gx, gy) = (g.transform.0[4] - fx, g.transform.0[5] - fy);
                same &= near(gx, w.1, X_TOLERANCE)
                    && near(g.font_size, w.2, SIZE_TOLERANCE)
                    && near(gy, w.3, X_TOLERANCE);
                got.push(format!("({gx:.3}, {:.1}pt, {gy:.3})", g.font_size));
            }
            ok &= same;
            report.push(format!(
                "{} {:32} {:8} engine base {baseline:.3} tag {tag:.3} word {word:.3} red {red} [{}]  \
                 indesign base {:.3} tag {:.3} word {:.3} red {} {:?}",
                case.tag,
                case.name,
                if same { "ok" } else { "DIFFERS" },
                got.join(" "),
                want.baseline,
                want.tag,
                want.word,
                want.red,
                want.marker
            ));
        }
    }
    eprintln!("{}", report.join("\n"));
    assert!(ok, "\n{}", report.join("\n"));
}
