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

//! thoughts ADR 027 plan step 2 — the page-growth loop, seen from the
//! model: an overset story grows by the frames its dropped lines need
//! instead of doubling, and the count carried over from the last layout is
//! held to the rule's current `max_pages`. Every result is checked against
//! a cold build (`CanvasModel::digest_gate_check`).

use paged_canvas::{CanvasModel, CanvasOptions, Mutation};
use paged_gen::samples::docx_pagination::section_story_id;

fn inter() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).expect("read Inter.ttf")
}

/// `docx-pagination` with `extra` five-line paragraphs in section 1 and
/// both sections growing (Word's rule: generated frames copy the options).
fn growing_docx(extra: usize) -> CanvasModel {
    let idml = paged_gen::write_idml(&paged_gen::samples::docx_pagination::build()).expect("idml");
    let mut doc = idml_import::import_idml_doc(&idml).expect("import");
    let s0 = section_story_id(0);
    let story = doc
        .stories
        .iter_mut()
        .find(|s| s.self_id == s0)
        .expect("section 1");
    let template = story.story.paragraphs[0].clone();
    for n in 0..extra {
        let mut p = template.clone();
        p.runs.truncate(1);
        p.runs[0].text = format!(
            "Paragraph {n} carries three sentences of ordinary prose, the way a Word \
             report would. The second one continues the thought at about the same \
             length so the paragraph wraps. The third closes it after roughly five \
             lines of ten point text in the section frame."
        );
        story.story.paragraphs.push(p);
    }
    let paged = paged_store::package::wrap_document(&doc, "Long.docx", 612.0, 792.0).expect("wrap");
    let opts = CanvasOptions {
        fonts: vec![inter()],
        ..CanvasOptions::default()
    };
    let mut m = CanvasModel::load("doc", &paged, opts).expect("load");
    for i in 0..2 {
        grow_rule(&mut m, i, None);
    }
    m
}

fn grow_rule(m: &mut CanvasModel, section: u32, max_pages: Option<u32>) {
    m.apply_mutation(&Mutation::SetFlowGrowRule {
        story_id: section_story_id(section),
        grow: true,
        max_pages,
        copy_frame_options: Some(true),
    })
    .expect("setFlowGrowRule");
}

fn generated_pages(m: &CanvasModel, story: &str) -> usize {
    let prefix = format!("{story}_grow");
    m.built()
        .pages
        .iter()
        .filter(|p| p.id.0.starts_with(&prefix))
        .count()
}

#[test]
fn lowering_max_pages_drops_the_pages_already_grown() {
    let mut m = growing_docx(150);
    let s0 = section_story_id(0);
    let grown = generated_pages(&m, &s0);
    assert!(grown > 4, "section 1 grew {grown} pages");
    grow_rule(&mut m, 0, Some(3));
    assert_eq!(generated_pages(&m, &s0), 3, "the lowered cap holds");
    m.digest_gate_check()
        .unwrap_or_else(|e| panic!("capped growth is not a cold build: {e}"));
    // Raising it again regrows to what the story needs.
    grow_rule(&mut m, 0, None);
    assert_eq!(generated_pages(&m, &s0), grown);
    m.digest_gate_check()
        .unwrap_or_else(|e| panic!("regrowth is not a cold build: {e}"));
}

/// A paste at another leading that adds pages: the story grows by the
/// estimate (sized from line heights, not line counts) and lands where a
/// cold build does. The pass count is pinned in paged-renderer's
/// `pipeline::tests::growth_by_estimate_takes_two_builds`.
#[test]
fn a_page_adding_paste_grows_like_a_cold_build() {
    let mut m = growing_docx(400);
    let s0 = section_story_id(0);
    let paragraphs = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == s0)
        .expect("section 1")
        .story
        .paragraphs
        .clone();
    let end: usize = paragraphs
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.len()).sum::<usize>() + 1)
        .sum::<usize>()
        - 1;
    let insert = |m: &mut CanvasModel, offset: usize, text: String| {
        m.apply_mutation(&Mutation::InsertText {
            story_id: s0.clone(),
            offset: offset as u32,
            text,
            cell: None,
        })
        .expect("insertText");
    };
    insert(&mut m, end, "x".into());
    let pages = m.built().pages.len();

    let paste: String = (0..60)
        .map(|i| format!("\nPasted paragraph {i} of ordinary prose, long enough to wrap twice or so in the section frame of the document."))
        .collect();
    insert(&mut m, end + 1, paste);
    assert!(m.built().pages.len() > pages, "the paste adds pages");
    m.digest_gate_check()
        .unwrap_or_else(|e| panic!("grown by estimate is not a cold build: {e}"));
}
