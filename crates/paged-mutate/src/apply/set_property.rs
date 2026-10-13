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
use crate::invert::invert_set_property;
use crate::operation::{
    AppliedOperation, GradientFeatherSpec, InvalidationHint, NodeId, Operation, PropertyPath, Value,
};

// ---------------------------------------------------------------------------
// SetProperty
// ---------------------------------------------------------------------------

pub(super) fn apply_set_property(
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
        // Wave B — the inverse gesture of PathOpenAt: close an open
        // subpath (merge coincident endpoints back, or just stop
        // being open). Same kind fan-out + snapshot inverse.
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_),
            PropertyPath::ClosePath,
        ) => {
            return apply_close_path(doc, node, value);
        }
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_),
            PropertyPath::OutlineStroke
            | PropertyPath::OutlineStrokeVariable
            | PropertyPath::OffsetPath
            | PropertyPath::SimplifyPath,
        ) => {
            return apply_path_kernel_op(doc, node, &path, value);
        }
        // Plugin-metadata carrier — its inverse carries the prev
        // snapshot inside the same Value, so it short-circuits like
        // the Track J ops. All five leaf page-item kinds, and (v70)
        // pages: a slide's notes and transition travel with its page.
        (
            NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::GraphicLine(_)
            | NodeId::Oval(_)
            | NodeId::Page(_),
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
        // ---- B-22: fill/stroke/opacity for the path kinds --------
        // Polygon/Oval/GraphicLine carry the same style fields as
        // Rectangle but had only joined the transform + path-point
        // arms. A draw plugin that inserts a path (→ Polygon) could
        // not style it via SetProperty and had to abuse
        // setDocumentDefaults; these arms close that. GraphicLine is a
        // stroke-only line (no fill_color / opacity field).
        (NodeId::Polygon(id), PropertyPath::FrameFillColor) => {
            let new_color = expect_color_ref(path, value)?;
            let poly = find_polygon_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = poly.fill_color.clone();
            poly.fill_color = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Oval(id), PropertyPath::FrameFillColor) => {
            let new_color = expect_color_ref(path, value)?;
            let oval =
                find_oval_mut(doc, id).ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = oval.fill_color.clone();
            oval.fill_color = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // B-22 — the region Pathfinder verbs rewrite a carrier's whole
        // path, so its frame box has to follow or the selection chrome
        // and hit-test rectangle keep describing the pre-Divide shape.
        // TextFrame / Rectangle already had this arm; Polygon and
        // GraphicLine complete the four path-bearing kinds. Plain field
        // writes — nothing rescales the anchors.
        (NodeId::Polygon(id), PropertyPath::FrameBounds) => {
            let new_bounds = expect_bounds(path, value)?;
            let poly = find_polygon_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = bounds_to_array(poly.bounds);
            poly.bounds = bounds_from_array(new_bounds);
            (
                Value::Bounds(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::GraphicLine(id), PropertyPath::FrameBounds) => {
            let new_bounds = expect_bounds(path, value)?;
            let line = find_graphic_line_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = bounds_to_array(line.bounds);
            line.bounds = bounds_from_array(new_bounds);
            (
                Value::Bounds(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // Ovals carried every sibling style arm but not FrameBounds, so
        // the unrotated translate gesture (canonical commit op:
        // SetProperty { FrameBounds }) failed on any oval — found by the
        // 2026-08-17 corpus sweep once the editor surfaced the
        // GestureFailure::Other message. Plain field write like its
        // siblings — the elliptical outline derives from the box.
        (NodeId::Oval(id), PropertyPath::FrameBounds) => {
            let new_bounds = expect_bounds(path, value)?;
            let oval =
                find_oval_mut(doc, id).ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = bounds_to_array(oval.bounds);
            oval.bounds = bounds_from_array(new_bounds);
            (
                Value::Bounds(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Polygon(id), PropertyPath::FrameStrokeColor) => {
            let new_color = expect_color_ref(path, value)?;
            let poly = find_polygon_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = poly.stroke_color.clone();
            poly.stroke_color = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Oval(id), PropertyPath::FrameStrokeColor) => {
            let new_color = expect_color_ref(path, value)?;
            let oval =
                find_oval_mut(doc, id).ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = oval.stroke_color.clone();
            oval.stroke_color = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::GraphicLine(id), PropertyPath::FrameStrokeColor) => {
            let new_color = expect_color_ref(path, value)?;
            let line = find_graphic_line_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = line.stroke_color.clone();
            line.stroke_color = new_color;
            (
                Value::ColorRef(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Polygon(id), PropertyPath::FrameStrokeWeight) => {
            let new_weight = expect_length(path, value)?;
            let poly = find_polygon_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = poly.stroke_weight;
            poly.stroke_weight = new_weight;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Oval(id), PropertyPath::FrameStrokeWeight) => {
            let new_weight = expect_length(path, value)?;
            let oval =
                find_oval_mut(doc, id).ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = oval.stroke_weight;
            oval.stroke_weight = new_weight;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::GraphicLine(id), PropertyPath::FrameStrokeWeight) => {
            let new_weight = expect_length(path, value)?;
            let line = find_graphic_line_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = line.stroke_weight;
            line.stroke_weight = new_weight;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Polygon(id), PropertyPath::FrameOpacity) => {
            let new_opacity = expect_length(path, value)?;
            let poly = find_polygon_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = poly.opacity;
            poly.opacity = new_opacity;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Oval(id), PropertyPath::FrameOpacity) => {
            let new_opacity = expect_length(path, value)?;
            let oval =
                find_oval_mut(doc, id).ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = oval.opacity;
            oval.opacity = new_opacity;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // C-63 — a `<Group>`'s own `<BlendingSetting Opacity>`. The
        // value lives on `Group::transparency` (the group has no
        // outline to carry a per-item slot) and the renderer's
        // `group_pass` brackets the members' commands in one blend
        // group whenever it is below 100 — so the group fades as ONE
        // object, which is not what setting each member's opacity does
        // (overlapping members would show through each other).
        (NodeId::Group(id), PropertyPath::FrameOpacity) => {
            let new_opacity = expect_length(path, value)?;
            let group = find_group_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = group.transparency.opacity;
            group.transparency.opacity = new_opacity;
            (Value::Length(prev), frame_style_hint(node))
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
        // ---- ADR 131/132 — a page item's `Name` (`Spread::item_names`).
        (
            NodeId::TextFrame(id)
            | NodeId::Rectangle(id)
            | NodeId::Oval(id)
            | NodeId::Polygon(id)
            | NodeId::GraphicLine(id)
            | NodeId::Group(id),
            PropertyPath::ItemName,
        ) => {
            let new_val = expect_text(path, value)?;
            let spread = doc
                .spreads
                .iter_mut()
                .map(|s| &mut s.spread)
                .find(|s| spread_holds_item(s, id))
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = if new_val.is_empty() {
                spread.item_names.remove(id.as_str())
            } else {
                spread.item_names.insert(id.clone(), new_val)
            };
            // A name is an identity, not paint.
            (
                Value::Text(prev.unwrap_or_default()),
                InvalidationHint::default(),
            )
        }
        // ---- ADR 132 — page margins and columns (`<MarginPreference>`).
        // A page with no margins of its own takes a fresh entry on the
        // first write; `null` (Length(None)) drops the page's entry, which
        // is also the inverse of that first write, so undo is exact.
        (
            NodeId::Page(id),
            PropertyPath::PageMarginTop
            | PropertyPath::PageMarginBottom
            | PropertyPath::PageMarginLeft
            | PropertyPath::PageMarginRight
            | PropertyPath::PageColumnCount
            | PropertyPath::PageColumnGutter,
        ) => {
            let new_val = expect_length(path, value)?;
            let margins = find_page_margins_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let hint = InvalidationHint {
                structural: true,
                ..Default::default()
            };
            let Some(new_val) = new_val else {
                // Undo puts the WHOLE entry back, field by field (the
                // first write recreates it), so a cleared page is exact.
                let inverse = match margins.remove(id.as_str()) {
                    Some(m) => Operation::Batch {
                        ops: PAGE_MARGIN_PATHS
                            .iter()
                            .map(|p| Operation::SetProperty {
                                node: node.clone(),
                                path: *p,
                                value: Value::Length(Some(page_margin_field(&m, *p))),
                            })
                            .collect(),
                    },
                    None => Operation::Batch { ops: Vec::new() },
                };
                return Ok(AppliedOperation {
                    op: Operation::SetProperty {
                        node: node.clone(),
                        path,
                        value: value.clone(),
                    },
                    inverse,
                    invalidation: hint,
                });
            };
            if matches!(path, PropertyPath::PageColumnCount)
                && (new_val < 1.0 || new_val.fract() != 0.0)
            {
                return Err(OperationError::InvalidValue {
                    node: node.clone(),
                    path,
                    reason: format!("a column count is a whole number >= 1, not {new_val}"),
                });
            }
            if new_val < 0.0 {
                return Err(OperationError::InvalidValue {
                    node: node.clone(),
                    path,
                    reason: format!("{new_val} is negative"),
                });
            }
            let prev = match margins.get_mut(id.as_str()) {
                Some(m) => {
                    let prev = page_margin_field(m, path);
                    set_page_margin_field(m, path, new_val);
                    Some(prev)
                }
                None => {
                    let mut m = paged_model::MarginPreference {
                        column_count: 1,
                        ..Default::default()
                    };
                    set_page_margin_field(&mut m, path, new_val);
                    margins.insert(id.clone(), m);
                    None
                }
            };
            (Value::Length(prev), hint)
        }
        // ---- ADR 132 — document setup: bleed and slug offsets.
        (
            NodeId::Document(_),
            PropertyPath::DocumentBleedTop
            | PropertyPath::DocumentBleedBottom
            | PropertyPath::DocumentBleedInsideOrLeft
            | PropertyPath::DocumentBleedOutsideOrRight
            | PropertyPath::DocumentSlugTop
            | PropertyPath::DocumentSlugBottom
            | PropertyPath::DocumentSlugInsideOrLeft
            | PropertyPath::DocumentSlugRightOrOutside,
        ) => {
            let new_val = expect_length(path, value)?.unwrap_or(0.0);
            if new_val < 0.0 {
                return Err(OperationError::InvalidValue {
                    node: node.clone(),
                    path,
                    reason: format!("{new_val} is negative"),
                });
            }
            let pref = &mut doc.designmap.document_preference;
            let slot = match path {
                PropertyPath::DocumentBleedTop => &mut pref.bleed_top,
                PropertyPath::DocumentBleedBottom => &mut pref.bleed_bottom,
                PropertyPath::DocumentBleedInsideOrLeft => &mut pref.bleed_inside_or_left,
                PropertyPath::DocumentBleedOutsideOrRight => &mut pref.bleed_outside_or_right,
                PropertyPath::DocumentSlugTop => &mut pref.slug_top,
                PropertyPath::DocumentSlugBottom => &mut pref.slug_bottom,
                PropertyPath::DocumentSlugInsideOrLeft => &mut pref.slug_inside_or_left,
                _ => &mut pref.slug_right_or_outside,
            };
            let prev = std::mem::replace(slot, new_val);
            // Bleed and slug are export geometry; nothing on the page moves.
            (Value::Length(Some(prev)), InvalidationHint::default())
        }
        // ---- ADR 132 — text variable definitions.
        (
            NodeId::TextVariable(id),
            PropertyPath::TextVariableName
            | PropertyPath::TextVariableType
            | PropertyPath::TextVariableContents
            | PropertyPath::TextVariableTextBefore
            | PropertyPath::TextVariableTextAfter
            | PropertyPath::TextVariableDateFormat,
        ) => {
            let new_val = expect_text(path, value)?;
            let var = doc
                .designmap
                .text_variables
                .iter_mut()
                .find(|v| v.self_id == *id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::TextVariableName => &mut var.name,
                PropertyPath::TextVariableType => &mut var.variable_type,
                PropertyPath::TextVariableContents => &mut var.contents,
                PropertyPath::TextVariableTextBefore => &mut var.text_before,
                PropertyPath::TextVariableTextAfter => &mut var.text_after,
                _ => &mut var.date_format,
            };
            let prev = slot.clone().unwrap_or_default();
            *slot = (!new_val.is_empty()).then_some(new_val);
            // v71 — re-bake every instance's ResultText, so the runs (and
            // an IDML save of them) carry the value the renderer shows.
            let display = super::path_topology::text_variable_display(var);
            for run in doc
                .stories
                .iter_mut()
                .flat_map(|s| s.story.paragraphs.iter_mut())
                .flat_map(|p| p.runs.iter_mut())
                .filter(|r| r.text_variable.as_deref() == Some(id.as_str()))
            {
                run.text.clone_from(&display);
            }
            (
                Value::Text(prev),
                // Every instance re-resolves its text from the definition.
                InvalidationHint {
                    structural: true,
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
        // Rectangle carried the field from the start; C-62 gave it to
        // Polygon (a pen path's two ends), GraphicLine and Oval (dash
        // ends), which the model had read only on `<Rectangle>` — so
        // until then a pen path could not take a cap at all. TextFrame
        // has no `end_cap` (its stroke is the frame box) and falls
        // through to UnsupportedProperty.
        (
            NodeId::Rectangle(_) | NodeId::Oval(_) | NodeId::Polygon(_) | NodeId::GraphicLine(_),
            PropertyPath::FrameStrokeEndCap,
        ) => {
            let new_val = expect_text(path, value)?;
            let slot = find_end_cap_mut(doc, node)
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
        // ---- v43 batch — stroke line ends (arrowheads) -------------
        // The kinds that carry `LeftLineEnd` / `RightLineEnd`: the
        // `<GraphicLine>` (v43), and since C-62 the `<Polygon>` — a pen
        // or pencil path is a polygon with an OPEN contour, and InDesign
        // draws line ends on open contours (v43 assumed IDML serialises
        // every open path as a `<GraphicLine>`; it does not). The wire
        // token is the IDML `ArrowHead` enumeration name; empty string
        // clears (= `"None"`). Unknown tokens are REJECTED rather than
        // stored as `ArrowheadType::Other` — `Other` has no faithful
        // `as_idml` spelling, so accepting it would corrupt the
        // inverse. A prior `Other` (out-of-vocabulary source token,
        // unreachable from real InDesign exports) inverts to clear.
        (
            NodeId::GraphicLine(_) | NodeId::Polygon(_),
            PropertyPath::FrameStrokeStartArrowhead | PropertyPath::FrameStrokeEndArrowhead,
        ) => {
            let new_val = expect_text(path, value)?;
            let parsed = if new_val.is_empty() {
                paged_model::ArrowheadType::None
            } else {
                match paged_model::ArrowheadType::from_idml(&new_val) {
                    paged_model::ArrowheadType::Other => {
                        return Err(OperationError::InvalidValue {
                            node: node.clone(),
                            path,
                            reason: format!("unknown line-end token {new_val:?}"),
                        });
                    }
                    t => t,
                }
            };
            let (start, end) = find_line_ends_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = if matches!(path, PropertyPath::FrameStrokeStartArrowhead) {
                start
            } else {
                end
            };
            let prev = match *slot {
                paged_model::ArrowheadType::None => String::new(),
                t => t.as_idml().to_string(),
            };
            *slot = parsed;
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
            // W2.5 — preserve any contour-option knobs through a mode set.
            let prev_contour = tw.and_then(|t| t.contour_type);
            let prev_inside = tw.and_then(|t| t.include_inside_edges);
            if new_val.is_empty() {
                *tw = None;
            } else {
                *tw = Some(paged_model::TextWrap {
                    mode: paged_model::TextWrapMode::from_idml(&new_val),
                    offsets: prev_offsets,
                    invert: prev_invert,
                    contour_type: prev_contour,
                    include_inside_edges: prev_inside,
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
                .unwrap_or(paged_model::TextWrapMode::None);
            let prev_offsets = tw.map(|t| t.offsets).unwrap_or([0.0; 4]);
            let prev_invert = tw.and_then(|t| t.invert);
            // W2.5 — preserve contour-option knobs through an offset set.
            let prev_contour = tw.and_then(|t| t.contour_type);
            let prev_inside = tw.and_then(|t| t.include_inside_edges);
            *tw = Some(paged_model::TextWrap {
                mode: prev_mode,
                offsets: new_offsets,
                invert: prev_invert,
                contour_type: prev_contour,
                include_inside_edges: prev_inside,
            });
            (
                Value::Bounds(prev_offsets),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // ---- W2.5 — text-wrap contour options -------------------
        // `<ContourOption ContourType / IncludeInsideEdges>` for
        // `ContourTextWrap`. Each writes one field of the TextWrap,
        // preserving the rest; a prior-None `text_wrap` materialises a
        // default wrap (mode None, zero offsets) so partial writes don't
        // drop information. The wrap exclusion can change other frames'
        // layout, so both carry a structural rebuild.
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameTextWrapContourType,
        ) => {
            let new_val = expect_text(path, value)?;
            let tw = find_text_wrap_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = tw
                .and_then(|t| t.contour_type)
                .map(|c| c.as_idml().to_string())
                .unwrap_or_default();
            let prev_mode = tw
                .map(|t| t.mode)
                .unwrap_or(paged_model::TextWrapMode::None);
            let prev_offsets = tw.map(|t| t.offsets).unwrap_or([0.0; 4]);
            let prev_invert = tw.and_then(|t| t.invert);
            let prev_inside = tw.and_then(|t| t.include_inside_edges);
            let new_contour = if new_val.is_empty() {
                None
            } else {
                Some(paged_model::ContourOptionType::from_idml(&new_val))
            };
            *tw = Some(paged_model::TextWrap {
                mode: prev_mode,
                offsets: prev_offsets,
                invert: prev_invert,
                contour_type: new_contour,
                include_inside_edges: prev_inside,
            });
            (
                Value::Text(prev),
                InvalidationHint {
                    structural: true,
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
            PropertyPath::FrameTextWrapContourIncludeInside,
        ) => {
            let new_val = expect_bool(path, value)?;
            let tw = find_text_wrap_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = tw.and_then(|t| t.include_inside_edges).unwrap_or(false);
            let prev_mode = tw
                .map(|t| t.mode)
                .unwrap_or(paged_model::TextWrapMode::None);
            let prev_offsets = tw.map(|t| t.offsets).unwrap_or([0.0; 4]);
            let prev_invert = tw.and_then(|t| t.invert);
            let prev_contour = tw.and_then(|t| t.contour_type);
            *tw = Some(paged_model::TextWrap {
                mode: prev_mode,
                offsets: prev_offsets,
                invert: prev_invert,
                contour_type: prev_contour,
                include_inside_edges: Some(new_val),
            });
            (
                Value::Bool(prev),
                InvalidationHint {
                    structural: true,
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
        // ---- W2.5 — element-level visible / locked --------------
        // Kind-agnostic over the five page-item variants that carry
        // `CommonAttrs` (TextFrame / Rectangle / Oval / GraphicLine /
        // Polygon). `ElementVisible="false"` hides the item from the
        // render (structural rebuild); `ElementLocked` is paint-neutral
        // (the renderer ignores it — the canvas hit-tester gates
        // selection on it), so it carries an empty hint. Both round-trip
        // bytewise (plain `bool` parse fields).
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::GraphicLine(_)
            | NodeId::Polygon(_),
            PropertyPath::ElementVisible,
        ) => {
            let new_val = expect_bool(path, value)?;
            let slot = find_element_bool_mut(doc, node, ElementBoolField::Visible)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = *slot;
            *slot = new_val;
            (
                Value::Bool(prev),
                InvalidationHint {
                    structural: true,
                    ..Default::default()
                },
            )
        }
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::GraphicLine(_)
            | NodeId::Polygon(_),
            PropertyPath::ElementLocked,
        ) => {
            let new_val = expect_bool(path, value)?;
            let slot = find_element_bool_mut(doc, node, ElementBoolField::Locked)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = *slot;
            *slot = new_val;
            (Value::Bool(prev), InvalidationHint::default())
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
        // C-20 — the derived path kinds join the tint lane. paged.draw's
        // appearance bake stacks N Polygons over one geometry, one paint
        // each, so a per-layer tint has to land on a Polygon. `Oval`
        // carries the same field; `GraphicLine` genuinely has no
        // `fill_tint` slot (lines carry no fill) so it keeps rejecting.
        (NodeId::Polygon(id), PropertyPath::FrameFillTint) => {
            let new_val = expect_length(path, value)?;
            let poly = find_polygon_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = poly.fill_tint;
            poly.fill_tint = new_val;
            (
                Value::Length(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Oval(id), PropertyPath::FrameFillTint) => {
            let new_val = expect_length(path, value)?;
            let oval =
                find_oval_mut(doc, id).ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = oval.fill_tint;
            oval.fill_tint = new_val;
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
            NodeId::TextFrame(_) | NodeId::Rectangle(_) | NodeId::Oval(_) | NodeId::Polygon(_),
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
        // mutates the named field. Every shape kind is wired (a
        // Group falls through to UnsupportedProperty).
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameDropShadowMode,
        ) => {
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
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameDropShadowXOffset,
        ) => {
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
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameDropShadowYOffset,
        ) => {
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
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameDropShadowSize,
        ) => {
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
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameDropShadowOpacity,
        ) => {
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
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameDropShadowColor,
        ) => {
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
        // Every shape carries `drop_shadow: Option<...>`.
        // Toggle semantics: true → default DropShadowSetting when
        // prior was None (preserves existing custom shadow);
        // false → clear.
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameDropShadow,
        ) => {
            let new_val = expect_bool(path, value)?;
            let slot = find_drop_shadow_slot_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = slot.is_some();
            *slot = if new_val {
                slot.clone().or_else(|| Some(default_drop_shadow()))
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
        // Rectangle / Oval / Polygon / GraphicLine (the kinds
        // `find_frame_effects_mut` reaches — C-63 added Polygon, ADR 132
        // the line, whose effects are cast by its stroke's outline band);
        // a Group falls through to UnsupportedProperty.

        // -- Inner shadow ------------------------------------------
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
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
        // C-20 — Polygon / Oval join here for the same reason as the
        // tint arms above (per-layer blend on a baked appearance
        // stack). `GraphicLine` has no `blend_mode` field on the parse
        // struct, so it stays out and rejects honestly.
        // C-63 — `Group` joins: its slot is
        // `Group::transparency.blend_mode`, which `group_pass` already
        // composites the bracketed members with.
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Polygon(_)
            | NodeId::Oval(_)
            | NodeId::GraphicLine(_)
            | NodeId::Group(_),
            PropertyPath::FrameBlendMode,
        ) => {
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
        // ---- R-10 — a line's object opacity (it had none before) ----
        (NodeId::GraphicLine(id), PropertyPath::FrameOpacity) => {
            let new_opacity = expect_length(path, value)?;
            let line = find_graphic_line_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = line.opacity;
            line.opacity = new_opacity;
            (Value::Length(prev), frame_style_hint(node))
        }
        // ---- R-10 — a transparency target's opacity / blend mode ----
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameFillOpacity
            | PropertyPath::FrameStrokeOpacity
            | PropertyPath::FrameContentOpacity,
        ) => {
            let new_val = expect_length(path, value)?;
            let target = match path {
                PropertyPath::FrameFillOpacity => TargetKind::Fill,
                PropertyPath::FrameStrokeOpacity => TargetKind::Stroke,
                _ => TargetKind::Content,
            };
            let slot = find_target_slot_mut(doc, node, target).ok_or_else(|| {
                OperationError::UnsupportedProperty {
                    node: node.clone(),
                    path,
                }
            })?;
            let t = slot.get_or_insert_with(Default::default);
            let prev = std::mem::replace(&mut t.opacity, new_val);
            drop_empty_target(slot);
            (Value::Length(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameFillBlendMode
            | PropertyPath::FrameStrokeBlendMode
            | PropertyPath::FrameContentBlendMode,
        ) => {
            let new_val = expect_text(path, value)?;
            let target = match path {
                PropertyPath::FrameFillBlendMode => TargetKind::Fill,
                PropertyPath::FrameStrokeBlendMode => TargetKind::Stroke,
                _ => TargetKind::Content,
            };
            let slot = find_target_slot_mut(doc, node, target).ok_or_else(|| {
                OperationError::UnsupportedProperty {
                    node: node.clone(),
                    path,
                }
            })?;
            let t = slot.get_or_insert_with(Default::default);
            let prev =
                std::mem::replace(&mut t.blend_mode, (!new_val.is_empty()).then_some(new_val));
            drop_empty_target(slot);
            (
                Value::Text(prev.unwrap_or_default()),
                frame_style_hint(node),
            )
        }
        // ---- R-10 — how a group's members composite ----------------
        (NodeId::Group(id), PropertyPath::GroupIsolateBlending | PropertyPath::GroupKnockout) => {
            let new_val = expect_bool(path, value)?;
            let group = find_group_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = if path == PropertyPath::GroupIsolateBlending {
                &mut group.transparency.isolate_blending
            } else {
                &mut group.transparency.knockout_group
            };
            let prev = std::mem::replace(slot, new_val);
            (Value::Bool(prev), frame_style_hint(node))
        }
        // ---- R-11 — the drop shadow's remaining attributes ----------
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameDropShadowBlendMode,
        ) => {
            let new_val = expect_text(path, value)?;
            let ds = find_drop_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev =
                std::mem::replace(&mut ds.blend_mode, (!new_val.is_empty()).then_some(new_val));
            (
                Value::Text(prev.unwrap_or_default()),
                frame_style_hint(node),
            )
        }
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameDropShadowSpread | PropertyPath::FrameDropShadowNoise,
        ) => {
            let new_val = expect_length(path, value)?;
            let ds = find_drop_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = if path == PropertyPath::FrameDropShadowSpread {
                &mut ds.spread_pct
            } else {
                &mut ds.noise_pct
            };
            let prev = std::mem::replace(slot, new_val);
            (Value::Length(prev), frame_style_hint(node))
        }
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameDropShadowKnockedOut | PropertyPath::FrameDropShadowUseGlobalLight,
        ) => {
            let new_val = expect_bool(path, value)?;
            let ds = find_drop_shadow_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let (slot, default) = if path == PropertyPath::FrameDropShadowKnockedOut {
                (&mut ds.knocked_out, true)
            } else {
                (&mut ds.use_global_light, false)
            };
            // InDesign's default is stored as "unset", so an undo that
            // writes it back restores the slot exactly.
            let prev =
                std::mem::replace(slot, (new_val != default).then_some(new_val)).unwrap_or(default);
            (Value::Bool(prev), frame_style_hint(node))
        }
        // ---- R-11 — the document's global light ---------------------
        (
            NodeId::Document(_),
            PropertyPath::DocumentGlobalLightAngle | PropertyPath::DocumentGlobalLightAltitude,
        ) => {
            let new_val = expect_length(path, value)?;
            let pref = &mut doc.designmap.transparency_preference;
            let slot = if path == PropertyPath::DocumentGlobalLightAngle {
                &mut pref.global_light_angle
            } else {
                &mut pref.global_light_altitude
            };
            let prev = std::mem::replace(slot, new_val);
            // Every shadow, inner shadow and bevel that follows the light
            // repaints: the whole document.
            (Value::Length(prev), InvalidationHint::default())
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
            rect.frame_fitting = Some(paged_model::FrameFittingOption {
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
                rect.frame_fitting = Some(paged_model::FrameFittingOption {
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
        // v66 — the same inner `<Image>` transform on the other two image
        // frames. The renderer has drawn `image_item_transform` for ovals
        // and polygons (clipped by their outline) and the IDML writer has
        // written it back all along; only the setter was missing, so a
        // commit that changed an image's size could not reach them.
        (NodeId::Oval(id), PropertyPath::ImageContentTransform) => {
            let new_transform = expect_transform(path, value)?;
            let oval =
                find_oval_mut(doc, id).ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = oval.image_item_transform;
            oval.image_item_transform = new_transform;
            (
                Value::Transform(prev),
                InvalidationHint {
                    frame_geometry: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (NodeId::Polygon(id), PropertyPath::ImageContentTransform) => {
            let new_transform = expect_transform(path, value)?;
            let poly = find_polygon_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = poly.image_item_transform;
            poly.image_item_transform = new_transform;
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
        // C-35 (v62) — move a page item to another LAYER. Same wire
        // shape as the object-style ref above (Value::Text carrying a
        // `<Layer>` self_id, empty string clears to the default
        // layer), and the same leaf-only addressing: a `<Group>` has
        // no `ItemLayer` in IDML, its members each carry their own.
        //
        // The invalidation is `structural`, not `frame_style`, and
        // that is the whole subtlety of this path. The renderer sorts
        // `frames_in_order` by `ItemLayer` BEFORE it paints (Q-10), so
        // changing one item's layer reorders the whole spread's paint
        // sequence and can change what occludes what several frames
        // away. Hinting `frame_style` here would repaint the item and
        // leave every other frame drawn in the old order.
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::ItemLayer,
        ) => {
            let new_val = expect_text(path, value)?;
            let field = find_item_layer_mut(doc, node)
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
                    structural: true,
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
            | PropertyPath::ParagraphHyphenationZone
            | PropertyPath::ParagraphKeepLinesTogether
            | PropertyPath::ParagraphKeepWithNext
            | PropertyPath::ParagraphKeepAllLinesTogether
            | PropertyPath::ParagraphKeepFirstLines
            | PropertyPath::ParagraphKeepLastLines
            | PropertyPath::ParagraphStartParagraph
            | PropertyPath::ParagraphSpanColumnType
            | PropertyPath::ParagraphSpanSplitColumnCount
            | PropertyPath::ParagraphSpanColumnMinSpaceBefore
            | PropertyPath::ParagraphSpanColumnMinSpaceAfter
            | PropertyPath::ParagraphSplitColumnInsideGutter
            | PropertyPath::ParagraphSplitColumnOutsideGutter
            | PropertyPath::ParagraphRuleAbove
            | PropertyPath::ParagraphRuleBelow
            | PropertyPath::ParagraphTabStops
            | PropertyPath::ParagraphListType
            | PropertyPath::ParagraphBulletCharacter
            | PropertyPath::ParagraphNumberingFormat
            | PropertyPath::ParagraphBulletsTextAfter
            | PropertyPath::ParagraphNumberingExpression
            | PropertyPath::ParagraphNumberingStartAt
            | PropertyPath::ParagraphNumberingContinue
            | PropertyPath::ParagraphBulletsCharacterStyle
            | PropertyPath::ParagraphNumberingCharacterStyle
            | PropertyPath::ParagraphComposer
            | PropertyPath::ParagraphAppliedNumberingList,
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
                paged_model::VerticalJustification::from_idml(&new_val)
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
                paged_model::AutoSizingType::from_idml(&new_val)
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
                paged_model::FirstBaselineOffset::from_idml(&new_val)
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
                .unwrap_or(paged_model::TextWrapMode::None);
            let prev_offsets = tw.map(|t| t.offsets).unwrap_or([0.0; 4]);
            // W2.5 — preserve contour-option knobs through an invert set.
            let prev_contour = tw.and_then(|t| t.contour_type);
            let prev_inside = tw.and_then(|t| t.include_inside_edges);
            *tw = Some(paged_model::TextWrap {
                mode: prev_mode,
                offsets: prev_offsets,
                invert: Some(new_val),
                contour_type: prev_contour,
                include_inside_edges: prev_inside,
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
        // W1.1 — per-frame `StrokeDashAndGap` dash override. Whole-list
        // replacement (the `TabStops` precedent): the empty vec CLEARS
        // the per-frame override. The inverse carries the prior list so
        // undo (incl. clear-then-restore) round-trips bytewise.
        (
            NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::Polygon(_)
            | NodeId::GraphicLine(_),
            PropertyPath::FrameStrokeDashArray,
        ) => {
            let Value::Lengths(new_dash) = value else {
                return Err(OperationError::TypeMismatch {
                    path,
                    expected: "Lengths".to_string(),
                });
            };
            let slot = find_stroke_dash_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = std::mem::replace(slot, new_dash.clone());
            (
                Value::Lengths(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        // Join / miter ride v35 across all closed-path kinds
        // (punch-list); stroke alignment follows below.
        (
            NodeId::Rectangle(_) | NodeId::Polygon(_) | NodeId::GraphicLine(_),
            PropertyPath::FrameStrokeJoin,
        ) => {
            let new_val = expect_text(path, value)?;
            let slot = find_end_join_mut(doc, node)
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
        // C-24 — every kind whose renderer offsets its outline by the
        // alignment takes the write: Rectangle (the original arm),
        // Polygon and Oval (W1.5 offsets their closed contours by
        // ±weight/2) and TextFrame (same offset, and the stroke's share
        // also insets the TEXT, so that one reflows). The model has
        // carried the field on all four and the importer has filled it;
        // only the Rectangle could be written. A `GraphicLine` has no
        // such field — an open stroke has no inside — and keeps
        // rejecting.
        (
            NodeId::Rectangle(_) | NodeId::Polygon(_) | NodeId::Oval(_) | NodeId::TextFrame(_),
            PropertyPath::FrameStrokeAlignment,
        ) => {
            let new_val = expect_text(path, value)?;
            let slot = find_stroke_alignment_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = slot.clone().unwrap_or_default();
            *slot = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            let text_reflow = if matches!(node, NodeId::TextFrame(_)) {
                vec![node.clone()]
            } else {
                Vec::new()
            };
            (
                Value::Text(prev),
                InvalidationHint {
                    frame_style: vec![node.clone()],
                    text_reflow,
                    ..Default::default()
                },
            )
        }
        (
            NodeId::Rectangle(_) | NodeId::Polygon(_) | NodeId::GraphicLine(_),
            PropertyPath::FrameStrokeMiterLimit,
        ) => {
            let new_val = expect_length(path, value)?;
            let slot = find_miter_limit_mut(doc, node)
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

        // ====== W0.3 — per-corner option + radius (every kind) ========
        // B-23 closed Rectangle + Polygon; C-18 closed the rest
        // (2026-08-04). The full lockstep sequence shipped for all six:
        // plugin-publish's `idml-import` reads the corner vocabulary off
        // every page-item element (one shared `read_corner_attrs`) and
        // its `idml-export` patches it back byte-preservingly (a
        // corner-ONLY patch lane for `<Group>`, which is not a
        // `VectorItem`); the model structs grew `corner_radius` /
        // `corner_option` / `corners` (serde-defaulted); and these arms
        // write every kind through `find_corners_mut`.
        //
        // Evidence, not symmetry: across the 61-file real-export corpus
        // every kind carries genuinely NON-default values —
        // `CornerRadius` on 392 rectangles / 228 polygons / 37 groups /
        // 21 lines / 17 text frames / 16 ovals, and a corner write that
        // answered `UnsupportedProperty` was silently unsaveable.
        //
        // Where the values shape GEOMETRY is a renderer question and
        // differs per kind; each model struct documents its own answer.
        // Rectangle and TextFrame address all four names exactly (their
        // bodies are the same outline). Polygon takes a uniform effect
        // from slot 0. Oval / GraphicLine / Group store and round-trip
        // without rendering — an ellipse has no corner, an open
        // stroke-only line has no enclosed corner, and a group has no
        // outline of its own.
        //
        // NAMED RESIDUALS (deliberate, evidence-backed):
        //   * Only the TopLeft slot (falling back to the global
        //     `CornerOption`/`CornerRadius` pair) drives polygon
        //     GEOMETRY; the other three are stored, mutable and
        //     round-tripped but rect-only as *addressing*. IDML has no
        //     per-name mapping for a polygon with N ≠ 4 corners and the
        //     real-export corpus never relies on one.
        //   * Corner effects apply to CLOSED contours with ≥ 3 anchors;
        //     an open contour (IDML `PathOpen="true"`) keeps its raw
        //     outline — matching InDesign, where a two-point open path
        //     inheriting `RoundedCorner` from an object style shows
        //     nothing. That is also why a `GraphicLine` never renders
        //     one.
        //   * A SMOOTH spline junction (non-degenerate Bezier handles)
        //     is never treated as a corner; the effect only cuts where
        //     two straight edges meet. That is also why an `Oval` never
        //     renders one — all four of an ellipse's anchors are smooth.
        //   * All five options (Rounded / Inverse / Bevel / Inset /
        //     Fancy) render on polygons because the shared emitter is
        //     kind-agnostic — but `Inset` and `Fancy` carry the same
        //     "approximation pending reference-PDF calibration" caveat
        //     they already carry for rectangles.
        //   * The object-style corner CASCADE
        //     (`module::object_style_cascade`) still fires on
        //     `Geometry::Rect` only, so a polygon / text frame takes its
        //     corners from its own attributes. No corpus document
        //     depends on the cascade: every `<ObjectStyle>` and the
        //     `<PageItemDefault>` in all 61 files spells every corner
        //     option `None`.
        (
            NodeId::Rectangle(_)
            | NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Oval(_)
            | NodeId::GraphicLine(_)
            | NodeId::Group(_),
            PropertyPath::FrameCornerOptionTopLeft
            | PropertyPath::FrameCornerOptionTopRight
            | PropertyPath::FrameCornerOptionBottomLeft
            | PropertyPath::FrameCornerOptionBottomRight,
        ) => {
            let new_val = expect_text(path, value)?;
            let i = corner_index(path);
            let corners = find_corners_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = corners[i]
                .option
                .map(corner_option_as_idml)
                .unwrap_or_default();
            corners[i].option = if new_val.is_empty() {
                None
            } else {
                paged_model::CornerOption::from_idml(&new_val)
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
            NodeId::Rectangle(_)
            | NodeId::Polygon(_)
            | NodeId::TextFrame(_)
            | NodeId::Oval(_)
            | NodeId::GraphicLine(_)
            | NodeId::Group(_),
            PropertyPath::FrameCornerRadiusTopLeft
            | PropertyPath::FrameCornerRadiusTopRight
            | PropertyPath::FrameCornerRadiusBottomLeft
            | PropertyPath::FrameCornerRadiusBottomRight,
        ) => {
            let new_val = expect_length(path, value)?;
            let i = corner_index(path);
            let corners = find_corners_mut(doc, node)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let prev = corners[i].radius;
            corners[i].radius = new_val;
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

        // ---- W1.16 — anchored-object settings -------------------
        // Kind-agnostic over the page-item NodeId variants: the
        // anchored frame is addressed by its own `Self` id regardless
        // of whether it is an anchored TextFrame / Rectangle / Group.
        // The setting lives in the stories, so `find_anchored_setting_mut`
        // scans there (materialising a default block on first write).
        // All ten share the `text_reflow` invalidation — moving an
        // anchored object reflows its host line.
        (
            NodeId::TextFrame(id)
            | NodeId::Rectangle(id)
            | NodeId::Group(id)
            | NodeId::Polygon(id)
            | NodeId::Oval(id)
            | NodeId::GraphicLine(id),
            PropertyPath::AnchoredPosition
            | PropertyPath::AnchorPoint
            | PropertyPath::AnchoredHorizontalReference
            | PropertyPath::AnchoredVerticalReference
            | PropertyPath::AnchoredHorizontalAlignment
            | PropertyPath::AnchoredVerticalAlignment,
        ) => {
            let new_val = expect_text(path, value)?;
            let setting = find_anchored_setting_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            // The empty string clears the override (back to the
            // cascaded default) for every Option<String> field.
            let next = if new_val.is_empty() {
                None
            } else {
                Some(new_val.clone())
            };
            let slot = match path {
                PropertyPath::AnchoredPosition => &mut setting.anchored_position,
                PropertyPath::AnchorPoint => &mut setting.anchor_point,
                PropertyPath::AnchoredHorizontalReference => {
                    &mut setting.horizontal_reference_point
                }
                PropertyPath::AnchoredVerticalReference => &mut setting.vertical_reference_point,
                PropertyPath::AnchoredHorizontalAlignment => &mut setting.horizontal_alignment,
                PropertyPath::AnchoredVerticalAlignment => &mut setting.vertical_alignment,
                _ => unreachable!("guarded by the outer match"),
            };
            let prev = std::mem::replace(slot, next);
            (
                // A `None` prior round-trips as the empty string (the
                // clear sentinel) — symmetric with the forward op.
                Value::Text(prev.unwrap_or_default()),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (
            NodeId::TextFrame(id)
            | NodeId::Rectangle(id)
            | NodeId::Group(id)
            | NodeId::Polygon(id)
            | NodeId::Oval(id)
            | NodeId::GraphicLine(id),
            PropertyPath::AnchoredXOffset | PropertyPath::AnchoredYOffset,
        ) => {
            // `Length(None)` resets the offset to 0 (IDML's default).
            let new_val = expect_length(path, value)?.unwrap_or(0.0);
            let setting = find_anchored_setting_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::AnchoredXOffset => &mut setting.anchor_x_offset,
                PropertyPath::AnchoredYOffset => &mut setting.anchor_y_offset,
                _ => unreachable!("guarded by the outer match"),
            };
            let prev = std::mem::replace(slot, new_val);
            (
                Value::Length(Some(prev)),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
                    ..Default::default()
                },
            )
        }
        (
            NodeId::TextFrame(id)
            | NodeId::Rectangle(id)
            | NodeId::Group(id)
            | NodeId::Polygon(id)
            | NodeId::Oval(id)
            | NodeId::GraphicLine(id),
            PropertyPath::AnchoredSpineRelative | PropertyPath::AnchoredLockPosition,
        ) => {
            let new_val = expect_bool(path, value)?;
            let setting = find_anchored_setting_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
            let slot = match path {
                PropertyPath::AnchoredSpineRelative => &mut setting.spine_relative,
                PropertyPath::AnchoredLockPosition => &mut setting.lock_position,
                _ => unreachable!("guarded by the outer match"),
            };
            let prev = std::mem::replace(slot, new_val);
            (
                Value::Bool(prev),
                InvalidationHint {
                    text_reflow: vec![node.clone()],
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

const PAGE_MARGIN_PATHS: [PropertyPath; 6] = [
    PropertyPath::PageMarginTop,
    PropertyPath::PageMarginBottom,
    PropertyPath::PageMarginLeft,
    PropertyPath::PageMarginRight,
    PropertyPath::PageColumnCount,
    PropertyPath::PageColumnGutter,
];

/// The `page_margins` side map of the spread that holds page `page_id`.
fn find_page_margins_mut<'a>(
    doc: &'a mut Document,
    page_id: &str,
) -> Option<&'a mut std::collections::HashMap<String, paged_model::MarginPreference>> {
    doc.spreads
        .iter_mut()
        .map(|s| &mut s.spread)
        .find(|s| {
            s.pages
                .iter()
                .any(|p| p.self_id.as_deref() == Some(page_id))
        })
        .map(|s| &mut s.page_margins)
}

fn page_margin_field(m: &paged_model::MarginPreference, path: PropertyPath) -> f32 {
    match path {
        PropertyPath::PageMarginTop => m.top,
        PropertyPath::PageMarginBottom => m.bottom,
        PropertyPath::PageMarginLeft => m.left,
        PropertyPath::PageMarginRight => m.right,
        PropertyPath::PageColumnCount => m.column_count as f32,
        _ => m.column_gutter,
    }
}

fn set_page_margin_field(m: &mut paged_model::MarginPreference, path: PropertyPath, v: f32) {
    match path {
        PropertyPath::PageMarginTop => m.top = v,
        PropertyPath::PageMarginBottom => m.bottom = v,
        PropertyPath::PageMarginLeft => m.left = v,
        PropertyPath::PageMarginRight => m.right = v,
        PropertyPath::PageColumnCount => m.column_count = v as u32,
        _ => m.column_gutter = v,
    }
}

/// True when `id` is one of `spread`'s page items (any of the six kinds).
fn spread_holds_item(spread: &paged_model::Spread, id: &str) -> bool {
    let is = |sid: &Option<String>| sid.as_deref() == Some(id);
    spread.text_frames.iter().any(|f| is(&f.self_id))
        || spread.rectangles.iter().any(|f| is(&f.self_id))
        || spread.ovals.iter().any(|f| is(&f.self_id))
        || spread.polygons.iter().any(|f| is(&f.self_id))
        || spread.graphic_lines.iter().any(|f| is(&f.self_id))
        || spread.groups.iter().any(|f| is(&f.self_id))
}

/// R-10 — a target with nothing left set is removed, so a cleared one
/// writes no block on save.
fn drop_empty_target(slot: &mut Option<paged_model::TargetTransparency>) {
    if slot
        .as_ref()
        .is_some_and(|t| t.opacity.is_none() && t.blend_mode.is_none() && t.effects.is_none())
    {
        *slot = None;
    }
}
