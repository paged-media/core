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

//! Hyperlinks deleted and edited over the wire (ADR 164): by the link's
//! id alone, undoable in one step, and saved. A link read back from a
//! saved package is a link of InDesign's spelling, so the edit after the
//! reload is the one that reaches an InDesign document's own links.

use paged_canvas::channel::HyperlinkSummary;
use paged_canvas::{CanvasModel, CanvasOptions, Mutation, PageId};
use paged_gen::samples::footnote_numbering as sample;

fn options() -> CanvasOptions {
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
    CanvasOptions {
        fonts: vec![font],
        ..CanvasOptions::default()
    }
}

fn model() -> CanvasModel {
    let idml = paged_gen::write_idml(&sample::build(&sample::NUMBERING)).expect("idml");
    CanvasModel::load("doc", &idml, options()).expect("load")
}

fn story() -> String {
    sample::story_id("footnote-numbering", "A")
}

/// A URL link over story A's first five characters; its id.
fn link(m: &mut CanvasModel) -> String {
    m.apply_mutation(&Mutation::InsertHyperlink {
        story_id: story(),
        start: 0,
        end: 5,
        url: "https://paged.media".into(),
        page: None,
    })
    .expect("insert");
    m.hyperlinks().last().expect("the link").self_id.clone()
}

fn find(m: &CanvasModel, id: &str) -> Option<HyperlinkSummary> {
    m.hyperlinks().into_iter().find(|h| h.self_id == id)
}

fn tagged_runs(m: &CanvasModel) -> usize {
    m.scene()
        .stories
        .iter()
        .flat_map(|s| &s.story.paragraphs)
        .flat_map(|p| &p.runs)
        .filter(|r| r.hyperlink_source.is_some())
        .count()
}

#[test]
fn a_deleted_link_unlinks_its_text_and_undo_and_redo_hold() {
    let mut m = model();
    let id = link(&mut m);
    assert_eq!(tagged_runs(&m), 1);
    let destinations = m.scene().designmap.hyperlink_destinations.len();

    m.apply_mutation(&Mutation::DeleteHyperlink {
        hyperlink_id: id.clone(),
    })
    .expect("delete");
    assert!(find(&m, &id).is_none());
    assert_eq!(tagged_runs(&m), 0);
    assert_eq!(
        m.scene().designmap.hyperlink_destinations.len(),
        destinations - 1,
        "the link's own URL destination goes with it"
    );

    m.undo().expect("undo");
    let back = find(&m, &id).expect("undo puts it back");
    assert_eq!(back.destination_url.as_deref(), Some("https://paged.media"));
    assert_eq!(tagged_runs(&m), 1);

    m.redo().expect("redo");
    assert!(find(&m, &id).is_none());

    assert!(
        m.apply_mutation(&Mutation::DeleteHyperlink {
            hyperlink_id: "Hyperlink/none".into(),
        })
        .is_err(),
        "an unknown link is refused"
    );
}

#[test]
fn an_edited_link_goes_somewhere_new_and_undoes() {
    let mut m = model();
    let id = link(&mut m);
    let own = find(&m, &id).unwrap().destination;

    // A new URL: the link's own destination changes in place.
    m.apply_mutation(&Mutation::EditHyperlink {
        hyperlink_id: id.clone(),
        name: Some("Docs".into()),
        url: Some("https://docs.paged.media".into()),
        page: None,
    })
    .expect("edit url");
    let h = find(&m, &id).unwrap();
    assert_eq!(h.name, "Docs");
    assert_eq!(h.destination, own);
    assert_eq!(
        h.destination_url.as_deref(),
        Some("https://docs.paged.media")
    );

    // A page: a new destination; the URL one it leaves goes.
    let page = m.pages()[1].self_id.clone();
    let destinations = m.scene().designmap.hyperlink_destinations.len();
    m.apply_mutation(&Mutation::EditHyperlink {
        hyperlink_id: id.clone(),
        name: None,
        url: None,
        page: Some(PageId(page.clone())),
    })
    .expect("edit page");
    let h = find(&m, &id).unwrap();
    assert_ne!(h.destination, own);
    assert_eq!(h.destination_page.as_deref(), Some(page.as_str()));
    assert_eq!(h.name, "Docs", "a name left out is unchanged");
    assert_eq!(
        m.scene().designmap.hyperlink_destinations.len(),
        destinations
    );

    m.undo().expect("undo page");
    let h = find(&m, &id).unwrap();
    assert_eq!(h.destination, own);
    assert_eq!(
        h.destination_url.as_deref(),
        Some("https://docs.paged.media")
    );
    m.undo().expect("undo url");
    let h = find(&m, &id).unwrap();
    assert_eq!(h.destination_url.as_deref(), Some("https://paged.media"));
    assert_ne!(h.name, "Docs");

    assert!(
        m.apply_mutation(&Mutation::EditHyperlink {
            hyperlink_id: id,
            name: None,
            url: None,
            page: Some(PageId("no-such-page".into())),
        })
        .is_err(),
        "a page the document lacks is refused"
    );
}

/// Saved, reopened (the link now InDesign's spelling in the package),
/// edited and deleted, saved again: the second package carries the edit.
#[test]
fn links_edited_and_deleted_after_a_reload_save_as_idml() {
    let mut m = model();
    let keep = link(&mut m);
    m.apply_mutation(&Mutation::InsertHyperlink {
        story_id: story(),
        start: 10,
        end: 14,
        url: "https://example.com".into(),
        page: None,
    })
    .expect("second link");
    let gone = m.hyperlinks().last().unwrap().self_id.clone();
    let saved = m.export_idml().expect("export");

    let mut m = CanvasModel::load("doc", &saved, options()).expect("reload");
    m.apply_mutation(&Mutation::EditHyperlink {
        hyperlink_id: keep.clone(),
        name: Some("Docs".into()),
        url: Some("https://docs.paged.media".into()),
        page: None,
    })
    .expect("edit");
    m.apply_mutation(&Mutation::DeleteHyperlink {
        hyperlink_id: gone.clone(),
    })
    .expect("delete");
    let resaved = m.export_idml().expect("export again");

    let doc = idml_import::import_idml_doc(&resaved).expect("re-import");
    let dm = &doc.designmap;
    assert!(dm.hyperlinks.iter().all(|h| h.self_id != gone));
    let h = dm
        .hyperlinks
        .iter()
        .find(|h| h.self_id == keep)
        .expect("the kept link");
    assert_eq!(h.name.as_deref(), Some("Docs"));
    let dest = dm
        .hyperlink_destinations
        .iter()
        .find(|d| Some(&d.self_id) == h.destination.as_ref())
        .expect("its destination");
    assert!(matches!(
        &dest.kind,
        paged_model::HyperlinkDestinationKind::Url(u) if u == "https://docs.paged.media"
    ));
    assert_eq!(
        dm.hyperlink_destinations.len(),
        1,
        "the deleted link's went"
    );
}
