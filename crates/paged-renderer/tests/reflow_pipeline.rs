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

//! thoughts ADR 026 — page growth over the generated `reflow.idml`,
//! against what InDesign 2025's Smart Text Reflow did with the same file
//! (`tools/indesign-export/reflow-probe.sh`, 2026-10-01):
//!
//! - two authored pages, one threaded frame each (33 lines a frame), 80
//!   one-line paragraphs: InDesign added ONE page, carrying paragraphs
//!   67-80 (14 lines), and the story stopped overset;
//! - the new frame sat at the page's margin box (36, 36, 300 x 400 in page
//!   coordinates), with the master of the chain's last page.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use paged_gen::samples::reflow;
use paged_renderer::{pipeline, PipelineOptions};

fn inter_font() -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

fn body_story() -> String {
    paged_gen::ids::self_id("reflow", "BodyStory", 0)
}

/// The fixture with the body story's chain set to grow.
fn growing_document(paragraphs: Option<usize>) -> paged_scene::Document {
    let bytes = paged_gen::write_idml(&reflow::build()).expect("write_idml");
    let mut doc = idml_import::import_idml_doc(&bytes).expect("import");
    let id = body_story();
    let story = doc
        .stories
        .iter_mut()
        .find(|s| s.self_id == id)
        .expect("body story");
    story.story.grow = Some(paged_model::FlowGrowRule::default());
    if let Some(n) = paragraphs {
        story.story.paragraphs.truncate(n);
    }
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

fn first_line_text(
    doc: &paged_scene::Document,
    built: &pipeline::BuiltDocument,
    page: usize,
) -> Option<String> {
    let id = body_story();
    let story = doc.stories.iter().find(|s| s.self_id == id)?;
    let page_id = &built.pages.get(page)?.id;
    let line = built
        .story_layout(&id)
        .into_iter()
        .find(|l| &l.page_id == page_id)?;
    let p = story.story.paragraphs.get(line.paragraph_idx as usize)?;
    Some(p.runs.iter().map(|r| r.text.as_str()).collect())
}

#[test]
fn an_overset_story_grows_one_page_like_indesign() {
    let doc = growing_document(None);
    let built = build(&doc, None);
    assert_eq!(built.pages.len(), 3, "InDesign added exactly one page");
    let overset = built
        .diagnostics
        .items
        .iter()
        .any(|d| d.code == paged_renderer::diagnostics::DiagnosticCode::OversetTextDropped);
    assert!(!overset, "the grown story is no longer overset");

    let id = body_story();
    let lines = built.story_layout(&id);
    let on_page_3 = lines
        .iter()
        .filter(|l| l.page_id == built.pages[2].id)
        .count();
    assert_eq!(
        on_page_3, 14,
        "InDesign put paragraphs 67-80 (14 lines) on page 3"
    );
    assert_eq!(
        first_line_text(&doc, &built, 2).as_deref(),
        Some("Paragraph 67 of the reflowing story."),
    );
    // The generated frame is the margin box: every line on page 3 starts
    // ON its left edge, 36 pt from the page's. The authored frames carry a
    // 0.25 pt stroke, so theirs start half of it further in, and the
    // generated one takes default options — no stroke. InDesign's own
    // reflowed export (`reflow-probe.sh`, `pdftotext -bbox`): page 1's
    // first word at 36.125, page 3's at 36.000.
    let x0 = |page: usize| {
        lines
            .iter()
            .filter(|l| l.page_id == built.pages[page].id)
            .flat_map(|l| l.clusters.iter().map(|c| c.x_pt))
            .fold(f32::INFINITY, f32::min)
    };
    assert!((x0(2) - 36.0).abs() < 0.01, "page 3 left edge {}", x0(2));
    assert!((x0(0) - 36.125).abs() < 0.01, "page 1 left edge {}", x0(0));
}

/// A growing story is bounded by its grow rule, not by the frame-chain
/// cycle guard (256 authored links). Before the fix the chain stopped at
/// 257 frames, the story oversetted there, and the grow loop doubled to
/// the 2 000-page cap: 1 744 empty pages. One paragraph per frame
/// (`StartParagraph = NextFrame`) makes a 400-page story cheap to lay out.
#[test]
fn a_400_page_story_grows_exactly_the_pages_it_needs() {
    let mut doc = growing_document(Some(1));
    let id = body_story();
    let story = doc
        .stories
        .iter_mut()
        .find(|s| s.self_id == id)
        .expect("body story");
    let template = story.story.paragraphs[0].clone();
    story.story.paragraphs = (1..=400)
        .map(|n| {
            let mut p = template.clone();
            p.runs.truncate(1);
            p.runs[0].text = format!("Paragraph {n} of a long story.");
            p.start_paragraph = Some(paged_model::StartParagraph::NextFrame);
            p
        })
        .collect();

    let built = build(&doc, None);
    assert_eq!(built.pages.len(), 400, "one page per paragraph, no padding");
    let overset = built
        .diagnostics
        .items
        .iter()
        .any(|d| d.code == paged_renderer::diagnostics::DiagnosticCode::OversetTextDropped);
    assert!(!overset, "the story fits its grown chain");
    let lines = built.story_layout(&id);
    for page in &built.pages {
        assert!(
            lines.iter().any(|l| l.page_id == page.id),
            "page {} holds a line of the story",
            page.id.0
        );
    }
    assert_eq!(
        built.pages.last().map(|p| p.id.0.clone()),
        Some(paged_scene::grow::generated_page_id(&id, 398))
    );
}

#[test]
fn generated_pages_have_stable_ids() {
    let doc = growing_document(None);
    let built = build(&doc, None);
    assert_eq!(
        built.pages[2].id.0,
        paged_scene::grow::generated_page_id(&body_story(), 1)
    );
}

#[test]
fn a_story_that_fits_generates_nothing() {
    // 60 paragraphs fit the two authored frames (66 lines).
    let doc = growing_document(Some(60));
    let built = build(&doc, None);
    assert_eq!(built.pages.len(), 2);
}

#[test]
fn the_hint_settles_and_is_reused() {
    let doc = growing_document(None);
    let hint = RefCell::new(HashMap::new());
    let first = build(&doc, Some(&hint));
    assert_eq!(first.pages.len(), 3);
    assert_eq!(hint.borrow().get(&body_story()).copied(), Some(1));
    // A rebuild from the hint lands on the same document.
    let again = build(&doc, Some(&hint));
    assert_eq!(again.pages.len(), 3);
    // Shrinking the story drops the generated page again.
    let short = growing_document(Some(30));
    let shrunk = build(&short, Some(&hint));
    assert_eq!(
        shrunk.pages.len(),
        2,
        "the generated page is removed when empty"
    );
    assert_eq!(hint.borrow().get(&body_story()).copied(), Some(0));
}

/// ADR 026 — an IDML export writes generated pages as REAL pages, as
/// InDesign does once it has reflowed. Re-imported WITHOUT any grow rule,
/// the document keeps its three pages and nothing is overset.
/// `PAGED_REFLOW_EXPORT=<path>` also writes the exported package, so
/// InDesign can be asked to open it (tools/indesign-export/reflow-probe.sh
/// with PAGED_REFLOW_EDIT=none).
#[test]
fn an_export_writes_generated_pages_as_real_pages() {
    let bytes = paged_gen::write_idml(&reflow::build()).expect("write_idml");
    let doc = growing_document(None);
    let mut counts = HashMap::new();
    counts.insert(body_story(), 1u32);
    let grown = doc.with_generated_pages(&counts);
    let exported = idml_export::write_idml(&grown, &bytes).expect("export");
    if let Ok(path) = std::env::var("PAGED_REFLOW_EXPORT") {
        std::fs::write(&path, &exported).expect("write exported idml");
    }

    let reimported = idml_import::import_idml_doc(&exported).expect("re-import");
    assert!(
        reimported.growing_stories().is_empty(),
        "IDML carries no grow rule"
    );
    let built = build(&reimported, None);
    assert_eq!(
        built.pages.len(),
        3,
        "the generated page is a real page now"
    );
    let overset = built
        .diagnostics
        .items
        .iter()
        .any(|d| d.code == paged_renderer::diagnostics::DiagnosticCode::OversetTextDropped);
    assert!(!overset, "the exported chain holds the whole story");
    let id = body_story();
    let on_page_3 = built
        .story_layout(&id)
        .iter()
        .filter(|l| l.page_id == built.pages[2].id)
        .count();
    assert_eq!(on_page_3, 14);
}
