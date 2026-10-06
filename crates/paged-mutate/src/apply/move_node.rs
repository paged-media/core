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
use paged_scene::Document;

use crate::error::OperationError;
use crate::invert::invert_move_node;
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, NodeSpec, Operation};

// ---------------------------------------------------------------------------
// MoveNode
// ---------------------------------------------------------------------------

pub(super) fn apply_move_node(
    doc: &mut Document,
    node: &NodeId,
    new_parent: &NodeId,
    position: usize,
) -> Result<AppliedOperation, OperationError> {
    let new_parent_id = match new_parent {
        NodeId::Spread(id) => id.clone(),
        _ => {
            return Err(OperationError::InvalidParent {
                parent: new_parent.clone(),
                child_kind: node.kind().to_string(),
            })
        }
    };

    // C-74: a group's member cannot be moved out from under its group.
    // The forward move would be well-defined (the member leaves the
    // group), but `MoveNode`'s inverse names a spread and a kind-vec
    // position and nothing else, so undo would bring the item back as a
    // top-level entry and the group would stay one member short — the
    // same reason a pasted-in child and a mask item are refused by
    // `RemoveNode`. Move the group, or ungroup first.
    let is_member = doc.spreads.iter().any(|p| {
        super::nested::leaf_ref_in_spread(&p.spread, node)
            .is_some_and(|r| p.spread.groups.iter().any(|g| g.members.contains(&r)))
    });
    if is_member {
        return Err(OperationError::InvalidValue {
            node: node.clone(),
            path: crate::operation::PropertyPath::FrameTransform,
            reason: "C-74: the item is a member of a group — move the group, or ungroup it first"
                .to_string(),
        });
    }

    // Capture before state by removing, then re-insert at the target.
    // If insertion fails, restore in place so the doc state is intact.
    let (previous_parent, previous_position, captured, previous_z_slot) =
        remove_and_capture(doc, node)?;

    // Read destination spread length without holding a borrow across
    // the potentially-rollback path.
    let target_len = match find_spread(doc, &new_parent_id) {
        Some(dest) => match &captured {
            NodeSpec::TextFrame { .. }
            | NodeSpec::Captured {
                node: NodeId::TextFrame(_),
                ..
            } => dest.spread.text_frames.len(),
            NodeSpec::Rectangle { .. }
            | NodeSpec::Captured {
                node: NodeId::Rectangle(_),
                ..
            } => dest.spread.rectangles.len(),
            NodeSpec::Oval { .. }
            | NodeSpec::Captured {
                node: NodeId::Oval(_),
                ..
            } => dest.spread.ovals.len(),
            NodeSpec::GraphicLine { .. }
            | NodeSpec::Captured {
                node: NodeId::GraphicLine(_),
                ..
            } => dest.spread.graphic_lines.len(),
            NodeSpec::Polygon { .. }
            | NodeSpec::Captured {
                node: NodeId::Polygon(_),
                ..
            } => dest.spread.polygons.len(),
            // A table is story content, never a page item (as below).
            NodeSpec::Captured {
                node: NodeId::Table { .. },
                ..
            } => {
                restore_capture(
                    doc,
                    &previous_parent,
                    previous_position,
                    captured,
                    previous_z_slot,
                );
                return Err(OperationError::InvalidParent {
                    parent: new_parent.clone(),
                    child_kind: "Table".to_string(),
                });
            }
            // A capture of anything else cannot exist — `RemoveNode`
            // captures leaf page items and tables only.
            NodeSpec::Captured { .. } => {
                restore_capture(
                    doc,
                    &previous_parent,
                    previous_position,
                    captured,
                    previous_z_slot,
                );
                return Err(OperationError::NodeNotFound(node.clone()));
            }
            // CloneTranslate is never captured from the doc — it's
            // an input-only spec for Phase H's Alt-duplicate. Treat
            // as a programmer error if it ever surfaces here.
            NodeSpec::CloneTranslate { .. } => {
                restore_capture(
                    doc,
                    &previous_parent,
                    previous_position,
                    captured,
                    previous_z_slot,
                );
                return Err(OperationError::NodeNotFound(node.clone()));
            }
            // S-03 — a table is story-nested, never a spread page item;
            // MoveNode (page-item z/spread reparent) doesn't apply. Roll
            // back the capture and reject.
            NodeSpec::Table { .. } => {
                restore_capture(
                    doc,
                    &previous_parent,
                    previous_position,
                    captured,
                    previous_z_slot,
                );
                return Err(OperationError::InvalidParent {
                    parent: new_parent.clone(),
                    child_kind: "Table".to_string(),
                });
            }
        },
        None => {
            restore_capture(
                doc,
                &previous_parent,
                previous_position,
                captured,
                previous_z_slot,
            );
            return Err(OperationError::NodeNotFound(new_parent.clone()));
        }
    };

    if position > target_len {
        restore_capture(
            doc,
            &previous_parent,
            previous_position,
            captured,
            previous_z_slot,
        );
        return Err(OperationError::InvalidPosition {
            parent: new_parent.clone(),
            position,
            len: target_len,
        });
    }

    // Forward move lands on top of the destination's z-order; the
    // origin slot only matters for the undo path (restore_capture).
    insert_captured(doc, &new_parent_id, position, captured, None)?;

    let inverse = invert_move_node(node.clone(), previous_parent, previous_position);
    Ok(AppliedOperation {
        op: Operation::MoveNode {
            node: node.clone(),
            new_parent: new_parent.clone(),
            position,
        },
        inverse,
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// Put the captured node back exactly where it was. Infallible — the
/// position came from the doc itself moments ago.
pub(super) fn restore_capture(
    doc: &mut Document,
    parent: &NodeId,
    position: usize,
    spec: NodeSpec,
    z_slot: Option<usize>,
) {
    let _ = apply_insert_node(doc, parent, position, z_slot, &spec);
}

/// Insert a captured node into the spread `parent_self_id`. One path
/// with `InsertNode` — the capture is a [`NodeSpec::Captured`] (the
/// whole node, C-75), or a `NodeSpec::Table` re-attaching to its story.
pub(super) fn insert_captured(
    doc: &mut Document,
    parent_self_id: &str,
    position: usize,
    spec: NodeSpec,
    z_slot: Option<usize>,
) -> Result<(), OperationError> {
    apply_insert_node(
        doc,
        &NodeId::Spread(parent_self_id.to_string()),
        position,
        z_slot,
        &spec,
    )
    .map(|_| ())
}
