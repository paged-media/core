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

//! B-22 — the planar arrangement kernel: N overlapping paths → the
//! distinct AREAS (faces of the planar map) they divide the plane into.
//!
//! The engine's hit-testing resolves ELEMENTS. Region-level tools —
//! Shape Builder, Live Paint, and the Pathfinder verbs Divide / Trim /
//! Merge / Crop / Outline / Minus Back — need the level below that: the
//! faces of the arrangement, each addressable on its own.
//!
//! # Model
//!
//! A face is a maximal connected region of the plane whose *containment
//! signature* is constant. The signature of a point is the set of input
//! path indices whose interior contains it (even-odd, matching
//! `flo_curves`' convention that every edge is an exterior edge). Two
//! overlapping circles have three non-empty signatures — `{0}`, `{1}`,
//! `{0,1}` — and therefore three faces; three circles in general
//! position have seven. The empty signature (outside everything) is not
//! a face: the arrangement covers exactly the union of its inputs.
//!
//! A single signature can materialise as SEVERAL disconnected regions
//! (e.g. `A − B` where `B` cuts `A` in two), so faces are the connected
//! COMPONENTS of each signature's region. A component may carry holes —
//! those stay with it as extra subpaths (a compound path), exactly the
//! way Illustrator's Divide keeps an annulus as one object.
//!
//! # Design — why the graph, not 2^N booleans
//!
//! Two designs were on the table.
//!
//! **(A) Face-walk the graph.** `flo_curves` 0.8's [`GraphPath`] *is* the
//! planar graph: `from_merged_paths` + `self_collide` splits every input
//! curve at every crossing. But it does not expose the one thing a face
//! walk needs — the cyclic (by angle) order of edges around a vertex.
//! `edges_for_point` yields storage order, `GraphEdgeRef`'s fields are
//! `pub(crate)` (so a walker cannot even synthesise the refs it wants),
//! and there is no twin/next accessor. A literal face walk would mean
//! rebuilding a DCEL from `all_edges()` + geometry — re-deriving vertex
//! identity from coordinates and re-solving tangential ordering that
//! `self_collide` had already settled.
//!
//! **(B) Containment signature + boolean ops.** Exact, but enumerating
//! every face costs `2^N` boolean chains and each chain re-runs the
//! expensive curve-curve collision pass from scratch.
//!
//! What shipped is the hybrid the real API invites. `flo_curves` exposes
//! the exact hook its own `path_add`/`path_sub`/`path_intersect` ride:
//! [`GraphPath::set_edge_kinds_by_ray_casting`], whose predicate receives
//! the PER-INPUT crossing counts (`PathLabel(i)` → `counts[i]`). So the
//! arrangement is built ONCE (one `self_collide`), and each face's
//! outline is then one `reset_edge_kinds` → `set_edge_kinds_by_ray_casting`
//! → `heal_exterior_gaps` → `exterior_paths` pass over that shared graph.
//! That is design B's semantics (signatures, exact outlines) at design
//! A's cost profile (O(E) per face, not a fresh N-way boolean chain), and
//! it runs through the very same numerical code path the shipped
//! Unite / Intersect / Subtract already use.
//!
//! Which signatures are non-empty is discovered by probing: every face of
//! the arrangement is bounded by at least one edge of the collided graph,
//! so offsetting each edge's midpoint by ±ε along its normal (at three
//! ε scales) lands inside the two faces it separates. Probing can only
//! over-generate — a signature with no region materialises to nothing and
//! is dropped — so false positives are free and only a missed face would
//! hurt. [`PlanarArrangement::complete`] is the honest check on that: the
//! summed face area is compared against the union area (computed on the
//! same graph), and a mismatch is reported rather than hidden.
//!
//! # Coordinate space
//!
//! Paths are combined in RAW path space (the anchors as stored), exactly
//! like [`crate::pathfinder`] — per-element `ItemTransform`s are not
//! composed in. Inputs with different transforms therefore arrange
//! approximately; that is the same residual `pathfinderBoolean` carries.
//!
//! # Residuals
//!
//! - **Live Paint** — its PERSISTENT face/edge graph with gap detection
//!   is out of scope here. That needs a document-resident object type
//!   that survives edits, not a query (RFI B-22).
//! - **`flo_curves` 0.8 `exterior_paths` can panic.** Its point sort
//!   uses an epsilon comparison on x (`(x_a - x_b).abs() < 0.01`) that
//!   is not transitive, and rustc's driftsort detects the violation and
//!   panics rather than returning garbage. This is UPSTREAM and
//!   pre-existing — every shipped `path_add` / `path_sub` /
//!   `path_intersect` ends in the same call, so `pathfinderBoolean`
//!   carries the same hazard — but a dense arrangement makes it easier
//!   to reach. Fixing it means patching or forking `flo_curves`;
//!   `catch_unwind` would not help where it matters (the wasm worker
//!   builds panic=abort).

use std::collections::BTreeMap;

use flo_curves::bezier::path::{
    path_add, path_intersect, path_intersects_ray, path_remove_overlapped_points, path_sub,
    GraphPath, PathLabel, SimpleBezierPath,
};
use flo_curves::bezier::{BezierCurve, BezierCurveFactory, NormalCurve};
use flo_curves::{Coord2, Coordinate2D};
use paged_model::PathAnchor;

use crate::bezier_conv::{flo_to_idml_path, idml_path_to_flo};

/// Subdivision accuracy handed to `flo_curves`. Matches
/// [`crate::pathfinder`]'s constant — one-hundredth of a point, well
/// below any visible threshold, and it keeps recursion bounded.
const PLANAR_ACCURACY: f64 = 0.01;

/// Hard cap on the number of input paths a FULL face enumeration
/// accepts. Face count grows as 2^N in the worst case (N shapes in
/// general position), and every face costs a ray-casting pass over the
/// whole arrangement, so an uncapped call is an easy way to hang a
/// worker thread.
///
/// Past the cap [`build_arrangement`] returns
/// [`PlanarError::TooManyInputs`] and does NO work — it never silently
/// truncates to the first 12. The point query
/// ([`face_at_point`]) is not capped by face count (it materialises one
/// signature), but takes the same input cap so a caller can't smuggle a
/// 200-path arrangement in through the hover door.
pub const MAX_PLANAR_INPUTS: usize = 12;

/// Hard cap on the number of faces a full enumeration will materialise.
/// Hitting it returns [`PlanarError::TooManyFaces`] — again a refusal,
/// not a truncation, because a truncated arrangement is worse than none
/// (its faces no longer tile the union, and every verb built on it would
/// silently drop artwork).
///
/// A BACKSTOP, not the usual limit: 12 simple shapes arrange into O(n²)
/// ≈ 150 faces at worst, so this only fires on pathological inputs —
/// compound paths with dozens of contours, or heavily self-intersecting
/// ones, where the face count is driven by contours rather than by
/// elements.
pub const MAX_PLANAR_FACES: usize = 256;

/// Relative tolerance for the "do the faces tile the union?" check.
const AREA_TOLERANCE: f64 = 1e-3;

/// Honest refusals from the arrangement kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanarError {
    /// More inputs than [`MAX_PLANAR_INPUTS`].
    TooManyInputs { count: usize, cap: usize },
    /// The arrangement resolved more than [`MAX_PLANAR_FACES`] faces.
    TooManyFaces { cap: usize },
    /// Fewer than one input carried usable geometry.
    NoGeometry,
}

impl std::fmt::Display for PlanarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanarError::TooManyInputs { count, cap } => write!(
                f,
                "planar arrangement takes at most {cap} input paths (got {count})"
            ),
            PlanarError::TooManyFaces { cap } => write!(
                f,
                "planar arrangement resolved more than {cap} faces; refine the selection"
            ),
            PlanarError::NoGeometry => write!(f, "no input carried usable path geometry"),
        }
    }
}

/// One face of the arrangement: a connected region whose containment
/// signature is constant.
#[derive(Debug, Clone)]
pub struct PlanarFace {
    /// Stable id, derived from the signature plus the component's
    /// position: `"0-1#0"` = the first component of the region covered
    /// by inputs 0 and 1. Stable across repeated calls with the same
    /// inputs, which is what lets a hover query's id be handed straight
    /// back to `pathfinderFaces`.
    pub id: String,
    /// Sorted input indices whose interior contains this face.
    pub signature: Vec<usize>,
    /// The face outline. Closed; a component with holes carries them as
    /// extra subpaths.
    pub anchors: Vec<PathAnchor>,
    /// Per-contour boundaries into `anchors` (always non-empty).
    pub subpath_starts: Vec<usize>,
    /// Unsigned area (outer contour minus holes).
    pub area: f64,
    /// A point strictly inside the face — the anchor a hover highlight
    /// or a fill drop can use without re-deriving containment.
    pub inside: (f32, f32),
}

/// The full arrangement of N input paths.
#[derive(Debug, Clone)]
pub struct PlanarArrangement {
    /// Every face, ordered by signature then component position.
    pub faces: Vec<PlanarFace>,
    /// How many paths went in (faces index into this range).
    pub input_count: usize,
    /// Area of the union of all inputs.
    pub union_area: f64,
    /// Summed face area.
    pub faces_area: f64,
    /// `faces_area ≈ union_area` — the faces tile the union. `false`
    /// means the probe pass missed a region (a sliver thinner than the
    /// finest probe offset); the faces returned are still real, the set
    /// is just not exhaustive. Callers that must be exact should refuse
    /// rather than proceed.
    pub complete: bool,
}

impl PlanarArrangement {
    /// Look a face up by its stable id.
    pub fn face(&self, id: &str) -> Option<&PlanarFace> {
        self.faces.iter().find(|f| f.id == id)
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Build the full arrangement of `inputs` (each a flat anchor list +
/// `subpath_starts`, the idml path representation).
pub fn build_arrangement(
    inputs: &[(Vec<PathAnchor>, Vec<usize>)],
) -> Result<PlanarArrangement, PlanarError> {
    if inputs.len() > MAX_PLANAR_INPUTS {
        return Err(PlanarError::TooManyInputs {
            count: inputs.len(),
            cap: MAX_PLANAR_INPUTS,
        });
    }
    let flo = to_flo(inputs)?;
    let mut graph = build_graph(&flo);

    // Probe both sides of every arrangement edge to discover which
    // signatures actually carry area.
    let candidates = probe_signatures(&graph, &flo);
    if candidates.len() > MAX_PLANAR_FACES {
        return Err(PlanarError::TooManyFaces {
            cap: MAX_PLANAR_FACES,
        });
    }

    let n = flo.len();
    let mut faces: Vec<PlanarFace> = Vec::new();
    for signature in &candidates {
        let loops = materialize(&mut graph, &flo, signature);
        for (anchors, starts, area, inside) in split_components(&loops) {
            // VALIDATE (found by the property sweep): `heal_exterior_gaps`
            // bridges skipped edges by walking up to three hops, and on a
            // dense five-path arrangement one such bridge closed a loop
            // around a NEIGHBOURING region — two faces came back with
            // different signatures over the very same 11 pt². A component
            // of signature S must contain points whose containment set IS
            // S, so re-derive it at the component's interior point and
            // drop the impostor. Components too thin to place a probe in
            // are kept unvalidated rather than guessed away.
            let inside = match inside {
                Some(p) => {
                    let actual = signature_at(&flo, Coord2(p.0 as f64, p.1 as f64));
                    if actual.is_some_and(|sig| sig != *signature) {
                        continue;
                    }
                    p
                }
                None => anchors.first().map(|a| a.anchor).unwrap_or((0.0, 0.0)),
            };
            faces.push(PlanarFace {
                id: String::new(), // assigned below, after ordering
                signature: signature.clone(),
                anchors,
                subpath_starts: starts,
                area,
                inside,
            });
            if faces.len() > MAX_PLANAR_FACES {
                return Err(PlanarError::TooManyFaces {
                    cap: MAX_PLANAR_FACES,
                });
            }
        }
    }

    // Deterministic order: by signature, then by the component's
    // top-left-most anchor (rounded, so float wobble can't reshuffle
    // ids between calls).
    faces.sort_by(|a, b| {
        a.signature
            .cmp(&b.signature)
            .then_with(|| order_key(a).partial_cmp(&order_key(b)).expect("finite"))
    });
    let mut per_signature: BTreeMap<Vec<usize>, usize> = BTreeMap::new();
    for face in faces.iter_mut() {
        let slot = per_signature.entry(face.signature.clone()).or_insert(0);
        face.id = face_id(&face.signature, *slot);
        *slot += 1;
    }

    let faces_area: f64 = faces.iter().map(|f| f.area).sum();
    let union_area = region_area(&materialize_union(&mut graph, &flo));
    let complete = (faces_area - union_area).abs() <= AREA_TOLERANCE * union_area.max(1.0);

    Ok(PlanarArrangement {
        faces,
        input_count: n,
        union_area,
        faces_area,
        complete,
    })
}

/// The Shape Builder hover query: which face lies under `point`?
///
/// Cheap relative to a full enumeration — the signature comes from N
/// point-in-path tests against the ORIGINAL inputs, and only that one
/// signature is materialised. `None` when the point is outside every
/// input (no face there).
///
/// Ids agree with [`build_arrangement`]'s for the same inputs: the
/// component ordering rule is shared.
pub fn face_at_point(
    inputs: &[(Vec<PathAnchor>, Vec<usize>)],
    point: (f32, f32),
) -> Result<Option<PlanarFace>, PlanarError> {
    if inputs.len() > MAX_PLANAR_INPUTS {
        return Err(PlanarError::TooManyInputs {
            count: inputs.len(),
            cap: MAX_PLANAR_INPUTS,
        });
    }
    let flo = to_flo(inputs)?;
    let p = Coord2(point.0 as f64, point.1 as f64);
    let signature = signature_at(&flo, p).unwrap_or_default();
    if signature.is_empty() {
        return Ok(None);
    }
    let mut graph = build_graph(&flo);
    let loops = materialize(&mut graph, &flo, &signature);
    // Same filter AND ordering rule `build_arrangement` uses — both,
    // because the component INDEX is half the face id: validating in one
    // place and not the other would renumber the components and hand
    // back an id that names a different face.
    let mut components: Vec<Component> = split_components(&loops)
        .into_iter()
        .filter(|(_, _, _, inside)| match inside {
            Some(p) => !signature_at(&flo, Coord2(p.0 as f64, p.1 as f64))
                .is_some_and(|sig| sig != signature),
            None => true,
        })
        .collect();
    components.sort_by(|a, b| {
        component_key(&a.0)
            .partial_cmp(&component_key(&b.0))
            .expect("finite")
    });
    for (slot, (anchors, starts, area, inside)) in components.into_iter().enumerate() {
        let flo_component = idml_path_to_flo(&anchors, &starts);
        if point_inside(&flo_component, p).unwrap_or(false) {
            let inside = inside.unwrap_or(point);
            return Ok(Some(PlanarFace {
                id: face_id(&signature, slot),
                signature,
                anchors,
                subpath_starts: starts,
                area,
                inside,
            }));
        }
    }
    Ok(None)
}

/// Union a set of faces into one path (Shape Builder's click/drag
/// output). Shared boundaries between adjacent faces DISSOLVE — the
/// result is one clean outline, not a compound path with internal
/// edges — because the merge runs through `path_add`, the same union
/// `pathfinderBoolean`'s Unite rides.
pub fn union_faces(faces: &[&PlanarFace]) -> (Vec<PathAnchor>, Vec<usize>) {
    let loops: Vec<SimpleBezierPath> = faces
        .iter()
        .flat_map(|face| idml_path_to_flo(&face.anchors, &face.subpath_starts))
        .collect();
    if loops.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // Faces are pairwise disjoint, so an even-odd sweep over their
    // combined contours IS their union — and it dissolves the edges two
    // adjacent faces share (each such edge has inside on both sides, so
    // it never comes back out as exterior). `path_remove_overlapped_points`
    // is flo_curves' name for exactly that sweep; it self-collides once
    // instead of running one `path_add` per face.
    let merged: Vec<SimpleBezierPath> = path_remove_overlapped_points(&loops, PLANAR_ACCURACY);
    let merged: Vec<SimpleBezierPath> = merged.into_iter().filter(|p| !is_sliver(p)).collect();
    flo_to_idml_path(&merged)
}

/// Every EDGE of the arrangement, split at each crossing, as an open
/// two-anchor path plus the index of the input that contributed it.
/// Backs the Outline verb (fills become strokes along the component
/// line segments).
pub fn arrangement_edges(
    inputs: &[(Vec<PathAnchor>, Vec<usize>)],
) -> Result<Vec<(usize, Vec<PathAnchor>)>, PlanarError> {
    if inputs.len() > MAX_PLANAR_INPUTS {
        return Err(PlanarError::TooManyInputs {
            count: inputs.len(),
            cap: MAX_PLANAR_INPUTS,
        });
    }
    let flo = to_flo(inputs)?;
    let graph = build_graph(&flo);
    let mut out: Vec<(usize, Vec<PathAnchor>)> = Vec::new();
    for edge_ref in graph.all_edge_refs().collect::<Vec<_>>() {
        let edge = graph.get_edge(edge_ref);
        let start = edge.start_point();
        let end = edge.end_point();
        let (cp1, cp2) = edge.control_points();
        // Degenerate (zero-length) edges are collision artefacts, not
        // segments anyone can stroke.
        if (start.x() - end.x()).abs() < 1e-6 && (start.y() - end.y()).abs() < 1e-6 {
            continue;
        }
        let PathLabel(label) = graph.edge_label(edge_ref);
        out.push((
            label as usize,
            vec![
                PathAnchor {
                    anchor: (start.x() as f32, start.y() as f32),
                    left: (start.x() as f32, start.y() as f32),
                    right: (cp1.x() as f32, cp1.y() as f32),
                },
                PathAnchor {
                    anchor: (end.x() as f32, end.y() as f32),
                    left: (cp2.x() as f32, cp2.y() as f32),
                    right: (end.x() as f32, end.y() as f32),
                },
            ],
        ));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn to_flo(
    inputs: &[(Vec<PathAnchor>, Vec<usize>)],
) -> Result<Vec<Vec<SimpleBezierPath>>, PlanarError> {
    let flo: Vec<Vec<SimpleBezierPath>> = inputs
        .iter()
        .map(|(anchors, starts)| idml_path_to_flo(anchors, starts))
        .collect();
    if flo.iter().all(|p| p.is_empty()) {
        return Err(PlanarError::NoGeometry);
    }
    Ok(flo)
}

/// Merge every input (labelled by its index) into one graph and split
/// every curve at every crossing — including an input's crossings with
/// ITSELF, which is what makes a self-intersecting input arrange
/// correctly (its doubly-wound lobe reads as outside, the even-odd
/// convention flo_curves uses throughout).
fn build_graph(flo: &[Vec<SimpleBezierPath>]) -> GraphPath<Coord2, PathLabel> {
    let labelled: Vec<(&SimpleBezierPath, PathLabel)> = flo
        .iter()
        .enumerate()
        .flat_map(|(i, paths)| paths.iter().map(move |p| (p, PathLabel(i as u32))))
        .collect();
    let mut graph = GraphPath::from_merged_paths(labelled.iter().map(|(p, l)| (*p, *l)));
    graph.self_collide(PLANAR_ACCURACY);
    graph.round(PLANAR_ACCURACY);
    graph
}

/// Mark the edges bounding the region whose signature is exactly
/// `signature`, then read the loops back out.
///
/// FALLBACK (found by the property sweep, not by reasoning): flo_curves'
/// ray-casting marks an edge exterior only when a ray crosses it away
/// from an intersection and away from the curve's ends, and it then
/// tries to bridge whatever it skipped with `heal_exterior_gaps`
/// (`MAX_HEAL_DEPTH` = 3). With TWO labels — every case the shipped
/// boolean ops exercise — that always closed. With a five-path
/// arrangement and a predicate as selective as "the signature is exactly
/// this", one region came back with 10 exterior edges that formed no
/// closed loop, so `exterior_paths` returned nothing for a 418 pt² face.
/// When the graph pass reports unhealed gaps or yields nothing, this
/// falls back to the design-B formula — `(∩ signature) − (∪ rest)` over
/// the ORIGINAL inputs, the same `path_intersect` / `path_sub` chain
/// `pathfinder_boolean` ships — which resolves those regions exactly.
/// The graph pass stays the primary because it is O(E) per face and
/// keeps adjacent faces sharing literally the same edges.
fn materialize(
    graph: &mut GraphPath<Coord2, PathLabel>,
    flo: &[Vec<SimpleBezierPath>],
    signature: &[usize],
) -> Vec<SimpleBezierPath> {
    let n = flo.len();
    let mut wanted = vec![false; n];
    for i in signature {
        wanted[*i] = true;
    }
    graph.reset_edge_kinds();
    graph.set_edge_kinds_by_ray_casting(|counts| {
        (0..n).all(|i| (counts.get(i).copied().unwrap_or(0) & 1 != 0) == wanted[i])
    });
    let healed = graph.heal_exterior_gaps();
    let loops = graph.exterior_paths::<SimpleBezierPath>();
    if healed && !loops.is_empty() {
        return loops;
    }
    boolean_chain(flo, signature)
}

/// `(∩ signature) − (∪ rest)` over the original inputs — the exact
/// region a signature names, computed the way the shipped Pathfinder
/// booleans compute theirs. The empty-operand short-circuits in
/// `path_intersect` / `path_sub` return the OTHER operand, so an empty
/// accumulator has to be caught before every step rather than fed in.
fn boolean_chain(flo: &[Vec<SimpleBezierPath>], signature: &[usize]) -> Vec<SimpleBezierPath> {
    let mut acc: Vec<SimpleBezierPath> = Vec::new();
    for i in signature {
        if flo[*i].is_empty() {
            return Vec::new();
        }
        acc = if acc.is_empty() {
            flo[*i].clone()
        } else {
            path_intersect::<SimpleBezierPath>(&acc, &flo[*i], PLANAR_ACCURACY)
        };
        if acc.is_empty() {
            return Vec::new();
        }
    }
    for (i, other) in flo.iter().enumerate() {
        if signature.contains(&i) || other.is_empty() {
            continue;
        }
        acc = path_sub::<SimpleBezierPath>(&acc, other, PLANAR_ACCURACY);
        if acc.is_empty() {
            return Vec::new();
        }
    }
    acc
}

/// The union of every input, off the same graph — the denominator of
/// the completeness check. Same fallback as [`materialize`]: a union the
/// graph pass can't close is recomputed as a `path_add` chain.
fn materialize_union(
    graph: &mut GraphPath<Coord2, PathLabel>,
    flo: &[Vec<SimpleBezierPath>],
) -> Vec<SimpleBezierPath> {
    let n = flo.len();
    graph.reset_edge_kinds();
    graph.set_edge_kinds_by_ray_casting(move |counts| {
        (0..n).any(|i| counts.get(i).copied().unwrap_or(0) & 1 != 0)
    });
    let healed = graph.heal_exterior_gaps();
    let loops = graph.exterior_paths::<SimpleBezierPath>();
    if healed && !loops.is_empty() {
        return loops;
    }
    let mut acc: Vec<SimpleBezierPath> = Vec::new();
    for paths in flo.iter().filter(|p| !p.is_empty()) {
        acc = if acc.is_empty() {
            paths.clone()
        } else {
            path_add::<SimpleBezierPath>(&acc, paths, PLANAR_ACCURACY)
        };
    }
    acc
}

/// Discover the non-empty signatures by stepping off both sides of
/// every arrangement edge. Over-generation is harmless (an empty
/// signature materialises to nothing); under-generation is what the
/// completeness check catches, so probe at three ε scales.
fn probe_signatures(
    graph: &GraphPath<Coord2, PathLabel>,
    flo: &[Vec<SimpleBezierPath>],
) -> Vec<Vec<usize>> {
    let diagonal = bbox_diagonal(flo).max(1e-3);
    let scales = [1e-2_f64, 1e-3, 1e-4];
    let mut found: std::collections::BTreeSet<Vec<usize>> = std::collections::BTreeSet::new();
    for edge_ref in graph.all_edge_refs().collect::<Vec<_>>() {
        let edge = graph.get_edge(edge_ref);
        let mid = edge.point_at_pos(0.5);
        let normal = edge.normal_at_pos(0.5);
        let len = (normal.x() * normal.x() + normal.y() * normal.y()).sqrt();
        if !len.is_finite() || len < 1e-12 {
            continue;
        }
        let unit = Coord2(normal.x() / len, normal.y() / len);
        for scale in scales {
            let eps = diagonal * scale;
            for side in [1.0_f64, -1.0] {
                let probe = Coord2(
                    mid.x() + unit.x() * eps * side,
                    mid.y() + unit.y() * eps * side,
                );
                let mut signature: Vec<usize> = Vec::new();
                let mut usable = true;
                for (i, paths) in flo.iter().enumerate() {
                    match point_inside(paths, probe) {
                        Some(true) => signature.push(i),
                        Some(false) => {}
                        None => {
                            usable = false;
                            break;
                        }
                    }
                }
                if usable && !signature.is_empty() {
                    found.insert(signature);
                }
            }
        }
    }
    found.into_iter().collect()
}

/// The containment signature at a point: which inputs' interiors hold
/// it. `None` when any input's test was ambiguous (the point sits on an
/// outline) — the caller drops the probe rather than guessing.
fn signature_at(flo: &[Vec<SimpleBezierPath>], point: Coord2) -> Option<Vec<usize>> {
    let mut signature = Vec::new();
    for (i, paths) in flo.iter().enumerate() {
        match point_inside(paths, point) {
            Some(true) => signature.push(i),
            Some(false) => {}
            None => return None,
        }
    }
    Some(signature)
}

/// Even-odd point-in-path over one input's subpaths.
///
/// `None` when no probe direction gave an unambiguous answer (the point
/// sits on the outline, or every ray we tried grazed a joint) — the
/// caller drops that probe rather than guessing.
fn point_inside(paths: &[SimpleBezierPath], point: Coord2) -> Option<bool> {
    if paths.is_empty() {
        return Some(false);
    }
    // Directions chosen to be off-axis and mutually non-parallel, so a
    // path that is tangent to one is not tangent to the next.
    const DIRECTIONS: [(f64, f64); 4] = [
        (0.930_2, 0.367_0),
        (-0.317_1, 0.948_4),
        (0.634_9, -0.772_6),
        (-0.881_3, -0.472_5),
    ];
    let reach = paths
        .iter()
        .flat_map(|(start, segments)| {
            std::iter::once(*start).chain(segments.iter().map(|(_, _, end)| *end))
        })
        .fold(1.0_f64, |acc, p| {
            acc.max((p.x() - point.x()).abs())
                .max((p.y() - point.y()).abs())
        })
        * 4.0;
    for (dx, dy) in DIRECTIONS {
        let far = Coord2(point.x() + dx * reach, point.y() + dy * reach);
        let ray = (point, far);
        let mut crossings = 0usize;
        let mut ambiguous = false;
        for path in paths {
            for (_section, curve_t, line_t) in path_intersects_ray(path, &ray) {
                // A hit at the ray's origin means the probe sits ON the
                // outline; a hit at a segment joint would be counted
                // twice. Both make this direction unusable.
                if line_t < 1e-9 || !(1e-6..=1.0 - 1e-6).contains(&curve_t) {
                    ambiguous = true;
                    break;
                }
                crossings += 1;
            }
            if ambiguous {
                break;
            }
        }
        if !ambiguous {
            return Some(crossings % 2 == 1);
        }
    }
    None
}

/// One connected component of a signature's region:
/// `(anchors, subpath_starts, area, interior point)`. The interior
/// point is `None` when no probe landed strictly inside (a region too
/// thin for any offset to fall in); such a component is kept but cannot
/// be signature-validated.
type Component = (Vec<PathAnchor>, Vec<usize>, f64, Option<(f32, f32)>);

/// Group the loops of one signature's region into connected components:
/// a loop at even nesting depth is a component's outer contour, and the
/// loops whose immediate parent it is are that component's holes.
fn split_components(loops: &[SimpleBezierPath]) -> Vec<Component> {
    let usable: Vec<&SimpleBezierPath> = loops
        .iter()
        .filter(|p| !p.1.is_empty() && !is_sliver(p))
        .collect();
    if usable.is_empty() {
        return Vec::new();
    }
    // depth[k] = how many other loops contain loop k.
    let mut depth = vec![0usize; usable.len()];
    let mut contained_by: Vec<Vec<usize>> = vec![Vec::new(); usable.len()];
    for k in 0..usable.len() {
        let probe = usable[k].0;
        for (j, other) in usable.iter().enumerate() {
            if j == k {
                continue;
            }
            if point_inside(std::slice::from_ref(*other), probe).unwrap_or(false) {
                depth[k] += 1;
                contained_by[k].push(j);
            }
        }
    }
    let mut out = Vec::new();
    for k in 0..usable.len() {
        if depth[k] % 2 != 0 {
            continue;
        }
        // Holes of component k: loops whose deepest container is k.
        let mut member_loops: Vec<&SimpleBezierPath> = vec![usable[k]];
        for (j, containers) in contained_by.iter().enumerate() {
            if j == k || depth[j] != depth[k] + 1 {
                continue;
            }
            if containers.contains(&k) {
                member_loops.push(usable[j]);
            }
        }
        let owned: Vec<SimpleBezierPath> = member_loops.iter().map(|p| (*p).clone()).collect();
        let (anchors, starts) = flo_to_idml_path(&owned);
        if anchors.is_empty() {
            continue;
        }
        let outer = path_area(usable[k]).abs();
        let holes: f64 = member_loops
            .iter()
            .skip(1)
            .map(|p| path_area(p).abs())
            .sum();
        let area = (outer - holes).max(0.0);
        out.push((anchors, starts, area, interior_point(&owned)));
    }
    out
}

/// A point strictly inside a component: step off its outer contour
/// along the inward normal, shrinking until the probe lands inside.
fn interior_point(component: &[SimpleBezierPath]) -> Option<(f32, f32)> {
    let (start, segments) = component.first()?;
    let mut scale = 1e-2_f64;
    let diag = bbox_diagonal(std::slice::from_ref(&component.to_vec())).max(1e-3);
    for _ in 0..6 {
        for (idx, (cp1, cp2, end)) in segments.iter().enumerate() {
            let from = if idx == 0 {
                *start
            } else {
                segments[idx - 1].2
            };
            let curve = flo_curves::bezier::Curve::from_points(from, (*cp1, *cp2), *end);
            let mid = curve.point_at_pos(0.5);
            let normal = curve.normal_at_pos(0.5);
            let len = (normal.x() * normal.x() + normal.y() * normal.y()).sqrt();
            if !len.is_finite() || len < 1e-12 {
                continue;
            }
            for side in [1.0_f64, -1.0] {
                let eps = diag * scale * side;
                let probe = Coord2(
                    mid.x() + normal.x() / len * eps,
                    mid.y() + normal.y() / len * eps,
                );
                if point_inside(component, probe) == Some(true) {
                    return Some((probe.x() as f32, probe.y() as f32));
                }
            }
        }
        scale /= 4.0;
    }
    None
}

/// True when a loop is thinner than the arrangement's own numerical
/// resolution EVERYWHERE — area below `accuracy × perimeter` means an
/// average width under `accuracy`. Two arcs that should have been
/// coincident but round to a hair apart leave exactly such a lens
/// behind; it is a collision artefact, not a region, and no real face
/// can be resolved at that width anyway.
fn is_sliver(path: &SimpleBezierPath) -> bool {
    let (start, segments) = path;
    let mut perimeter = 0.0;
    let mut from = *start;
    for (_, _, end) in segments {
        perimeter += ((end.x() - from.x()).powi(2) + (end.y() - from.y()).powi(2)).sqrt();
        from = *end;
    }
    path_area(path).abs() < PLANAR_ACCURACY * perimeter
}

/// Signed area of one closed cubic path via Green's theorem. The
/// integrand is degree 5 in `t`, so a 3-point Gauss-Legendre rule
/// integrates it EXACTLY — no flattening, no tolerance.
fn path_area(path: &SimpleBezierPath) -> f64 {
    const NODES: [f64; 3] = [
        0.5 - 0.5 * 0.774_596_669_241_483_4,
        0.5,
        0.5 + 0.5 * 0.774_596_669_241_483_4,
    ];
    const WEIGHTS: [f64; 3] = [5.0 / 18.0, 8.0 / 18.0, 5.0 / 18.0];
    let (start, segments) = path;
    let mut total = 0.0;
    let mut from = *start;
    for (cp1, cp2, end) in segments {
        let (p0, p1, p2, p3) = (from, *cp1, *cp2, *end);
        for (t, w) in NODES.iter().zip(WEIGHTS.iter()) {
            let mt = 1.0 - t;
            let (b0, b1, b2, b3) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
            let x = p0.x() * b0 + p1.x() * b1 + p2.x() * b2 + p3.x() * b3;
            let y = p0.y() * b0 + p1.y() * b1 + p2.y() * b2 + p3.y() * b3;
            let (d0, d1, d2) = (3.0 * mt * mt, 6.0 * mt * t, 3.0 * t * t);
            let dx = (p1.x() - p0.x()) * d0 + (p2.x() - p1.x()) * d1 + (p3.x() - p2.x()) * d2;
            let dy = (p1.y() - p0.y()) * d0 + (p2.y() - p1.y()) * d1 + (p3.y() - p2.y()) * d2;
            total += w * (x * dy - y * dx);
        }
        from = *end;
    }
    total / 2.0
}

/// Unsigned area of a region given as a loop set.
///
/// Deliberately orientation-AGNOSTIC. `exterior_paths` does not promise
/// a consistent winding across disconnected components — a region made
/// of two islands can come back with one loop clockwise and the other
/// anticlockwise — so summing signed areas silently subtracts one island
/// from the other. (That is exactly how the property sweep caught this:
/// a three-shape case reported a union of 335 pt² for 851 pt² of faces.)
/// Nesting depth is the invariant that holds instead: a loop at even
/// depth is an outer contour and adds, a loop at odd depth is a hole and
/// subtracts.
fn region_area(loops: &[SimpleBezierPath]) -> f64 {
    let usable: Vec<&SimpleBezierPath> = loops.iter().filter(|p| !p.1.is_empty()).collect();
    let mut total = 0.0;
    for (k, loop_k) in usable.iter().enumerate() {
        let depth = usable
            .iter()
            .enumerate()
            .filter(|(j, other)| {
                *j != k && point_inside(std::slice::from_ref(**other), loop_k.0).unwrap_or(false)
            })
            .count();
        let area = path_area(loop_k).abs();
        if depth % 2 == 0 {
            total += area;
        } else {
            total -= area;
        }
    }
    total.max(0.0)
}

fn bbox_diagonal(flo: &[Vec<SimpleBezierPath>]) -> f64 {
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for paths in flo {
        for (start, segments) in paths {
            for p in std::iter::once(start).chain(segments.iter().map(|(_, _, end)| end)) {
                min_x = min_x.min(p.x());
                min_y = min_y.min(p.y());
                max_x = max_x.max(p.x());
                max_y = max_y.max(p.y());
            }
        }
    }
    if !min_x.is_finite() {
        return 0.0;
    }
    ((max_x - min_x).powi(2) + (max_y - min_y).powi(2)).sqrt()
}

fn face_id(signature: &[usize], component: usize) -> String {
    let sig = signature
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("-");
    format!("{sig}#{component}")
}

/// Ordering key for components of the same signature: the top-left-most
/// anchor, rounded so float wobble can't reshuffle ids between calls.
fn component_key(anchors: &[PathAnchor]) -> (f64, f64) {
    let mut best = (f64::INFINITY, f64::INFINITY);
    for a in anchors {
        let key = (
            (a.anchor.0 as f64 * 1000.0).round() / 1000.0,
            (a.anchor.1 as f64 * 1000.0).round() / 1000.0,
        );
        if key < best {
            best = key;
        }
    }
    best
}

fn order_key(face: &PlanarFace) -> (f64, f64) {
    component_key(&face.anchors)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed circle as four cubic arcs (the standard 0.5523 kappa).
    fn circle(cx: f32, cy: f32, r: f32) -> Vec<PathAnchor> {
        let k = r * 0.552_284_8;
        vec![
            PathAnchor {
                anchor: (cx, cy - r),
                left: (cx - k, cy - r),
                right: (cx + k, cy - r),
            },
            PathAnchor {
                anchor: (cx + r, cy),
                left: (cx + r, cy - k),
                right: (cx + r, cy + k),
            },
            PathAnchor {
                anchor: (cx, cy + r),
                left: (cx + k, cy + r),
                right: (cx - k, cy + r),
            },
            PathAnchor {
                anchor: (cx - r, cy),
                left: (cx - r, cy + k),
                right: (cx - r, cy - k),
            },
        ]
    }

    fn rect(left: f32, top: f32, right: f32, bottom: f32) -> Vec<PathAnchor> {
        let p = |x: f32, y: f32| PathAnchor {
            anchor: (x, y),
            left: (x, y),
            right: (x, y),
        };
        vec![
            p(left, top),
            p(right, top),
            p(right, bottom),
            p(left, bottom),
        ]
    }

    fn input(anchors: Vec<PathAnchor>) -> (Vec<PathAnchor>, Vec<usize>) {
        (anchors, vec![0])
    }

    fn signatures(arr: &PlanarArrangement) -> Vec<Vec<usize>> {
        let mut sigs: Vec<Vec<usize>> = arr.faces.iter().map(|f| f.signature.clone()).collect();
        sigs.sort();
        sigs
    }

    #[test]
    fn two_overlapping_circles_resolve_three_faces() {
        let arr =
            build_arrangement(&[input(circle(0.0, 0.0, 10.0)), input(circle(8.0, 0.0, 10.0))])
                .expect("arrangement");
        assert_eq!(arr.faces.len(), 3, "A-only, B-only, A∩B");
        assert_eq!(signatures(&arr), vec![vec![0], vec![0, 1], vec![1]]);
        assert!(arr.complete, "faces must tile the union");
    }

    #[test]
    fn three_circle_venn_resolves_seven_faces() {
        let arr = build_arrangement(&[
            input(circle(0.0, 0.0, 10.0)),
            input(circle(9.0, 0.0, 10.0)),
            input(circle(4.5, 8.0, 10.0)),
        ])
        .expect("arrangement");
        assert_eq!(arr.faces.len(), 7, "the classic 7-region Venn diagram");
        assert_eq!(
            signatures(&arr),
            vec![
                vec![0],
                vec![0, 1],
                vec![0, 1, 2],
                vec![0, 2],
                vec![1],
                vec![1, 2],
                vec![2],
            ]
        );
        assert!(arr.complete);
    }

    #[test]
    fn disjoint_shapes_stay_one_face_each() {
        let arr = build_arrangement(&[
            input(rect(0.0, 0.0, 10.0, 10.0)),
            input(rect(50.0, 50.0, 60.0, 60.0)),
        ])
        .expect("arrangement");
        assert_eq!(arr.faces.len(), 2);
        assert_eq!(signatures(&arr), vec![vec![0], vec![1]]);
        for face in &arr.faces {
            assert!((face.area - 100.0).abs() < 1e-2, "area {}", face.area);
        }
        assert!(arr.complete);
    }

    #[test]
    fn nested_shapes_make_a_ring_face_with_a_hole() {
        // A strictly contains B: two faces — the ring (A only) and the
        // disc (A and B). The ring is ONE face carrying a hole.
        let arr = build_arrangement(&[input(circle(0.0, 0.0, 20.0)), input(circle(0.0, 0.0, 8.0))])
            .expect("arrangement");
        assert_eq!(arr.faces.len(), 2);
        let ring = arr
            .faces
            .iter()
            .find(|f| f.signature == vec![0])
            .expect("ring face");
        assert_eq!(ring.subpath_starts.len(), 2, "outer contour + hole");
        let disc = arr
            .faces
            .iter()
            .find(|f| f.signature == vec![0, 1])
            .expect("disc face");
        assert_eq!(disc.subpath_starts.len(), 1);
        // π(20² − 8²) ≈ 1055.6, π·8² ≈ 201.1 (the cubic circle
        // approximation is accurate to ~0.02%).
        assert!((ring.area - 1055.6).abs() < 2.0, "ring area {}", ring.area);
        assert!((disc.area - 201.1).abs() < 1.0, "disc area {}", disc.area);
        assert!(arr.complete);
    }

    #[test]
    fn a_split_in_two_yields_two_components_of_one_signature() {
        // A wide bar crossed by a taller bar: the wide bar's exclusive
        // area falls into TWO disconnected components, so one signature
        // yields two faces with distinct ids.
        let arr = build_arrangement(&[
            input(rect(0.0, 4.0, 30.0, 6.0)),
            input(rect(10.0, 0.0, 20.0, 10.0)),
        ])
        .expect("arrangement");
        let bar_only: Vec<&PlanarFace> = arr
            .faces
            .iter()
            .filter(|f| f.signature == vec![0])
            .collect();
        assert_eq!(bar_only.len(), 2, "left stub + right stub");
        assert_ne!(bar_only[0].id, bar_only[1].id);
        assert_eq!(bar_only[0].id, "0#0");
        assert_eq!(bar_only[1].id, "0#1");
        assert!(arr.complete);
    }

    #[test]
    fn self_intersecting_input_reads_even_odd() {
        // A bow-tie: two triangular lobes meeting at a crossing. Each
        // lobe is its own face of signature {0}.
        let p = |x: f32, y: f32| PathAnchor {
            anchor: (x, y),
            left: (x, y),
            right: (x, y),
        };
        let bowtie = vec![p(0.0, 0.0), p(10.0, 10.0), p(0.0, 10.0), p(10.0, 0.0)];
        let arr = build_arrangement(&[(bowtie, vec![0])]).expect("arrangement");
        assert_eq!(arr.faces.len(), 2, "two lobes");
        for face in &arr.faces {
            assert_eq!(face.signature, vec![0]);
            // Each lobe is the triangle (5,5)-(10,10)-(0,10) (and its
            // mirror): ½ · base 10 · height 5 = 25.
            assert!((face.area - 25.0).abs() < 0.5, "lobe area {}", face.area);
        }
    }

    #[test]
    fn tangent_contact_does_not_invent_a_face() {
        // Two circles touching at exactly one point: no overlap area,
        // so two faces and no {0,1}.
        let arr = build_arrangement(&[
            input(circle(0.0, 0.0, 10.0)),
            input(circle(20.0, 0.0, 10.0)),
        ])
        .expect("arrangement");
        assert_eq!(signatures(&arr), vec![vec![0], vec![1]]);
        assert!(arr.faces.iter().all(|f| f.signature.len() == 1));
    }

    #[test]
    fn shared_edge_contact_does_not_invent_a_face() {
        // Two rectangles sharing a full edge — the degenerate case the
        // collision code has to order consistently.
        let arr = build_arrangement(&[
            input(rect(0.0, 0.0, 10.0, 10.0)),
            input(rect(10.0, 0.0, 20.0, 10.0)),
        ])
        .expect("arrangement");
        assert_eq!(signatures(&arr), vec![vec![0], vec![1]]);
    }

    #[test]
    fn faces_are_disjoint_and_tile_the_union() {
        // The core invariant, checked directly: no two faces overlap,
        // and together they cover the union exactly.
        let inputs = vec![
            input(circle(0.0, 0.0, 10.0)),
            input(circle(9.0, 0.0, 10.0)),
            input(rect(-4.0, -4.0, 14.0, 4.0)),
        ];
        let arr = build_arrangement(&inputs).expect("arrangement");
        assert!(
            arr.complete,
            "faces {} union {}",
            arr.faces_area, arr.union_area
        );
        for (i, a) in arr.faces.iter().enumerate() {
            for b in arr.faces.iter().skip(i + 1) {
                let overlap = flo_curves::bezier::path::path_intersect::<SimpleBezierPath>(
                    &idml_path_to_flo(&a.anchors, &a.subpath_starts),
                    &idml_path_to_flo(&b.anchors, &b.subpath_starts),
                    PLANAR_ACCURACY,
                );
                let area = region_area(&overlap);
                assert!(area < 1e-2, "faces {} and {} overlap by {area}", a.id, b.id);
            }
        }
    }

    #[test]
    fn face_at_point_answers_the_hover_query() {
        let inputs = vec![input(circle(0.0, 0.0, 10.0)), input(circle(8.0, 0.0, 10.0))];
        // (4, 0) is in both circles.
        let both = face_at_point(&inputs, (4.0, 0.0))
            .expect("query")
            .expect("a face under the point");
        assert_eq!(both.signature, vec![0, 1]);
        // (-8, 0) is only in the first.
        let left = face_at_point(&inputs, (-8.0, 0.0))
            .expect("query")
            .expect("a face under the point");
        assert_eq!(left.signature, vec![0]);
        // Far away: no face.
        assert!(face_at_point(&inputs, (100.0, 100.0))
            .expect("query")
            .is_none());
    }

    #[test]
    fn face_at_point_ids_match_the_full_enumeration() {
        let inputs = vec![
            input(rect(0.0, 4.0, 30.0, 6.0)),
            input(rect(10.0, 0.0, 20.0, 10.0)),
        ];
        let arr = build_arrangement(&inputs).expect("arrangement");
        for face in &arr.faces {
            let hit = face_at_point(&inputs, face.inside)
                .expect("query")
                .expect("the representative point must land on its own face");
            assert_eq!(hit.id, face.id, "signature {:?}", face.signature);
        }
    }

    #[test]
    fn union_of_all_faces_recovers_the_union_of_inputs() {
        let inputs = vec![input(circle(0.0, 0.0, 10.0)), input(circle(8.0, 0.0, 10.0))];
        let arr = build_arrangement(&inputs).expect("arrangement");
        let refs: Vec<&PlanarFace> = arr.faces.iter().collect();
        let (anchors, starts) = union_faces(&refs);
        let merged = idml_path_to_flo(&anchors, &starts);
        assert_eq!(merged.len(), 1, "one clean outline, no internal edges");
        assert!((region_area(&merged) - arr.union_area).abs() < 1e-1);
    }

    #[test]
    fn arrangement_edges_split_at_every_crossing() {
        // Two crossing rectangles: each contributes 4 sides, and the 8
        // crossings split them into 8 + 8 pieces.
        let edges = arrangement_edges(&[
            input(rect(0.0, 4.0, 30.0, 6.0)),
            input(rect(10.0, 0.0, 20.0, 10.0)),
        ])
        .expect("edges");
        assert_eq!(edges.len(), 16);
        assert_eq!(edges.iter().filter(|(label, _)| *label == 0).count(), 8);
        assert_eq!(edges.iter().filter(|(label, _)| *label == 1).count(), 8);
    }

    #[test]
    fn too_many_inputs_is_an_honest_refusal() {
        let inputs: Vec<(Vec<PathAnchor>, Vec<usize>)> = (0..MAX_PLANAR_INPUTS + 1)
            .map(|i| input(rect(i as f32, 0.0, i as f32 + 5.0, 5.0)))
            .collect();
        assert_eq!(
            build_arrangement(&inputs).unwrap_err(),
            PlanarError::TooManyInputs {
                count: MAX_PLANAR_INPUTS + 1,
                cap: MAX_PLANAR_INPUTS
            }
        );
        assert_eq!(
            face_at_point(&inputs, (1.0, 1.0)).unwrap_err(),
            PlanarError::TooManyInputs {
                count: MAX_PLANAR_INPUTS + 1,
                cap: MAX_PLANAR_INPUTS
            }
        );
    }

    #[test]
    fn empty_geometry_is_refused() {
        assert_eq!(
            build_arrangement(&[(Vec::new(), Vec::new())]).unwrap_err(),
            PlanarError::NoGeometry
        );
    }

    #[test]
    fn face_ids_are_stable_across_calls() {
        let inputs = vec![
            input(circle(0.0, 0.0, 10.0)),
            input(circle(9.0, 0.0, 10.0)),
            input(circle(4.5, 8.0, 10.0)),
        ];
        let a = build_arrangement(&inputs).expect("arrangement");
        let b = build_arrangement(&inputs).expect("arrangement");
        let ids_a: Vec<&str> = a.faces.iter().map(|f| f.id.as_str()).collect();
        let ids_b: Vec<&str> = b.faces.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids_a, ids_b);
    }

    /// Property-style sweep over pseudo-random shape sets: the faces
    /// must tile the union exactly, carry unique ids, and each report an
    /// interior point whose containment set is its own signature.
    #[test]
    fn random_shape_sets_tile_their_union() {
        // Deterministic LCG — a fixed seed keeps a failure
        // reproducible, which a real rng would not. 300 cases keeps the
        // unit run ~2.5 s; the same sweep was taken to 1500 cases
        // locally (12 s) while the kernel was being hardened, and it was
        // this sweep that found BOTH real defects: the orientation bug
        // in `region_area` and the `heal_exterior_gaps` impostor face.
        let mut state: u64 = 0x2026_0803;
        let mut next = |lo: f32, hi: f32| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let unit = ((state >> 33) as f32) / ((1u64 << 31) as f32);
            lo + unit * (hi - lo)
        };
        for case in 0..300 {
            let count = 2 + (case % 4);
            // Mix straight-edged and curved inputs so the sweep covers
            // line/line, line/curve AND curve/curve crossings.
            let inputs: Vec<(Vec<PathAnchor>, Vec<usize>)> = (0..count)
                .map(|k| {
                    let x = next(0.0, 40.0);
                    let y = next(0.0, 40.0);
                    if (case + k) % 2 == 0 {
                        let w = next(8.0, 30.0);
                        let h = next(8.0, 30.0);
                        input(rect(x, y, x + w, y + h))
                    } else {
                        input(circle(x, y, next(6.0, 18.0)))
                    }
                })
                .collect();
            let arr = build_arrangement(&inputs).expect("arrangement");
            // 1. The faces tile the union: nothing missed, nothing extra.
            assert!(
                arr.complete,
                "case {case}: faces {} vs union {}",
                arr.faces_area, arr.union_area
            );
            // 2. Ids are unique — they are how callers address a face.
            let mut ids: Vec<&str> = arr.faces.iter().map(|f| f.id.as_str()).collect();
            ids.sort_unstable();
            let unique = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), unique, "case {case}: duplicate face id");
            // 3. Every face's reported interior point really is inside
            //    it, and its signature is the containment set there.
            for face in &arr.faces {
                let hit = face_at_point(&inputs, face.inside)
                    .expect("query")
                    .unwrap_or_else(|| {
                        panic!("case {case}: {} has no face at {:?}", face.id, face.inside)
                    });
                assert_eq!(hit.id, face.id, "case {case}");
                assert_eq!(hit.signature, face.signature, "case {case}");
            }
            // 4. Disjointness follows from (1): every face lies inside
            //    the union, so faces summing to exactly the union area
            //    cannot overlap. The explicit pairwise boolean check
            //    lives in `faces_are_disjoint_and_tile_the_union` on
            //    fixed geometry — running N² `path_intersect`s over
            //    hundreds of random cases trips a LATENT flo_curves 0.8
            //    bug (see the residual note on `materialize`).
        }
    }
}
