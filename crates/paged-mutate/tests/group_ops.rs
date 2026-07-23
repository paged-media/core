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

use paged_model::FrameRef;
use paged_mutate::{apply, GroupSpec, NodeId, Operation};
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
fn node_for(spread: &paged_model::Spread, r: FrameRef) -> Option<NodeId> {
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

/// The `(tx, ty)` translations of every FillPath emitted for `page_idx`
/// — a coordinate-level read of where the renderer actually paints, so
/// a group-transform's effect on member geometry is provable, not just
/// "the digest changed".
fn fill_translations(doc: &Document, page_idx: usize) -> Vec<(f32, f32)> {
    use paged_compose::DisplayCommand;
    let built = build_document(doc, &PipelineOptions::default()).expect("build");
    built.pages[page_idx]
        .list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. }
            | DisplayCommand::FillPathBlend { transform, .. } => {
                Some((transform.0[4], transform.0[5]))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn create_group_is_z_order_neutral_and_round_trips() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
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
                parent: None,
                item_transform: None,
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
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
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
                parent: None,
                item_transform: None,
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
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
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
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    let (si, members) = two_ungrouped_leaves(&doc);
    let before = order_snapshot(&doc, si);

    // Unknown member.
    let err = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: vec![members[0].clone(), NodeId::Rectangle("uNOPE".into())],
                parent: None,
                item_transform: None,
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
                parent: None,
                item_transform: None,
            },
        },
    );
    assert!(err.is_err());
    assert_eq!(order_snapshot(&doc, si), before);

    // Unknown group as member (v2 allows group members, but this id
    // resolves to nothing → still rejects, document untouched).
    let err = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: vec![members[0].clone(), NodeId::Group("uNOGROUP".into())],
                parent: None,
                item_transform: None,
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
                parent: None,
                item_transform: None,
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
                parent: None,
                item_transform: None,
            },
        },
    );
    assert!(err.is_err(), "already-grouped member must reject");
    apply(&mut doc, &applied.inverse).expect("cleanup");
    assert_eq!(order_snapshot(&doc, si), before);
}

// ---------------------------------------------------------------------------
// W1.20 — groups v2: nested create, group transforms, nested dissolve.
// ---------------------------------------------------------------------------

fn close6(a: [f32; 6], b: [f32; 6]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-3)
}

/// Effective `item_transform` of a leaf NodeId on a spread.
fn leaf_transform(spread: &paged_model::Spread, node: &NodeId) -> Option<[f32; 6]> {
    let id = node.self_id();
    match node {
        NodeId::TextFrame(_) => spread
            .text_frames
            .iter()
            .find(|f| f.self_id.as_deref() == Some(id))
            .map(|f| f.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])),
        NodeId::Rectangle(_) => spread
            .rectangles
            .iter()
            .find(|f| f.self_id.as_deref() == Some(id))
            .map(|f| f.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])),
        NodeId::Oval(_) => spread
            .ovals
            .iter()
            .find(|f| f.self_id.as_deref() == Some(id))
            .map(|f| f.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])),
        NodeId::Polygon(_) => spread
            .polygons
            .iter()
            .find(|f| f.self_id.as_deref() == Some(id))
            .map(|f| f.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])),
        NodeId::GraphicLine(_) => spread
            .graphic_lines
            .iter()
            .find(|f| f.self_id.as_deref() == Some(id))
            .map(|f| f.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])),
        _ => None,
    }
}

/// Find a spread holding an EXISTING top-level group plus at least one
/// top-level leaf, so we can nest the group-of-groups (group an
/// existing `Group` with a leaf). Returns `(spread, existing_group_id,
/// a_top_level_leaf)`.
fn group_plus_leaf(doc: &Document) -> Option<(usize, String, NodeId)> {
    for (si, parsed) in doc.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        let group_id = spread.frames_in_order.iter().find_map(|r| match r {
            FrameRef::Group(gi) => spread.groups.get(*gi).and_then(|g| g.self_id.clone()),
            _ => None,
        });
        let leaf = spread
            .frames_in_order
            .iter()
            .find_map(|r| node_for(spread, *r));
        if let (Some(gid), Some(leaf)) = (group_id, leaf) {
            return Some((si, gid, leaf));
        }
    }
    None
}

#[test]
fn nested_create_round_trips_and_restores_prior_structure() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    // NESTED create: group an EXISTING top-level group G1 together with
    // a top-level leaf into a new outer group G2 (group-of-groups).
    let Some((si, g1_id, leaf)) = group_plus_leaf(&doc) else {
        eprintln!("fixture has no top-level group + leaf spread — skipping");
        return;
    };

    let before_order = order_snapshot(&doc, si);
    let before_paint = command_stream_digest(&doc);

    let g2 = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                // A `Group` id as a member — the v2 nesting case.
                members: vec![NodeId::Group(g1_id.clone()), leaf.clone()],
                parent: None,
                item_transform: None,
            },
        },
    )
    .expect("create nested group-of-groups");
    let g2_id = match &g2.op {
        Operation::CreateGroup { spec } => spec.self_id.clone().unwrap(),
        _ => unreachable!(),
    };

    // Structure: G2 exists and nests G1 as a `FrameRef::Group` member;
    // G1 left the spread root, G2 took its place.
    {
        let spread = &doc.spreads[si].spread;
        let g2_idx = spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(g2_id.as_str()))
            .expect("G2 present");
        let g1_idx = spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(g1_id.as_str()))
            .expect("G1 still present");
        assert!(
            spread.groups[g2_idx]
                .members
                .contains(&FrameRef::Group(g1_idx)),
            "G2 must nest G1 as a member"
        );
        assert!(
            !spread.frames_in_order.contains(&FrameRef::Group(g1_idx)),
            "nested G1 must leave the spread root"
        );
        assert!(spread.frames_in_order.contains(&FrameRef::Group(g2_idx)));
    }

    // KEYSTONE — grouping is paint-neutral (the members' effective
    // transforms are unchanged; only the wrapper structure changed).
    assert_eq!(
        command_stream_digest(&doc),
        before_paint,
        "nested grouping must not change the rendered paint stream"
    );

    // Undo (DissolveGroup of G2) restores the exact prior structure,
    // re-surfacing G1 at the spread root at its original slot.
    let undone = apply(&mut doc, &g2.inverse).expect("undo G2");
    assert_eq!(
        order_snapshot(&doc, si),
        before_order,
        "undo of the nested group restores the exact prior structure"
    );
    assert_eq!(command_stream_digest(&doc), before_paint);

    // Redo (the undo's own inverse re-creates G2 with the same id +
    // nested structure).
    apply(&mut doc, &undone.inverse).expect("redo G2");
    {
        let spread = &doc.spreads[si].spread;
        let g2_idx = spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(g2_id.as_str()))
            .expect("G2 re-created on redo");
        let g1_idx = spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(g1_id.as_str()))
            .expect("G1 present after redo");
        assert!(
            spread.groups[g2_idx]
                .members
                .contains(&FrameRef::Group(g1_idx)),
            "redo re-nests G1 inside G2"
        );
    }
    assert_eq!(command_stream_digest(&doc), before_paint);
}

#[test]
fn group_transform_moves_members_hit_and_render_agree_and_undo_restores() {
    use paged_mutate::path_math::affine_multiply;

    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    // Group two adjacent leaves so we own a group with a known member
    // set, then transform the GROUP and verify each member's effective
    // transform composed the delta.
    let (si, members) = adjacent_leaf_pair(&doc);
    let created = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: members.clone(),
                parent: None,
                item_transform: None,
            },
        },
    )
    .expect("create group");
    let group_id = match &created.op {
        Operation::CreateGroup { spec } => spec.self_id.clone().unwrap(),
        _ => unreachable!(),
    };

    // Member effective transforms before the group move.
    let before: Vec<[f32; 6]> = members
        .iter()
        .map(|m| leaf_transform(&doc.spreads[si].spread, m).expect("leaf"))
        .collect();
    let before_paint = command_stream_digest(&doc);

    // Move the group: translate by (40, 25) (group's own prev = None ⇒
    // identity, so delta == this matrix).
    let g_new = [1.0, 0.0, 0.0, 1.0, 40.0, 25.0];
    let moved = apply(
        &mut doc,
        &Operation::SetGroupTransform {
            group: group_id.clone(),
            transform: Some(g_new),
            prev: None,
        },
    )
    .expect("set group transform");

    // Each member's EFFECTIVE transform = delta * old (delta == g_new
    // since prev was identity).
    for (m, old) in members.iter().zip(&before) {
        let now = leaf_transform(&doc.spreads[si].spread, m).expect("leaf");
        let expected = affine_multiply(g_new, *old);
        assert!(
            close6(now, expected),
            "member {m:?} effective transform must compose the group delta\n got {now:?}\n want {expected:?}"
        );
    }
    // The group's own transform was set.
    {
        let spread = &doc.spreads[si].spread;
        let g = spread
            .groups
            .iter()
            .find(|g| g.self_id.as_deref() == Some(group_id.as_str()))
            .unwrap();
        assert!(close6(
            g.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
            g_new
        ));
    }
    // Paint changed (the members moved on the page).
    assert_ne!(
        command_stream_digest(&doc),
        before_paint,
        "moving the group must change the rendered geometry"
    );

    // Undo restores every member's effective transform + the paint.
    apply(&mut doc, &moved.inverse).expect("undo group transform");
    for (m, old) in members.iter().zip(&before) {
        let now = leaf_transform(&doc.spreads[si].spread, m).expect("leaf");
        assert!(
            close6(now, *old),
            "undo must restore member {m:?} effective transform"
        );
    }
    assert_eq!(
        command_stream_digest(&doc),
        before_paint,
        "undo of the group transform restores the paint stream"
    );

    // Cleanup: dissolve the group we created.
    apply(&mut doc, &created.inverse).expect("cleanup dissolve");
}

#[test]
fn nested_dissolve_splices_into_parent_geometry_invariant() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    // Find a NESTED parsed group: a group G that is a member of some
    // parent group P. The geometry-groups fixture's variant 4 builds a
    // 3-deep outer→middle→inner stack, so such a group exists.
    let mut target: Option<(usize, String, String)> = None; // (spread, inner_id, parent_id)
    'outer: for (si, parsed) in doc.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        for (pi, p) in spread.groups.iter().enumerate() {
            for m in &p.members {
                if let FrameRef::Group(gi) = *m {
                    if let (Some(inner_id), Some(parent_id)) =
                        (spread.groups[gi].self_id.clone(), p.self_id.clone())
                    {
                        target = Some((si, inner_id, parent_id));
                        break 'outer;
                    }
                }
            }
            let _ = pi;
        }
    }
    let Some((si, inner_id, parent_id)) = target else {
        eprintln!("fixture has no nested group — skipping");
        return;
    };

    // KEYSTONE: the rendered paint stream is the invariant. Dissolving a
    // nested group only removes the wrapper; members keep their pre-baked
    // EFFECTIVE transforms, so geometry must be byte-identical.
    let before_paint = command_stream_digest(&doc);

    // Capture the inner group's members + the parent's member-slot.
    let (inner_members, parent_slot_before): (Vec<FrameRef>, usize) = {
        let spread = &doc.spreads[si].spread;
        let gi = spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(inner_id.as_str()))
            .unwrap();
        let pi = spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(parent_id.as_str()))
            .unwrap();
        let slot = spread.groups[pi]
            .members
            .iter()
            .position(|r| *r == FrameRef::Group(gi))
            .unwrap();
        (spread.groups[gi].members.clone(), slot)
    };

    let dissolved = apply(
        &mut doc,
        &Operation::DissolveGroup {
            group_id: inner_id.clone(),
            restore_slots: None,
        },
    )
    .expect("dissolve nested group");

    // The inner group is gone; its members are now direct members of the
    // parent at the inner group's former slot.
    {
        let spread = &doc.spreads[si].spread;
        assert!(
            spread
                .groups
                .iter()
                .all(|g| g.self_id.as_deref() != Some(inner_id.as_str())),
            "inner group removed"
        );
        let pi = spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(parent_id.as_str()))
            .expect("parent still present");
        // The spliced members occupy `inner_members.len()` consecutive
        // slots starting at the inner group's former position.
        for (k, _) in inner_members.iter().enumerate() {
            assert!(
                parent_slot_before + k < spread.groups[pi].members.len(),
                "spliced member slot in range"
            );
        }
    }

    // GEOMETRY INVARIANT: the rendered output is unchanged.
    assert_eq!(
        command_stream_digest(&doc),
        before_paint,
        "dissolving a nested group must not change the rendered geometry"
    );

    // Undo re-nests the inner group inside the parent at the same slot,
    // and the paint returns bytewise.
    apply(&mut doc, &dissolved.inverse).expect("undo nested dissolve");
    {
        let spread = &doc.spreads[si].spread;
        let pi = spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(parent_id.as_str()))
            .expect("parent present after undo");
        let gi = spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(inner_id.as_str()))
            .expect("inner group restored after undo");
        let slot = spread.groups[pi]
            .members
            .iter()
            .position(|r| *r == FrameRef::Group(gi))
            .expect("inner group re-nested in parent");
        assert_eq!(
            slot, parent_slot_before,
            "re-nest restores the inner group at its original parent slot"
        );
    }
    assert_eq!(
        command_stream_digest(&doc),
        before_paint,
        "undo of the nested dissolve restores the paint stream"
    );
}

#[test]
fn renderer_paints_group_members_at_the_composed_transform() {
    // Renderer-level proof (extends the geometry-groups sample): apply a
    // SetGroupTransform to a PARSED group and confirm every FillPath the
    // renderer emits for that page shifts by exactly the group delta —
    // i.e. the members paint at the composed transform, and undo paints
    // them back.
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    // Pick the first spread whose top-level item is a single group with
    // leaf members (variant 1 — "identity · two-rects").
    let (si, group_id) = doc
        .spreads
        .iter()
        .enumerate()
        .find_map(|(si, parsed)| {
            let spread = &parsed.spread;
            let only_group = spread.frames_in_order.iter().find_map(|r| match r {
                FrameRef::Group(gi) => {
                    let g = &spread.groups[*gi];
                    let flat_leaves = !g.members.is_empty()
                        && g.members.iter().all(|m| !matches!(m, FrameRef::Group(_)));
                    // Any flat top-level group works — the pure-translate
                    // delta left-multiplies onto whatever prior transform
                    // the parser baked in, so the member fills shift by
                    // exactly (dx, dy) regardless of the group's own
                    // starting transform.
                    flat_leaves.then(|| g.self_id.clone()).flatten()
                }
                _ => None,
            });
            only_group.map(|gid| (si, gid))
        })
        .expect("fixture has a flat top-level group");

    let before = fill_translations(&doc, si);
    assert!(
        before.len() >= 2,
        "the variant page paints at least the two group member rects"
    );

    // Read the group's prior own transform so we can predict where each
    // member lands: `SetGroupTransform` writes the ABSOLUTE group
    // transform, so each member's effective transform becomes
    // `g_new * inv(g_old) * effective_old`. For these identity-local
    // members that means they move RIGIDLY by `g_new_translate -
    // g_old_translate`, preserving the inter-member spacing.
    let g_old = doc.spreads[si]
        .spread
        .groups
        .iter()
        .find(|g| g.self_id.as_deref() == Some(group_id.as_str()))
        .and_then(|g| g.item_transform)
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let g_new = [1.0, 0.0, 0.0, 1.0, 37.0, -19.0];
    let shift = (g_new[4] - g_old[4], g_new[5] - g_old[5]);

    let applied = apply(
        &mut doc,
        &Operation::SetGroupTransform {
            group: group_id.clone(),
            transform: Some(g_new),
            prev: None,
        },
    )
    .expect("set group transform");

    let after = fill_translations(&doc, si);
    assert_eq!(
        after.len(),
        before.len(),
        "the same number of fills paint (no member dropped)"
    );
    // At least the two group member rects paint shifted RIGIDLY by the
    // group's translation change (the lone label TextFrame doesn't move).
    let moved = before
        .iter()
        .zip(&after)
        .filter(|((bx, by), (ax, ay))| {
            (ax - bx - shift.0).abs() < 1e-2 && (ay - by - shift.1).abs() < 1e-2
        })
        .count();
    assert!(
        moved >= 2,
        "at least the two group member rects must paint shifted by the group's \
         translation change {shift:?} (before={before:?}, after={after:?})"
    );

    // Undo restores the original paint translations bytewise.
    apply(&mut doc, &applied.inverse).expect("undo group transform");
    assert_eq!(
        fill_translations(&doc, si),
        before,
        "undo repaints the members at their original positions"
    );
}

/// Regression — grouping must work on a spread whose `frames_in_order`
/// z-table is EMPTY, the state a synthesised blank document is in after
/// building it up via `InsertNode` (`register_frame_ref` no-ops on an
/// empty table, so it never materialises). Pre-fix, `CreateGroup` here
/// failed with "member is not a top-level spread item"; the op now
/// materialises the table from the kind vecs first.
#[test]
fn create_group_on_empty_frames_in_order_materialises_and_succeeds() {
    let mut doc = idml_import::import_idml_doc(&fixture_bytes()).expect("open");
    // Capture members BEFORE clearing — the helper reads frames_in_order.
    let (si, members) = two_ungrouped_leaves(&doc);
    let groups_before = doc.spreads[si].spread.groups.len();

    // Drop the z-table to mimic the never-materialised (blank-doc) state.
    doc.spreads[si].spread.frames_in_order.clear();

    let applied = apply(
        &mut doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: members.clone(),
                parent: None,
                item_transform: None,
            },
        },
    )
    .expect("create group on an empty-frames_in_order spread");
    let group_id = match &applied.op {
        Operation::CreateGroup { spec } => spec.self_id.clone().expect("minted id"),
        other => panic!("unexpected echoed op: {other:?}"),
    };

    let spread = &doc.spreads[si].spread;
    assert!(
        !spread.frames_in_order.is_empty(),
        "the op materialised the z-table"
    );
    assert_eq!(spread.groups.len(), groups_before + 1);
    let group = spread
        .groups
        .iter()
        .find(|g| g.self_id.as_deref() == Some(group_id.as_str()))
        .expect("group present");
    assert_eq!(group.members.len(), members.len());
    for m in &group.members {
        assert!(
            !spread.frames_in_order.contains(m),
            "grouped member must leave frames_in_order"
        );
    }

    // Dissolve (undo) round-trips the group count back.
    apply(&mut doc, &applied.inverse).expect("dissolve (undo)");
    assert_eq!(doc.spreads[si].spread.groups.len(), groups_before);
}
