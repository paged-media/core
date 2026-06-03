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

use paged_parse::{Bounds, FrameRef, GraphicLine, Polygon, Rectangle, Spread, TextFrame};
use paged_scene::Document;

use crate::error::OperationError;
use crate::invert::{
    invert_batch, invert_insert_node, invert_move_node, invert_remove_node,
    invert_set_property,
};
use crate::operation::{
    AppliedOperation, ColorGroupSpec, GradientSpec, GradientStopSpec, InvalidationHint, NodeId,
    NodeSpec, Operation, PathAnchorSpec, PathfinderKind, PropertyPath, StyleCollection, SwatchSpec,
    Value,
};
use crate::pathfinder::{pathfinder_boolean, PathfinderKind as InternalPathfinderKind};

/// Apply an operation to `doc`. Returns the captured `AppliedOperation`
/// (carrying op + inverse + invalidation hint) on success. The only
/// mutation entry point in the crate.
pub fn apply(doc: &mut Document, op: &Operation) -> Result<AppliedOperation, OperationError> {
    match op {
        Operation::SetProperty { node, path, value } => apply_set_property(doc, node, *path, value),
        Operation::InsertNode { parent, position, node, z_slot } => {
            apply_insert_node(doc, parent, *position, *z_slot, node)
        }
        Operation::RemoveNode { node } => apply_remove_node(doc, node),
        Operation::MoveNode { node, new_parent, position } => {
            apply_move_node(doc, node, new_parent, *position)
        }
        Operation::Batch { ops } => apply_batch(doc, ops),
        Operation::MoveLayer { layer_id, new_index } => {
            apply_move_layer(doc, layer_id, *new_index)
        }
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
        Operation::DeleteParagraphStyle { style_id } => {
            apply_delete_paragraph_style(doc, style_id)
        }
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
        Operation::DeleteCharacterStyle { style_id } => {
            apply_delete_character_style(doc, style_id)
        }
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
        Operation::CreateGradient { spec } => apply_create_gradient(doc, spec),
        Operation::EditGradient { gradient_id, spec } => {
            apply_edit_gradient(doc, gradient_id, spec)
        }
        Operation::DeleteGradient { gradient_id } => apply_delete_gradient(doc, gradient_id),
        Operation::CreateColorGroup { spec } => apply_create_color_group(doc, spec),
        Operation::EditColorGroup { group_id, spec } => {
            apply_edit_color_group(doc, group_id, spec)
        }
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
    let mut inputs: Vec<(Vec<paged_parse::PathAnchor>, Vec<usize>)> = Vec::with_capacity(1 + others.len());
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
        batch_children.push(Operation::RemoveNode { node: other.clone() });
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
            let (anchors, _starts) = find_path_anchors_mut(doc, node)
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
            let prev_mode = tw
                .map(|t| t.mode.as_idml().to_string())
                .unwrap_or_default();
            let prev_offsets = tw.map(|t| t.offsets).unwrap_or([0.0; 4]);
            if new_val.is_empty() {
                *tw = None;
            } else {
                *tw = Some(paged_parse::TextWrap {
                    mode: paged_parse::TextWrapMode::from_idml(&new_val),
                    offsets: prev_offsets,
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
            *tw = Some(paged_parse::TextWrap {
                mode: prev_mode,
                offsets: new_offsets,
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
            let (anchors, starts) = find_path_anchors_mut(doc, node)
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
            ds.mode = if new_val.is_empty() { "Drop".to_string() } else { new_val.clone() };
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
            rect.frame_fitting = Some(paged_parse::FrameFittingOption {
                top_crop: Some(new_bounds[0]),
                left_crop: Some(new_bounds[1]),
                bottom_crop: Some(new_bounds[2]),
                right_crop: Some(new_bounds[3]),
                fitting_on_empty_frame: prev_type,
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
            let new_type = if new_val.is_empty() { None } else { Some(new_val.clone()) };
            // Clearing both halves leaves frame_fitting at `None`
            // for honest defaults; otherwise materialise the
            // FrameFitting with the merged state.
            if new_type.is_none()
                && prev_top.is_none()
                && prev_left.is_none()
                && prev_bottom.is_none()
                && prev_right.is_none()
            {
                rect.frame_fitting = None;
            } else {
                rect.frame_fitting = Some(paged_parse::FrameFittingOption {
                    top_crop: prev_top,
                    left_crop: prev_left,
                    bottom_crop: prev_bottom,
                    right_crop: prev_right,
                    fitting_on_empty_frame: new_type,
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
            | PropertyPath::AppliedConditions,
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
            | PropertyPath::ParagraphJustification,
        ) => {
            return apply_paragraph_property(doc, story_id, *start, *end, node, path, value);
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
                    let (prev_value, _) =
                        apply_character_field_on_run(&mut right, path, value)?;
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
                    let (prev_value, _) =
                        apply_character_field_on_run(&mut left, path, value)?;
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
                    let (prev_value, _) =
                        apply_character_field_on_run(&mut mid, path, value)?;
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
    // renderer's text-reflow cache invalidates the right page. The
    // story-to-frame index is built at document open; if it's empty
    // (shouldn't happen for parsed docs) we leave the hint default.
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
    let mut byte_idx = run.text.len();
    let mut chars_seen: u32 = 0;
    for (byte, _) in run.text.char_indices() {
        if chars_seen == char_idx {
            byte_idx = byte;
            break;
        }
        chars_seen += 1;
    }
    let left_text = run.text[..byte_idx].to_string();
    let right_text = run.text[byte_idx..].to_string();
    let mut left = run.clone();
    left.text = left_text;
    let mut right = run;
    right.text = right_text;
    (left, right)
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
                new_val
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect()
            };
            Ok((Value::Text(prev), Value::Text(new_val.clone())))
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
fn unregister_frame_ref(
    spread: &mut Spread,
    template: FrameRef,
    vec_pos: usize,
) -> Option<usize> {
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
        } => {
            let len = spread.spread.text_frames.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut frame =
                new_text_frame(self_id.clone(), bounds_from_array(*bounds), fill_color.clone());
            frame.stroke_color = stroke_color.clone();
            frame.stroke_weight = *stroke_weight;
            spread.spread.text_frames.insert(position, frame);
            register_frame_ref(&mut spread.spread, FrameRef::TextFrame(0), position, z_slot);
        }
        NodeSpec::Rectangle {
            self_id,
            bounds,
            fill_color,
            stroke_color,
            stroke_weight,
        } => {
            let len = spread.spread.rectangles.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut rect =
                new_rectangle(self_id.clone(), bounds_from_array(*bounds), fill_color.clone());
            rect.stroke_color = stroke_color.clone();
            rect.stroke_weight = *stroke_weight;
            spread.spread.rectangles.insert(position, rect);
            register_frame_ref(&mut spread.spread, FrameRef::Rectangle(0), position, z_slot);
        }
        NodeSpec::GraphicLine {
            self_id,
            bounds,
            anchors,
            subpath_starts,
            subpath_open,
            stroke_color,
            stroke_weight,
        } => {
            let len = spread.spread.graphic_lines.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            spread.spread.graphic_lines.insert(
                position,
                new_graphic_line(
                    self_id.clone(),
                    bounds_from_array(*bounds),
                    anchors.iter().map(PathAnchorSpec::to_parse).collect(),
                    subpath_starts.clone(),
                    subpath_open.clone(),
                    stroke_color.clone(),
                    *stroke_weight,
                ),
            );
            register_frame_ref(&mut spread.spread, FrameRef::GraphicLine(0), position, z_slot);
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
        } => {
            let len = spread.spread.polygons.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            spread.spread.polygons.insert(
                position,
                new_polygon(
                    self_id.clone(),
                    bounds_from_array(*bounds),
                    anchors.iter().map(PathAnchorSpec::to_parse).collect(),
                    subpath_starts.clone(),
                    subpath_open.clone(),
                    fill_color.clone(),
                    stroke_color.clone(),
                    *stroke_weight,
                ),
            );
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
                        anchors: line.anchors.iter().map(PathAnchorSpec::from_parse).collect(),
                        subpath_starts: line.subpath_starts,
                        subpath_open: line.subpath_open,
                        stroke_color: line.stroke_color,
                        stroke_weight: line.stroke_weight,
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
                        anchors: poly.anchors.iter().map(PathAnchorSpec::from_parse).collect(),
                        subpath_starts: poly.subpath_starts,
                        subpath_open: poly.subpath_open,
                        fill_color: poly.fill_color,
                        stroke_color: poly.stroke_color,
                        stroke_weight: poly.stroke_weight,
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
            NodeSpec::GraphicLine { .. } => dest.spread.graphic_lines.len(),
            NodeSpec::Polygon { .. } => dest.spread.polygons.len(),
            // CloneTranslate is never captured from the doc — it's
            // an input-only spec for Phase H's Alt-duplicate. Treat
            // as a programmer error if it ever surfaces here.
            NodeSpec::CloneTranslate { .. } => {
                restore_capture(doc, &previous_parent, previous_position, captured, previous_z_slot);
                return Err(OperationError::NodeNotFound(node.clone()));
            }
        },
        None => {
            restore_capture(doc, &previous_parent, previous_position, captured, previous_z_slot);
            return Err(OperationError::NodeNotFound(new_parent.clone()));
        }
    };

    if position > target_len {
        restore_capture(doc, &previous_parent, previous_position, captured, previous_z_slot);
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
    let spread = find_spread_mut(doc, parent_self_id).ok_or_else(|| {
        OperationError::NodeNotFound(NodeId::Spread(parent_self_id.to_string()))
    })?;
    match spec {
        NodeSpec::TextFrame {
            self_id,
            bounds,
            fill_color,
            stroke_color,
            stroke_weight,
        } => {
            let mut frame = new_text_frame(self_id, bounds_from_array(bounds), fill_color);
            frame.stroke_color = stroke_color;
            frame.stroke_weight = stroke_weight;
            spread.spread.text_frames.insert(position, frame);
            register_frame_ref(&mut spread.spread, FrameRef::TextFrame(0), position, z_slot);
        }
        NodeSpec::Rectangle {
            self_id,
            bounds,
            fill_color,
            stroke_color,
            stroke_weight,
        } => {
            let mut rect = new_rectangle(self_id, bounds_from_array(bounds), fill_color);
            rect.stroke_color = stroke_color;
            rect.stroke_weight = stroke_weight;
            spread.spread.rectangles.insert(position, rect);
            register_frame_ref(&mut spread.spread, FrameRef::Rectangle(0), position, z_slot);
        }
        NodeSpec::GraphicLine {
            self_id,
            bounds,
            anchors,
            subpath_starts,
            subpath_open,
            stroke_color,
            stroke_weight,
        } => {
            spread.spread.graphic_lines.insert(
                position,
                new_graphic_line(
                    self_id,
                    bounds_from_array(bounds),
                    anchors.iter().map(PathAnchorSpec::to_parse).collect(),
                    subpath_starts,
                    subpath_open,
                    stroke_color,
                    stroke_weight,
                ),
            );
            register_frame_ref(&mut spread.spread, FrameRef::GraphicLine(0), position, z_slot);
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
        } => {
            spread.spread.polygons.insert(
                position,
                new_polygon(
                    self_id,
                    bounds_from_array(bounds),
                    anchors.iter().map(PathAnchorSpec::to_parse).collect(),
                    subpath_starts,
                    subpath_open,
                    fill_color,
                    stroke_color,
                    stroke_weight,
                ),
            );
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
    let existing = colors.get(swatch_id).ok_or_else(|| {
        OperationError::CollectionEntryNotFound {
            collection: "swatch".to_string(),
            id: swatch_id.to_string(),
        }
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
    let captured = colors.remove(swatch_id).ok_or_else(|| {
        OperationError::CollectionEntryNotFound {
            collection: "swatch".to_string(),
            id: swatch_id.to_string(),
        }
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
    gradients.insert(self_id.clone(), gradient_entry_from_spec(self_id.clone(), spec));
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
    let existing = gradients.get(gradient_id).ok_or_else(|| {
        OperationError::CollectionEntryNotFound {
            collection: "gradient".to_string(),
            id: gradient_id.to_string(),
        }
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
    let existing = groups.get(group_id).ok_or_else(|| {
        OperationError::CollectionEntryNotFound {
            collection: "color group".to_string(),
            id: group_id.to_string(),
        }
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
                let def: $def = serde_json::from_str(json).map_err(|e| {
                    OperationError::InvalidValue {
                        node: NodeId::Layer(String::new()),
                        path: PropertyPath::LayerName,
                        reason: format!("malformed {} restore payload: {e}", $label),
                    }
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
            let def = map.get_mut(style_id).ok_or_else(|| {
                OperationError::CollectionEntryNotFound {
                    collection: $label.to_string(),
                    id: style_id.to_string(),
                }
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
            let captured = map.remove(style_id).ok_or_else(|| {
                OperationError::CollectionEntryNotFound {
                    collection: $label.to_string(),
                    id: style_id.to_string(),
                }
            })?;
            // Serialize the captured def for a lossless create-inverse.
            let json = serde_json::to_string(&captured).map_err(|e| {
                OperationError::InvalidValue {
                    node: NodeId::Layer(String::new()),
                    path: PropertyPath::LayerName,
                    reason: format!("failed to capture {} for undo: {e}", $label),
                }
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
            let Value::Length(n) = value else { return Err(type_err()) };
            let prior = Value::Length(def.point_size);
            def.point_size = *n;
            Ok(prior)
        }
        PropertyPath::CharacterTracking => {
            let Value::Length(n) = value else { return Err(type_err()) };
            let prior = Value::Length(def.tracking);
            def.tracking = *n;
            Ok(prior)
        }
        PropertyPath::CharacterFillColor => {
            let Value::ColorRef(c) = value else { return Err(type_err()) };
            let prior = Value::ColorRef(def.fill_color.clone());
            def.fill_color = c.clone();
            Ok(prior)
        }
        PropertyPath::ParagraphSpaceBefore => {
            let Value::Length(n) = value else { return Err(type_err()) };
            let prior = Value::Length(def.space_before);
            def.space_before = *n;
            Ok(prior)
        }
        PropertyPath::ParagraphSpaceAfter => {
            let Value::Length(n) = value else { return Err(type_err()) };
            let prior = Value::Length(def.space_after);
            def.space_after = *n;
            Ok(prior)
        }
        PropertyPath::ParagraphFirstLineIndent => {
            let Value::Length(n) = value else { return Err(type_err()) };
            let prior = Value::Length(def.first_line_indent);
            def.first_line_indent = *n;
            Ok(prior)
        }
        PropertyPath::ParagraphJustification => {
            let Value::Text(s) = value else { return Err(type_err()) };
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
            let Value::Length(n) = value else { return Err(type_err()) };
            let prior = Value::Length(def.point_size);
            def.point_size = *n;
            Ok(prior)
        }
        PropertyPath::CharacterTracking => {
            let Value::Length(n) = value else { return Err(type_err()) };
            let prior = Value::Length(def.tracking);
            def.tracking = *n;
            Ok(prior)
        }
        PropertyPath::CharacterFillColor => {
            let Value::ColorRef(c) = value else { return Err(type_err()) };
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

fn find_oval_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut paged_parse::Oval> {
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
                PropertyPath::FrameGradientStrokeLength => {
                    Some(&mut $item.gradient_stroke_length)
                }
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

fn find_group_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut paged_parse::Group> {
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
fn find_layer_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut paged_parse::Layer> {
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

fn expect_transform(
    path: PropertyPath,
    value: &Value,
) -> Result<Option<[f32; 6]>, OperationError> {
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
                    return Some((&mut p.anchors, &mut p.subpath_starts));
                }
            }
            NodeId::TextFrame(_) => {
                if let Some(p) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some((&mut p.anchors, &mut p.subpath_starts));
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(p) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some((&mut p.anchors, &mut p.subpath_starts));
                }
            }
            NodeId::GraphicLine(_) => {
                if let Some(p) = parsed
                    .spread
                    .graphic_lines
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some((&mut p.anchors, &mut p.subpath_starts));
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
fn increment_subpath_starts(starts: &mut Vec<usize>, n: usize) {
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
    let (anchors, subpath_starts) = find_path_anchors_mut(doc, node)
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
    let (anchors, subpath_starts) = find_path_anchors_mut(doc, node)
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
    let (anchors, subpath_starts) = find_path_anchors_mut(doc, node)
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
            apply_translate_in_place(
                &mut clone.bounds,
                &mut clone.item_transform,
                eff_dx,
                eff_dy,
            );
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
            apply_translate_in_place(
                &mut clone.bounds,
                &mut clone.item_transform,
                eff_dx,
                eff_dy,
            );
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
            !((a - 1.0).abs() < 1e-4
                && (d - 1.0).abs() < 1e-4
                && b.abs() < 1e-4
                && c.abs() < 1e-4)
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
fn new_text_frame(self_id: String, bounds: Bounds, fill_color: Option<String>) -> TextFrame {
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

fn new_rectangle(self_id: String, bounds: Bounds, fill_color: Option<String>) -> Rectangle {
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
