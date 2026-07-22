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
use crate::invert::invert_move_node;
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, NodeSpec, Operation, PathAnchorSpec,
};

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

    // Capture before state by removing, then re-insert at the target.
    // If insertion fails, restore in place so the doc state is intact.
    let (previous_parent, previous_position, captured, previous_z_slot) =
        remove_and_capture(doc, node)?;

    // Read destination spread length without holding a borrow across
    // the potentially-rollback path.
    let target_len = match find_spread(doc, &new_parent_id) {
        Some(dest) => match &captured {
            NodeSpec::TextFrame { .. } => dest.spread.text_frames.len(),
            NodeSpec::Rectangle { .. } => dest.spread.rectangles.len(),
            NodeSpec::Oval { .. } => dest.spread.ovals.len(),
            NodeSpec::GraphicLine { .. } => dest.spread.graphic_lines.len(),
            NodeSpec::Polygon { .. } => dest.spread.polygons.len(),
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
    let _ = insert_captured(doc, parent.self_id(), position, spec, z_slot);
}

pub(super) fn insert_captured(
    doc: &mut Document,
    parent_self_id: &str,
    position: usize,
    spec: NodeSpec,
    z_slot: Option<usize>,
) -> Result<(), OperationError> {
    // S-03 — a table re-attaches to its host STORY, not a spread.
    // `parent_self_id` is the story id (`NodeId::Story::self_id()`).
    // Re-create the table paragraph at the story end (same offset rule
    // as `apply_insert_table`); `position` / `z_slot` are N/A here.
    if let NodeSpec::Table { .. } = &spec {
        let _ = (position, z_slot);
        let si = doc
            .stories
            .iter()
            .position(|s| s.self_id == parent_self_id)
            .ok_or_else(|| {
                OperationError::NodeNotFound(NodeId::Story(parent_self_id.to_string()))
            })?;
        let table = spec.to_parse_table();
        doc.stories[si]
            .story
            .paragraphs
            .push(paged_model::Paragraph {
                table: Some(table),
                ..Default::default()
            });
        return Ok(());
    }
    let spread = find_spread_mut(doc, parent_self_id)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Spread(parent_self_id.to_string())))?;
    match spec {
        NodeSpec::TextFrame {
            self_id,
            bounds,
            fill_color,
            stroke_color,
            stroke_weight,
            item_transform,
            parent_story,
        } => {
            let mut frame = new_text_frame(self_id, bounds_from_array(bounds), fill_color);
            frame.parent_story = parent_story;
            frame.stroke_color = stroke_color;
            frame.stroke_weight = stroke_weight;
            frame.item_transform = item_transform;
            spread.spread.text_frames.insert(position, frame);
            register_frame_ref(&mut spread.spread, FrameRef::TextFrame(0), position, z_slot);
        }
        NodeSpec::Rectangle {
            self_id,
            bounds,
            fill_color,
            stroke_color,
            stroke_weight,
            item_transform,
        } => {
            let mut rect = new_rectangle(self_id, bounds_from_array(bounds), fill_color);
            rect.stroke_color = stroke_color;
            rect.stroke_weight = stroke_weight;
            rect.item_transform = item_transform;
            spread.spread.rectangles.insert(position, rect);
            register_frame_ref(&mut spread.spread, FrameRef::Rectangle(0), position, z_slot);
        }
        NodeSpec::Oval {
            self_id,
            bounds,
            fill_color,
            stroke_color,
            stroke_weight,
            item_transform,
        } => {
            let mut oval = new_oval(self_id, bounds_from_array(bounds), fill_color);
            oval.stroke_color = stroke_color;
            oval.stroke_weight = stroke_weight;
            oval.item_transform = item_transform;
            spread.spread.ovals.insert(position, oval);
            register_frame_ref(&mut spread.spread, FrameRef::Oval(0), position, z_slot);
        }
        NodeSpec::GraphicLine {
            self_id,
            bounds,
            anchors,
            subpath_starts,
            subpath_open,
            stroke_color,
            stroke_weight,
            item_transform,
        } => {
            let mut line = new_graphic_line(
                self_id,
                bounds_from_array(bounds),
                anchors.iter().map(PathAnchorSpec::to_parse).collect(),
                subpath_starts,
                subpath_open,
                stroke_color,
                stroke_weight,
            );
            line.item_transform = item_transform;
            spread.spread.graphic_lines.insert(position, line);
            register_frame_ref(
                &mut spread.spread,
                FrameRef::GraphicLine(0),
                position,
                z_slot,
            );
        }
        NodeSpec::Polygon {
            self_id,
            bounds,
            anchors,
            subpath_starts,
            subpath_open,
            fill_color,
            stroke_color,
            stroke_weight,
            item_transform,
        } => {
            let mut poly = new_polygon(
                self_id,
                bounds_from_array(bounds),
                anchors.iter().map(PathAnchorSpec::to_parse).collect(),
                subpath_starts,
                subpath_open,
                fill_color,
                stroke_color,
                stroke_weight,
            );
            poly.item_transform = item_transform;
            spread.spread.polygons.insert(position, poly);
            register_frame_ref(&mut spread.spread, FrameRef::Polygon(0), position, z_slot);
        }
        // Same rationale as in apply_move_node: CloneTranslate is
        // never re-inserted via this path.
        NodeSpec::CloneTranslate { source, .. } => {
            return Err(OperationError::NodeNotFound(source));
        }
        // S-03 — handled by the story-re-attach early-return above.
        NodeSpec::Table { .. } => {
            unreachable!("Table re-insert routed via the early-return");
        }
    }
    Ok(())
}
