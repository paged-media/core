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

//! `deleteFrame` on the WIRE, as a host sees it: the scene tree after
//! the delete and after the undo. The kernel halves live in
//! `paged-mutate/tests/remove_node_*.rs`; the documents here are the
//! editor's own reproductions (`engine-findings.md` §10–§12), read back
//! the way the editor reads them — the scene tree, by id.

use paged_canvas::{
    channel::{Mutation, SceneTreeNode},
    element_selection::ElementId,
    CanvasModel, CanvasOptions, PageId,
};

fn model() -> CanvasModel {
    let sample = paged_gen::samples::build("geometry").expect("geometry sample");
    let bytes = paged_gen::write_idml(&sample).expect("write idml");
    CanvasModel::load("delete", &bytes, CanvasOptions::default()).expect("load")
}

fn page(m: &CanvasModel) -> PageId {
    PageId(m.pages()[0].self_id.clone())
}

/// Insert a rectangle; returns its id.
fn frame(m: &mut CanvasModel, x: f32) -> ElementId {
    let page_id = page(m);
    m.apply_mutation(&Mutation::InsertFrame {
        page_id,
        bounds: (100.0, x, 160.0, x + 40.0),
    })
    .expect("insert frame")
    .created_id
    .expect("created id")
}

fn raw(id: &ElementId) -> String {
    id.raw_id().to_string()
}

/// The scene tree under the first page as `id` / `id[child, …]` text,
/// for the ids in `of` (and anything nested under them).
fn tree(m: &CanvasModel, of: &[&ElementId]) -> Vec<String> {
    fn label(n: &SceneTreeNode) -> String {
        let id =
            n.id.as_ref()
                .map(|i| i.raw_id().to_string())
                .unwrap_or_else(|| n.kind.clone());
        if n.children.is_empty() {
            id
        } else {
            format!(
                "{id}[{}]",
                n.children.iter().map(label).collect::<Vec<_>>().join(", ")
            )
        }
    }
    fn walk(n: &SceneTreeNode, of: &[String], out: &mut Vec<String>) {
        match &n.id {
            Some(id) if of.contains(&id.raw_id().to_string()) => out.push(label(n)),
            _ => n.children.iter().for_each(|c| walk(c, of, out)),
        }
    }
    let of: Vec<String> = of.iter().map(|i| raw(i)).collect();
    let mut out = Vec::new();
    for n in &m.scene_tree() {
        walk(n, &of, &mut out);
    }
    out
}

/// §10 (RFI C-74) — four rectangles, `group [u2, u3]`, delete `u1`.
/// It used to read `group[u3, u4], u4`.
#[test]
fn deleting_an_earlier_frame_does_not_reseat_a_group() {
    let mut m = model();
    let u: Vec<ElementId> = (0..4)
        .map(|i| frame(&mut m, 100.0 + 60.0 * i as f32))
        .collect();
    let g = m
        .apply_mutation(&Mutation::CreateGroup {
            member_ids: vec![u[1].clone(), u[2].clone()],
        })
        .expect("group")
        .created_id
        .expect("group id");
    let all = [&u[0], &g, &u[3]];
    let before = tree(&m, &all);
    assert_eq!(
        before,
        vec![
            raw(&u[0]),
            format!("{}[{}, {}]", raw(&g), raw(&u[1]), raw(&u[2])),
            raw(&u[3])
        ]
    );

    m.apply_mutation(&Mutation::DeleteFrame {
        frame_id: raw(&u[0]),
    })
    .expect("delete u1");
    assert_eq!(
        tree(&m, &all),
        vec![
            format!("{}[{}, {}]", raw(&g), raw(&u[1]), raw(&u[2])),
            raw(&u[3])
        ],
        "the group still holds u2 and u3, and u4 is listed once"
    );
    assert!(m.undo().is_some());
    assert_eq!(tree(&m, &all), before, "undo restores the tree");
}

/// §10, second half — the deleted frame IS a member. It leaves the
/// group, and undo re-seats it there (it used to come back as a second
/// top-level entry).
#[test]
fn deleting_a_member_and_undoing_puts_it_back_in_its_group() {
    let mut m = model();
    let u: Vec<ElementId> = (0..3)
        .map(|i| frame(&mut m, 100.0 + 60.0 * i as f32))
        .collect();
    let g = m
        .apply_mutation(&Mutation::CreateGroup {
            member_ids: vec![u[0].clone(), u[1].clone()],
        })
        .expect("group")
        .created_id
        .expect("group id");
    let all = [&g, &u[2], &u[0]];
    let before = tree(&m, &all);
    assert_eq!(
        before,
        vec![
            format!("{}[{}, {}]", raw(&g), raw(&u[0]), raw(&u[1])),
            raw(&u[2])
        ]
    );

    m.apply_mutation(&Mutation::DeleteFrame {
        frame_id: raw(&u[0]),
    })
    .expect("delete a member");
    assert_eq!(
        tree(&m, &all),
        vec![format!("{}[{}]", raw(&g), raw(&u[1])), raw(&u[2])],
        "the member is gone from its group; nothing else moved"
    );
    assert!(m.undo().is_some());
    assert_eq!(
        tree(&m, &all),
        before,
        "undo puts the member back INSIDE the group, first as before"
    );
    assert!(m.redo().is_some());
    assert_eq!(
        tree(&m, &all),
        vec![format!("{}[{}]", raw(&g), raw(&u[1])), raw(&u[2])]
    );
}
