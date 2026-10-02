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

//! thoughts ADR 026 + 028 — Word's widow control on a GROWING chain, over
//! the generated `keeps-reflow.idml`, against InDesign 2025's answer for
//! the same file (`tools/indesign-export/reflow-probe.sh`,
//! `PAGED_REFLOW_EDIT=thread`, 2026-10-02; `keeps-reflow.reflow.json`).
//!
//! Every paragraph keeps 2 / 2 lines at a break and the headings keep with
//! the next paragraph: the lowering plugin-doc gives every Word document.
//! InDesign lays the story on nine pages, each full but the fourth (17
//! lines: the heading P22 and its 3-line paragraph do not fit the 3 lines
//! left) and the last. The engine used to lay it on ten, page 5 holding
//! the lone heading P22 and page 9 two lines of P38, each followed by
//! empty space: a break decided for a paragraph straddling a page stayed
//! forced after an earlier break had moved the paragraph to the top of the
//! next page.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use paged_gen::samples::keeps_reflow;
use paged_renderer::{pipeline, PipelineOptions};

/// InDesign's page map: each page's first line, last line, line count.
const INDESIGN: [(&str, &str, usize); 9] = [
    ("P01head", "P06lineDmmmm", 20),
    ("P07lineAmmmm", "P12lineBmmmm", 20),
    ("P12lineCmmmm", "P17lineCmmmm", 20),
    ("P18head", "P21lineEmmmm", 17),
    ("P22head", "P28lineBmmmm", 20),
    ("P28lineCmmmm", "P33lineCmmmm", 20),
    ("P33lineDmmmm", "P38lineBmmmm", 20),
    ("P38lineCmmmm", "P44lineBmmmm", 20),
    ("P44lineCmmmm", "P44lineDmmmm", 2),
];

fn inter_font() -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

fn body_story() -> String {
    paged_gen::ids::self_id("keeps-reflow", "BodyStory", 0)
}

/// The fixture with the body story's chain set to grow (InDesign's rule:
/// generated frames take default options).
fn growing_document() -> paged_scene::Document {
    let bytes = paged_gen::write_idml(&keeps_reflow::build()).expect("write_idml");
    let mut doc = idml_import::import_idml_doc(&bytes).expect("import");
    let id = body_story();
    let story = doc
        .stories
        .iter_mut()
        .find(|s| s.self_id == id)
        .expect("body story");
    story.story.grow = Some(paged_model::FlowGrowRule::default());
    doc
}

fn build(
    doc: &paged_scene::Document,
    hint: Option<&RefCell<HashMap<String, u32>>>,
) -> pipeline::BuiltDocument {
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        grow_hint: hint,
        ..PipelineOptions::default()
    };
    pipeline::build_document(doc, &opts).expect("build_document")
}

/// Every page's (first line, last line, line count) of the body story.
fn page_map(
    doc: &paged_scene::Document,
    built: &pipeline::BuiltDocument,
) -> Vec<(String, String, usize)> {
    let id = body_story();
    let story = doc.stories.iter().find(|s| s.self_id == id).unwrap();
    let lines = built.story_layout(&id);
    let text = |l: &pipeline::LineLayout| -> String {
        let p = &story.story.paragraphs[l.paragraph_idx as usize];
        let t: String = p.runs.iter().map(|r| r.text.as_str()).collect();
        t[l.byte_range.start as usize..l.byte_range.end as usize]
            .trim()
            .to_string()
    };
    built
        .pages
        .iter()
        .map(|page| {
            let on: Vec<_> = lines.iter().filter(|l| l.page_id == page.id).collect();
            match (on.first(), on.last()) {
                (Some(a), Some(b)) => (text(a), text(b), on.len()),
                _ => (String::new(), String::new(), 0),
            }
        })
        .collect()
}

fn indesign() -> Vec<(String, String, usize)> {
    INDESIGN
        .iter()
        .map(|&(a, b, n)| (a.to_string(), b.to_string(), n))
        .collect()
}

#[test]
fn widow_control_on_a_growing_chain_paginates_like_indesign() {
    let doc = growing_document();
    let built = build(&doc, None);
    assert_eq!(page_map(&doc, &built), indesign());
}

#[test]
fn every_paragraph_is_a_line_per_token() {
    // The fixture's premise: a K-token paragraph is K lines, 159 in all.
    let doc = growing_document();
    let built = build(&doc, None);
    let lines = built.story_layout(&body_story());
    assert_eq!(lines.len(), 159);
    for (i, (tokens, _)) in keeps_reflow::paragraphs().iter().enumerate() {
        let n = lines
            .iter()
            .filter(|l| l.paragraph_idx as usize == i)
            .count();
        assert_eq!(n, tokens.len(), "paragraph {}", i + 1);
    }
}

#[test]
fn a_build_that_starts_at_the_final_page_count_lays_the_same_pages() {
    // The grow loop carries each build's settled breaks into the next;
    // one build on the final chain, from no breaks, must agree.
    let doc = growing_document();
    let hint = RefCell::new(HashMap::from([(body_story(), 7u32)]));
    let built = build(&doc, Some(&hint));
    assert_eq!(page_map(&doc, &built), indesign());
    assert_eq!(hint.borrow().get(&body_story()), Some(&7));
}
