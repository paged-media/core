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

//! Text variables and page markers resolve as InDesign resolves them.
//!
//! The `variables` paged-gen sample is written in InDesign's own
//! vocabulary (thoughts ADR 033, RFI C-37). [`INDESIGN`] is what InDesign
//! 20.0.1 printed for it — `pdftotext -layout` of its PDF export of the
//! sample, `corpus/generated/variables.pdf`, exported 2026-10-01 — one
//! entry per page, header and footer lines plus the variable-bearing body
//! lines. Every line must appear on the same page of our render.
//!
//! The dates are the day InDesign opened the file (creation, modification)
//! and exported it (output); the test pins the document clock to that day.
//! A cross-reference to page 2's story also resolves against the current
//! layout, and re-resolves when the destination moves.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use paged_compose::LinkTarget;
use paged_renderer::{pipeline, DateParts, DocumentClock, PipelineOptions};

/// InDesign's text, per page (`corpus/generated/variables.pdf`).
const INDESIGN: [&[&str]; 5] = [
    &[
        "First: Introduction: the First Heading. | Last: a second heading, on page one?",
        "Keyword first: alpha keyword | last: beta",
        "Upper, no end punctuation: INTRODUCTION: THE FIRST HEADING",
        "Lower: introduction: the first heading. | Title: Introduction: The First Heading.",
        "Sentence: Introduction: the first heading.",
        "Created 2026-10-01 | modified October 1, 2026 | output Thursday 01.10.26",
        "Page 1 of 3 | section ends at 2 | roman III",
        "Section: Part One | chapter 1 | Edition 7",
    ],
    &[
        "First: Introduction: the First Heading. | Last: a second heading, on page one?",
        "Keyword first: alpha keyword | last: beta",
        "Upper, no end punctuation: INTRODUCTION: THE FIRST HEADING",
        "Lower: introduction: the first heading. | Title: Introduction: The First Heading.",
        "Sentence: Introduction: the first heading.",
        "Page 2 of 3 | section ends at 2 | roman III",
        "Section: Part One | chapter 1 | Edition 7",
    ],
    &[
        "First: PART TWO begins (a third heading) | Last: PART TWO begins (a third heading)",
        "Keyword first: gamma | last: gamma",
        "Upper, no end punctuation: PART TWO BEGINS (A THIRD HEADING)",
        "Lower: part two begins (a third heading) | Title: Part Two Begins (a Third Heading)",
        "Sentence: Part two begins (a third heading)",
        "Jump: continued on page 3, previous 1.",
        "Page 1 of 3 | section ends at 3 | roman III",
        "Section: Part Two | chapter 1 | Edition 7",
    ],
    &[
        "First: PART TWO begins (a third heading) | Last: PART TWO begins (a third heading)",
        "Keyword first: gamma | last: gamma",
        "Upper, no end punctuation: PART TWO BEGINS (A THIRD HEADING)",
        "Lower: part two begins (a third heading) | Title: Part Two Begins (a Third Heading)",
        "Sentence: Part two begins (a third heading)",
        "Page 2 of 3 | section ends at 3 | roman III",
        "Section: Part Two | chapter 1 | Edition 7",
    ],
    &[
        "First: the final heading! | Last: the final heading!",
        "Keyword first: delta | last: delta",
        "Upper, no end punctuation: THE FINAL HEADING",
        "Lower: the final heading! | Title: The Final Heading!",
        "Sentence: The final heading!",
        "Jump: continued from page 1, next 3.",
        "Page 3 of 3 | section ends at 3 | roman III",
        "Section: Part Two | chapter 1 | Edition 7",
    ],
];

fn read_font(name: &str) -> Vec<u8> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// A page's glyph-run unicode in command (reading) order, whitespace
/// dropped (whitespace glyphs carry no unicode), so the comparison does
/// not depend on where lines wrap.
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

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The day InDesign opened and exported the reference (its export meta:
/// "Thu Oct 01 2026 23:14:00 GMT+0200").
fn indesign_clock() -> DocumentClock {
    let day = DateParts {
        year: 2026,
        month: 10,
        day: 1,
        hour: 23,
        minute: 14,
        second: 0,
    };
    DocumentClock {
        creation: day,
        modification: day,
        output: day,
    }
}

fn build_sample(
    sample: paged_gen::package::Sample,
    clock: DocumentClock,
) -> paged_renderer::BuiltDocument {
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let font = read_font("Inter.ttf");
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        collect_link_regions: true,
        document_clock: clock,
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).unwrap()
}

#[test]
fn every_page_resolves_its_variables_as_indesign_does() {
    let built = build_sample(paged_gen::samples::variables::build(), indesign_clock());
    assert_eq!(built.pages.len(), INDESIGN.len());
    let mut misses = Vec::new();
    for (page, lines) in INDESIGN.iter().enumerate() {
        let text = glyph_text(&built.pages[page]);
        for line in *lines {
            if !text.contains(&squash(line)) {
                misses.push(format!("page {}: {line:?}", page + 1));
            }
        }
        // The stored results never print: placeholders like
        // `<Header First>` are InDesign's master-page stand-ins.
        assert!(
            !text.contains('<'),
            "page {}: a stored placeholder leaked: {text:?}",
            page + 1
        );
    }
    assert!(
        misses.is_empty(),
        "lines InDesign printed that our render does not:\n{}\n\npage texts:\n{}",
        misses.join("\n"),
        built
            .pages
            .iter()
            .map(glyph_text)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn output_date_follows_the_injected_clock() {
    let mut clock = indesign_clock();
    clock.output = DateParts {
        year: 2030,
        month: 7,
        day: 4,
        hour: 0,
        minute: 0,
        second: 0,
    };
    let a = glyph_text(&build_sample(paged_gen::samples::variables::build(), clock).pages[0]);
    // 2030-07-04 is a Thursday too; the format is "EEEE dd.MM.yy".
    assert!(a.contains("outputThursday04.07.30"), "{a:?}");
    assert_eq!(
        glyph_text(&build_sample(paged_gen::samples::variables::build(), clock).pages[0]),
        a,
        "same clock, same output"
    );
}

#[test]
fn xref_resolves_to_destination_page_and_follows_it() {
    let at = |doc: &paged_renderer::BuiltDocument| {
        doc.pages.iter().find_map(|page| {
            page.list.link_regions.as_ref().and_then(|t| {
                t.regions.iter().find_map(|r| match r.target {
                    LinkTarget::PageIndex(i) => Some(i),
                    _ => None,
                })
            })
        })
    };
    let baseline = build_sample(paged_gen::samples::variables::build(), indesign_clock());
    assert_eq!(at(&baseline), Some(1), "page 2's story");
    // The moved variant inserts a blank page before it.
    let moved = build_sample(
        paged_gen::samples::variables::build_moved(),
        indesign_clock(),
    );
    assert_eq!(at(&moved), Some(2), "re-resolved against the new layout");
}

/// ADR 033's suspected staleness: the master-text emit cache was keyed by
/// (frame, page) and survived the grow passes, so a footer printing the
/// last page number kept the value of the pass that first emitted it.
/// The `reflow` fixture grows from two pages to three; its master story
/// here prints "Page <n> of <last page number>".
#[test]
fn a_growing_story_prints_the_final_page_count_in_every_footer() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::reflow::build()).unwrap();
    let mut doc = idml_import::import_idml_doc(&bytes).unwrap();
    let body = paged_gen::ids::self_id("reflow", "BodyStory", 0);
    let master = paged_gen::ids::self_id("reflow", "MasterStory", 0);
    doc.designmap
        .text_variables
        .push(paged_model::TextVariable {
            self_id: "TextVariable/last".to_string(),
            variable_type: Some("LastPageNumberType".to_string()),
            page_number_scope: Some("DocumentScope".to_string()),
            number_format: Some("Current".to_string()),
            ..Default::default()
        });
    for story in &mut doc.stories {
        if story.self_id == body {
            story.story.grow = Some(paged_model::FlowGrowRule::default());
        }
        if story.self_id == master {
            let para = &mut story.story.paragraphs[0];
            let template = para.runs.first().cloned().unwrap_or_default();
            let run = |text: &str, var: Option<&str>| paged_model::CharacterRun {
                text: text.to_string(),
                text_variable: var.map(str::to_string),
                ..template.clone()
            };
            para.runs = vec![
                run(
                    &format!("Page {} of ", paged_model::AUTO_PAGE_NUMBER_MARKER),
                    None,
                ),
                run("", Some("TextVariable/last")),
            ];
        }
    }
    let font = read_font("Inter.ttf");
    let cache = RefCell::new(HashMap::new());
    let hint = RefCell::new(HashMap::new());
    let opts = PipelineOptions {
        font: Some(&font),
        collect_glyph_runs: true,
        master_text_emit_cache: Some(&cache),
        grow_hint: Some(&hint),
        ..PipelineOptions::default()
    };
    // One build runs several grow passes over the same cache: the first
    // pass (two pages) fills it, the next (three pages) hits it. A hit
    // splices commands without glyph-run records, so a page whose footer
    // came from the cache shows NO footer text here, and a stale one
    // shows "of 2" — either fails.
    let built = pipeline::build_document(&doc, &opts).unwrap();
    assert_eq!(built.pages.len(), 3, "the story grows by one page");
    for (i, page) in built.pages.iter().enumerate() {
        let text = glyph_text(page);
        let footer: String = text
            .find("Page")
            .map(|at| text[at..].chars().take(9).collect())
            .unwrap_or_default();
        assert_eq!(footer, format!("Page{}of3", i + 1), "page {}", i + 1);
    }
}
