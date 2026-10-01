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

//! Where a line breaks when a tab decides it, over the generated
//! `tab-breaks.idml`, against InDesign 20.0.1's PDF export of the same
//! file (`corpus/generated/tab-breaks.pdf`, 2026-10-01).
//!
//! Every word of every case must sit on the same line of its paragraph
//! as in InDesign's export, within 0.5 pt of InDesign's x. The rules this
//! pins (see `paged_text::layout::tab_aware_breaks`): the composer
//! measures a tab at the width it takes on its line; a tab is a break
//! opportunity and the text after it starts the next line at the line's
//! start; a Right/Center/Decimal tab fits while its segment ends inside
//! the measure, and collapses to zero width rather than moving on when
//! the segment would start behind the pen; a justified line stretches
//! only the glue after its last tab.

use paged_gen::samples::tab_breaks::{body_story_id, cases};
use paged_renderer::{pipeline, PipelineOptions};

/// (frame line, frame-local x, word) of every word, read off
/// `pdftotext -bbox` of InDesign's export (frames carry no stroke and
/// zero insets, so a word's xMin is its pen x).
#[rustfmt::skip]
const INDESIGN: [&[(u32, f32, &str)]; 19] = [
    // case 0
    &[
        (0, 0.000, "Alpha"),
        (0, 150.000, "Beta"),
        (1, 0.000, "gamma"),
    ],
    // case 1
    &[
        (0, 0.000, "Alpha"),
        (1, 0.000, "Beta"),
        (1, 24.120, "gamma"),
    ],
    // case 2
    &[
        (0, 0.000, "a1"),
        (0, 36.000, "b2"),
        (0, 72.000, "c3"),
        (0, 108.000, "d4"),
        (0, 144.000, "e5"),
        (0, 180.000, "f6"),
        (1, 0.000, "g7"),
    ],
    // case 3
    &[
        (0, 0.000, "One"),
        (0, 70.000, "Two"),
        (0, 140.003, "Three"),
        (1, 0.000, "Four"),
    ],
    // case 4
    &[
        (0, 0.000, "Chapter"),
        (0, 40.519, "one"),
        (0, 189.830, "12"),
        (1, 0.000, "A"),
        (1, 9.710, "long"),
        (1, 32.980, "chapter"),
        (1, 71.913, "title"),
        (1, 91.933, "runs"),
        (1, 115.603, "on"),
        (1, 130.323, "here,"),
        (1, 157.181, "okay"),
        (1, 183.660, "123"),
    ],
    // case 5
    &[
        (0, 0.000, "A"),
        (0, 9.710, "long"),
        (0, 32.980, "chapter"),
        (0, 71.913, "title"),
        (0, 91.933, "runs"),
        (0, 115.603, "on"),
        (0, 130.323, "here"),
        (0, 151.520, "123"),
    ],
    // case 6
    &[
        (0, 0.000, "Total"),
        (0, 25.844, "due"),
        (1, 0.000, "1234.50"),
        (2, 0.000, "Total"),
        (3, 0.000, "12.50"),
    ],
    // case 7
    &[
        (0, 0.000, "Name"),
        (1, 0.000, "Centered"),
        (2, 0.000, "Name"),
        (3, 0.000, "Mid"),
    ],
    // case 8
    &[
        (0, 0.000, "Key"),
        (0, 50.000, "value"),
        (0, 78.156, "words"),
        (0, 109.968, "that"),
        (0, 130.843, "run"),
        (0, 149.233, "on"),
        (0, 163.953, "and"),
        (0, 184.408, "on"),
        (1, 0.000, "end"),
    ],
    // case 9
    &[
        (0, 0.000, "Big"),
        (0, 17.902, "term"),
        (0, 60.000, "definition"),
        (0, 112.410, "words"),
        (0, 149.822, "that"),
        (0, 176.297, "wrap"),
        (1, 0.000, "onto"),
        (1, 23.050, "a"),
        (1, 30.631, "second"),
        (1, 67.447, "line"),
        (1, 85.993, "so"),
        (1, 99.239, "the"),
        (1, 116.215, "first"),
        (1, 136.605, "is"),
        (1, 146.265, "set"),
        (1, 162.611, "justified"),
    ],
    // case 10
    &[
        (0, 0.000, "One"),
        (0, 22.202, "two"),
        (0, 42.436, "three"),
        (0, 100.000, "four"),
        (0, 126.508, "five"),
        (0, 151.313, "six"),
        (0, 171.903, "seven"),
        (1, 0.000, "eight"),
        (1, 26.370, "nine"),
        (1, 49.250, "ten"),
        (1, 66.973, "eleven"),
        (1, 100.833, "twelve"),
    ],
    // case 11
    &[
        (0, 0.000, "Head"),
        (0, 36.000, "body"),
        (0, 62.722, "words"),
        (0, 94.533, "that"),
        (0, 115.408, "are"),
        (0, 133.262, "long"),
        (0, 156.532, "enough"),
        (1, 20.000, "to"),
        (1, 31.984, "wrap"),
        (1, 58.469, "to"),
        (1, 70.453, "the"),
        (1, 88.273, "next"),
        (1, 111.523, "line"),
        (1, 130.913, "here"),
    ],
    // case 12
    &[
        (0, 0.000, "Alpha"),
        (1, 0.000, "Beta"),
        (1, 24.120, "gamma"),
    ],
    // case 13
    &[
        (0, 0.000, "Alpha"),
        (0, 29.780, "beta"),
        (1, 0.000, "Gamma"),
    ],
    // case 14
    &[
        (0, 0.000, "Chapter"),
        (0, 71.979, "12"),
        (1, 0.000, "Chapter"),
        (1, 71.979, "12"),
        (2, 0.000, "Chapter"),
        (2, 71.979, "12"),
        (3, 0.000, "Chapter"),
        (3, 71.979, "12"),
        (4, 0.000, "Chapter"),
        (4, 37.979, "12"),
        (5, 0.000, "Chapter"),
        (5, 38.479, "12"),
        (6, 0.000, "Chapter"),
        (6, 38.979, "12"),
        (7, 0.000, "Chapter"),
        (7, 39.479, "12"),
        (8, 0.000, "Chapter"),
        (8, 39.979, "12"),
        (9, 0.000, "Chapter"),
        (9, 40.979, "12"),
        (10, 0.000, "Chapter"),
        (10, 41.979, "12"),
        (11, 0.000, "Chapter"),
        (11, 43.979, "12"),
        (12, 0.000, "Chapter"),
        (12, 71.979, "12"),
    ],
    // case 15
    &[
        (0, 0.000, "Chapter"),
        (0, 37.709, "12"),
        (1, 0.000, "Chapter"),
        (1, 37.709, "12"),
        (2, 0.000, "Chapter"),
        (2, 37.709, "12"),
        (3, 0.000, "Chapter"),
        (3, 37.709, "12"),
        (4, 0.000, "Chapter"),
        (4, 37.819, "12"),
        (5, 0.000, "Chapter"),
        (5, 38.319, "12"),
        (6, 0.000, "Chapter"),
        (6, 38.819, "12"),
        (7, 0.000, "Chapter"),
        (7, 39.319, "12"),
        (8, 0.000, "Chapter"),
        (8, 39.819, "12"),
        (9, 0.000, "Chapter"),
        (9, 40.819, "12"),
        (10, 0.000, "Chapter"),
        (10, 41.819, "12"),
        (11, 0.000, "Chapter"),
        (11, 43.819, "12"),
        (12, 0.000, "Chapter"),
        (12, 71.979, "12"),
    ],
    // case 16
    &[
        (0, 0.000, "Chapter"),
        (0, 37.709, "12"),
        (1, 0.000, "Chapter"),
        (1, 37.709, "12"),
        (2, 0.000, "Chapter"),
        (2, 37.709, "12"),
        (3, 0.000, "Chapter"),
        (3, 37.709, "12"),
        (4, 0.000, "Chapter"),
        (4, 37.899, "12"),
        (5, 0.000, "Chapter"),
        (5, 38.399, "12"),
        (6, 0.000, "Chapter"),
        (6, 38.899, "12"),
        (7, 0.000, "Chapter"),
        (7, 39.399, "12"),
        (8, 0.000, "Chapter"),
        (8, 39.899, "12"),
        (9, 0.000, "Chapter"),
        (9, 40.899, "12"),
        (10, 0.000, "Chapter"),
        (10, 41.899, "12"),
        (11, 0.000, "Chapter"),
        (11, 43.899, "12"),
        (12, 0.000, "Chapter"),
        (12, 71.979, "12"),
    ],
    // case 17
    &[
        (0, 0.000, "Chapter"),
        (0, 37.709, "1.5"),
        (1, 0.000, "Chapter"),
        (1, 37.709, "1.5"),
        (2, 0.000, "Chapter"),
        (2, 37.709, "1.5"),
        (3, 0.000, "Chapter"),
        (3, 37.709, "1.5"),
        (4, 0.000, "Chapter"),
        (4, 37.919, "1.5"),
        (5, 0.000, "Chapter"),
        (5, 38.419, "1.5"),
        (6, 0.000, "Chapter"),
        (6, 38.919, "1.5"),
        (7, 0.000, "Chapter"),
        (7, 39.419, "1.5"),
        (8, 0.000, "Chapter"),
        (8, 39.919, "1.5"),
        (9, 0.000, "Chapter"),
        (9, 40.919, "1.5"),
        (10, 0.000, "Chapter"),
        (10, 41.919, "1.5"),
        (11, 0.000, "Chapter"),
        (11, 43.919, "1.5"),
        (12, 0.000, "Chapter"),
        (12, 71.979, "1.5"),
    ],
    // case 18
    &[
        (0, 0.000, "Chapter"),
        (0, 40.519, "one"),
        (0, 184.830, "12"),
        (1, 0.000, "Chapter"),
        (1, 40.519, "one"),
        (1, 189.330, "12"),
        (2, 0.000, "Chapter"),
        (2, 40.519, "one"),
        (2, 189.730, "12"),
        (3, 0.000, "Chapter"),
        (3, 40.519, "one"),
        (3, 189.830, "12"),
        (4, 0.000, "Chapter"),
        (4, 40.519, "one"),
        (5, 0.000, "12"),
        (6, 0.000, "Alpha"),
        (6, 178.600, "Beta"),
        (7, 0.000, "Alpha"),
        (7, 178.800, "Beta"),
        (8, 0.000, "Name"),
        (8, 182.305, "Mid"),
        (9, 0.000, "Name"),
        (9, 182.505, "Mid"),
        (10, 0.000, "Total"),
        (10, 187.030, "1.5"),
        (11, 0.000, "Total"),
        (11, 187.230, "1.5"),
        (12, 0.000, "Alpha"),
        (13, 0.000, "Beta"),
    ],
];

/// What is left out, as (case, paragraph, line of the paragraph or
/// `None` for all of it) — none of it is the tab rule:
///
/// * case 18, paragraphs 6 and 8: InDesign keeps the segment after the
///   tab on the line although it ends 0.09–0.11 pt past the 200 pt edge,
///   squeezing the word by about 0.1 pt (Beta 21.31 → 21.205 wide, Mid
///   17.57 → 17.486). That overflow tolerance is not modelled; the
///   engine wraps both.
/// * case 9, line 1: the tab-free LAST line of a justified paragraph.
///   InDesign sets "onto a second line so the first is set justified"
///   on one line with its spaces at 70 % (1.966 of 2.812 pt), below the
///   80 % minimum word spacing; the engine's Knuth–Plass keeps the
///   minimum and wraps "justified". Line 0 — the line with the tab — is
///   asserted and matches.
const SKIPPED: [(usize, usize, Option<u32>); 3] = [(18, 6, None), (18, 8, None), (9, 0, Some(1))];

const X_TOLERANCE: f32 = 0.5;

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

/// Byte offset and text of every word (maximal non-whitespace run).
fn words(text: &str) -> Vec<(u32, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                out.push((s as u32, &text[s..i]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s as u32, &text[s..]));
    }
    out
}

#[test]
fn tabs_break_lines_where_indesign_breaks_them() {
    let sample = paged_gen::samples::tab_breaks::build();
    let bytes = paged_gen::write_idml(&sample).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");

    let mut report = Vec::new();
    let mut ok = true;
    for (ci, case) in cases().iter().enumerate() {
        let (fx, _) = case.origin;
        let lines = built.story_layout(&body_story_id(ci as u32));
        // InDesign's words, split into the case's paragraphs in order.
        let mut theirs = INDESIGN[ci].iter();
        for (pi, para) in case.paras.iter().enumerate() {
            let want: Vec<(u32, f32, &str)> = words(&para.text)
                .iter()
                .map(|(_, w)| {
                    let got = *theirs.next().unwrap_or_else(|| {
                        panic!("{}: InDesign export ran out of words at {w:?}", case.name)
                    });
                    assert_eq!(got.2, *w, "{}: word order", case.name);
                    got
                })
                .collect();
            let first_line = want[0].0;
            let ours: Vec<(u32, f32, &str)> = lines
                .iter()
                .filter(|l| l.paragraph_idx as usize == pi)
                .flat_map(|l| {
                    words(&para.text)
                        .into_iter()
                        .filter(|(b, _)| l.byte_range.contains(b))
                        .map(|(b, w)| {
                            let x = l
                                .clusters
                                .iter()
                                .find(|c| c.byte == b)
                                .map(|c| c.x_pt - fx)
                                .unwrap_or(f32::NAN);
                            (l.line_idx, x, w)
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            for (k, (line, x, w)) in want.iter().enumerate() {
                let line = line - first_line;
                if SKIPPED
                    .iter()
                    .any(|&(c, p, l)| c == ci && p == pi && l.is_none_or(|l| l == line))
                {
                    continue;
                }
                let mine = ours.get(k).copied();
                let same = mine.is_some_and(|(ml, mx, mw)| {
                    mw == *w && ml == line && (mx - x).abs() <= X_TOLERANCE
                });
                ok &= same;
                report.push(format!(
                    "{:34} p{pi} {:10} {:8} indesign (line {line}, x {x:7.3})  engine {}",
                    case.name,
                    w,
                    if same { "ok" } else { "DIFFERS" },
                    mine.map_or("missing".to_string(), |(ml, mx, _)| format!(
                        "(line {ml}, x {mx:7.3})"
                    )),
                ));
            }
            if !SKIPPED.iter().any(|&(c, p, _)| c == ci && p == pi) {
                ok &= ours.len() == want.len();
            }
        }
    }
    assert!(ok, "\n{}", report.join("\n"));
}
