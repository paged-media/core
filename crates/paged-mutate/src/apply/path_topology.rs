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
use paged_parse::{Bounds, FrameRef, GraphicLine, Oval, Polygon, Rectangle, TextFrame};
use paged_scene::Document;

use crate::error::OperationError;
use crate::invert::invert_insert_node;
use crate::operation::{
    AppliedOperation, FieldKind, InvalidationHint, NodeId, NodeSpec, Operation, PathAnchorSpec,
    PropertyPath, StyleScope, Value,
};

// ---------------------------------------------------------------------------
// Track J — path topology helpers
// ---------------------------------------------------------------------------

/// Track J fan-out — return mutable references to the `anchors` +
/// `subpath_starts` vecs of any path-bearing page item (Polygon,
/// TextFrame, Rectangle, GraphicLine). All four kinds carry these
/// fields with identical semantics so the path-topology apply arms
/// stay kind-agnostic.
pub(super) fn find_path_anchors_mut<'a>(
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
pub(super) fn increment_subpath_starts(starts: &mut [usize], n: usize) {
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
pub(super) fn decrement_subpath_starts(starts: &mut Vec<usize>, n: usize, new_anchors_len: usize) {
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
pub(super) fn apply_path_open_at(
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
pub(super) fn apply_path_kernel_op(
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
        | Value::OutlineStrokeVariable {
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
                expected: "OutlineStroke | OutlineStrokeVariable | OffsetPath | SimplifyPath"
                    .to_string(),
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
            Value::OutlineStrokeVariable {
                widths,
                cap,
                join,
                miter_limit,
                ..
            } => kurbo_kernel::variable_width_outline_stroke(
                anchors,
                subpath_starts,
                subpath_open,
                widths,
                parse_cap(cap).ok_or_else(|| invalid(format!("unknown cap \"{cap}\"")))?,
                parse_join(join).ok_or_else(|| invalid(format!("unknown join \"{join}\"")))?,
                *miter_limit,
            ),
            Value::OffsetPath {
                delta,
                join,
                miter_limit,
                ..
            } => {
                let join_k =
                    parse_join(join).ok_or_else(|| invalid(format!("unknown join \"{join}\"")))?;
                let is_open = subpath_open.iter().any(|o| *o) || subpath_starts.len() > 1;
                if is_open {
                    // B-21: an OPEN path has no inside/outside, so its
                    // offset is the both-sides band of width 2·|δ|
                    // (Illustrator's Offset Path closes an open path into
                    // an outline). Delegate to the stroke-outline kernel
                    // with a butt cap (the band ends square at the path
                    // endpoints). Closed single contours keep the
                    // single-contour offset below.
                    kurbo_kernel::outline_stroke(
                        anchors,
                        subpath_starts,
                        subpath_open,
                        2.0 * delta.abs(),
                        StrokeCap::Butt,
                        join_k,
                        *miter_limit,
                    )
                } else {
                    kurbo_kernel::offset_closed_path(
                        anchors,
                        subpath_starts,
                        subpath_open,
                        *delta,
                        join_k,
                        *miter_limit,
                    )
                }
            }
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
            Value::OutlineStrokeVariable {
                widths,
                cap,
                join,
                miter_limit,
                ..
            } => Value::OutlineStrokeVariable {
                widths,
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

pub(super) fn apply_path_point_insert(
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

pub(super) fn apply_path_point_remove(
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

pub(super) fn apply_path_point_curve_type(
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
pub(super) fn subpath_bounds_for(
    starts: &[usize],
    anchors_len: usize,
    index: usize,
) -> (usize, usize) {
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
pub(super) fn apply_insert_clone_translate(
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
pub(super) fn spread_origin(item_transform: &Option<[f32; 6]>) -> (f32, f32) {
    match item_transform {
        Some(m) => (m[4], m[5]),
        None => (0.0, 0.0),
    }
}

/// Phase H — shift either the bounds (un-rotated frame) or the
/// `item_transform`'s tx/ty (rotated frame) so the cloned frame
/// lands at the user's drop position regardless of frame rotation.
pub(super) fn apply_translate_in_place(
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

pub(super) fn bounds_to_array(b: Bounds) -> [f32; 4] {
    [b.top, b.left, b.bottom, b.right]
}

pub(super) fn bounds_from_array(a: [f32; 4]) -> Bounds {
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
        stroke_dash: Vec::new(),
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
        visible: true,
        locked: false,
    }
}

pub(super) fn new_graphic_line(
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
        end_join: None,
        miter_limit: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        stroke_dash: Vec::new(),
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
        visible: true,
        locked: false,
        start_arrow: paged_parse::ArrowheadType::None,
        end_arrow: paged_parse::ArrowheadType::None,
        start_arrow_scale: 100.0,
        end_arrow_scale: 100.0,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn new_polygon(
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
        stroke_alignment: None,
        end_join: None,
        miter_limit: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        stroke_dash: Vec::new(),
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
        image_clip: None,
        overprint_fill: false,
        overprint_stroke: false,
        nonprinting: false,
        visible: true,
        locked: false,
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
        stroke_alignment: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        stroke_dash: Vec::new(),
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
        image_clip: None,
        overprint_fill: false,
        overprint_stroke: false,
        nonprinting: false,
        visible: true,
        locked: false,
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
        image_clip: None,
        applied_object_style: None,
        text_wrap: None,
        frame_fitting: None,
        stroke_type: None,
        stroke_gap_color: None,
        stroke_gap_tint: None,
        stroke_dash: Vec::new(),
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
        visible: true,
        locked: false,
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
pub(super) fn find_text_frame_pos(doc: &Document, frame_id: &str) -> Option<(usize, usize)> {
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
pub(super) fn reflow_hint_for_story(doc: &Document, story_id: &str) -> InvalidationHint {
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
pub(super) fn find_table_pos(
    doc: &Document,
    story_id: &str,
    table_id: &str,
) -> Option<(usize, usize)> {
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
pub(super) fn find_table_mut<'a>(
    doc: &'a mut Document,
    story_id: &str,
    table_id: &str,
) -> Option<&'a mut paged_parse::Table> {
    let (si, pi) = find_table_pos(doc, story_id, table_id)?;
    doc.stories[si].story.paragraphs[pi].table.as_mut()
}

/// S-03 — create a `<Table>` inside a story. The table-CREATE op
/// (`InsertNode { parent: NodeId::Story, node: NodeSpec::Table }`),
/// mirror of `Mutation::InsertOval` but story-scoped.
///
/// **Offset decision:** a table hangs off `Paragraph::table` (one
/// table per paragraph — see `find_table_pos`), and IDML has no story-
/// character offset for a table the way it does for runs. So the new
/// table is attached on a FRESH paragraph appended to the END of the
/// story's paragraph list. This never disturbs existing content and
/// matches "default to the story end" from the plan. The `position`
/// argument is accepted for the `InsertNode` shape but ignored (a
/// story is a single insert slot for a table); a future v2 could honour
/// it as a paragraph index.
///
/// mutate-never-throws: an unknown story id, a non-Story parent, or a
/// duplicate table id return an `OperationError` outcome (the channel /
/// model layer surfaces it as a failed-mutation reply, never a panic).
/// Inverse: `RemoveNode { NodeId::Table { story_id, table_id } }`.
pub(super) fn apply_insert_table(
    doc: &mut Document,
    parent: &NodeId,
    _position: usize,
    spec: &NodeSpec,
) -> Result<AppliedOperation, OperationError> {
    let NodeId::Story(story_id) = parent else {
        return Err(OperationError::InvalidParent {
            parent: parent.clone(),
            child_kind: "Table".to_string(),
        });
    };
    let NodeSpec::Table { self_id, .. } = spec else {
        unreachable!("apply_insert_table dispatched on a non-Table NodeSpec");
    };
    let table_id = self_id.clone();
    let node = NodeId::Table {
        story_id: story_id.clone(),
        table_id: table_id.clone(),
    };

    // Table ids must be unique within the document (the IDML `Self`
    // invariant). Reject a collision rather than silently shadowing.
    if find_table_pos(doc, story_id, &table_id).is_some() {
        return Err(OperationError::DuplicateNodeId {
            id: table_id.clone(),
        });
    }

    let si = doc
        .stories
        .iter()
        .position(|s| s.self_id == *story_id)
        .ok_or_else(|| OperationError::NodeNotFound(parent.clone()))?;

    let table = spec.to_parse_table();
    // Append the table on a fresh paragraph at the END of the story.
    doc.stories[si]
        .story
        .paragraphs
        .push(paged_parse::Paragraph {
            table: Some(table),
            ..Default::default()
        });

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::InsertNode {
            parent: parent.clone(),
            position: _position,
            node: spec.clone(),
            z_slot: None,
        },
        inverse: Operation::RemoveNode { node },
        invalidation,
    })
}

/// W3.A1 — `AppliedTableStyle` write on a `NodeId::Table`. The only
/// table-scoped `SetProperty` path today (row/column/structure edits
/// are their own Operations). Empty string clears the override.
pub(super) fn apply_table_property(
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
pub(super) fn apply_cell_property(
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
        // W1.11b — per-cell edge-stroke overrides. Colour paths take a
        // `Value::ColorRef` (`None` clears the inline override back to
        // the cell-style cascade); weight / tint paths take a
        // `Value::Length` (`None` clears). Each arm snapshots the prior
        // value into the inverse so undo round-trips bytewise. The
        // renderer already honours these parse fields (per-edge stroke
        // emit in `pipeline/tables.rs`), so a write is immediately
        // visible once the host story reflows.
        PropertyPath::CellTopEdgeStrokeColor => {
            let new = expect_color_ref(path, value)?;
            let prev = cell.top_edge_stroke_color.clone();
            cell.top_edge_stroke_color = new;
            Value::ColorRef(prev)
        }
        PropertyPath::CellTopEdgeStrokeWeight => {
            let new = expect_length(path, value)?;
            let prev = cell.top_edge_stroke_weight;
            cell.top_edge_stroke_weight = new;
            Value::Length(prev)
        }
        PropertyPath::CellTopEdgeStrokeTint => {
            let new = expect_length(path, value)?;
            let prev = cell.top_edge_stroke_tint;
            cell.top_edge_stroke_tint = new;
            Value::Length(prev)
        }
        PropertyPath::CellBottomEdgeStrokeColor => {
            let new = expect_color_ref(path, value)?;
            let prev = cell.bottom_edge_stroke_color.clone();
            cell.bottom_edge_stroke_color = new;
            Value::ColorRef(prev)
        }
        PropertyPath::CellBottomEdgeStrokeWeight => {
            let new = expect_length(path, value)?;
            let prev = cell.bottom_edge_stroke_weight;
            cell.bottom_edge_stroke_weight = new;
            Value::Length(prev)
        }
        PropertyPath::CellBottomEdgeStrokeTint => {
            let new = expect_length(path, value)?;
            let prev = cell.bottom_edge_stroke_tint;
            cell.bottom_edge_stroke_tint = new;
            Value::Length(prev)
        }
        PropertyPath::CellLeftEdgeStrokeColor => {
            let new = expect_color_ref(path, value)?;
            let prev = cell.left_edge_stroke_color.clone();
            cell.left_edge_stroke_color = new;
            Value::ColorRef(prev)
        }
        PropertyPath::CellLeftEdgeStrokeWeight => {
            let new = expect_length(path, value)?;
            let prev = cell.left_edge_stroke_weight;
            cell.left_edge_stroke_weight = new;
            Value::Length(prev)
        }
        PropertyPath::CellLeftEdgeStrokeTint => {
            let new = expect_length(path, value)?;
            let prev = cell.left_edge_stroke_tint;
            cell.left_edge_stroke_tint = new;
            Value::Length(prev)
        }
        PropertyPath::CellRightEdgeStrokeColor => {
            let new = expect_color_ref(path, value)?;
            let prev = cell.right_edge_stroke_color.clone();
            cell.right_edge_stroke_color = new;
            Value::ColorRef(prev)
        }
        PropertyPath::CellRightEdgeStrokeWeight => {
            let new = expect_length(path, value)?;
            let prev = cell.right_edge_stroke_weight;
            cell.right_edge_stroke_weight = new;
            Value::Length(prev)
        }
        PropertyPath::CellRightEdgeStrokeTint => {
            let new = expect_length(path, value)?;
            let prev = cell.right_edge_stroke_tint;
            cell.right_edge_stroke_tint = new;
            Value::Length(prev)
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
pub(super) fn apply_set_row_height(
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
pub(super) fn apply_set_column_width(
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
pub(super) fn set_cell_name(cell: &mut paged_parse::TableCell, col: u32, row: u32) {
    cell.name = Some(format!("{col}:{row}"));
}

/// W3.A1 — insert a row at `at`. Cells in rows ≥ `at` shift down (+1);
/// a fresh empty cell per column is minted at the new row;
/// `body_row_count` / the `rows` vec grow. When `restore` is `Some`
/// (the `DeleteTableRow` inverse), the captured row + cells are
/// re-inserted verbatim instead of minting empties.
pub(super) fn apply_insert_table_row(
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
pub(super) fn apply_delete_table_row(
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
pub(super) fn apply_insert_table_column(
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
pub(super) fn apply_delete_table_column(
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

/// W1.12a — which row band (`HeaderRowCount` / `FooterRowCount`) an
/// insert / remove targets. The body band is implicit (total − header −
/// footer); header rows sit at the top of the row sequence, footer rows
/// at the bottom.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TableBand {
    Header,
    Footer,
}

/// W1.12a — insert an empty row into the header (top) or footer (bottom)
/// band, bumping the band's count. Header inserts land at row index 0
/// (everything shifts down); footer inserts land after the last row.
/// `restore` (the `Remove*` inverse) re-inserts the captured row + cells
/// verbatim; otherwise a fresh empty cell per column is minted. Inverse:
/// `Remove{Header,Footer}Row`.
pub(super) fn apply_insert_band_row(
    doc: &mut Document,
    story_id: &str,
    table_id: &str,
    band: TableBand,
    restore: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::Table {
        story_id: story_id.to_string(),
        table_id: table_id.to_string(),
    };
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let total_rows = table.rows.len() as u32;
    // Header rows prepend at index 0; footer rows append at the end.
    let at = match band {
        TableBand::Header => 0,
        TableBand::Footer => total_rows,
    };
    let col_count = table.columns.len().max(table.column_count as usize);

    // Shift existing cells in rows ≥ `at` down by one (header insert
    // pushes the whole table down; a footer append shifts nothing).
    for cell in &mut table.cells {
        if let Some((c, r)) = cell.coords() {
            if r >= at {
                set_cell_name(cell, c, r + 1);
            }
        }
    }

    let (new_row, new_cells): (paged_parse::TableRow, Vec<paged_parse::TableCell>) = match restore {
        Some(blob) => {
            let removed = parse_restore_blob(blob, story_id, table_id)?;
            let mut cells: Vec<paged_parse::TableCell> =
                removed.cells.iter().map(TableCellSpec::to_parse).collect();
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
    match band {
        TableBand::Header => {
            table.header_row_count = table.header_row_count.saturating_add(1);
        }
        TableBand::Footer => {
            table.footer_row_count = table.footer_row_count.saturating_add(1);
        }
    }
    renumber_table_rows(table);

    let (op, inverse) = match band {
        TableBand::Header => (
            Operation::InsertHeaderRow {
                story_id: story_id.to_string(),
                table_id: table_id.to_string(),
                restore: restore.map(str::to_string),
            },
            Operation::RemoveHeaderRow {
                story_id: story_id.to_string(),
                table_id: table_id.to_string(),
            },
        ),
        TableBand::Footer => (
            Operation::InsertFooterRow {
                story_id: story_id.to_string(),
                table_id: table_id.to_string(),
                restore: restore.map(str::to_string),
            },
            Operation::RemoveFooterRow {
                story_id: story_id.to_string(),
                table_id: table_id.to_string(),
            },
        ),
    };

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op,
        inverse,
        invalidation,
    })
}

/// W1.12a — remove the first header row (band == Header) or last footer
/// row (band == Footer), decrementing the band's count. Captures the
/// removed row + its cells into the inverse's `restore` blob so undo
/// (`Insert{Header,Footer}Row { restore }`) is lossless. Rejected when
/// the band is empty.
pub(super) fn apply_remove_band_row(
    doc: &mut Document,
    story_id: &str,
    table_id: &str,
    band: TableBand,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::Table {
        story_id: story_id.to_string(),
        table_id: table_id.to_string(),
    };
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let total_rows = table.rows.len() as u32;
    let band_count = match band {
        TableBand::Header => table.header_row_count,
        TableBand::Footer => table.footer_row_count,
    };
    if band_count == 0 {
        let which = match band {
            TableBand::Header => "header",
            TableBand::Footer => "footer",
        };
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::FrameBounds,
            reason: format!("table has no {which} row to remove"),
        });
    }
    // Header removes the top row (index 0); footer removes the bottom.
    let at = match band {
        TableBand::Header => 0,
        TableBand::Footer => total_rows - 1,
    };

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
    match band {
        TableBand::Header => {
            table.header_row_count = table.header_row_count.saturating_sub(1);
        }
        TableBand::Footer => {
            table.footer_row_count = table.footer_row_count.saturating_sub(1);
        }
    }
    renumber_table_rows(table);

    let restore_blob = serde_json::to_string(&RemovedTableLine {
        row: Some(TableRowSpec::from_parse(&removed_row)),
        column: None,
        cells: captured_cells,
    })
    .expect("RemovedTableLine serialises");

    let (op, inverse) = match band {
        TableBand::Header => (
            Operation::RemoveHeaderRow {
                story_id: story_id.to_string(),
                table_id: table_id.to_string(),
            },
            Operation::InsertHeaderRow {
                story_id: story_id.to_string(),
                table_id: table_id.to_string(),
                restore: Some(restore_blob),
            },
        ),
        TableBand::Footer => (
            Operation::RemoveFooterRow {
                story_id: story_id.to_string(),
                table_id: table_id.to_string(),
            },
            Operation::InsertFooterRow {
                story_id: story_id.to_string(),
                table_id: table_id.to_string(),
                restore: Some(restore_blob),
            },
        ),
    };

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op,
        inverse,
        invalidation,
    })
}

/// W1.12b — set the `RowSpan` / `ColumnSpan` of the cell originating at
/// `(row, col)`. The inverse carries the prior `(row_span, column_span)`
/// so undo restores the exact prior spans. Spans clamp to ≥ 1 (a 0 span
/// is meaningless; IDML's minimum is 1). Rejected when no cell
/// originates at `(row, col)` — span only applies to a real grid origin,
/// not a slot already covered by another cell's span.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_set_cell_span(
    doc: &mut Document,
    story_id: &str,
    table_id: &str,
    row: u32,
    col: u32,
    row_span: u32,
    column_span: u32,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::TableCell {
        story_id: story_id.to_string(),
        table_id: table_id.to_string(),
        row,
        col,
    };
    let table = find_table_mut(doc, story_id, table_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let cell = table
        .cells
        .iter_mut()
        .find(|c| c.coords() == Some((col, row)))
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
    let prev_row_span = cell.row_span.max(1);
    let prev_col_span = cell.column_span.max(1);
    cell.row_span = row_span.max(1);
    cell.column_span = column_span.max(1);

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::SetCellSpan {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            row,
            col,
            row_span,
            column_span,
        },
        inverse: Operation::SetCellSpan {
            story_id: story_id.to_string(),
            table_id: table_id.to_string(),
            row,
            col,
            row_span: prev_row_span,
            column_span: prev_col_span,
        },
        invalidation,
    })
}

/// W3.A1 — decode a `DeleteTable{Row,Column}` restore blob. On a bad
/// blob, raises `InvalidValue` rather than panicking (the blob crosses
/// the wasm boundary on redo of a deserialised op log).
pub(super) fn parse_restore_blob(
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
pub(super) fn renumber_table_rows(table: &mut paged_parse::Table) {
    for (i, row) in table.rows.iter_mut().enumerate() {
        row.name = Some(i.to_string());
    }
}

/// W3.A1 — renumber `<Column>` `Name` attributes after an insert /
/// delete.
pub(super) fn renumber_table_columns(table: &mut paged_parse::Table) {
    for (i, col) in table.columns.iter_mut().enumerate() {
        col.name = Some(i.to_string());
    }
}

/// True when `frame_id` carries no story content of its own — either it
/// has no `ParentStory`, or its parent story has no non-empty runs. Used
/// by `LinkFrames` to honour InDesign's "thread into empty frames only"
/// rule.
pub(super) fn frame_has_no_own_content(doc: &Document, frame_id: &str) -> bool {
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
pub(super) fn chain_reaches(doc: &Document, start: &str, target: &str) -> bool {
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

pub(super) fn apply_link_frames(
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

pub(super) fn apply_unlink_frames(
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

pub(super) fn apply_apply_style(
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

pub(super) fn apply_insert_field(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    field: &FieldKind,
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
    // v43 (D-01) — plugin placeholders insert a TAGGED RUN (display
    // text + identity), not a single marker char; separate lane.
    if let FieldKind::Placeholder { plugin, key, value } = field {
        return insert_placeholder_run(doc, story_idx, story_id, offset, plugin, key, value);
    }
    let marker = field
        .marker_char()
        .expect("non-placeholder FieldKind has a marker char");
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
            field: field.clone(),
        },
        // Undo removes the one marker char we inserted at `offset`.
        inverse: Operation::DeleteField {
            story_id: story_id.to_string(),
            offset,
            field: field.clone(),
        },
        invalidation,
    })
}

/// v43 (D-01) — insert a `FieldKind::Placeholder` as its own tagged
/// run at the story char `offset`, splitting the host run when the
/// offset falls inside one. The new run clones the surrounding run's
/// formatting (the field takes the character style at the insertion
/// point, like the page-number marker) and displays the cached value
/// (or the `<key>` token while unresolved).
fn insert_placeholder_run(
    doc: &mut Document,
    story_idx: usize,
    story_id: &str,
    offset: u32,
    plugin: &str,
    key: &str,
    value: &Option<String>,
) -> Result<AppliedOperation, OperationError> {
    let story = &mut doc.stories[story_idx].story;
    let tag = paged_parse::PlaceholderField {
        plugin: plugin.to_string(),
        key: key.to_string(),
        value: value.clone(),
    };
    let display = tag.display_text();

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

    // Locate the run hosting the offset (inclusive of its trailing
    // boundary — same convention as the marker-char lane).
    let mut char_cursor: u32 = 0;
    let mut target: Option<(usize, usize, usize)> = None;
    'outer: for (pi, para) in story.paragraphs.iter().enumerate() {
        for (ri, run) in para.runs.iter().enumerate() {
            let run_len = run.text.chars().count() as u32;
            if offset <= char_cursor + run_len {
                target = Some((pi, ri, (offset - char_cursor) as usize));
                break 'outer;
            }
            char_cursor += run_len;
        }
    }

    match target {
        Some((pi, ri, local)) => {
            let runs = &mut story.paragraphs[pi].runs;
            // Clone formatting from the host run; a clone of an
            // ordinary run carries no variable/placeholder tags of its
            // own to scrub except these two.
            let mut ph_run = runs[ri].clone();
            ph_run.text = display;
            ph_run.text_variable = None;
            ph_run.placeholder = Some(tag);
            let host_len = runs[ri].text.chars().count();
            if local == 0 {
                runs.insert(ri, ph_run);
            } else if local == host_len {
                runs.insert(ri + 1, ph_run);
            } else {
                // Split the host run around the insertion point.
                let byte = runs[ri]
                    .text
                    .char_indices()
                    .nth(local)
                    .map(|(b, _)| b)
                    .expect("local < host_len");
                let mut tail = runs[ri].clone();
                tail.text = runs[ri].text[byte..].to_string();
                runs[ri].text.truncate(byte);
                runs.insert(ri + 1, ph_run);
                runs.insert(ri + 2, tail);
            }
        }
        // Story with no runs at all: append a fresh run to the last
        // (or a new) paragraph.
        None => {
            if story.paragraphs.is_empty() {
                story.paragraphs.push(paged_parse::Paragraph::default());
            }
            let para = story.paragraphs.last_mut().expect("ensured above");
            para.runs.push(paged_parse::CharacterRun {
                text: display,
                placeholder: Some(tag),
                ..Default::default()
            });
        }
    }

    let field = FieldKind::Placeholder {
        plugin: plugin.to_string(),
        key: key.to_string(),
        value: value.clone(),
    };
    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::InsertField {
            story_id: story_id.to_string(),
            offset,
            field: field.clone(),
        },
        // Undo removes the whole tagged run starting at `offset`.
        inverse: Operation::DeleteField {
            story_id: story_id.to_string(),
            offset,
            field,
        },
        invalidation,
    })
}

pub(super) fn apply_delete_field(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    field: &FieldKind,
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
    if let FieldKind::Placeholder { plugin, key, .. } = field {
        return delete_placeholder_run(doc, story_idx, story_id, offset, plugin, key);
    }
    let marker = field
        .marker_char()
        .expect("non-placeholder FieldKind has a marker char");
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
            field: field.clone(),
        },
        inverse: Operation::InsertField {
            story_id: story_id.to_string(),
            offset,
            field: field.clone(),
        },
        invalidation,
    })
}

/// v43 (D-01) — remove the placeholder run with identity
/// `(plugin, key)` starting at the story char `offset`. The inverse
/// re-inserts with the run's CURRENT cached value (which may differ
/// from the op's `field.value` after `SetFieldValue` re-resolutions),
/// so delete-then-undo restores what was actually displayed.
fn delete_placeholder_run(
    doc: &mut Document,
    story_idx: usize,
    story_id: &str,
    offset: u32,
    plugin: &str,
    key: &str,
) -> Result<AppliedOperation, OperationError> {
    let story = &mut doc.stories[story_idx].story;
    let mut char_cursor: u32 = 0;
    let mut removed: Option<paged_parse::PlaceholderField> = None;
    'outer: for para in story.paragraphs.iter_mut() {
        let mut ri = 0;
        while ri < para.runs.len() {
            let run = &para.runs[ri];
            let run_len = run.text.chars().count() as u32;
            let is_match = char_cursor == offset
                && run
                    .placeholder
                    .as_ref()
                    .is_some_and(|tag| tag.plugin == plugin && tag.key == key);
            if is_match {
                removed = para.runs.remove(ri).placeholder;
                break 'outer;
            }
            if char_cursor > offset {
                break 'outer;
            }
            char_cursor += run_len;
            ri += 1;
        }
    }
    let Some(tag) = removed else {
        return Err(OperationError::InvalidValue {
            node: NodeId::StoryRange {
                story_id: story_id.to_string(),
                start: offset,
                end: offset,
            },
            path: PropertyPath::AppliedCharacterStyle,
            reason: format!("no placeholder field ({plugin}, {key}) starting at offset {offset}"),
        });
    };

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::DeleteField {
            story_id: story_id.to_string(),
            offset,
            field: FieldKind::Placeholder {
                plugin: plugin.to_string(),
                key: key.to_string(),
                value: tag.value.clone(),
            },
        },
        inverse: Operation::InsertField {
            story_id: story_id.to_string(),
            offset,
            field: FieldKind::Placeholder {
                plugin: tag.plugin,
                key: tag.key,
                value: tag.value,
            },
        },
        invalidation,
    })
}

/// v43 (D-01) — `Operation::SetFieldValue`: update the cached display
/// value of the placeholder run containing `offset`. One undoable
/// step; the inverse carries the prior value and the run-start offset
/// (re-applying the inverse re-finds the run regardless of how the
/// new display's length shifted downstream offsets).
pub(super) fn apply_set_field_value(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    value: Option<&str>,
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
    let story = &mut doc.stories[story_idx].story;

    let mut char_cursor: u32 = 0;
    let mut hit: Option<(u32, Option<String>)> = None;
    'outer: for para in story.paragraphs.iter_mut() {
        for run in para.runs.iter_mut() {
            let run_len = run.text.chars().count() as u32;
            if let Some(tag) = run.placeholder.as_mut() {
                // The placeholder owning `offset`: starting exactly
                // here (run-start addressing — what the enumerate door
                // reports, and matches even a zero-length empty-value
                // run) or strictly inside its display span.
                if offset == char_cursor || (offset > char_cursor && offset < char_cursor + run_len)
                {
                    let prior = tag.value.clone();
                    tag.value = value.map(|v| v.to_string());
                    run.text = tag.display_text();
                    hit = Some((char_cursor, prior));
                    break 'outer;
                }
            } else if offset < char_cursor + run_len {
                // Strictly inside ordinary text — no field here. (An
                // offset AT a run boundary falls through to the next
                // run, so a placeholder starting right after plain
                // text is addressable by its start.)
                break 'outer;
            }
            char_cursor += run_len;
        }
    }
    let Some((run_start, prior)) = hit else {
        return Err(OperationError::InvalidValue {
            node: NodeId::StoryRange {
                story_id: story_id.to_string(),
                start: offset,
                end: offset,
            },
            path: PropertyPath::AppliedCharacterStyle,
            reason: format!("no placeholder field at offset {offset}"),
        });
    };

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::SetFieldValue {
            story_id: story_id.to_string(),
            offset: run_start,
            value: value.map(|v| v.to_string()),
        },
        inverse: Operation::SetFieldValue {
            story_id: story_id.to_string(),
            offset: run_start,
            value: prior,
        },
        invalidation,
    })
}
