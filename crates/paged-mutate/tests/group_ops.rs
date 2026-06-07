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

//! B-04 (protocol v32) — CreateGroup/DissolveGroup INV suite
//! (decision 7's conditions): atomicity (validation before any
//! mutation), exact undo restoration (bytewise `frames_in_order` +
//! `groups`, via the inverse's `restore_slots` snapshot), and Z-ORDER
//! STABILITY — the keystone: for members CONTIGUOUS in z-order the
//! `build_document` display-list command stream is IDENTICAL before
//! and after grouping; scattered members deterministically collect at
//! the earliest member's paint slot (the InDesign semantic) and undo
//! still restores the original order exactly.

use std::path::PathBuf;

use paged_mutate::{apply, GroupSpec, NodeId, Operation};
use paged_parse::FrameRef;
use paged_renderer::pipeline::{build_document, PipelineOptions};
use paged_scene::Document;

fn fixture_bytes() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry-groups.idml");
    std::fs::read(path).expect("read geometry-groups fixture")
}

/// Resolve a leaf FrameRef to a NodeId within a spread.
fn node_for(spread: &paged_parse::Spread, r: FrameRef) -> Option<NodeId> {
    Some(match r {
        FrameRef::TextFrame(i) => NodeId::TextFrame(spread.text_frames.get(i)?.self_id.clone()?),
        FrameRef::Rectangle(i) => NodeId::Rectangle(spread.rectangles.get(i)?.self_id.clone()?),
        FrameRef::Oval(i) => NodeId::Oval(spread.ovals.get(i)?.self_id.clone()?),
        FrameRef::GraphicLine(i) => {
            NodeId::GraphicLine(spread.graphic_lines.get(i)?.self_id.clone()?)
        }
        FrameRef::Polygon(i) => NodeId::Polygon(spread.polygons.get(i)?.self_id.clone()?),
        FrameRef::Group(_) => return None,
    })
}

/// Two UNGROUPED leaf items from the first spread that has them, plus
/// the spread index — chosen from `frames_in_order` so they're
/// guaranteed top-level. NOT necessarily adjacent in z-order.
fn two_ungrouped_leaves(doc: &Document) -> (usize, Vec<NodeId>) {
    for (si, parsed) in doc.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        let leaves: Vec<NodeId> = spread
            .frames_in_order
            .iter()
            .filter_map(|r| node_for(spread, *r))
            .take(2)
            .collect();
        if leaves.len() == 2 {
            return (si, leaves);
        }
    }
    panic!("fixture has no spread with two ungrouped leaf items");
}

/// Two leaves at CONSECUTIVE `frames_in_order` slots — the
/// paint-neutral grouping case.
fn adjacent_leaf_pair(doc: &Document) -> (usize, Vec<NodeId>) {
    for (si, parsed) in doc.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        for w in spread.frames_in_order.windows(2) {
            if let (Some(a), Some(b)) = (node_for(spread, w[0]), node_for(spread, w[1])) {
                return (si, vec![a, b]);
            }
        }
    }
    panic!("fixture has no spread with two z-adjacent leaf items");
}

fn order_snapshot(doc: &Document, si: usize) -> (Vec<FrameRef>, usize) {
    let s = &doc.spreads[si].spread;
    (s.frames_in_order.clone(), s.groups.len())
}

fn command_stream_digest(doc: &Document) -> Vec<String> {
    let built = build_document(doc, &PipelineOptions::default()).expect("build");
    built
        .pages
        .iter()
        .flat_map(|p| p.list.commands.iter().map(|c| format!("{c:?}")))
        .collect()
}

#[test]
fn create_group_is_z_order_neutral_and_round_trips() {
    let mut doc = Document::open(&fixture_bytes()).expect("open");
    let (si, members) = adjacent_leaf_pair(&doc);
    let before_order = order_snapshot(&doc, si);
    let before_paint = command_stream_digest(&doc);

    // CREATE — minted id is echoed in the applied op.
    let applied = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: members.clone(),
            },
        },
    )
    .expect("create group");
    let group_id = match &applied.op {
        Operation::CreateGroup { spec } => spec.self_id.clone().expect("minted id echoed"),
        other => panic!("unexpected echoed op: {other:?}"),
    };

    // Structure: one new group, members in document order, members
    // gone from frames_in_order, group ref present exactly once.
    let spread = &doc.spreads[si].spread;
    assert_eq!(spread.groups.len(), before_order.1 + 1);
    let group = spread
        .groups
        .iter()
        .find(|g| g.self_id.as_deref() == Some(group_id.as_str()))
        .expect("group present");
    assert_eq!(group.members.len(), 2);
    let group_refs: Vec<_> = spread
        .frames_in_order
        .iter()
        .filter(|r| matches!(r, FrameRef::Group(_)))
        .collect();
    assert_eq!(
        group_refs.len(),
        before_order
            .0
            .iter()
            .filter(|r| matches!(r, FrameRef::Group(_)))
            .count()
            + 1
    );
    for m in &group.members {
        assert!(
            !spread.frames_in_order.contains(m),
            "grouped member must leave frames_in_order"
        );
    }

    // KEYSTONE — paint stream identical.
    assert_eq!(
        command_stream_digest(&doc),
        before_paint,
        "grouping must not change the display-list command stream"
    );

    // UNDO — bytewise restore of frames_in_order + groups count.
    let undone = apply(&mut doc, &applied.inverse).expect("dissolve (undo)");
    assert_eq!(
        order_snapshot(&doc, si),
        before_order,
        "undo restores order"
    );
    assert_eq!(command_stream_digest(&doc), before_paint);

    // REDO — same id, same structure.
    apply(&mut doc, &undone.inverse).expect("redo");
    let spread = &doc.spreads[si].spread;
    assert!(spread
        .groups
        .iter()
        .any(|g| g.self_id.as_deref() == Some(group_id.as_str())));
    assert_eq!(command_stream_digest(&doc), before_paint);
}

#[test]
fn scattered_members_collect_deterministically_and_undo_is_exact() {
    let mut doc = Document::open(&fixture_bytes()).expect("open");
    // First spread whose two leaves are NOT adjacent (something sits
    // between them in z-order).
    let (si, members) = doc
        .spreads
        .iter()
        .enumerate()
        .find_map(|(si, parsed)| {
            let spread = &parsed.spread;
            let slots: Vec<usize> = spread
                .frames_in_order
                .iter()
                .enumerate()
                .filter(|(_, r)| node_for(spread, **r).is_some())
                .map(|(i, _)| i)
                .collect();
            (slots.len() >= 2 && slots[1] > slots[0] + 1).then(|| {
                (
                    si,
                    vec![
                        node_for(spread, spread.frames_in_order[slots[0]]).unwrap(),
                        node_for(spread, spread.frames_in_order[slots[1]]).unwrap(),
                    ],
                )
            })
        })
        .expect("fixture has a spread with scattered leaves");
    let before_order = order_snapshot(&doc, si);
    let before_paint = command_stream_digest(&doc);

    let applied = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: members.clone(),
            },
        },
    )
    .expect("create group over scattered members");

    // Deterministic collect: the group ref sits at the FIRST member's
    // original slot; both members left frames_in_order.
    let spread = &doc.spreads[si].spread;
    let first_slot = before_order
        .0
        .iter()
        .position(|r| node_for(spread, *r).is_some())
        .unwrap();
    assert!(
        matches!(spread.frames_in_order[first_slot], FrameRef::Group(_)),
        "group ref takes the earliest member's slot"
    );
    assert_eq!(
        spread.frames_in_order.len(),
        before_order.0.len() - 1,
        "two members out, one group ref in"
    );

    // Exact undo — restore_slots puts members back at their ORIGINAL
    // (non-contiguous) indices; paint stream returns bytewise.
    apply(&mut doc, &applied.inverse).expect("dissolve (undo)");
    assert_eq!(
        order_snapshot(&doc, si),
        before_order,
        "undo restores scattered z-order exactly"
    );
    assert_eq!(command_stream_digest(&doc), before_paint);
}

#[test]
fn dissolve_parsed_group_round_trips_with_index_fixup() {
    let mut doc = Document::open(&fixture_bytes()).expect("open");
    // First spread with a TOP-LEVEL flat group (members all leaves).
    let mut target: Option<(usize, String)> = None;
    'outer: for (si, parsed) in doc.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        for (gi, g) in spread.groups.iter().enumerate() {
            let top_level = spread.frames_in_order.contains(&FrameRef::Group(gi));
            let flat = g.members.iter().all(|m| !matches!(m, FrameRef::Group(_)));
            let not_nested = !spread
                .groups
                .iter()
                .any(|other| other.members.contains(&FrameRef::Group(gi)));
            if top_level && flat && not_nested {
                if let Some(id) = g.self_id.clone() {
                    target = Some((si, id));
                    break 'outer;
                }
            }
        }
    }
    let Some((si, group_id)) = target else {
        eprintln!("fixture has no top-level flat group — skipping");
        return;
    };
    let before_order = order_snapshot(&doc, si);
    let before_paint = command_stream_digest(&doc);

    let applied = apply(
        &mut doc,
        &Operation::DissolveGroup {
            group_id: group_id.clone(),
            restore_slots: None,
        },
    )
    .expect("dissolve parsed group");
    // Members back in frames_in_order; group gone; paint unchanged.
    let spread = &doc.spreads[si].spread;
    assert!(spread
        .groups
        .iter()
        .all(|g| g.self_id.as_deref() != Some(group_id.as_str())));
    assert_eq!(command_stream_digest(&doc), before_paint);

    // Inverse recreates bytewise (incl. FrameRef::Group index fix-up
    // correctness for any remaining groups).
    apply(&mut doc, &applied.inverse).expect("recreate (undo)");
    assert_eq!(order_snapshot(&doc, si), before_order);
    assert_eq!(command_stream_digest(&doc), before_paint);
}

#[test]
fn atomic_rejections_leave_the_document_untouched() {
    let mut doc = Document::open(&fixture_bytes()).expect("open");
    let (si, members) = two_ungrouped_leaves(&doc);
    let before = order_snapshot(&doc, si);

    // Unknown member.
    let err = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: vec![members[0].clone(), NodeId::Rectangle("uNOPE".into())],
            },
        },
    );
    assert!(err.is_err());
    assert_eq!(order_snapshot(&doc, si), before);

    // Duplicate member.
    let err = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: vec![members[0].clone(), members[0].clone()],
            },
        },
    );
    assert!(err.is_err());
    assert_eq!(order_snapshot(&doc, si), before);

    // Group as member (flat v1).
    let err = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: vec![members[0].clone(), NodeId::Group("uG".into())],
            },
        },
    );
    assert!(err.is_err());
    assert_eq!(order_snapshot(&doc, si), before);

    // Already-grouped member: group the pair, then try to group one
    // of them again.
    let applied = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: members.clone(),
            },
        },
    )
    .expect("first group");
    let err = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: vec![members[0].clone()],
            },
        },
    );
    assert!(err.is_err(), "already-grouped member must reject");
    apply(&mut doc, &applied.inverse).expect("cleanup");
    assert_eq!(order_snapshot(&doc, si), before);
}
