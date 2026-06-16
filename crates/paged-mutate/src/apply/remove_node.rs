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
use paged_parse::FrameRef;
use paged_scene::Document;

use crate::error::OperationError;
use crate::invert::invert_remove_node;
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, NodeSpec, Operation, PathAnchorSpec, PropertyPath,
};

// ---------------------------------------------------------------------------
// RemoveNode
// ---------------------------------------------------------------------------

pub(super) fn apply_remove_node(
    doc: &mut Document,
    node: &NodeId,
) -> Result<AppliedOperation, OperationError> {
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

/// Locate `node` in its containing spread, snapshot its current state
/// into a `NodeSpec`, and remove it (including its `frames_in_order`
/// entry). Returns `(parent_id, position, spec, z_slot)` for the
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
                    let z_slot =
                        unregister_frame_ref(&mut parsed.spread, FrameRef::TextFrame(0), pos);
                    let parent = spread_parent_id(parsed);
                    let spec = NodeSpec::TextFrame {
                        self_id: id.clone(),
                        bounds: bounds_to_array(frame.bounds),
                        fill_color: frame.fill_color,
                        stroke_color: frame.stroke_color,
                        stroke_weight: frame.stroke_weight,
                        item_transform: frame.item_transform,
                        // Captured so undo-of-delete REATTACHES the
                        // story (the text comes back with the frame).
                        parent_story: frame.parent_story,
                    };
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
                    let rect = parsed.spread.rectangles.remove(pos);
                    let z_slot =
                        unregister_frame_ref(&mut parsed.spread, FrameRef::Rectangle(0), pos);
                    let parent = spread_parent_id(parsed);
                    let spec = NodeSpec::Rectangle {
                        self_id: id.clone(),
                        bounds: bounds_to_array(rect.bounds),
                        fill_color: rect.fill_color,
                        stroke_color: rect.stroke_color,
                        stroke_weight: rect.stroke_weight,
                        item_transform: rect.item_transform,
                    };
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
                    let oval = parsed.spread.ovals.remove(pos);
                    let z_slot = unregister_frame_ref(&mut parsed.spread, FrameRef::Oval(0), pos);
                    let parent = spread_parent_id(parsed);
                    let spec = NodeSpec::Oval {
                        self_id: id.clone(),
                        bounds: bounds_to_array(oval.bounds),
                        fill_color: oval.fill_color,
                        stroke_color: oval.stroke_color,
                        stroke_weight: oval.stroke_weight,
                        item_transform: oval.item_transform,
                    };
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
                    let z_slot =
                        unregister_frame_ref(&mut parsed.spread, FrameRef::GraphicLine(0), pos);
                    let parent = spread_parent_id(parsed);
                    let spec = NodeSpec::GraphicLine {
                        self_id: id.clone(),
                        bounds: bounds_to_array(line.bounds),
                        anchors: line
                            .anchors
                            .iter()
                            .map(PathAnchorSpec::from_parse)
                            .collect(),
                        subpath_starts: line.subpath_starts,
                        subpath_open: line.subpath_open,
                        stroke_color: line.stroke_color,
                        stroke_weight: line.stroke_weight,
                        item_transform: line.item_transform,
                    };
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
                    let poly = parsed.spread.polygons.remove(pos);
                    let z_slot =
                        unregister_frame_ref(&mut parsed.spread, FrameRef::Polygon(0), pos);
                    let parent = spread_parent_id(parsed);
                    let spec = NodeSpec::Polygon {
                        self_id: id.clone(),
                        bounds: bounds_to_array(poly.bounds),
                        anchors: poly
                            .anchors
                            .iter()
                            .map(PathAnchorSpec::from_parse)
                            .collect(),
                        subpath_starts: poly.subpath_starts,
                        subpath_open: poly.subpath_open,
                        fill_color: poly.fill_color,
                        stroke_color: poly.stroke_color,
                        stroke_weight: poly.stroke_weight,
                        item_transform: poly.item_transform,
                    };
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
            let para = doc.stories[si].story.paragraphs.remove(pi);
            let table = para
                .table
                .expect("find_table_pos guarantees the paragraph carries a table");
            let column_widths: Vec<f32> = table
                .columns
                .iter()
                .map(|c| c.single_column_width.unwrap_or(0.0))
                .collect();
            let row_heights: Vec<f32> = table
                .rows
                .iter()
                .map(|r| r.single_row_height.unwrap_or(0.0))
                .collect();
            let spec = NodeSpec::Table {
                self_id: table_id.clone(),
                rows: table.rows.len() as u32,
                cols: table.columns.len().max(table.column_count as usize) as u32,
                header_rows: table.header_row_count,
                footer_rows: table.footer_row_count,
                column_widths,
                row_heights,
            };
            // The parent is the host story; position is the dropped
            // paragraph index (accepted but ignored by re-insert, which
            // appends — see `apply_insert_table`). z_slot is N/A for a
            // story-nested node.
            Ok((NodeId::Story(story_id.clone()), pi, spec, None))
        }
        _ => Err(OperationError::UnsupportedProperty {
            node: node.clone(),
            path: PropertyPath::FrameBounds, // unused; signals "this node kind isn't removable yet"
        }),
    }
}
