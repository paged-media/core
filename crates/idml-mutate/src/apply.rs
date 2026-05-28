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
//! Stage 1 limitations (flagged in `docs/verso/scripting-layer.md`'s
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

use idml_parse::{Bounds, Rectangle, TextFrame};
use idml_scene::Document;

use crate::error::OperationError;
use crate::invert::{
    invert_batch, invert_insert_node, invert_move_node, invert_remove_node,
    invert_set_property,
};
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, NodeSpec, Operation, PropertyPath, Value,
};

/// Apply an operation to `doc`. Returns the captured `AppliedOperation`
/// (carrying op + inverse + invalidation hint) on success. The only
/// mutation entry point in the crate.
pub fn apply(doc: &mut Document, op: &Operation) -> Result<AppliedOperation, OperationError> {
    match op {
        Operation::SetProperty { node, path, value } => apply_set_property(doc, node, *path, value),
        Operation::InsertNode { parent, position, node } => {
            apply_insert_node(doc, parent, *position, node)
        }
        Operation::RemoveNode { node } => apply_remove_node(doc, node),
        Operation::MoveNode { node, new_parent, position } => {
            apply_move_node(doc, node, new_parent, *position)
        }
        Operation::Batch { ops } => apply_batch(doc, ops),
    }
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
        (NodeId::Polygon(id), PropertyPath::PathPointInsert) => {
            return apply_path_point_insert(doc, node, id, value);
        }
        (NodeId::Polygon(id), PropertyPath::PathPointRemove) => {
            return apply_path_point_remove(doc, node, id, value);
        }
        (NodeId::Polygon(id), PropertyPath::PathPointCurveType) => {
            return apply_path_point_curve_type(doc, node, id, value);
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
        // ---- Phase H: FramePathPoint (Polygon) --------------------
        (NodeId::Polygon(id), PropertyPath::FramePathPoint) => {
            let (address, position) = expect_path_point(path, value)?;
            // Find the polygon globally (any spread, not bound to a
            // parent like the SetProperty arms above).
            let polygon = doc
                .spreads
                .iter_mut()
                .flat_map(|s| s.spread.polygons.iter_mut())
                .find(|p| p.self_id.as_deref() == Some(id.as_str()))
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let Some(anchor) = polygon.anchors.get_mut(address.index) else {
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
// InsertNode
// ---------------------------------------------------------------------------

fn apply_insert_node(
    doc: &mut Document,
    parent: &NodeId,
    position: usize,
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
        } => {
            let len = spread.spread.text_frames.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            spread.spread.text_frames.insert(
                position,
                new_text_frame(self_id.clone(), bounds_from_array(*bounds), fill_color.clone()),
            );
        }
        NodeSpec::Rectangle {
            self_id,
            bounds,
            fill_color,
        } => {
            let len = spread.spread.rectangles.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            spread.spread.rectangles.insert(
                position,
                new_rectangle(self_id.clone(), bounds_from_array(*bounds), fill_color.clone()),
            );
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
    let (parent, position, captured) = remove_and_capture(doc, node)?;
    let inverse = invert_remove_node(parent, position, captured);
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
/// into a `NodeSpec`, and remove it. Returns `(parent_id, position,
/// spec)` for the caller to feed into the inverse.
fn remove_and_capture(
    doc: &mut Document,
    node: &NodeId,
) -> Result<(NodeId, usize, NodeSpec), OperationError> {
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
                    let parent = spread_parent_id(parsed);
                    let spec = NodeSpec::TextFrame {
                        self_id: id.clone(),
                        bounds: bounds_to_array(frame.bounds),
                        fill_color: frame.fill_color,
                    };
                    return Ok((parent, pos, spec));
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
                    let parent = spread_parent_id(parsed);
                    let spec = NodeSpec::Rectangle {
                        self_id: id.clone(),
                        bounds: bounds_to_array(rect.bounds),
                        fill_color: rect.fill_color,
                    };
                    return Ok((parent, pos, spec));
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
    let (previous_parent, previous_position, captured) = remove_and_capture(doc, node)?;

    // Read destination spread length without holding a borrow across
    // the potentially-rollback path.
    let target_len = match find_spread(doc, &new_parent_id) {
        Some(dest) => match &captured {
            NodeSpec::TextFrame { .. } => dest.spread.text_frames.len(),
            NodeSpec::Rectangle { .. } => dest.spread.rectangles.len(),
            // CloneTranslate is never captured from the doc — it's
            // an input-only spec for Phase H's Alt-duplicate. Treat
            // as a programmer error if it ever surfaces here.
            NodeSpec::CloneTranslate { .. } => {
                restore_capture(doc, &previous_parent, previous_position, captured);
                return Err(OperationError::NodeNotFound(node.clone()));
            }
        },
        None => {
            restore_capture(doc, &previous_parent, previous_position, captured);
            return Err(OperationError::NodeNotFound(new_parent.clone()));
        }
    };

    if position > target_len {
        restore_capture(doc, &previous_parent, previous_position, captured);
        return Err(OperationError::InvalidPosition {
            parent: new_parent.clone(),
            position,
            len: target_len,
        });
    }

    insert_captured(doc, &new_parent_id, position, captured)?;

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
fn restore_capture(doc: &mut Document, parent: &NodeId, position: usize, spec: NodeSpec) {
    let _ = insert_captured(doc, parent.self_id(), position, spec);
}

fn insert_captured(
    doc: &mut Document,
    parent_self_id: &str,
    position: usize,
    spec: NodeSpec,
) -> Result<(), OperationError> {
    let spread = find_spread_mut(doc, parent_self_id).ok_or_else(|| {
        OperationError::NodeNotFound(NodeId::Spread(parent_self_id.to_string()))
    })?;
    match spec {
        NodeSpec::TextFrame {
            self_id,
            bounds,
            fill_color,
        } => {
            spread
                .spread
                .text_frames
                .insert(position, new_text_frame(self_id, bounds_from_array(bounds), fill_color));
        }
        NodeSpec::Rectangle {
            self_id,
            bounds,
            fill_color,
        } => {
            spread
                .spread
                .rectangles
                .insert(position, new_rectangle(self_id, bounds_from_array(bounds), fill_color));
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

fn find_spread<'a>(doc: &'a Document, self_id: &str) -> Option<&'a idml_scene::ParsedSpread> {
    doc.spreads
        .iter()
        .find(|p| p.spread.self_id.as_deref() == Some(self_id))
}

fn find_spread_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut idml_scene::ParsedSpread> {
    doc.spreads
        .iter_mut()
        .find(|p| p.spread.self_id.as_deref() == Some(self_id))
}

fn spread_parent_id(parsed: &idml_scene::ParsedSpread) -> NodeId {
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
            _ => {}
        }
    }
    false
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
    doc: &mut idml_scene::Document,
    node: &NodeId,
    polygon_id: &str,
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
    let polygon = doc
        .spreads
        .iter_mut()
        .flat_map(|s| s.spread.polygons.iter_mut())
        .find(|p| p.self_id.as_deref() == Some(polygon_id))
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    // Insert is allowed at end (index == len), not past it.
    if index > polygon.anchors.len() {
        return Err(OperationError::NodeNotFound(node.clone()));
    }
    polygon.anchors.insert(index, anchor_spec.to_parse());
    if let Some(restore) = prev_subpath_starts {
        // Inverse-of-Remove case: restore the pre-Remove starts
        // verbatim. The starts captured at Remove time pointed into
        // an anchors vec one element smaller; inserting brings the
        // length back, so the snapshot is valid as-is.
        polygon.subpath_starts = restore;
    } else {
        increment_subpath_starts(&mut polygon.subpath_starts, index);
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
    doc: &mut idml_scene::Document,
    node: &NodeId,
    polygon_id: &str,
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
    let polygon = doc
        .spreads
        .iter_mut()
        .flat_map(|s| s.spread.polygons.iter_mut())
        .find(|p| p.self_id.as_deref() == Some(polygon_id))
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    if index >= polygon.anchors.len() {
        return Err(OperationError::NodeNotFound(node.clone()));
    }
    // Capture for the inverse BEFORE mutating.
    let captured = crate::operation::PathAnchorSpec::from_parse(&polygon.anchors[index]);
    let prev_starts = polygon.subpath_starts.clone();
    // Remove + adjust subpath_starts.
    polygon.anchors.remove(index);
    let new_len = polygon.anchors.len();
    decrement_subpath_starts(&mut polygon.subpath_starts, index, new_len);
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
    doc: &mut idml_scene::Document,
    node: &NodeId,
    polygon_id: &str,
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
    // Find polygon + bounds-check + collect neighbour anchor
    // positions before grabbing the mutable borrow for the anchor.
    let polygon = doc
        .spreads
        .iter_mut()
        .flat_map(|s| s.spread.polygons.iter_mut())
        .find(|p| p.self_id.as_deref() == Some(polygon_id))
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    if index >= polygon.anchors.len() {
        return Err(OperationError::NodeNotFound(node.clone()));
    }
    // Neighbour positions for the smooth derivation, restricted to
    // the same subpath. (Crossing subpath boundaries would derive a
    // tangent against an anchor on a different contour, which is
    // nonsensical.)
    let (sub_start, sub_end) = subpath_bounds_for(&polygon.subpath_starts, polygon.anchors.len(), index);
    let prev_neighbour = if index > sub_start {
        Some(polygon.anchors[index - 1].anchor)
    } else {
        None
    };
    let next_neighbour = if index + 1 < sub_end {
        Some(polygon.anchors[index + 1].anchor)
    } else {
        None
    };
    let captured = crate::operation::PathAnchorSpec::from_parse(&polygon.anchors[index]);
    let anchor = &mut polygon.anchors[index];
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
    let Some(idx) = source_spread_idx else {
        return Err(OperationError::NodeNotFound(source.clone()));
    };
    let spread = &mut doc.spreads[idx];
    let parent_spread_id = spread.spread.self_id.clone().unwrap_or_default();
    match source {
        NodeId::TextFrame(src_id) => {
            let src_frame: TextFrame = spread
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
                *dx,
                *dy,
            );
            let len = spread.spread.text_frames.len();
            let pos = position.min(len);
            spread.spread.text_frames.insert(pos, clone);
        }
        NodeId::Rectangle(src_id) => {
            let src_rect: Rectangle = spread
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
                *dx,
                *dy,
            );
            let len = spread.spread.rectangles.len();
            let pos = position.min(len);
            spread.spread.rectangles.insert(pos, clone);
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
        },
        inverse,
        invalidation,
    })
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
        anchors: Vec::new(),
        subpath_starts: Vec::new(),
        subpath_open: Vec::new(),
    }
}
