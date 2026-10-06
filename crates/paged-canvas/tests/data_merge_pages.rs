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

//! paged.data Wave 8b — what a Data Merge that ADDS pages needs from the
//! engine, pinned against a template InDesign itself wrote
//! (`fixtures/data-merge-empty-field-lines.idml`: one page with 36 pt
//! margins on a master with the same margins, one text frame whose story
//! is three lines of Data Merge placeholders — each a
//! `<HyperlinkTextSource>` with a `<Hyperlink>` in the designmap).
//!
//! - a page minted by `insertPage` / `duplicatePage` can be named by a
//!   `bindCreated` and addressed as `$h:<name>` by the batch's later
//!   children, so pages and their content are ONE undo step;
//! - a duplicated page owns COPIES of its frames' stories, and keeps its
//!   margins; an inserted page takes its master's (else its neighbour's);
//! - `duplicateElements` copies a story that holds hyperlink sources,
//!   minting fresh source and hyperlink ids.

use paged_canvas::{channel::Mutation, element_selection::ElementId, CanvasModel, CanvasOptions};
use serde_json::json;

fn template() -> CanvasModel {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/data-merge-empty-field-lines.idml");
    let bytes = std::fs::read(p).expect("fixture");
    CanvasModel::load("merge", &bytes, CanvasOptions::default()).expect("load")
}

fn wire(v: serde_json::Value) -> Mutation {
    serde_json::from_value(v).expect("wire spelling")
}

fn batch(ops: serde_json::Value) -> Mutation {
    wire(json!({ "op": "batch", "args": { "ops": ops } }))
}

fn page_ids(m: &CanvasModel) -> Vec<String> {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.pages.iter().filter_map(|p| p.self_id.clone()))
        .collect()
}

/// `(frame id, story id)` of every text frame on the spread holding `page`.
fn frames_on(m: &CanvasModel, page: &str) -> Vec<(String, String)> {
    let parsed = m
        .scene()
        .spreads
        .iter()
        .find(|s| {
            s.spread
                .pages
                .iter()
                .any(|p| p.self_id.as_deref() == Some(page))
        })
        .expect("page's spread");
    parsed
        .spread
        .text_frames
        .iter()
        .map(|f| {
            (
                f.self_id.clone().unwrap_or_default(),
                f.parent_story.clone().unwrap_or_default(),
            )
        })
        .collect()
}

fn story<'a>(m: &'a CanvasModel, id: &str) -> &'a paged_model::Story {
    &m.scene()
        .stories
        .iter()
        .find(|s| s.self_id == id)
        .unwrap_or_else(|| panic!("story {id}"))
        .story
}

fn text(m: &CanvasModel, id: &str) -> String {
    story(m, id)
        .paragraphs
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `(hyperlink_source, text)` of every source-tagged run in story `id`.
fn sources(m: &CanvasModel, id: &str) -> Vec<(String, String)> {
    story(m, id)
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .filter_map(|r| r.hyperlink_source.clone().map(|s| (s, r.text.clone())))
        .collect()
}

fn margins(m: &CanvasModel, page: &str) -> [f32; 4] {
    let p = m
        .pages()
        .into_iter()
        .find(|p| p.self_id == page)
        .unwrap_or_else(|| panic!("page {page}"));
    [
        p.margin_top_pt,
        p.margin_left_pt,
        p.margin_bottom_pt,
        p.margin_right_pt,
    ]
}

const TEMPLATE_PAGE: &str = "ud4";
const TEMPLATE_STORY: &str = "udf";

// ---------------------------------------------------------------------------
// D-22 — a page minted in a batch can be named
// ---------------------------------------------------------------------------

#[test]
fn an_inserted_page_is_addressable_by_handle_in_the_same_batch() {
    let mut m = template();
    let before = page_ids(&m);
    let undo_depth = m.applied_log_len();
    m.apply_mutation(&batch(json!([
        { "op": "insertPage", "args": { "afterPageId": TEMPLATE_PAGE, "masterId": null } },
        { "op": "bindCreated", "args": { "handle": "p" } },
        { "op": "insertTextFrame", "args": { "pageId": "$h:p", "bounds": [100.0, 100.0, 300.0, 400.0] } },
        { "op": "bindCreated", "args": { "handle": "f" } },
        { "op": "insertText", "args": { "storyId": "$h:f", "offset": 0, "text": "record 2" } },
    ])))
    .expect("pages and their content in one batch");

    let after = page_ids(&m);
    assert_eq!(after.len(), before.len() + 1, "one page added");
    let new_page = after.iter().find(|p| !before.contains(p)).unwrap().clone();
    let frames = frames_on(&m, &new_page);
    assert_eq!(frames.len(), 1, "the frame landed on the NEW page");
    assert_eq!(text(&m, &frames[0].1), "record 2");
    assert_eq!(m.applied_log_len(), undo_depth + 1, "ONE undo step");

    m.undo().expect("undo");
    assert_eq!(page_ids(&m), before, "one undo removes page and content");
}

#[test]
fn a_duplicated_page_is_addressable_by_handle_in_the_same_batch() {
    let mut m = template();
    let before = page_ids(&m);
    let undo_depth = m.applied_log_len();
    m.apply_mutation(&batch(json!([
        { "op": "duplicatePage", "args": { "page": TEMPLATE_PAGE } },
        { "op": "bindCreated", "args": { "handle": "copy" } },
        { "op": "insertFrame", "args": { "pageId": "$h:copy", "bounds": [40.0, 40.0, 80.0, 80.0] } },
        { "op": "duplicatePage", "args": { "page": "$h:copy" } },
        { "op": "bindCreated", "args": { "handle": "copy2" } },
        { "op": "applyMasterToPage", "args": { "page": "$h:copy2", "master": null } },
    ])))
    .expect("duplicate, address, duplicate the duplicate");
    let after = page_ids(&m);
    assert_eq!(after.len(), before.len() + 2);
    assert_eq!(m.applied_log_len(), undo_depth + 1, "ONE undo step");
    m.undo().expect("undo");
    assert_eq!(page_ids(&m), before);
}

#[test]
fn a_page_handle_is_not_an_element_and_an_element_handle_is_not_a_page() {
    let mut m = template();
    let before = page_ids(&m);
    let err = m
        .apply_mutation(&batch(json!([
            { "op": "insertPage", "args": { "afterPageId": TEMPLATE_PAGE, "masterId": null } },
            { "op": "bindCreated", "args": { "handle": "p" } },
            { "op": "deleteFrame", "args": { "frameId": "$h:p" } },
        ])))
        .expect_err("a page in a frame position");
    assert!(format!("{err:?}").contains("page"), "{err:?}");
    assert_eq!(page_ids(&m), before, "rolled back");

    let err = m
        .apply_mutation(&batch(json!([
            { "op": "insertFrame", "args": { "pageId": TEMPLATE_PAGE, "bounds": [40.0, 40.0, 80.0, 80.0] } },
            { "op": "bindCreated", "args": { "handle": "r" } },
            { "op": "duplicatePage", "args": { "page": "$h:r" } },
        ])))
        .expect_err("an element in a page position");
    assert!(format!("{err:?}").contains("page"), "{err:?}");
    assert_eq!(page_ids(&m), before, "rolled back");
}

// ---------------------------------------------------------------------------
// D-23 — margins, and a duplicate that owns its stories
// ---------------------------------------------------------------------------

#[test]
fn a_duplicated_page_owns_copies_of_its_stories_and_keeps_its_margins() {
    let mut m = template();
    let template_text = text(&m, TEMPLATE_STORY);
    let template_sources = sources(&m, TEMPLATE_STORY);
    assert_eq!(template_sources.len(), 3, "three placeholders");
    let hyperlinks = m.scene().designmap.hyperlinks.len();

    let before = page_ids(&m);
    m.apply_mutation(&wire(
        json!({ "op": "duplicatePage", "args": { "page": TEMPLATE_PAGE } }),
    ))
    .expect("duplicate");
    let copy = page_ids(&m)
        .into_iter()
        .find(|p| !before.contains(p))
        .unwrap();

    assert_eq!(
        margins(&m, &copy),
        margins(&m, TEMPLATE_PAGE),
        "margins kept"
    );
    assert_eq!(margins(&m, &copy), [36.0; 4]);

    let frames = frames_on(&m, &copy);
    assert_eq!(frames.len(), 1);
    let copy_story = frames[0].1.clone();
    assert_ne!(copy_story, TEMPLATE_STORY, "the copy has its own story");
    assert_eq!(text(&m, &copy_story), template_text, "with the same text");

    // Its placeholders are fresh sources with fresh hyperlinks that point
    // where the template's did.
    let copy_sources = sources(&m, &copy_story);
    assert_eq!(copy_sources.len(), 3);
    for ((src, t), (orig, ot)) in copy_sources.iter().zip(&template_sources) {
        assert_ne!(src, orig, "fresh source id");
        assert_eq!(t, ot);
    }
    let dm = &m.scene().designmap;
    assert_eq!(
        dm.hyperlinks.len(),
        hyperlinks + 3,
        "one hyperlink per copied source"
    );
    for ((src, _), (orig, _)) in copy_sources.iter().zip(&template_sources) {
        let dest_of = |s: &str| {
            dm.hyperlinks
                .iter()
                .find(|h| {
                    h.source
                        .as_deref()
                        .map(|x| x.rsplit('/').next() == Some(s))
                        .unwrap_or(false)
                })
                .and_then(|h| h.destination.clone())
        };
        assert!(
            dest_of(src).is_some(),
            "copied source {src} has a hyperlink"
        );
        assert_eq!(dest_of(src), dest_of(orig));
    }

    // Editing the copy leaves the template alone.
    m.apply_mutation(&wire(json!({ "op": "insertText", "args": {
        "storyId": copy_story, "offset": 0, "text": "COPY " } })))
        .expect("edit the copy");
    assert_eq!(
        text(&m, TEMPLATE_STORY),
        template_text,
        "template untouched"
    );
    assert!(text(&m, &copy_story).starts_with("COPY "));

    // Undo takes the stories and hyperlinks with the page.
    m.undo().expect("undo edit");
    m.undo().expect("undo duplicate");
    assert_eq!(page_ids(&m), before);
    assert!(
        !m.scene().stories.iter().any(|s| s.self_id == copy_story),
        "the copied story went with the page"
    );
    assert_eq!(m.scene().designmap.hyperlinks.len(), hyperlinks);

    // Redo brings back the same ids.
    m.redo().expect("redo duplicate");
    assert!(m.scene().stories.iter().any(|s| s.self_id == copy_story));
    assert_eq!(m.scene().designmap.hyperlinks.len(), hyperlinks + 3);
}

#[test]
fn an_inserted_page_takes_its_masters_margins_else_its_neighbours() {
    let mut m = template();
    let master = m.scene().spreads[0].spread.pages[0]
        .applied_master
        .clone()
        .expect("template page has a master");
    assert!(
        !m.scene()
            .master_spread(&master)
            .expect("master")
            .spread
            .page_margins
            .is_empty(),
        "the master declares margins, so the first insert reads them"
    );
    for master_id in [Some(master.clone()), None] {
        let before = page_ids(&m);
        m.apply_mutation(&Mutation::InsertPage {
            after_page_id: Some(paged_canvas::PageId(TEMPLATE_PAGE.into())),
            master_id: master_id.clone(),
        })
        .expect("insert");
        let new_page = page_ids(&m)
            .into_iter()
            .find(|p| !before.contains(p))
            .unwrap();
        assert_eq!(
            margins(&m, &new_page),
            [36.0; 4],
            "master {master_id:?}: InDesign gives a new page its master's margins"
        );
    }
}

// ---------------------------------------------------------------------------
// D-24 — duplicateElements copies a story with hyperlink sources
// ---------------------------------------------------------------------------

#[test]
fn duplicate_elements_copies_a_story_that_holds_placeholders() {
    let mut m = template();
    let (frame, _) = frames_on(&m, TEMPLATE_PAGE)[0].clone();
    let hyperlinks = m.scene().designmap.hyperlinks.len();
    let template_sources = sources(&m, TEMPLATE_STORY);
    let out = m
        .apply_mutation(&Mutation::DuplicateElements {
            element_ids: vec![ElementId::TextFrame(frame.clone())],
            offset: (0.0, 300.0),
        })
        .expect("a frame whose story holds hyperlink sources duplicates");
    let new_frame = out.created_id.expect("created").raw_id().to_string();
    let copy_story = frames_on(&m, TEMPLATE_PAGE)
        .into_iter()
        .find(|(f, _)| *f == new_frame)
        .expect("clone on the page")
        .1;
    assert_ne!(copy_story, TEMPLATE_STORY);
    assert_eq!(text(&m, &copy_story), text(&m, TEMPLATE_STORY));
    let copy_sources = sources(&m, &copy_story);
    assert_eq!(copy_sources.len(), template_sources.len());
    for ((s, _), (o, _)) in copy_sources.iter().zip(&template_sources) {
        assert_ne!(s, o);
    }
    assert_eq!(m.scene().designmap.hyperlinks.len(), hyperlinks + 3);
    m.undo().expect("undo");
    assert_eq!(m.scene().designmap.hyperlinks.len(), hyperlinks);
    assert!(!m.scene().stories.iter().any(|s| s.self_id == copy_story));
}

// ---------------------------------------------------------------------------
// D-25 — a paragraph mark between two adjacent placeholders
// ---------------------------------------------------------------------------

/// The reader lives in plugin-publish (`idml-import`); the fix is
/// plugin-publish `29710d1` (branch `data/br-between-sources`). Verified
/// green here with a local `[patch]` onto that commit; un-ignore when the
/// workspace's `idml-import` pin moves to a rev that contains it.
#[test]
#[ignore = "D-25: needs the idml-import pin to include plugin-publish 29710d1"]
fn a_paragraph_mark_between_adjacent_placeholders_stays_between_them() {
    let m = template();
    assert_eq!(
        text(&m, TEMPLATE_STORY),
        "<<name>>\n<<subtitle>>\nPrice: <<price>>"
    );
    let texts: Vec<String> = sources(&m, TEMPLATE_STORY)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(texts, ["<<name>>", "<<subtitle>>", "<<price>>"]);
}
