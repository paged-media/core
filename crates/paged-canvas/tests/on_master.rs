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

//! v70 — `OnMaster`: a master's items edited through the ordinary
//! mutations, repainting every page that uses the master, undone in one
//! step; page operations refused inside it.

use paged_canvas::{render_snapshot_png, CanvasModel, CanvasOptions, ElementId};
use paged_mutate::{PropertyPath, SwatchSpec, Value};
use paged_wire::{Mutation, PageId};

fn model() -> CanvasModel {
    let sample = paged_gen::samples::build("masters").expect("masters sample");
    let bytes = paged_gen::write_idml(&sample).expect("write");
    CanvasModel::load("om", &bytes, CanvasOptions::default()).expect("load")
}

/// A master with a rectangle, and that rectangle's id.
fn master_rect(m: &CanvasModel) -> (String, String) {
    let mut masters: Vec<_> = m.scene().master_spreads.iter().collect();
    masters.sort_by_key(|(k, _)| k.as_str());
    masters
        .into_iter()
        .find_map(|(id, ms)| {
            ms.spread
                .rectangles
                .first()
                .and_then(|r| r.self_id.clone())
                .map(|r| (id.clone(), r))
        })
        .expect("the masters sample has a master rectangle")
}

/// The first page that applies `master`.
fn page_of(m: &CanvasModel, master: &str) -> PageId {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.pages.iter())
        .find(|p| {
            p.applied_master
                .as_deref()
                .is_some_and(|a| a.rsplit('/').next() == Some(master))
        })
        .and_then(|p| p.self_id.clone())
        .map(PageId)
        .expect("a page applies the master")
}

fn png(m: &CanvasModel, page: &PageId) -> Vec<u8> {
    render_snapshot_png(m, page, 200)
        .expect("snapshot")
        .png_bytes
}

#[test]
fn a_master_item_is_edited_like_a_page_item() {
    let mut m = model();
    let (master, rect) = master_rect(&m);
    let page = page_of(&m, &master);
    // A swatch nothing paints with yet, so the recolour is visible.
    let swatch = "Color/on-master".to_string();
    m.apply_mutation(&Mutation::CreateSwatch {
        spec: SwatchSpec {
            self_id: Some(swatch.clone()),
            name: Some("On Master".into()),
            space: "RGB".into(),
            value: vec![240.0, 20.0, 140.0],
            model: None,
            alternate_space: None,
            alternate_value: Vec::new(),
            tint: None,
            alpha: None,
        },
    })
    .expect("mint a swatch");
    let before = png(&m, &page);

    m.apply_mutation(&Mutation::OnMaster {
        master: master.clone(),
        mutation: Box::new(Mutation::SetElementProperty {
            element_id: ElementId::Rectangle(rect.clone()),
            path: PropertyPath::FrameFillColor,
            value: Value::ColorRef(Some(swatch.clone())),
        }),
    })
    .expect("recolour the master rectangle");
    let fill = |m: &CanvasModel| {
        m.scene().master_spreads[&master]
            .spread
            .rectangles
            .iter()
            .find(|r| r.self_id.as_deref() == Some(rect.as_str()))
            .and_then(|r| r.fill_color.clone())
    };
    assert_eq!(fill(&m).as_deref(), Some(swatch.as_str()));
    assert_ne!(png(&m, &page), before, "the page shows its master's change");

    m.undo().expect("undo");
    assert_ne!(fill(&m).as_deref(), Some(swatch.as_str()));
    assert_eq!(png(&m, &page), before);

    // Deleting a master item, and undoing it.
    m.apply_mutation(&Mutation::OnMaster {
        master: master.clone(),
        mutation: Box::new(Mutation::DeleteFrame {
            frame_id: rect.clone(),
        }),
    })
    .expect("delete the master rectangle");
    assert!(m.scene().master_spreads[&master]
        .spread
        .rectangles
        .iter()
        .all(|r| r.self_id.as_deref() != Some(rect.as_str())));
    m.undo().expect("undo delete");
    assert_eq!(png(&m, &page), before);
}

#[test]
fn page_operations_are_refused_inside_a_master() {
    let mut m = model();
    let (master, _) = master_rect(&m);
    let page = page_of(&m, &master);
    assert!(m
        .apply_mutation(&Mutation::OnMaster {
            master: master.clone(),
            mutation: Box::new(Mutation::DuplicatePage { page }),
        })
        .is_err());
    assert!(m
        .apply_mutation(&Mutation::OnMaster {
            master: "nope".into(),
            mutation: Box::new(Mutation::DeleteFrame {
                frame_id: "x".into(),
            }),
        })
        .is_err());
}
