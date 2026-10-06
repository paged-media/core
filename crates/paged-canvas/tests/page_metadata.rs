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

//! v69 — a page's plugin metadata (`SetPageMetadata`): set, read back
//! through the pages collection, carried by `MovePage`, copied by
//! `DuplicatePage` under the clone's own id, restored by undo of a
//! delete, and gated like item metadata.

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_wire::{Mutation, PageId};

const KEY: &str = "x-paged:media.paged.slide";

fn model() -> CanvasModel {
    let sample = paged_gen::samples::build("layout").expect("layout sample");
    let bytes = paged_gen::write_idml(&sample).expect("write");
    CanvasModel::load("pm", &bytes, CanvasOptions::default()).expect("load")
}

fn ids(m: &CanvasModel) -> Vec<String> {
    m.pages().into_iter().map(|p| p.self_id).collect()
}

fn meta(m: &CanvasModel, page: &str) -> Option<String> {
    m.pages()
        .into_iter()
        .find(|p| p.self_id == page)?
        .plugin_metadata
        .into_iter()
        .find(|e| e.key == KEY)
        .map(|e| e.value)
}

fn set(m: &mut CanvasModel, page: &str, value: Option<&str>, caller: Option<&str>) -> bool {
    m.apply_mutation(&Mutation::SetPageMetadata {
        page: PageId(page.to_string()),
        key: KEY.to_string(),
        value: value.map(str::to_string),
        caller: caller.map(str::to_string),
    })
    .is_ok()
}

const NOTES: &str = r#"{"v":1,"data":{"notes":"Speak slowly."}}"#;

#[test]
fn page_metadata_travels_with_its_page() {
    let mut m = model();
    let pages = ids(&m);
    assert!(pages.len() >= 2, "the layout sample has several pages");
    let (a, b) = (pages[0].clone(), pages[1].clone());

    assert!(set(&mut m, &b, Some(NOTES), Some("media.paged.slide")));
    assert_eq!(meta(&m, &b).as_deref(), Some(NOTES));
    assert_eq!(meta(&m, &a), None);

    // Moved to the front, the page keeps it.
    assert!(m
        .apply_mutation(&Mutation::MovePage {
            page: PageId(b.clone()),
            after: None,
        })
        .is_ok());
    assert_eq!(ids(&m)[0], b);
    assert_eq!(meta(&m, &b).as_deref(), Some(NOTES));

    // A duplicate carries its own copy, under its own id.
    assert!(m
        .apply_mutation(&Mutation::DuplicatePage {
            page: PageId(b.clone()),
        })
        .is_ok());
    let dup = ids(&m)[1].clone();
    assert_ne!(dup, b);
    assert_eq!(meta(&m, &dup).as_deref(), Some(NOTES));

    // Deleted and restored, it comes back with the page.
    assert!(m
        .apply_mutation(&Mutation::DeletePage {
            page_id: PageId(b.clone()),
        })
        .is_ok());
    assert!(!ids(&m).contains(&b));
    m.undo().expect("undo delete");
    assert_eq!(meta(&m, &b).as_deref(), Some(NOTES));

    // Deleting the entry, then undoing that.
    assert!(set(&mut m, &b, None, None));
    assert_eq!(meta(&m, &b), None);
    m.undo().expect("undo delete entry");
    assert_eq!(meta(&m, &b).as_deref(), Some(NOTES));
}

#[test]
fn page_metadata_is_gated_like_item_metadata() {
    let mut m = model();
    let p = ids(&m)[0].clone();
    // Not the caller's namespace.
    assert!(!set(&mut m, &p, Some(NOTES), Some("media.paged.draw")));
    // Not the envelope.
    assert!(!set(&mut m, &p, Some("notes"), None));
    // No such page.
    assert!(!set(&mut m, "nope", Some(NOTES), None));
    assert_eq!(meta(&m, &p), None);
}

/// v69 — `InsertHyperlink { page }`: a link to a page, read back with its
/// target from the hyperlinks collection, undone in one step, refused
/// for a page that does not exist.
#[test]
fn a_text_range_links_to_a_page() {
    let mut m = model();
    let pages = ids(&m);
    let story = m
        .scene()
        .stories
        .iter()
        .find(|s| {
            s.story
                .paragraphs
                .iter()
                .flat_map(|p| &p.runs)
                .map(|r| r.text.len())
                .sum::<usize>()
                >= 4
        })
        .map(|s| s.self_id.clone())
        .expect("the layout sample has text");
    let link = |page: &str| Mutation::InsertHyperlink {
        story_id: story.clone(),
        start: 0,
        end: 3,
        url: String::new(),
        page: Some(PageId(page.to_string())),
    };

    assert!(m.apply_mutation(&link("nope")).is_err());
    assert!(m.hyperlinks().is_empty());

    m.apply_mutation(&link(&pages[1])).expect("link to page 2");
    let links = m.hyperlinks();
    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].destination_page.as_deref(),
        Some(pages[1].as_str())
    );
    assert_eq!(links[0].destination_url, None);
    assert!(links[0]
        .destination
        .starts_with("HyperlinkPageDestination/"));

    m.undo().expect("undo");
    assert!(m.hyperlinks().is_empty());
    m.redo().expect("redo");
    assert_eq!(
        m.hyperlinks()[0].destination_page.as_deref(),
        Some(pages[1].as_str())
    );
}
