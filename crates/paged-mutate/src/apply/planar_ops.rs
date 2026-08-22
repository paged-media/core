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

//! B-22 — the region-level Pathfinder verbs and Shape Builder's
//! per-face materialize, both over one [`crate::planar`] arrangement.
//!
//! # Shape of the mutation
//!
//! Every verb reduces to the same three-part internal `Batch`, the one
//! `apply_pathfinder` established:
//!
//!   1. `SetProperty(input, ClosePath restore-branch)` for each input
//!      that carries a result — the `ClosePath` value's restore branch
//!      is the only door that writes the whole
//!      `(anchors, subpath_starts, subpath_open)` triple in one shot,
//!      and its inverse restores the prior triple verbatim. Where the
//!      kind supports it (Rectangle / TextFrame / Polygon /
//!      GraphicLine), a `FrameBounds` write keeps the frame box in step
//!      with the new outline so selection chrome and hit-testing follow.
//!   2. `InsertNode(Polygon | GraphicLine)` for every EXTRA result (a
//!      Divide of two shapes makes three regions out of two elements).
//!   3. `RemoveNode(input)` for inputs left holding nothing.
//!
//! `apply_batch` rolls the whole thing back if any child fails, and the
//! Batch's inverse is what the op reports — so ONE Cmd-Z restores every
//! original element, path and all.
//!
//! # Style inheritance
//!
//! `pathfinderBoolean` keeps the surviving element's own attributes
//! untouched and deletes the rest. The region verbs generalise that
//! rule: a result region is owned by the TOPMOST input covering it
//! (`min(signature)`, since `elements` arrives top-to-bottom), and it is
//! written onto that input's element when possible — so fill, stroke,
//! transform, effects, object style, plugin metadata, everything the
//! element carried, rides along. Only the surplus regions become fresh
//! `Polygon`s, and those can only carry what `NodeSpec::Polygon`
//! carries: fill, stroke, stroke weight, item transform. That asymmetry
//! is the documented limit — an extra face does not inherit its owner's
//! drop shadow.

use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{
    AppliedOperation, FaceSelectMode, NodeId, NodeSpec, Operation, PathAnchorSpec,
    PathfinderRegionVerb, PropertyPath, Value,
};
use crate::planar::{
    arrangement_edges, build_arrangement, union_faces, PlanarError, PlanarFace, MAX_PLANAR_INPUTS,
};

use super::apply_inner;
use super::helpers::spread_parent_id;

/// IDML's "no paint" swatch. Trim / Merge / Crop drop strokes (that is
/// Illustrator's documented behaviour for all three), and this is the
/// value that says so in the format — `is_none_swatch_id` in the
/// renderer already honours it.
const NONE_SWATCH: &str = "Swatch/None";

/// The stroke weight an Outline result takes when its source carried no
/// stroke of its own. Illustrator uses a hairline; 0.25 pt is the
/// hairline InDesign writes.
const HAIRLINE_PT: f32 = 0.25;

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// B-22 — `Operation::PathfinderRegion`.
pub(super) fn apply_pathfinder_region(
    doc: &mut Document,
    elements: &[NodeId],
    verb: PathfinderRegionVerb,
) -> Result<AppliedOperation, OperationError> {
    let inputs = gather(doc, elements)?;
    let paths: Vec<(Vec<paged_model::PathAnchor>, Vec<usize>)> =
        inputs.iter().map(|i| i.path.clone()).collect();

    let plan = match verb {
        PathfinderRegionVerb::Outline => outline_plan(&paths, &inputs)?,
        _ => region_plan(&paths, &inputs, verb)?,
    };
    materialize(
        doc,
        &inputs,
        plan,
        Operation::PathfinderRegion {
            elements: elements.to_vec(),
            verb,
        },
    )
}

/// B-22 — `Operation::PathfinderFaces` (Shape Builder).
pub(super) fn apply_pathfinder_faces(
    doc: &mut Document,
    elements: &[NodeId],
    faces: &[String],
    mode: FaceSelectMode,
) -> Result<AppliedOperation, OperationError> {
    let inputs = gather(doc, elements)?;
    let paths: Vec<(Vec<paged_model::PathAnchor>, Vec<usize>)> =
        inputs.iter().map(|i| i.path.clone()).collect();
    let arrangement = build_arrangement(&paths).map_err(|e| planar_err(&inputs[0].node, e))?;

    // Unknown ids are a caller bug worth surfacing — silently uniting a
    // subset would look like the engine dropped a click.
    for id in faces {
        if arrangement.face(id).is_none() {
            return Err(invalid(
                &inputs[0].node,
                format!("no face {id} in this arrangement"),
            ));
        }
    }
    let selected: Vec<&PlanarFace> = arrangement
        .faces
        .iter()
        .filter(|f| match mode {
            FaceSelectMode::Keep => faces.contains(&f.id),
            FaceSelectMode::Remove => !faces.contains(&f.id),
        })
        .collect();
    if selected.is_empty() {
        return Err(invalid(
            &inputs[0].node,
            "the face selection is empty; nothing to build".to_string(),
        ));
    }
    // The result belongs to the topmost input covering any selected
    // face — the same ownership rule the region verbs use.
    let owner = selected
        .iter()
        .filter_map(|f| f.signature.first().copied())
        .min()
        .unwrap_or(0);
    let (anchors, starts) = union_faces(&selected);
    if anchors.is_empty() {
        return Err(invalid(
            &inputs[0].node,
            "the selected faces united to an empty region".to_string(),
        ));
    }
    let open = vec![false; starts.len().max(1)];
    let plan = Plan {
        results: vec![ResultRegion {
            owner,
            anchors,
            subpath_starts: starts,
            subpath_open: open,
            kind: ResultKind::Region,
        }],
        reuse_inputs: true,
        drop_stroke: false,
    };
    materialize(
        doc,
        &inputs,
        plan,
        Operation::PathfinderFaces {
            elements: elements.to_vec(),
            faces: faces.to_vec(),
            mode,
        },
    )
}

// ---------------------------------------------------------------------------
// Planning — faces → result regions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResultKind {
    /// A closed filled region → a `Polygon` when it needs a new element.
    Region,
    /// An open stroked segment → a `GraphicLine`.
    Edge,
}

struct ResultRegion {
    owner: usize,
    anchors: Vec<paged_model::PathAnchor>,
    subpath_starts: Vec<usize>,
    subpath_open: Vec<bool>,
    kind: ResultKind,
}

struct Plan {
    results: Vec<ResultRegion>,
    /// Whether an input element may be rewritten in place as the
    /// carrier of one of its results. Outline says no: its results are
    /// stroked line segments, and turning a filled rectangle into one
    /// would leave the fill behind.
    reuse_inputs: bool,
    /// Illustrator's Trim / Merge / Crop all "remove any strokes".
    drop_stroke: bool,
}

/// Divide / Trim / Merge / Crop / MinusBack — all five read the same
/// face list and differ only in which faces survive and who owns them.
fn region_plan(
    paths: &[(Vec<paged_model::PathAnchor>, Vec<usize>)],
    inputs: &[ElementInfo],
    verb: PathfinderRegionVerb,
) -> Result<Plan, OperationError> {
    let arrangement = build_arrangement(paths).map_err(|e| planar_err(&inputs[0].node, e))?;
    let faces = &arrangement.faces;
    let n = inputs.len();

    // Ownership: the topmost input covering the face. `elements` is
    // top-to-bottom, so that is the smallest index in the signature.
    let owner_of = |f: &PlanarFace| f.signature.first().copied().unwrap_or(0);

    let mut results: Vec<ResultRegion> = Vec::new();
    match verb {
        PathfinderRegionVerb::Divide => {
            // Every face becomes its own object.
            for face in faces {
                results.push(region_from(owner_of(face), face));
            }
        }
        PathfinderRegionVerb::Trim => {
            // Each input keeps only what nothing above it covers.
            for i in 0..n {
                let mine: Vec<&PlanarFace> = faces.iter().filter(|f| owner_of(f) == i).collect();
                if let Some(region) = united(i, &mine) {
                    results.push(region);
                }
            }
        }
        PathfinderRegionVerb::Merge => {
            // Trim, then coalesce inputs that share a fill colour.
            // Groups are keyed by the fill ref (absent fill is its own
            // key) and ordered by their topmost member, so the merged
            // object lands on the element the user sees on top.
            let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::new();
            for (i, input) in inputs.iter().enumerate() {
                match groups.iter_mut().find(|(fill, _)| *fill == input.fill) {
                    Some((_, members)) => members.push(i),
                    None => groups.push((input.fill.clone(), vec![i])),
                }
            }
            for (_, members) in &groups {
                let mine: Vec<&PlanarFace> = faces
                    .iter()
                    .filter(|f| members.contains(&owner_of(f)))
                    .collect();
                let top = members.iter().copied().min().unwrap_or(0);
                if let Some(region) = united(top, &mine) {
                    results.push(region);
                }
            }
        }
        PathfinderRegionVerb::Crop => {
            // The topmost element is the cookie cutter: only what falls
            // inside it survives, coloured by whatever is beneath.
            // Faces covered ONLY by the cutter are outside every other
            // object and go away with it. (Illustrator's Crop consumes
            // the topmost path; that is the documented behaviour and
            // what makes Crop different from Intersect.)
            for face in faces {
                if !face.signature.contains(&0) || face.signature.len() < 2 {
                    continue;
                }
                let owner = face.signature[1];
                results.push(region_from(owner, face));
            }
        }
        PathfinderRegionVerb::MinusBack => {
            // The BACKMOST object minus everything in front of it: the
            // faces covered by it alone.
            let back = n - 1;
            let mine: Vec<&PlanarFace> = faces
                .iter()
                .filter(|f| f.signature.as_slice() == [back])
                .collect();
            if let Some(region) = united(back, &mine) {
                results.push(region);
            }
        }
        PathfinderRegionVerb::Outline => unreachable!("routed to outline_plan"),
    }

    Ok(Plan {
        results,
        reuse_inputs: true,
        drop_stroke: matches!(
            verb,
            PathfinderRegionVerb::Trim | PathfinderRegionVerb::Merge | PathfinderRegionVerb::Crop
        ),
    })
}

/// Outline — every arrangement EDGE becomes an open stroked segment
/// carrying its source's FILL as the stroke colour (that is what
/// "converts fills to strokes" means). No input is reused: an open
/// two-anchor segment is not something a filled rectangle should turn
/// into, so all inputs are removed and the segments come in fresh.
fn outline_plan(
    paths: &[(Vec<paged_model::PathAnchor>, Vec<usize>)],
    inputs: &[ElementInfo],
) -> Result<Plan, OperationError> {
    let edges = arrangement_edges(paths).map_err(|e| planar_err(&inputs[0].node, e))?;
    let results = edges
        .into_iter()
        .map(|(owner, anchors)| ResultRegion {
            owner,
            anchors,
            subpath_starts: vec![0],
            subpath_open: vec![true],
            kind: ResultKind::Edge,
        })
        .collect();
    Ok(Plan {
        results,
        reuse_inputs: false,
        drop_stroke: false,
    })
}

fn region_from(owner: usize, face: &PlanarFace) -> ResultRegion {
    ResultRegion {
        owner,
        subpath_open: vec![false; face.subpath_starts.len().max(1)],
        anchors: face.anchors.clone(),
        subpath_starts: face.subpath_starts.clone(),
        kind: ResultKind::Region,
    }
}

/// Union a set of faces into one result owned by `owner`. `None` when
/// the set is empty or unites to nothing (the input was fully hidden —
/// Trim deletes it, which is the point of the verb).
fn united(owner: usize, faces: &[&PlanarFace]) -> Option<ResultRegion> {
    if faces.is_empty() {
        return None;
    }
    let (anchors, subpath_starts) = union_faces(faces);
    if anchors.is_empty() {
        return None;
    }
    Some(ResultRegion {
        owner,
        subpath_open: vec![false; subpath_starts.len().max(1)],
        anchors,
        subpath_starts,
        kind: ResultKind::Region,
    })
}

// ---------------------------------------------------------------------------
// Materializing — result regions → an internal Batch
// ---------------------------------------------------------------------------

fn materialize(
    doc: &mut Document,
    inputs: &[ElementInfo],
    plan: Plan,
    recorded: Operation,
) -> Result<AppliedOperation, OperationError> {
    if plan.results.is_empty() {
        return Err(invalid(
            &inputs[0].node,
            "the operation resolved no regions; nothing to do".to_string(),
        ));
    }
    let spread = inputs[0].spread.clone();
    let (mut polygon_slot, mut line_slot) = spread_slots(doc, &spread);
    let mut mint_offset: u64 = 0;

    let mut carried = vec![false; inputs.len()];
    let mut children: Vec<Operation> = Vec::new();
    for result in &plan.results {
        let owner = &inputs[result.owner.min(inputs.len() - 1)];
        let reuse = plan.reuse_inputs && owner.writable && !carried[result.owner];
        if reuse {
            carried[result.owner] = true;
            children.push(Operation::SetProperty {
                node: owner.node.clone(),
                path: PropertyPath::ClosePath,
                value: Value::ClosePath {
                    subpath: None,
                    prev_anchors: Some(
                        result
                            .anchors
                            .iter()
                            .map(PathAnchorSpec::from_parse)
                            .collect(),
                    ),
                    prev_subpath_starts: Some(result.subpath_starts.clone()),
                    prev_subpath_open: Some(result.subpath_open.clone()),
                },
            });
            children.push(Operation::SetProperty {
                node: owner.node.clone(),
                path: PropertyPath::FrameBounds,
                value: Value::Bounds(bounds_of(&result.anchors)),
            });
            if plan.drop_stroke {
                children.push(Operation::SetProperty {
                    node: owner.node.clone(),
                    path: PropertyPath::FrameStrokeColor,
                    value: Value::ColorRef(Some(NONE_SWATCH.to_string())),
                });
            }
            continue;
        }
        let self_id = mint_page_item_id(doc, &mut mint_offset);
        let spec = match result.kind {
            ResultKind::Region => {
                let position = polygon_slot;
                polygon_slot += 1;
                (
                    position,
                    NodeSpec::Polygon {
                        self_id,
                        bounds: bounds_of(&result.anchors),
                        anchors: result
                            .anchors
                            .iter()
                            .map(PathAnchorSpec::from_parse)
                            .collect(),
                        subpath_starts: result.subpath_starts.clone(),
                        subpath_open: result.subpath_open.clone(),
                        fill_color: owner.fill.clone(),
                        stroke_color: if plan.drop_stroke {
                            Some(NONE_SWATCH.to_string())
                        } else {
                            owner.stroke.clone()
                        },
                        stroke_weight: owner.weight,
                        item_transform: owner.transform,
                    },
                )
            }
            ResultKind::Edge => {
                let position = line_slot;
                line_slot += 1;
                (
                    position,
                    NodeSpec::GraphicLine {
                        self_id,
                        bounds: bounds_of(&result.anchors),
                        anchors: result
                            .anchors
                            .iter()
                            .map(PathAnchorSpec::from_parse)
                            .collect(),
                        subpath_starts: result.subpath_starts.clone(),
                        subpath_open: result.subpath_open.clone(),
                        // "Converts fills to strokes": the segment is
                        // painted with the colour its source was FILLED
                        // with, falling back to the source's own stroke
                        // when it had no fill.
                        stroke_color: owner.fill.clone().or_else(|| owner.stroke.clone()),
                        stroke_weight: Some(owner.weight.unwrap_or(HAIRLINE_PT)),
                        item_transform: owner.transform,
                    },
                )
            }
        };
        children.push(Operation::InsertNode {
            parent: spread.clone(),
            position: spec.0,
            node: spec.1,
            z_slot: None,
        });
    }
    // Inputs that carry nothing go away. Removes come LAST so the
    // inserts above address stable positions.
    for (i, input) in inputs.iter().enumerate() {
        if !carried[i] {
            // RESIDUAL (pre-existing, not B-22's): `NodeSpec::Rectangle`
            // and `NodeSpec::TextFrame` carry no anchor table, so a
            // RemoveNode of a rectangle that had been given an explicit
            // path restores it PATHLESS on undo — the same hole
            // `pathfinderBoolean`'s Subtract already falls into for the
            // elements it deletes. Clearing the path FIRST closes it
            // here: the Batch inverse runs in reverse, so the element is
            // re-inserted and then handed its original triple back.
            if input.clears_path_before_remove {
                children.push(Operation::SetProperty {
                    node: input.node.clone(),
                    path: PropertyPath::ClosePath,
                    value: Value::ClosePath {
                        subpath: None,
                        prev_anchors: Some(Vec::new()),
                        prev_subpath_starts: Some(Vec::new()),
                        prev_subpath_open: Some(Vec::new()),
                    },
                });
            }
            children.push(Operation::RemoveNode {
                node: input.node.clone(),
            });
        }
    }

    let batch = Operation::Batch { ops: children };
    let applied = apply_inner(doc, &batch)?;
    Ok(AppliedOperation {
        op: recorded,
        inverse: applied.inverse,
        invalidation: applied.invalidation,
    })
}

// ---------------------------------------------------------------------------
// Reading the inputs
// ---------------------------------------------------------------------------

/// Everything the planner and the materializer need about one input,
/// read once up front (no borrow of `doc` survives).
struct ElementInfo {
    node: NodeId,
    path: (Vec<paged_model::PathAnchor>, Vec<usize>),
    spread: NodeId,
    fill: Option<String>,
    stroke: Option<String>,
    weight: Option<f32>,
    transform: Option<[f32; 6]>,
    /// Whether the kind can be rewritten in place — the four kinds
    /// `find_path_anchors_mut` serves. An `Oval` carries no anchor
    /// table, so it can only be replaced, never carried.
    writable: bool,
    /// Whether a delete of this kind needs its path cleared first for
    /// the inverse to be faithful (see the note at the removal site):
    /// true for the kinds whose `NodeSpec` drops the anchor table.
    clears_path_before_remove: bool,
}

fn gather(doc: &Document, elements: &[NodeId]) -> Result<Vec<ElementInfo>, OperationError> {
    if elements.is_empty() {
        return Err(OperationError::NodeNotFound(NodeId::Spread(String::new())));
    }
    if elements.len() > MAX_PLANAR_INPUTS {
        return Err(invalid(
            &elements[0],
            format!(
                "planar arrangement takes at most {MAX_PLANAR_INPUTS} inputs (got {})",
                elements.len()
            ),
        ));
    }
    for (i, a) in elements.iter().enumerate() {
        if elements.iter().skip(i + 1).any(|b| b == a) {
            return Err(invalid(a, "the same element was listed twice".to_string()));
        }
    }
    let mut out = Vec::with_capacity(elements.len());
    for node in elements {
        let info = locate(doc, node).ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;
        out.push(info);
    }
    // The arrangement is planar: every input has to live on one spread.
    let spread = out[0].spread.clone();
    if out.iter().any(|i| i.spread != spread) {
        return Err(invalid(
            &out[0].node,
            "all inputs must live on the same spread".to_string(),
        ));
    }
    Ok(out)
}

/// B-22 — one element's path in the RAW anchor space the planar kernel
/// works in, with the bounds-rectangle fallback a primitive frame needs.
/// Public (re-exported from [`crate::apply`]) so the canvas read door
/// can build an arrangement without re-implementing the per-kind
/// lookup — the door and the ops MUST see identical geometry or a
/// hovered face id would not address the face the verb produces.
pub fn element_path(
    doc: &Document,
    node: &NodeId,
) -> Option<(Vec<paged_model::PathAnchor>, Vec<usize>)> {
    locate(doc, node).map(|info| info.path)
}

fn locate(doc: &Document, node: &NodeId) -> Option<ElementInfo> {
    let raw = node.self_id();
    for parsed in &doc.spreads {
        let spread_id = spread_parent_id(parsed);
        let s = &parsed.spread;
        let found: Option<ElementInfo> = match node {
            NodeId::TextFrame(_) => s
                .text_frames
                .iter()
                .find(|f| f.self_id.as_deref() == Some(raw))
                .map(|f| ElementInfo {
                    node: node.clone(),
                    path: path_or_bounds(&f.anchors, &f.subpath_starts, f.bounds),
                    spread: spread_id.clone(),
                    fill: f.fill_color.clone(),
                    stroke: f.stroke_color.clone(),
                    weight: f.stroke_weight,
                    transform: f.item_transform,
                    writable: true,
                    clears_path_before_remove: true,
                }),
            NodeId::Rectangle(_) => s
                .rectangles
                .iter()
                .find(|f| f.self_id.as_deref() == Some(raw))
                .map(|f| ElementInfo {
                    node: node.clone(),
                    path: path_or_bounds(&f.anchors, &f.subpath_starts, f.bounds),
                    spread: spread_id.clone(),
                    fill: f.fill_color.clone(),
                    stroke: f.stroke_color.clone(),
                    weight: f.stroke_weight,
                    transform: f.item_transform,
                    writable: true,
                    clears_path_before_remove: true,
                }),
            NodeId::Oval(_) => s
                .ovals
                .iter()
                .find(|f| f.self_id.as_deref() == Some(raw))
                .map(|f| ElementInfo {
                    node: node.clone(),
                    // An Oval has no parsed anchor table; the same
                    // bounds-rectangle approximation `apply_pathfinder`
                    // uses stands in (a follow-up emits the proper
                    // four-arc ellipse).
                    path: path_or_bounds(&[], &[], f.bounds),
                    spread: spread_id.clone(),
                    fill: f.fill_color.clone(),
                    stroke: f.stroke_color.clone(),
                    weight: f.stroke_weight,
                    transform: f.item_transform,
                    writable: false,
                    clears_path_before_remove: false,
                }),
            NodeId::Polygon(_) => s
                .polygons
                .iter()
                .find(|f| f.self_id.as_deref() == Some(raw))
                .map(|f| ElementInfo {
                    node: node.clone(),
                    path: path_or_bounds(&f.anchors, &f.subpath_starts, f.bounds),
                    spread: spread_id.clone(),
                    fill: f.fill_color.clone(),
                    stroke: f.stroke_color.clone(),
                    weight: f.stroke_weight,
                    transform: f.item_transform,
                    writable: true,
                    clears_path_before_remove: false,
                }),
            NodeId::GraphicLine(_) => s
                .graphic_lines
                .iter()
                .find(|f| f.self_id.as_deref() == Some(raw))
                .map(|f| ElementInfo {
                    node: node.clone(),
                    path: path_or_bounds(&f.anchors, &f.subpath_starts, f.bounds),
                    spread: spread_id.clone(),
                    fill: None,
                    stroke: f.stroke_color.clone(),
                    weight: f.stroke_weight,
                    transform: f.item_transform,
                    writable: true,
                    clears_path_before_remove: false,
                }),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// A primitive frame (`GeometricBounds` only, no anchor table) still has
/// geometry — its bounds rectangle. Same fallback `apply_pathfinder`
/// takes, so the two Pathfinder rows agree on what a rectangle IS.
fn path_or_bounds(
    anchors: &[paged_model::PathAnchor],
    starts: &[usize],
    bounds: paged_model::Bounds,
) -> (Vec<paged_model::PathAnchor>, Vec<usize>) {
    if !anchors.is_empty() {
        return (anchors.to_vec(), starts.to_vec());
    }
    let corner = |x: f32, y: f32| paged_model::PathAnchor {
        anchor: (x, y),
        left: (x, y),
        right: (x, y),
    };
    (
        vec![
            corner(bounds.left, bounds.top),
            corner(bounds.right, bounds.top),
            corner(bounds.right, bounds.bottom),
            corner(bounds.left, bounds.bottom),
        ],
        vec![0],
    )
}

/// Current lengths of the spread's `polygons` / `graphic_lines` vectors
/// — the append positions the inserted results take.
fn spread_slots(doc: &Document, spread: &NodeId) -> (usize, usize) {
    for parsed in &doc.spreads {
        if spread_parent_id(parsed) == *spread {
            return (
                parsed.spread.polygons.len(),
                parsed.spread.graphic_lines.len(),
            );
        }
    }
    (0, 0)
}

/// Mint a document-unique `u<hex>` page-item id. `offset` counts ids
/// already minted in this translation pass — the whole Batch is built
/// against the UNMUTATED document, so without it every insert would
/// claim the same id (FINDING #6 in the canvas model).
fn mint_page_item_id(doc: &Document, offset: &mut u64) -> String {
    let base = super::layer::mint_group_id(doc);
    let n = base
        .strip_prefix('u')
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .unwrap_or(1);
    let id = format!("u{:x}", n + *offset);
    *offset += 1;
    id
}

fn bounds_of(anchors: &[paged_model::PathAnchor]) -> [f32; 4] {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for a in anchors {
        for (x, y) in [a.anchor, a.left, a.right] {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if !min_x.is_finite() {
        return [0.0, 0.0, 0.0, 0.0];
    }
    // `Bounds` order is [top, left, bottom, right].
    [min_y, min_x, max_y, max_x]
}

fn invalid(node: &NodeId, reason: String) -> OperationError {
    OperationError::InvalidValue {
        node: node.clone(),
        path: PropertyPath::FramePath,
        reason,
    }
}

fn planar_err(node: &NodeId, err: PlanarError) -> OperationError {
    invalid(node, err.to_string())
}
