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

//! v70 — `CreateMaster` / `DeleteMaster` / `RenameMaster`: masters made,
//! copied, named and removed over the wire, each one undo step.

use std::collections::HashSet;

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_wire::{Mutation, PageId};

fn model() -> CanvasModel {
    let sample = paged_gen::samples::build("masters").expect("masters sample");
    let bytes = paged_gen::write_idml(&sample).expect("write");
    CanvasModel::load("mc", &bytes, CanvasOptions::default()).expect("load")
}

fn master_ids(m: &CanvasModel) -> Vec<String> {
    let mut ids: Vec<_> = m.scene().master_spreads.keys().cloned().collect();
    ids.sort();
    ids
}

fn applied(m: &CanvasModel) -> (PageId, String) {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.pages.iter())
        .find_map(|p| {
            let master = p.applied_master.as_deref()?.rsplit('/').next()?.to_string();
            Some((PageId(p.self_id.clone()?), master))
        })
        .expect("a page applies a master")
}

/// Every `Self` id on a spread: pages and items.
fn ids_of(spread: &paged_model::Spread) -> HashSet<String> {
    let mut out: HashSet<String> = spread
        .pages
        .iter()
        .filter_map(|p| p.self_id.clone())
        .collect();
    out.extend(spread.rectangles.iter().filter_map(|r| r.self_id.clone()));
    out.extend(spread.text_frames.iter().filter_map(|f| f.self_id.clone()));
    out
}

#[test]
fn a_new_master_has_one_page_of_the_given_size_and_undoes() {
    let mut m = model();
    let before = master_ids(&m);
    m.apply_mutation(&Mutation::CreateMaster {
        master: "uNew".into(),
        name: Some("Title Slide".into()),
        width_pt: 960.0,
        height_pt: 540.0,
        duplicate_of: None,
    })
    .expect("create");
    let ms = &m.scene().master_spreads["uNew"];
    assert_eq!(ms.name.as_deref(), Some("Title Slide"));
    assert_eq!(ms.spread.pages.len(), 1);
    let b = &ms.spread.pages[0].bounds;
    assert_eq!((b.right - b.left, b.bottom - b.top), (960.0, 540.0));
    let label = m
        .master_pages()
        .into_iter()
        .find(|s| s.self_id == "uNew")
        .expect("listed")
        .label;
    assert_eq!(label, "Title Slide");

    // Its page is addressable: applying it to a page works.
    let (page, _) = applied(&m);
    m.apply_mutation(&Mutation::ApplyMasterToPage {
        page,
        master: Some("MasterSpread/uNew".into()),
    })
    .expect("apply the new master");

    m.undo().expect("undo apply");
    m.undo().expect("undo create");
    assert_eq!(master_ids(&m), before);
    m.redo().expect("redo create");
    assert!(m.scene().master_spreads.contains_key("uNew"));
}

#[test]
fn a_duplicated_master_copies_items_with_fresh_ids() {
    let mut m = model();
    let (_, src) = applied(&m);
    m.apply_mutation(&Mutation::CreateMaster {
        master: "uCopy".into(),
        name: None,
        width_pt: 0.0,
        height_pt: 0.0,
        duplicate_of: Some(format!("MasterSpread/{src}")),
    })
    .expect("duplicate");
    let a = &m.scene().master_spreads[&src].spread;
    let b = &m.scene().master_spreads["uCopy"].spread;
    assert_eq!(a.pages.len(), b.pages.len());
    assert_eq!(a.rectangles.len(), b.rectangles.len());
    assert!(
        !b.rectangles.is_empty(),
        "the masters sample's master has items"
    );
    let (ia, ib) = (ids_of(a), ids_of(b));
    assert!(
        ia.is_disjoint(&ib),
        "copied ids collide: {:?}",
        ia.intersection(&ib)
    );

    // Redo recreates the same ids, so later redone ops still find them.
    let first = ids_of(b);
    m.undo().expect("undo");
    m.redo().expect("redo");
    assert_eq!(ids_of(&m.scene().master_spreads["uCopy"].spread), first);
}

#[test]
fn delete_is_refused_while_applied_and_restores_exactly() {
    let mut m = model();
    let (page, src) = applied(&m);
    assert!(
        m.apply_mutation(&Mutation::DeleteMaster {
            master: src.clone()
        })
        .is_err(),
        "a master a page applies must not be deleted"
    );
    // Detach every page from it, then it goes.
    let pages: Vec<PageId> = m
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.pages.iter())
        .filter(|p| {
            p.applied_master
                .as_deref()
                .and_then(|a| a.rsplit('/').next())
                == Some(&src)
        })
        .filter_map(|p| p.self_id.clone().map(PageId))
        .collect();
    assert!(pages.contains(&page));
    for p in pages {
        m.apply_mutation(&Mutation::ApplyMasterToPage {
            page: p,
            master: None,
        })
        .expect("detach");
    }
    let before = ids_of(&m.scene().master_spreads[&src].spread);
    m.apply_mutation(&Mutation::DeleteMaster {
        master: src.clone(),
    })
    .expect("delete");
    assert!(!m.scene().master_spreads.contains_key(&src));
    m.undo().expect("undo delete");
    assert_eq!(ids_of(&m.scene().master_spreads[&src].spread), before);
}

#[test]
fn rename_sets_and_undo_restores_the_name() {
    let mut m = model();
    let id = master_ids(&m)[0].clone();
    let prev = m.scene().master_spreads[&id].name.clone();
    m.apply_mutation(&Mutation::RenameMaster {
        master: id.clone(),
        name: Some("Section Header".into()),
    })
    .expect("rename");
    assert_eq!(
        m.scene().master_spreads[&id].name.as_deref(),
        Some("Section Header")
    );
    m.undo().expect("undo");
    assert_eq!(m.scene().master_spreads[&id].name, prev);
    assert!(m
        .apply_mutation(&Mutation::RenameMaster {
            master: "uNoSuch".into(),
            name: None,
        })
        .is_err());
}
