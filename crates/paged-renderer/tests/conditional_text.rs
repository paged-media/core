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

//! End-to-end conditional-text visibility, against InDesign's answer.
//!
//! Renders the `conditions` gen sample — a numbered matrix of runs under
//! visible, hidden, several and undeclared conditions, in body text, on
//! an anchored object's marker, in table cells and on footnote
//! references — and asserts every case prints exactly when InDesign
//! 20.0.1 printed it (`conditions::Case::prints`; the reference is
//! `corpus/generated/conditions.pdf`).
//!
//! Both spellings: the fixture's own (hidden cases inside `<HiddenText>`,
//! as InDesign writes them; the importer drops that text) and the inline
//! one (every case in the story, the `Visible` flags alone saying what is
//! hidden — what the renderer's filter has to get right).

use std::path::PathBuf;

use paged_gen::samples::conditions::{self, Place};
use paged_renderer::{pipeline, PipelineOptions};

fn read_font(name: &str) -> Vec<u8> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// All glyph-run unicode on a page, joined in command order with
/// whitespace dropped (whitespace glyphs carry no unicode).
fn glyph_text(page: &paged_renderer::BuiltPage) -> String {
    let table = page
        .list
        .glyph_runs
        .as_ref()
        .expect("collect_glyph_runs must be on");
    let mut entries: Vec<_> = table.entries.iter().collect();
    entries.sort_by_key(|e| e.command_index);
    entries
        .iter()
        .filter_map(|e| e.unicode)
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn build(sample: paged_gen::Sample) -> paged_renderer::BuiltDocument {
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let mut document = idml_import::import_idml_doc(&bytes).unwrap();
    anchor_footnotes(&mut document);
    let font = read_font("OpenSans.ttf");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).unwrap()
}

/// Give each footnote the anchor the importer does not record yet
/// (`Footnote::anchor` is `None` from `idml-import`): the character
/// offset right after its reference's word, where the sample writes the
/// `<Footnote>`. Without it every reference reads as standing at its
/// paragraph's start, and a hidden one cannot take its note along.
fn anchor_footnotes(document: &mut paged_scene::Document) {
    let cases = conditions::cases();
    for story in &mut document.stories {
        for paragraph in &mut story.story.paragraphs {
            let mut spans = Vec::new();
            let mut pos = 0u32;
            for r in &paragraph.runs {
                let n = r.text.chars().count() as u32;
                spans.push((r.text.clone(), pos + n));
                pos += n;
            }
            for note in &mut paragraph.footnotes {
                let body: String = note
                    .paragraphs
                    .iter()
                    .flat_map(|p| p.runs.iter())
                    .map(|r| r.text.as_str())
                    .collect();
                let word = cases
                    .iter()
                    .find(|c| conditions::footnote_text(c.number) == body)
                    .map(|c| c.word);
                note.anchor = word.and_then(|w| {
                    spans
                        .iter()
                        .find(|(text, _)| text.contains(w))
                        .map(|&(_, end)| end)
                });
            }
        }
    }
}

fn assert_prints_as_indesign(built: &paged_renderer::BuiltDocument, spelling: &str) {
    let text = glyph_text(&built.pages[0]);
    for case in conditions::cases() {
        assert_eq!(
            text.contains(case.word),
            case.prints,
            "{spelling}: case {} ({}) — InDesign {} {:?}; page text {text:?}",
            case.number,
            case.label,
            if case.prints { "prints" } else { "hides" },
            case.word,
        );
        if case.place == Place::Footnote {
            let note: String = conditions::footnote_text(case.number)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            assert_eq!(
                text.contains(&note),
                case.prints,
                "{spelling}: case {}'s note goes with its reference; page text {text:?}",
                case.number,
            );
        }
    }
}

#[test]
fn fixture_prints_what_indesign_printed() {
    assert_prints_as_indesign(&build(conditions::build()), "<HiddenText> spelling");
}

#[test]
fn inline_conditions_print_what_indesign_printed() {
    assert_prints_as_indesign(&build(conditions::build_inline()), "inline spelling");
}

/// The anchored squares a page draws: unit paths filled black at the
/// sample's square size.
fn anchored_squares(built: &paged_renderer::BuiltDocument) -> usize {
    built.pages[0]
        .list
        .commands
        .iter()
        .filter(|c| match c {
            paged_compose::DisplayCommand::FillPath { transform, .. } => {
                let side = conditions::ANCHOR_SIDE_PT;
                transform.0[0] == side && transform.0[3] == side
            }
            _ => false,
        })
        .count()
}

fn anchors_printing() -> usize {
    conditions::cases()
        .iter()
        .filter(|c| c.place == Place::Anchor && c.prints)
        .count()
}

/// A hidden anchor marker takes its object along: only the squares of
/// the anchor cases InDesign prints are drawn (and the one that prints
/// keeps its own place: the filter runs before objects are matched to
/// their placeholder characters, which go by order).
#[test]
fn hidden_anchor_marker_takes_its_object() {
    let built = build(conditions::build_inline());
    assert_eq!(anchored_squares(&built), anchors_printing());
}

/// The same through the fixture's own spelling. `idml-import` drops the
/// TEXT inside `<HiddenText>` but still attaches an anchored object
/// written there to its paragraph, so the hidden square is drawn.
#[test]
#[ignore = "plugin-publish idml-import attaches an anchored object inside <HiddenText>"]
fn hidden_text_takes_its_anchored_object() {
    let built = build(conditions::build());
    assert_eq!(anchored_squares(&built), anchors_printing());
}
