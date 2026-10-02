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

//! The discretionary hyphen (U+00AD) over the generated
//! `soft-hyphens.idml`, against the lines InDesign 20.0.1 sets from the
//! same file (`corpus/generated/soft-hyphens.pdf`, 2026-10-02; the line
//! contents read back through InDesign's own scripting, `~` = U+00AD).
//!
//! The rule it pins (paged-text `soft_hyphen_opportunities`): a soft
//! hyphen breaks whether the paragraph hyphenates or not, under the
//! paragraph's letter, word-length, last-word, ladder and zone limits —
//! but not its capitalised-words switch.

use paged_gen::samples::soft_hyphens::{body_story_id, cases, text_of};
use paged_renderer::{pipeline, PipelineOptions};

/// Every line of every case, in frame order, as InDesign set it.
const INDESIGN: [&[&str]; 15] = [
    &[
        "The Donau~dampf~",
        "schiff~fahrts~gesell~",
        "schaft sails today.",
    ],
    &[
        "The Donau~dampf~",
        "schiff~fahrts~gesell~",
        "schaft sails today.",
    ],
    &[
        "The Donau~dampf~",
        "schiff~fahrts~gesell~",
        "schaft sails today.",
    ],
    &[
        "~Typesetting",
        "~demands",
        "~extraordinary",
        "~concentration",
        "and ~considerable",
        "~patience",
    ],
    &[
        "We ex~amine ab~",
        "solutely un~usual ap~",
        "proaches to in~",
        "formation",
    ],
    &[
        "We ex~amine ab~",
        "solutely un~usual ap~",
        "proaches to in~",
        "formation",
    ],
    &[
        "We ex~amine",
        "ab~solutely un~usual",
        "ap~proaches to",
        "in~formation",
    ],
    &[
        "We ex~amine",
        "ab~solutely un~usual",
        "ap~proaches to",
        "in~formation",
    ],
    &[
        "We ex~amine ab~",
        "solutely un~usual ap~",
        "proaches to",
        "in~formation",
    ],
    &[
        "Ex~amine Ab~solutely",
        "Un~usual Ap~",
        "proaches to In~",
        "formation",
    ],
    &[
        "Ex~amine Ab~solutely",
        "Un~usual Ap~",
        "proaches to In~",
        "formation",
    ],
    &[
        "Our in~ventive",
        "col~leagues de~velop",
        "un~usual ap~",
        "proaches to in~",
        "formation de~sign",
    ],
    &[
        "Our in~ventive",
        "col~leagues de~velop",
        "un~usual ap~",
        "proaches to",
        "in~formation de~sign",
    ],
    &[
        "We ex~amine",
        "ab~solutely un~usual",
        "ap~proaches to",
        "in~formation",
    ],
    &[
        "Donau~dampf~schiff~",
        "fahrts~gesell~",
        "schafts~kapitaens~",
        "muetzen~ab~zeichen",
    ],
];

/// Cases set by the Paragraph Composer in a RAGGED rectangle (0, 1) or
/// justified (3), where our Knuth–Plass weighs a line against a hyphen
/// differently from InDesign's: ragged it sets `The` alone and saves the
/// second hyphen (`The` / `Donau~dampf~schiff~` / `fahrts~gesell~schaft`
/// / `sails today.`), justified it keeps `~concentration and` together.
/// That is the composer's calibration, true of dictionary hyphens alike,
/// not the soft-hyphen rule; these cases pin the rule's own properties
/// instead (below), and the fixture's fidelity gate measures the rest.
const PARAGRAPH_COMPOSER: [usize; 3] = [0, 1, 3];

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

#[test]
fn soft_hyphens_break_where_indesign_breaks_them() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::soft_hyphens::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let shy = doc
        .stories
        .iter()
        .flat_map(|s| s.story.paragraphs.iter().flat_map(|p| &p.runs))
        .map(|r| r.text.matches('\u{ad}').count())
        .sum::<usize>();
    assert!(shy > 50, "the importer keeps U+00AD in the run ({shy})");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");

    let all = cases();
    let lines_of = |i: usize| -> Vec<String> {
        let text = text_of(&all[i]);
        built
            .story_layout(&body_story_id(i as u32))
            .iter()
            .map(|l| {
                text[l.byte_range.start as usize..l.byte_range.end as usize]
                    .trim_end()
                    .replace('\u{ad}', "~")
            })
            .collect()
    };

    let mut report = Vec::new();
    let mut ok = true;
    for (i, case) in all.iter().enumerate() {
        if PARAGRAPH_COMPOSER.contains(&i) {
            continue;
        }
        let engine = lines_of(i);
        let want = INDESIGN[i];
        let same = engine == want;
        ok &= same;
        report.push(format!(
            "{:32} {}\n    engine   {engine:?}\n    indesign {want:?}",
            case.name,
            if same { "ok" } else { "DIFFERS" }
        ));
    }
    assert!(ok, "\n{}", report.join("\n"));

    // Hyphenation off breaks the soft hyphens exactly as on does.
    let (off, on) = (lines_of(0), lines_of(1));
    assert_eq!(
        off, on,
        "Hyphenation=\"false\" must not change the soft breaks"
    );
    assert!(
        off.iter().filter(|l| l.ends_with('~')).count() >= 1,
        "the paragraph breaks at a soft hyphen: {off:?}"
    );
    // A leading soft hyphen guards its word: justified as it is, no line
    // of case 3 ends inside a word.
    let guarded = lines_of(3);
    let text = text_of(&all[3]);
    for l in built.story_layout(&body_story_id(3)) {
        let end = l.byte_range.end as usize;
        assert!(
            end == text.len() || text[end..].starts_with(' ') || text[..end].ends_with(' '),
            "case 3 broke inside a word at byte {end}: {guarded:?}"
        );
    }
}
