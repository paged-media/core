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

//! Bookmarks created, renamed, moved and deleted over the wire (ADR 173),
//! as InDesign 20.0.1 does it (`bookmarks-probe`), on `navigation` (two
//! bookmarks to text destinations): a bookmark to a page goes through a
//! page destination of its own; a child nests under its parent; deleting
//! a parent takes its children and the page destinations nothing else
//! goes to and keeps a text destination; every op undoes exactly; the
//! tree saves as IDML and is the PDF's outline.

use paged_canvas::channel::ExportPdfWireOptions;
use paged_canvas::export::CanvasExportSession;
use paged_canvas::{CanvasModel, CanvasOptions, Mutation, PageId};

fn model() -> CanvasModel {
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("Inter");
    let idml = paged_gen::write_idml(&paged_gen::samples::navigation::build()).expect("navigation");
    CanvasModel::load(
        "nav",
        &idml,
        CanvasOptions {
            fonts: vec![font],
            ..CanvasOptions::default()
        },
    )
    .expect("load")
}

/// `(name, parent's name)` of every bookmark, in document order.
fn tree(m: &CanvasModel) -> Vec<(String, Option<String>)> {
    let all = m.bookmarks();
    all.iter()
        .map(|b| {
            let parent = b.parent.as_ref().map(|p| {
                all.iter()
                    .find(|x| &x.self_id == p)
                    .map(|x| x.name.clone())
                    .unwrap_or_default()
            });
            (b.name.clone(), parent)
        })
        .collect()
}

fn named(m: &CanvasModel, name: &str) -> String {
    m.bookmarks()
        .into_iter()
        .find(|b| b.name == name)
        .unwrap_or_else(|| panic!("no bookmark {name}"))
        .self_id
}

fn page(m: &CanvasModel, i: usize) -> PageId {
    PageId(m.pages()[i].self_id.clone())
}

fn page_destinations(m: &CanvasModel) -> usize {
    m.scene()
        .designmap
        .hyperlink_destinations
        .iter()
        .filter(|d| matches!(d.kind, paged_model::HyperlinkDestinationKind::Page(_)))
        .count()
}

fn create(m: &mut CanvasModel, name: &str, page: Option<PageId>, parent: Option<String>) {
    m.apply_mutation(&Mutation::CreateBookmark {
        name: name.into(),
        page,
        destination: None,
        parent,
        index: None,
    })
    .expect("create");
}

fn s(v: &str) -> String {
    v.to_string()
}

#[test]
fn a_page_bookmark_goes_through_a_page_destination_of_its_own() {
    let mut m = model();
    let before = tree(&m);
    assert_eq!(
        before,
        [(s("Introduction"), None), (s("Further reading"), None)]
    );
    let pages_before = page_destinations(&m);
    let p = page(&m, 0);
    create(&mut m, "Page one", Some(p.clone()), None);
    assert_eq!(tree(&m).last(), Some(&(s("Page one"), None)));
    assert_eq!(page_destinations(&m), pages_before + 1);
    let b = m.bookmarks().into_iter().last().unwrap();
    let dest = m
        .scene()
        .designmap
        .hyperlink_destinations
        .iter()
        .find(|d| d.self_id == b.destination)
        .expect("its destination");
    assert!(
        matches!(&dest.kind, paged_model::HyperlinkDestinationKind::Page(id) if *id == p.0),
        "{:?}",
        dest.kind
    );
    m.undo().expect("undo");
    assert_eq!(tree(&m), before);
    assert_eq!(page_destinations(&m), pages_before);
}

#[test]
fn a_child_nests_and_a_move_carries_it_with_its_parent() {
    let mut m = model();
    let intro = named(&m, "Introduction");
    let p = page(&m, 0);
    create(
        &mut m,
        "Under the introduction",
        Some(p.clone()),
        Some(intro.clone()),
    );
    assert_eq!(
        tree(&m),
        [
            (s("Introduction"), None),
            (s("Under the introduction"), Some(s("Introduction"))),
            (s("Further reading"), None)
        ]
    );
    // The introduction, with its child, after "Further reading".
    m.apply_mutation(&Mutation::MoveBookmark {
        bookmark_id: intro.clone(),
        parent: None,
        index: None,
    })
    .expect("move");
    assert_eq!(
        tree(&m),
        [
            (s("Further reading"), None),
            (s("Introduction"), None),
            (s("Under the introduction"), Some(s("Introduction")))
        ]
    );
    // Then under "Further reading", first.
    m.apply_mutation(&Mutation::MoveBookmark {
        bookmark_id: intro.clone(),
        parent: Some(named(&m, "Further reading")),
        index: Some(0),
    })
    .expect("move under");
    assert_eq!(
        tree(&m),
        [
            (s("Further reading"), None),
            (s("Introduction"), Some(s("Further reading"))),
            (s("Under the introduction"), Some(s("Introduction")))
        ]
    );
    // A bookmark cannot move under itself.
    assert!(m
        .apply_mutation(&Mutation::MoveBookmark {
            bookmark_id: intro.clone(),
            parent: Some(named(&m, "Under the introduction")),
            index: None,
        })
        .is_err());
    m.undo().expect("undo");
    m.undo().expect("undo");
    assert_eq!(
        tree(&m),
        [
            (s("Introduction"), None),
            (s("Under the introduction"), Some(s("Introduction"))),
            (s("Further reading"), None)
        ]
    );
}

#[test]
fn a_rename_undoes() {
    let mut m = model();
    let id = named(&m, "Further reading");
    m.apply_mutation(&Mutation::RenameBookmark {
        bookmark_id: id,
        name: "Reading list".into(),
    })
    .expect("rename");
    assert_eq!(tree(&m)[1].0, "Reading list");
    m.undo().expect("undo");
    assert_eq!(tree(&m)[1].0, "Further reading");
}

#[test]
fn deleting_a_parent_takes_its_children_and_their_page_destinations() {
    let mut m = model();
    let intro = named(&m, "Introduction");
    let text_destination = m.bookmarks()[0].destination.clone();
    let pages_before = page_destinations(&m);
    let p = page(&m, 0);
    create(&mut m, "Child", Some(p), Some(intro.clone()));
    assert_eq!(page_destinations(&m), pages_before + 1);
    m.apply_mutation(&Mutation::DeleteBookmark { bookmark_id: intro })
        .expect("delete");
    assert_eq!(tree(&m), [(s("Further reading"), None)]);
    // The child's page destination went with it; the introduction's text
    // destination stays, as InDesign keeps it.
    assert_eq!(page_destinations(&m), pages_before);
    assert!(m
        .scene()
        .designmap
        .hyperlink_destinations
        .iter()
        .any(|d| d.self_id == text_destination));
    m.undo().expect("undo");
    assert_eq!(
        tree(&m),
        [
            (s("Introduction"), None),
            (s("Child"), Some(s("Introduction"))),
            (s("Further reading"), None)
        ]
    );
    assert_eq!(page_destinations(&m), pages_before + 1);
}

#[test]
fn the_tree_saves_as_idml_and_is_the_pdfs_outline() {
    let mut m = model();
    let intro = named(&m, "Introduction");
    let p = page(&m, 0);
    create(&mut m, "Page one", Some(p), Some(intro));
    m.apply_mutation(&Mutation::RenameBookmark {
        bookmark_id: named(&m, "Further reading"),
        name: "Reading list".into(),
    })
    .expect("rename");
    let bytes = m.export_idml().expect("export");
    let back = idml_import::import_idml_doc(&bytes).expect("re-import");
    let dm = &back.designmap;
    let saved: Vec<(Option<&str>, Option<&str>)> = dm
        .bookmarks
        .iter()
        .map(|b| {
            let parent = dm.bookmark_parents.get(&b.self_id).and_then(|p| {
                dm.bookmarks
                    .iter()
                    .find(|x| &x.self_id == p)
                    .and_then(|x| x.name.as_deref())
            });
            (b.name.as_deref(), parent)
        })
        .collect();
    assert_eq!(
        saved,
        [
            (Some("Introduction"), None),
            (Some("Page one"), Some("Introduction")),
            (Some("Reading list"), None)
        ]
    );
    // The PDF's outline is the tree.
    let wire = ExportPdfWireOptions {
        bookmarks: true,
        ..ExportPdfWireOptions::default()
    };
    let (mut session, pages) = CanvasExportSession::begin(&m, &wire).expect("begin");
    for _ in 0..pages {
        session.export_next_page().expect("page");
    }
    let pdf = session.finish().expect("finish").pdf_bytes;
    let text = String::from_utf8_lossy(&pdf);
    for title in ["(Introduction)", "(Page one)", "(Reading list)"] {
        assert!(text.contains(&format!("/Title {title}")), "{title}");
    }
}

/// The edited tree saved for InDesign to read back
/// (`PROBE_IN=<path> bash tools/indesign-export/bookmarks-probe.sh`):
/// `PAGED_BOOKMARKS_IDML=<path> cargo test -p paged-canvas --test
/// bookmark_ops -- --ignored`.
#[test]
#[ignore]
fn writes_the_saved_tree_for_indesign() {
    let path = std::env::var("PAGED_BOOKMARKS_IDML").expect("PAGED_BOOKMARKS_IDML");
    let mut m = model();
    let intro = named(&m, "Introduction");
    let p = page(&m, 0);
    create(&mut m, "Page one", Some(p), Some(intro));
    m.apply_mutation(&Mutation::RenameBookmark {
        bookmark_id: named(&m, "Further reading"),
        name: "Reading list".into(),
    })
    .expect("rename");
    std::fs::write(path, m.export_idml().expect("export")).expect("write");
}
