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

// ── §11 (RFI C-75): undo of a delete restores the node ──────────────

use paged_mutate::operation::PathAnchorSpec;
use paged_mutate::{PropertyPath, Value};

/// A value of the row's own type that differs from what it holds.
/// `None` for a row with no settable counterpart of that shape.
fn changed(path: PropertyPath, current: &Value) -> Option<Value> {
    Some(match current {
        Value::Bool(b) => Value::Bool(!b),
        Value::Length(v) => Value::Length(Some(v.unwrap_or(0.0) + 7.5)),
        Value::ColorRef(_) => Value::ColorRef(Some("Color/Black".to_string())),
        Value::Lengths(_) => Value::Lengths(vec![6.0, 3.0]),
        Value::Transform(t) => {
            let mut m = t.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
            m[4] += 11.0;
            m[5] += 13.0;
            Value::Transform(Some(m))
        }
        // An enumeration: the token each path accepts.
        Value::Text(_) => Value::Text(
            match path {
                PropertyPath::FrameBlendMode => "Multiply",
                PropertyPath::FrameStrokeAlignment => "InsideAlignment",
                PropertyPath::FrameStrokeJoin => "RoundEndJoin",
                PropertyPath::FrameStrokeEndCap => "RoundEndCap",
                PropertyPath::FrameStrokeType => "StrokeStyle/$ID/Dashed",
                PropertyPath::FrameStrokeStartArrowhead | PropertyPath::FrameStrokeEndArrowhead => {
                    "TriangleArrowHead"
                }
                p if format!("{p:?}").starts_with("FrameCornerOption") => "RoundedCorner",
                p if format!("{p:?}").ends_with("BlendMode") => "Overlay",
                _ => return None,
            }
            .to_string(),
        ),
        _ => return None,
    })
}

/// The page item as the MODEL holds it, whole.
fn node(m: &CanvasModel, id: &ElementId) -> serde_json::Value {
    let raw = id.raw_id();
    for parsed in &m.scene().spreads {
        let s = &parsed.spread;
        let found = match id {
            ElementId::TextFrame(_) => s
                .text_frames
                .iter()
                .find(|f| f.self_id.as_deref() == Some(raw))
                .map(serde_json::to_value),
            ElementId::Rectangle(_) => s
                .rectangles
                .iter()
                .find(|f| f.self_id.as_deref() == Some(raw))
                .map(serde_json::to_value),
            ElementId::Oval(_) => s
                .ovals
                .iter()
                .find(|f| f.self_id.as_deref() == Some(raw))
                .map(serde_json::to_value),
            ElementId::GraphicLine(_) => s
                .graphic_lines
                .iter()
                .find(|f| f.self_id.as_deref() == Some(raw))
                .map(serde_json::to_value),
            ElementId::Polygon(_) => s
                .polygons
                .iter()
                .find(|f| f.self_id.as_deref() == Some(raw))
                .map(serde_json::to_value),
            other => panic!("{other:?} is not a leaf page item"),
        };
        if let Some(v) = found {
            return v.expect("a model node serialises");
        }
    }
    serde_json::Value::Null
}

/// One of each leaf kind, minted the way its tool mints it.
fn one_of_each(m: &mut CanvasModel) -> Vec<(&'static str, ElementId)> {
    let page_id = page(m);
    let corner = |x: f32, y: f32| PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    };
    let inserts = vec![
        (
            "Rectangle",
            Mutation::InsertFrame {
                page_id: page_id.clone(),
                bounds: (100.0, 100.0, 180.0, 220.0),
            },
        ),
        (
            "Oval",
            Mutation::InsertOval {
                page_id: page_id.clone(),
                bounds: (100.0, 260.0, 180.0, 380.0),
            },
        ),
        (
            "Polygon",
            Mutation::InsertPath {
                page_id: page_id.clone(),
                anchors: vec![
                    corner(100.0, 220.0),
                    corner(220.0, 220.0),
                    corner(220.0, 300.0),
                    corner(100.0, 300.0),
                ],
                open: false,
                smooth: false,
            },
        ),
        (
            "GraphicLine",
            Mutation::InsertLine {
                page_id: page_id.clone(),
                start: (260.0, 220.0),
                end: (380.0, 300.0),
            },
        ),
        (
            "TextFrame",
            Mutation::InsertTextFrame {
                page_id,
                bounds: (340.0, 100.0, 420.0, 220.0),
            },
        ),
    ];
    inserts
        .into_iter()
        .map(|(kind, mutation)| {
            let id = m
                .apply_mutation(&mutation)
                .unwrap_or_else(|e| panic!("insert {kind}: {e:?}"))
                .created_id
                .expect("created id");
            (kind, id)
        })
        .collect()
}

/// EVERY property the descriptor reads is written with a changed value
/// (the roster is the descriptor's own, not a list kept here), then the
/// item is deleted and the delete undone. The whole node — the model's
/// struct, as one value — and every row of the descriptor must be what
/// they were.
#[test]
fn delete_then_undo_restores_every_settable_property_of_every_kind() {
    let mut m = model();
    for (kind, id) in one_of_each(&mut m) {
        let rows = m.element_properties(&id).expect("props").entries;
        let mut written = Vec::new();
        for row in &rows {
            let Some(current) = &row.value else { continue };
            // The box and the plugin carrier are not "formatting"; the
            // geometry is covered by the transform row.
            if matches!(
                row.path,
                PropertyPath::FrameBounds | PropertyPath::PluginMetadata
            ) {
                continue;
            }
            let Some(value) = changed(row.path, current) else {
                continue;
            };
            if m.apply_mutation(&Mutation::SetElementProperty {
                element_id: id.clone(),
                path: row.path,
                value,
            })
            .is_ok()
            {
                written.push(row.path);
            }
        }
        assert!(
            written.len() >= 10,
            "{kind}: only {} of {} rows took a write — the test is not dressing the node: {written:?}",
            written.len(),
            rows.len()
        );
        if matches!(
            id,
            ElementId::Rectangle(_) | ElementId::Oval(_) | ElementId::Polygon(_)
        ) {
            // A placed image: the one loss the editor could detect.
            m.apply_mutation(&Mutation::ReplaceImageBytes {
                element_id: raw(&id),
                bytes: Some(paged_canvas::channel::ByteBuf(vec![
                    0x89, b'P', b'N', b'G', 1, 2, 3, 4,
                ])),
            })
            .unwrap_or_else(|e| panic!("{kind}: image bytes: {e:?}"));
        }

        let before_node = node(&m, &id);
        let before_rows = m.element_properties(&id).expect("props").entries;
        let tree_before = tree(&m, &[&id]);

        m.apply_mutation(&Mutation::DeleteFrame { frame_id: raw(&id) })
            .unwrap_or_else(|e| panic!("{kind}: delete: {e:?}"));
        assert!(m.element_properties(&id).is_none(), "{kind}: deleted");
        assert!(m.undo().is_some(), "{kind}: undo");

        let after_node = node(&m, &id);
        let lost: Vec<String> = before_node
            .as_object()
            .expect("a node is an object")
            .iter()
            .filter(|(k, v)| after_node.get(k.as_str()) != Some(*v))
            .map(|(k, v)| {
                format!(
                    "{k}: {v} → {}",
                    after_node
                        .get(k.as_str())
                        .unwrap_or(&serde_json::Value::Null)
                )
            })
            .collect();
        assert!(
            lost.is_empty(),
            "{kind}: undo of the delete lost these fields ({} rows had been written):\n  {}",
            written.len(),
            lost.join("\n  ")
        );
        assert_eq!(after_node, before_node, "{kind}: the whole node");
        let after_rows = m.element_properties(&id).expect("props").entries;
        for (b, a) in before_rows.iter().zip(&after_rows) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.value, b.value, "{kind}: {:?} after undo", b.path);
        }
        assert_eq!(
            after_rows.len(),
            before_rows.len(),
            "{kind}: descriptor rows"
        );
        assert_eq!(
            tree(&m, &[&id]),
            tree_before,
            "{kind}: its place in the tree"
        );
    }
}
