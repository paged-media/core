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

//! `apply(doc, op)` — the only function that mutates a [`Document`].
//!
//! Each variant of [`Operation`] dispatches to a small per-variant
//! helper. The helper captures the "before" state, performs the
//! mutation in place on the parse-layer structs, and hands the
//! captured pieces to the [`invert`](crate::invert) helpers to build
//! the matching inverse op. The result is bundled into an
//! [`AppliedOperation`] along with an [`InvalidationHint`].
//!
//! Batch atomicity: if any child fails, every previously-applied
//! child is rolled back by applying its inverse in reverse order
//! *before* `apply` returns the error. The document is then in the
//! state it was in before the batch began. The error carries the
//! index that failed.
//!
//! Stage 1 limitations (flagged in `docs/paged/scripting-layer.md`'s
//! Stage-1 deliverables):
//!   - `Document`'s pre-built indices (`text_frame_index`,
//!     `frame_for_story`) are not surgically maintained — they're
//!     valid for the unmutated open, and consumers that want them
//!     fresh after Insert/Remove/Move should rebuild via
//!     `Document::open` or a future `rebuild_indices` helper. The
//!     parse-layer leaf data is the source of truth.
//!   - `InsertNode`/`RemoveNode`/`MoveNode` support TextFrame and
//!     Rectangle children under a Spread parent. Group nesting,
//!     Page-level routing, and the other shape kinds (Oval, Polygon,
//!     GraphicLine) come as later stages.

use paged_parse::{Bounds, FrameRef, GraphicLine, Oval, Polygon, Rectangle, Spread, TextFrame};
use paged_scene::Document;

use crate::error::OperationError;
use crate::invert::{
    invert_batch, invert_insert_node, invert_move_node, invert_remove_node, invert_set_property,
};
use crate::operation::{
    AppliedOperation, ColorGroupSpec, FieldKind, GradientFeatherSpec, GradientSpec,
    GradientStopSpec, GroupSpec, GuideOrientationSpec, InvalidationHint, NodeId, NodeSpec,
    Operation, PathAnchorSpec, PathfinderKind, PropertyPath, StyleCollection, StyleScope,
    SwatchSpec, Value,
};
use crate::pathfinder::{pathfinder_boolean, PathfinderKind as InternalPathfinderKind};

/// Apply an operation to `doc`. Returns the captured `AppliedOperation`
/// (carrying op + inverse + invalidation hint) on success. The only
/// mutation entry point in the crate.
pub fn apply(doc: &mut Document, op: &Operation) -> Result<AppliedOperation, OperationError> {
    match op {
        Operation::SetProperty { node, path, value } => apply_set_property(doc, node, *path, value),
        Operation::InsertNode {
            parent,
            position,
            node,
            z_slot,
        } => apply_insert_node(doc, parent, *position, *z_slot, node),
        Operation::RemoveNode { node } => apply_remove_node(doc, node),
        Operation::MoveNode {
            node,
            new_parent,
            position,
        } => apply_move_node(doc, node, new_parent, *position),
        Operation::Batch { ops } => apply_batch(doc, ops),
        Operation::InsertPage {
            after_page_id,
            master_id,
            spread_self_id,
            page_self_id,
            restore_spread_json,
        } => apply_insert_page(
            doc,
            after_page_id.as_deref(),
            master_id.as_deref(),
            spread_self_id.clone(),
            page_self_id.clone(),
            restore_spread_json.as_deref(),
        ),
        Operation::RemovePage { page_id } => apply_remove_page(doc, page_id),
        Operation::MoveLayer {
            layer_id,
            new_index,
        } => apply_move_layer(doc, layer_id, *new_index),
        Operation::InsertLayer {
            position,
            name,
            self_id,
        } => apply_insert_layer(doc, *position, name, self_id.as_deref()),
        Operation::RemoveLayer { layer_id } => apply_remove_layer(doc, layer_id),
        Operation::CreateSwatch { spec } => apply_create_swatch(doc, spec),
        Operation::EditSwatch { swatch_id, spec } => apply_edit_swatch(doc, swatch_id, spec),
        Operation::DeleteSwatch { swatch_id } => apply_delete_swatch(doc, swatch_id),
        Operation::CreateParagraphStyle {
            self_id,
            name,
            based_on,
            restore_json,
        } => apply_create_paragraph_style(
            doc,
            self_id.clone(),
            name.clone(),
            based_on.clone(),
            restore_json.as_deref(),
        ),
        Operation::RenameParagraphStyle { style_id, name } => {
            apply_rename_paragraph_style(doc, style_id, name)
        }
        Operation::DeleteParagraphStyle { style_id } => apply_delete_paragraph_style(doc, style_id),
        Operation::CreateCharacterStyle {
            self_id,
            name,
            based_on,
            restore_json,
        } => apply_create_character_style(
            doc,
            self_id.clone(),
            name.clone(),
            based_on.clone(),
            restore_json.as_deref(),
        ),
        Operation::RenameCharacterStyle { style_id, name } => {
            apply_rename_character_style(doc, style_id, name)
        }
        Operation::DeleteCharacterStyle { style_id } => apply_delete_character_style(doc, style_id),
        Operation::CreateObjectStyle {
            self_id,
            name,
            based_on,
            restore_json,
        } => apply_create_object_style(
            doc,
            self_id.clone(),
            name.clone(),
            based_on.clone(),
            restore_json.as_deref(),
        ),
        Operation::RenameObjectStyle { style_id, name } => {
            apply_rename_object_style(doc, style_id, name)
        }
        Operation::DeleteObjectStyle { style_id } => apply_delete_object_style(doc, style_id),
        Operation::CreateCellStyle {
            self_id,
            name,
            based_on,
            restore_json,
        } => apply_create_cell_style(
            doc,
            self_id.clone(),
            name.clone(),
            based_on.clone(),
            restore_json.as_deref(),
        ),
        Operation::RenameCellStyle { style_id, name } => {
            apply_rename_cell_style(doc, style_id, name)
        }
        Operation::DeleteCellStyle { style_id } => apply_delete_cell_style(doc, style_id),
        Operation::CreateTableStyle {
            self_id,
            name,
            based_on,
            restore_json,
        } => apply_create_table_style(
            doc,
            self_id.clone(),
            name.clone(),
            based_on.clone(),
            restore_json.as_deref(),
        ),
        Operation::RenameTableStyle { style_id, name } => {
            apply_rename_table_style(doc, style_id, name)
        }
        Operation::DeleteTableStyle { style_id } => apply_delete_table_style(doc, style_id),
        Operation::CreateGroup { spec } => apply_create_group(doc, spec),
        Operation::DissolveGroup {
            group_id,
            restore_slots,
        } => apply_dissolve_group(doc, group_id, restore_slots.as_deref()),
        Operation::CreateGradient { spec } => apply_create_gradient(doc, spec),
        Operation::EditGradient { gradient_id, spec } => {
            apply_edit_gradient(doc, gradient_id, spec)
        }
        Operation::DeleteGradient { gradient_id } => apply_delete_gradient(doc, gradient_id),
        Operation::CreateColorGroup { spec } => apply_create_color_group(doc, spec),
        Operation::EditColorGroup { group_id, spec } => apply_edit_color_group(doc, group_id, spec),
        Operation::DeleteColorGroup { group_id } => apply_delete_color_group(doc, group_id),
        Operation::SetStyleProperty {
            collection,
            style_id,
            path,
            value,
        } => apply_set_style_property(doc, *collection, style_id, *path, value),
        Operation::PathfinderBoolean {
            kept,
            others,
            op_kind,
        } => apply_pathfinder(doc, kept, others, *op_kind),
        Operation::LinkFrames { from, to } => apply_link_frames(doc, from, to),
        Operation::UnlinkFrames { frame, prev_next } => {
            apply_unlink_frames(doc, frame, prev_next.as_deref())
        }
        Operation::ApplyStyle {
            story_id,
            start,
            end,
            style,
            scope,
        } => apply_apply_style(doc, story_id, *start, *end, style, *scope),
        Operation::InsertField {
            story_id,
            offset,
            field,
        } => apply_insert_field(doc, story_id, *offset, *field),
        Operation::DeleteField {
            story_id,
            offset,
            field,
        } => apply_delete_field(doc, story_id, *offset, *field),
        Operation::InsertGuide {
            spread_id,
            orientation,
            position,
            page_index,
            guide_id,
        } => apply_insert_guide(
            doc,
            spread_id,
            *orientation,
            *position,
            *page_index,
            guide_id.clone(),
        ),
        Operation::MoveGuide { guide_id, position } => apply_move_guide(doc, guide_id, *position),
        Operation::DeleteGuide { guide_id } => apply_delete_guide(doc, guide_id),
        Operation::SetConditionVisible { condition, visible } => {
            apply_set_condition_visible(doc, condition, *visible)
        }
        Operation::ActivateConditionSet { set } => apply_activate_condition_set(doc, set),
        Operation::RestoreConditionVisibility { states } => {
            apply_restore_condition_visibility(doc, states)
        }
        Operation::ApplyMasterToPage { page, master } => {
            apply_master_to_page(doc, page, master.as_deref())
        }
        Operation::DuplicatePage {
            page,
            clone_spread_json,
        } => apply_duplicate_page(doc, page, clone_spread_json.as_deref()),
        Operation::InsertSection {
            at_page,
            prefix,
            numbering_style,
            start_at,
            self_id,
        } => apply_insert_section(
            doc,
            at_page,
            prefix.clone(),
            numbering_style.clone(),
            *start_at,
            self_id.clone(),
        ),
        Operation::EditSection {
            section_id,
            prefix,
            numbering_style,
            start_at,
        } => apply_edit_section(
            doc,
            section_id,
            prefix.clone(),
            numbering_style.clone(),
            *start_at,
        ),
        Operation::DeleteSection { section_id } => apply_delete_section(doc, section_id),
        Operation::SetRowHeight {
            story_id,
            table_id,
            row,
            height,
        } => apply_set_row_height(doc, story_id, table_id, *row, *height),
        Operation::SetColumnWidth {
            story_id,
            table_id,
            col,
            width,
        } => apply_set_column_width(doc, story_id, table_id, *col, *width),
        Operation::InsertTableRow {
            story_id,
            table_id,
            at,
            restore,
        } => apply_insert_table_row(doc, story_id, table_id, *at, restore.as_deref()),
        Operation::DeleteTableRow {
            story_id,
            table_id,
            at,
        } => apply_delete_table_row(doc, story_id, table_id, *at),
        Operation::InsertTableColumn {
            story_id,
            table_id,
            at,
            restore,
        } => apply_insert_table_column(doc, story_id, table_id, *at, restore.as_deref()),
        Operation::DeleteTableColumn {
            story_id,
            table_id,
            at,
        } => apply_delete_table_column(doc, story_id, table_id, *at),
    }
}

/// SDK Phase 5 (v1 sweep) — apply a multi-target Pathfinder
/// boolean. Reads every input's path, runs flo_curves CSG via
/// `pathfinder::pathfinder_boolean`, then builds + applies an
/// internal Batch (FramePath on kept + RemoveNode for each other).
/// The returned AppliedOperation is the Batch — undo reverses
/// everything in one Cmd-Z. Inverse restoration of the removed
/// frames goes through the same path RemoveNode normally takes
/// (specs captured at remove time).
fn apply_pathfinder(
    doc: &mut Document,
    kept: &NodeId,
    others: &[NodeId],
    kind: PathfinderKind,
) -> Result<AppliedOperation, OperationError> {
    // 1. Snapshot every input's path. find_path_anchors_mut also
    //    handles the "no anchors → bounds rectangle" fallback in
    //    its callers; for Pathfinder we read anchors directly
    //    since the result IS a path replacement.
    let mut inputs: Vec<(Vec<paged_parse::PathAnchor>, Vec<usize>)> =
        Vec::with_capacity(1 + others.len());
    for node in std::iter::once(kept).chain(others.iter()) {
        let (anchors, starts) = read_path(doc, node)?;
        inputs.push((anchors, starts));
    }
    // 2. Run the boolean.
    let internal_kind = match kind {
        PathfinderKind::Union => InternalPathfinderKind::Union,
        PathfinderKind::Intersect => InternalPathfinderKind::Intersect,
        PathfinderKind::Subtract => InternalPathfinderKind::Subtract,
        PathfinderKind::Exclude => InternalPathfinderKind::Exclude,
    };
    let (result_anchors, result_starts) = pathfinder_boolean(&inputs, internal_kind);
    // 3. Build the inner Batch.
    let result_spec_anchors: Vec<PathAnchorSpec> = result_anchors
        .iter()
        .map(PathAnchorSpec::from_parse)
        .collect();
    let mut batch_children: Vec<Operation> = Vec::with_capacity(1 + others.len());
    batch_children.push(Operation::SetProperty {
        node: kept.clone(),
        path: PropertyPath::FramePath,
        value: Value::FramePath {
            anchors: result_spec_anchors,
            subpath_starts: result_starts,
        },
    });
    for other in others {
        batch_children.push(Operation::RemoveNode {
            node: other.clone(),
        });
    }
    let batch = Operation::Batch {
        ops: batch_children,
    };
    // 4. Apply the Batch — the existing `apply_batch` machinery
    //    rolls back on any child failure and produces the inverse.
    let applied = apply(doc, &batch)?;
    // The op we record is the original PathfinderBoolean (so the
    // forward op is meaningful in logs); the inverse is the
    // Batch's inverse (which restores the removed frames + the
    // kept frame's prior path).
    Ok(AppliedOperation {
        op: Operation::PathfinderBoolean {
            kept: kept.clone(),
            others: others.to_vec(),
            op_kind: kind,
        },
        inverse: applied.inverse,
        invalidation: applied.invalidation,
    })
}

/// SDK Phase 5 (v1 sweep) — read a frame's full path. Returns
/// the anchors + subpath_starts as a cloned snapshot. When the
/// frame has no explicit anchors (a Rectangle declared via
/// `GeometricBounds` alone), synthesises a four-corner closed
/// path from the bounds so Pathfinder still has geometry to
/// operate on.
fn read_path(
    doc: &Document,
    node: &NodeId,
) -> Result<(Vec<paged_parse::PathAnchor>, Vec<usize>), OperationError> {
    use paged_parse::PathAnchor;
    let raw = node.self_id();
    for parsed in &doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if let Some(f) = parsed
                    .spread
                    .text_frames
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    if !f.anchors.is_empty() {
                        return Ok((f.anchors.clone(), f.subpath_starts.clone()));
                    }
                    return Ok((rect_anchors_from_bounds(f.bounds), vec![0]));
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(f) = parsed
                    .spread
                    .rectangles
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    if !f.anchors.is_empty() {
                        return Ok((f.anchors.clone(), f.subpath_starts.clone()));
                    }
                    return Ok((rect_anchors_from_bounds(f.bounds), vec![0]));
                }
            }
            NodeId::Oval(_) => {
                if let Some(f) = parsed
                    .spread
                    .ovals
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    // Oval's parse layer doesn't expose an
                    // explicit anchors vec — synthesise a four-
                    // corner approximation from bounds for now.
                    // A future polish emits the proper four-arc
                    // ellipse anchors.
                    return Ok((rect_anchors_from_bounds(f.bounds), vec![0]));
                }
            }
            NodeId::Polygon(_) => {
                if let Some(f) = parsed
                    .spread
                    .polygons
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Ok((f.anchors.clone(), f.subpath_starts.clone()));
                }
            }
            NodeId::GraphicLine(_) => {
                if let Some(f) = parsed
                    .spread
                    .graphic_lines
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Ok((f.anchors.clone(), f.subpath_starts.clone()));
                }
            }
            _ => {}
        }
    }
    let _ = PathAnchor {
        anchor: (0.0, 0.0),
        left: (0.0, 0.0),
        right: (0.0, 0.0),
    };
    Err(OperationError::NodeNotFound(node.clone()))
}

fn rect_anchors_from_bounds(b: paged_parse::Bounds) -> Vec<paged_parse::PathAnchor> {
    use paged_parse::PathAnchor;
    let (t, l, r, btm) = (b.top, b.left, b.right, b.bottom);
    let corner = |x: f32, y: f32| PathAnchor {
        anchor: (x, y),
        left: (x, y),
        right: (x, y),
    };
    vec![corner(l, t), corner(r, t), corner(r, btm), corner(l, btm)]
}

// ---------------------------------------------------------------------------
// SetProperty
// ---------------------------------------------------------------------------

fn apply_set_property(
    doc: &mut Document,
    node: &NodeId,
    path: PropertyPath,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    // Track J — path-topology ops construct their inverse on a
    // different `PropertyPath` than the forward op (Insert ↔ Remove,
    // CurveType ↔ CurveType-with-restore), so they can't share the
    // bottom-of-function `invert_set_property` path. Each helper
    // returns a fully-formed AppliedOperation.
    match (node, path) {
        // Track J fan-out — path-topology ops accept any path-bearing
        // page item kind (Polygon, TextFrame, Rectangle, GraphicLine).
        // The four kinds carry identical `anchors` + `subpath_starts`
        // fields in paged-parse; the helper `find_path_anchors_mut`
        // returns &mut access regardless of variant so the apply
        // arms stay kind-agnostic.
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_),
            PropertyPath::PathPointInsert,
        ) => {
            return apply_path_point_insert(doc, node, value);
        }
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_),
            PropertyPath::PathPointRemove,
        ) => {
            return apply_path_point_remove(doc, node, value);
        }
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_),
            PropertyPath::PathPointCurveType,
        ) => {
            return apply_path_point_curve_type(doc, node, value);
        }
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_),
            PropertyPath::PathOpenAt,
        ) => {
            return apply_path_open_at(doc, node, value);
        }
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_),
            PropertyPath::OutlineStroke | PropertyPath::OffsetPath | PropertyPath::SimplifyPath,
        ) => {
            return apply_path_kernel_op(doc, node, &path, value);
        }
        // Plugin-metadata carrier — its inverse carries the prev
        // snapshot inside the same Value, so it short-circuits like
        // the Track J ops. All five leaf page-item kinds.
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_)
            | NodeId::Oval(_),
            PropertyPath::PluginMetadata,
        ) => {
            return apply_plugin_metadata(doc, node, value);
        }
        // W3.A1 — table-scoped writes: `AppliedTableStyle` on a
        // `NodeId::Table`, and every cell-scoped path on a
        // `NodeId::TableCell`. These resolve `(story_id, table_id[,
        // row, col])` and build their own inverse (the standard
        // `invert_set_property` tail keys off page-item kinds and
        // doesn't reach tables), so they short-circuit here.
        (NodeId::Table { .. }, _) => {
            return apply_table_property(doc, node, path, value);
        }
        (NodeId::TableCell { .. }, _) => {
            return apply_cell_property(doc, node, path, value);
        }
        _ => {}
    }
    let (previous, invalidation) = match (node, path) {
        (NodeId::TextFrame(id), PropertyPath::FrameBounds) => {
            let new_bounds = expect_bounds(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = bounds_to_array(frame.bounds);
            frame.bounds = bounds_from_array(new_bounds);
            (
                Value::Bounds(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(id), PropertyPath::FrameFillColor) => {
            let new_color = expect_color_ref(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.fill_color.clone();
            frame.fill_color = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameBounds) => {
            let new_bounds = expect_bounds(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = bounds_to_array(rect.bounds);
            rect.bounds = bounds_from_array(new_bounds);
            (
                Value::Bounds(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameFillColor) => {
            let new_color = expect_color_ref(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.fill_color.clone();
            rect.fill_color = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- Inspector M1 Phase A: stroke + opacity --------------
        (NodeId::TextFrame(id), PropertyPath::FrameStrokeColor) => {
            let new_color = expect_color_ref(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.stroke_color.clone();
            frame.stroke_color = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameStrokeColor) => {
            let new_color = expect_color_ref(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.stroke_color.clone();
            rect.stroke_color = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(id), PropertyPath::FrameStrokeWeight) => {
            let new_weight = expect_length(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.stroke_weight;
            frame.stroke_weight = new_weight;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameStrokeWeight) => {
            let new_weight = expect_length(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.stroke_weight;
            rect.stroke_weight = new_weight;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(id), PropertyPath::FrameOpacity) => {
            let new_opacity = expect_length(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.opacity;
            frame.opacity = new_opacity;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameOpacity) => {
            let new_opacity = expect_length(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.opacity;
            rect.opacity = new_opacity;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- Phase D: FrameTransform ------------------------------
        (NodeId::TextFrame(id), PropertyPath::FrameTransform) => {
            let new_transform = expect_transform(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.item_transform;
            frame.item_transform = new_transform;
            (
                Value::Transform(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // Track L — Group's own ItemTransform. The leaves carry the
        // composed transform pre-baked by the parser
        // (`paged-parse/spread.rs:141-144`), so mutating only the
        // Group would visually shift everything. Pair this op with
        // per-leaf SetProperty(FrameTransform, G' * inv(G) * old)
        // ops in a Batch — the gesture spine (L.2) does that
        // composition; this arm just stores the Group's own
        // transform so reserialization preserves the grouped
        // structure.
        (NodeId::Group(id), PropertyPath::FrameTransform) => {
            let new_transform = expect_transform(path, value)?;
            let group = find_group_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = group.item_transform;
            group.item_transform = new_transform;
            (
                Value::Transform(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameTransform) => {
            let new_transform = expect_transform(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.item_transform;
            rect.item_transform = new_transform;
            (
                Value::Transform(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // Editor-ops — the remaining page-item kinds join the
        // transform path (closes the latent Rotate/Scale gap; the
        // Shear gesture needs all of them).
        (NodeId::Polygon(id), PropertyPath::FrameTransform) => {
            let new_transform = expect_transform(path, value)?;
            let poly = find_polygon_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = poly.item_transform;
            poly.item_transform = new_transform;
            (
                Value::Transform(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Oval(id), PropertyPath::FrameTransform) => {
            let new_transform = expect_transform(path, value)?;
            let oval =
                find_oval_mut(doc, id).ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = oval.item_transform;
            oval.item_transform = new_transform;
            (
                Value::Transform(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::GraphicLine(id), PropertyPath::FrameTransform) => {
            let new_transform = expect_transform(path, value)?;
            let line = find_graphic_line_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = line.item_transform;
            line.item_transform = new_transform;
            (
                Value::Transform(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- Phase H: FramePathPoint (any path-bearing kind) -----
        // Track J fan-out — accepts Polygon, TextFrame, Rectangle,
        // GraphicLine. All four kinds share the anchor field shape.
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FramePathPoint,
        ) => {
            let (address, position) = expect_path_point(path, value)?;
            let (anchors, _starts, _open) = find_path_anchors_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let Some(anchor) = anchors.get_mut(address.index) else {
                return Err(OperationError::NodeNotFound(node.clone()));
            };
            let prev_pos = match address.role {
                crate::operation::PathPointRole::Anchor => anchor.anchor,
                crate::operation::PathPointRole::Left => anchor.left,
                crate::operation::PathPointRole::Right => anchor.right,
            };
            match address.role {
                crate::operation::PathPointRole::Anchor => {
                    // Moving the anchor drags both handles by the same
                    // delta so the curve shape stays put relative to
                    // the anchor (industry convention).
                    let dx = position[0] - anchor.anchor.0;
                    let dy = position[1] - anchor.anchor.1;
                    anchor.anchor = (position[0], position[1]);
                    anchor.left = (anchor.left.0 + dx, anchor.left.1 + dy);
                    anchor.right = (anchor.right.0 + dx, anchor.right.1 + dy);
                }
                crate::operation::PathPointRole::Left => {
                    anchor.left = (position[0], position[1]);
                }
                crate::operation::PathPointRole::Right => {
                    anchor.right = (position[0], position[1]);
                }
            }
            (
                Value::PathPoint {
                    address,
                    position: [prev_pos.0, prev_pos.1],
                },
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- Track M: Layer toggles (visible / locked / printable)
        (NodeId::Layer(id), PropertyPath::LayerVisible) => {
            let new_value = expect_bool(path, value)?;
            let layer = find_layer_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = layer.visible;
            layer.visible = new_value;
            (
                Value::Bool(prev),
                InvalidationHint {
                    structural: true,
                    ..Default::default()
                },
            )
        }
        (NodeId::Layer(id), PropertyPath::LayerLocked) => {
            let new_value = expect_bool(path, value)?;
            let layer = find_layer_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = layer.locked;
            layer.locked = new_value;
            (
                Value::Bool(prev),
                // Locked is a hit-test concern only; no scene
                // geometry / layout depends on it.
                InvalidationHint::default(),
            )
        }
        (NodeId::Layer(id), PropertyPath::LayerPrintable) => {
            let new_value = expect_bool(path, value)?;
            let layer = find_layer_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = layer.printable;
            layer.printable = new_value;
            (
                Value::Bool(prev),
                InvalidationHint {
                    structural: true,
                    ..Default::default()
                },
            )
        }
        (NodeId::Layer(id), PropertyPath::LayerName) => {
            let new_value = expect_text(path, value)?;
            let layer = find_layer_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = layer.name.clone().unwrap_or_default();
            layer.name = Some(new_value);
            (
                Value::Text(prev),
                // Name is purely a label; no scene geometry depends.
                InvalidationHint::default(),
            )
        }
        // ---- SDK Phase 5 (v1 sweep) — stroke end-cap (enum string)
        // Per-frame override. Empty string clears the override.
        // Only Rectangle / Oval / Polygon / GraphicLine carry the
        // `end_cap` field in the parse layer — TextFrame's stroke
        // shape does not (its renderer path uses a simple solid
        // outline rather than a stroked path with cap/join). Falls
        // through to UnsupportedProperty for TextFrame.
        (NodeId::Rectangle(id), PropertyPath::FrameStrokeEndCap) => {
            let new_val = expect_text(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.end_cap.clone().unwrap_or_default();
            rect.end_cap = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            (
                Value::Text(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- SDK Phase 5 (v1 sweep) — text wrap mode + offsets ----
        // All five page-item kinds (TextFrame / Rectangle / Oval /
        // Polygon / GraphicLine) carry `text_wrap: Option<TextWrap>`.
        // Each property writes one field of the TextWrap, preserving
        // the other; if the prior state was `None`, the apply layer
        // materialises a default TextWrap (mode=None, offsets=[0;4])
        // so partial writes don't drop information silently.
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameTextWrapMode,
        ) => {
            let new_val = expect_text(path, value)?;
            let tw = find_text_wrap_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev_mode = tw.map(|t| t.mode.as_idml().to_string()).unwrap_or_default();
            let prev_offsets = tw.map(|t| t.offsets).unwrap_or([0.0; 4]);
            let prev_invert = tw.and_then(|t| t.invert);
            if new_val.is_empty() {
                *tw = None;
            } else {
                *tw = Some(paged_parse::TextWrap {
                    mode: paged_parse::TextWrapMode::from_idml(&new_val),
                    offsets: prev_offsets,
                    invert: prev_invert,
                });
            }
            let _ = prev_offsets;
            (
                Value::Text(prev_mode),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameTextWrapOffsets,
        ) => {
            let new_offsets = expect_bounds(path, value)?;
            let tw = find_text_wrap_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev_mode = tw
                .map(|t| t.mode)
                .unwrap_or(paged_parse::TextWrapMode::None);
            let prev_offsets = tw.map(|t| t.offsets).unwrap_or([0.0; 4]);
            let prev_invert = tw.and_then(|t| t.invert);
            *tw = Some(paged_parse::TextWrap {
                mode: prev_mode,
                offsets: new_offsets,
                invert: prev_invert,
            });
            (
                Value::Bounds(prev_offsets),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- SDK Phase 5 (v1 sweep) — whole-path replacement ----
        // Pathfinder's Subtract / Exclude (and any future op that
        // produces a fresh polygon set) drops in a new anchor list
        // in one shot. Inverse captures the prior anchors +
        // subpath_starts so undo round-trips bytewise. Targets any
        // path-bearing page item via the existing
        // `find_path_anchors_mut` helper.
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FramePath,
        ) => {
            let (new_anchors, new_subpath_starts) = match value {
                Value::FramePath {
                    anchors,
                    subpath_starts,
                } => (anchors.clone(), subpath_starts.clone()),
                _ => {
                    return Err(OperationError::TypeMismatch {
                        path,
                        expected: "FramePath".to_string(),
                    })
                }
            };
            let (anchors, starts, _open) = find_path_anchors_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev_anchors: Vec<crate::operation::PathAnchorSpec> = anchors
                .iter()
                .map(crate::operation::PathAnchorSpec::from_parse)
                .collect();
            let prev_starts: Vec<usize> = starts.clone();
            *anchors = new_anchors.iter().map(|a| a.to_parse()).collect();
            *starts = new_subpath_starts;
            (
                Value::FramePath {
                    anchors: prev_anchors,
                    subpath_starts: prev_starts,
                },
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- SDK Phase 5 (v1 sweep) — frame nonprinting toggle --
        // Excludes the frame from print/export passes; canvas
        // still renders it. v1 wires TextFrame + Rectangle; the
        // other kinds (Oval / Polygon / GraphicLine) also carry
        // the parsed field but their apply arms fall through to
        // UnsupportedProperty until they're added.
        (NodeId::TextFrame(id), PropertyPath::FrameNonprinting) => {
            let new_val = expect_bool(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.nonprinting;
            frame.nonprinting = new_val;
            (
                Value::Bool(prev),
                InvalidationHint {
                    structural: true,
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameNonprinting) => {
            let new_val = expect_bool(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.nonprinting;
            rect.nonprinting = new_val;
            (
                Value::Bool(prev),
                InvalidationHint {
                    structural: true,
                    ..Default::default()
                },
            )
        }
        // ---- SDK Phase 5 (v1 sweep) — frame fill tint percent --
        // Per-frame override on TextFrame + Rectangle. `None`
        // (Value::Length(None)) clears the tint, restoring the
        // swatch's full strength.
        (NodeId::TextFrame(id), PropertyPath::FrameFillTint) => {
            let new_val = expect_length(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.fill_tint;
            frame.fill_tint = new_val;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameFillTint) => {
            let new_val = expect_length(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.fill_tint;
            rect.fill_tint = new_val;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- Editor-ops — Page tool: page resize -----------------
        (NodeId::Page(id), PropertyPath::PageBounds) => {
            let new_bounds = expect_bounds(path, value)?;
            let page =
                find_page_mut(doc, id).ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = bounds_to_array(page.bounds);
            page.bounds = bounds_from_array(new_bounds);
            (
                Value::Bounds(prev),
                // Page geometry shifts every later spread origin —
                // a structural rebuild, not a per-frame repaint.
                InvalidationHint {
                    structural: true,
                    ..Default::default()
                },
            )
        }
        // ---- Editor-ops — Gradient Feather (whole-struct) ---------
        // Lines carry no fill, so the effect is meaningless there
        // (falls through to UnsupportedProperty).
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameGradientFeather,
        ) => {
            let new_spec = expect_gradient_feather(path, value)?;
            let effects = find_frame_effects_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = effects
                .gradient_feather
                .as_ref()
                .map(GradientFeatherSpec::from_parse);
            effects.gradient_feather = new_spec.map(|s| s.to_parse());
            (
                Value::GradientFeather(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- Editor-ops — gradient axis (Gradient Swatch tool) ----
        // One arm for all four angle/length fields across every
        // path-bearing kind; the field dispatch lives in
        // `find_gradient_field_mut`. Style-only invalidation — the
        // renderer re-reads the fields on the next rebuild.
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Polygon(_) | NodeId::Oval(_),
            PropertyPath::FrameGradientFillAngle
            | PropertyPath::FrameGradientFillLength
            | PropertyPath::FrameGradientStrokeAngle
            | PropertyPath::FrameGradientStrokeLength,
        ) => {
            let new_val = expect_length(path, value)?;
            let slot = find_gradient_field_mut(doc, node, path)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = *slot;
            *slot = new_val;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- SDK Phase 5 (v1 sweep) — drop-shadow per-field ----
        // Six fields on `frame.drop_shadow`. Each materialises a
        // default DropShadowSetting if the prior was `None`, then
        // mutates the named field. v1 wires TextFrame + Rectangle
        // (others fall through to UnsupportedProperty).
        (NodeId::TextFrame(_), PropertyPath::FrameDropShadowMode)
        | (NodeId::Rectangle(_), PropertyPath::FrameDropShadowMode) => {
            let new_val = expect_text(path, value)?;
            let ds = find_drop_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = ds.mode.clone();
            ds.mode = if new_val.is_empty() {
                "Drop".to_string()
            } else {
                new_val.clone()
            };
            (
                Value::Text(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(_), PropertyPath::FrameDropShadowXOffset)
        | (NodeId::Rectangle(_), PropertyPath::FrameDropShadowXOffset) => {
            let new_val = expect_length(path, value)?.unwrap_or(0.0);
            let ds = find_drop_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = ds.x_offset;
            ds.x_offset = new_val;
            (
                Value::Length(Some(prev)),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(_), PropertyPath::FrameDropShadowYOffset)
        | (NodeId::Rectangle(_), PropertyPath::FrameDropShadowYOffset) => {
            let new_val = expect_length(path, value)?.unwrap_or(0.0);
            let ds = find_drop_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = ds.y_offset;
            ds.y_offset = new_val;
            (
                Value::Length(Some(prev)),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(_), PropertyPath::FrameDropShadowSize)
        | (NodeId::Rectangle(_), PropertyPath::FrameDropShadowSize) => {
            let new_val = expect_length(path, value)?.unwrap_or(0.0);
            let ds = find_drop_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = ds.size;
            ds.size = new_val;
            (
                Value::Length(Some(prev)),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(_), PropertyPath::FrameDropShadowOpacity)
        | (NodeId::Rectangle(_), PropertyPath::FrameDropShadowOpacity) => {
            let new_val = expect_length(path, value)?.unwrap_or(100.0);
            let ds = find_drop_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = ds.opacity_pct;
            ds.opacity_pct = new_val;
            (
                Value::Length(Some(prev)),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(_), PropertyPath::FrameDropShadowColor)
        | (NodeId::Rectangle(_), PropertyPath::FrameDropShadowColor) => {
            let new_color = expect_color_ref(path, value)?;
            let ds = find_drop_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = ds.effect_color.clone();
            ds.effect_color = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- SDK Phase 5 (v1 sweep) — drop-shadow toggle ---------
        // TextFrame + Rectangle carry `drop_shadow: Option<...>`.
        // Toggle semantics: true → default DropShadowSetting when
        // prior was None (preserves existing custom shadow);
        // false → clear. Other kinds (Oval / Polygon / GraphicLine
        // also carry the field but the apply layer's helper map
        // doesn't reach them yet — they'd add a fan-out helper
        // like find_text_wrap_mut).
        (NodeId::TextFrame(id), PropertyPath::FrameDropShadow) => {
            let new_val = expect_bool(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.drop_shadow.is_some();
            frame.drop_shadow = if new_val {
                frame
                    .drop_shadow
                    .clone()
                    .or_else(|| Some(default_drop_shadow()))
            } else {
                None
            };
            (
                Value::Bool(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameDropShadow) => {
            let new_val = expect_bool(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.drop_shadow.is_some();
            rect.drop_shadow = if new_val {
                rect.drop_shadow
                    .clone()
                    .or_else(|| Some(default_drop_shadow()))
            } else {
                None
            };
            (
                Value::Bool(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- W0.4 — transparency effects (gap 18) ----------------
        // Per-field + per-effect-toggle editors for the non-DropShadow
        // effect blocks parsed onto `effects: Option<FrameEffects>`.
        // Each per-field arm materialises the effect block (and the
        // parent bag) with its InDesign-preset default if absent, then
        // mutates the named field. Each `*Enabled` toggle materialises
        // (true) / clears (false) the whole `Option<…Params>` — the
        // presence of the block is the enabled bit (the parser drops it
        // when `Applied="false"`), so this mirrors `FrameDropShadow`.
        // All paint-only → `frame_style`. Wired on TextFrame /
        // Rectangle / Oval (the kinds `find_frame_effects_mut`
        // reaches); other kinds fall through to UnsupportedProperty.

        // -- Inner shadow ------------------------------------------
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameInnerShadowEnabled,
        ) => {
            let new_val = expect_bool(path, value)?;
            let effects = find_frame_effects_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = effects.inner_shadow.is_some();
            effects.inner_shadow = if new_val {
                effects
                    .inner_shadow
                    .take()
                    .or_else(|| Some(default_inner_shadow()))
            } else {
                None
            };
            (Value::Bool(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameInnerShadowBlendMode,
        ) => {
            let new_val = expect_text(path, value)?;
            let e = find_inner_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.blend_mode.take();
            e.blend_mode = if new_val.is_empty() {
                None
            } else {
                Some(new_val)
            };
            (
                Value::Text(prev.unwrap_or_default()),
                frame_style_hint(node),
            )
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameInnerShadowColor,
        ) => {
            let new_color = expect_color_ref(path, value)?;
            let e = find_inner_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.effect_color.take();
            e.effect_color = new_color;
            (Value::ColorRef(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameInnerShadowOpacity
            | PropertyPath::FrameInnerShadowAngle
            | PropertyPath::FrameInnerShadowDistance
            | PropertyPath::FrameInnerShadowSize
            | PropertyPath::FrameInnerShadowChoke
            | PropertyPath::FrameInnerShadowNoise,
        ) => {
            let new_val = expect_length(path, value)?;
            let e = find_inner_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::FrameInnerShadowOpacity => &mut e.opacity_pct,
                PropertyPath::FrameInnerShadowAngle => &mut e.angle_deg,
                PropertyPath::FrameInnerShadowDistance => &mut e.distance,
                PropertyPath::FrameInnerShadowSize => &mut e.size,
                PropertyPath::FrameInnerShadowChoke => &mut e.choke_pct,
                _ => &mut e.noise_pct,
            };
            let prev = *slot;
            *slot = new_val;
            (Value::Length(prev), frame_style_hint(node))
        }

        // -- Outer glow --------------------------------------------
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameOuterGlowEnabled,
        ) => {
            let new_val = expect_bool(path, value)?;
            let effects = find_frame_effects_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = effects.outer_glow.is_some();
            effects.outer_glow = if new_val {
                effects
                    .outer_glow
                    .take()
                    .or_else(|| Some(default_outer_glow()))
            } else {
                None
            };
            (Value::Bool(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameOuterGlowBlendMode,
        ) => {
            let new_val = expect_text(path, value)?;
            let e = find_outer_glow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.blend_mode.take();
            e.blend_mode = if new_val.is_empty() {
                None
            } else {
                Some(new_val)
            };
            (
                Value::Text(prev.unwrap_or_default()),
                frame_style_hint(node),
            )
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameOuterGlowColor,
        ) => {
            let new_color = expect_color_ref(path, value)?;
            let e = find_outer_glow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.effect_color.take();
            e.effect_color = new_color;
            (Value::ColorRef(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameOuterGlowOpacity
            | PropertyPath::FrameOuterGlowSpread
            | PropertyPath::FrameOuterGlowSize
            | PropertyPath::FrameOuterGlowNoise,
        ) => {
            let new_val = expect_length(path, value)?;
            let e = find_outer_glow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::FrameOuterGlowOpacity => &mut e.opacity_pct,
                PropertyPath::FrameOuterGlowSpread => &mut e.spread_pct,
                PropertyPath::FrameOuterGlowSize => &mut e.size,
                _ => &mut e.noise_pct,
            };
            let prev = *slot;
            *slot = new_val;
            (Value::Length(prev), frame_style_hint(node))
        }

        // -- Inner glow --------------------------------------------
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameInnerGlowEnabled,
        ) => {
            let new_val = expect_bool(path, value)?;
            let effects = find_frame_effects_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = effects.inner_glow.is_some();
            effects.inner_glow = if new_val {
                effects
                    .inner_glow
                    .take()
                    .or_else(|| Some(default_inner_glow()))
            } else {
                None
            };
            (Value::Bool(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameInnerGlowBlendMode,
        ) => {
            let new_val = expect_text(path, value)?;
            let e = find_inner_glow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.blend_mode.take();
            e.blend_mode = if new_val.is_empty() {
                None
            } else {
                Some(new_val)
            };
            (
                Value::Text(prev.unwrap_or_default()),
                frame_style_hint(node),
            )
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameInnerGlowColor,
        ) => {
            let new_color = expect_color_ref(path, value)?;
            let e = find_inner_glow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.effect_color.take();
            e.effect_color = new_color;
            (Value::ColorRef(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameInnerGlowSource,
        ) => {
            let new_val = expect_text(path, value)?;
            let e = find_inner_glow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.source.take();
            e.source = if new_val.is_empty() {
                None
            } else {
                Some(new_val)
            };
            (
                Value::Text(prev.unwrap_or_default()),
                frame_style_hint(node),
            )
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameInnerGlowOpacity
            | PropertyPath::FrameInnerGlowChoke
            | PropertyPath::FrameInnerGlowSize
            | PropertyPath::FrameInnerGlowNoise,
        ) => {
            let new_val = expect_length(path, value)?;
            let e = find_inner_glow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::FrameInnerGlowOpacity => &mut e.opacity_pct,
                PropertyPath::FrameInnerGlowChoke => &mut e.choke_pct,
                PropertyPath::FrameInnerGlowSize => &mut e.size,
                _ => &mut e.noise_pct,
            };
            let prev = *slot;
            *slot = new_val;
            (Value::Length(prev), frame_style_hint(node))
        }

        // -- Bevel / emboss ----------------------------------------
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameBevelEnabled,
        ) => {
            let new_val = expect_bool(path, value)?;
            let effects = find_frame_effects_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = effects.bevel.is_some();
            effects.bevel = if new_val {
                effects.bevel.take().or_else(|| Some(default_bevel()))
            } else {
                None
            };
            (Value::Bool(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameBevelStyle
            | PropertyPath::FrameBevelTechnique
            | PropertyPath::FrameBevelDirection,
        ) => {
            let new_val = expect_text(path, value)?;
            let e = find_bevel_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::FrameBevelStyle => &mut e.style,
                PropertyPath::FrameBevelTechnique => &mut e.technique,
                _ => &mut e.direction,
            };
            let prev = slot.take();
            *slot = if new_val.is_empty() {
                None
            } else {
                Some(new_val)
            };
            (
                Value::Text(prev.unwrap_or_default()),
                frame_style_hint(node),
            )
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameBevelHighlightColor | PropertyPath::FrameBevelShadowColor,
        ) => {
            let new_color = expect_color_ref(path, value)?;
            let e = find_bevel_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::FrameBevelHighlightColor => &mut e.highlight_color,
                _ => &mut e.shadow_color,
            };
            let prev = slot.take();
            *slot = new_color;
            (Value::ColorRef(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameBevelDepth
            | PropertyPath::FrameBevelSize
            | PropertyPath::FrameBevelSoften
            | PropertyPath::FrameBevelAngle
            | PropertyPath::FrameBevelAltitude
            | PropertyPath::FrameBevelHighlightOpacity
            | PropertyPath::FrameBevelShadowOpacity,
        ) => {
            let new_val = expect_length(path, value)?;
            let e = find_bevel_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::FrameBevelDepth => &mut e.depth_pct,
                PropertyPath::FrameBevelSize => &mut e.size,
                PropertyPath::FrameBevelSoften => &mut e.soften,
                PropertyPath::FrameBevelAngle => &mut e.angle_deg,
                PropertyPath::FrameBevelAltitude => &mut e.altitude_deg,
                PropertyPath::FrameBevelHighlightOpacity => &mut e.highlight_opacity_pct,
                _ => &mut e.shadow_opacity_pct,
            };
            let prev = *slot;
            *slot = new_val;
            (Value::Length(prev), frame_style_hint(node))
        }

        // -- Satin -------------------------------------------------
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameSatinEnabled,
        ) => {
            let new_val = expect_bool(path, value)?;
            let effects = find_frame_effects_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = effects.satin.is_some();
            effects.satin = if new_val {
                effects.satin.take().or_else(|| Some(default_satin()))
            } else {
                None
            };
            (Value::Bool(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameSatinBlendMode,
        ) => {
            let new_val = expect_text(path, value)?;
            let e = find_satin_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.blend_mode.take();
            e.blend_mode = if new_val.is_empty() {
                None
            } else {
                Some(new_val)
            };
            (
                Value::Text(prev.unwrap_or_default()),
                frame_style_hint(node),
            )
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameSatinColor,
        ) => {
            let new_color = expect_color_ref(path, value)?;
            let e = find_satin_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.effect_color.take();
            e.effect_color = new_color;
            (Value::ColorRef(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameSatinInvert,
        ) => {
            let new_val = expect_bool(path, value)?;
            let e = find_satin_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.invert.unwrap_or(false);
            e.invert = Some(new_val);
            (Value::Bool(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameSatinOpacity
            | PropertyPath::FrameSatinAngle
            | PropertyPath::FrameSatinDistance
            | PropertyPath::FrameSatinSize,
        ) => {
            let new_val = expect_length(path, value)?;
            let e = find_satin_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::FrameSatinOpacity => &mut e.opacity_pct,
                PropertyPath::FrameSatinAngle => &mut e.angle_deg,
                PropertyPath::FrameSatinDistance => &mut e.distance,
                _ => &mut e.size,
            };
            let prev = *slot;
            *slot = new_val;
            (Value::Length(prev), frame_style_hint(node))
        }

        // -- Feather (basic) ---------------------------------------
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameFeatherEnabled,
        ) => {
            let new_val = expect_bool(path, value)?;
            let effects = find_frame_effects_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = effects.feather.is_some();
            effects.feather = if new_val {
                effects.feather.take().or_else(|| Some(default_feather()))
            } else {
                None
            };
            (Value::Bool(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameFeatherCornerType,
        ) => {
            let new_val = expect_text(path, value)?;
            let e = find_feather_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = e.corner_type.take();
            e.corner_type = if new_val.is_empty() {
                None
            } else {
                Some(new_val)
            };
            (
                Value::Text(prev.unwrap_or_default()),
                frame_style_hint(node),
            )
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameFeatherWidth
            | PropertyPath::FrameFeatherNoise
            | PropertyPath::FrameFeatherChoke,
        ) => {
            let new_val = expect_length(path, value)?;
            let e = find_feather_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::FrameFeatherWidth => &mut e.width,
                PropertyPath::FrameFeatherNoise => &mut e.noise_pct,
                _ => &mut e.choke_pct,
            };
            let prev = *slot;
            *slot = new_val;
            (Value::Length(prev), frame_style_hint(node))
        }

        // -- Directional feather -----------------------------------
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameDirectionalFeatherEnabled,
        ) => {
            let new_val = expect_bool(path, value)?;
            let effects = find_frame_effects_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = effects.directional_feather.is_some();
            effects.directional_feather = if new_val {
                effects
                    .directional_feather
                    .take()
                    .or_else(|| Some(default_directional_feather()))
            } else {
                None
            };
            (Value::Bool(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_),
            PropertyPath::FrameDirectionalFeatherLeftWidth
            | PropertyPath::FrameDirectionalFeatherRightWidth
            | PropertyPath::FrameDirectionalFeatherTopWidth
            | PropertyPath::FrameDirectionalFeatherBottomWidth
            | PropertyPath::FrameDirectionalFeatherAngle
            | PropertyPath::FrameDirectionalFeatherNoise
            | PropertyPath::FrameDirectionalFeatherChoke,
        ) => {
            let new_val = expect_length(path, value)?;
            let e = find_directional_feather_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::FrameDirectionalFeatherLeftWidth => &mut e.left_width,
                PropertyPath::FrameDirectionalFeatherRightWidth => &mut e.right_width,
                PropertyPath::FrameDirectionalFeatherTopWidth => &mut e.top_width,
                PropertyPath::FrameDirectionalFeatherBottomWidth => &mut e.bottom_width,
                PropertyPath::FrameDirectionalFeatherAngle => &mut e.angle_deg,
                PropertyPath::FrameDirectionalFeatherNoise => &mut e.noise_pct,
                _ => &mut e.choke_pct,
            };
            let prev = *slot;
            *slot = new_val;
            (Value::Length(prev), frame_style_hint(node))
        }

        // -- Object-level blend mode -------------------------------
        (NodeId::TextFrame(_) | NodeId::Rectangle(_), PropertyPath::FrameBlendMode) => {
            let new_val = expect_text(path, value)?;
            let slot = find_blend_mode_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = slot.take();
            *slot = if new_val.is_empty() {
                None
            } else {
                Some(new_val)
            };
            (
                Value::Text(prev.unwrap_or_default()),
                frame_style_hint(node),
            )
        }
        // ---- SDK Phase 5 (v1 sweep) — frame fitting (Rectangle) ----
        // The placed-image crop set + fitting-type enum live in
        // `Rectangle::frame_fitting: Option<FrameFittingOption>`.
        // Both apply arms materialise a default FrameFitting when
        // the prior was `None`, preserving the other half. Other
        // page-item kinds raise UnsupportedProperty.
        (NodeId::Rectangle(id), PropertyPath::FrameFittingCrops) => {
            let new_bounds = expect_bounds(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev_bounds = rect
                .frame_fitting
                .as_ref()
                .map(|f| {
                    [
                        f.top_crop.unwrap_or(0.0),
                        f.left_crop.unwrap_or(0.0),
                        f.bottom_crop.unwrap_or(0.0),
                        f.right_crop.unwrap_or(0.0),
                    ]
                })
                .unwrap_or([0.0; 4]);
            let prev_type = rect
                .frame_fitting
                .as_ref()
                .and_then(|f| f.fitting_on_empty_frame.clone());
            // Preserve the W0.3 alignment / auto-fit knobs across a
            // crop-only edit.
            let (prev_ref, prev_auto) = rect
                .frame_fitting
                .as_ref()
                .map(|f| (f.reference_point.clone(), f.auto_fit))
                .unwrap_or((None, None));
            rect.frame_fitting = Some(paged_parse::FrameFittingOption {
                top_crop: Some(new_bounds[0]),
                left_crop: Some(new_bounds[1]),
                bottom_crop: Some(new_bounds[2]),
                right_crop: Some(new_bounds[3]),
                fitting_on_empty_frame: prev_type,
                reference_point: prev_ref,
                auto_fit: prev_auto,
            });
            (
                Value::Bounds(prev_bounds),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameFittingType) => {
            let new_val = expect_text(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev_type = rect
                .frame_fitting
                .as_ref()
                .and_then(|f| f.fitting_on_empty_frame.clone())
                .unwrap_or_default();
            let (prev_top, prev_left, prev_bottom, prev_right) = rect
                .frame_fitting
                .as_ref()
                .map(|f| (f.top_crop, f.left_crop, f.bottom_crop, f.right_crop))
                .unwrap_or((None, None, None, None));
            let (prev_ref, prev_auto) = rect
                .frame_fitting
                .as_ref()
                .map(|f| (f.reference_point.clone(), f.auto_fit))
                .unwrap_or((None, None));
            let new_type = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            // Clearing all knobs leaves frame_fitting at `None`
            // for honest defaults; otherwise materialise the
            // FrameFitting with the merged state.
            if new_type.is_none()
                && prev_top.is_none()
                && prev_left.is_none()
                && prev_bottom.is_none()
                && prev_right.is_none()
                && prev_ref.is_none()
                && prev_auto.is_none()
            {
                rect.frame_fitting = None;
            } else {
                rect.frame_fitting = Some(paged_parse::FrameFittingOption {
                    top_crop: prev_top,
                    left_crop: prev_left,
                    bottom_crop: prev_bottom,
                    right_crop: prev_right,
                    fitting_on_empty_frame: new_type,
                    reference_point: prev_ref,
                    auto_fit: prev_auto,
                });
            }
            (
                Value::Text(prev_type),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- SDK Phase 5 (v1 sweep) — TextFrame inset spacing ----
        // Only TextFrame carries the inset_spacing field; other
        // page-item kinds fall through to the default
        // UnsupportedProperty arm.
        (NodeId::TextFrame(id), PropertyPath::FrameInsetSpacing) => {
            let new_bounds = expect_bounds(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.inset_spacing;
            frame.inset_spacing = Some(new_bounds);
            (
                // Inverse: a `None` prior round-trips as
                // `[0,0,0,0]`. A typed null-bounds wire variant would
                // distinguish "default" from "explicit zero"; for v1
                // the two are indistinguishable and the renderer
                // treats them the same.
                Value::Bounds(prev.unwrap_or([0.0; 4])),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- Phase F: ImageContentTransform -----------------------
        (NodeId::Rectangle(id), PropertyPath::ImageContentTransform) => {
            let new_transform = expect_transform(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.image_item_transform;
            rect.image_item_transform = new_transform;
            (
                Value::Transform(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (
            NodeId::StoryRange {
                story_id,
                start,
                end,
            },
            PropertyPath::CharacterFontSize
            | PropertyPath::CharacterLeading
            | PropertyPath::CharacterTracking
            | PropertyPath::CharacterFillColor
            | PropertyPath::AppliedCharacterStyle
            | PropertyPath::AppliedConditions
            | PropertyPath::CharacterFontFamily
            | PropertyPath::CharacterFontStyle
            | PropertyPath::CharacterKerningMethod
            | PropertyPath::CharacterCase
            | PropertyPath::CharacterPosition
            | PropertyPath::CharacterLanguage
            | PropertyPath::CharacterBaselineShift
            | PropertyPath::CharacterHorizontalScale
            | PropertyPath::CharacterVerticalScale
            | PropertyPath::CharacterSkew
            | PropertyPath::CharacterUnderline
            | PropertyPath::CharacterStrikethru
            | PropertyPath::CharacterLigatures
            | PropertyPath::CharacterOtfFeatures,
        ) => {
            return apply_character_property(doc, story_id, *start, *end, node, path, value);
        }
        // SDK Phase 5 (D3 completion) — applied object style on any
        // leaf page-item kind. The cascade resolves on next rebuild;
        // we only rewrite the per-item override ref here. Apply-an-
        // entity pattern: the wire shape is the same as a scalar
        // SetProperty, with Value::Text carrying the style's
        // `self_id`. Empty string clears the override (returns to
        // "[None]" in IDML terms). NodeId::Group is intentionally
        // excluded — IDML applies object styles to leaf items, not
        // structural containers.
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::AppliedObjectStyle,
        ) => {
            let new_val = expect_text(path, value)?;
            let field = find_applied_object_style_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = field.clone().unwrap_or_default();
            *field = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            (
                Value::Text(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (
            NodeId::StoryRange {
                story_id,
                start,
                end,
            },
            PropertyPath::ParagraphSpaceBefore
            | PropertyPath::ParagraphSpaceAfter
            | PropertyPath::ParagraphFirstLineIndent
            | PropertyPath::AppliedParagraphStyle
            | PropertyPath::ParagraphJustification
            | PropertyPath::ParagraphLeftIndent
            | PropertyPath::ParagraphRightIndent
            | PropertyPath::ParagraphDropCapCharacters
            | PropertyPath::ParagraphDropCapLines
            | PropertyPath::ParagraphHyphenation
            | PropertyPath::ParagraphKeepLinesTogether
            | PropertyPath::ParagraphKeepWithNext
            | PropertyPath::ParagraphRuleAbove
            | PropertyPath::ParagraphRuleBelow
            | PropertyPath::ParagraphTabStops
            | PropertyPath::ParagraphListType
            | PropertyPath::ParagraphBulletCharacter
            | PropertyPath::ParagraphNumberingFormat,
        ) => {
            return apply_paragraph_property(doc, story_id, *start, *end, node, path, value);
        }

        // ============ W0.3 — text-frame prefs (TextFrame only) ========
        (NodeId::TextFrame(id), PropertyPath::TextFrameColumnCount) => {
            let new_val = expect_length(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.column_count;
            frame.column_count = new_val.map(|n| n.max(1.0).round() as u32);
            (
                Value::Length(prev.map(|c| c as f32)),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(id), PropertyPath::TextFrameColumnGutter) => {
            let new_val = expect_length(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.column_gutter;
            frame.column_gutter = new_val;
            (
                Value::Length(prev),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(id), PropertyPath::TextFrameColumnBalance) => {
            let new_val = expect_bool(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame.column_balance.unwrap_or(false);
            frame.column_balance = Some(new_val);
            (
                Value::Bool(prev),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(id), PropertyPath::TextFrameVerticalJustification) => {
            let new_val = expect_text(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame
                .vertical_justification
                .map(vj_as_idml)
                .unwrap_or_default();
            frame.vertical_justification = if new_val.is_empty() {
                None
            } else {
                paged_parse::VerticalJustification::from_idml(&new_val)
            };
            (
                Value::Text(prev.to_string()),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(id), PropertyPath::TextFrameAutoSizing) => {
            let new_val = expect_text(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame
                .auto_sizing
                .map(auto_sizing_as_idml)
                .unwrap_or_default();
            frame.auto_sizing = if new_val.is_empty() {
                None
            } else {
                paged_parse::AutoSizingType::from_idml(&new_val)
            };
            (
                Value::Text(prev.to_string()),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::TextFrame(id), PropertyPath::TextFrameFirstBaseline) => {
            let new_val = expect_text(path, value)?;
            let frame = find_text_frame_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = frame
                .first_baseline_offset
                .map(first_baseline_as_idml)
                .unwrap_or_default();
            frame.first_baseline_offset = if new_val.is_empty() {
                None
            } else {
                paged_parse::FirstBaselineOffset::from_idml(&new_val)
            };
            (
                Value::Text(prev.to_string()),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }

        // ============ W0.3 — text-wrap invert (all wrap kinds) ========
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::TextWrapInvert,
        ) => {
            let new_val = expect_bool(path, value)?;
            let tw = find_text_wrap_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = tw.and_then(|t| t.invert).unwrap_or(false);
            let prev_mode = tw
                .map(|t| t.mode)
                .unwrap_or(paged_parse::TextWrapMode::None);
            let prev_offsets = tw.map(|t| t.offsets).unwrap_or([0.0; 4]);
            *tw = Some(paged_parse::TextWrap {
                mode: prev_mode,
                offsets: prev_offsets,
                invert: Some(new_val),
            });
            (
                Value::Bool(prev),
                // The wrap exclusion changes; other frames reflow.
                InvalidationHint {
                    structural: true,
                    ..Default::default()
                },
            )
        }

        // ============ W0.3 — frame fitting (Rectangle only) ===========
        (NodeId::Rectangle(id), PropertyPath::FrameFittingReferencePoint) => {
            let new_val = expect_text(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let ff = rect.frame_fitting.get_or_insert_with(Default::default);
            let prev = ff.reference_point.clone().unwrap_or_default();
            ff.reference_point = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            (
                Value::Text(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameAutoFit) => {
            let new_val = expect_bool(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let ff = rect.frame_fitting.get_or_insert_with(Default::default);
            let prev = ff.auto_fit.unwrap_or(false);
            ff.auto_fit = Some(new_val);
            (
                Value::Bool(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }

        // ============ W0.3 — stroke type / gap (all stroked kinds) ====
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameStrokeType,
        ) => {
            let new_val = expect_text(path, value)?;
            let slot = find_stroke_type_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = slot.clone().unwrap_or_default();
            *slot = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            (
                Value::Text(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameStrokeGapColor,
        ) => {
            let new_color = expect_color_ref(path, value)?;
            let slot = find_stroke_gap_color_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = slot.clone();
            *slot = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameStrokeGapTint,
        ) => {
            let new_val = expect_length(path, value)?;
            let slot = find_stroke_gap_tint_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = *slot;
            *slot = new_val;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // Stroke join / miter / alignment are Rectangle-only parse
        // fields.
        (NodeId::Rectangle(id), PropertyPath::FrameStrokeJoin) => {
            let new_val = expect_text(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.end_join.clone().unwrap_or_default();
            rect.end_join = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            (
                Value::Text(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameStrokeAlignment) => {
            let new_val = expect_text(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.stroke_alignment.clone().unwrap_or_default();
            rect.stroke_alignment = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            (
                Value::Text(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Rectangle(id), PropertyPath::FrameStrokeMiterLimit) => {
            let new_val = expect_length(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = rect.miter_limit;
            rect.miter_limit = new_val;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }

        // ============ W0.3 — per-corner option + radius (Rectangle) ===
        (
            NodeId::Rectangle(id),
            PropertyPath::FrameCornerOptionTopLeft
            | PropertyPath::FrameCornerOptionTopRight
            | PropertyPath::FrameCornerOptionBottomLeft
            | PropertyPath::FrameCornerOptionBottomRight,
        ) => {
            let new_val = expect_text(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let i = corner_index(path);
            let prev = rect.corners[i]
                .option
                .map(corner_option_as_idml)
                .unwrap_or_default();
            rect.corners[i].option = if new_val.is_empty() {
                None
            } else {
                paged_parse::CornerOption::from_idml(&new_val)
            };
            (
                Value::Text(prev.to_string()),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (
            NodeId::Rectangle(id),
            PropertyPath::FrameCornerRadiusTopLeft
            | PropertyPath::FrameCornerRadiusTopRight
            | PropertyPath::FrameCornerRadiusBottomLeft
            | PropertyPath::FrameCornerRadiusBottomRight,
        ) => {
            let new_val = expect_length(path, value)?;
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let i = corner_index(path);
            let prev = rect.corners[i].radius;
            rect.corners[i].radius = new_val;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }

        // ============ W0.3 — transform decompose (all path kinds) =====
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_)
            | NodeId::Group(_),
            PropertyPath::FrameRotationAngle
            | PropertyPath::FrameScaleX
            | PropertyPath::FrameScaleY
            | PropertyPath::FrameFlipH
            | PropertyPath::FrameFlipV,
        ) => {
            let slot = find_item_transform_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let mut d = crate::operation::decompose_transform(*slot);
            let prev_value = match path {
                PropertyPath::FrameRotationAngle => {
                    let prev = Value::Length(Some(d.angle_deg));
                    d.angle_deg = expect_length(path, value)?.unwrap_or(0.0);
                    prev
                }
                PropertyPath::FrameScaleX => {
                    let prev = Value::Length(Some(d.scale_x));
                    d.scale_x = expect_length(path, value)?.unwrap_or(1.0);
                    prev
                }
                PropertyPath::FrameScaleY => {
                    let prev = Value::Length(Some(d.scale_y));
                    d.scale_y = expect_length(path, value)?.unwrap_or(1.0);
                    prev
                }
                PropertyPath::FrameFlipH => {
                    let prev = Value::Bool(d.flip_h);
                    d.flip_h = expect_bool(path, value)?;
                    prev
                }
                PropertyPath::FrameFlipV => {
                    let prev = Value::Bool(d.flip_v);
                    d.flip_v = expect_bool(path, value)?;
                    prev
                }
                _ => unreachable!("guarded by the match pattern"),
            };
            *slot = Some(crate::operation::recompose_transform(&d));
            (
                prev_value,
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }

        // ============ W0.3 — overprint (fill: all fills; stroke: all) ==
        (
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_) | NodeId::Polygon(_),
            PropertyPath::FrameOverprintFill,
        ) => {
            let new_val = expect_bool(path, value)?;
            let slot = find_overprint_fill_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = *slot;
            *slot = new_val;
            (
                Value::Bool(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameOverprintStroke,
        ) => {
            let new_val = expect_bool(path, value)?;
            let slot = find_overprint_stroke_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = *slot;
            *slot = new_val;
            (
                Value::Bool(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }

        _ => {
            return Err(OperationError::UnsupportedProperty {
                node: node.clone(),
                path,
            })
        }
    };

    let inverse = invert_set_property(node.clone(), path, previous);
    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path,
            value: value.clone(),
        },
        inverse,
        invalidation,
    })
}

// ---------------------------------------------------------------------------
// SDK Phase 3 — character properties addressed by `NodeId::StoryRange`
// ---------------------------------------------------------------------------
//
// The forward op walks `doc.stories[story_id].story.paragraphs`,
// computing the running character offset across all `CharacterRun.text`
// fields in order. Runs whose `[run_start, run_end)` intersect
// `[start, end)` receive the new property value; an inverse `Batch`
// of restorations is built per affected run.
//
// Constraint (this commit): the range must align with whole-run
// boundaries. If `start` or `end` cuts inside a `CharacterRun.text`,
// the apply returns `OperationError::Unimplemented`. Run-splitting
// at arbitrary character offsets is a Phase 3.x follow-up — it
// needs a story-snapshot inverse strategy (clone the affected
// paragraphs' run lists pre-mutation, restore on undo) to round-
// trip bytewise, which in turn needs `CharacterRun` to derive
// Deserialize/PartialEq/Tsify. Out of scope for this commit;
// today's editor-binding-flow can target catalog-bound writes that
// already snap to run boundaries.

fn apply_character_property(
    doc: &mut Document,
    story_id: &str,
    start: u32,
    end: u32,
    node: &NodeId,
    path: PropertyPath,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    if start >= end {
        return Err(OperationError::InvalidValue {
            node: node.clone(),
            path,
            reason: format!("empty range: start={start} >= end={end}"),
        });
    }

    let story_idx = doc
        .stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;

    let story = &mut doc.stories[story_idx].story;
    let mut inverse_ops: Vec<Operation> = Vec::new();
    let mut char_offset: u32 = 0;

    // Per-paragraph walk. Runs that intersect [start, end) are
    // split as needed and the "middle" piece (the one fully inside
    // [start, end)) receives the new property value. Inverse is a
    // Batch of per-(now-split-)run SetProperty restorations
    // addressed at the post-split range — undo restores each
    // affected run's previous value without re-merging the splits.
    // A future "merge consecutive runs with identical properties"
    // pass can canonicalize the document; today's correctness is
    // bytewise even with extra boundaries.
    for para in story.paragraphs.iter_mut() {
        let para_chars: u32 = para
            .runs
            .iter()
            .map(|r| r.text.chars().count() as u32)
            .sum();
        let para_start = char_offset;
        let para_end = char_offset + para_chars;
        char_offset = para_end;

        // Skip paragraphs entirely outside [start, end).
        if para_end <= start || para_start >= end {
            continue;
        }

        // Rebuild this paragraph's runs vec, splitting as needed.
        let original_runs: Vec<paged_parse::CharacterRun> = para.runs.drain(..).collect();
        let mut new_runs: Vec<paged_parse::CharacterRun> =
            Vec::with_capacity(original_runs.len() * 2);
        let mut local_offset: u32 = 0;

        for run in original_runs {
            let run_len = run.text.chars().count() as u32;
            let run_start = para_start + local_offset;
            let run_end = run_start + run_len;
            local_offset += run_len;

            let intersects = run_end > start && run_start < end;
            if !intersects {
                new_runs.push(run);
                continue;
            }

            // Local split offsets within the run (in characters):
            // - left split at `local_left` if run starts BEFORE the
            //   requested range — everything before it stays as the
            //   pre-mutation value.
            // - right split at `local_right` if run ends AFTER the
            //   range — everything past it stays as well.
            let local_left = if run_start < start {
                Some(start - run_start)
            } else {
                None
            };
            let local_right = if run_end > end {
                Some(end - run_start)
            } else {
                None
            };

            match (local_left, local_right) {
                (None, None) => {
                    // Whole run in range. Mutate in place.
                    let mut mutated = run;
                    let (prev_value, _new_set) =
                        apply_character_field_on_run(&mut mutated, path, value)?;
                    inverse_ops.push(Operation::SetProperty {
                        node: NodeId::StoryRange {
                            story_id: story_id.to_string(),
                            start: run_start,
                            end: run_end,
                        },
                        path,
                        value: prev_value,
                    });
                    new_runs.push(mutated);
                }
                (Some(split_at), None) => {
                    // Run starts before the range; one split at
                    // `start`. Left piece stays; right piece gets
                    // mutated.
                    let (left, mut right) = split_run_at(run, split_at);
                    let mid_start = run_start + split_at;
                    let mid_end = run_end;
                    let (prev_value, _) = apply_character_field_on_run(&mut right, path, value)?;
                    inverse_ops.push(Operation::SetProperty {
                        node: NodeId::StoryRange {
                            story_id: story_id.to_string(),
                            start: mid_start,
                            end: mid_end,
                        },
                        path,
                        value: prev_value,
                    });
                    new_runs.push(left);
                    new_runs.push(right);
                }
                (None, Some(split_at)) => {
                    // Run ends after the range; one split at `end`.
                    // Left piece gets mutated; right piece stays.
                    let (mut left, right) = split_run_at(run, split_at);
                    let mid_start = run_start;
                    let mid_end = run_start + split_at;
                    let (prev_value, _) = apply_character_field_on_run(&mut left, path, value)?;
                    inverse_ops.push(Operation::SetProperty {
                        node: NodeId::StoryRange {
                            story_id: story_id.to_string(),
                            start: mid_start,
                            end: mid_end,
                        },
                        path,
                        value: prev_value,
                    });
                    new_runs.push(left);
                    new_runs.push(right);
                }
                (Some(left_at), Some(right_at)) => {
                    // Run straddles both ends of the range; two
                    // splits — three pieces. Middle gets mutated.
                    let (left, rest) = split_run_at(run, left_at);
                    let (mut mid, right) = split_run_at(rest, right_at - left_at);
                    let mid_start = run_start + left_at;
                    let mid_end = run_start + right_at;
                    let (prev_value, _) = apply_character_field_on_run(&mut mid, path, value)?;
                    inverse_ops.push(Operation::SetProperty {
                        node: NodeId::StoryRange {
                            story_id: story_id.to_string(),
                            start: mid_start,
                            end: mid_end,
                        },
                        path,
                        value: prev_value,
                    });
                    new_runs.push(left);
                    new_runs.push(mid);
                    new_runs.push(right);
                }
            }
        }

        para.runs = new_runs;
    }

    if inverse_ops.is_empty() {
        // No runs in the range — empty story or pre/post the
        // entire content. Return a no-op AppliedOperation so the
        // caller's undo stack stays consistent.
        return Ok(AppliedOperation {
            op: Operation::SetProperty {
                node: node.clone(),
                path,
                value: value.clone(),
            },
            inverse: Operation::SetProperty {
                node: node.clone(),
                path,
                value: value.clone(),
            },
            invalidation: InvalidationHint::default(),
        });
    }

    // Build an InvalidationHint targeting the host text frame so the
    // renderer's cache invalidates the right page. Most character
    // properties remeasure the line (font / size / scale / kerning /
    // ligatures …), so they invalidate `text_reflow`. Paint-only
    // decorations (underline / strikethru) don't change the line's
    // geometry — they invalidate `frame_style` instead so the
    // renderer repaints without re-running layout. The story-to-frame
    // index is built at document open; if it's empty (shouldn't happen
    // for parsed docs) we leave the hint default.
    let paint_only = matches!(
        path,
        PropertyPath::CharacterUnderline | PropertyPath::CharacterStrikethru
    );
    let invalidation = match doc.frame_for_story.get(story_id) {
        Some(frame) => {
            if let Some(self_id) = &frame.self_id {
                let frame_node = NodeId::TextFrame(self_id.clone());
                if paint_only {
                    InvalidationHint {
                        frame_style: vec![frame_node],
                        ..Default::default()
                    }
                } else {
                    InvalidationHint {
                        text_reflow: vec![frame_node],
                        ..Default::default()
                    }
                }
            } else {
                InvalidationHint::default()
            }
        }
        None => InvalidationHint::default(),
    };

    // The forward op's recorded form is the original (caller-provided)
    // node/path/value. The inverse is a Batch of per-run restorations
    // — even if there's only one affected run, wrapping in Batch keeps
    // the inverse shape stable across the cardinality of the range.
    let inverse = if inverse_ops.len() == 1 {
        inverse_ops.into_iter().next().unwrap()
    } else {
        Operation::Batch { ops: inverse_ops }
    };

    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path,
            value: value.clone(),
        },
        inverse,
        invalidation,
    })
}

// ---------------------------------------------------------------------------
// SDK Phase 3 — paragraph properties addressed by `NodeId::StoryRange`
// ---------------------------------------------------------------------------
//
// Paragraphs are atomic: you can't half-apply `ParagraphSpaceBefore`
// to the middle of a paragraph. The apply layer walks `story.paragraphs`,
// finds every paragraph whose `[para_start, para_end)` intersects the
// requested `[start, end)`, and writes the property to each. Inverse
// is a `Batch` of per-paragraph SetProperty restorations addressed
// at each paragraph's full range — undo applies them in order to
// restore prior values without needing to know the original input
// range. Paragraph boundaries are NOT split (unlike CharacterRuns) —
// the apply layer rounds the range to whole paragraphs by treating
// intersection as the trigger.

fn apply_paragraph_property(
    doc: &mut Document,
    story_id: &str,
    start: u32,
    end: u32,
    node: &NodeId,
    path: PropertyPath,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    if start >= end {
        return Err(OperationError::InvalidValue {
            node: node.clone(),
            path,
            reason: format!("empty range: start={start} >= end={end}"),
        });
    }

    let story_idx = doc
        .stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;

    let story = &mut doc.stories[story_idx].story;
    let mut inverse_ops: Vec<Operation> = Vec::new();
    let mut char_offset: u32 = 0;

    for para in story.paragraphs.iter_mut() {
        let para_chars: u32 = para
            .runs
            .iter()
            .map(|r| r.text.chars().count() as u32)
            .sum();
        let para_start = char_offset;
        let para_end = char_offset + para_chars;
        char_offset = para_end;

        // Skip paragraphs entirely outside [start, end).
        if para_end <= start || para_start >= end {
            continue;
        }

        let (prev_value, _new_set) = apply_paragraph_field(para, path, value)?;
        inverse_ops.push(Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: story_id.to_string(),
                start: para_start,
                end: para_end,
            },
            path,
            value: prev_value,
        });
    }

    if inverse_ops.is_empty() {
        return Ok(AppliedOperation {
            op: Operation::SetProperty {
                node: node.clone(),
                path,
                value: value.clone(),
            },
            inverse: Operation::SetProperty {
                node: node.clone(),
                path,
                value: value.clone(),
            },
            invalidation: InvalidationHint::default(),
        });
    }

    let invalidation = match doc.frame_for_story.get(story_id) {
        Some(frame) => {
            if let Some(self_id) = &frame.self_id {
                InvalidationHint {
                    text_reflow: vec![NodeId::TextFrame(self_id.clone())],
                    ..Default::default()
                }
            } else {
                InvalidationHint::default()
            }
        }
        None => InvalidationHint::default(),
    };

    let inverse = if inverse_ops.len() == 1 {
        inverse_ops.into_iter().next().unwrap()
    } else {
        Operation::Batch { ops: inverse_ops }
    };

    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path,
            value: value.clone(),
        },
        inverse,
        invalidation,
    })
}

/// W0.2 — set one `Option<f32>` field on a `Paragraph` from a
/// `Value::Length`. `Length(None)` clears the override; the captured
/// prior `Option<f32>` round-trips bytewise through the inverse.
/// Paragraph-scope analogue of `set_run_length_field`.
fn set_para_length_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut Option<f32>,
) -> Result<(Value, Value), OperationError> {
    let Value::Length(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Length".to_string(),
        });
    };
    let prev = *slot;
    *slot = *new_val;
    Ok((Value::Length(prev), Value::Length(*new_val)))
}

/// W0.2 — set one `u32` count field on a `Paragraph` from a
/// `Value::Length` carrying the integer (the inspector's
/// integer-as-Length convention). `Length(None)` ⇒ 0. The captured
/// prior is returned as `Value::Length(Some(prev as f32))` so the
/// inverse round-trips bytewise. `field` is a non-`Option` `u32`
/// (the drop-cap counts default to 0, not `None`).
fn set_para_u32_length_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut u32,
) -> Result<(Value, Value), OperationError> {
    let Value::Length(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Length".to_string(),
        });
    };
    let prev = *slot;
    // Round defensively; counts are authored as whole numbers but the
    // wire carries f32. Negative / NaN clamps to 0.
    *slot = new_val.map(|n| n.max(0.0).round() as u32).unwrap_or(0);
    Ok((
        Value::Length(Some(prev as f32)),
        Value::Length(Some(*slot as f32)),
    ))
}

/// W0.2 — set one `Option<u32>` count field on a `Paragraph` from a
/// `Value::Length` carrying the integer. `Length(None)` clears the
/// override. The captured prior `Option<u32>` round-trips bytewise.
fn set_para_opt_u32_length_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut Option<u32>,
) -> Result<(Value, Value), OperationError> {
    let Value::Length(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Length".to_string(),
        });
    };
    let prev = *slot;
    *slot = new_val.map(|n| n.max(0.0).round() as u32);
    Ok((
        Value::Length(prev.map(|n| n as f32)),
        Value::Length(*new_val),
    ))
}

/// W0.2 — set one `Option<bool>` field on a `Paragraph` from a
/// `Value::Bool`. The write always stores `Some(new_val)`. The
/// inverse captures `prev.unwrap_or(default_when_none)` — a write
/// over an explicit prior round-trips bytewise; a prior-`None` undoes
/// to `Some(default_when_none)` (the `Value::Bool` wire shape carries
/// no `None`). Paragraph-scope analogue of `set_run_bool_field`, with
/// an explicit default so each toggle restores its own IDML default.
fn set_para_bool_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut Option<bool>,
    default_when_none: bool,
) -> Result<(Value, Value), OperationError> {
    let Value::Bool(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Bool".to_string(),
        });
    };
    let prev = slot.unwrap_or(default_when_none);
    *slot = Some(*new_val);
    Ok((Value::Bool(prev), Value::Bool(*new_val)))
}

/// W0.2 — set one `Option<String>` field on a `Paragraph` from a
/// `Value::Text`. The empty string clears the override (`None`); the
/// captured prior is returned as `Value::Text` (`None ⇒ ""`).
/// Paragraph-scope analogue of `set_run_text_field`.
fn set_para_text_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut Option<String>,
) -> Result<(Value, Value), OperationError> {
    let Value::Text(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Text".to_string(),
        });
    };
    let prev = slot.clone().unwrap_or_default();
    *slot = if new_val.is_empty() {
        None
    } else {
        Some(new_val.clone())
    };
    Ok((Value::Text(prev), Value::Text(new_val.clone())))
}

/// W0.2 — set the whole `ParagraphRule` struct (`rule_above` /
/// `rule_below`) from a `Value::ParagraphRule`. `ParagraphRule(None)`
/// clears the rule to the all-`None` default. The captured prior is
/// returned as a `Value::ParagraphRule(Some(prior))` so the inverse
/// round-trips the rule bytewise. Whole-struct analogue of the
/// `FrameGradientFeather` apply.
fn set_para_rule_field(
    path: PropertyPath,
    value: &Value,
    slot: &mut paged_parse::styles::ParagraphRule,
) -> Result<(Value, Value), OperationError> {
    let Value::ParagraphRule(new_spec) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "ParagraphRule".to_string(),
        });
    };
    let prev = crate::operation::ParagraphRuleSpec::from_parse(slot);
    *slot = match new_spec {
        Some(spec) => spec.to_parse(),
        None => paged_parse::styles::ParagraphRule::default(),
    };
    Ok((
        Value::ParagraphRule(Some(prev)),
        Value::ParagraphRule(new_spec.clone()),
    ))
}

fn apply_paragraph_field(
    para: &mut paged_parse::Paragraph,
    path: PropertyPath,
    value: &Value,
) -> Result<(Value, Value), OperationError> {
    match path {
        PropertyPath::ParagraphSpaceBefore => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = para.space_before;
            para.space_before = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::ParagraphSpaceAfter => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = para.space_after;
            para.space_after = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::ParagraphFirstLineIndent => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = para.first_line_indent;
            para.first_line_indent = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::AppliedParagraphStyle => {
            // Apply-an-entity. Empty string clears the override.
            let Value::Text(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Text".to_string(),
                });
            };
            let prev = para.paragraph_style.clone().unwrap_or_default();
            para.paragraph_style = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
        }
        PropertyPath::ParagraphJustification => {
            // SDK Phase 5 (v1 sweep) — paragraph alignment via the
            // IDML attribute string. Empty value clears the override
            // (`None` ⇒ inherit from style cascade); non-empty parses
            // through `Justification::from_idml` and stores. Unknown
            // strings raise `InvalidValue` (the toggle-group primitive
            // ensures the UI never emits an unknown value).
            let Value::Text(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Text".to_string(),
                });
            };
            let prev = para
                .justification
                .map(|j| j.as_idml().to_string())
                .unwrap_or_default();
            para.justification = if new_val.is_empty() {
                None
            } else {
                match paged_parse::Justification::from_idml(new_val) {
                    Some(j) => Some(j),
                    None => {
                        return Err(OperationError::InvalidValue {
                            node: NodeId::StoryRange {
                                story_id: String::new(),
                                start: 0,
                                end: 0,
                            },
                            path,
                            reason: format!("unknown Justification: {new_val:?}"),
                        });
                    }
                }
            };
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
        }
        // W0.2 — paragraph indents. `Value::Length(None)` clears the
        // per-paragraph override (inherit from the cascade).
        PropertyPath::ParagraphLeftIndent => {
            set_para_length_field(path, value, &mut para.left_indent)
        }
        PropertyPath::ParagraphRightIndent => {
            set_para_length_field(path, value, &mut para.right_indent)
        }
        // W0.2 — drop-cap counts. The run fields are non-`Option`
        // `u32` (0 ⇒ no drop cap), carried on the wire as
        // integer-Length.
        PropertyPath::ParagraphDropCapCharacters => {
            set_para_u32_length_field(path, value, &mut para.drop_cap_characters)
        }
        PropertyPath::ParagraphDropCapLines => {
            set_para_u32_length_field(path, value, &mut para.drop_cap_lines)
        }
        // W0.2 — keep-with-next is an `Option<u32>` line count.
        PropertyPath::ParagraphKeepWithNext => {
            set_para_opt_u32_length_field(path, value, &mut para.keep_with_next)
        }
        // W0.2 — boolean toggles. Each restores its own IDML default
        // on a prior-`None` undo: hyphenation defaults true,
        // keep-lines-together defaults false.
        PropertyPath::ParagraphHyphenation => {
            set_para_bool_field(path, value, &mut para.hyphenation, true)
        }
        PropertyPath::ParagraphKeepLinesTogether => {
            set_para_bool_field(path, value, &mut para.keep_lines_together, false)
        }
        // W0.2 — whole rule structs.
        PropertyPath::ParagraphRuleAbove => set_para_rule_field(path, value, &mut para.rule_above),
        PropertyPath::ParagraphRuleBelow => set_para_rule_field(path, value, &mut para.rule_below),
        // W0.2 — whole `<TabList>` replacement. The captured prior is
        // returned as a `Value::TabStops` so the inverse restores the
        // exact prior stop list (bytewise round-trip).
        PropertyPath::ParagraphTabStops => {
            let Value::TabStops(new_stops) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "TabStops".to_string(),
                });
            };
            let prev: Vec<crate::operation::TabStopSpec> = para
                .tab_list
                .iter()
                .map(crate::operation::TabStopSpec::from_parse)
                .collect();
            para.tab_list = new_stops.iter().map(|s| s.to_parse()).collect();
            Ok((Value::TabStops(prev), Value::TabStops(new_stops.clone())))
        }
        // W0.2 — bullets / numbering list type. Stored verbatim as the
        // IDML enum string; empty clears the override.
        PropertyPath::ParagraphListType => {
            set_para_text_field(path, value, &mut para.bullets_list_type)
        }
        // W0.2 — bullet glyph. The wire carries the glyph character
        // (`Value::Text`); the run field is a `u32` codepoint. The
        // empty string clears the override; a multi-char string takes
        // the first scalar. The inverse re-encodes the prior codepoint
        // back to its glyph (a prior-`None` round-trips to `""`).
        PropertyPath::ParagraphBulletCharacter => {
            let Value::Text(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Text".to_string(),
                });
            };
            let prev = para
                .bullet_character
                .and_then(char::from_u32)
                .map(|c| c.to_string())
                .unwrap_or_default();
            para.bullet_character = new_val.chars().next().map(|c| c as u32);
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
        }
        // W0.2 — numbering-format expression. Stored verbatim; empty
        // clears the override.
        PropertyPath::ParagraphNumberingFormat => {
            set_para_text_field(path, value, &mut para.numbering_format)
        }
        _ => Err(OperationError::UnsupportedProperty {
            node: NodeId::StoryRange {
                story_id: String::new(),
                start: 0,
                end: 0,
            },
            path,
        }),
    }
}

/// SDK Phase 3.x — split a `CharacterRun` at character offset
/// `char_idx`. The left piece contains the first `char_idx`
/// characters of `run.text`; the right piece contains the rest.
/// Every other field is duplicated via `Clone` so the two pieces
/// inherit identical properties pre-mutation. `char_idx` must lie
/// strictly inside the run (0 < char_idx < run.text.chars().count()) —
/// the caller is responsible for that constraint; this function
/// produces undefined byte boundaries otherwise.
fn split_run_at(
    run: paged_parse::CharacterRun,
    char_idx: u32,
) -> (paged_parse::CharacterRun, paged_parse::CharacterRun) {
    // Find the byte position of the char_idx'th character. char_indices
    // yields each char's byte offset; chars past the end map to the
    // string's total byte length.
    let byte_idx = run
        .text
        .char_indices()
        .nth(char_idx as usize)
        .map(|(byte, _)| byte)
        .unwrap_or(run.text.len());
    let left_text = run.text[..byte_idx].to_string();
    let right_text = run.text[byte_idx..].to_string();
    let mut left = run.clone();
    left.text = left_text;
    let mut right = run;
    right.text = right_text;
    (left, right)
}

/// W0.1 — set one `Option<String>` field on a `CharacterRun` from a
/// `Value::Text`. The empty string clears the override (`None`); the
/// captured prior is returned as `Value::Text` (`None ⇒ ""`) so the
/// inverse re-applies the prior string and round-trips a prior-`None`
/// back to `None`. `field` selects the run field by `&mut` reference.
fn set_run_text_field(
    run: &mut paged_parse::CharacterRun,
    path: PropertyPath,
    value: &Value,
    field: impl FnOnce(&mut paged_parse::CharacterRun) -> &mut Option<String>,
) -> Result<(Value, Value), OperationError> {
    let Value::Text(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Text".to_string(),
        });
    };
    let slot = field(run);
    let prev = slot.clone().unwrap_or_default();
    *slot = if new_val.is_empty() {
        None
    } else {
        Some(new_val.clone())
    };
    Ok((Value::Text(prev), Value::Text(new_val.clone())))
}

/// W0.1 — set one `Option<f32>` field on a `CharacterRun` from a
/// `Value::Length`. `Length(None)` clears the override; the captured
/// prior `Option<f32>` round-trips bytewise through the inverse.
fn set_run_length_field(
    run: &mut paged_parse::CharacterRun,
    path: PropertyPath,
    value: &Value,
    field: impl FnOnce(&mut paged_parse::CharacterRun) -> &mut Option<f32>,
) -> Result<(Value, Value), OperationError> {
    let Value::Length(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Length".to_string(),
        });
    };
    let slot = field(run);
    let prev = *slot;
    *slot = *new_val;
    Ok((Value::Length(prev), Value::Length(*new_val)))
}

/// W0.1 — set one `Option<bool>` field on a `CharacterRun` from a
/// `Value::Bool`. The write always stores `Some(new_val)`. The
/// inverse captures `prev.unwrap_or(false)` — a write over an
/// explicit prior round-trips bytewise; a prior-`None` undoes to
/// `Some(false)` (the `Value::Bool` wire shape carries no `None`).
fn set_run_bool_field(
    run: &mut paged_parse::CharacterRun,
    path: PropertyPath,
    value: &Value,
    field: impl FnOnce(&mut paged_parse::CharacterRun) -> &mut Option<bool>,
) -> Result<(Value, Value), OperationError> {
    let Value::Bool(new_val) = value else {
        return Err(OperationError::TypeMismatch {
            path,
            expected: "Bool".to_string(),
        });
    };
    let slot = field(run);
    let prev = slot.unwrap_or(false);
    *slot = Some(*new_val);
    Ok((Value::Bool(prev), Value::Bool(*new_val)))
}

/// Apply one character property to one `CharacterRun`. Returns
/// (previous_value, new_value) on success. The new_value mirrors
/// what was set so downstream logging can attribute correctly even
/// when the caller passes through e.g. `Length(None)`.
fn apply_character_field_on_run(
    run: &mut paged_parse::CharacterRun,
    path: PropertyPath,
    value: &Value,
) -> Result<(Value, Value), OperationError> {
    match path {
        PropertyPath::CharacterFontSize => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = run.point_size;
            run.point_size = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::CharacterLeading => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = run.leading;
            run.leading = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::CharacterTracking => {
            let Value::Length(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Length".to_string(),
                });
            };
            let prev = run.tracking;
            run.tracking = *new_val;
            Ok((Value::Length(prev), Value::Length(*new_val)))
        }
        PropertyPath::CharacterFillColor => {
            let Value::ColorRef(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "ColorRef".to_string(),
                });
            };
            let prev = run.fill_color.clone();
            run.fill_color = new_val.clone();
            Ok((Value::ColorRef(prev), Value::ColorRef(new_val.clone())))
        }
        PropertyPath::AppliedCharacterStyle => {
            // Apply-an-entity (D3 of panel-catalog doc): the
            // character_style ref is a string-id payload. Empty
            // string clears the override; otherwise stores the
            // style's `self_id`.
            let Value::Text(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Text".to_string(),
                });
            };
            let prev = run.character_style.clone().unwrap_or_default();
            run.character_style = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
        }
        PropertyPath::AppliedConditions => {
            // SDK Phase 5 (D3 completion) — applied conditions per
            // CharacterRun. Wire encoding mirrors IDML's
            // `AppliedConditions="A B C"` attribute: a single
            // Value::Text whose payload is a whitespace-separated
            // list of `<Condition>` self_ids. Empty string clears.
            // Set semantics (de-dup, individual add/remove) are
            // the caller's concern for v1.
            let Value::Text(new_val) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Text".to_string(),
                });
            };
            let prev = run.applied_conditions.join(" ");
            run.applied_conditions = if new_val.is_empty() {
                Vec::new()
            } else {
                new_val.split_whitespace().map(|s| s.to_string()).collect()
            };
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
        }
        // W0.1 — string-valued character properties. Each stores the
        // raw IDML attribute string (enum strings pass through
        // verbatim — the toggle-group UI never emits an unknown
        // value). The empty string clears the per-run override back
        // to `None` (inherit from the style cascade); the inverse
        // re-applies the captured prior string, which round-trips a
        // prior-`None` back to `None` since `unwrap_or_default()`
        // maps `None ⇒ ""`.
        PropertyPath::CharacterFontFamily => set_run_text_field(run, path, value, |r| &mut r.font),
        PropertyPath::CharacterFontStyle => {
            set_run_text_field(run, path, value, |r| &mut r.font_style)
        }
        PropertyPath::CharacterKerningMethod => {
            set_run_text_field(run, path, value, |r| &mut r.kerning_method)
        }
        PropertyPath::CharacterCase => {
            set_run_text_field(run, path, value, |r| &mut r.capitalization)
        }
        PropertyPath::CharacterPosition => {
            set_run_text_field(run, path, value, |r| &mut r.position)
        }
        PropertyPath::CharacterLanguage => {
            set_run_text_field(run, path, value, |r| &mut r.applied_language)
        }
        PropertyPath::CharacterOtfFeatures => {
            set_run_text_field(run, path, value, |r| &mut r.otf_features)
        }
        // W0.1 — numeric character properties. `Value::Length(None)`
        // clears the per-run override (inherit from the cascade);
        // the captured prior `Option<f32>` round-trips bytewise.
        PropertyPath::CharacterBaselineShift => {
            set_run_length_field(run, path, value, |r| &mut r.baseline_shift)
        }
        PropertyPath::CharacterHorizontalScale => {
            set_run_length_field(run, path, value, |r| &mut r.horizontal_scale)
        }
        PropertyPath::CharacterVerticalScale => {
            set_run_length_field(run, path, value, |r| &mut r.vertical_scale)
        }
        PropertyPath::CharacterSkew => set_run_length_field(run, path, value, |r| &mut r.skew),
        // W0.1 — boolean character properties. `Value::Bool` carries
        // the new toggle; the field is `Option<bool>`. The inverse
        // captures `prev.unwrap_or(false)` — writes over an explicit
        // prior round-trip bytewise; a prior-`None` undoes to
        // `Some(false)` (see the path doc-comments for the
        // documented default-restore limitation).
        PropertyPath::CharacterUnderline => {
            set_run_bool_field(run, path, value, |r| &mut r.underline)
        }
        PropertyPath::CharacterStrikethru => {
            set_run_bool_field(run, path, value, |r| &mut r.strikethru)
        }
        PropertyPath::CharacterLigatures => {
            set_run_bool_field(run, path, value, |r| &mut r.ligatures_on)
        }
        _ => Err(OperationError::UnsupportedProperty {
            node: NodeId::StoryRange {
                story_id: String::new(),
                start: 0,
                end: 0,
            },
            path,
        }),
    }
}

// ---------------------------------------------------------------------------
// InsertNode
// ---------------------------------------------------------------------------

/// `frames_in_order` bookkeeping for structural inserts/removals.
///
/// The renderer, hit-tester, and scene-tree all walk a spread's
/// `frames_in_order` (cross-shape z-order) whenever it is non-empty —
/// a page item present in its kind vec but absent from the table is
/// invisible AND unclickable, and inserting/removing mid-vec shifts
/// every later same-kind `FrameRef` index. These helpers keep the
/// table consistent. On spreads whose table is EMPTY they do nothing:
/// the consumers' legacy fallback synthesises the walk order from the
/// kind vecs directly, and making the table non-empty with a single
/// entry would hide every other frame.
fn fr_index(fr: &FrameRef) -> usize {
    match fr {
        FrameRef::TextFrame(i)
        | FrameRef::Rectangle(i)
        | FrameRef::Oval(i)
        | FrameRef::GraphicLine(i)
        | FrameRef::Polygon(i)
        | FrameRef::Group(i) => *i,
    }
}

fn fr_with_index(fr: &FrameRef, i: usize) -> FrameRef {
    match fr {
        FrameRef::TextFrame(_) => FrameRef::TextFrame(i),
        FrameRef::Rectangle(_) => FrameRef::Rectangle(i),
        FrameRef::Oval(_) => FrameRef::Oval(i),
        FrameRef::GraphicLine(_) => FrameRef::GraphicLine(i),
        FrameRef::Polygon(_) => FrameRef::Polygon(i),
        FrameRef::Group(_) => FrameRef::Group(i),
    }
}

fn fr_same_kind(a: &FrameRef, b: &FrameRef) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Register a page item inserted at `vec_pos` of its kind vec:
/// same-kind refs at `>= vec_pos` shift up by one, then the new ref
/// lands at `z_slot` (or on top when `None` — new creations stack
/// like InDesign's draw tools).
fn register_frame_ref(
    spread: &mut Spread,
    template: FrameRef,
    vec_pos: usize,
    z_slot: Option<usize>,
) {
    if spread.frames_in_order.is_empty() {
        return; // legacy vec-walk fallback covers this spread
    }
    for fr in spread.frames_in_order.iter_mut() {
        if fr_same_kind(fr, &template) {
            let i = fr_index(fr);
            if i >= vec_pos {
                *fr = fr_with_index(fr, i + 1);
            }
        }
    }
    let len = spread.frames_in_order.len();
    let slot = z_slot.unwrap_or(len).min(len);
    spread
        .frames_in_order
        .insert(slot, fr_with_index(&template, vec_pos));
}

/// Unregister a page item removed from `vec_pos` of its kind vec;
/// returns the z slot it occupied so the `RemoveNode` inverse can
/// restore the exact stacking position.
fn unregister_frame_ref(spread: &mut Spread, template: FrameRef, vec_pos: usize) -> Option<usize> {
    if spread.frames_in_order.is_empty() {
        return None;
    }
    let target = fr_with_index(&template, vec_pos);
    let slot = spread
        .frames_in_order
        .iter()
        .position(|fr| fr_same_kind(fr, &target) && fr_index(fr) == vec_pos);
    if let Some(s) = slot {
        spread.frames_in_order.remove(s);
    }
    for fr in spread.frames_in_order.iter_mut() {
        if fr_same_kind(fr, &template) {
            let i = fr_index(fr);
            if i > vec_pos {
                *fr = fr_with_index(fr, i - 1);
            }
        }
    }
    slot
}

fn apply_insert_node(
    doc: &mut Document,
    parent: &NodeId,
    position: usize,
    z_slot: Option<usize>,
    spec: &NodeSpec,
) -> Result<AppliedOperation, OperationError> {
    // Phase H — CloneTranslate is a special "find the source, copy
    // it into its own spread" path. It ignores `parent` and uses the
    // source's host spread, so the gesture-spine caller doesn't have
    // to discover the spread itself.
    if let NodeSpec::CloneTranslate { .. } = spec {
        return apply_insert_clone_translate(doc, position, spec);
    }
    let parent_id = match parent {
        NodeId::Spread(id) => id,
        _ => {
            return Err(OperationError::InvalidParent {
                parent: parent.clone(),
                child_kind: spec.node_id().kind().to_string(),
            })
        }
    };

    // Uniqueness across the document — IDML Self IDs must be unique.
    let new_self_id = spec.node_id();
    if node_exists(doc, &new_self_id) {
        return Err(OperationError::DuplicateNodeId {
            id: new_self_id.self_id().to_string(),
        });
    }

    let spread = find_spread_mut(doc, parent_id)
        .ok_or_else(|| OperationError::NodeNotFound(parent.clone()))?;

    let invalidation = InvalidationHint {
        structural: true,
        ..Default::default()
    };

    match spec {
        NodeSpec::TextFrame {
            self_id,
            bounds,
            fill_color,
            stroke_color,
            stroke_weight,
            item_transform,
        } => {
            let len = spread.spread.text_frames.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut frame = new_text_frame(
                self_id.clone(),
                bounds_from_array(*bounds),
                fill_color.clone(),
            );
            frame.stroke_color = stroke_color.clone();
            frame.stroke_weight = *stroke_weight;
            frame.item_transform = *item_transform;
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
            let len = spread.spread.rectangles.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut rect = new_rectangle(
                self_id.clone(),
                bounds_from_array(*bounds),
                fill_color.clone(),
            );
            rect.stroke_color = stroke_color.clone();
            rect.stroke_weight = *stroke_weight;
            rect.item_transform = *item_transform;
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
            let len = spread.spread.ovals.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut oval = new_oval(
                self_id.clone(),
                bounds_from_array(*bounds),
                fill_color.clone(),
            );
            oval.stroke_color = stroke_color.clone();
            oval.stroke_weight = *stroke_weight;
            oval.item_transform = *item_transform;
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
            let len = spread.spread.graphic_lines.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut line = new_graphic_line(
                self_id.clone(),
                bounds_from_array(*bounds),
                anchors.iter().map(PathAnchorSpec::to_parse).collect(),
                subpath_starts.clone(),
                subpath_open.clone(),
                stroke_color.clone(),
                *stroke_weight,
            );
            line.item_transform = *item_transform;
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
            let len = spread.spread.polygons.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut poly = new_polygon(
                self_id.clone(),
                bounds_from_array(*bounds),
                anchors.iter().map(PathAnchorSpec::to_parse).collect(),
                subpath_starts.clone(),
                subpath_open.clone(),
                fill_color.clone(),
                stroke_color.clone(),
                *stroke_weight,
            );
            poly.item_transform = *item_transform;
            spread.spread.polygons.insert(position, poly);
            register_frame_ref(&mut spread.spread, FrameRef::Polygon(0), position, z_slot);
        }
        NodeSpec::CloneTranslate { .. } => {
            // Handled by `apply_insert_clone_translate` above.
            unreachable!("CloneTranslate routed via the early-return");
        }
    }

    let inverse = invert_insert_node(spec);
    Ok(AppliedOperation {
        op: Operation::InsertNode {
            parent: parent.clone(),
            position,
            node: spec.clone(),
            z_slot,
        },
        inverse,
        invalidation,
    })
}

// ---------------------------------------------------------------------------
// RemoveNode
// ---------------------------------------------------------------------------

fn apply_remove_node(
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
fn remove_and_capture(
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
        _ => Err(OperationError::UnsupportedProperty {
            node: node.clone(),
            path: PropertyPath::FrameBounds, // unused; signals "this node kind isn't removable yet"
        }),
    }
}

// ---------------------------------------------------------------------------
// MoveNode
// ---------------------------------------------------------------------------

fn apply_move_node(
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
fn restore_capture(
    doc: &mut Document,
    parent: &NodeId,
    position: usize,
    spec: NodeSpec,
    z_slot: Option<usize>,
) {
    let _ = insert_captured(doc, parent.self_id(), position, spec, z_slot);
}

fn insert_captured(
    doc: &mut Document,
    parent_self_id: &str,
    position: usize,
    spec: NodeSpec,
    z_slot: Option<usize>,
) -> Result<(), OperationError> {
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
        } => {
            let mut frame = new_text_frame(self_id, bounds_from_array(bounds), fill_color);
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
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Batch
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Editor-ops — Page tool (InsertPage / RemovePage / PageBounds)
// ---------------------------------------------------------------------------

/// Lossless undo capture for `RemovePage`: the whole hosting spread
/// (every page item included) plus its position in `doc.spreads` and
/// its manifest src. Serialized to JSON inside the inverse Operation
/// so the op stays wire-shaped; `paged_parse::Spread` derives
/// `Serialize`+`Deserialize` for exactly this round-trip.
#[derive(serde::Serialize, serde::Deserialize)]
struct SpreadRestore {
    index: usize,
    src: String,
    spread: Spread,
}

/// Letter portrait — the fallback page size when there is no
/// reference page to clone (matches the renderer's empty-document
/// fallback).
const FALLBACK_PAGE_BOUNDS: [f32; 4] = [0.0, 0.0, 792.0, 612.0];
/// Pasteboard gap between an inserted spread and everything above it.
const SPREAD_STACK_GAP_PT: f32 = 72.0;

/// Mint two fresh `u<hex>` ids (spread + page), unique across every
/// self id in the document — page items, spreads, and pages alike.
fn mint_spread_page_ids(doc: &Document) -> (String, String) {
    let mut max: u64 = 0;
    let mut scan = |id: Option<&str>| {
        let Some(id) = id else { return };
        let Some(hex) = id.strip_prefix('u') else {
            return;
        };
        if hex.is_empty() || hex.len() > 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return;
        }
        if let Ok(v) = u64::from_str_radix(hex, 16) {
            max = max.max(v);
        }
    };
    for parsed in &doc.spreads {
        let s = &parsed.spread;
        scan(s.self_id.as_deref());
        for p in &s.pages {
            scan(p.self_id.as_deref());
        }
        for f in &s.text_frames {
            scan(f.self_id.as_deref());
        }
        for r in &s.rectangles {
            scan(r.self_id.as_deref());
        }
        for o in &s.ovals {
            scan(o.self_id.as_deref());
        }
        for l in &s.graphic_lines {
            scan(l.self_id.as_deref());
        }
        for p in &s.polygons {
            scan(p.self_id.as_deref());
        }
        for g in &s.groups {
            scan(g.self_id.as_deref());
        }
    }
    (format!("u{:x}", max + 1), format!("u{:x}", max + 2))
}

fn find_page_mut<'a>(doc: &'a mut Document, self_id: &str) -> Option<&'a mut paged_parse::Page> {
    for parsed in &mut doc.spreads {
        if let Some(p) = parsed
            .spread
            .pages
            .iter_mut()
            .find(|p| p.self_id.as_deref() == Some(self_id))
        {
            return Some(p);
        }
    }
    None
}

fn apply_insert_page(
    doc: &mut Document,
    after_page_id: Option<&str>,
    master_id: Option<&str>,
    spread_self_id: Option<String>,
    page_self_id: Option<String>,
    restore_spread_json: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let invalidation = InvalidationHint {
        structural: true,
        ..Default::default()
    };

    // Undo path — reinsert the captured spread verbatim at its
    // original index.
    if let Some(json) = restore_spread_json {
        let restore: SpreadRestore =
            serde_json::from_str(json).map_err(|e| OperationError::InvalidValue {
                node: NodeId::Spread(String::new()),
                path: PropertyPath::PageBounds,
                reason: format!("malformed spread restore payload: {e}"),
            })?;
        let page_id = restore
            .spread
            .pages
            .first()
            .and_then(|p| p.self_id.clone())
            .unwrap_or_default();
        let index = restore.index.min(doc.spreads.len());
        doc.spreads.insert(
            index,
            paged_scene::ParsedSpread {
                src: restore.src,
                spread: restore.spread,
            },
        );
        return Ok(AppliedOperation {
            op: Operation::InsertPage {
                after_page_id: after_page_id.map(str::to_string),
                master_id: master_id.map(str::to_string),
                spread_self_id,
                page_self_id,
                restore_spread_json: Some(json.to_string()),
            },
            inverse: Operation::RemovePage { page_id },
            invalidation,
        });
    }

    // Fresh insert — resolve the host spread (after_page_id's, else
    // the last) and clone its page size.
    let (host_idx, ref_bounds) = match after_page_id {
        Some(pid) => {
            let mut found = None;
            for (i, parsed) in doc.spreads.iter().enumerate() {
                if let Some(p) = parsed
                    .spread
                    .pages
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(pid))
                {
                    found = Some((i, bounds_to_array(p.bounds)));
                    break;
                }
            }
            found.ok_or_else(|| OperationError::NodeNotFound(NodeId::Page(pid.to_string())))?
        }
        None => {
            let last = doc.spreads.len().saturating_sub(1);
            let bounds = doc
                .spreads
                .last()
                .and_then(|parsed| parsed.spread.pages.first())
                .map(|p| bounds_to_array(p.bounds))
                .unwrap_or(FALLBACK_PAGE_BOUNDS);
            (last, bounds)
        }
    };

    // Stack the new spread below everything on the pasteboard so the
    // spread-origin consumers (hit-test, marquee, snap) never see
    // overlapping spread AABBs. Pure-translate transforms are the
    // universal case; rotated pages are out of scope (flagged).
    let mut max_bottom: f32 = 0.0;
    for parsed in &doc.spreads {
        let sty = parsed.spread.item_transform.map(|m| m[5]).unwrap_or(0.0);
        for p in &parsed.spread.pages {
            let pty = p.item_transform.map(|m| m[5]).unwrap_or(0.0);
            max_bottom = max_bottom.max(sty + pty + p.bounds.bottom);
        }
    }

    // Redo re-applies the echoed op, so prefer ids it carries; only
    // mint on the first application.
    let (sid, pid) = match (spread_self_id, page_self_id) {
        (Some(s), Some(p)) => (s, p),
        _ => mint_spread_page_ids(doc),
    };
    if doc.spreads.iter().any(|parsed| {
        parsed.spread.self_id.as_deref() == Some(sid.as_str())
            || parsed
                .spread
                .pages
                .iter()
                .any(|p| p.self_id.as_deref() == Some(pid.as_str()))
    }) {
        return Err(OperationError::DuplicateNodeId { id: sid });
    }

    let page = paged_parse::Page {
        self_id: Some(pid.clone()),
        bounds: bounds_from_array(ref_bounds),
        applied_master: master_id.map(str::to_string),
        item_transform: None,
        master_page_transform: None,
        override_list: Vec::new(),
        name: None,
        show_master_items: None,
    };
    let spread = Spread {
        self_id: Some(sid.clone()),
        item_transform: Some([1.0, 0.0, 0.0, 1.0, 0.0, max_bottom + SPREAD_STACK_GAP_PT]),
        pages: vec![page],
        ..Spread::default()
    };
    doc.spreads.insert(
        host_idx + 1,
        paged_scene::ParsedSpread {
            src: format!("Spreads/Spread_{sid}.xml"),
            spread,
        },
    );

    Ok(AppliedOperation {
        op: Operation::InsertPage {
            after_page_id: after_page_id.map(str::to_string),
            master_id: master_id.map(str::to_string),
            spread_self_id: Some(sid),
            page_self_id: Some(pid.clone()),
            restore_spread_json: None,
        },
        inverse: Operation::RemovePage { page_id: pid },
        invalidation,
    })
}

fn apply_remove_page(
    doc: &mut Document,
    page_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::Page(page_id.to_string());
    let Some(idx) = doc.spreads.iter().position(|parsed| {
        parsed
            .spread
            .pages
            .iter()
            .any(|p| p.self_id.as_deref() == Some(page_id))
    }) else {
        return Err(OperationError::NodeNotFound(node));
    };
    let page_count = doc.spreads[idx].spread.pages.len();
    if page_count > 1 {
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::PageBounds,
            reason: "page belongs to a multi-page spread; per-page deletion within a spread \
                     is not supported in v1"
                .to_string(),
        });
    }
    if doc.spreads.len() == 1 {
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::PageBounds,
            reason: "cannot delete the document's only page".to_string(),
        });
    }
    let parsed = doc.spreads.remove(idx);
    let spread_self_id = parsed.spread.self_id.clone();
    let restore = SpreadRestore {
        index: idx,
        src: parsed.src,
        spread: parsed.spread,
    };
    let json = serde_json::to_string(&restore).map_err(|e| OperationError::InvalidValue {
        node: NodeId::Page(page_id.to_string()),
        path: PropertyPath::PageBounds,
        reason: format!("spread capture failed: {e}"),
    })?;
    Ok(AppliedOperation {
        op: Operation::RemovePage {
            page_id: page_id.to_string(),
        },
        inverse: Operation::InsertPage {
            after_page_id: None,
            master_id: None,
            spread_self_id,
            page_self_id: Some(page_id.to_string()),
            restore_spread_json: Some(json),
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

// ---------------------------------------------------------------------------
// Track M — structural layer ops
// ---------------------------------------------------------------------------

fn apply_move_layer(
    doc: &mut Document,
    layer_id: &str,
    new_index: usize,
) -> Result<AppliedOperation, OperationError> {
    let layers = &mut doc.container.designmap.layers;
    let original_index = layers
        .iter()
        .position(|l| l.self_id == layer_id)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Layer(layer_id.to_string())))?;
    let clamped = new_index.min(layers.len().saturating_sub(1));
    if clamped == original_index {
        // No-op move still records as a forward op so the undo log
        // keeps its index in sync with caller expectations.
    } else {
        let layer = layers.remove(original_index);
        layers.insert(clamped, layer);
    }
    let inverse = Operation::MoveLayer {
        layer_id: layer_id.to_string(),
        new_index: original_index,
    };
    Ok(AppliedOperation {
        op: Operation::MoveLayer {
            layer_id: layer_id.to_string(),
            new_index: clamped,
        },
        inverse,
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_insert_layer(
    doc: &mut Document,
    position: usize,
    name: &str,
    requested_self_id: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let layers = &mut doc.container.designmap.layers;
    let clamped = position.min(layers.len());
    let self_id = match requested_self_id {
        Some(s) => {
            if layers.iter().any(|l| l.self_id == s) {
                return Err(OperationError::DuplicateNodeId { id: s.to_string() });
            }
            s.to_string()
        }
        None => {
            // Deterministic self-id derived from a counter —
            // `Layer/u<n>` where `n` is the smallest non-colliding
            // integer. Real-world IDMLs use IDs like `u1fe`, but for
            // in-editor authored layers the simple monotone pattern
            // is sufficient + readable.
            let mut n = layers.len();
            let mut id = format!("Layer/u{n}");
            while layers.iter().any(|l| l.self_id == id) {
                n += 1;
                id = format!("Layer/u{n}");
            }
            id
        }
    };
    layers.insert(
        clamped,
        paged_parse::Layer {
            self_id: self_id.clone(),
            name: Some(name.to_string()),
            visible: true,
            locked: false,
            printable: true,
            // Editor-inserted layers are top-level peers; nested
            // layer-group authoring isn't a mutation op yet.
            parent_id: None,
        },
    );
    let inverse = Operation::RemoveLayer {
        layer_id: self_id.clone(),
    };
    Ok(AppliedOperation {
        op: Operation::InsertLayer {
            position: clamped,
            name: name.to_string(),
            self_id: Some(self_id),
        },
        inverse,
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_remove_layer(
    doc: &mut Document,
    layer_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let layers = &mut doc.container.designmap.layers;
    let idx = layers
        .iter()
        .position(|l| l.self_id == layer_id)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Layer(layer_id.to_string())))?;
    let captured = layers.remove(idx);
    // Inverse: re-insert at the original index, then rename to
    // restore name + re-apply flags. We pack the restore into a
    // Batch so a single Cmd-Z reverses the whole removal.
    let restore_flags: Vec<Operation> = vec![
        Operation::SetProperty {
            node: NodeId::Layer(captured.self_id.clone()),
            path: PropertyPath::LayerName,
            value: Value::Text(captured.name.clone().unwrap_or_default()),
        },
        Operation::SetProperty {
            node: NodeId::Layer(captured.self_id.clone()),
            path: PropertyPath::LayerVisible,
            value: Value::Bool(captured.visible),
        },
        Operation::SetProperty {
            node: NodeId::Layer(captured.self_id.clone()),
            path: PropertyPath::LayerLocked,
            value: Value::Bool(captured.locked),
        },
        Operation::SetProperty {
            node: NodeId::Layer(captured.self_id.clone()),
            path: PropertyPath::LayerPrintable,
            value: Value::Bool(captured.printable),
        },
    ];
    let inverse = Operation::Batch {
        ops: std::iter::once(Operation::InsertLayer {
            position: idx,
            name: captured.name.clone().unwrap_or_default(),
            self_id: Some(captured.self_id.clone()),
        })
        .chain(restore_flags)
        .collect(),
    };
    Ok(AppliedOperation {
        op: Operation::RemoveLayer {
            layer_id: layer_id.to_string(),
        },
        inverse,
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

// ── Swatch collection mutations ───────────────────────────────────
//
// A "swatch" in the editor's Swatches panel is a `<Color>` entry in
// `doc.palette.colors` (a `BTreeMap` keyed by `Self` id). Create / edit
// / delete mirror the layer-op pattern: each builds its own lossless
// inverse so a single Cmd-Z reverses it. A palette change can affect any
// frame that references the swatch, and we don't track which, so the
// invalidation is the conservative `structural` (forces a rebuild that
// re-resolves the palette) — there's no finer per-NodeId palette hint.

/// Build a `ColorEntry` from a wire `SwatchSpec` at a resolved id.
fn color_entry_from_spec(self_id: String, spec: &SwatchSpec) -> paged_parse::ColorEntry {
    paged_parse::ColorEntry {
        self_id,
        name: spec.name.clone(),
        space: paged_parse::ColorSpace::from_attr(&spec.space),
        value: spec.value.clone(),
        model: spec
            .model
            .as_deref()
            .map(paged_parse::ColorModel::from_attr)
            .unwrap_or(paged_parse::ColorModel::Process),
        alternate_space: spec
            .alternate_space
            .as_deref()
            .map(paged_parse::ColorSpace::from_attr),
        alternate_value: spec.alternate_value.clone(),
        tint: spec.tint,
        alpha: spec.alpha,
    }
}

/// Capture a `ColorEntry` back into a `SwatchSpec` (for lossless
/// inverses). `self_id` is carried so a delete→undo recreates the
/// swatch at its original id.
fn swatch_spec_from_entry(entry: &paged_parse::ColorEntry) -> SwatchSpec {
    SwatchSpec {
        self_id: Some(entry.self_id.clone()),
        name: entry.name.clone(),
        space: entry.space.as_attr().to_string(),
        value: entry.value.clone(),
        model: Some(entry.model.as_attr().to_string()),
        alternate_space: entry.alternate_space.map(|s| s.as_attr().to_string()),
        alternate_value: entry.alternate_value.clone(),
        tint: entry.tint,
        alpha: entry.alpha,
    }
}

fn apply_create_swatch(
    doc: &mut Document,
    spec: &SwatchSpec,
) -> Result<AppliedOperation, OperationError> {
    let colors = &mut doc.palette.colors;
    let self_id = match &spec.self_id {
        Some(s) => {
            if colors.contains_key(s) {
                return Err(OperationError::DuplicateNodeId { id: s.clone() });
            }
            s.clone()
        }
        None => {
            // Deterministic, non-colliding `Color/u<n>` — mirrors the
            // layer-op id assignment.
            let mut n = colors.len();
            let mut id = format!("Color/u{n}");
            while colors.contains_key(&id) {
                n += 1;
                id = format!("Color/u{n}");
            }
            id
        }
    };
    let entry = color_entry_from_spec(self_id.clone(), spec);
    colors.insert(self_id.clone(), entry);
    // Echo the resolved id back in the recorded op so a redo (or a
    // remote replay) reuses it verbatim.
    let mut resolved_spec = spec.clone();
    resolved_spec.self_id = Some(self_id.clone());
    Ok(AppliedOperation {
        op: Operation::CreateSwatch {
            spec: resolved_spec,
        },
        inverse: Operation::DeleteSwatch { swatch_id: self_id },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_edit_swatch(
    doc: &mut Document,
    swatch_id: &str,
    spec: &SwatchSpec,
) -> Result<AppliedOperation, OperationError> {
    let colors = &mut doc.palette.colors;
    let existing =
        colors
            .get(swatch_id)
            .ok_or_else(|| OperationError::CollectionEntryNotFound {
                collection: "swatch".to_string(),
                id: swatch_id.to_string(),
            })?;
    // Capture the prior state for the inverse before overwriting.
    let prior = swatch_spec_from_entry(existing);
    // Replace the editable fields in place; the id (map key) is the
    // identity and never changes here.
    let updated = color_entry_from_spec(swatch_id.to_string(), spec);
    colors.insert(swatch_id.to_string(), updated);
    Ok(AppliedOperation {
        op: Operation::EditSwatch {
            swatch_id: swatch_id.to_string(),
            spec: spec.clone(),
        },
        inverse: Operation::EditSwatch {
            swatch_id: swatch_id.to_string(),
            spec: prior,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_delete_swatch(
    doc: &mut Document,
    swatch_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let colors = &mut doc.palette.colors;
    let captured =
        colors
            .remove(swatch_id)
            .ok_or_else(|| OperationError::CollectionEntryNotFound {
                collection: "swatch".to_string(),
                id: swatch_id.to_string(),
            })?;
    // Inverse recreates the swatch at its original id with every field.
    let inverse = Operation::CreateSwatch {
        spec: swatch_spec_from_entry(&captured),
    };
    Ok(AppliedOperation {
        op: Operation::DeleteSwatch {
            swatch_id: swatch_id.to_string(),
        },
        inverse,
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

// ── Gradient + colour-group collection mutations ──────────────────
//
// Same shape as swatches (typed spec ↔ entry; create/edit/delete with
// lossless inverses), over `doc.palette.gradients` / `.color_groups`.

fn gradient_kind_from_attr(s: &str) -> paged_parse::GradientKind {
    match s {
        "Linear" => paged_parse::GradientKind::Linear,
        "Radial" => paged_parse::GradientKind::Radial,
        _ => paged_parse::GradientKind::Unknown,
    }
}

fn gradient_kind_as_attr(k: paged_parse::GradientKind) -> &'static str {
    match k {
        paged_parse::GradientKind::Linear => "Linear",
        paged_parse::GradientKind::Radial => "Radial",
        paged_parse::GradientKind::Unknown => "Unknown",
    }
}

fn gradient_entry_from_spec(self_id: String, spec: &GradientSpec) -> paged_parse::GradientEntry {
    paged_parse::GradientEntry {
        self_id,
        name: spec.name.clone(),
        kind: gradient_kind_from_attr(&spec.kind),
        stops: spec
            .stops
            .iter()
            .map(|s| paged_parse::GradientStopRef {
                stop_color: s.stop_color.clone(),
                location_pct: s.location_pct,
                midpoint_pct: s.midpoint_pct,
            })
            .collect(),
    }
}

fn gradient_spec_from_entry(entry: &paged_parse::GradientEntry) -> GradientSpec {
    GradientSpec {
        self_id: Some(entry.self_id.clone()),
        name: entry.name.clone(),
        kind: gradient_kind_as_attr(entry.kind).to_string(),
        stops: entry
            .stops
            .iter()
            .map(|s| GradientStopSpec {
                stop_color: s.stop_color.clone(),
                location_pct: s.location_pct,
                midpoint_pct: s.midpoint_pct,
            })
            .collect(),
    }
}

/// B-04 — mint a page-item id (`u<hex>`) unique across every page
/// item in the document, groups included. Mutate-side twin of the
/// canvas `mint_page_item_id_with_offset` scanner.
fn mint_group_id(doc: &paged_scene::Document) -> String {
    fn scan(max: &mut u64, id: Option<&str>) {
        if let Some(rest) = id.and_then(|s| s.strip_prefix('u')) {
            if let Ok(n) = u64::from_str_radix(rest, 16) {
                *max = (*max).max(n);
            }
        }
    }
    let mut max: u64 = 0;
    for parsed in &doc.spreads {
        let s = &parsed.spread;
        for f in &s.text_frames {
            scan(&mut max, f.self_id.as_deref());
        }
        for r in &s.rectangles {
            scan(&mut max, r.self_id.as_deref());
        }
        for o in &s.ovals {
            scan(&mut max, o.self_id.as_deref());
        }
        for l in &s.graphic_lines {
            scan(&mut max, l.self_id.as_deref());
        }
        for p in &s.polygons {
            scan(&mut max, p.self_id.as_deref());
        }
        for g in &s.groups {
            scan(&mut max, g.self_id.as_deref());
        }
    }
    format!("u{:x}", max + 1)
}

/// Resolve a leaf-member NodeId to its `FrameRef` within `spread`.
fn leaf_frame_ref(spread: &paged_parse::Spread, node: &NodeId) -> Option<paged_parse::FrameRef> {
    use paged_parse::FrameRef;
    let find = |id: &str, ids: Vec<Option<&str>>| -> Option<usize> {
        ids.iter().position(|s| *s == Some(id))
    };
    match node {
        NodeId::TextFrame(id) => find(
            id,
            spread
                .text_frames
                .iter()
                .map(|f| f.self_id.as_deref())
                .collect(),
        )
        .map(FrameRef::TextFrame),
        NodeId::Rectangle(id) => find(
            id,
            spread
                .rectangles
                .iter()
                .map(|f| f.self_id.as_deref())
                .collect(),
        )
        .map(FrameRef::Rectangle),
        NodeId::Oval(id) => find(
            id,
            spread.ovals.iter().map(|f| f.self_id.as_deref()).collect(),
        )
        .map(FrameRef::Oval),
        NodeId::GraphicLine(id) => find(
            id,
            spread
                .graphic_lines
                .iter()
                .map(|f| f.self_id.as_deref())
                .collect(),
        )
        .map(FrameRef::GraphicLine),
        NodeId::Polygon(id) => find(
            id,
            spread
                .polygons
                .iter()
                .map(|f| f.self_id.as_deref())
                .collect(),
        )
        .map(FrameRef::Polygon),
        _ => None,
    }
}

/// Plugin-metadata write cap: keeps documents loadable and the Label
/// mechanism friendly to other IDML consumers (facility design §2).
const PLUGIN_METADATA_MAX_BYTES: usize = 64 * 1024;

/// Locate the spread holding a leaf page item by NodeId. Returns the
/// spread index so the caller can borrow mutably afterwards.
fn find_spread_for_leaf(doc: &Document, node: &NodeId) -> Option<usize> {
    fn has<'a>(mut ids: impl Iterator<Item = Option<&'a str>>, id: &str) -> bool {
        ids.any(|s| s == Some(id))
    }
    for (si, parsed) in doc.spreads.iter().enumerate() {
        let s = &parsed.spread;
        let found = match node {
            NodeId::TextFrame(id) => has(s.text_frames.iter().map(|f| f.self_id.as_deref()), id),
            NodeId::Rectangle(id) => has(s.rectangles.iter().map(|f| f.self_id.as_deref()), id),
            NodeId::Oval(id) => has(s.ovals.iter().map(|f| f.self_id.as_deref()), id),
            NodeId::GraphicLine(id) => {
                has(s.graphic_lines.iter().map(|f| f.self_id.as_deref()), id)
            }
            NodeId::Polygon(id) => has(s.polygons.iter().map(|f| f.self_id.as_deref()), id),
            _ => false,
        };
        if found {
            return Some(si);
        }
    }
    None
}

/// Plugin-metadata carrier (decision 9 facility) — set / replace /
/// delete one Label `KeyValuePair` in the reserved `x-paged:`
/// namespace. Gates BEFORE any mutation: key prefix, 64 KiB cap, and
/// the JSON envelope `{ v: number >= 1, data: object, … }`. The
/// inverse carries the prev snapshot so undo restores exactly
/// (including "was absent").
fn apply_plugin_metadata(
    doc: &mut Document,
    node: &NodeId,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    let invalid = |reason: String| OperationError::InvalidValue {
        node: node.clone(),
        path: PropertyPath::PluginMetadata,
        reason,
    };
    let Value::PluginMetadata {
        key,
        value: new_value,
        ..
    } = value
    else {
        return Err(invalid("expected Value::PluginMetadata".into()));
    };
    if !key.starts_with("x-paged:") || key.len() <= "x-paged:".len() {
        return Err(invalid(format!(
            "metadata keys live in the reserved namespace: expected \"x-paged:<plugin>\", got \"{key}\""
        )));
    }
    if let Some(v) = new_value {
        if v.len() > PLUGIN_METADATA_MAX_BYTES {
            return Err(invalid(format!(
                "metadata value is {} bytes; the cap is {PLUGIN_METADATA_MAX_BYTES} (assets belong in the asset store, not inline)",
                v.len()
            )));
        }
        let parsed: serde_json::Value = serde_json::from_str(v)
            .map_err(|e| invalid(format!("metadata value must be the JSON envelope: {e}")))?;
        let envelope_ok = parsed.as_object().is_some_and(|o| {
            o.get("v")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|n| n >= 1)
                && o.get("data").is_some_and(serde_json::Value::is_object)
        });
        if !envelope_ok {
            return Err(invalid(
                "metadata envelope must be { v: <int >= 1>, data: {…}, engine?: {…} }".into(),
            ));
        }
    }
    let Some(si) = find_spread_for_leaf(doc, node) else {
        return Err(OperationError::NodeNotFound(node.clone()));
    };

    // ---- mutation ----
    let self_id = node.self_id().to_string();
    let labels = &mut doc.spreads[si].spread.labels;
    let prev: Option<String> = labels
        .get(&self_id)
        .and_then(|entries| entries.iter().find(|(k, _)| k == key))
        .map(|(_, v)| v.clone());
    match new_value {
        Some(v) => {
            let entries = labels.entry(self_id).or_default();
            match entries.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = v.clone(),
                None => entries.push((key.clone(), v.clone())),
            }
        }
        None => {
            if let Some(entries) = labels.get_mut(&self_id) {
                entries.retain(|(k, _)| k != key);
                if entries.is_empty() {
                    labels.remove(&self_id);
                }
            }
        }
    }

    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::PluginMetadata,
            value: Value::PluginMetadata {
                key: key.clone(),
                value: new_value.clone(),
                prev: Some(prev.clone()),
            },
        },
        inverse: Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::PluginMetadata,
            value: Value::PluginMetadata {
                key: key.clone(),
                value: prev,
                prev: Some(new_value.clone()),
            },
        },
        // Metadata is invisible to the renderer — no invalidation.
        invalidation: InvalidationHint::default(),
    })
}

/// B-04 — group leaf page items. Fully validated BEFORE any mutation
/// (atomicity invariant). Members contiguous in z-order group
/// paint-neutrally (the group ref takes the earliest member's
/// `frames_in_order` slot, paint recursion emits members there in
/// stored order); scattered members deterministically collect at the
/// earliest slot. The inverse carries the original slots so undo
/// restores z-order EXACTLY either way.
fn apply_create_group(
    doc: &mut paged_scene::Document,
    spec: &GroupSpec,
) -> Result<AppliedOperation, OperationError> {
    use paged_parse::FrameRef;

    let invalid = |reason: String| OperationError::InvalidValue {
        node: NodeId::Group(spec.self_id.clone().unwrap_or_default()),
        path: PropertyPath::FrameTransform,
        reason,
    };

    if spec.members.is_empty() {
        return Err(invalid("a group needs at least one member".into()));
    }
    // v1: flat groups — leaf kinds only.
    if spec.members.iter().any(|m| matches!(m, NodeId::Group(_))) {
        return Err(invalid(
            "nested groups are not supported yet (flat groups in v1)".into(),
        ));
    }
    // Duplicate member ids.
    {
        let mut seen = std::collections::HashSet::new();
        for m in &spec.members {
            if !seen.insert(m.self_id().to_string()) {
                return Err(invalid(format!("duplicate member \"{}\"", m.self_id())));
            }
        }
    }
    // Locate the ONE spread holding every member; resolve FrameRefs.
    let mut located: Option<(usize, Vec<FrameRef>)> = None;
    for (si, parsed) in doc.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        let refs: Vec<Option<FrameRef>> = spec
            .members
            .iter()
            .map(|m| leaf_frame_ref(spread, m))
            .collect();
        if refs.iter().all(|r| r.is_some()) {
            located = Some((si, refs.into_iter().flatten().collect()));
            break;
        }
        if refs.iter().any(|r| r.is_some()) {
            return Err(invalid("all members must live on the same spread".into()));
        }
    }
    let Some((spread_idx, member_refs)) = located else {
        return Err(invalid("member not found in any spread".into()));
    };
    let spread = &doc.spreads[spread_idx].spread;
    // Already grouped? (Direct membership scan — nesting is v1-flat.)
    for g in &spread.groups {
        for r in &member_refs {
            if g.members.contains(r) {
                return Err(invalid("a member already belongs to another group".into()));
            }
        }
    }
    // Every member must sit in frames_in_order (top-level item).
    for r in &member_refs {
        if !spread.frames_in_order.contains(r) {
            return Err(invalid("member is not a top-level spread item".into()));
        }
    }
    // Mint or validate the id.
    let self_id = match &spec.self_id {
        Some(s) => {
            if spread
                .groups
                .iter()
                .any(|g| g.self_id.as_deref() == Some(s))
            {
                return Err(OperationError::DuplicateNodeId { id: s.clone() });
            }
            s.clone()
        }
        None => mint_group_id(doc),
    };

    // ---- mutation (validated; cannot fail past this point) ----
    let spread = &mut doc.spreads[spread_idx].spread;
    // Members in DOCUMENT order (their frames_in_order positions).
    let mut ordered: Vec<(usize, FrameRef)> = member_refs
        .iter()
        .map(|r| {
            let pos = spread
                .frames_in_order
                .iter()
                .position(|x| x == r)
                .expect("validated above");
            (pos, *r)
        })
        .collect();
    ordered.sort_by_key(|(pos, _)| *pos);
    let earliest = ordered[0].0;
    let members_doc_order: Vec<FrameRef> = ordered.iter().map(|(_, r)| *r).collect();
    // Snapshot for the inverse: exact pre-group slots (doc order).
    let restore_slots: Vec<u32> = ordered.iter().map(|(pos, _)| *pos as u32).collect();

    let new_group_idx = spread.groups.len();
    spread.groups.push(paged_parse::Group {
        self_id: Some(self_id.clone()),
        members: members_doc_order.clone(),
        transparency: Default::default(),
        item_transform: None,
    });
    // frames_in_order surgery: group ref at the earliest slot, member
    // entries removed.
    spread
        .frames_in_order
        .insert(earliest, FrameRef::Group(new_group_idx));
    spread
        .frames_in_order
        .retain(|r| !members_doc_order.contains(r) || matches!(r, FrameRef::Group(_)));

    let mut resolved = spec.clone();
    resolved.self_id = Some(self_id.clone());
    Ok(AppliedOperation {
        op: Operation::CreateGroup { spec: resolved },
        inverse: Operation::DissolveGroup {
            group_id: self_id,
            restore_slots: Some(restore_slots),
        },
        invalidation: InvalidationHint {
            structural: true,
            frame_geometry: spec.members.clone(),
            ..Default::default()
        },
    })
}

/// B-04 — dissolve a group: members return to the group's
/// `frames_in_order` slot in stored order — or, when an undo inverse
/// carries `restore_slots`, at their exact pre-group indices;
/// `FrameRef::Group` indices above the removed entry are fixed up
/// across the spread.
fn apply_dissolve_group(
    doc: &mut paged_scene::Document,
    group_id: &str,
    restore_slots: Option<&[u32]>,
) -> Result<AppliedOperation, OperationError> {
    use paged_parse::FrameRef;

    let node = NodeId::Group(group_id.to_string());
    let invalid = |reason: String| OperationError::InvalidValue {
        node: node.clone(),
        path: PropertyPath::FrameTransform,
        reason,
    };

    // Locate the group.
    let mut found: Option<(usize, usize)> = None;
    for (si, parsed) in doc.spreads.iter().enumerate() {
        if let Some(gi) = parsed
            .spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(group_id))
        {
            found = Some((si, gi));
            break;
        }
    }
    let Some((spread_idx, group_idx)) = found else {
        return Err(OperationError::NodeNotFound(node));
    };
    let spread = &doc.spreads[spread_idx].spread;
    // v1: refuse to dissolve a group that is itself a member of
    // another group (parsed nested structures stay intact).
    if spread
        .groups
        .iter()
        .any(|g| g.members.contains(&FrameRef::Group(group_idx)))
    {
        return Err(invalid(
            "group is nested inside another group; dissolve the outer group first".into(),
        ));
    }
    // The group must be a top-level frames_in_order entry.
    let Some(slot) = spread
        .frames_in_order
        .iter()
        .position(|r| *r == FrameRef::Group(group_idx))
    else {
        return Err(invalid("group is not a top-level spread item".into()));
    };
    // Members must be resolvable to NodeIds for the inverse spec
    // (leaf members only in v1 — nested parsed groups refuse above
    // only covers THIS group being nested; a group CONTAINING groups
    // also stays untouched in v1).
    let member_nodes: Option<Vec<NodeId>> = spread.groups[group_idx]
        .members
        .iter()
        .map(|r| match *r {
            FrameRef::TextFrame(i) => spread
                .text_frames
                .get(i)
                .and_then(|f| f.self_id.clone())
                .map(NodeId::TextFrame),
            FrameRef::Rectangle(i) => spread
                .rectangles
                .get(i)
                .and_then(|f| f.self_id.clone())
                .map(NodeId::Rectangle),
            FrameRef::Oval(i) => spread
                .ovals
                .get(i)
                .and_then(|f| f.self_id.clone())
                .map(NodeId::Oval),
            FrameRef::GraphicLine(i) => spread
                .graphic_lines
                .get(i)
                .and_then(|f| f.self_id.clone())
                .map(NodeId::GraphicLine),
            FrameRef::Polygon(i) => spread
                .polygons
                .get(i)
                .and_then(|f| f.self_id.clone())
                .map(NodeId::Polygon),
            FrameRef::Group(_) => None,
        })
        .collect();
    let Some(member_nodes) = member_nodes else {
        return Err(invalid(
            "group contains nested groups or id-less members; v1 dissolves flat groups only".into(),
        ));
    };

    // ---- mutation ----
    let spread = &mut doc.spreads[spread_idx].spread;
    let group = spread.groups.remove(group_idx);
    spread.frames_in_order.remove(slot);
    match restore_slots {
        // Undo path: members back at their exact pre-group indices
        // (captured ascending, paired with stored member order).
        Some(slots) if slots.len() == group.members.len() => {
            for (r, s) in group.members.iter().zip(slots) {
                let at = (*s as usize).min(spread.frames_in_order.len());
                spread.frames_in_order.insert(at, *r);
            }
        }
        // User-initiated ungroup: members stay together at the
        // group's slot (the InDesign semantic).
        _ => {
            for (k, r) in group.members.iter().enumerate() {
                spread.frames_in_order.insert(slot + k, *r);
            }
        }
    }
    // Index fix-up: every FrameRef::Group(j) with j > group_idx
    // decrements, in frames_in_order AND in remaining groups' members.
    let fix = |r: &mut FrameRef| {
        if let FrameRef::Group(j) = r {
            if *j > group_idx {
                *j -= 1;
            }
        }
    };
    for r in spread.frames_in_order.iter_mut() {
        fix(r);
    }
    for g in spread.groups.iter_mut() {
        for r in g.members.iter_mut() {
            fix(r);
        }
    }

    Ok(AppliedOperation {
        op: Operation::DissolveGroup {
            group_id: group_id.to_string(),
            restore_slots: restore_slots.map(<[u32]>::to_vec),
        },
        inverse: Operation::CreateGroup {
            spec: GroupSpec {
                self_id: group.self_id.clone(),
                members: member_nodes,
            },
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_create_gradient(
    doc: &mut Document,
    spec: &GradientSpec,
) -> Result<AppliedOperation, OperationError> {
    let gradients = &mut doc.palette.gradients;
    let self_id = match &spec.self_id {
        Some(s) => {
            if gradients.contains_key(s) {
                return Err(OperationError::DuplicateNodeId { id: s.clone() });
            }
            s.clone()
        }
        None => {
            let mut n = gradients.len();
            let mut id = format!("Gradient/u{n}");
            while gradients.contains_key(&id) {
                n += 1;
                id = format!("Gradient/u{n}");
            }
            id
        }
    };
    gradients.insert(
        self_id.clone(),
        gradient_entry_from_spec(self_id.clone(), spec),
    );
    let mut resolved = spec.clone();
    resolved.self_id = Some(self_id.clone());
    Ok(AppliedOperation {
        op: Operation::CreateGradient { spec: resolved },
        inverse: Operation::DeleteGradient {
            gradient_id: self_id,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_edit_gradient(
    doc: &mut Document,
    gradient_id: &str,
    spec: &GradientSpec,
) -> Result<AppliedOperation, OperationError> {
    let gradients = &mut doc.palette.gradients;
    let existing =
        gradients
            .get(gradient_id)
            .ok_or_else(|| OperationError::CollectionEntryNotFound {
                collection: "gradient".to_string(),
                id: gradient_id.to_string(),
            })?;
    let prior = gradient_spec_from_entry(existing);
    gradients.insert(
        gradient_id.to_string(),
        gradient_entry_from_spec(gradient_id.to_string(), spec),
    );
    Ok(AppliedOperation {
        op: Operation::EditGradient {
            gradient_id: gradient_id.to_string(),
            spec: spec.clone(),
        },
        inverse: Operation::EditGradient {
            gradient_id: gradient_id.to_string(),
            spec: prior,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_delete_gradient(
    doc: &mut Document,
    gradient_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let captured = doc.palette.gradients.remove(gradient_id).ok_or_else(|| {
        OperationError::CollectionEntryNotFound {
            collection: "gradient".to_string(),
            id: gradient_id.to_string(),
        }
    })?;
    Ok(AppliedOperation {
        op: Operation::DeleteGradient {
            gradient_id: gradient_id.to_string(),
        },
        inverse: Operation::CreateGradient {
            spec: gradient_spec_from_entry(&captured),
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn color_group_entry_from_spec(
    self_id: String,
    spec: &ColorGroupSpec,
) -> paged_parse::graphic::ColorGroupEntry {
    paged_parse::graphic::ColorGroupEntry {
        self_id,
        name: spec.name.clone(),
        members: spec.members.clone(),
    }
}

fn apply_create_color_group(
    doc: &mut Document,
    spec: &ColorGroupSpec,
) -> Result<AppliedOperation, OperationError> {
    let groups = &mut doc.palette.color_groups;
    let self_id = match &spec.self_id {
        Some(s) => {
            if groups.contains_key(s) {
                return Err(OperationError::DuplicateNodeId { id: s.clone() });
            }
            s.clone()
        }
        None => {
            let mut n = groups.len();
            let mut id = format!("ColorGroup/u{n}");
            while groups.contains_key(&id) {
                n += 1;
                id = format!("ColorGroup/u{n}");
            }
            id
        }
    };
    groups.insert(
        self_id.clone(),
        color_group_entry_from_spec(self_id.clone(), spec),
    );
    let mut resolved = spec.clone();
    resolved.self_id = Some(self_id.clone());
    Ok(AppliedOperation {
        op: Operation::CreateColorGroup { spec: resolved },
        inverse: Operation::DeleteColorGroup { group_id: self_id },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_edit_color_group(
    doc: &mut Document,
    group_id: &str,
    spec: &ColorGroupSpec,
) -> Result<AppliedOperation, OperationError> {
    let groups = &mut doc.palette.color_groups;
    let existing = groups
        .get(group_id)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "color group".to_string(),
            id: group_id.to_string(),
        })?;
    let prior = ColorGroupSpec {
        self_id: Some(existing.self_id.clone()),
        name: existing.name.clone(),
        members: existing.members.clone(),
    };
    groups.insert(
        group_id.to_string(),
        color_group_entry_from_spec(group_id.to_string(), spec),
    );
    Ok(AppliedOperation {
        op: Operation::EditColorGroup {
            group_id: group_id.to_string(),
            spec: spec.clone(),
        },
        inverse: Operation::EditColorGroup {
            group_id: group_id.to_string(),
            spec: prior,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_delete_color_group(
    doc: &mut Document,
    group_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let captured = doc.palette.color_groups.remove(group_id).ok_or_else(|| {
        OperationError::CollectionEntryNotFound {
            collection: "color group".to_string(),
            id: group_id.to_string(),
        }
    })?;
    Ok(AppliedOperation {
        op: Operation::DeleteColorGroup {
            group_id: group_id.to_string(),
        },
        inverse: Operation::CreateColorGroup {
            spec: ColorGroupSpec {
                self_id: Some(captured.self_id.clone()),
                name: captured.name.clone(),
                members: captured.members.clone(),
            },
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

// ── Style collection mutations ────────────────────────────────────
//
// Paragraph + character styles live in `doc.styles.{paragraph,character}_styles`
// (`BTreeMap` keyed by `Self` id). The two kinds are structurally
// identical for CRUD — same `self_id`/`name`/`based_on` fields — so a
// macro emits both, differing only in the def type, the map, the id
// prefix, and the `Operation` variants. Lossless delete-undo serialises
// the captured def to JSON (`restore_json`) and the create path
// deserialises it back verbatim (the defs are `Serialize + Deserialize`).
// Like swatches, a style change can affect many frames we don't track,
// so the invalidation is the conservative `structural`.

macro_rules! style_crud {
    (
        $def:path, $map:ident, $prefix:literal,
        $create_fn:ident, $rename_fn:ident, $delete_fn:ident,
        $CreateOp:ident, $DeleteOp:ident, $RenameOp:ident, $label:literal
    ) => {
        fn $create_fn(
            doc: &mut Document,
            self_id: Option<String>,
            name: Option<String>,
            based_on: Option<String>,
            restore_json: Option<&str>,
        ) -> Result<AppliedOperation, OperationError> {
            let map = &mut doc.styles.$map;
            // Lossless-restore path (the delete inverse): the def is
            // carried whole as JSON and inserted verbatim.
            if let Some(json) = restore_json {
                let def: $def =
                    serde_json::from_str(json).map_err(|e| OperationError::InvalidValue {
                        node: NodeId::Layer(String::new()),
                        path: PropertyPath::LayerName,
                        reason: format!("malformed {} restore payload: {e}", $label),
                    })?;
                let id = def.self_id.clone();
                if map.contains_key(&id) {
                    return Err(OperationError::DuplicateNodeId { id });
                }
                map.insert(id.clone(), def);
                return Ok(AppliedOperation {
                    op: Operation::$CreateOp {
                        self_id: Some(id.clone()),
                        name: None,
                        based_on: None,
                        restore_json: Some(json.to_string()),
                    },
                    inverse: Operation::$DeleteOp { style_id: id },
                    invalidation: InvalidationHint {
                        structural: true,
                        ..Default::default()
                    },
                });
            }
            // Fresh create: build a default def carrying name/based_on;
            // every other field defaults and resolves via the cascade.
            let id = match self_id {
                Some(s) => {
                    if map.contains_key(&s) {
                        return Err(OperationError::DuplicateNodeId { id: s });
                    }
                    s
                }
                None => {
                    let mut n = map.len();
                    let mut id = format!(concat!($prefix, "/u{}"), n);
                    while map.contains_key(&id) {
                        n += 1;
                        id = format!(concat!($prefix, "/u{}"), n);
                    }
                    id
                }
            };
            // Build via `default()` + field assignment rather than a
            // struct literal: a macro `$def:path` fragment can't head a
            // struct literal in expression position.
            let mut def = <$def>::default();
            def.self_id = id.clone();
            def.name = name.clone();
            def.based_on = based_on.clone();
            map.insert(id.clone(), def);
            Ok(AppliedOperation {
                op: Operation::$CreateOp {
                    self_id: Some(id.clone()),
                    name,
                    based_on,
                    restore_json: None,
                },
                inverse: Operation::$DeleteOp { style_id: id },
                invalidation: InvalidationHint {
                    structural: true,
                    ..Default::default()
                },
            })
        }

        fn $rename_fn(
            doc: &mut Document,
            style_id: &str,
            name: &str,
        ) -> Result<AppliedOperation, OperationError> {
            let map = &mut doc.styles.$map;
            let def =
                map.get_mut(style_id)
                    .ok_or_else(|| OperationError::CollectionEntryNotFound {
                        collection: $label.to_string(),
                        id: style_id.to_string(),
                    })?;
            let prior = def.name.clone();
            def.name = Some(name.to_string());
            Ok(AppliedOperation {
                op: Operation::$RenameOp {
                    style_id: style_id.to_string(),
                    name: name.to_string(),
                },
                inverse: Operation::$RenameOp {
                    style_id: style_id.to_string(),
                    name: prior.unwrap_or_default(),
                },
                invalidation: InvalidationHint {
                    structural: true,
                    ..Default::default()
                },
            })
        }

        fn $delete_fn(
            doc: &mut Document,
            style_id: &str,
        ) -> Result<AppliedOperation, OperationError> {
            let map = &mut doc.styles.$map;
            let captured =
                map.remove(style_id)
                    .ok_or_else(|| OperationError::CollectionEntryNotFound {
                        collection: $label.to_string(),
                        id: style_id.to_string(),
                    })?;
            // Serialize the captured def for a lossless create-inverse.
            let json =
                serde_json::to_string(&captured).map_err(|e| OperationError::InvalidValue {
                    node: NodeId::Layer(String::new()),
                    path: PropertyPath::LayerName,
                    reason: format!("failed to capture {} for undo: {e}", $label),
                })?;
            Ok(AppliedOperation {
                op: Operation::$DeleteOp {
                    style_id: style_id.to_string(),
                },
                inverse: Operation::$CreateOp {
                    self_id: None,
                    name: None,
                    based_on: None,
                    restore_json: Some(json),
                },
                invalidation: InvalidationHint {
                    structural: true,
                    ..Default::default()
                },
            })
        }
    };
}

style_crud!(
    paged_parse::styles::ParagraphStyleDef,
    paragraph_styles,
    "ParagraphStyle",
    apply_create_paragraph_style,
    apply_rename_paragraph_style,
    apply_delete_paragraph_style,
    CreateParagraphStyle,
    DeleteParagraphStyle,
    RenameParagraphStyle,
    "paragraph style"
);

style_crud!(
    paged_parse::styles::CharacterStyleDef,
    character_styles,
    "CharacterStyle",
    apply_create_character_style,
    apply_rename_character_style,
    apply_delete_character_style,
    CreateCharacterStyle,
    DeleteCharacterStyle,
    RenameCharacterStyle,
    "character style"
);

style_crud!(
    paged_parse::styles::ObjectStyleDef,
    object_styles,
    "ObjectStyle",
    apply_create_object_style,
    apply_rename_object_style,
    apply_delete_object_style,
    CreateObjectStyle,
    DeleteObjectStyle,
    RenameObjectStyle,
    "object style"
);

style_crud!(
    paged_parse::styles::CellStyleDef,
    cell_styles,
    "CellStyle",
    apply_create_cell_style,
    apply_rename_cell_style,
    apply_delete_cell_style,
    CreateCellStyle,
    DeleteCellStyle,
    RenameCellStyle,
    "cell style"
);

style_crud!(
    paged_parse::styles::TableStyleDef,
    table_styles,
    "TableStyle",
    apply_create_table_style,
    apply_rename_table_style,
    apply_delete_table_style,
    CreateTableStyle,
    DeleteTableStyle,
    RenameTableStyle,
    "table style"
);

// ── Style-property editing (SetStyleProperty) ─────────────────────
//
// Edits one field on a *style definition*, reusing the PropertyPath +
// Value vocabulary so the style-options panel shares the Character /
// Paragraph leaves. Each helper returns the prior `Value` so the
// inverse is a SetStyleProperty back to it. Paragraph + character defs
// are covered (the shipped style panels); object/cell/table editing
// raises `UnsupportedProperty` for now (extensible the same way).

/// Placeholder NodeId for style-targeted errors (styles aren't nodes);
/// keeps the error's `path` meaningful while signalling the target.
fn style_node_marker(style_id: &str) -> NodeId {
    NodeId::Layer(style_id.to_string())
}

fn set_paragraph_style_field(
    def: &mut paged_parse::styles::ParagraphStyleDef,
    path: PropertyPath,
    value: &Value,
    style_id: &str,
) -> Result<Value, OperationError> {
    let type_err = || OperationError::TypeMismatch {
        path,
        expected: "value kind for this style property".to_string(),
    };
    match path {
        PropertyPath::CharacterFontSize => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.point_size);
            def.point_size = *n;
            Ok(prior)
        }
        PropertyPath::CharacterTracking => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.tracking);
            def.tracking = *n;
            Ok(prior)
        }
        PropertyPath::CharacterFillColor => {
            let Value::ColorRef(c) = value else {
                return Err(type_err());
            };
            let prior = Value::ColorRef(def.fill_color.clone());
            def.fill_color = c.clone();
            Ok(prior)
        }
        PropertyPath::ParagraphSpaceBefore => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.space_before);
            def.space_before = *n;
            Ok(prior)
        }
        PropertyPath::ParagraphSpaceAfter => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.space_after);
            def.space_after = *n;
            Ok(prior)
        }
        PropertyPath::ParagraphFirstLineIndent => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.first_line_indent);
            def.first_line_indent = *n;
            Ok(prior)
        }
        PropertyPath::ParagraphJustification => {
            let Value::Text(s) = value else {
                return Err(type_err());
            };
            let prior = Value::Text(
                def.justification
                    .map(|j| j.as_idml().to_string())
                    .unwrap_or_default(),
            );
            def.justification = paged_parse::story::Justification::from_idml(s);
            Ok(prior)
        }
        _ => Err(OperationError::UnsupportedProperty {
            node: style_node_marker(style_id),
            path,
        }),
    }
}

fn set_character_style_field(
    def: &mut paged_parse::styles::CharacterStyleDef,
    path: PropertyPath,
    value: &Value,
    style_id: &str,
) -> Result<Value, OperationError> {
    let type_err = || OperationError::TypeMismatch {
        path,
        expected: "value kind for this style property".to_string(),
    };
    match path {
        PropertyPath::CharacterFontSize => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.point_size);
            def.point_size = *n;
            Ok(prior)
        }
        PropertyPath::CharacterTracking => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.tracking);
            def.tracking = *n;
            Ok(prior)
        }
        PropertyPath::CharacterFillColor => {
            let Value::ColorRef(c) = value else {
                return Err(type_err());
            };
            let prior = Value::ColorRef(def.fill_color.clone());
            def.fill_color = c.clone();
            Ok(prior)
        }
        _ => Err(OperationError::UnsupportedProperty {
            node: style_node_marker(style_id),
            path,
        }),
    }
}

fn apply_set_style_property(
    doc: &mut Document,
    collection: StyleCollection,
    style_id: &str,
    path: PropertyPath,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    let not_found = || OperationError::CollectionEntryNotFound {
        collection: "style".to_string(),
        id: style_id.to_string(),
    };
    let prior = match collection {
        StyleCollection::Paragraph => {
            let def = doc
                .styles
                .paragraph_styles
                .get_mut(style_id)
                .ok_or_else(not_found)?;
            set_paragraph_style_field(def, path, value, style_id)?
        }
        StyleCollection::Character => {
            let def = doc
                .styles
                .character_styles
                .get_mut(style_id)
                .ok_or_else(not_found)?;
            set_character_style_field(def, path, value, style_id)?
        }
        // Object / cell / table style-property editing is a follow-up;
        // their panels are not yet built.
        StyleCollection::Object | StyleCollection::Cell | StyleCollection::Table => {
            return Err(OperationError::UnsupportedProperty {
                node: style_node_marker(style_id),
                path,
            });
        }
    };
    Ok(AppliedOperation {
        op: Operation::SetStyleProperty {
            collection,
            style_id: style_id.to_string(),
            path,
            value: value.clone(),
        },
        inverse: Operation::SetStyleProperty {
            collection,
            style_id: style_id.to_string(),
            path,
            value: prior,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_batch(
    doc: &mut Document,
    children: &[Operation],
) -> Result<AppliedOperation, OperationError> {
    let mut applied_children: Vec<AppliedOperation> = Vec::with_capacity(children.len());
    let mut combined_invalidation = InvalidationHint::default();

    for (index, child) in children.iter().enumerate() {
        match apply(doc, child) {
            Ok(applied) => {
                combined_invalidation.merge(applied.invalidation.clone());
                applied_children.push(applied);
            }
            Err(source) => {
                // Roll back already-applied children in reverse order.
                for applied in applied_children.iter().rev() {
                    // Best-effort: if rollback itself fails the doc is
                    // genuinely wedged. This shouldn't happen because
                    // we just applied the forward op and captured its
                    // inverse.
                    let _ = apply(doc, &applied.inverse);
                }
                return Err(OperationError::BatchFailed {
                    failed_at: index,
                    source: Box::new(source),
                });
            }
        }
    }

    let inverses: Vec<Operation> = applied_children.iter().map(|a| a.inverse.clone()).collect();
    let inverse = invert_batch(inverses);

    Ok(AppliedOperation {
        op: Operation::Batch {
            ops: children.to_vec(),
        },
        inverse,
        invalidation: combined_invalidation,
    })
}

// ---------------------------------------------------------------------------
// Helpers — finders + converters + constructors
// ---------------------------------------------------------------------------

fn find_text_frame_mut<'a>(doc: &'a mut Document, self_id: &str) -> Option<&'a mut TextFrame> {
    for parsed in &mut doc.spreads {
        for frame in &mut parsed.spread.text_frames {
            if frame.self_id.as_deref() == Some(self_id) {
                return Some(frame);
            }
        }
    }
    None
}

fn find_polygon_mut<'a>(doc: &'a mut Document, self_id: &str) -> Option<&'a mut Polygon> {
    for parsed in &mut doc.spreads {
        if let Some(p) = parsed
            .spread
            .polygons
            .iter_mut()
            .find(|p| p.self_id.as_deref() == Some(self_id))
        {
            return Some(p);
        }
    }
    None
}

fn find_graphic_line_mut<'a>(doc: &'a mut Document, self_id: &str) -> Option<&'a mut GraphicLine> {
    for parsed in &mut doc.spreads {
        if let Some(l) = parsed
            .spread
            .graphic_lines
            .iter_mut()
            .find(|l| l.self_id.as_deref() == Some(self_id))
        {
            return Some(l);
        }
    }
    None
}

fn find_oval_mut<'a>(doc: &'a mut Document, self_id: &str) -> Option<&'a mut paged_parse::Oval> {
    for parsed in &mut doc.spreads {
        if let Some(o) = parsed
            .spread
            .ovals
            .iter_mut()
            .find(|o| o.self_id.as_deref() == Some(self_id))
        {
            return Some(o);
        }
    }
    None
}

/// Editor-ops — resolve the gradient angle/length field a
/// `FrameGradient*` path addresses on whichever kind hosts `node`.
fn find_gradient_field_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
    path: PropertyPath,
) -> Option<&'a mut Option<f32>> {
    macro_rules! pick {
        ($item:expr) => {
            match path {
                PropertyPath::FrameGradientFillAngle => Some(&mut $item.gradient_fill_angle),
                PropertyPath::FrameGradientFillLength => Some(&mut $item.gradient_fill_length),
                PropertyPath::FrameGradientStrokeAngle => Some(&mut $item.gradient_stroke_angle),
                PropertyPath::FrameGradientStrokeLength => Some(&mut $item.gradient_stroke_length),
                _ => None,
            }
        };
    }
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).and_then(|f| pick!(f)),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).and_then(|r| pick!(r)),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).and_then(|p| pick!(p)),
        NodeId::Oval(id) => find_oval_mut(doc, id).and_then(|o| pick!(o)),
        _ => None,
    }
}

fn find_rectangle_mut<'a>(doc: &'a mut Document, self_id: &str) -> Option<&'a mut Rectangle> {
    for parsed in &mut doc.spreads {
        for rect in &mut parsed.spread.rectangles {
            if rect.self_id.as_deref() == Some(self_id) {
                return Some(rect);
            }
        }
    }
    None
}

// ---- W0.3 — enum string round-trippers (parse `from_idml`s are
// non-injective for some variants, so we name the canonical string
// explicitly rather than reusing a parse helper). -----------------

fn vj_as_idml(v: paged_parse::VerticalJustification) -> &'static str {
    use paged_parse::VerticalJustification as V;
    match v {
        V::Top => "TopAlign",
        V::Center => "CenterAlign",
        V::Bottom => "BottomAlign",
        V::Justify => "JustifyAlign",
    }
}

fn auto_sizing_as_idml(v: paged_parse::AutoSizingType) -> &'static str {
    use paged_parse::AutoSizingType as A;
    match v {
        A::Off => "Off",
        A::HeightOnly => "HeightOnly",
        A::WidthOnly => "WidthOnly",
        A::HeightAndWidth => "HeightAndWidth",
        A::HeightAndWidthProportionally => "HeightAndWidthProportionally",
    }
}

fn first_baseline_as_idml(v: paged_parse::FirstBaselineOffset) -> &'static str {
    use paged_parse::FirstBaselineOffset as F;
    match v {
        F::AscentOffset => "AscentOffset",
        F::CapHeight => "CapHeight",
        F::XHeight => "XHeight",
        F::EmBoxHeight => "EmBoxHeight",
        F::LeadingOffset => "LeadingOffset",
        F::FixedHeight => "FixedHeight",
    }
}

fn corner_option_as_idml(v: paged_parse::CornerOption) -> &'static str {
    use paged_parse::CornerOption as C;
    match v {
        C::None => "None",
        C::Rounded => "RoundedCorner",
        C::Inverse => "InverseRoundedCorner",
        C::Inset => "InsetCorner",
        C::Bevel => "BeveledCorner",
        C::Fancy => "FancyCorner",
    }
}

/// W0.3 — map a per-corner `PropertyPath` to its index in
/// `Rectangle::corners` (IDML order `[top_left, top_right,
/// bottom_right, bottom_left]`).
fn corner_index(path: PropertyPath) -> usize {
    match path {
        PropertyPath::FrameCornerOptionTopLeft | PropertyPath::FrameCornerRadiusTopLeft => 0,
        PropertyPath::FrameCornerOptionTopRight | PropertyPath::FrameCornerRadiusTopRight => 1,
        PropertyPath::FrameCornerOptionBottomRight | PropertyPath::FrameCornerRadiusBottomRight => {
            2
        }
        PropertyPath::FrameCornerOptionBottomLeft | PropertyPath::FrameCornerRadiusBottomLeft => 3,
        _ => unreachable!("corner_index called with a non-corner path"),
    }
}

/// W0.3 — locate the `stroke_type: Option<String>` field on any
/// stroked page-item kind.
fn find_stroke_type_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<String>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.stroke_type),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.stroke_type),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.stroke_type),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.stroke_type),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.stroke_type),
        _ => None,
    }
}

/// W0.3 — locate the `stroke_gap_color: Option<String>` field.
fn find_stroke_gap_color_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<String>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.stroke_gap_color),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.stroke_gap_color),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.stroke_gap_color),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.stroke_gap_color),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.stroke_gap_color),
        _ => None,
    }
}

/// W0.3 — locate the `stroke_gap_tint: Option<f32>` field.
fn find_stroke_gap_tint_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<f32>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.stroke_gap_tint),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.stroke_gap_tint),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.stroke_gap_tint),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.stroke_gap_tint),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.stroke_gap_tint),
        _ => None,
    }
}

/// W0.3 — locate the `item_transform: Option<[f32; 6]>` field on any
/// page-item kind (including Group, whose own transform decomposes).
fn find_item_transform_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<[f32; 6]>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.item_transform),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.item_transform),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.item_transform),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.item_transform),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.item_transform),
        NodeId::Group(id) => find_group_mut(doc, id).map(|g| &mut g.item_transform),
        _ => None,
    }
}

/// W0.3 — locate the `overprint_fill: bool` field (fill-bearing kinds;
/// GraphicLine has no fill, so it's excluded).
fn find_overprint_fill_mut<'a>(doc: &'a mut Document, node: &NodeId) -> Option<&'a mut bool> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.overprint_fill),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.overprint_fill),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.overprint_fill),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.overprint_fill),
        _ => None,
    }
}

/// W0.3 — locate the `overprint_stroke: bool` field (every stroked
/// kind, including GraphicLine).
fn find_overprint_stroke_mut<'a>(doc: &'a mut Document, node: &NodeId) -> Option<&'a mut bool> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.overprint_stroke),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.overprint_stroke),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.overprint_stroke),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.overprint_stroke),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.overprint_stroke),
        _ => None,
    }
}

fn find_group_mut<'a>(doc: &'a mut Document, self_id: &str) -> Option<&'a mut paged_parse::Group> {
    for parsed in &mut doc.spreads {
        for group in &mut parsed.spread.groups {
            if group.self_id.as_deref() == Some(self_id) {
                return Some(group);
            }
        }
    }
    None
}

/// SDK Phase 5 (v1 sweep) — synthesise a default DropShadowSetting
/// for the toggle-on case + per-field editors that write into a
/// prior-None state. Values mirror InDesign's "Drop Shadow"
/// preset (multiply blend, ~3pt offset, ~30% opacity).
fn default_drop_shadow() -> paged_parse::DropShadowSetting {
    paged_parse::DropShadowSetting {
        mode: "Drop".to_string(),
        x_offset: 3.0,
        y_offset: 3.0,
        size: 4.0,
        opacity_pct: 30.0,
        effect_color: None,
    }
}

// W0.4 — paint-only single-node invalidation, the shared shape every
// transparency-effect arm returns (the rasterizer re-reads the effect
// fields on the next rebuild; none of them reflow text).
fn frame_style_hint(node: &NodeId) -> InvalidationHint {
    InvalidationHint {
        frame_style: vec![node.clone()],
        ..Default::default()
    }
}

// W0.4 — InDesign-preset defaults for the non-DropShadow effect
// blocks. Materialised when a per-field editor (or the `*Enabled`
// toggle) writes into a prior-`None` block, exactly like
// `default_drop_shadow`. Values mirror InDesign's "Effects" dialog
// presets for each effect (Multiply/Screen blend, 75% opacity, the
// 120°/19° light angles, 5 pt sizes, …).

fn default_inner_shadow() -> paged_parse::InnerShadowParams {
    paged_parse::InnerShadowParams {
        x_offset: None,
        y_offset: None,
        size: Some(5.0),
        opacity_pct: Some(75.0),
        effect_color: None,
        angle_deg: Some(120.0),
        distance: Some(5.0),
        choke_pct: Some(0.0),
        blend_mode: Some("Multiply".to_string()),
        noise_pct: Some(0.0),
    }
}

fn default_outer_glow() -> paged_parse::OuterGlowParams {
    paged_parse::OuterGlowParams {
        size: Some(5.0),
        opacity_pct: Some(75.0),
        effect_color: None,
        spread_pct: Some(0.0),
        blend_mode: Some("Screen".to_string()),
        noise_pct: Some(0.0),
    }
}

fn default_inner_glow() -> paged_parse::InnerGlowParams {
    paged_parse::InnerGlowParams {
        size: Some(5.0),
        opacity_pct: Some(75.0),
        effect_color: None,
        choke_pct: Some(0.0),
        blend_mode: Some("Screen".to_string()),
        source: Some("EdgeGlow".to_string()),
        noise_pct: Some(0.0),
    }
}

fn default_bevel() -> paged_parse::BevelEmbossParams {
    paged_parse::BevelEmbossParams {
        depth_pct: Some(100.0),
        size: Some(5.0),
        angle_deg: Some(120.0),
        altitude_deg: Some(30.0),
        highlight_color: None,
        shadow_color: None,
        highlight_opacity_pct: Some(75.0),
        shadow_opacity_pct: Some(75.0),
        style: Some("InnerBevel".to_string()),
        direction: Some("Up".to_string()),
        technique: Some("Smooth".to_string()),
        soften: Some(0.0),
    }
}

fn default_satin() -> paged_parse::SatinParams {
    paged_parse::SatinParams {
        size: Some(14.0),
        angle_deg: Some(19.0),
        distance: Some(11.0),
        effect_color: None,
        opacity_pct: Some(50.0),
        blend_mode: Some("Multiply".to_string()),
        invert: Some(true),
    }
}

fn default_feather() -> paged_parse::FeatherParams {
    paged_parse::FeatherParams {
        width: Some(5.0),
        corner_type: Some("Diffusion".to_string()),
        noise_pct: Some(0.0),
        choke_pct: Some(0.0),
    }
}

fn default_directional_feather() -> paged_parse::DirectionalFeatherParams {
    paged_parse::DirectionalFeatherParams {
        left_width: Some(5.0),
        right_width: Some(5.0),
        top_width: Some(5.0),
        bottom_width: Some(5.0),
        angle_deg: Some(0.0),
        noise_pct: Some(0.0),
        choke_pct: Some(0.0),
        corner_type: None,
    }
}

/// SDK Phase 5 (v1 sweep) — locate a mutable DropShadowSetting on
/// the named page item, materialising a default on `None` so
/// per-field editors always have a target to mutate. Supports
/// TextFrame + Rectangle (the two kinds with apply arms today);
/// returns `None` for other kinds.
fn find_drop_shadow_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::DropShadowSetting> {
    let raw = node.self_id();
    for parsed in &mut doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if let Some(f) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    if f.drop_shadow.is_none() {
                        f.drop_shadow = Some(default_drop_shadow());
                    }
                    return f.drop_shadow.as_mut();
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(f) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    if f.drop_shadow.is_none() {
                        f.drop_shadow = Some(default_drop_shadow());
                    }
                    return f.drop_shadow.as_mut();
                }
            }
            _ => {}
        }
    }
    None
}

/// SDK Phase 5 (v1 sweep) — locate the `text_wrap: Option<TextWrap>`
/// field on any page-item kind that carries it. TextFrame /
/// Rectangle / Oval / Polygon / GraphicLine all do (Group doesn't —
/// the wrap rect is a leaf-item concept). Returns a mutable
/// reference so the apply arm can swap `mode` / `offsets`
/// independently while preserving the other.
fn find_text_wrap_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<paged_parse::TextWrap>> {
    let raw = node.self_id();
    for parsed in &mut doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if let Some(p) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.text_wrap);
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(p) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.text_wrap);
                }
            }
            NodeId::Oval(_) => {
                if let Some(p) = parsed
                    .spread
                    .ovals
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.text_wrap);
                }
            }
            NodeId::GraphicLine(_) => {
                if let Some(p) = parsed
                    .spread
                    .graphic_lines
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.text_wrap);
                }
            }
            NodeId::Polygon(_) => {
                if let Some(p) = parsed
                    .spread
                    .polygons
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.text_wrap);
                }
            }
            _ => {}
        }
    }
    None
}

/// SDK Phase 5 (D3 completion) — locate the `applied_object_style:
/// Option<String>` field on any page-item kind. All six page-item
/// variants carry the same field with identical semantics; this
/// helper makes the AppliedObjectStyle apply arm kind-agnostic.
fn find_applied_object_style_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<String>> {
    let raw = node.self_id();
    for parsed in &mut doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if let Some(p) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.applied_object_style);
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(p) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.applied_object_style);
                }
            }
            NodeId::Oval(_) => {
                if let Some(p) = parsed
                    .spread
                    .ovals
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.applied_object_style);
                }
            }
            NodeId::Polygon(_) => {
                if let Some(p) = parsed
                    .spread
                    .polygons
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.applied_object_style);
                }
            }
            NodeId::GraphicLine(_) => {
                if let Some(p) = parsed
                    .spread
                    .graphic_lines
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.applied_object_style);
                }
            }
            // Group does not carry an `applied_object_style` field on
            // the parse-layer struct — object styles are applied to
            // leaf items, not structural containers. Falls through.
            _ => {}
        }
    }
    None
}

fn find_spread<'a>(doc: &'a Document, self_id: &str) -> Option<&'a paged_scene::ParsedSpread> {
    doc.spreads
        .iter()
        .find(|p| p.spread.self_id.as_deref() == Some(self_id))
}

fn find_spread_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut paged_scene::ParsedSpread> {
    doc.spreads
        .iter_mut()
        .find(|p| p.spread.self_id.as_deref() == Some(self_id))
}

fn spread_parent_id(parsed: &paged_scene::ParsedSpread) -> NodeId {
    // Spreads always have a `self_id` in well-formed IDMLs; synthetic
    // test docs that omit it fall back to the manifest src path so the
    // inverse op still names the same container.
    let id = parsed
        .spread
        .self_id
        .clone()
        .unwrap_or_else(|| parsed.src.clone());
    NodeId::Spread(id)
}

/// Cheap document-wide existence check — used for duplicate-ID
/// detection on InsertNode.
fn node_exists(doc: &Document, node: &NodeId) -> bool {
    let target = node.self_id();
    for parsed in &doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if parsed
                    .spread
                    .text_frames
                    .iter()
                    .any(|f| f.self_id.as_deref() == Some(target))
                {
                    return true;
                }
            }
            NodeId::Rectangle(_) => {
                if parsed
                    .spread
                    .rectangles
                    .iter()
                    .any(|r| r.self_id.as_deref() == Some(target))
                {
                    return true;
                }
            }
            NodeId::GraphicLine(_) => {
                if parsed
                    .spread
                    .graphic_lines
                    .iter()
                    .any(|l| l.self_id.as_deref() == Some(target))
                {
                    return true;
                }
            }
            NodeId::Polygon(_) => {
                if parsed
                    .spread
                    .polygons
                    .iter()
                    .any(|p| p.self_id.as_deref() == Some(target))
                {
                    return true;
                }
            }
            NodeId::Oval(_) => {
                if parsed
                    .spread
                    .ovals
                    .iter()
                    .any(|o| o.self_id.as_deref() == Some(target))
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Track M — locate a `<Layer>` by its `Self` id in the document's
/// designmap. The designmap is the only place layers live; spread /
/// page items only carry an `ItemLayer` reference back into it.
fn find_layer_mut<'a>(doc: &'a mut Document, self_id: &str) -> Option<&'a mut paged_parse::Layer> {
    doc.container
        .designmap
        .layers
        .iter_mut()
        .find(|l| l.self_id == self_id)
}

fn expect_bool(path: PropertyPath, value: &Value) -> Result<bool, OperationError> {
    match value {
        Value::Bool(b) => Ok(*b),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "Bool".to_string(),
        }),
    }
}

fn expect_text(path: PropertyPath, value: &Value) -> Result<String, OperationError> {
    match value {
        Value::Text(s) => Ok(s.clone()),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "Text".to_string(),
        }),
    }
}

fn expect_gradient_feather(
    path: PropertyPath,
    value: &Value,
) -> Result<Option<GradientFeatherSpec>, OperationError> {
    match value {
        Value::GradientFeather(spec) => Ok(spec.clone()),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "GradientFeather".to_string(),
        }),
    }
}

/// Editor-ops — the `FrameEffects` block of an effect-bearing item,
/// materialising the default block when the item had none yet.
fn find_frame_effects_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::FrameEffects> {
    match node {
        NodeId::TextFrame(id) => {
            find_text_frame_mut(doc, id).map(|f| f.effects.get_or_insert_with(Default::default))
        }
        NodeId::Rectangle(id) => {
            find_rectangle_mut(doc, id).map(|r| r.effects.get_or_insert_with(Default::default))
        }
        NodeId::Oval(id) => {
            find_oval_mut(doc, id).map(|o| o.effects.get_or_insert_with(Default::default))
        }
        _ => None,
    }
}

// W0.4 — per-effect mutable accessors. Each locates the
// `FrameEffects` bag (materialising it + the named effect block with
// its InDesign-preset default when the prior was `None`) so the
// per-field apply arms always have a target. Mirrors
// `find_drop_shadow_mut`. Returns `None` only when the node isn't an
// effect-bearing kind (TextFrame / Rectangle / Oval).
fn find_inner_shadow_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::InnerShadowParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(
        effects
            .inner_shadow
            .get_or_insert_with(default_inner_shadow),
    )
}

fn find_outer_glow_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::OuterGlowParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(effects.outer_glow.get_or_insert_with(default_outer_glow))
}

fn find_inner_glow_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::InnerGlowParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(effects.inner_glow.get_or_insert_with(default_inner_glow))
}

fn find_bevel_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::BevelEmbossParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(effects.bevel.get_or_insert_with(default_bevel))
}

fn find_satin_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::SatinParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(effects.satin.get_or_insert_with(default_satin))
}

fn find_feather_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::FeatherParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(effects.feather.get_or_insert_with(default_feather))
}

fn find_directional_feather_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::DirectionalFeatherParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(
        effects
            .directional_feather
            .get_or_insert_with(default_directional_feather),
    )
}

// W0.4 — object-level transparency blend mode. Locates the
// `blend_mode: Option<String>` slot on the kinds that parse it
// (TextFrame / Rectangle). The `<BlendingSetting Opacity>` half is
// already wired as `FrameOpacity`.
fn find_blend_mode_mut<'a>(doc: &'a mut Document, node: &NodeId) -> Option<&'a mut Option<String>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.blend_mode),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.blend_mode),
        _ => None,
    }
}

fn expect_bounds(path: PropertyPath, value: &Value) -> Result<[f32; 4], OperationError> {
    match value {
        Value::Bounds(b) => Ok(*b),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "Bounds".to_string(),
        }),
    }
}

fn expect_color_ref(path: PropertyPath, value: &Value) -> Result<Option<String>, OperationError> {
    match value {
        Value::ColorRef(c) => Ok(c.clone()),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "ColorRef".to_string(),
        }),
    }
}

fn expect_length(path: PropertyPath, value: &Value) -> Result<Option<f32>, OperationError> {
    match value {
        Value::Length(v) => Ok(*v),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "Length".to_string(),
        }),
    }
}

fn expect_transform(path: PropertyPath, value: &Value) -> Result<Option<[f32; 6]>, OperationError> {
    match value {
        Value::Transform(m) => Ok(*m),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "Transform".to_string(),
        }),
    }
}

fn expect_path_point(
    path: PropertyPath,
    value: &Value,
) -> Result<(crate::operation::PathPointAddress, [f32; 2]), OperationError> {
    match value {
        Value::PathPoint { address, position } => Ok((*address, *position)),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "PathPoint".to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Track J — path topology helpers
// ---------------------------------------------------------------------------

/// Track J fan-out — return mutable references to the `anchors` +
/// `subpath_starts` vecs of any path-bearing page item (Polygon,
/// TextFrame, Rectangle, GraphicLine). All four kinds carry these
/// fields with identical semantics so the path-topology apply arms
/// stay kind-agnostic.
fn find_path_anchors_mut<'a>(
    doc: &'a mut paged_scene::Document,
    node: &NodeId,
) -> Option<(
    &'a mut Vec<paged_parse::PathAnchor>,
    &'a mut Vec<usize>,
    &'a mut Vec<bool>,
)> {
    let raw = node.self_id();
    for parsed in doc.spreads.iter_mut() {
        match node {
            NodeId::Polygon(_) => {
                if let Some(p) = parsed
                    .spread
                    .polygons
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some((&mut p.anchors, &mut p.subpath_starts, &mut p.subpath_open));
                }
            }
            NodeId::TextFrame(_) => {
                if let Some(p) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some((&mut p.anchors, &mut p.subpath_starts, &mut p.subpath_open));
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(p) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some((&mut p.anchors, &mut p.subpath_starts, &mut p.subpath_open));
                }
            }
            NodeId::GraphicLine(_) => {
                if let Some(p) = parsed
                    .spread
                    .graphic_lines
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some((&mut p.anchors, &mut p.subpath_starts, &mut p.subpath_open));
                }
            }
            _ => {}
        }
    }
    None
}

/// Apply rule for `subpath_starts` on Insert at flat index `n`. Each
/// entry strictly greater than `n` increments by one — entries equal
/// to or below `n` stay put, so the inserted anchor naturally joins
/// the subpath whose start index sits at-or-just-below `n`. The
/// real-world dispatch path (segment-click between two anchors of the
/// same subpath) never inserts AT a subpath boundary, so this rule is
/// sufficient. Edge cases that need a verbatim restore are handled
/// via `prev_subpath_starts` on the inverse.
fn increment_subpath_starts(starts: &mut [usize], n: usize) {
    for s in starts.iter_mut() {
        if *s > n {
            *s += 1;
        }
    }
}

/// Apply rule for `subpath_starts` on Remove at flat index `n`. Each
/// entry strictly greater than `n` decrements by one. After the
/// shift, two adjustments keep the invariant intact:
///   - any entry == `anchors.len()` (now off the end) is trimmed,
///   - adjacent equal entries are de-duped (a subpath collapsed
///     because its single anchor was the one we removed).
fn decrement_subpath_starts(starts: &mut Vec<usize>, n: usize, new_anchors_len: usize) {
    for s in starts.iter_mut() {
        if *s > n {
            *s -= 1;
        }
    }
    starts.retain(|s| *s < new_anchors_len);
    starts.dedup();
}

/// Editor-ops (Scissors) — cut the path at the anchor at flat
/// `index`. Closed subpath → opens there (the cut anchor splits into
/// two coincident endpoints, every original edge survives). Open
/// subpath, interior anchor → splits into two open subpaths sharing
/// duplicated endpoints. Inverse = verbatim restore of the snapshot
/// `(anchors, subpath_starts, subpath_open)` triple — the one path
/// topology `FramePath` cannot express (it lacks `subpath_open`).
fn apply_path_open_at(
    doc: &mut paged_scene::Document,
    node: &NodeId,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    let (index, prev_anchors, prev_starts, prev_open) = match value {
        Value::PathOpenAt {
            index,
            prev_anchors,
            prev_subpath_starts,
            prev_subpath_open,
        } => (
            *index,
            prev_anchors.clone(),
            prev_subpath_starts.clone(),
            prev_subpath_open.clone(),
        ),
        _ => {
            return Err(OperationError::TypeMismatch {
                path: PropertyPath::PathOpenAt,
                expected: "PathOpenAt".to_string(),
            })
        }
    };
    let (anchors, subpath_starts, subpath_open) = find_path_anchors_mut(doc, node)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;

    // Snapshot for the inverse (and for the restore branch's own
    // inverse, keeping redo working).
    let snap_anchors: Vec<PathAnchorSpec> =
        anchors.iter().map(PathAnchorSpec::from_parse).collect();
    let snap_starts = subpath_starts.clone();
    let snap_open = subpath_open.clone();

    if let (Some(ra), Some(rs), Some(ro)) = (prev_anchors, prev_starts, prev_open) {
        // Inverse/redo path — restore the carried triple verbatim.
        *anchors = ra.iter().map(PathAnchorSpec::to_parse).collect();
        *subpath_starts = rs;
        *subpath_open = ro;
    } else {
        if anchors.is_empty() || index >= anchors.len() {
            return Err(OperationError::InvalidValue {
                node: node.clone(),
                path: PropertyPath::PathOpenAt,
                reason: format!("anchor index {index} out of range"),
            });
        }
        // Normalise the parallel tables: a path with no explicit
        // boundaries is one closed contour starting at 0.
        if subpath_starts.is_empty() {
            subpath_starts.push(0);
        }
        if subpath_open.len() < subpath_starts.len() {
            subpath_open.resize(subpath_starts.len(), false);
        }
        // Locate the subpath containing `index`.
        let s = subpath_starts
            .iter()
            .rposition(|&start| start <= index)
            .unwrap_or(0);
        let start = subpath_starts[s];
        let end = subpath_starts.get(s + 1).copied().unwrap_or(anchors.len());
        let len = end - start;
        if len < 2 {
            return Err(OperationError::InvalidValue {
                node: node.clone(),
                path: PropertyPath::PathOpenAt,
                reason: "cannot cut a degenerate single-anchor contour".to_string(),
            });
        }

        if !subpath_open[s] {
            // CLOSED → open at `index`: rotate the slice so the cut
            // anchor leads, then append its coincident twin so every
            // original edge survives (InDesign scissors-at-anchor).
            anchors[start..end].rotate_left(index - start);
            let mut head = anchors[start];
            let mut tail = head;
            // The head keeps the outgoing handle; the tail keeps the
            // incoming one. The severed sides collapse to endpoints.
            head.left = head.anchor;
            tail.right = tail.anchor;
            anchors[start] = head;
            anchors.insert(end, tail);
            subpath_open[s] = true;
            // Later subpath boundaries shift by the inserted twin.
            for boundary in subpath_starts.iter_mut().skip(s + 1) {
                *boundary += 1;
            }
        } else {
            // OPEN → split at an interior anchor into two open halves
            // sharing duplicated endpoints.
            if index == start || index == end - 1 {
                return Err(OperationError::InvalidValue {
                    node: node.clone(),
                    path: PropertyPath::PathOpenAt,
                    reason: "cutting an open contour at its endpoint is a no-op".to_string(),
                });
            }
            let mut first_half_end = anchors[index];
            let mut second_half_start = first_half_end;
            first_half_end.right = first_half_end.anchor;
            second_half_start.left = second_half_start.anchor;
            anchors[index] = first_half_end;
            anchors.insert(index + 1, second_half_start);
            // New boundary where the second half begins.
            subpath_starts.insert(s + 1, index + 1);
            subpath_open.insert(s + 1, true);
            for boundary in subpath_starts.iter_mut().skip(s + 2) {
                *boundary += 1;
            }
        }
    }

    let inverse = Operation::SetProperty {
        node: node.clone(),
        path: PropertyPath::PathOpenAt,
        value: Value::PathOpenAt {
            index,
            prev_anchors: Some(snap_anchors),
            prev_subpath_starts: Some(snap_starts),
            prev_subpath_open: Some(snap_open),
        },
    };
    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::PathOpenAt,
            value: value.clone(),
        },
        inverse,
        invalidation: InvalidationHint {
            frame_geometry: vec![node.clone()],
            ..Default::default()
        },
    })
}

/// B-05 — shared apply for the whole-path-replacing kernel ops
/// (`OutlineStroke` / `OffsetPath` / `SimplifyPath`). Identical
/// snapshot-inverse convention to `apply_path_open_at`: the inverse
/// carries the verbatim `(anchors, subpath_starts, subpath_open)`
/// triple, and a value arriving WITH the triple is the restore
/// branch (undo/redo).
fn apply_path_kernel_op(
    doc: &mut paged_scene::Document,
    node: &NodeId,
    path: &PropertyPath,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    use crate::kurbo_kernel::{self, StrokeCap, StrokeJoin};

    fn parse_cap(s: &str) -> Option<StrokeCap> {
        match s {
            "butt" => Some(StrokeCap::Butt),
            "round" => Some(StrokeCap::Round),
            "square" => Some(StrokeCap::Square),
            _ => None,
        }
    }
    fn parse_join(s: &str) -> Option<StrokeJoin> {
        match s {
            "miter" => Some(StrokeJoin::Miter),
            "round" => Some(StrokeJoin::Round),
            "bevel" => Some(StrokeJoin::Bevel),
            _ => None,
        }
    }

    let invalid = |reason: String| OperationError::InvalidValue {
        node: node.clone(),
        path: *path,
        reason,
    };

    // Destructure the prev triple uniformly across the three values.
    let (prev_anchors, prev_starts, prev_open) = match value {
        Value::OutlineStroke {
            prev_anchors,
            prev_subpath_starts,
            prev_subpath_open,
            ..
        }
        | Value::OffsetPath {
            prev_anchors,
            prev_subpath_starts,
            prev_subpath_open,
            ..
        }
        | Value::SimplifyPath {
            prev_anchors,
            prev_subpath_starts,
            prev_subpath_open,
            ..
        } => (
            prev_anchors.clone(),
            prev_subpath_starts.clone(),
            prev_subpath_open.clone(),
        ),
        _ => {
            return Err(OperationError::TypeMismatch {
                path: *path,
                expected: "OutlineStroke | OffsetPath | SimplifyPath".to_string(),
            })
        }
    };

    let (anchors, subpath_starts, subpath_open) = find_path_anchors_mut(doc, node)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;

    let snap_anchors: Vec<PathAnchorSpec> =
        anchors.iter().map(PathAnchorSpec::from_parse).collect();
    let snap_starts = subpath_starts.clone();
    let snap_open = subpath_open.clone();

    if let (Some(ra), Some(rs), Some(ro)) = (prev_anchors, prev_starts, prev_open) {
        // Restore branch (undo/redo): verbatim triple.
        *anchors = ra.iter().map(PathAnchorSpec::to_parse).collect();
        *subpath_starts = rs;
        *subpath_open = ro;
    } else {
        // Normalise parallel tables (one closed contour by default).
        if subpath_starts.is_empty() {
            subpath_starts.push(0);
        }
        if subpath_open.len() < subpath_starts.len() {
            subpath_open.resize(subpath_starts.len(), false);
        }
        let result = match value {
            Value::OutlineStroke {
                width,
                cap,
                join,
                miter_limit,
                ..
            } => kurbo_kernel::outline_stroke(
                anchors,
                subpath_starts,
                subpath_open,
                *width,
                parse_cap(cap).ok_or_else(|| invalid(format!("unknown cap \"{cap}\"")))?,
                parse_join(join).ok_or_else(|| invalid(format!("unknown join \"{join}\"")))?,
                *miter_limit,
            ),
            Value::OffsetPath {
                delta,
                join,
                miter_limit,
                ..
            } => kurbo_kernel::offset_closed_path(
                anchors,
                subpath_starts,
                subpath_open,
                *delta,
                parse_join(join).ok_or_else(|| invalid(format!("unknown join \"{join}\"")))?,
                *miter_limit,
            ),
            Value::SimplifyPath { tolerance, .. } => {
                kurbo_kernel::simplify_path(anchors, subpath_starts, subpath_open, *tolerance)
            }
            _ => unreachable!("matched above"),
        };
        let (na, ns, no) = result.ok_or_else(|| {
            invalid(
                "kernel produced no result (degenerate input, open path \
                 where closed is required, or an offset past the medial axis)"
                    .to_string(),
            )
        })?;
        *anchors = na;
        *subpath_starts = ns;
        *subpath_open = no;
    }

    // Inverse: same params, prev triple filled.
    let with_prev = |v: &Value| -> Value {
        match v.clone() {
            Value::OutlineStroke {
                width,
                cap,
                join,
                miter_limit,
                ..
            } => Value::OutlineStroke {
                width,
                cap,
                join,
                miter_limit,
                prev_anchors: Some(snap_anchors.clone()),
                prev_subpath_starts: Some(snap_starts.clone()),
                prev_subpath_open: Some(snap_open.clone()),
            },
            Value::OffsetPath {
                delta,
                join,
                miter_limit,
                ..
            } => Value::OffsetPath {
                delta,
                join,
                miter_limit,
                prev_anchors: Some(snap_anchors.clone()),
                prev_subpath_starts: Some(snap_starts.clone()),
                prev_subpath_open: Some(snap_open.clone()),
            },
            Value::SimplifyPath { tolerance, .. } => Value::SimplifyPath {
                tolerance,
                prev_anchors: Some(snap_anchors.clone()),
                prev_subpath_starts: Some(snap_starts.clone()),
                prev_subpath_open: Some(snap_open.clone()),
            },
            other => other,
        }
    };

    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path: *path,
            value: value.clone(),
        },
        inverse: Operation::SetProperty {
            node: node.clone(),
            path: *path,
            value: with_prev(value),
        },
        invalidation: InvalidationHint {
            frame_geometry: vec![node.clone()],
            ..Default::default()
        },
    })
}

fn apply_path_point_insert(
    doc: &mut paged_scene::Document,
    node: &NodeId,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    let (index, anchor_spec, prev_subpath_starts) = match value {
        Value::PathPointInsert {
            index,
            anchor,
            prev_subpath_starts,
        } => (*index, *anchor, prev_subpath_starts.clone()),
        _ => {
            return Err(OperationError::TypeMismatch {
                path: PropertyPath::PathPointInsert,
                expected: "PathPointInsert".to_string(),
            })
        }
    };
    let (anchors, subpath_starts, _open) = find_path_anchors_mut(doc, node)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    // Insert is allowed at end (index == len), not past it.
    if index > anchors.len() {
        return Err(OperationError::NodeNotFound(node.clone()));
    }
    anchors.insert(index, anchor_spec.to_parse());
    if let Some(restore) = prev_subpath_starts {
        // Inverse-of-Remove case: restore the pre-Remove starts
        // verbatim. The starts captured at Remove time pointed into
        // an anchors vec one element smaller; inserting brings the
        // length back, so the snapshot is valid as-is.
        *subpath_starts = restore;
    } else {
        increment_subpath_starts(subpath_starts, index);
    }
    // Inverse: remove the just-inserted anchor at the same index.
    // No prev_subpath_starts on the inverse — the forward Insert's
    // increment rule was non-collapsing, so the decrement rule
    // reverses it exactly.
    let inverse = Operation::SetProperty {
        node: node.clone(),
        path: PropertyPath::PathPointRemove,
        value: Value::PathPointRemove {
            index,
            prev_subpath_starts: None,
        },
    };
    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::PathPointInsert,
            value: value.clone(),
        },
        inverse,
        invalidation: InvalidationHint {
            frame_geometry: vec![node.clone()],
            ..Default::default()
        },
    })
}

fn apply_path_point_remove(
    doc: &mut paged_scene::Document,
    node: &NodeId,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    let index = match value {
        Value::PathPointRemove { index, .. } => *index,
        _ => {
            return Err(OperationError::TypeMismatch {
                path: PropertyPath::PathPointRemove,
                expected: "PathPointRemove".to_string(),
            })
        }
    };
    let (anchors, subpath_starts, _open) = find_path_anchors_mut(doc, node)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    if index >= anchors.len() {
        return Err(OperationError::NodeNotFound(node.clone()));
    }
    // Capture for the inverse BEFORE mutating.
    let captured = crate::operation::PathAnchorSpec::from_parse(&anchors[index]);
    let prev_starts = subpath_starts.clone();
    // Remove + adjust subpath_starts.
    anchors.remove(index);
    let new_len = anchors.len();
    decrement_subpath_starts(subpath_starts, index, new_len);
    // Inverse: re-insert the captured anchor at the same index, and
    // restore subpath_starts verbatim so a Remove that collapsed a
    // degenerate single-anchor subpath round-trips bytewise.
    let inverse = Operation::SetProperty {
        node: node.clone(),
        path: PropertyPath::PathPointInsert,
        value: Value::PathPointInsert {
            index,
            anchor: captured,
            prev_subpath_starts: Some(prev_starts),
        },
    };
    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::PathPointRemove,
            value: value.clone(),
        },
        inverse,
        invalidation: InvalidationHint {
            frame_geometry: vec![node.clone()],
            ..Default::default()
        },
    })
}

fn apply_path_point_curve_type(
    doc: &mut paged_scene::Document,
    node: &NodeId,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    let (index, smooth, prev_override) = match value {
        Value::PathPointCurveType {
            index,
            smooth,
            prev,
        } => (*index, *smooth, *prev),
        _ => {
            return Err(OperationError::TypeMismatch {
                path: PropertyPath::PathPointCurveType,
                expected: "PathPointCurveType".to_string(),
            })
        }
    };
    let (anchors, subpath_starts, _open) = find_path_anchors_mut(doc, node)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    if index >= anchors.len() {
        return Err(OperationError::NodeNotFound(node.clone()));
    }
    // Neighbour positions for the smooth derivation, restricted to
    // the same subpath. (Crossing subpath boundaries would derive a
    // tangent against an anchor on a different contour, which is
    // nonsensical.)
    let (sub_start, sub_end) = subpath_bounds_for(subpath_starts, anchors.len(), index);
    let prev_neighbour = if index > sub_start {
        Some(anchors[index - 1].anchor)
    } else {
        None
    };
    let next_neighbour = if index + 1 < sub_end {
        Some(anchors[index + 1].anchor)
    } else {
        None
    };
    let captured = crate::operation::PathAnchorSpec::from_parse(&anchors[index]);
    let anchor = &mut anchors[index];
    if let Some(restore) = prev_override {
        // Inverse-application path: restore the carried anchor.
        anchor.left = (restore.left[0], restore.left[1]);
        anchor.right = (restore.right[0], restore.right[1]);
        // anchor.anchor (on-curve point) is preserved on a curve-type
        // toggle, but restore it too for safety against any edge
        // case where neighbour-derivation rounded it.
        anchor.anchor = (restore.anchor[0], restore.anchor[1]);
    } else if smooth {
        let curr = [anchor.anchor.0, anchor.anchor.1];
        // Need both neighbours; fall back to corner if either is
        // missing (open-path endpoint).
        match (prev_neighbour, next_neighbour) {
            (Some(p), Some(n)) => {
                let p = [p.0, p.1];
                let n = [n.0, n.1];
                let (l, r) = crate::path_math::smooth_handles_from_neighbours(p, curr, n);
                anchor.left = (l[0], l[1]);
                anchor.right = (r[0], r[1]);
            }
            _ => {
                anchor.left = anchor.anchor;
                anchor.right = anchor.anchor;
            }
        }
    } else {
        // Corner: collapse handles onto the anchor.
        anchor.left = anchor.anchor;
        anchor.right = anchor.anchor;
    }
    // Inverse: CurveType with `prev: Some(captured)` so undo
    // restores the exact prior handles regardless of what the
    // smooth-derivation produced.
    let inverse = Operation::SetProperty {
        node: node.clone(),
        path: PropertyPath::PathPointCurveType,
        value: Value::PathPointCurveType {
            index,
            smooth: !smooth,
            prev: Some(captured),
        },
    };
    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::PathPointCurveType,
            value: value.clone(),
        },
        inverse,
        invalidation: InvalidationHint {
            frame_geometry: vec![node.clone()],
            ..Default::default()
        },
    })
}

/// Return the half-open `[start, end)` index range of the subpath
/// containing `index`. The end is either the next subpath's start or
/// `anchors_len` for the last subpath. An empty `subpath_starts`
/// represents a single implicit subpath covering all anchors.
fn subpath_bounds_for(starts: &[usize], anchors_len: usize, index: usize) -> (usize, usize) {
    if starts.is_empty() {
        return (0, anchors_len);
    }
    // Find the largest start <= index.
    let pos = match starts.binary_search(&index) {
        Ok(p) => p,
        Err(p) => p.saturating_sub(1),
    };
    let start = starts[pos];
    let end = starts.get(pos + 1).copied().unwrap_or(anchors_len);
    (start, end)
}

/// Phase H — dedicated apply path for `NodeSpec::CloneTranslate`.
/// Ignores `parent` (the gesture-spine caller doesn't carry it) and
/// finds the source's host spread globally. Inserts the clone there,
/// shifted by `(dx, dy)` in bounds (un-rotated) or
/// `item_transform.tx/ty` (rotated).
fn apply_insert_clone_translate(
    doc: &mut Document,
    position: usize,
    spec: &NodeSpec,
) -> Result<AppliedOperation, OperationError> {
    let NodeSpec::CloneTranslate {
        self_id,
        source,
        dx,
        dy,
        destination_spread_id,
    } = spec
    else {
        unreachable!("apply_insert_clone_translate called with non-clone spec");
    };
    let new_node_id = spec.node_id();
    if node_exists(doc, &new_node_id) {
        return Err(OperationError::DuplicateNodeId {
            id: self_id.clone(),
        });
    }
    // Find the spread containing the source frame.
    let source_spread_idx = match source {
        NodeId::TextFrame(src_id) => doc.spreads.iter().position(|s| {
            s.spread
                .text_frames
                .iter()
                .any(|f| f.self_id.as_deref() == Some(src_id.as_str()))
        }),
        NodeId::Rectangle(src_id) => doc.spreads.iter().position(|s| {
            s.spread
                .rectangles
                .iter()
                .any(|r| r.self_id.as_deref() == Some(src_id.as_str()))
        }),
        _ => None,
    };
    let Some(src_idx) = source_spread_idx else {
        return Err(OperationError::NodeNotFound(source.clone()));
    };

    // Track K — resolve the destination spread. Default (None) is
    // the source's spread (Phase H.4 behaviour). When Some, locate
    // the dest by self_id and compute the additional spread-origin
    // offset so the clone's per-spread-local bounds land at the
    // pointer's WORLD position regardless of cross-spread move.
    let (dest_idx, eff_dx, eff_dy) = match destination_spread_id {
        None => (src_idx, *dx, *dy),
        Some(dest_id) => {
            let dest_idx = doc
                .spreads
                .iter()
                .position(|s| s.spread.self_id.as_deref() == Some(dest_id.as_str()))
                .ok_or_else(|| OperationError::NodeNotFound(NodeId::Spread(dest_id.clone())))?;
            // Each spread's item_transform maps its inner coords
            // into the pasteboard. We only need the translation
            // component; InDesign limits spread transforms to
            // translation + 0/90/180/270 rotation (paged-parse spread.rs:81).
            // Real-world IDMLs are translation-only at the spread
            // level, so the additive correction is exact in the
            // common case.
            let src_origin = spread_origin(&doc.spreads[src_idx].spread.item_transform);
            let dest_origin = spread_origin(&doc.spreads[dest_idx].spread.item_transform);
            (
                dest_idx,
                *dx + src_origin.0 - dest_origin.0,
                *dy + src_origin.1 - dest_origin.1,
            )
        }
    };

    // Capture the parent spread id BEFORE the source clone (the
    // borrow for cloning needs to read self.spreads[src_idx], so
    // we can't hold a separate &mut to the destination yet).
    let parent_spread_id = doc.spreads[dest_idx]
        .spread
        .self_id
        .clone()
        .unwrap_or_default();
    match source {
        NodeId::TextFrame(src_id) => {
            let src_frame: TextFrame = doc.spreads[src_idx]
                .spread
                .text_frames
                .iter()
                .find(|f| f.self_id.as_deref() == Some(src_id.as_str()))
                .cloned()
                .ok_or_else(|| OperationError::NodeNotFound(source.clone()))?;
            let mut clone = src_frame;
            clone.self_id = Some(self_id.clone());
            apply_translate_in_place(&mut clone.bounds, &mut clone.item_transform, eff_dx, eff_dy);
            let dest_spread = &mut doc.spreads[dest_idx];
            let len = dest_spread.spread.text_frames.len();
            let pos = position.min(len);
            dest_spread.spread.text_frames.insert(pos, clone);
            // Duplicates stack on top, like InDesign's Alt-drag.
            register_frame_ref(&mut dest_spread.spread, FrameRef::TextFrame(0), pos, None);
        }
        NodeId::Rectangle(src_id) => {
            let src_rect: Rectangle = doc.spreads[src_idx]
                .spread
                .rectangles
                .iter()
                .find(|r| r.self_id.as_deref() == Some(src_id.as_str()))
                .cloned()
                .ok_or_else(|| OperationError::NodeNotFound(source.clone()))?;
            let mut clone = src_rect;
            clone.self_id = Some(self_id.clone());
            apply_translate_in_place(&mut clone.bounds, &mut clone.item_transform, eff_dx, eff_dy);
            let dest_spread = &mut doc.spreads[dest_idx];
            let len = dest_spread.spread.rectangles.len();
            let pos = position.min(len);
            dest_spread.spread.rectangles.insert(pos, clone);
            // Duplicates stack on top, like InDesign's Alt-drag.
            register_frame_ref(&mut dest_spread.spread, FrameRef::Rectangle(0), pos, None);
        }
        other => {
            return Err(OperationError::UnsupportedProperty {
                node: other.clone(),
                path: PropertyPath::FrameBounds,
            });
        }
    }
    let invalidation = InvalidationHint {
        structural: true,
        ..Default::default()
    };
    let inverse = invert_insert_node(spec);
    Ok(AppliedOperation {
        op: Operation::InsertNode {
            parent: NodeId::Spread(parent_spread_id),
            position,
            node: spec.clone(),
            z_slot: None,
        },
        inverse,
        invalidation,
    })
}

/// Track K — extract a spread's translation origin from its
/// `item_transform`. Returns (0, 0) when the transform is absent
/// (identity per the spec). Rotation is ignored — InDesign limits
/// spread transforms to translation + cardinal rotation, and the
/// pasteboard-mapping cases that real IDMLs ship are all
/// translation-only.
fn spread_origin(item_transform: &Option<[f32; 6]>) -> (f32, f32) {
    match item_transform {
        Some(m) => (m[4], m[5]),
        None => (0.0, 0.0),
    }
}

/// Phase H — shift either the bounds (un-rotated frame) or the
/// `item_transform`'s tx/ty (rotated frame) so the cloned frame
/// lands at the user's drop position regardless of frame rotation.
fn apply_translate_in_place(
    bounds: &mut Bounds,
    item_transform: &mut Option<[f32; 6]>,
    dx: f32,
    dy: f32,
) {
    let rotated = match item_transform {
        None => false,
        Some(m) => {
            let a = m[0];
            let b = m[1];
            let c = m[2];
            let d = m[3];
            !((a - 1.0).abs() < 1e-4 && (d - 1.0).abs() < 1e-4 && b.abs() < 1e-4 && c.abs() < 1e-4)
        }
    };
    if rotated {
        if let Some(m) = item_transform.as_mut() {
            m[4] += dx;
            m[5] += dy;
        }
    } else {
        bounds.top += dy;
        bounds.left += dx;
        bounds.bottom += dy;
        bounds.right += dx;
    }
}

fn bounds_to_array(b: Bounds) -> [f32; 4] {
    [b.top, b.left, b.bottom, b.right]
}

fn bounds_from_array(a: [f32; 4]) -> Bounds {
    Bounds {
        top: a[0],
        left: a[1],
        bottom: a[2],
        right: a[3],
    }
}

/// Build a TextFrame with the Stage-1 supported field set populated
/// and everything else at the parse-layer's sensible defaults. The
/// `parent_story`, transform, drop-shadow, vertical-justify, and
/// other rich fields stay `None`/empty — adding them is the natural
/// extension as the inspector grows.
pub(crate) fn new_text_frame(
    self_id: String,
    bounds: Bounds,
    fill_color: Option<String>,
) -> TextFrame {
    TextFrame {
        self_id: Some(self_id),
        parent_story: None,
        bounds,
        item_transform: None,
        fill_color,
        fill_tint: None,
        stroke_color: None,
        stroke_weight: None,
        stroke_type: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        drop_shadow: None,
        stroke_drop_shadow: None,
        next_text_frame: None,
        vertical_justification: None,
        first_baseline_offset: None,
        minimum_first_baseline_offset: None,
        inset_spacing: None,
        auto_sizing: None,
        auto_sizing_reference_point: None,
        minimum_width_for_auto_sizing: None,
        minimum_height_for_auto_sizing: None,
        use_minimum_height_for_auto_sizing: None,
        column_count: None,
        column_gutter: None,
        column_balance: None,
        applied_object_style: None,
        text_wrap: None,
        item_layer: None,
        is_anchored: false,
        opacity: None,
        blend_mode: None,
        anchors: Vec::new(),
        subpath_starts: Vec::new(),
        subpath_open: Vec::new(),
        effects: None,
        gradient_fill_angle: None,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        applied_toc_style: None,
        overprint_fill: false,
        overprint_stroke: false,
        nonprinting: false,
    }
}

fn new_graphic_line(
    self_id: String,
    bounds: Bounds,
    anchors: Vec<paged_parse::PathAnchor>,
    subpath_starts: Vec<usize>,
    subpath_open: Vec<bool>,
    stroke_color: Option<String>,
    stroke_weight: Option<f32>,
) -> GraphicLine {
    GraphicLine {
        self_id: Some(self_id),
        bounds,
        item_transform: None,
        stroke_color,
        stroke_weight,
        stroke_type: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        applied_object_style: None,
        text_wrap: None,
        item_layer: None,
        anchors,
        subpath_starts,
        subpath_open,
        text_paths: Vec::new(),
        effects: None,
        overprint_stroke: false,
        nonprinting: false,
        start_arrow: paged_parse::ArrowheadType::None,
        end_arrow: paged_parse::ArrowheadType::None,
        start_arrow_scale: 100.0,
        end_arrow_scale: 100.0,
    }
}

#[allow(clippy::too_many_arguments)]
fn new_polygon(
    self_id: String,
    bounds: Bounds,
    anchors: Vec<paged_parse::PathAnchor>,
    subpath_starts: Vec<usize>,
    subpath_open: Vec<bool>,
    fill_color: Option<String>,
    stroke_color: Option<String>,
    stroke_weight: Option<f32>,
) -> Polygon {
    Polygon {
        self_id: Some(self_id),
        bounds,
        item_transform: None,
        fill_color,
        fill_tint: None,
        stroke_color,
        stroke_weight,
        stroke_type: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        applied_object_style: None,
        anchors,
        subpath_starts,
        subpath_open,
        text_wrap: None,
        item_layer: None,
        effects: None,
        gradient_fill_angle: None,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        opacity: None,
        blend_mode: None,
        text_paths: Vec::new(),
        image_link: None,
        has_image_element: false,
        has_inline_pdf: false,
        image_item_transform: None,
        image_bytes: None,
        overprint_fill: false,
        overprint_stroke: false,
        nonprinting: false,
    }
}

pub(crate) fn new_oval(self_id: String, bounds: Bounds, fill_color: Option<String>) -> Oval {
    Oval {
        self_id: Some(self_id),
        bounds,
        item_transform: None,
        fill_color,
        fill_tint: None,
        stroke_color: None,
        stroke_weight: None,
        stroke_type: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        drop_shadow: None,
        stroke_drop_shadow: None,
        applied_object_style: None,
        text_wrap: None,
        item_layer: None,
        effects: None,
        gradient_fill_angle: None,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        opacity: None,
        blend_mode: None,
        image_link: None,
        has_image_element: false,
        has_inline_pdf: false,
        image_item_transform: None,
        image_bytes: None,
        overprint_fill: false,
        overprint_stroke: false,
        nonprinting: false,
    }
}

pub(crate) fn new_rectangle(
    self_id: String,
    bounds: Bounds,
    fill_color: Option<String>,
) -> Rectangle {
    Rectangle {
        self_id: Some(self_id),
        bounds,
        item_transform: None,
        fill_color,
        fill_tint: None,
        stroke_color: None,
        stroke_weight: None,
        drop_shadow: None,
        stroke_drop_shadow: None,
        image_link: None,
        has_image_element: false,
        has_inline_pdf: false,
        image_item_transform: None,
        image_bytes: None,
        applied_object_style: None,
        text_wrap: None,
        frame_fitting: None,
        stroke_type: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        stroke_alignment: None,
        end_cap: None,
        end_join: None,
        miter_limit: None,
        item_layer: None,
        corner_radius: None,
        corner_option: None,
        corners: Default::default(),
        is_anchored: false,
        opacity: None,
        blend_mode: None,
        effects: None,
        gradient_fill_angle: None,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        text_paths: Vec::new(),
        overprint_fill: false,
        overprint_stroke: false,
        nonprinting: false,
        anchors: Vec::new(),
        subpath_starts: Vec::new(),
        subpath_open: Vec::new(),
    }
}

// ===========================================================================
// W0.5 — wire-expansion operations
// ===========================================================================

/// Locate a `TextFrame` by `Self` id across every spread (mut). Returns
/// the spread index + frame index for an O(1)-ish revisit.
fn find_text_frame_pos(doc: &Document, frame_id: &str) -> Option<(usize, usize)> {
    for (si, parsed) in doc.spreads.iter().enumerate() {
        if let Some(fi) = parsed
            .spread
            .text_frames
            .iter()
            .position(|f| f.self_id.as_deref() == Some(frame_id))
        {
            return Some((si, fi));
        }
    }
    None
}

/// Build a `text_reflow` invalidation hint targeting the frame hosting
/// `story_id` (best-effort; default hint when the story has no frame).
fn reflow_hint_for_story(doc: &Document, story_id: &str) -> InvalidationHint {
    match doc.frame_for_story.get(story_id) {
        Some(frame) => match &frame.self_id {
            Some(self_id) => InvalidationHint {
                text_reflow: vec![NodeId::TextFrame(self_id.clone())],
                ..Default::default()
            },
            None => InvalidationHint::default(),
        },
        None => InvalidationHint::default(),
    }
}

// ===========================================================================
// W3.A1 — table NodeId surface (addressing + mutation)
// ===========================================================================

use crate::operation::{RemovedTableLine, TableCellSpec, TableColumnSpec, TableRowSpec};

/// W3.A1 — locate a `<Table>` inside a story by `(story_id, table_id)`.
/// Returns the story index + the host paragraph index. Tables hang off
/// `Paragraph::table`, so the table lives at
/// `doc.stories[si].story.paragraphs[pi].table`.
fn find_table_pos(doc: &Document, story_id: &str, table_id: &str) -> Option<(usize, usize)> {
    let si = doc.stories.iter().position(|s| s.self_id == story_id)?;
    let pi = doc.stories[si]
        .story
        .paragraphs
        .iter()
        .position(|p| p.table.as_ref().and_then(|t| t.self_id.as_deref()) == Some(table_id))?;
    Some((si, pi))
}

/// Mutable access to a located `<Table>`. `find_table_pos` first, then
/// reborrow mutably (the position scan is immutable).
fn find_table_mut<'a>(
    doc: &'a mut Document,
    story_id: &str,
    table_id: &str,
) -> Option<&'a mut paged_parse::Table> {
    let (si, pi) = find_table_pos(doc, story_id, table_id)?;
    doc.stories[si].story.paragraphs[pi].table.as_mut()
}

/// W3.A1 — `AppliedTableStyle` write on a `NodeId::Table`. The only
/// table-scoped `SetProperty` path today (row/column/structure edits
/// are their own Operations). Empty string clears the override.
fn apply_table_property(
    doc: &mut Document,
    node: &NodeId,
    path: PropertyPath,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    let NodeId::Table { story_id, table_id } = node else {
        unreachable!("apply_table_property dispatched on a non-Table node");
    };
    if path != PropertyPath::AppliedTableStyle {
        return Err(OperationError::UnsupportedProperty {
            node: node.clone(),
            path,
        });
    }
    let new = expect_text(path, value)?;
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let prev = table.applied_table_style.clone().unwrap_or_default();
    table.applied_table_style = if new.is_empty() { None } else { Some(new) };

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path,
            value: value.clone(),
        },
        inverse: Operation::SetProperty {
            node: node.clone(),
            path,
            value: Value::Text(prev),
        },
        invalidation,
    })
}

/// W3.A1 — scalar cell-property write on a `NodeId::TableCell`. The
/// `(row, col)` index lives on the NodeId; the path picks the field.
/// Builds its own inverse (carrying the prior value) so undo
/// round-trips. The host story reflows on any cell edit.
fn apply_cell_property(
    doc: &mut Document,
    node: &NodeId,
    path: PropertyPath,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    let NodeId::TableCell {
        story_id,
        table_id,
        row,
        col,
    } = node
    else {
        unreachable!("apply_cell_property dispatched on a non-TableCell node");
    };
    // Resolve the cell. Cells are keyed by `Name="col:row"`; match on
    // the parsed `(col, row)` coords.
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let cell = table
        .cells
        .iter_mut()
        .find(|c| c.coords() == Some((*col, *row)))
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;

    // Apply by path, capturing the prior value as the inverse payload.
    let inverse_value = match path {
        PropertyPath::CellFillColor => {
            let new = expect_color_ref(path, value)?;
            let prev = cell.fill_color.clone();
            cell.fill_color = new;
            Value::ColorRef(prev)
        }
        // v1: IDML has no standalone per-cell fill-tint attribute;
        // the parse side doesn't model one. We accept the path for the
        // wire surface but route it through the cell-style cascade —
        // there's nothing to store, so the write is a typed no-op that
        // still round-trips (inverse restores the same absent value).
        // Honest scope: a real fill-tint needs a parse field; deferred.
        PropertyPath::CellFillTint => {
            let _ = expect_length(path, value)?;
            Value::Length(None)
        }
        PropertyPath::CellInsetTop => {
            let new = expect_length(path, value)?.unwrap_or(0.0);
            let prev = cell.text_top_inset;
            cell.text_top_inset = new;
            Value::Length(Some(prev))
        }
        PropertyPath::CellInsetLeft => {
            let new = expect_length(path, value)?.unwrap_or(0.0);
            let prev = cell.text_left_inset;
            cell.text_left_inset = new;
            Value::Length(Some(prev))
        }
        PropertyPath::CellInsetBottom => {
            let new = expect_length(path, value)?.unwrap_or(0.0);
            let prev = cell.text_bottom_inset;
            cell.text_bottom_inset = new;
            Value::Length(Some(prev))
        }
        PropertyPath::CellInsetRight => {
            let new = expect_length(path, value)?.unwrap_or(0.0);
            let prev = cell.text_right_inset;
            cell.text_right_inset = new;
            Value::Length(Some(prev))
        }
        PropertyPath::CellVerticalJustification => {
            let new = expect_text(path, value)?;
            let prev = cell.vertical_justification.clone().unwrap_or_default();
            cell.vertical_justification = if new.is_empty() { None } else { Some(new) };
            Value::Text(prev)
        }
        PropertyPath::AppliedCellStyle => {
            let new = expect_text(path, value)?;
            let prev = cell.applied_cell_style.clone().unwrap_or_default();
            cell.applied_cell_style = if new.is_empty() { None } else { Some(new) };
            Value::Text(prev)
        }
        _ => {
            return Err(OperationError::UnsupportedProperty {
                node: node.clone(),
                path,
            });
        }
    };

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path,
            value: value.clone(),
        },
        inverse: Operation::SetProperty {
            node: node.clone(),
            path,
            value: inverse_value,
        },
        invalidation,
    })
}

/// W3.A1 — set a row's `SingleRowHeight`. Inverse carries the prior
/// height. `row` out of range → `InvalidValue`.
fn apply_set_row_height(
    doc: &mut Document,
    story_id: &str,
    table_id: &str,
    row: u32,
    height: Option<f32>,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::Table {
        story_id: story_id.to_string(),
        table_id: table_id.to_string(),
    };
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let row_count = table.rows.len();
    let tr = table
        .rows
        .get_mut(row as usize)
        .ok_or_else(|| OperationError::InvalidValue {
            node: node.clone(),
            path: PropertyPath::FrameBounds,
            reason: format!("row {row} out of range ({row_count} rows)"),
        })?;
    let prev = tr.single_row_height;
    tr.single_row_height = height;

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::SetRowHeight {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            row,
            height,
        },
        inverse: Operation::SetRowHeight {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            row,
            height: prev,
        },
        invalidation,
    })
}

/// W3.A1 — set a column's `SingleColumnWidth`. Inverse carries the
/// prior width.
fn apply_set_column_width(
    doc: &mut Document,
    story_id: &str,
    table_id: &str,
    col: u32,
    width: Option<f32>,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::Table {
        story_id: story_id.to_string(),
        table_id: table_id.to_string(),
    };
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let col_count = table.columns.len();
    let tc = table
        .columns
        .get_mut(col as usize)
        .ok_or_else(|| OperationError::InvalidValue {
            node: node.clone(),
            path: PropertyPath::FrameBounds,
            reason: format!("column {col} out of range ({col_count} cols)"),
        })?;
    let prev = tc.single_column_width;
    tc.single_column_width = width;

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::SetColumnWidth {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            col,
            width,
        },
        inverse: Operation::SetColumnWidth {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            col,
            width: prev,
        },
        invalidation,
    })
}

/// W3.A1 — recompute every cell's `Name` from its position after a row
/// or column shift. Cells are keyed by `Name="col:row"`; an insert /
/// delete renumbers the affected axis. We rewrite names in place so the
/// renderer's `coords()` lookups and our own re-addressing stay
/// consistent.
fn set_cell_name(cell: &mut paged_parse::TableCell, col: u32, row: u32) {
    cell.name = Some(format!("{col}:{row}"));
}

/// W3.A1 — insert a row at `at`. Cells in rows ≥ `at` shift down (+1);
/// a fresh empty cell per column is minted at the new row;
/// `body_row_count` / the `rows` vec grow. When `restore` is `Some`
/// (the `DeleteTableRow` inverse), the captured row + cells are
/// re-inserted verbatim instead of minting empties.
fn apply_insert_table_row(
    doc: &mut Document,
    story_id: &str,
    table_id: &str,
    at: u32,
    restore: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::Table {
        story_id: story_id.to_string(),
        table_id: table_id.to_string(),
    };
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let total_rows = table.rows.len() as u32;
    if at > total_rows {
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::FrameBounds,
            reason: format!("insert row at {at} out of range ({total_rows} rows)"),
        });
    }
    let col_count = table.columns.len().max(table.column_count as usize);

    // Shift existing cells in rows ≥ `at` down by one (rewrite names).
    for cell in &mut table.cells {
        if let Some((c, r)) = cell.coords() {
            if r >= at {
                set_cell_name(cell, c, r + 1);
            }
        }
    }

    // Build the new row + its cells.
    let (new_row, new_cells): (paged_parse::TableRow, Vec<paged_parse::TableCell>) = match restore {
        Some(blob) => {
            let removed = parse_restore_blob(blob, story_id, table_id)?;
            let mut cells: Vec<paged_parse::TableCell> =
                removed.cells.iter().map(TableCellSpec::to_parse).collect();
            // Re-key the restored cells to the insertion row.
            for cell in &mut cells {
                if let Some((c, _)) = cell.coords() {
                    set_cell_name(cell, c, at);
                }
            }
            let row = removed
                .row
                .as_ref()
                .map(TableRowSpec::to_parse)
                .unwrap_or_default();
            (row, cells)
        }
        None => {
            let row = paged_parse::TableRow {
                name: Some(at.to_string()),
                ..Default::default()
            };
            let cells: Vec<paged_parse::TableCell> = (0..col_count as u32)
                .map(|c| paged_parse::TableCell {
                    name: Some(format!("{c}:{at}")),
                    row_span: 1,
                    column_span: 1,
                    ..Default::default()
                })
                .collect();
            (row, cells)
        }
    };

    table.rows.insert(at as usize, new_row);
    table.cells.extend(new_cells);
    table.body_row_count = table.body_row_count.saturating_add(1);
    renumber_table_rows(table);

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::InsertTableRow {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            at,
            restore: restore.map(str::to_string),
        },
        inverse: Operation::DeleteTableRow {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            at,
        },
        invalidation,
    })
}

/// W3.A1 — delete the row at `at`. Captures the row + originating cells
/// for the inverse. Cells in rows > `at` shift up. Rejected when the
/// table has only one row or `at` is out of range.
fn apply_delete_table_row(
    doc: &mut Document,
    story_id: &str,
    table_id: &str,
    at: u32,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::Table {
        story_id: story_id.to_string(),
        table_id: table_id.to_string(),
    };
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let total_rows = table.rows.len() as u32;
    if at >= total_rows {
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::FrameBounds,
            reason: format!("delete row at {at} out of range ({total_rows} rows)"),
        });
    }
    if total_rows <= 1 {
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::FrameBounds,
            reason: "cannot delete the last row of a table".to_string(),
        });
    }

    // Capture the row declaration + every cell originating on it.
    let removed_row = table.rows.remove(at as usize);
    let mut captured_cells: Vec<TableCellSpec> = Vec::new();
    table.cells.retain(|cell| match cell.coords() {
        Some((_, r)) if r == at => {
            captured_cells.push(TableCellSpec::from_parse(cell));
            false
        }
        _ => true,
    });
    // Shift cells in rows > `at` up by one.
    for cell in &mut table.cells {
        if let Some((c, r)) = cell.coords() {
            if r > at {
                set_cell_name(cell, c, r - 1);
            }
        }
    }
    table.body_row_count = table.body_row_count.saturating_sub(1);
    renumber_table_rows(table);

    let restore_blob = serde_json::to_string(&RemovedTableLine {
        row: Some(TableRowSpec::from_parse(&removed_row)),
        column: None,
        cells: captured_cells,
    })
    .expect("RemovedTableLine serialises");

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::DeleteTableRow {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            at,
        },
        inverse: Operation::InsertTableRow {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            at,
            restore: Some(restore_blob),
        },
        invalidation,
    })
}

/// W3.A1 — insert a column at `at`. Cells in columns ≥ `at` shift
/// right; a fresh empty cell per row is minted. `restore` re-inserts
/// captured column content (the `DeleteTableColumn` inverse).
fn apply_insert_table_column(
    doc: &mut Document,
    story_id: &str,
    table_id: &str,
    at: u32,
    restore: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::Table {
        story_id: story_id.to_string(),
        table_id: table_id.to_string(),
    };
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let total_cols = table.columns.len() as u32;
    if at > total_cols {
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::FrameBounds,
            reason: format!("insert column at {at} out of range ({total_cols} cols)"),
        });
    }
    let row_count = table
        .rows
        .len()
        .max((table.header_row_count + table.body_row_count + table.footer_row_count) as usize);

    // Shift cells in columns ≥ `at` right by one.
    for cell in &mut table.cells {
        if let Some((c, r)) = cell.coords() {
            if c >= at {
                set_cell_name(cell, c + 1, r);
            }
        }
    }

    let (new_col, new_cells): (paged_parse::TableColumn, Vec<paged_parse::TableCell>) =
        match restore {
            Some(blob) => {
                let removed = parse_restore_blob(blob, story_id, table_id)?;
                let mut cells: Vec<paged_parse::TableCell> =
                    removed.cells.iter().map(TableCellSpec::to_parse).collect();
                for cell in &mut cells {
                    if let Some((_, r)) = cell.coords() {
                        set_cell_name(cell, at, r);
                    }
                }
                let col = removed
                    .column
                    .as_ref()
                    .map(TableColumnSpec::to_parse)
                    .unwrap_or_default();
                (col, cells)
            }
            None => {
                let col = paged_parse::TableColumn {
                    name: Some(at.to_string()),
                    ..Default::default()
                };
                let cells: Vec<paged_parse::TableCell> = (0..row_count as u32)
                    .map(|r| paged_parse::TableCell {
                        name: Some(format!("{at}:{r}")),
                        row_span: 1,
                        column_span: 1,
                        ..Default::default()
                    })
                    .collect();
                (col, cells)
            }
        };

    table.columns.insert(at as usize, new_col);
    table.cells.extend(new_cells);
    table.column_count = table.column_count.saturating_add(1);
    renumber_table_columns(table);

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::InsertTableColumn {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            at,
            restore: restore.map(str::to_string),
        },
        inverse: Operation::DeleteTableColumn {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            at,
        },
        invalidation,
    })
}

/// W3.A1 — delete the column at `at`. Captures the column + cells for
/// the inverse. Cells in columns > `at` shift left.
fn apply_delete_table_column(
    doc: &mut Document,
    story_id: &str,
    table_id: &str,
    at: u32,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::Table {
        story_id: story_id.to_string(),
        table_id: table_id.to_string(),
    };
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let total_cols = table.columns.len() as u32;
    if at >= total_cols {
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::FrameBounds,
            reason: format!("delete column at {at} out of range ({total_cols} cols)"),
        });
    }
    if total_cols <= 1 {
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::FrameBounds,
            reason: "cannot delete the last column of a table".to_string(),
        });
    }

    let removed_col = table.columns.remove(at as usize);
    let mut captured_cells: Vec<TableCellSpec> = Vec::new();
    table.cells.retain(|cell| match cell.coords() {
        Some((c, _)) if c == at => {
            captured_cells.push(TableCellSpec::from_parse(cell));
            false
        }
        _ => true,
    });
    for cell in &mut table.cells {
        if let Some((c, r)) = cell.coords() {
            if c > at {
                set_cell_name(cell, c - 1, r);
            }
        }
    }
    table.column_count = table.column_count.saturating_sub(1);
    renumber_table_columns(table);

    let restore_blob = serde_json::to_string(&RemovedTableLine {
        row: None,
        column: Some(TableColumnSpec::from_parse(&removed_col)),
        cells: captured_cells,
    })
    .expect("RemovedTableLine serialises");

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::DeleteTableColumn {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            at,
        },
        inverse: Operation::InsertTableColumn {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            at,
            restore: Some(restore_blob),
        },
        invalidation,
    })
}

/// W3.A1 — decode a `DeleteTable{Row,Column}` restore blob. On a bad
/// blob, raises `InvalidValue` rather than panicking (the blob crosses
/// the wasm boundary on redo of a deserialised op log).
fn parse_restore_blob(
    blob: &str,
    story_id: &str,
    table_id: &str,
) -> Result<RemovedTableLine, OperationError> {
    serde_json::from_str(blob).map_err(|e| OperationError::InvalidValue {
        node: NodeId::Table {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
        },
        path: PropertyPath::FrameBounds,
        reason: format!("bad table restore blob: {e}"),
    })
}

/// W3.A1 — renumber `<Row>` `Name` attributes to their positional
/// index after an insert / delete (IDML expects `Name="0".."n-1"`).
fn renumber_table_rows(table: &mut paged_parse::Table) {
    for (i, row) in table.rows.iter_mut().enumerate() {
        row.name = Some(i.to_string());
    }
}

/// W3.A1 — renumber `<Column>` `Name` attributes after an insert /
/// delete.
fn renumber_table_columns(table: &mut paged_parse::Table) {
    for (i, col) in table.columns.iter_mut().enumerate() {
        col.name = Some(i.to_string());
    }
}

/// True when `frame_id` carries no story content of its own — either it
/// has no `ParentStory`, or its parent story has no non-empty runs. Used
/// by `LinkFrames` to honour InDesign's "thread into empty frames only"
/// rule.
fn frame_has_no_own_content(doc: &Document, frame_id: &str) -> bool {
    let Some((si, fi)) = find_text_frame_pos(doc, frame_id) else {
        return false;
    };
    let frame = &doc.spreads[si].spread.text_frames[fi];
    let Some(story_id) = frame.parent_story.as_deref() else {
        return true;
    };
    // A frame is "empty" if its story has no characters. A shared story
    // (the frame is a continuation of an existing chain) still counts as
    // content for the purpose of refusing the link.
    match doc.stories.iter().find(|s| s.self_id == story_id) {
        Some(parsed) => parsed
            .story
            .paragraphs
            .iter()
            .all(|p| p.runs.iter().all(|r| r.text.is_empty())),
        // ParentStory points at a story we can't see → treat as content
        // present (conservative: refuse the link).
        None => false,
    }
}

/// Walk the existing `NextTextFrame` chain forward from `start` and
/// report whether `target` is reachable (i.e. linking `start → target`
/// would close a cycle). Bounded so a pre-existing malformed cycle in
/// the document can't loop forever.
fn chain_reaches(doc: &Document, start: &str, target: &str) -> bool {
    let mut cursor = Some(start.to_string());
    let mut guard = 0usize;
    while let Some(cur) = cursor {
        if guard > 4096 {
            return true; // pathological; treat as a cycle to be safe
        }
        guard += 1;
        if cur == target {
            return true;
        }
        let Some((si, fi)) = find_text_frame_pos(doc, &cur) else {
            return false;
        };
        cursor = doc.spreads[si].spread.text_frames[fi]
            .next_text_frame
            .clone();
    }
    false
}

fn apply_link_frames(
    doc: &mut Document,
    from: &str,
    to: &str,
) -> Result<AppliedOperation, OperationError> {
    if from == to {
        return Err(OperationError::InvalidValue {
            node: NodeId::TextFrame(from.to_string()),
            path: PropertyPath::FrameBounds,
            reason: "cannot thread a frame to itself".to_string(),
        });
    }
    let (from_si, from_fi) = find_text_frame_pos(doc, from)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::TextFrame(from.to_string())))?;
    if find_text_frame_pos(doc, to).is_none() {
        return Err(OperationError::NodeNotFound(NodeId::TextFrame(
            to.to_string(),
        )));
    }
    // InDesign threads into empty frames only.
    if !frame_has_no_own_content(doc, to) {
        return Err(OperationError::InvalidValue {
            node: NodeId::TextFrame(to.to_string()),
            path: PropertyPath::FrameBounds,
            reason: "target frame already owns story content; threading into \
                     a non-empty frame is not allowed"
                .to_string(),
        });
    }
    // Reject cycles: if `to` can already reach `from` along the chain,
    // linking `from → to` closes a loop.
    if chain_reaches(doc, to, from) {
        return Err(OperationError::InvalidValue {
            node: NodeId::TextFrame(from.to_string()),
            path: PropertyPath::FrameBounds,
            reason: "linking these frames would create a cycle".to_string(),
        });
    }

    let prev_next = doc.spreads[from_si].spread.text_frames[from_fi]
        .next_text_frame
        .clone();
    doc.spreads[from_si].spread.text_frames[from_fi].next_text_frame = Some(to.to_string());

    // The story flowing through `from` reflows across the new link.
    let story_id = doc.spreads[from_si].spread.text_frames[from_fi]
        .parent_story
        .clone();
    let invalidation = match story_id {
        Some(sid) => reflow_hint_for_story(doc, &sid),
        None => InvalidationHint {
            structural: true,
            ..Default::default()
        },
    };

    Ok(AppliedOperation {
        op: Operation::LinkFrames {
            from: from.to_string(),
            to: to.to_string(),
        },
        // Undo restores `from`'s prior next-target (None clears it; a
        // prior link re-points it).
        inverse: Operation::UnlinkFrames {
            frame: from.to_string(),
            prev_next,
        },
        invalidation,
    })
}

fn apply_unlink_frames(
    doc: &mut Document,
    frame: &str,
    prev_next: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let (si, fi) = find_text_frame_pos(doc, frame)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::TextFrame(frame.to_string())))?;
    let captured = doc.spreads[si].spread.text_frames[fi]
        .next_text_frame
        .clone();
    // Forward unlink clears; the inverse-only `prev_next` restores.
    doc.spreads[si].spread.text_frames[fi].next_text_frame = prev_next.map(str::to_string);

    let story_id = doc.spreads[si].spread.text_frames[fi].parent_story.clone();
    let invalidation = match story_id {
        Some(sid) => reflow_hint_for_story(doc, &sid),
        None => InvalidationHint {
            structural: true,
            ..Default::default()
        },
    };

    // Inverse re-links to the captured prior target (if any). When the
    // frame was already end-of-chain (captured None), the inverse is a
    // no-op UnlinkFrames so undo stays balanced.
    let inverse = match captured {
        Some(to) => Operation::LinkFrames {
            from: frame.to_string(),
            to,
        },
        None => Operation::UnlinkFrames {
            frame: frame.to_string(),
            prev_next: None,
        },
    };

    Ok(AppliedOperation {
        op: Operation::UnlinkFrames {
            frame: frame.to_string(),
            prev_next: prev_next.map(str::to_string),
        },
        inverse,
        invalidation,
    })
}

fn apply_apply_style(
    doc: &mut Document,
    story_id: &str,
    start: u32,
    end: u32,
    style: &str,
    scope: StyleScope,
) -> Result<AppliedOperation, OperationError> {
    // Delegate to the existing run/paragraph splitter via the
    // AppliedCharacterStyle / AppliedParagraphStyle property paths. The
    // splitter captures a per-segment inverse Batch. We then rewrap the
    // returned AppliedOperation so the *forward* op records as
    // `ApplyStyle` (the inverse stays the splitter's SetProperty Batch,
    // which `apply` can replay directly).
    let node = NodeId::StoryRange {
        story_id: story_id.to_string(),
        start,
        end,
    };
    let value = Value::Text(style.to_string());
    let applied = match scope {
        StyleScope::Character => apply_character_property(
            doc,
            story_id,
            start,
            end,
            &node,
            PropertyPath::AppliedCharacterStyle,
            &value,
        )?,
        StyleScope::Paragraph => apply_paragraph_property(
            doc,
            story_id,
            start,
            end,
            &node,
            PropertyPath::AppliedParagraphStyle,
            &value,
        )?,
    };
    Ok(AppliedOperation {
        op: Operation::ApplyStyle {
            story_id: story_id.to_string(),
            start,
            end,
            style: style.to_string(),
            scope,
        },
        inverse: applied.inverse,
        invalidation: applied.invalidation,
    })
}

fn apply_insert_field(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    field: FieldKind,
) -> Result<AppliedOperation, OperationError> {
    let story_idx = doc
        .stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| {
            OperationError::NodeNotFound(NodeId::StoryRange {
                story_id: story_id.to_string(),
                start: offset,
                end: offset,
            })
        })?;
    let marker = field.marker_char();
    let story = &mut doc.stories[story_idx].story;

    // Walk to the offset and insert the marker char into the run that
    // contains it (splitting nothing — a one-char insert just grows the
    // run's text). The page-number marker inherits the surrounding
    // run's formatting, which matches InDesign (the field takes the
    // character style at the insertion point).
    let total: u32 = story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.chars().count() as u32)
        .sum();
    if offset > total {
        return Err(OperationError::InvalidValue {
            node: NodeId::StoryRange {
                story_id: story_id.to_string(),
                start: offset,
                end: offset,
            },
            path: PropertyPath::AppliedCharacterStyle,
            reason: format!("offset {offset} past end of story (len {total})"),
        });
    }

    let mut char_cursor: u32 = 0;
    let mut inserted = false;
    'outer: for para in story.paragraphs.iter_mut() {
        for run in para.runs.iter_mut() {
            let run_len = run.text.chars().count() as u32;
            // Insert when the offset falls within this run (inclusive of
            // its trailing boundary, so an offset at end-of-run lands
            // before the next run / at the run's tail).
            if offset <= char_cursor + run_len {
                let local = (offset - char_cursor) as usize;
                let byte = run
                    .text
                    .char_indices()
                    .nth(local)
                    .map(|(b, _)| b)
                    .unwrap_or(run.text.len());
                run.text.insert(byte, marker);
                inserted = true;
                break 'outer;
            }
            char_cursor += run_len;
        }
    }
    // Empty story (no runs at all): append a fresh run holding the
    // marker to the first (or a new) paragraph.
    if !inserted {
        if story.paragraphs.is_empty() {
            story.paragraphs.push(paged_parse::Paragraph::default());
        }
        let para = story.paragraphs.last_mut().expect("ensured above");
        if let Some(run) = para.runs.last_mut() {
            run.text.push(marker);
        } else {
            let mut run = paged_parse::CharacterRun::default();
            run.text.push(marker);
            para.runs.push(run);
        }
    }

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::InsertField {
            story_id: story_id.to_string(),
            offset,
            field,
        },
        // Undo removes the one marker char we inserted at `offset`.
        inverse: Operation::DeleteField {
            story_id: story_id.to_string(),
            offset,
            field,
        },
        invalidation,
    })
}

fn apply_delete_field(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    field: FieldKind,
) -> Result<AppliedOperation, OperationError> {
    let story_idx = doc
        .stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| {
            OperationError::NodeNotFound(NodeId::StoryRange {
                story_id: story_id.to_string(),
                start: offset,
                end: offset,
            })
        })?;
    let marker = field.marker_char();
    let story = &mut doc.stories[story_idx].story;

    let mut char_cursor: u32 = 0;
    let mut removed = false;
    'outer: for para in story.paragraphs.iter_mut() {
        for run in para.runs.iter_mut() {
            let run_len = run.text.chars().count() as u32;
            // The char to delete sits at `offset`; it must land within
            // this run (offset in [char_cursor, char_cursor+run_len)).
            if offset < char_cursor + run_len {
                let local = (offset - char_cursor) as usize;
                if let Some((byte, ch)) = run.text.char_indices().nth(local) {
                    if ch != marker {
                        return Err(OperationError::InvalidValue {
                            node: NodeId::StoryRange {
                                story_id: story_id.to_string(),
                                start: offset,
                                end: offset + 1,
                            },
                            path: PropertyPath::AppliedCharacterStyle,
                            reason: format!(
                                "expected field marker at offset {offset}, found {ch:?}"
                            ),
                        });
                    }
                    run.text.remove(byte);
                    removed = true;
                }
                break 'outer;
            }
            char_cursor += run_len;
        }
    }
    if !removed {
        return Err(OperationError::InvalidValue {
            node: NodeId::StoryRange {
                story_id: story_id.to_string(),
                start: offset,
                end: offset + 1,
            },
            path: PropertyPath::AppliedCharacterStyle,
            reason: format!("no field marker at offset {offset}"),
        });
    }

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::DeleteField {
            story_id: story_id.to_string(),
            offset,
            field,
        },
        inverse: Operation::InsertField {
            story_id: story_id.to_string(),
            offset,
            field,
        },
        invalidation,
    })
}

// ---------------------------------------------------------------------------
// W0.5 — guide CRUD
// ---------------------------------------------------------------------------

/// Guides carry no id in the parse struct, so we address them
/// positionally as `Guide/<spread_self_id>/<index>`. Resolve such an id
/// to its `(spread_index, guide_index)`.
fn resolve_guide(doc: &Document, guide_id: &str) -> Option<(usize, usize)> {
    let rest = guide_id.strip_prefix("Guide/")?;
    let (spread_self, idx_str) = rest.rsplit_once('/')?;
    let gi: usize = idx_str.parse().ok()?;
    let si = doc
        .spreads
        .iter()
        .position(|p| p.spread.self_id.as_deref() == Some(spread_self))?;
    if gi < doc.spreads[si].spread.guides.len() {
        Some((si, gi))
    } else {
        None
    }
}

/// The positional id for the guide at `(spread_index, guide_index)`.
fn guide_id_for(doc: &Document, si: usize, gi: usize) -> String {
    let spread_self = doc.spreads[si]
        .spread
        .self_id
        .as_deref()
        .unwrap_or_default();
    format!("Guide/{spread_self}/{gi}")
}

fn apply_insert_guide(
    doc: &mut Document,
    spread_id: &str,
    orientation: GuideOrientationSpec,
    position: f32,
    page_index: u32,
    _guide_id: Option<String>,
) -> Result<AppliedOperation, OperationError> {
    let si = doc
        .spreads
        .iter()
        .position(|p| p.spread.self_id.as_deref() == Some(spread_id))
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Spread(spread_id.to_string())))?;
    let guide = paged_parse::RulerGuide {
        orientation: orientation.to_parse(),
        location: position,
        page_index,
    };
    doc.spreads[si].spread.guides.push(guide);
    let gi = doc.spreads[si].spread.guides.len() - 1;
    let id = guide_id_for(doc, si, gi);
    Ok(AppliedOperation {
        op: Operation::InsertGuide {
            spread_id: spread_id.to_string(),
            orientation,
            position,
            page_index,
            guide_id: Some(id.clone()),
        },
        inverse: Operation::DeleteGuide { guide_id: id },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_move_guide(
    doc: &mut Document,
    guide_id: &str,
    position: f32,
) -> Result<AppliedOperation, OperationError> {
    let (si, gi) =
        resolve_guide(doc, guide_id).ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "guides".to_string(),
            id: guide_id.to_string(),
        })?;
    let prev = doc.spreads[si].spread.guides[gi].location;
    doc.spreads[si].spread.guides[gi].location = position;
    Ok(AppliedOperation {
        op: Operation::MoveGuide {
            guide_id: guide_id.to_string(),
            position,
        },
        inverse: Operation::MoveGuide {
            guide_id: guide_id.to_string(),
            position: prev,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_delete_guide(
    doc: &mut Document,
    guide_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let (si, gi) =
        resolve_guide(doc, guide_id).ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "guides".to_string(),
            id: guide_id.to_string(),
        })?;
    let removed = doc.spreads[si].spread.guides.remove(gi);
    let spread_id = doc.spreads[si].spread.self_id.clone().unwrap_or_default();
    Ok(AppliedOperation {
        op: Operation::DeleteGuide {
            guide_id: guide_id.to_string(),
        },
        // Undo re-inserts at the tail; positional ids past this index
        // are stable because deletion shifts only higher indices and we
        // re-append. (v1: a delete-then-undo restores geometry, not the
        // exact mid-vec slot — acceptable since guides are unordered.)
        inverse: Operation::InsertGuide {
            spread_id,
            orientation: GuideOrientationSpec::from_parse(removed.orientation),
            position: removed.location,
            page_index: removed.page_index,
            guide_id: None,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

// ---------------------------------------------------------------------------
// W0.5 — conditions
// ---------------------------------------------------------------------------

fn apply_set_condition_visible(
    doc: &mut Document,
    condition: &str,
    visible: bool,
) -> Result<AppliedOperation, OperationError> {
    let cond = doc.styles.conditions.get_mut(condition).ok_or_else(|| {
        OperationError::CollectionEntryNotFound {
            collection: "conditions".to_string(),
            id: condition.to_string(),
        }
    })?;
    // `None` ⇒ visible (IDML default), so capture the resolved prior.
    let prev = cond.visible.unwrap_or(true);
    cond.visible = Some(visible);
    Ok(AppliedOperation {
        op: Operation::SetConditionVisible {
            condition: condition.to_string(),
            visible,
        },
        inverse: Operation::SetConditionVisible {
            condition: condition.to_string(),
            visible: prev,
        },
        // Conditional text changes which runs render → reflow the
        // whole document (advisory: structural).
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_activate_condition_set(
    doc: &mut Document,
    set: &str,
) -> Result<AppliedOperation, OperationError> {
    let members: Vec<String> = doc
        .styles
        .condition_sets
        .get(set)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "conditionSets".to_string(),
            id: set.to_string(),
        })?
        .conditions
        .clone();
    // Capture every condition's prior visibility so the inverse is a
    // single RestoreConditionVisibility.
    let mut states: Vec<(String, bool)> = Vec::with_capacity(doc.styles.conditions.len());
    for (id, def) in doc.styles.conditions.iter() {
        states.push((id.clone(), def.visible.unwrap_or(true)));
    }
    // Activate: members visible, everyone else hidden.
    let member_set: std::collections::HashSet<&str> = members.iter().map(String::as_str).collect();
    for (id, def) in doc.styles.conditions.iter_mut() {
        def.visible = Some(member_set.contains(id.as_str()));
    }
    Ok(AppliedOperation {
        op: Operation::ActivateConditionSet {
            set: set.to_string(),
        },
        inverse: Operation::RestoreConditionVisibility { states },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_restore_condition_visibility(
    doc: &mut Document,
    states: &[(String, bool)],
) -> Result<AppliedOperation, OperationError> {
    // Capture the current state for THIS op's own inverse so a
    // restore is itself undoable (redo of ActivateConditionSet).
    let mut prior: Vec<(String, bool)> = Vec::with_capacity(states.len());
    for (id, vis) in states {
        if let Some(def) = doc.styles.conditions.get_mut(id) {
            prior.push((id.clone(), def.visible.unwrap_or(true)));
            def.visible = Some(*vis);
        }
    }
    Ok(AppliedOperation {
        op: Operation::RestoreConditionVisibility {
            states: states.to_vec(),
        },
        inverse: Operation::RestoreConditionVisibility { states: prior },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

// ---------------------------------------------------------------------------
// W0.5 — master application
// ---------------------------------------------------------------------------

fn apply_master_to_page(
    doc: &mut Document,
    page: &str,
    master: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let page_ref = find_page_mut(doc, page)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Page(page.to_string())))?;
    let prev = page_ref.applied_master.clone();
    page_ref.applied_master = master.map(str::to_string);
    Ok(AppliedOperation {
        op: Operation::ApplyMasterToPage {
            page: page.to_string(),
            master: master.map(str::to_string),
        },
        inverse: Operation::ApplyMasterToPage {
            page: page.to_string(),
            master: prev,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

// ---------------------------------------------------------------------------
// W0.5 — duplicate page
// ---------------------------------------------------------------------------

fn apply_duplicate_page(
    doc: &mut Document,
    page: &str,
    clone_spread_json: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    // Redo path — re-materialise the captured clone verbatim.
    if let Some(json) = clone_spread_json {
        let restore: SpreadRestore =
            serde_json::from_str(json).map_err(|e| OperationError::InvalidValue {
                node: NodeId::Page(page.to_string()),
                path: PropertyPath::PageBounds,
                reason: format!("malformed duplicate-page payload: {e}"),
            })?;
        let cloned_page_id = restore
            .spread
            .pages
            .first()
            .and_then(|p| p.self_id.clone())
            .unwrap_or_default();
        let index = restore.index.min(doc.spreads.len());
        doc.spreads.insert(
            index,
            paged_scene::ParsedSpread {
                src: restore.src,
                spread: restore.spread,
            },
        );
        return Ok(AppliedOperation {
            op: Operation::DuplicatePage {
                page: page.to_string(),
                clone_spread_json: Some(json.to_string()),
            },
            inverse: Operation::RemovePage {
                page_id: cloned_page_id,
            },
            invalidation: InvalidationHint {
                structural: true,
                ..Default::default()
            },
        });
    }

    let src_idx = doc
        .spreads
        .iter()
        .position(|p| {
            p.spread
                .pages
                .iter()
                .any(|pg| pg.self_id.as_deref() == Some(page))
        })
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Page(page.to_string())))?;
    if doc.spreads[src_idx].spread.pages.len() != 1 {
        return Err(OperationError::InvalidValue {
            node: NodeId::Page(page.to_string()),
            path: PropertyPath::PageBounds,
            reason: "duplicating a page out of a multi-page spread is not supported in v1"
                .to_string(),
        });
    }

    // Deep-clone the source spread, then remap every Self id to a fresh
    // one (spread, page, and all page items) so the clone is a distinct
    // document object. Reuse the id-minting convention used by
    // InsertPage / id scans (`u<hex>`).
    let mut clone = doc.spreads[src_idx].clone();
    let mut next = next_id_seed(doc);
    let remap = |slot: &mut Option<String>, next: &mut u64| {
        *slot = Some(format!("u{:x}", *next));
        *next += 1;
    };
    remap(&mut clone.spread.self_id, &mut next);
    for pg in &mut clone.spread.pages {
        remap(&mut pg.self_id, &mut next);
    }
    for f in &mut clone.spread.text_frames {
        remap(&mut f.self_id, &mut next);
    }
    for r in &mut clone.spread.rectangles {
        remap(&mut r.self_id, &mut next);
    }
    for o in &mut clone.spread.ovals {
        remap(&mut o.self_id, &mut next);
    }
    for l in &mut clone.spread.graphic_lines {
        remap(&mut l.self_id, &mut next);
    }
    for p in &mut clone.spread.polygons {
        remap(&mut p.self_id, &mut next);
    }
    for g in &mut clone.spread.groups {
        remap(&mut g.self_id, &mut next);
    }

    // Stack the clone below everything on the pasteboard (same rule as
    // InsertPage) so spread AABBs never overlap.
    let mut max_bottom: f32 = 0.0;
    for parsed in &doc.spreads {
        let sty = parsed.spread.item_transform.map(|m| m[5]).unwrap_or(0.0);
        for p in &parsed.spread.pages {
            let pty = p.item_transform.map(|m| m[5]).unwrap_or(0.0);
            max_bottom = max_bottom.max(sty + pty + p.bounds.bottom);
        }
    }
    clone.spread.item_transform = Some([1.0, 0.0, 0.0, 1.0, 0.0, max_bottom + SPREAD_STACK_GAP_PT]);
    let cloned_spread_self = clone.spread.self_id.clone().unwrap_or_default();
    clone.src = format!("Spreads/Spread_{cloned_spread_self}.xml");
    let cloned_page_id = clone
        .spread
        .pages
        .first()
        .and_then(|p| p.self_id.clone())
        .unwrap_or_default();

    let insert_index = src_idx + 1;
    // Capture the materialised clone so redo re-creates the exact ids.
    let restore = SpreadRestore {
        index: insert_index,
        src: clone.src.clone(),
        spread: clone.spread.clone(),
    };
    let json = serde_json::to_string(&restore).map_err(|e| OperationError::InvalidValue {
        node: NodeId::Page(page.to_string()),
        path: PropertyPath::PageBounds,
        reason: format!("duplicate-page capture failed: {e}"),
    })?;

    doc.spreads.insert(insert_index, clone);

    Ok(AppliedOperation {
        op: Operation::DuplicatePage {
            page: page.to_string(),
            clone_spread_json: Some(json),
        },
        inverse: Operation::RemovePage {
            page_id: cloned_page_id,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// Highest `u<hex>` id seen across the document + 1, as a raw counter
/// for minting a run of fresh ids (DuplicatePage needs many at once).
fn next_id_seed(doc: &Document) -> u64 {
    let mut max: u64 = 0;
    let mut scan = |id: Option<&str>| {
        let Some(id) = id else { return };
        let Some(hex) = id.strip_prefix('u') else {
            return;
        };
        if hex.is_empty() || hex.len() > 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return;
        }
        if let Ok(v) = u64::from_str_radix(hex, 16) {
            max = max.max(v);
        }
    };
    for parsed in &doc.spreads {
        let s = &parsed.spread;
        scan(s.self_id.as_deref());
        for p in &s.pages {
            scan(p.self_id.as_deref());
        }
        for f in &s.text_frames {
            scan(f.self_id.as_deref());
        }
        for r in &s.rectangles {
            scan(r.self_id.as_deref());
        }
        for o in &s.ovals {
            scan(o.self_id.as_deref());
        }
        for l in &s.graphic_lines {
            scan(l.self_id.as_deref());
        }
        for p in &s.polygons {
            scan(p.self_id.as_deref());
        }
        for g in &s.groups {
            scan(g.self_id.as_deref());
        }
    }
    max + 1
}

// ---------------------------------------------------------------------------
// W0.5 — sections
// ---------------------------------------------------------------------------

/// Map a parsed `NumberingStyle` back to its IDML `PageNumberStyle`
/// attribute spelling so an inverse op round-trips through
/// `NumberingStyle::from_idml`. (`NumberingStyle::as_str` yields the
/// editor's lower-camel wire name, which `from_idml` does NOT accept.)
fn numbering_style_to_idml(s: paged_parse::NumberingStyle) -> &'static str {
    use paged_parse::NumberingStyle::*;
    match s {
        Arabic => "Arabic",
        UpperRoman => "UpperRoman",
        LowerRoman => "LowerRoman",
        UpperAlpha => "UpperLetters",
        LowerAlpha => "LowerLetters",
    }
}

fn apply_insert_section(
    doc: &mut Document,
    at_page: &str,
    prefix: Option<String>,
    numbering_style: Option<String>,
    start_at: Option<u32>,
    self_id: Option<String>,
) -> Result<AppliedOperation, OperationError> {
    // The anchor page must exist.
    if find_page_mut(doc, at_page).is_none() {
        return Err(OperationError::NodeNotFound(NodeId::Page(
            at_page.to_string(),
        )));
    }
    let sections = &mut doc.container.designmap.sections;
    let id = match self_id {
        Some(id) => id,
        None => {
            // Deterministic non-colliding `Section/u<n>`.
            let mut n = sections.len();
            let mut id = format!("Section/u{n}");
            while sections.iter().any(|s| s.self_id == id) {
                n += 1;
                id = format!("Section/u{n}");
            }
            id
        }
    };
    if sections.iter().any(|s| s.self_id == id) {
        return Err(OperationError::DuplicateNodeId { id });
    }
    let section = paged_parse::Section {
        self_id: id.clone(),
        page_start: Some(at_page.to_string()),
        continue_numbering: false,
        start_at,
        numbering_style: numbering_style
            .as_deref()
            .map(paged_parse::NumberingStyle::from_idml)
            .unwrap_or(paged_parse::NumberingStyle::Arabic),
        section_prefix: prefix.clone(),
        marker: None,
        include_prefix: prefix.is_some(),
    };
    sections.push(section);
    Ok(AppliedOperation {
        op: Operation::InsertSection {
            at_page: at_page.to_string(),
            prefix,
            numbering_style,
            start_at,
            self_id: Some(id.clone()),
        },
        inverse: Operation::DeleteSection { section_id: id },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_edit_section(
    doc: &mut Document,
    section_id: &str,
    prefix: Option<Option<String>>,
    numbering_style: Option<String>,
    start_at: Option<Option<u32>>,
) -> Result<AppliedOperation, OperationError> {
    let sections = &mut doc.container.designmap.sections;
    let section = sections
        .iter_mut()
        .find(|s| s.self_id == section_id)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "sections".to_string(),
            id: section_id.to_string(),
        })?;
    // Capture prior values for the inverse.
    let prev_prefix = section.section_prefix.clone();
    let prev_style = numbering_style_to_idml(section.numbering_style).to_string();
    let prev_start = section.start_at;

    if let Some(p) = &prefix {
        section.section_prefix = p.clone();
        section.include_prefix = p.is_some();
    }
    if let Some(style) = &numbering_style {
        section.numbering_style = paged_parse::NumberingStyle::from_idml(style);
    }
    if let Some(s) = &start_at {
        section.start_at = *s;
    }

    Ok(AppliedOperation {
        op: Operation::EditSection {
            section_id: section_id.to_string(),
            prefix,
            numbering_style,
            start_at,
        },
        inverse: Operation::EditSection {
            section_id: section_id.to_string(),
            prefix: Some(prev_prefix),
            numbering_style: Some(prev_style),
            start_at: Some(prev_start),
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

fn apply_delete_section(
    doc: &mut Document,
    section_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let sections = &mut doc.container.designmap.sections;
    let pos = sections
        .iter()
        .position(|s| s.self_id == section_id)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "sections".to_string(),
            id: section_id.to_string(),
        })?;
    let removed = sections.remove(pos);
    let style_str = numbering_style_to_idml(removed.numbering_style).to_string();
    let at_page = removed.page_start.clone().unwrap_or_default();
    Ok(AppliedOperation {
        op: Operation::DeleteSection {
            section_id: section_id.to_string(),
        },
        inverse: Operation::InsertSection {
            at_page,
            prefix: removed.section_prefix.clone(),
            numbering_style: Some(style_str),
            start_at: removed.start_at,
            self_id: Some(removed.self_id.clone()),
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_with(text: &str) -> paged_parse::CharacterRun {
        paged_parse::CharacterRun {
            text: text.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn split_run_at_partitions_by_char_index() {
        let (l, r) = split_run_at(run_with("hello"), 2);
        assert_eq!(l.text, "he");
        assert_eq!(r.text, "llo");
    }

    #[test]
    fn split_run_at_zero_keeps_all_on_the_right() {
        let (l, r) = split_run_at(run_with("hello"), 0);
        assert_eq!(l.text, "");
        assert_eq!(r.text, "hello");
    }

    #[test]
    fn split_run_at_or_past_end_keeps_all_on_the_left() {
        let (l, r) = split_run_at(run_with("hi"), 2);
        assert_eq!(l.text, "hi");
        assert_eq!(r.text, "");
        // char_idx beyond the char count clamps to the byte length.
        let (l2, r2) = split_run_at(run_with("hi"), 99);
        assert_eq!(l2.text, "hi");
        assert_eq!(r2.text, "");
    }

    #[test]
    fn split_run_at_respects_multibyte_char_boundaries() {
        // "é" + "🚀" are 2- and 4-byte chars; splitting at char 1 must
        // land on a valid UTF-8 boundary, not mid-codepoint.
        let (l, r) = split_run_at(run_with("é🚀x"), 1);
        assert_eq!(l.text, "é");
        assert_eq!(r.text, "🚀x");
    }
}
