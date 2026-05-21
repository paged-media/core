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
