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

use super::*;
use paged_model::FrameRef;
use paged_scene::Document;

use crate::error::OperationError;
use crate::invert::invert_remove_node;
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, NodeSpec, Operation, PropertyPath,
};

// ---------------------------------------------------------------------------
// RemoveNode
// ---------------------------------------------------------------------------

pub(super) fn apply_remove_node(
    doc: &mut Document,
    node: &NodeId,
) -> Result<AppliedOperation, OperationError> {
    // B-18: a nested (pasted-into) child can't be removed directly —
    // its RemoveNode inverse would restore it TOP-LEVEL, breaking the
    // mutate-then-undo identity invariant. Release it first.
    if super::nested::is_nested_child(doc, node) {
        return Err(OperationError::InvalidValue {
            node: node.clone(),
            path: crate::operation::PropertyPath::FrameTransform,
            reason: "B-18: the item is pasted into a container — release it before removing"
                .to_string(),
        });
    }
    // C-28: same argument for a mask item — its RemoveNode inverse
    // would restore it TOP-LEVEL and leave the target pointing at an
    // id that is now an ordinary visible object.
    if super::opacity_mask::is_mask_item(doc, node) {
        return Err(OperationError::InvalidValue {
            node: node.clone(),
            path: crate::operation::PropertyPath::FrameTransform,
            reason: "C-28: the item is serving as an opacity mask — release it before removing"
                .to_string(),
        });
    }
    // C-28: deleting a MASKED item would strand its mask artwork
    // outside `frames_in_order` — invisible and unreachable. Release
    // the mask first so the artwork returns to the z-table.
    if super::opacity_mask::is_masked_target(doc, node) {
        return Err(OperationError::InvalidValue {
            node: node.clone(),
            path: crate::operation::PropertyPath::FrameTransform,
            reason: "C-28: the item carries an opacity mask — release it before removing"
                .to_string(),
        });
    }
    // C-76: a CONTAINER takes what was pasted into it along, the way
    // InDesign deletes a frame's pasted-in content with the frame. It
    // used to leave the children in their kind vecs under a
    // `nested_children` entry whose host was gone — the editor saw them
    // come back as free top-level items, and undo restored the
    // container beside them with the nesting lost.
    //
    // Done as the batch the user could have sent: each child released
    // and removed (last first, so each release captures the index its
    // undo pastes it back at), then the container. The batch's inverse
    // re-inserts the container, then each child and pastes it back in,
    // in order — the nesting and every z slot come back exactly. A
    // child that is itself a container recurses through this same arm.
    let pasted_in = pasted_in_children(doc, node);
    if !pasted_in.is_empty() {
        let mut ops = Vec::with_capacity(2 * pasted_in.len() + 1);
        for child in pasted_in.into_iter().rev() {
            ops.push(Operation::ReleaseFrom {
                child: child.clone(),
                restore_slot: None,
            });
            ops.push(Operation::RemoveNode { node: child });
        }
        ops.push(Operation::RemoveNode { node: node.clone() });
        let applied = super::apply_inner(doc, &Operation::Batch { ops })?;
        return Ok(AppliedOperation {
            op: Operation::RemoveNode { node: node.clone() },
            inverse: applied.inverse,
            invalidation: InvalidationHint {
                structural: true,
                ..Default::default()
            },
        });
    }
    let (parent, position, captured, z_slot) = remove_and_capture(doc, node)?;
    let inverse = invert_remove_node(parent, position, captured, z_slot);
    Ok(AppliedOperation {
        op: Operation::RemoveNode { node: node.clone() },
        inverse,
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// The items pasted into `node` (B-18), in their stored order; empty
/// when it hosts none.
fn pasted_in_children(doc: &Document, node: &NodeId) -> Vec<NodeId> {
    let id = node.self_id();
    for parsed in &doc.spreads {
        let spread = &parsed.spread;
        if super::nested::leaf_ref_in_spread(spread, node).is_none() {
            continue;
        }
        return spread
            .nested_children
            .get(id)
            .map(|refs| {
                refs.iter()
                    .filter_map(|r| super::layer::node_for_frame_ref(spread, *r))
                    .collect()
            })
            .unwrap_or_default();
    }
    Vec::new()
}

/// Take a just-removed item's ref out of whichever list named it, and
/// say where it was — as the `(parent, slot)` an `InsertNode` puts it
/// back with. The spread and its z slot for a top-level item; C-74: the
/// GROUP and the member slot for a group's member, so undo re-seats it
/// in its group. (A member of an id-less group cannot be addressed that
/// way and falls back to the spread, on top.)
///
/// C-84: when the item was its group's LAST member the group goes too
/// (and so does every ancestor that leaves empty); the third value is
/// those groups, captured whole for the item's own capture to carry.
fn release_home(
    parsed: &mut paged_scene::ParsedSpread,
    r: FrameRef,
) -> (NodeId, Option<usize>, Vec<serde_json::Value>) {
    let home = ref_home(&parsed.spread, r);
    let z_slot = unregister_frame_ref(&mut parsed.spread, r, fr_index(&r));
    if let Some(RefHome::Group(group_idx, slot)) = home {
        // The removed item is a leaf, so group indices have not moved.
        if let Some(gid) = parsed.spread.groups[group_idx].self_id.clone() {
            let emptied = take_emptied_groups(parsed, group_idx);
            return (NodeId::Group(gid), Some(slot), emptied);
        }
    }
    (spread_parent_id(parsed), z_slot, Vec::new())
}

/// C-84 — InDesign removes a group together with its last member, and a
/// group whose only member was that group with it (asked of InDesign:
/// `tools/indesign-export/object-integrity-probe.json`, 5a–5c — removing
/// members keeps a group of one; removing the last one removes the
/// group). The engine used to keep the EMPTY group: it painted nothing,
/// still sat in the z-order, and the plugin batch `remove a, remove b,
/// dissolve g` then applied with an inverse that cannot re-create an
/// empty group (`CreateGroup` needs a member), so its one undo failed
/// whole and the group and both members were gone for good.
///
/// Starting at `group_idx`, take every group left without members out of
/// the spread — the group, its ref in whichever list named it, and its
/// labels — and climb to its parent group. Each is captured WHOLE, with
/// the index it held in `spread.groups`, the group that held it (or none:
/// the spread) and its slot there, OUTERMOST FIRST: the order an undo
/// re-creates them in. A group without an id, or one whose parent has
/// none, cannot be put back by name and is left where it is.
pub(super) fn take_emptied_groups(
    parsed: &mut paged_scene::ParsedSpread,
    group_idx: usize,
) -> Vec<serde_json::Value> {
    let mut taken = Vec::new();
    let mut gi = group_idx;
    loop {
        let spread = &parsed.spread;
        let Some(group) = spread.groups.get(gi) else {
            break;
        };
        if !group.members.is_empty() {
            break;
        }
        let Some(gid) = group.self_id.clone() else {
            break;
        };
        let r = FrameRef::Group(gi);
        let home = ref_home(spread, r);
        let parent_group = match home {
            Some(RefHome::Group(pgi, slot)) => match spread.groups[pgi].self_id.clone() {
                Some(pid) => Some((pgi, pid, slot)),
                None => break,
            },
            _ => None,
        };
        let spread = &mut parsed.spread;
        let group = spread.groups.remove(gi);
        let z_slot = unregister_frame_ref(spread, r, gi);
        let labels = spread.labels.remove(&gid);
        let (parent, slot) = match &parent_group {
            Some((_, pid, slot)) => (Some(pid.clone()), Some(*slot)),
            None => (None, z_slot),
        };
        taken.push(serde_json::json!({
            "item": group,
            "index": gi,
            "parentGroup": parent,
            "slot": slot,
            "labels": labels,
        }));
        match parent_group {
            // The removed group's index sat below the parent's, so the
            // parent stepped down with every later group.
            Some((pgi, _, _)) => gi = if pgi > gi { pgi - 1 } else { pgi },
            None => break,
        }
    }
    taken.reverse();
    taken
}

/// C-75 — capture a just-removed page item WHOLE, as the
/// [`NodeSpec::Captured`] its inverse re-inserts.
///
/// `item` is the model struct itself, serialised as it stood — not a
/// list of the fields someone thought to copy, which is what made undo
/// of a delete restore a bare frame. The rows of the spread's side maps
/// keyed by the item's id go into the capture too and LEAVE the spread:
/// they used to stay behind, and since an id is minted as "highest in
/// the document + 1", the next item created after deleting the newest
/// one took the same id and with it the deleted item's plugin metadata.
///
/// `image_bytes` is carried beside the JSON (the caller has already
/// taken it out of `item`), so an inline image is not re-encoded as a
/// number array on every delete.
///
/// C-84: `emptied` — the groups the removal left without members (see
/// [`take_emptied_groups`]) — rides in the same capture, so re-inserting
/// it re-creates them around the item, and removing it again takes them
/// again: undo and redo are the same two ops at any depth.
fn capture<T: serde::Serialize>(
    parsed: &mut paged_scene::ParsedSpread,
    node: &NodeId,
    item: &T,
    image_bytes: Option<Vec<u8>>,
    emptied: Vec<serde_json::Value>,
) -> NodeSpec {
    let id = node.self_id();
    let spread = &mut parsed.spread;
    let mut envelope = serde_json::json!({
        "item": item,
        "labels": spread.labels.remove(id),
        "imageMetadata": spread.image_metadata.remove(id),
    });
    if !emptied.is_empty() {
        envelope["emptiedGroups"] = serde_json::Value::Array(emptied);
        envelope["spread"] = serde_json::json!(spread_parent_id(parsed).self_id());
    }
    NodeSpec::Captured {
        node: node.clone(),
        json: envelope.to_string(),
        image_bytes,
    }
}

/// C-84 — remove an EMPTY group (and every ancestor that leaves empty),
/// captured the way [`take_emptied_groups`] captures them. The inverse of
/// re-inserting such a capture, and what dissolving an empty group does;
/// a group that still holds members is refused — removing a member takes
/// it with its last one, and `DissolveGroup` releases them.
fn remove_empty_group(
    doc: &mut Document,
    node: &NodeId,
    gid: &str,
) -> Result<(NodeId, usize, NodeSpec, Option<usize>), OperationError> {
    for parsed in &mut doc.spreads {
        let Some(gi) = parsed
            .spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(gid))
        else {
            continue;
        };
        if !parsed.spread.groups[gi].members.is_empty() {
            return Err(OperationError::InvalidValue {
                node: node.clone(),
                path: PropertyPath::FrameTransform,
                reason: "a group goes with its last member — dissolve it, or delete its members"
                    .to_string(),
            });
        }
        let spread_id = spread_parent_id(parsed);
        let emptied = take_emptied_groups(parsed, gi);
        if emptied.is_empty() {
            return Err(OperationError::InvalidValue {
                node: node.clone(),
                path: PropertyPath::FrameTransform,
                reason: "the group, or the group holding it, has no id to restore it by"
                    .to_string(),
            });
        }
        let json = serde_json::json!({
            "emptiedGroups": emptied,
            "spread": spread_id.self_id(),
        })
        .to_string();
        let spec = NodeSpec::Captured {
            node: node.clone(),
            json,
            image_bytes: None,
        };
        return Ok((spread_id, gi, spec, None));
    }
    Err(OperationError::NodeNotFound(node.clone()))
}

/// Locate `node` in its containing spread, capture it whole
/// ([`capture`]), and remove it (including its entry in whichever list
/// named it). Returns `(parent_id, position, spec, z_slot)` for the
/// caller to feed into the inverse.
pub(super) fn remove_and_capture(
    doc: &mut Document,
    node: &NodeId,
) -> Result<(NodeId, usize, NodeSpec, Option<usize>), OperationError> {
    match node {
        NodeId::TextFrame(id) => {
            for parsed in &mut doc.spreads {
                if let Some(pos) = parsed
                    .spread
                    .text_frames
                    .iter()
                    .position(|f| f.self_id.as_deref() == Some(id.as_str()))
                {
                    let frame = parsed.spread.text_frames.remove(pos);
                    let (parent, z_slot, emptied) = release_home(parsed, FrameRef::TextFrame(pos));
                    let image_bytes = None;
                    let spec = capture(parsed, node, &frame, image_bytes, emptied);
                    return Ok((parent, pos, spec, z_slot));
                }
            }
            Err(OperationError::NodeNotFound(node.clone()))
        }
        NodeId::Rectangle(id) => {
            for parsed in &mut doc.spreads {
                if let Some(pos) = parsed
                    .spread
                    .rectangles
                    .iter()
                    .position(|r| r.self_id.as_deref() == Some(id.as_str()))
                {
                    let mut rect = parsed.spread.rectangles.remove(pos);
                    let (parent, z_slot, emptied) = release_home(parsed, FrameRef::Rectangle(pos));
                    let image_bytes = rect.image_bytes.take();
                    let spec = capture(parsed, node, &rect, image_bytes, emptied);
                    return Ok((parent, pos, spec, z_slot));
                }
            }
            Err(OperationError::NodeNotFound(node.clone()))
        }
        NodeId::Oval(id) => {
            for parsed in &mut doc.spreads {
                if let Some(pos) = parsed
                    .spread
                    .ovals
                    .iter()
                    .position(|o| o.self_id.as_deref() == Some(id.as_str()))
                {
                    let mut oval = parsed.spread.ovals.remove(pos);
                    let (parent, z_slot, emptied) = release_home(parsed, FrameRef::Oval(pos));
                    let image_bytes = oval.image_bytes.take();
                    let spec = capture(parsed, node, &oval, image_bytes, emptied);
                    return Ok((parent, pos, spec, z_slot));
                }
            }
            Err(OperationError::NodeNotFound(node.clone()))
        }
        NodeId::GraphicLine(id) => {
            for parsed in &mut doc.spreads {
                if let Some(pos) = parsed
                    .spread
                    .graphic_lines
                    .iter()
                    .position(|l| l.self_id.as_deref() == Some(id.as_str()))
                {
                    let line = parsed.spread.graphic_lines.remove(pos);
                    let (parent, z_slot, emptied) =
                        release_home(parsed, FrameRef::GraphicLine(pos));
                    let image_bytes = None;
                    let spec = capture(parsed, node, &line, image_bytes, emptied);
                    return Ok((parent, pos, spec, z_slot));
                }
            }
            Err(OperationError::NodeNotFound(node.clone()))
        }
        NodeId::Polygon(id) => {
            for parsed in &mut doc.spreads {
                if let Some(pos) = parsed
                    .spread
                    .polygons
                    .iter()
                    .position(|p| p.self_id.as_deref() == Some(id.as_str()))
                {
                    let mut poly = parsed.spread.polygons.remove(pos);
                    let (parent, z_slot, emptied) = release_home(parsed, FrameRef::Polygon(pos));
                    let image_bytes = poly.image_bytes.take();
                    let spec = capture(parsed, node, &poly, image_bytes, emptied);
                    return Ok((parent, pos, spec, z_slot));
                }
            }
            Err(OperationError::NodeNotFound(node.clone()))
        }
        // S-03 — remove a `<Table>` from its host story. Backs the undo
        // of an `InsertTable` (and a future table-delete op). Captures
        // the table's STRUCTURE into a `NodeSpec::Table` so the inverse
        // `InsertNode` re-creates the same shape (rows × cols, bands,
        // line sizing). Cell TEXT content is not round-tripped — empty
        // cells are minted on re-insert, matching the `NodeSpec`
        // "minimal supported field set" precedent (the same limitation
        // `TableCellSpec` documents for delete-row undo). The host
        // paragraph is dropped wholesale (a table paragraph carries
        // nothing but the table — see `apply_insert_table`).
        NodeId::Table { story_id, table_id } => {
            let Some((si, pi)) = find_table_pos(doc, story_id, table_id) else {
                return Err(OperationError::NodeNotFound(node.clone()));
            };
            // The host paragraph is captured WHOLE — every cell's text,
            // styling and spans — so the inverse puts back the table that
            // was there, at the paragraph it occupied. It used to hand back
            // a `NodeSpec::Table` of the bare grid, which re-inserted an
            // empty table at the END of the story: undoing a delete lost
            // every cell. The parent is the host story; `position` is the
            // paragraph index; z_slot is N/A for story content.
            let para = doc.stories[si].story.paragraphs.remove(pi);
            let json = serde_json::json!({ "paragraph": para }).to_string();
            let spec = NodeSpec::Captured {
                node: node.clone(),
                json,
                image_bytes: None,
            };
            Ok((NodeId::Story(story_id.clone()), pi, spec, None))
        }
        // C-84 — an EMPTY group only: what re-inserting an emptied
        // group's capture inverts to, and what dissolving an empty
        // group (a file can hold one) does.
        NodeId::Group(gid) => remove_empty_group(doc, node, gid),
        _ => Err(OperationError::UnsupportedProperty {
            node: node.clone(),
            path: PropertyPath::FrameBounds, // unused; signals "this node kind isn't removable yet"
        }),
    }
}
