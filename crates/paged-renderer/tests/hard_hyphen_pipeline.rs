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

//! A line breaks after a hyphen the text already has (RFI C-54), as
//! InDesign 20.0.1 set the `hard-hyphen` paged-gen fixture: each row a
//! text, each column a frame width. The expected lines are InDesign's,
//! read from its PDF export of the same file.

use paged_gen::samples::hard_hyphen::{rows, story_id, WIDTHS};
use paged_renderer::{pipeline, PipelineOptions};

/// InDesign's lines per frame: `[row][column]`, lines joined by " / ".
const INDESIGN: [[&str; 5]; 6] = [
    [
        "The two- / way street",
        "The two- / way street",
        "The two-way / street",
        "The two-way / street",
        "The two-way street",
    ],
    // The Single-line Composer weighs the hyphen like a hyphenation point:
    // at 50 pt the zone keeps `two-way` whole.
    [
        "The / two-way / street",
        "The two- / way street",
        "The two-way / street",
        "The two-way / street",
        "The two-way street",
    ],
    [
        "The two- / way street",
        "The two- / way street",
        "The two-way / street",
        "The two-way / street",
        "The two-way street",
    ],
    [
        "The two- / way street",
        "The two- / way street",
        "The two-way / street",
        "The two-way / street",
        "The two-way street",
    ],
    [
        "A state- / of-the-art / x-ray",
        "A state-of- / the-art x-ray",
        "A state-of- / the-art x-ray",
        "A state-of-the- / art x-ray",
        "A state-of-the-art / x-ray",
    ],
    [
        "pages / 10\u{2013}20 and / so\u{2014}on",
        "pages 10\u{2013}20 / and so\u{2014}on",
        "pages 10\u{2013}20 / and so\u{2014}on",
        "pages 10\u{2013}20 and / so\u{2014}on",
        "pages 10\u{2013}20 and / so\u{2014}on",
    ],
];

/// Frames where the engine still differs from InDesign, none of them about
/// the hyphen rule: `(row, column, the engine's lines)`.
/// - (3, 1): justified. The Paragraph Composer refuses the loose line
///   `The two-` (one space to stretch) that InDesign sets.
/// - (4, 0): `of-the-art` fits InDesign's 50 pt line and misses the
///   engine's by a fraction of a point (InDesign squeezes such overflows,
///   see `tab-breaks`).
/// - (5, 3): no hyphen involved; the Paragraph Composer balances
///   `pages 10-20 / and so-on` where InDesign fills the first line.
const KNOWN: [(usize, usize, &str); 3] = [
    (3, 1, "The / two-way / street"),
    (4, 0, "A / state-of- / the-art / x-ray"),
    (5, 3, "pages 10\u{2013}20 / and so\u{2014}on"),
];

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

#[test]
fn lines_break_after_a_hard_hyphen_where_indesign_breaks_them() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::hard_hyphen::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    let mut wrong = Vec::new();
    for (r, (text, _)) in rows().iter().enumerate() {
        for c in 0..WIDTHS.len() {
            let lines: Vec<String> = built
                .story_layout(&story_id(r as u32, c as u32))
                .iter()
                .map(|l| {
                    text[l.byte_range.start as usize..l.byte_range.end as usize]
                        .trim()
                        .to_string()
                })
                .collect();
            let ours = lines.join(" / ");
            if let Some((_, _, known)) = KNOWN.iter().find(|(kr, kc, _)| (*kr, *kc) == (r, c)) {
                assert_eq!(
                    &ours, known,
                    "row {r} at {} pt changed: update or drop its KNOWN entry",
                    WIDTHS[c]
                );
                continue;
            }
            if ours != INDESIGN[r][c] {
                wrong.push(format!(
                    "row {r} at {} pt: ours {ours:?}, InDesign {:?}",
                    WIDTHS[c], INDESIGN[r][c]
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
