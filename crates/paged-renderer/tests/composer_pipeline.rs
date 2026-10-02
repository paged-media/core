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

//! The Adobe Single-line Composer (`Composer="HL Single"`) over the
//! generated `composer.idml`, against every line InDesign 2025 set in the
//! same file (read back from its DOM, `Line.contents`, 2026-10-02; the
//! PDF export `corpus/generated/composer.pdf` is the same composition).
//!
//! Each case is set twice, Paragraph Composer beside Single-line
//! Composer; this pins the single-line half line by line. A trailing
//! `-` marks a line InDesign ended in a hyphen.

use paged_gen::samples::composer::{body_story_id, cases};
use paged_renderer::{pipeline, PipelineOptions};

const INDESIGN: [&[&[&str]]; 14] = [
    // 1
    &[
        &[
            "The composer reads the",
            "whole paragraph before it",
            "sets a single line, weighing",
            "every place a line could",
            "end against every other,",
            "and it will happily make an",
            "early line a little looser if",
            "that saves a later one from",
            "looking wrong.",
        ],
        &[
            "A typesetter working by",
            "hand had no such luxury.",
            "Each line was filled from",
            "the left until the next word",
            "would not fit, and then the",
            "gaps were adjusted until",
            "the line was exactly as",
            "wide as the measure,",
            "which is what most word",
            "processors still do today.",
        ],
    ],
    // 2
    &[
        &[
            "Narrow columns are where",
            "the difference shows: with",
            "only a few words to a line",
            "there is very little space to",
            "share out, so one long",
            "word arriving at the wrong",
            "moment can open a gap",
            "that the reader sees from",
            "across the room.",
        ],
        &[
            "Hyphenation gives the",
            "breaker more ways to stop.",
            "Unquestionably the excep-",
            "tional characteristics of",
            "uncharacteristically com-",
            "prehensive documentation",
            "encourage considerable",
            "experimentation with",
            "typographical conventions.",
        ],
    ],
    // 3
    &[
        &[
            "A typesetter working by",
            "hand had no such luxury.",
            "Each line was filled from",
            "the left until the next word",
            "would not fit, and then the",
            "gaps were adjusted until",
            "the line was exactly as wide",
            "as the measure, which is",
            "what most word processors",
            "still do today.",
        ],
        &[
            "Narrow columns are where",
            "the difference shows: with",
            "only a few words to a line",
            "there is very little space to",
            "share out, so one long word",
            "arriving at the wrong",
            "moment can open a gap",
            "that the reader sees from",
            "across the room.",
        ],
    ],
    // 4
    &[
        &[
            "The composer reads the",
            "whole paragraph before it",
            "sets a single line, weighing",
            "every place a line could end",
            "against every other, and it",
            "will happily make an early",
            "line a little looser if that",
            "saves a later one from look-",
            "ing wrong.",
        ],
        &[
            "Hyphenation gives the",
            "breaker more ways to stop.",
            "Unquestionably the excep-",
            "tional characteristics of un-",
            "characteristically compre-",
            "hensive documentation",
            "encourage considerable",
            "experimentation with typo-",
            "graphical conventions.",
        ],
    ],
    // 5
    &[
        &[
            "Short words in a row, as in",
            "a list of it, is, as, to, be, or,",
            "at, by, on, up, so, no, an,",
            "we, he, do, go, me, my, of,",
            "if, us, give a line breaker",
            "many choices, and bigger",
            "ones like documentation",
            "and responsibilities give it",
            "few.",
        ],
        &[
            "The composer reads the",
            "whole paragraph before it",
            "sets a single line, weighing",
            "every place a line could",
            "end against every other,",
            "and it will happily make an",
            "early line a little looser if",
            "that saves a later one from",
            "looking wrong.",
        ],
    ],
    // 6
    &[
        &[
            "Short words in a row, as in a",
            "list of it, is, as, to, be, or, at,",
            "by, on, up, so, no, an, we,",
            "he, do, go, me, my, of, if, us,",
            "give a line breaker many",
            "choices, and bigger ones",
            "like documentation and",
            "responsibilities give it few.",
        ],
        &[
            "The composer reads the",
            "whole paragraph before it",
            "sets a single line, weighing",
            "every place a line could end",
            "against every other, and it",
            "will happily make an early",
            "line a little looser if that",
            "saves a later one from",
            "looking wrong.",
        ],
    ],
    // 7
    &[
        &[
            "A typesetter working by",
            "hand had no such luxury.",
            "Each line was filled from the",
            "left until the next word would",
            "not fit, and then the gaps",
            "were adjusted until the line",
            "was exactly as wide as the",
            "measure, which is what",
            "most word processors still",
            "do today.",
        ],
        &[
            "Narrow columns are where",
            "the difference shows: with",
            "only a few words to a line",
            "there is very little space to",
            "share out, so one long word",
            "arriving at the wrong",
            "moment can open a gap that",
            "the reader sees from across",
            "the room.",
        ],
    ],
    // 8
    &[
        &[
            "Short words in a row, as in",
            "a list of it, is, as, to, be, or,",
            "at, by, on, up, so, no, an,",
            "we, he, do, go, me, my, of,",
            "if, us, give a line breaker",
            "many choices, and bigger",
            "ones like documentation",
            "and responsibilities give it",
            "few.",
        ],
        &[
            "A typesetter working by",
            "hand had no such luxury.",
            "Each line was filled from",
            "the left until the next word",
            "would not fit, and then the",
            "gaps were adjusted until",
            "the line was exactly as",
            "wide as the measure,",
            "which is what most word",
            "processors still do today.",
        ],
    ],
    // 9
    &[
        &[
            "Internationaliza-",
            "tion and incom-",
            "prehensibility ar-",
            "rive together in",
            "this sentence so",
            "that a measure",
            "narrower than",
            "either word",
            "makes the com-",
            "poser break in-",
            "side them even if",
            "it does not want",
            "to.",
        ],
    ],
    // 10
    &[
        &[
            "Hyphenation gives the",
            "breaker more ways to stop.",
            "Unquestionably the excep-",
            "tional characteristics of un-",
            "characteristically compre-",
            "hensive documentation",
            "encourage considerable",
            "experimentation with typo-",
            "graphical conventions.",
        ],
        &[
            "Narrow columns are where",
            "the difference shows: with",
            "only a few words to a line",
            "there is very little space to",
            "share out, so one long",
            "word arriving at the wrong",
            "moment can open a gap",
            "that the reader sees from",
            "across the room.",
        ],
    ],
    // 11
    &[
        &[
            "Narrow columns are where",
            "the difference shows: with",
            "only a few words to a line",
            "there is very little space to",
            "share out, so one long word",
            "arriving at the wrong",
            "moment can open a gap",
            "that the reader sees from",
            "across the room.",
        ],
        &[
            "Short words in a row, as in a",
            "list of it, is, as, to, be, or, at,",
            "by, on, up, so, no, an, we,",
            "he, do, go, me, my, of, if, us,",
            "give a line breaker many",
            "choices, and bigger ones",
            "like documentation and",
            "responsibilities give it few.",
        ],
    ],
    // 12
    &[
        &[
            "The composer reads the",
            "whole paragraph before it",
            "sets a single line, weighing",
            "every place a line could",
            "end against every other,",
            "and it will happily make an",
            "early line a little looser if",
            "that saves a later one from",
            "looking wrong.",
        ],
        &[
            "Hyphenation gives the",
            "breaker more ways to stop.",
            "Unquestionably the excep-",
            "tional characteristics of",
            "uncharacteristically com-",
            "prehensive documentation",
            "encourage considerable",
            "experimentation with",
            "typographical conventions.",
        ],
    ],
    // 13
    &[
        &[
            "The composer reads the whole paragraph before it sets a single line, weighing every place a line",
            "could end against every other, and it will happily make an early line a little looser if that saves a",
            "later one from looking wrong.",
        ],
        &[
            "A typesetter working by hand had no such luxury. Each line was filled from the left until the next",
            "word would not fit, and then the gaps were adjusted until the line was exactly as wide as the",
            "measure, which is what most word processors still do today.",
        ],
        &[
            "Hyphenation gives the breaker more ways to stop. Unquestionably the exceptional characteris-",
            "tics of uncharacteristically comprehensive documentation encourage considerable experimen-",
            "tation with typographical conventions.",
        ],
    ],
    // 14
    &[
        &[
            "Narrow columns are where the difference shows: with only a few words to a line there is very",
            "little space to share out, so one long word arriving at the wrong moment can open a gap that",
            "the reader sees from across the room.",
        ],
        &[
            "Short words in a row, as in a list of it, is, as, to, be, or, at, by, on, up, so, no, an, we, he, do, go,",
            "me, my, of, if, us, give a line breaker many choices, and bigger ones like documentation and",
            "responsibilities give it few.",
        ],
        &[
            "Internationalization and incomprehensibility arrive together in this sentence so that a measure",
            "narrower than either word makes the composer break inside them even if it does not want to.",
        ],
    ],
];

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

/// Each paragraph's laid-out lines, a hyphenated one with its `-`.
fn engine_lines(
    doc: &paged_scene::Document,
    built: &pipeline::BuiltDocument,
    story_id: &str,
) -> Vec<Vec<String>> {
    let story = doc
        .stories
        .iter()
        .find(|s| s.self_id == story_id)
        .expect("story");
    let texts: Vec<String> = story
        .story
        .paragraphs
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect())
        .collect();
    let mut out: Vec<Vec<String>> = vec![Vec::new(); texts.len()];
    for l in built.story_layout(story_id) {
        let t = &texts[l.paragraph_idx as usize];
        let (a, b) = (l.byte_range.start as usize, l.byte_range.end as usize);
        let mut s = t[a..b.min(t.len())].trim_end().to_string();
        // A line that stops inside a word was hyphenated.
        let next = t[b.min(t.len())..].chars().next();
        if next.is_some_and(|c| !c.is_whitespace()) {
            s.push('-');
        }
        out[l.paragraph_idx as usize].push(s);
    }
    out
}

#[test]
fn single_line_composer_breaks_every_line_where_indesign_does() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::composer::build()).expect("idml");
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
        let engine = engine_lines(&doc, &built, &body_story_id(i as u32, 1));
        let want: Vec<Vec<String>> = INDESIGN[i]
            .iter()
            .map(|p| p.iter().map(|l| l.to_string()).collect())
            .collect();
        if engine != want {
            ok = false;
            report.push(format!(
                "{}. {}\n    engine   {engine:?}\n    indesign {want:?}",
                i + 1,
                case.name
            ));
        }
    }
    assert!(ok, "\n{}", report.join("\n"));
}

#[test]
fn the_composer_reaches_the_model_and_changes_the_breaks() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::composer::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    // Case 1 names its composer on the range; case 5 through its
    // paragraph style.
    let composer = |case: u32, c: u32| {
        let id = body_story_id(case, c);
        let s = doc.stories.iter().find(|s| s.self_id == id).unwrap();
        let p = &s.story.paragraphs[0];
        p.composer.clone().or_else(|| {
            let style = p.paragraph_style.as_deref()?;
            doc.styles.paragraph_styles.get(style)?.composer.clone()
        })
    };
    assert_eq!(composer(0, 0), Some(paged_model::Composer::Paragraph));
    assert_eq!(composer(0, 1), Some(paged_model::Composer::SingleLine));
    assert_eq!(composer(4, 1), Some(paged_model::Composer::SingleLine));

    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    // The same text and measure break differently under the two: the
    // Paragraph Composer keeps "the" off line 14 of case 1, the
    // Single-line Composer fills it, as InDesign does.
    let para = engine_lines(&doc, &built, &body_story_id(0, 0));
    let single = engine_lines(&doc, &built, &body_story_id(0, 1));
    assert_eq!(para[1][4], "would not fit, and then");
    assert_eq!(single[1][4], "would not fit, and then the");
}
