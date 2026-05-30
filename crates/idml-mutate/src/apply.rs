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
        Operation::MoveLayer { layer_id, new_index } => {
            apply_move_layer(doc, layer_id, *new_index)
        }
        Operation::InsertLayer {
            position,
            name,
            self_id,
        } => apply_insert_layer(doc, *position, name, self_id.as_deref()),
        Operation::RemoveLayer { layer_id } => apply_remove_layer(doc, layer_id),
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
        // Track J fan-out — path-topology ops accept any path-bearing
        // page item kind (Polygon, TextFrame, Rectangle, GraphicLine).
        // The four kinds carry identical `anchors` + `subpath_starts`
        // fields in idml-parse; the helper `find_path_anchors_mut`
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
        // (`idml-parse/spread.rs:141-144`), so mutating only the
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
            | PropertyPath::CharacterFillColor,
        ) => {
            return apply_character_property(doc, story_id, *start, *end, node, path, value);
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
        let original_runs: Vec<idml_parse::CharacterRun> = para.runs.drain(..).collect();
        let mut new_runs: Vec<idml_parse::CharacterRun> =
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

/// SDK Phase 3.x — split a `CharacterRun` at character offset
/// `char_idx`. The left piece contains the first `char_idx`
/// characters of `run.text`; the right piece contains the rest.
/// Every other field is duplicated via `Clone` so the two pieces
/// inherit identical properties pre-mutation. `char_idx` must lie
/// strictly inside the run (0 < char_idx < run.text.chars().count()) —
/// the caller is responsible for that constraint; this function
/// produces undefined byte boundaries otherwise.
fn split_run_at(
    run: idml_parse::CharacterRun,
    char_idx: u32,
) -> (idml_parse::CharacterRun, idml_parse::CharacterRun) {
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
    run: &mut idml_parse::CharacterRun,
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
        idml_parse::Layer {
            self_id: self_id.clone(),
            name: Some(name.to_string()),
            visible: true,
            locked: false,
            printable: true,
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

fn find_group_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut idml_parse::Group> {
    for parsed in &mut doc.spreads {
        for group in &mut parsed.spread.groups {
            if group.self_id.as_deref() == Some(self_id) {
                return Some(group);
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

/// Track M — locate a `<Layer>` by its `Self` id in the document's
/// designmap. The designmap is the only place layers live; spread /
/// page items only carry an `ItemLayer` reference back into it.
fn find_layer_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut idml_parse::Layer> {
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
    doc: &'a mut idml_scene::Document,
    node: &NodeId,
) -> Option<(
    &'a mut Vec<idml_parse::PathAnchor>,
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
    doc: &mut idml_scene::Document,
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
    doc: &mut idml_scene::Document,
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
    doc: &mut idml_scene::Document,
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
            // translation + 0/90/180/270 rotation (idml-parse spread.rs:81).
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
