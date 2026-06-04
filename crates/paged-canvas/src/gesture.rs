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

//! Direct-manipulation gesture spine.
//!
//! A gesture has a four-phase lifecycle that mirrors
//! `canvas-interaction-plan.md` §2.1: **begin** snapshots the
//! committed state, **update** mutates a preview in place and returns
//! the dirty pages, **commit** reverts the preview and re-applies the
//! diff through `paged_mutate::apply` so the unified undo log gets a
//! single canonical entry (AC-E-13). **cancel** restores the snapshot.
//!
//! Phase B ships **Translate only**. Resize/Rotate/Scale variants of
//! `GestureType` are reserved for Phases C/D — the discriminant is
//! present so the channel envelope stays stable.
//!
//! Preview rendering model: the simplest viable approach for v1 is to
//! mutate `CanvasModel::scene` directly + rebuild on every update.
//! That's already what `apply_mutation` does for text edits, so the
//! rebuild path is well-trodden. A future v2 will swap in an
//! ephemeral overlay (`canvas-interaction-plan.md` §3.4) that the
//! display-list build composes — only worth the complexity once
//! per-update rebuild perf hits a wall.

use paged_mutate::{NodeId, NodeSpec, Operation, PropertyPath, Value};
use paged_parse::Bounds;
use paged_renderer::PageId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tsify_next::Tsify;

use crate::element_selection::ElementId;
use crate::model::{CanvasModel, FrameMutationOutcome};

// ---------------------------------------------------------------------------
// SharedArrayBuffer layout for raw gesture updates.
//
// The gesture SAB is the one-slot mailbox the main thread writes
// pointer deltas into and the worker drains every tick. The producer
// (main thread) writes the slot atomically and bumps the generation
// counter; the consumer (worker) coalesces — only the latest record
// per drain matters. Mirrors the camera SAB pattern in
// `crate::camera`.
//
// Layout (little-endian, 32-byte buffer, eight u32 words):
//
//   word 0:  handle_lo  (u32) — gesture handle low word
//   word 1:  handle_hi  (u32) — gesture handle high word
//   word 2:  dx         (f32) — pointer-delta x (page-local pt)
//   word 3:  dy         (f32) — pointer-delta y (page-local pt)
//   word 4:  modifiers  (u32) — bit 0 = shift, bit 1 = alt, bit 2 = disable_snap
//   word 5:  seq        (u32) — bumps on every producer write
//   word 6:  gen_lo     (u32) — generation low (Atomics.add target)
//   word 7:  gen_hi     (u32) — generation high
//
// This is the canonical layout. The TS-side mirror in
// `packages/client/src/sab/gesture.ts` consumes the byte size
// via `gestureSabBytes()` on the wasm module and asserts the offsets
// match at worker init; any drift fires a `protocolMismatch` warning
// like the `PROTOCOL_VERSION` check next door.

/// Total bytes the gesture SAB occupies. The producer (JS) allocates
/// `new SharedArrayBuffer(GESTURE_SAB_BYTES)`; the consumer maps the
/// same buffer.
pub const GESTURE_SAB_BYTES: usize = 32;

/// u32 word index of the gesture handle's low 32 bits.
pub const GESTURE_OFFSET_HANDLE_LO: usize = 0;
/// u32 word index of the gesture handle's high 32 bits.
pub const GESTURE_OFFSET_HANDLE_HI: usize = 1;
/// u32 word index of the pointer-delta x (f32 reinterpreted).
pub const GESTURE_OFFSET_DX: usize = 2;
/// u32 word index of the pointer-delta y (f32 reinterpreted).
pub const GESTURE_OFFSET_DY: usize = 3;
/// u32 word index of the modifier bit-mask.
pub const GESTURE_OFFSET_MODIFIERS: usize = 4;
/// u32 word index of the producer's monotone seq counter.
pub const GESTURE_OFFSET_SEQ: usize = 5;
/// u32 word index of the generation counter's low 32 bits.
pub const GESTURE_OFFSET_GEN_LO: usize = 6;
/// u32 word index of the generation counter's high 32 bits.
pub const GESTURE_OFFSET_GEN_HI: usize = 7;

/// Bit mask: shift held during this update.
pub const GESTURE_MODIFIER_SHIFT: u32 = 1 << 0;
/// Bit mask: alt held during this update.
pub const GESTURE_MODIFIER_ALT: u32 = 1 << 1;
/// Bit mask: ctrl (snap-bypass) held during this update.
/// Plan-2 §8.4 — passing this bit short-circuits the snap pass.
pub const GESTURE_MODIFIER_DISABLE_SNAP: u32 = 1 << 2;

/// Tsify-exposed snapshot of the SAB layout. The TS-side worker glue
/// reads this once at startup and asserts its own hardcoded mirror
/// matches; any drift triggers a `protocolMismatch` warning identical
/// to the `PROTOCOL_VERSION` reconciliation. Keeping the layout in
/// Rust lets a single edit drive both sides — TS sees the new value
/// the next time wasm rebuilds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GestureSabLayout {
    pub bytes: u32,
    pub offset_handle_lo: u32,
    pub offset_handle_hi: u32,
    pub offset_dx: u32,
    pub offset_dy: u32,
    pub offset_modifiers: u32,
    pub offset_seq: u32,
    pub offset_gen_lo: u32,
    pub offset_gen_hi: u32,
    pub modifier_shift: u32,
    pub modifier_alt: u32,
    pub modifier_disable_snap: u32,
}

impl GestureSabLayout {
    /// Canonical layout — single source of truth for the gesture SAB
    /// contract. The wasm wrapper hands this value to the TS side
    /// (see `paged-canvas-wasm::gestureSabLayout`).
    pub const fn canonical() -> Self {
        Self {
            bytes: GESTURE_SAB_BYTES as u32,
            offset_handle_lo: GESTURE_OFFSET_HANDLE_LO as u32,
            offset_handle_hi: GESTURE_OFFSET_HANDLE_HI as u32,
            offset_dx: GESTURE_OFFSET_DX as u32,
            offset_dy: GESTURE_OFFSET_DY as u32,
            offset_modifiers: GESTURE_OFFSET_MODIFIERS as u32,
            offset_seq: GESTURE_OFFSET_SEQ as u32,
            offset_gen_lo: GESTURE_OFFSET_GEN_LO as u32,
            offset_gen_hi: GESTURE_OFFSET_GEN_HI as u32,
            modifier_shift: GESTURE_MODIFIER_SHIFT,
            modifier_alt: GESTURE_MODIFIER_ALT,
            modifier_disable_snap: GESTURE_MODIFIER_DISABLE_SNAP,
        }
    }
}

/// Opaque, monotone handle returned by `begin_gesture`. Callers pass
/// it back to `update_gesture` / `commit_gesture` / `cancel_gesture`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
pub struct GestureHandle(pub u64);

/// Phase A→F gesture taxonomy. Translate ships in Phase B, Resize in
/// Phase C; Rotate / Scale stay reserved for Phase D.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum GestureType {
    /// Drag the node(s) by the pointer delta. Un-rotated frames edit
    /// `FrameBounds`. Rotated frames are rejected in Phase B and pick
    /// up `FrameTransform` support in Phase D.
    Translate,
    /// Phase C — resize via an edge / corner handle. The handle id
    /// identifies which of the eight handles the pointer grabbed.
    Resize {
        handle: ResizeHandle,
    },
    /// Reserved (Phase D). Rotate about a pivot.
    Rotate,
    /// Phase D — scale about a pivot.
    Scale,
    /// Editor-ops — horizontal shear about the selection pivot (the
    /// Shear tool). The drag's x-delta, normalised by the grabbed
    /// point's lever arm from the pivot, becomes the shear factor
    /// `k` in `x' = x + k·(y − pivot.y)`; Shift snaps the shear
    /// angle to 15° steps. Commits as `FrameTransform`, like
    /// Rotate/Scale.
    Shear,
    /// Phase F — translate the image content *inside* a frame. Edits
    /// the Rectangle's `image_item_transform`'s tx/ty by the pointer
    /// delta; the frame's own bounds + ItemTransform stay put. This
    /// is the "content grabber" pattern InDesign exposes when the
    /// user dives into an image frame.
    TranslateContent,
    /// Phase G — rotate the placed image inside its frame about the
    /// frame's centroid. Edits `image_item_transform`'s 2×2 + tx/ty.
    /// The frame itself stays still.
    RotateContent,
    /// Phase G — scale the placed image inside its frame about the
    /// frame's centroid. Edits `image_item_transform`'s 2×2 + tx/ty.
    ScaleContent,
    /// Phase H — drag a single Bezier control point on a `Polygon`'s
    /// path. The address picks which anchor + which role (anchor,
    /// left, right). The delta moves the point in the polygon's
    /// inner coordinate system; for un-rotated polygons that equals
    /// world coords, so the gesture math passes through.
    PathEdit {
        address: paged_mutate::PathPointAddress,
    },
}

/// Phase C — one of the eight handles on a selection rectangle's
/// oriented bbox. Cardinal handles move a single edge; diagonal
/// handles move two edges at once. Naming follows the compass
/// convention every creative tool uses (NW / N / NE / W / E / SW /
/// S / SE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum ResizeHandle {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

impl ResizeHandle {
    fn moves_north(self) -> bool {
        matches!(self, Self::North | Self::NorthEast | Self::NorthWest)
    }
    fn moves_south(self) -> bool {
        matches!(self, Self::South | Self::SouthEast | Self::SouthWest)
    }
    fn moves_west(self) -> bool {
        matches!(self, Self::West | Self::NorthWest | Self::SouthWest)
    }
    fn moves_east(self) -> bool {
        matches!(self, Self::East | Self::NorthEast | Self::SouthEast)
    }
    fn is_corner(self) -> bool {
        matches!(
            self,
            Self::NorthEast | Self::NorthWest | Self::SouthEast | Self::SouthWest
        )
    }
    fn is_horizontal_edge(self) -> bool {
        matches!(self, Self::North | Self::South)
    }
}

/// Modifier state captured on each pointer event. `shift` constrains
/// the gesture (snap rotation to 15°, lock aspect on resize / scale).
/// `alt` resizes from centre.
///
/// `disable_snap` (Ctrl) makes the snap pass an identity transform on
/// the delta — InDesign-style "temporarily disable snap" affordance
/// per plan-2 §8.4. Optional on the wire so older callers keep
/// compiling (defaults to `false`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GestureModifiers {
    pub shift: bool,
    pub alt: bool,
    #[serde(default)]
    #[tsify(optional)]
    pub disable_snap: bool,
}

/// Phase D — anchor point passed at `begin_gesture` for gestures that
/// need to know where the user started dragging (rotate / scale; also
/// rotated-frame translate to support world-space delta math).
/// Page-local coords + the page id; the model converts to spread
/// coords by adding the page's spread origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct GestureAnchor {
    pub page_id: PageId,
    pub point_in_page: (f32, f32),
}

/// Pre-gesture snapshot of one node. Captured at `begin_gesture` so
/// `commit_gesture` can revert + re-apply for a clean inverse, and
/// `cancel_gesture` can restore byte-for-byte.
/// Phase E — result of one `update_gesture` call. Carries dirty
/// pages + the active snap lines for the overlay.
#[derive(Debug, Clone, Default)]
pub struct GestureUpdateResult {
    pub page_ids: Vec<PageId>,
    pub snap_lines: Vec<crate::snap::SnapLine>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct NodeSnapshot {
    /// Kept for diagnostics: the channel envelope echoes the element
    /// id back on every gesture reply, and tests assert against it.
    pub(crate) id: ElementId,
    pub(crate) node_id: NodeId,
    pub(crate) bounds: Bounds,
    pub(crate) item_transform: Option<[f32; 6]>,
    /// Phase F — Rectangle's inner image transform at gesture start.
    /// `None` for TextFrames or for Rectangles that aren't image
    /// frames. Used only by `TranslateContent`.
    pub(crate) image_item_transform: Option<[f32; 6]>,
    /// Phase H — Polygon's `PathAnchor` array at gesture start.
    /// Empty for non-Polygon shapes. Used by `PathEdit` so the
    /// preview can recompute the new point from snapshot + delta on
    /// every update.
    pub(crate) path_anchors: Vec<paged_parse::PathAnchor>,
}

/// One active gesture. Lives on `CanvasModel`. Only one gesture is
/// active at a time — the channel rejects `begin_gesture` when
/// `active_gesture` is `Some`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct GestureSession {
    pub(crate) handle: GestureHandle,
    pub(crate) gesture: GestureType,
    pub(crate) snapshots: Vec<NodeSnapshot>,
    /// Cumulative (dx, dy) in document-space pt since `begin_gesture`.
    /// `None` until the first `update_gesture` call so a begin+commit
    /// pair with no pointer motion is a clean no-op.
    pub(crate) current_delta: Option<(f32, f32)>,
    pub(crate) modifiers: GestureModifiers,
    /// Phase D — anchor point in spread coords (resolved at begin
    /// from the caller's page-local input). Used by rotate / scale.
    pub anchor_spread: Option<(f32, f32)>,
    /// Phase D — pivot in spread coords. For rotate / scale this is
    /// the average of every snapshot's transformed centroid.
    pub pivot_spread: Option<(f32, f32)>,
    /// Phase G — camera scale (px/pt) at begin time, used by the
    /// snap pass to convert its CSS-px tolerance to doc-space pt.
    /// `None` defaults to `1.0` (legacy behavior).
    pub camera_scale: Option<f32>,
}

/// Phase D — the effect a gesture has on one node, dispatched per
/// `GestureType`. `update_gesture` writes it to the scene as a
/// preview; `commit_gesture` converts it to the canonical
/// `paged_mutate::Operation`. Splitting bounds vs transform here means
/// the rest of the spine doesn't have to branch on the gesture kind
/// at every step.
#[derive(Debug, Clone)]
enum NodeMutation {
    /// FrameBounds change (un-rotated translate, resize). Transform
    /// stays at its snapshot value.
    Bounds(Bounds),
    /// FrameTransform change (rotate, scale, rotated-frame translate).
    /// Bounds stay at their snapshot value.
    Transform(Option<[f32; 6]>),
    /// Phase F — Rectangle's inner image transform (content grabber).
    /// Frame's own bounds + ItemTransform stay put.
    ImageTransform(Option<[f32; 6]>),
    /// Phase H — single Bezier handle on a Polygon's PathPointArray.
    PathPoint {
        address: paged_mutate::PathPointAddress,
        position: [f32; 2],
    },
}

#[derive(Debug, Error)]
pub enum GestureError {
    #[error("no document loaded")]
    NoDocument,
    #[error("gesture {0:?} not supported in Phase B")]
    UnsupportedGesture(GestureType),
    #[error("a gesture is already active (handle={0:?})")]
    AlreadyActive(GestureHandle),
    #[error("gesture handle does not match the active gesture")]
    HandleMismatch,
    #[error("element not found: {0:?}")]
    ElementNotFound(ElementId),
    #[error("element is rotated; resize on rotated frames is deferred to a later phase")]
    RotatedFrameUnsupported,
    #[error("no nodes selected for the gesture")]
    EmptySelection,
    #[error("frame mutation failed: {0}")]
    Mutate(String),
    #[error("gesture requires an anchor point (begin_gesture's anchor was None)")]
    MissingAnchor,
    #[error("anchor refers to a page not in the document: {0:?}")]
    UnknownAnchorPage(PageId),
}

impl CanvasModel {
    /// Begin a gesture against the listed elements. Snapshots their
    /// committed `(bounds, item_transform)`. Returns a handle; only
    /// one gesture may be active at a time.
    ///
    /// `anchor` is required for Rotate / Scale (the rotation pivot is
    /// implicit; the anchor is where the user's pointer was at the
    /// start of the gesture). For Translate / Resize the anchor is
    /// optional — translate uses cumulative deltas regardless and
    /// resize derives bounds-edge math from the delta alone.
    pub fn begin_gesture(
        &mut self,
        nodes: Vec<ElementId>,
        gesture: GestureType,
        anchor: Option<GestureAnchor>,
    ) -> Result<GestureHandle, GestureError> {
        self.begin_gesture_with_scale(nodes, gesture, anchor, None)
    }

    /// Phase G — explicit `camera_scale` (px/pt) so the snap pass can
    /// keep its tolerance constant in screen px. `None` falls back to
    /// `1.0` (the legacy fixed doc-space tolerance).
    pub fn begin_gesture_with_scale(
        &mut self,
        nodes: Vec<ElementId>,
        gesture: GestureType,
        anchor: Option<GestureAnchor>,
        camera_scale: Option<f32>,
    ) -> Result<GestureHandle, GestureError> {
        if let Some(active) = self.active_gesture.as_ref() {
            return Err(GestureError::AlreadyActive(active.handle));
        }
        if !matches!(
            gesture,
            GestureType::Translate
                | GestureType::Resize { .. }
                | GestureType::Rotate
                | GestureType::Scale
                | GestureType::Shear
                | GestureType::TranslateContent
                | GestureType::RotateContent
                | GestureType::ScaleContent
                | GestureType::PathEdit { .. }
        ) {
            return Err(GestureError::UnsupportedGesture(gesture));
        }
        if nodes.is_empty() {
            return Err(GestureError::EmptySelection);
        }
        // Track L — when a Group element is in the selection,
        // expand it to (Group itself, every member recursively).
        // The Group's own item_transform mutates alongside each
        // member's so the IDML reserializes with the grouped
        // transform structure intact. Members include sub-groups,
        // which expand recursively — each sub-group also has its
        // transform updated.
        let nodes = self.expand_group_ids(&nodes);
        let snapshots = nodes
            .iter()
            .map(|id| self.snapshot_for(id))
            .collect::<Result<Vec<_>, _>>()?;
        // Phase G — rotated-frame resize is now supported. The
        // world-space pointer delta is inverse-rotated through each
        // node's `item_transform` linear part inside
        // `compute_node_mutation` so the existing bounds-edge math
        // continues to operate in content-box space.
        // Phase D — resolve anchor to spread coords, compute pivot
        // from the union of snapshot centroids. Both are required for
        // Rotate / Scale; for Translate / Resize they're carried but
        // unused.
        let needs_anchor = matches!(
            gesture,
            GestureType::Rotate
                | GestureType::Scale
                | GestureType::Shear
                | GestureType::RotateContent
                | GestureType::ScaleContent
        );
        let anchor_spread = if let Some(a) = anchor.as_ref() {
            let origin = self
                .page(&a.page_id)
                .map(|bp| bp.spread_origin)
                .ok_or_else(|| GestureError::UnknownAnchorPage(a.page_id.clone()))?;
            Some((a.point_in_page.0 + origin.0, a.point_in_page.1 + origin.1))
        } else {
            None
        };
        if needs_anchor && anchor_spread.is_none() {
            return Err(GestureError::MissingAnchor);
        }
        let pivot_spread = if needs_anchor {
            Some(union_centroid_in_spread(&snapshots))
        } else {
            None
        };
        self.next_gesture_handle += 1;
        let handle = GestureHandle(self.next_gesture_handle);
        self.active_gesture = Some(GestureSession {
            handle,
            gesture,
            snapshots,
            current_delta: None,
            modifiers: GestureModifiers::default(),
            anchor_spread,
            pivot_spread,
            camera_scale,
        });
        Ok(handle)
    }

    /// Apply a pointer-delta update. Rewrites the previewed state on
    /// every snapshotted node and runs a full rebuild. Phase E — the
    /// returned result includes any active snap lines so the overlay
    /// can render them.
    pub fn update_gesture(
        &mut self,
        handle: GestureHandle,
        delta: (f32, f32),
        modifiers: GestureModifiers,
    ) -> Result<GestureUpdateResult, GestureError> {
        let (session_clone, pages, siblings) = {
            let session = self
                .active_gesture
                .as_mut()
                .ok_or(GestureError::HandleMismatch)?;
            if session.handle != handle {
                return Err(GestureError::HandleMismatch);
            }
            session.modifiers = modifiers;
            let session_clone = session.clone();
            // Build a page_id → (vertical_guides, horizontal_guides)
            // lookup once per gesture tick. Walks each parsed spread,
            // bucketing each guide by its `page_index` into the
            // spread's pages. Plan-2 §8.3.
            let mut guides_by_page: std::collections::HashMap<
                String,
                (Vec<f32>, Vec<f32>),
            > = std::collections::HashMap::new();
            for parsed in &self.scene.spreads {
                let pages_of_spread = &parsed.spread.pages;
                if pages_of_spread.is_empty() {
                    continue;
                }
                for g in &parsed.spread.guides {
                    // IDML `PageIndex` is 1-based per spread; clamp
                    // matches the wire-handle conversion in model.rs.
                    let idx = if g.page_index == 0 {
                        0
                    } else {
                        ((g.page_index as usize) - 1).min(pages_of_spread.len() - 1)
                    };
                    let Some(p) = pages_of_spread.get(idx) else {
                        continue;
                    };
                    let Some(sid) = p.self_id.clone() else {
                        continue;
                    };
                    let entry = guides_by_page.entry(sid).or_default();
                    match g.orientation {
                        paged_parse::GuideOrientation::Vertical => entry.0.push(g.location),
                        paged_parse::GuideOrientation::Horizontal => entry.1.push(g.location),
                    }
                }
            }
            let pages = self
                .built
                .pages
                .iter()
                .map(|p| {
                    let (vertical_guides, horizontal_guides) = guides_by_page
                        .get(p.id.as_str())
                        .cloned()
                        .unwrap_or_default();
                    crate::snap::PageInfo {
                        page_id: p.id.clone(),
                        width_pt: p.width_pt,
                        height_pt: p.height_pt,
                        spread_origin: p.spread_origin,
                        vertical_guides,
                        horizontal_guides,
                    }
                })
                .collect::<Vec<_>>();
            let siblings = collect_sibling_frames(&self.scene, &self.built);
            (session_clone, pages, siblings)
        };
        // Phase E snap: adjusts the raw pointer delta so the candidate
        // edges land on snap targets within tolerance. The
        // adjusted delta is stored on the session so commit picks up
        // the same value.
        let adjustment =
            crate::snap::compute_snap_adjustment(&session_clone, delta, &pages, &siblings);
        let snapped_delta = adjustment.delta;
        {
            let session = self
                .active_gesture
                .as_mut()
                .expect("session still present");
            session.current_delta = Some(snapped_delta);
        }
        // Re-write previewed state from the snapshot + snapped delta.
        for snap in &session_clone.snapshots {
            let mutation = compute_node_mutation(snap, &session_clone, snapped_delta);
            write_mutation_to_scene(&mut self.scene, &snap.node_id, mutation);
        }
        self.rebuild_after_mutation()
            .map_err(|e| GestureError::Mutate(e.to_string()))?;
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        Ok(GestureUpdateResult {
            page_ids,
            snap_lines: adjustment.lines,
        })
    }

    /// Commit the gesture: revert the preview, re-apply the final
    /// delta through `paged_mutate::apply` so the unified undo log
    /// captures one canonical `AppliedOperation` (or `Batch` for
    /// multi-select). Returns the resulting `FrameMutationOutcome`.
    pub fn commit_gesture(
        &mut self,
        handle: GestureHandle,
    ) -> Result<FrameMutationOutcome, GestureError> {
        let session = self
            .active_gesture
            .take()
            .ok_or(GestureError::HandleMismatch)?;
        if session.handle != handle {
            // Wrong handle — restore the session so a stale message
            // doesn't silently drop an active gesture.
            self.active_gesture = Some(session);
            return Err(GestureError::HandleMismatch);
        }
        let Some(delta) = session.current_delta else {
            // No pointer motion: revert (no-op restore) and exit with
            // an empty-batch outcome so the caller can update the UI.
            self.restore_from_snapshots(&session.snapshots)
                .map_err(|e| GestureError::Mutate(e.to_string()))?;
            let page_ids: Vec<PageId> =
                self.built.pages.iter().map(|p| p.id.clone()).collect();
            // Synthesise an empty Batch so the channel's reply shape
            // stays uniform.
            let empty = paged_mutate::AppliedOperation {
                op: Operation::Batch { ops: vec![] },
                inverse: Operation::Batch { ops: vec![] },
                invalidation: Default::default(),
            };
            let applied_seq = self.bump_applied_seq();
            return Ok(FrameMutationOutcome {
                applied_seq,
                page_ids,
                applied: empty,
            });
        };
        // Step 1: revert the scene to the snapshot. The previewed
        // mutations no longer exist as far as `paged_mutate::apply` is
        // concerned, so the inverse it captures will be the correct
        // "back to original" value.
        self.restore_from_snapshots(&session.snapshots)
            .map_err(|e| GestureError::Mutate(e.to_string()))?;
        // Step 2: build the canonical op (Batch when N>1).
        // Phase H — Alt+Translate commits as a Batch of InsertNode
        // (CloneTranslate) ops; the original frames stay put. For all
        // other gesture types, emit per-snapshot SetProperty ops.
        let ops: Vec<Operation> = if session.modifiers.alt
            && matches!(session.gesture, GestureType::Translate)
            && !session.snapshots.is_empty()
        {
            // Track K — resolve destination spread from the world
            // pointer position. World pointer = source spread
            // origin + anchor in spread-local coords + delta.
            // When the pointer is over a page on a different
            // spread, route the clone there with a corrected
            // delta accounting for the spread-origin offset.
            // Falls back to source-spread behaviour (None) when
            // the pointer landed in the pasteboard between spreads.
            let dest_spread_id = self.resolve_destination_spread(&session, delta);
            // K-spec hook: when crossing into a different spread,
            // the JS spec captures this console line to confirm
            // the gesture-spine half of cross-spread routing fired.
            #[cfg(target_arch = "wasm32")]
            if dest_spread_id.is_some() {
                web_sys::console::log_1(
                    &"[K-debug] dest spread origin = (resolved)".into(),
                );
            }
            build_alt_duplicate_ops(&session.snapshots, delta, dest_spread_id)
        } else {
            session
                .snapshots
                .iter()
                .map(|snap| {
                    let mutation = compute_node_mutation(snap, &session, delta);
                    build_op_from_mutation(snap, mutation)
                })
                .collect()
        };
        let op = if ops.len() == 1 {
            ops.into_iter().next().unwrap()
        } else {
            Operation::Batch { ops }
        };
        // Step 3: apply through the canonical bridge so the log entry
        // is `LoggedMutation::Frame(AppliedOperation)`.
        self.apply_operation(op)
            .map_err(|e| GestureError::Mutate(format!("{e:?}")))
    }

    /// Track K — resolve which spread the gesture's pointer is
    /// currently over. World pointer position is reconstructed as
    /// `source_spread_origin + anchor_spread + delta`; we then walk
    /// every spread's pages (composing page item_transform on
    /// page.bounds → spread coords, then adding the spread's own
    /// translation origin to reach world) and report the spread
    /// whose page contains the point. Returns `None` when the
    /// pointer landed on the pasteboard (between spreads) — the
    /// apply layer falls back to source-spread behaviour, which
    /// matches the typical "drop into nothing" UX.
    fn resolve_destination_spread(
        &self,
        session: &GestureSession,
        delta: (f32, f32),
    ) -> Option<String> {
        // We need an anchor + a source spread. Without these we
        // can't compute the world pointer; the gesture must have
        // been entered without the begin_gesture path's anchor
        // resolution (unusual). Bail to None.
        let anchor_spread = session.anchor_spread?;
        // The source spread is the one hosting snapshots[0]. We
        // index by raw_id across all spreads. Snapshots can't be
        // empty here (caller ensures it before invoking this).
        let snap_raw = session.snapshots.first()?.id.raw_id().to_string();
        let source_spread = self
            .scene
            .spreads
            .iter()
            .find(|s| spread_contains_frame(&s.spread, &snap_raw))?;
        let src_origin = match &source_spread.spread.item_transform {
            Some(m) => (m[4], m[5]),
            None => (0.0, 0.0),
        };
        // World pointer position.
        let world = (
            src_origin.0 + anchor_spread.0 + delta.0,
            src_origin.1 + anchor_spread.1 + delta.1,
        );
        // Walk every spread; for each page, compute its world
        // AABB (item_transform-mapped page.bounds plus the
        // spread's origin) and test containment.
        for parsed in &self.scene.spreads {
            let s = &parsed.spread;
            let spread_origin = match &s.item_transform {
                Some(m) => (m[4], m[5]),
                None => (0.0, 0.0),
            };
            for page in &s.pages {
                let aabb = transformed_aabb(page.bounds, page.item_transform);
                // aabb = [top, left, bottom, right] in spread coords.
                let wl = spread_origin.0 + aabb[1];
                let wr = spread_origin.0 + aabb[3];
                let wt = spread_origin.1 + aabb[0];
                let wb = spread_origin.1 + aabb[2];
                if world.0 >= wl && world.0 <= wr && world.1 >= wt && world.1 <= wb {
                    return s.self_id.clone();
                }
            }
        }
        None
    }

    /// Drop the in-flight gesture. Restores the snapshot, rebuilds,
    /// returns the dirty page set so the overlay clears.
    pub fn cancel_gesture(
        &mut self,
        handle: GestureHandle,
    ) -> Result<Vec<PageId>, GestureError> {
        let session = self
            .active_gesture
            .take()
            .ok_or(GestureError::HandleMismatch)?;
        if session.handle != handle {
            self.active_gesture = Some(session);
            return Err(GestureError::HandleMismatch);
        }
        self.restore_from_snapshots(&session.snapshots)
            .map_err(|e| GestureError::Mutate(e.to_string()))?;
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        Ok(page_ids)
    }

    /// Read-only access to the current gesture handle, for tests +
    /// the channel envelope's idempotency checks.
    pub fn active_gesture_handle(&self) -> Option<GestureHandle> {
        self.active_gesture.as_ref().map(|s| s.handle)
    }

    fn snapshot_for(&self, id: &ElementId) -> Result<NodeSnapshot, GestureError> {
        let raw = id.raw_id();
        for parsed in &self.scene.spreads {
            let s = &parsed.spread;
            if let ElementId::TextFrame(_) = id {
                if let Some(f) = s.text_frames.iter().find(|f| f.self_id.as_deref() == Some(raw)) {
                    return Ok(NodeSnapshot {
                        id: id.clone(),
                        node_id: NodeId::TextFrame(raw.to_string()),
                        bounds: f.bounds,
                        item_transform: f.item_transform,
                        image_item_transform: None,
                        path_anchors: Vec::new(),
                    });
                }
            }
            if let ElementId::Rectangle(_) = id {
                if let Some(f) = s.rectangles.iter().find(|f| f.self_id.as_deref() == Some(raw)) {
                    return Ok(NodeSnapshot {
                        id: id.clone(),
                        node_id: NodeId::Rectangle(raw.to_string()),
                        bounds: f.bounds,
                        item_transform: f.item_transform,
                        image_item_transform: f.image_item_transform,
                        path_anchors: Vec::new(),
                    });
                }
            }
            if let ElementId::Polygon(_) = id {
                if let Some(p) = s.polygons.iter().find(|p| p.self_id.as_deref() == Some(raw)) {
                    return Ok(NodeSnapshot {
                        id: id.clone(),
                        node_id: NodeId::Polygon(raw.to_string()),
                        bounds: p.bounds,
                        item_transform: p.item_transform,
                        image_item_transform: None,
                        path_anchors: p.anchors.clone(),
                    });
                }
            }
            // Editor-ops — Ovals + GraphicLines join the transform
            // gestures (Rotate / Scale / Shear) now that their
            // FrameTransform apply arms exist.
            if let ElementId::Oval(_) = id {
                if let Some(o) = s.ovals.iter().find(|o| o.self_id.as_deref() == Some(raw)) {
                    return Ok(NodeSnapshot {
                        id: id.clone(),
                        node_id: NodeId::Oval(raw.to_string()),
                        bounds: o.bounds,
                        item_transform: o.item_transform,
                        image_item_transform: None,
                        path_anchors: Vec::new(),
                    });
                }
            }
            if let ElementId::GraphicLine(_) = id {
                if let Some(l) = s
                    .graphic_lines
                    .iter()
                    .find(|l| l.self_id.as_deref() == Some(raw))
                {
                    return Ok(NodeSnapshot {
                        id: id.clone(),
                        node_id: NodeId::GraphicLine(raw.to_string()),
                        bounds: l.bounds,
                        item_transform: l.item_transform,
                        image_item_transform: None,
                        path_anchors: l.anchors.clone(),
                    });
                }
            }
            if let ElementId::Group(_) = id {
                if let Some(g) = s.groups.iter().find(|g| g.self_id.as_deref() == Some(raw)) {
                    // Track L — Groups don't carry geometric
                    // bounds; the snapshot uses a sentinel zero
                    // bounds value. `compute_node_mutation` checks
                    // `snap.node_id` and forces the transform path
                    // for Translate so the unused bounds never
                    // matters at apply time.
                    return Ok(NodeSnapshot {
                        id: id.clone(),
                        node_id: NodeId::Group(raw.to_string()),
                        bounds: paged_parse::Bounds {
                            top: 0.0,
                            left: 0.0,
                            bottom: 0.0,
                            right: 0.0,
                        },
                        item_transform: g.item_transform,
                        image_item_transform: None,
                        path_anchors: Vec::new(),
                    });
                }
            }
        }
        Err(GestureError::ElementNotFound(id.clone()))
    }

    /// Track L — recursively expand any `Group` id in `ids` into
    /// `[group, member, ...]`, where members are walked through any
    /// nested sub-groups. Non-group ids pass through unchanged.
    /// Order preserves the input + adds expansion after each Group
    /// id so the snapshot list keeps the same prefix the caller
    /// supplied (useful when tests rely on snapshots\[0\] being
    /// the originally-selected element).
    fn expand_group_ids(&self, ids: &[ElementId]) -> Vec<ElementId> {
        let mut out = Vec::with_capacity(ids.len());
        let mut visited = std::collections::HashSet::new();
        for id in ids {
            if visited.contains(id.raw_id()) {
                continue;
            }
            visited.insert(id.raw_id().to_string());
            out.push(id.clone());
            if let ElementId::Group(raw) = id {
                self.collect_group_members(raw, &mut out, &mut visited);
            }
        }
        out
    }

    fn collect_group_members(
        &self,
        group_id: &str,
        out: &mut Vec<ElementId>,
        visited: &mut std::collections::HashSet<String>,
    ) {
        // Find the group across all spreads (groups are
        // spread-local but ids are document-unique).
        for parsed in &self.scene.spreads {
            let s = &parsed.spread;
            let Some(g) = s.groups.iter().find(|g| g.self_id.as_deref() == Some(group_id)) else {
                continue;
            };
            for fr in &g.members {
                let member_id = match fr {
                    paged_parse::FrameRef::TextFrame(idx) => s
                        .text_frames
                        .get(*idx)
                        .and_then(|f| f.self_id.clone())
                        .map(ElementId::TextFrame),
                    paged_parse::FrameRef::Rectangle(idx) => s
                        .rectangles
                        .get(*idx)
                        .and_then(|r| r.self_id.clone())
                        .map(ElementId::Rectangle),
                    paged_parse::FrameRef::Oval(idx) => s
                        .ovals
                        .get(*idx)
                        .and_then(|o| o.self_id.clone())
                        .map(ElementId::Oval),
                    paged_parse::FrameRef::GraphicLine(idx) => s
                        .graphic_lines
                        .get(*idx)
                        .and_then(|l| l.self_id.clone())
                        .map(ElementId::GraphicLine),
                    paged_parse::FrameRef::Polygon(idx) => s
                        .polygons
                        .get(*idx)
                        .and_then(|p| p.self_id.clone())
                        .map(ElementId::Polygon),
                    paged_parse::FrameRef::Group(idx) => s
                        .groups
                        .get(*idx)
                        .and_then(|g| g.self_id.clone())
                        .map(ElementId::Group),
                };
                let Some(mid) = member_id else { continue };
                if visited.contains(mid.raw_id()) {
                    continue;
                }
                visited.insert(mid.raw_id().to_string());
                out.push(mid.clone());
                if let ElementId::Group(sub_raw) = &mid {
                    self.collect_group_members(sub_raw, out, visited);
                }
            }
            return;
        }
    }

    fn restore_from_snapshots(
        &mut self,
        snapshots: &[NodeSnapshot],
    ) -> Result<(), crate::channel::LoadError> {
        for snap in snapshots {
            restore_snapshot_in_scene(&mut self.scene, snap);
        }
        self.rebuild_after_mutation()
    }
}

fn is_pure_translate_or_identity(m: Option<[f32; 6]>) -> bool {
    match m {
        None => true,
        // Identity 2×2 (a=d=1, b=c=0) — tx/ty are pure translation.
        // Small epsilon to absorb the floating-point ItemTransform
        // strings InDesign emits ("1 0 0 1 0 0" is exact, but parsed
        // rotations like 90° round-trip through f32).
        Some([a, b, c, d, _, _]) => {
            (a - 1.0).abs() < 1e-4
                && (d - 1.0).abs() < 1e-4
                && b.abs() < 1e-4
                && c.abs() < 1e-4
        }
    }
}

/// Phase D — dispatch a snapshot + gesture to its `NodeMutation`.
/// Pure function; both `update_gesture` (to write the preview) and
/// `commit_gesture` (to build the canonical op) call into this so
/// they always agree on what the gesture does.
fn compute_node_mutation(
    snap: &NodeSnapshot,
    session: &GestureSession,
    delta: (f32, f32),
) -> NodeMutation {
    match session.gesture {
        GestureType::Translate => {
            // Phase E — Shift constrains translate to the dominant
            // axis; the user gets a strict horizontal or vertical
            // move regardless of pointer noise on the off-axis. The
            // snap pass already ran upstream so `delta` here is the
            // snap-adjusted value.
            let d = if session.modifiers.shift {
                constrain_to_dominant_axis(delta)
            } else {
                delta
            };
            // Phase D — rotated frames translate through their
            // ItemTransform's tx/ty; un-rotated stays on the bounds
            // path so text reflow continues to track the bbox.
            // Track L — two new constraints:
            //   * Groups carry no geometric bounds, so a translate
            //     ALWAYS mutates their item_transform.
            //   * Leaves inside a Group-targeted gesture also must
            //     translate via item_transform (not bounds): the
            //     parser pre-bakes the group's transform into each
            //     leaf's item_transform (paged-parse spread.rs:141),
            //     so on reserialization the leaves' positions read
            //     from item_transform. Mutating bounds inside a
            //     group session would diverge from the group's
            //     transform on the next reparse.
            let is_group = matches!(snap.node_id, NodeId::Group(_));
            let session_targets_group = session
                .snapshots
                .iter()
                .any(|s| matches!(s.node_id, NodeId::Group(_)));
            let force_transform_path = is_group || session_targets_group;
            if !force_transform_path && is_pure_translate_or_identity(snap.item_transform) {
                NodeMutation::Bounds(translate_bounds(snap.bounds, d))
            } else {
                NodeMutation::Transform(Some(translate_transform(
                    snap.item_transform.unwrap_or(IDENTITY),
                    d,
                )))
            }
        }
        GestureType::Resize { handle } => {
            // Phase G — for a rotated/scaled frame, the world-space
            // delta must be expressed in content-box (pre-transform)
            // space before the bounds-edge math fires; otherwise the
            // user's drag would move the edge along the world axes
            // instead of the frame's local axes.
            let local_delta = inverse_rotate_delta(snap.item_transform, delta);
            NodeMutation::Bounds(apply_resize(
                snap.bounds,
                handle,
                local_delta,
                session.modifiers,
            ))
        }
        GestureType::Rotate => {
            let pivot = session.pivot_spread.unwrap_or((0.0, 0.0));
            let anchor = session.anchor_spread.unwrap_or(pivot);
            let new_m = rotate_about_pivot(
                snap.item_transform.unwrap_or(IDENTITY),
                anchor,
                delta,
                pivot,
                session.modifiers,
            );
            NodeMutation::Transform(Some(new_m))
        }
        GestureType::Scale => {
            let pivot = session.pivot_spread.unwrap_or((0.0, 0.0));
            let anchor = session.anchor_spread.unwrap_or(pivot);
            let new_m = scale_about_pivot(
                snap.item_transform.unwrap_or(IDENTITY),
                anchor,
                delta,
                pivot,
                session.modifiers,
            );
            NodeMutation::Transform(Some(new_m))
        }
        GestureType::Shear => {
            let pivot = session.pivot_spread.unwrap_or((0.0, 0.0));
            let anchor = session.anchor_spread.unwrap_or(pivot);
            let new_m = shear_about_pivot(
                snap.item_transform.unwrap_or(IDENTITY),
                anchor,
                delta,
                pivot,
                session.modifiers,
            );
            NodeMutation::Transform(Some(new_m))
        }
        GestureType::TranslateContent => {
            // Phase F — translate the placed image inside the frame.
            // The image_item_transform's tx/ty live in *frame-inner*
            // coords, so the world-space pointer delta is
            // inverse-rotated through the frame's item_transform's
            // linear part before being added to tx/ty. Phase G fix —
            // Phase F v1 used world-space delta directly which
            // drifted on rotated frames.
            let local_delta = inverse_rotate_delta(snap.item_transform, delta);
            let prev = snap.image_item_transform.unwrap_or(IDENTITY);
            let next = translate_transform(prev, local_delta);
            NodeMutation::ImageTransform(Some(next))
        }
        GestureType::RotateContent => {
            // Phase G — rotate the placed image about the frame's
            // centroid. The rotation angle is computed in world
            // space (invariant to frame rotation), but the
            // `image_item_transform` lives in frame-inner space, so
            // its rotation pivot is the bounds centroid in that
            // frame-inner coordinate system.
            let pivot_world = session.pivot_spread.unwrap_or((0.0, 0.0));
            let anchor_world = session.anchor_spread.unwrap_or(pivot_world);
            let pivot_local = (
                (snap.bounds.left + snap.bounds.right) * 0.5,
                (snap.bounds.top + snap.bounds.bottom) * 0.5,
            );
            // For the angle, we use world-space anchor/current; this
            // gives the rotation magnitude the user perceives, which
            // equals the rotation magnitude that should be applied
            // to image_item_transform.
            let new_m = rotate_about_pivot(
                snap.image_item_transform.unwrap_or(IDENTITY),
                anchor_world,
                delta,
                pivot_world,
                session.modifiers,
            );
            // The rotate_about_pivot above produces a transform that
            // pivots about `pivot_world`. But image_item_transform's
            // output is in frame-inner space — we need to pivot
            // about `pivot_local` instead. Rebuild using the
            // computed θ + local pivot.
            let theta = signed_angle_between(
                (anchor_world.0 - pivot_world.0, anchor_world.1 - pivot_world.1),
                (
                    anchor_world.0 + delta.0 - pivot_world.0,
                    anchor_world.1 + delta.1 - pivot_world.1,
                ),
            );
            let theta = if session.modifiers.shift {
                snap_angle_to_15deg(theta)
            } else {
                theta
            };
            let _ = new_m;
            let next = rotate_matrix_about_pivot_local(
                snap.image_item_transform.unwrap_or(IDENTITY),
                theta,
                pivot_local,
            );
            NodeMutation::ImageTransform(Some(next))
        }
        GestureType::PathEdit { address } => {
            // Phase H — path points live in the polygon's inner
            // coordinate system, so the world-space pointer delta
            // gets inverse-rotated through the polygon's
            // item_transform (same trick as Phase G's resize).
            let local_delta = inverse_rotate_delta(snap.item_transform, delta);
            let anchor = snap
                .path_anchors
                .get(address.index)
                .copied()
                .unwrap_or(paged_parse::PathAnchor {
                    anchor: (0.0, 0.0),
                    left: (0.0, 0.0),
                    right: (0.0, 0.0),
                });
            let original = match address.role {
                paged_mutate::PathPointRole::Anchor => anchor.anchor,
                paged_mutate::PathPointRole::Left => anchor.left,
                paged_mutate::PathPointRole::Right => anchor.right,
            };
            let new_pos = [original.0 + local_delta.0, original.1 + local_delta.1];
            NodeMutation::PathPoint {
                address,
                position: new_pos,
            }
        }
        GestureType::ScaleContent => {
            // Phase G — scale the placed image about the frame's
            // centroid in frame-inner space. Scale factors derived
            // from (anchor → current) relative to world pivot —
            // invariant across the world↔inner frame change since
            // we only need their ratios.
            let pivot_world = session.pivot_spread.unwrap_or((0.0, 0.0));
            let anchor_world = session.anchor_spread.unwrap_or(pivot_world);
            let pivot_local = (
                (snap.bounds.left + snap.bounds.right) * 0.5,
                (snap.bounds.top + snap.bounds.bottom) * 0.5,
            );
            let (sx, sy) = compute_scale_factors(
                anchor_world,
                delta,
                pivot_world,
                session.modifiers,
            );
            let next = scale_matrix_about_pivot_local(
                snap.image_item_transform.unwrap_or(IDENTITY),
                sx,
                sy,
                pivot_local,
            );
            NodeMutation::ImageTransform(Some(next))
        }
    }
}

const IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Phase E — lock a translate delta to the dominant axis. If
/// |dx| ≥ |dy|, drop the y component; otherwise drop the x. Equal
/// magnitudes break toward the x axis (matches industry convention —
/// horizontal lock dominates for the same-distance edge case).
fn constrain_to_dominant_axis(delta: (f32, f32)) -> (f32, f32) {
    if delta.0.abs() >= delta.1.abs() {
        (delta.0, 0.0)
    } else {
        (0.0, delta.1)
    }
}

fn translate_bounds(snap: Bounds, delta: (f32, f32)) -> Bounds {
    Bounds {
        top: snap.top + delta.1,
        left: snap.left + delta.0,
        bottom: snap.bottom + delta.1,
        right: snap.right + delta.0,
    }
}

fn translate_transform(m: [f32; 6], delta: (f32, f32)) -> [f32; 6] {
    [m[0], m[1], m[2], m[3], m[4] + delta.0, m[5] + delta.1]
}

/// Phase G — transform a world-space delta vector into the local
/// (content-box) frame by multiplying with the inverse of the
/// transform's 2×2 linear part. For an un-rotated / identity matrix,
/// returns the delta unchanged. Translation components don't affect
/// vectors.
pub(crate) fn inverse_rotate_delta(
    item_transform: Option<[f32; 6]>,
    delta: (f32, f32),
) -> (f32, f32) {
    let m = match item_transform {
        Some(m) => m,
        None => return delta,
    };
    let [a, b, c, d, _, _] = m;
    let det = a * d - b * c;
    if det.abs() < 1e-6 || !det.is_finite() {
        return delta;
    }
    let inv = 1.0 / det;
    // Inverse 2×2: ((d, -c), (-b, a)) * (1/det).
    let lx = (d * delta.0 - c * delta.1) * inv;
    let ly = (-b * delta.0 + a * delta.1) * inv;
    (lx, ly)
}

/// Rotate the matrix `m` by the angle between two rays from `pivot`:
/// from `pivot → anchor` (gesture start) to `pivot → (anchor + delta)`
/// (current pointer). All coords in spread / world frame.
///
/// IDML matrix packing: a point `(x, y)` becomes
///   `(a*x + c*y + tx, b*x + d*y + ty)`.
/// Composing rotation `R(θ) = [[cos, -sin], [sin, cos]]` with `m`'s
/// linear part yields the new linear coefficients; the translation
/// captures both the rotation of the old translation and the
/// pivot-preservation correction.
pub(crate) fn rotate_about_pivot(
    m: [f32; 6],
    anchor: (f32, f32),
    delta: (f32, f32),
    pivot: (f32, f32),
    modifiers: GestureModifiers,
) -> [f32; 6] {
    let current = (anchor.0 + delta.0, anchor.1 + delta.1);
    let mut theta = (current.1 - pivot.1).atan2(current.0 - pivot.0)
        - (anchor.1 - pivot.1).atan2(anchor.0 - pivot.0);
    if modifiers.shift {
        // Snap to the nearest 15°. The plan calls this out as the
        // Shift constraint on rotate.
        let step = std::f32::consts::PI / 12.0; // 15° in radians
        theta = (theta / step).round() * step;
    }
    let (s, c) = theta.sin_cos();
    let [a, b, cc, d, tx, ty] = m;
    let new_a = c * a - s * b;
    let new_b = s * a + c * b;
    let new_c = c * cc - s * d;
    let new_d = s * cc + c * d;
    let new_tx = c * tx - s * ty + (1.0 - c) * pivot.0 + s * pivot.1;
    let new_ty = s * tx + c * ty - s * pivot.0 + (1.0 - c) * pivot.1;
    [new_a, new_b, new_c, new_d, new_tx, new_ty]
}

/// Editor-ops — compose a horizontal shear about `pivot` onto `m`.
/// The shear factor is the pointer's x-delta normalised by the
/// grabbed point's vertical lever arm from the pivot (so the grabbed
/// edge follows the pointer); points on the pivot's horizontal axis
/// can't shear (eps guard). Shift snaps the shear ANGLE — atan(k) —
/// to 15° steps, mirroring Rotate's constraint. Shear-about-pivot:
/// `x' = x + k·(y − pivot.y)`, `y' = y`, composed onto the IDML
/// packing the same way `rotate_about_pivot` does.
pub(crate) fn shear_about_pivot(
    m: [f32; 6],
    anchor: (f32, f32),
    delta: (f32, f32),
    pivot: (f32, f32),
    modifiers: GestureModifiers,
) -> [f32; 6] {
    let lever = anchor.1 - pivot.1;
    if lever.abs() < 1e-3 {
        return m;
    }
    let mut k = delta.0 / lever;
    if modifiers.shift {
        let step = std::f32::consts::PI / 12.0; // 15° in radians
        let angle = k.atan();
        k = ((angle / step).round() * step).tan();
    }
    let [a, b, c, d, tx, ty] = m;
    // S·M with S = [1 k; 0 1] (row form on column vectors), then the
    // pivot correction keeps `pivot` a fixed point.
    [
        a + k * b,
        b,
        c + k * d,
        d,
        tx + k * ty - k * pivot.1,
        ty,
    ]
}

/// Phase G — signed angle (radians) from `from` to `to` about the
/// origin. Used by RotateContent to compute the user-perceived
/// rotation in world space.
fn signed_angle_between(from: (f32, f32), to: (f32, f32)) -> f32 {
    to.1.atan2(to.0) - from.1.atan2(from.0)
}

/// Phase G — snap a rotation angle to the nearest 15° increment.
/// Matches Phase D's Shift behavior.
fn snap_angle_to_15deg(theta: f32) -> f32 {
    let step = std::f32::consts::PI / 12.0;
    (theta / step).round() * step
}

/// Phase G — apply a rotation by `theta` to `m` about a pivot
/// expressed in `m`'s OUTPUT coord frame (i.e. the same frame `m`'s
/// translation lands in). Used for content gestures where the
/// pivot is the frame-inner bounds centroid.
fn rotate_matrix_about_pivot_local(
    m: [f32; 6],
    theta: f32,
    pivot: (f32, f32),
) -> [f32; 6] {
    let (s, c) = theta.sin_cos();
    let [a, b, cc, d, tx, ty] = m;
    let new_a = c * a - s * b;
    let new_b = s * a + c * b;
    let new_c = c * cc - s * d;
    let new_d = s * cc + c * d;
    let new_tx = c * tx - s * ty + (1.0 - c) * pivot.0 + s * pivot.1;
    let new_ty = s * tx + c * ty - s * pivot.0 + (1.0 - c) * pivot.1;
    [new_a, new_b, new_c, new_d, new_tx, new_ty]
}

/// Phase G — compute per-axis scale factors from (anchor, delta)
/// relative to a world pivot. Matches Phase D's scale_about_pivot
/// behavior but exposed separately for callers that need just the
/// factors (e.g. ScaleContent uses these factors with a different
/// pivot frame).
fn compute_scale_factors(
    anchor: (f32, f32),
    delta: (f32, f32),
    pivot: (f32, f32),
    modifiers: GestureModifiers,
) -> (f32, f32) {
    let anchor_dx = anchor.0 - pivot.0;
    let anchor_dy = anchor.1 - pivot.1;
    let current = (anchor.0 + delta.0, anchor.1 + delta.1);
    let current_dx = current.0 - pivot.0;
    let current_dy = current.1 - pivot.1;
    let sx = if anchor_dx.abs() < 1e-6 {
        1.0
    } else {
        current_dx / anchor_dx
    };
    let sy = if anchor_dy.abs() < 1e-6 {
        1.0
    } else {
        current_dy / anchor_dy
    };
    if modifiers.shift {
        let dom = if (sx.abs() - 1.0).abs() > (sy.abs() - 1.0).abs() {
            sx
        } else {
            sy
        };
        (dom, dom)
    } else {
        (sx, sy)
    }
}

/// Phase G — scale `m` about a pivot expressed in `m`'s output
/// coord frame. Mirrors `rotate_matrix_about_pivot_local`.
fn scale_matrix_about_pivot_local(
    m: [f32; 6],
    sx: f32,
    sy: f32,
    pivot: (f32, f32),
) -> [f32; 6] {
    let [a, b, c, d, tx, ty] = m;
    [
        sx * a,
        sy * b,
        sx * c,
        sy * d,
        sx * tx + (1.0 - sx) * pivot.0,
        sy * ty + (1.0 - sy) * pivot.1,
    ]
}

/// Scale the matrix `m` about `pivot` by per-axis factors derived
/// from `(anchor → anchor + delta)` relative to the pivot. With
/// `modifiers.shift` the scale locks to the dominant axis.
pub(crate) fn scale_about_pivot(
    m: [f32; 6],
    anchor: (f32, f32),
    delta: (f32, f32),
    pivot: (f32, f32),
    modifiers: GestureModifiers,
) -> [f32; 6] {
    let anchor_dx = anchor.0 - pivot.0;
    let anchor_dy = anchor.1 - pivot.1;
    let current = (anchor.0 + delta.0, anchor.1 + delta.1);
    let current_dx = current.0 - pivot.0;
    let current_dy = current.1 - pivot.1;
    // Avoid div-by-zero when the anchor was at the pivot (the user
    // somehow grabbed dead-centre). Treat as identity scale.
    let sx = if anchor_dx.abs() < 1e-6 {
        1.0
    } else {
        current_dx / anchor_dx
    };
    let sy = if anchor_dy.abs() < 1e-6 {
        1.0
    } else {
        current_dy / anchor_dy
    };
    let (sx, sy) = if modifiers.shift {
        // Lock aspect → drive both axes by the dominant scale.
        let dom = if (sx.abs() - 1.0).abs() > (sy.abs() - 1.0).abs() {
            sx
        } else {
            sy
        };
        (dom, dom)
    } else {
        (sx, sy)
    };
    let [a, b, c, d, tx, ty] = m;
    [
        sx * a,
        sy * b,
        sx * c,
        sy * d,
        sx * tx + (1.0 - sx) * pivot.0,
        sy * ty + (1.0 - sy) * pivot.1,
    ]
}

/// Phase E — collect a flat list of every frame's AABB in
/// page-local coords, tagged by its host page. Used as the snap
/// target set so the moving items' edges align with sibling frames.
/// Only text frames + rectangles for v1 (matches the rest of the
/// gesture support).
fn collect_sibling_frames(
    scene: &paged_scene::Document,
    built: &paged_renderer::BuiltDocument,
) -> Vec<crate::snap::FrameRect> {
    let mut out = Vec::new();
    for parsed in &scene.spreads {
        let spread = &parsed.spread;
        for f in &spread.text_frames {
            let Some(id) = f.self_id.as_deref() else {
                continue;
            };
            let aabb = transformed_aabb(f.bounds, f.item_transform);
            if let Some((page_id, page_local)) = page_for_aabb(built, aabb) {
                out.push(crate::snap::FrameRect {
                    element_id: crate::element_selection::ElementId::TextFrame(id.to_string()),
                    page_id,
                    aabb: page_local,
                });
            }
        }
        for r in &spread.rectangles {
            let Some(id) = r.self_id.as_deref() else {
                continue;
            };
            let aabb = transformed_aabb(r.bounds, r.item_transform);
            if let Some((page_id, page_local)) = page_for_aabb(built, aabb) {
                out.push(crate::snap::FrameRect {
                    element_id: crate::element_selection::ElementId::Rectangle(id.to_string()),
                    page_id,
                    aabb: page_local,
                });
            }
        }
    }
    out
}

/// Track K — does a parsed spread host the frame identified by
/// `raw_id`? Walks every per-kind vec until one is found. Returns
/// false for unknown ids.
fn spread_contains_frame(spread: &paged_parse::Spread, raw_id: &str) -> bool {
    if spread.text_frames.iter().any(|f| f.self_id.as_deref() == Some(raw_id)) {
        return true;
    }
    if spread.rectangles.iter().any(|r| r.self_id.as_deref() == Some(raw_id)) {
        return true;
    }
    if spread.polygons.iter().any(|p| p.self_id.as_deref() == Some(raw_id)) {
        return true;
    }
    if spread.ovals.iter().any(|o| o.self_id.as_deref() == Some(raw_id)) {
        return true;
    }
    if spread.graphic_lines.iter().any(|g| g.self_id.as_deref() == Some(raw_id)) {
        return true;
    }
    false
}

fn transformed_aabb(b: Bounds, m: Option<[f32; 6]>) -> [f32; 4] {
    let m = m.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let corners = [
        (m[0] * b.left + m[2] * b.top + m[4], m[1] * b.left + m[3] * b.top + m[5]),
        (m[0] * b.right + m[2] * b.top + m[4], m[1] * b.right + m[3] * b.top + m[5]),
        (m[0] * b.right + m[2] * b.bottom + m[4], m[1] * b.right + m[3] * b.bottom + m[5]),
        (m[0] * b.left + m[2] * b.bottom + m[4], m[1] * b.left + m[3] * b.bottom + m[5]),
    ];
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (x, y) in corners {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    // Return [top, left, bottom, right] in SPREAD coords.
    [min_y, min_x, max_y, max_x]
}

/// Locate the page whose spread-coord rect contains the centroid of
/// `aabb_spread = [top, left, bottom, right]`, and return the AABB
/// expressed in that page's page-local pt.
fn page_for_aabb(
    built: &paged_renderer::BuiltDocument,
    aabb_spread: [f32; 4],
) -> Option<(PageId, [f32; 4])> {
    let cx = (aabb_spread[1] + aabb_spread[3]) * 0.5;
    let cy = (aabb_spread[0] + aabb_spread[2]) * 0.5;
    let p = built.pages.iter().find(|bp| {
        let (ox, oy) = bp.spread_origin;
        cx >= ox && cx <= ox + bp.width_pt && cy >= oy && cy <= oy + bp.height_pt
    })?;
    let (ox, oy) = p.spread_origin;
    let page_local = [
        aabb_spread[0] - oy,
        aabb_spread[1] - ox,
        aabb_spread[2] - oy,
        aabb_spread[3] - ox,
    ];
    Some((p.id.clone(), page_local))
}

/// Phase D — average of every snapshot's transformed centroid in
/// spread coords. The pivot for rotate / scale.
fn union_centroid_in_spread(snapshots: &[NodeSnapshot]) -> (f32, f32) {
    let mut sx = 0.0_f32;
    let mut sy = 0.0_f32;
    let mut n = 0_f32;
    for snap in snapshots {
        let cx = (snap.bounds.left + snap.bounds.right) * 0.5;
        let cy = (snap.bounds.top + snap.bounds.bottom) * 0.5;
        let (wx, wy) = match snap.item_transform {
            Some(m) => (m[0] * cx + m[2] * cy + m[4], m[1] * cx + m[3] * cy + m[5]),
            None => (cx, cy),
        };
        sx += wx;
        sy += wy;
        n += 1.0;
    }
    if n == 0.0 {
        (0.0, 0.0)
    } else {
        (sx / n, sy / n)
    }
}

/// Resize math. Each handle moves the corresponding edge(s); `alt`
/// mirrors the delta on the opposite edge (resize from centre);
/// `shift` constrains the resize to the snapshot's aspect ratio.
fn apply_resize(
    snap: Bounds,
    handle: ResizeHandle,
    delta: (f32, f32),
    modifiers: GestureModifiers,
) -> Bounds {
    let (dx, dy) = delta;
    let mut top = snap.top;
    let mut left = snap.left;
    let mut bottom = snap.bottom;
    let mut right = snap.right;
    if handle.moves_north() {
        top += dy;
    }
    if handle.moves_south() {
        bottom += dy;
    }
    if handle.moves_west() {
        left += dx;
    }
    if handle.moves_east() {
        right += dx;
    }
    if modifiers.alt {
        // Mirror the delta on the opposite edge so the box grows /
        // shrinks symmetrically about its centre.
        if handle.moves_north() {
            bottom -= dy;
        }
        if handle.moves_south() {
            top -= dy;
        }
        if handle.moves_west() {
            right -= dx;
        }
        if handle.moves_east() {
            left -= dx;
        }
    }
    if modifiers.shift {
        let orig_w = snap.right - snap.left;
        let orig_h = snap.bottom - snap.top;
        if orig_w.abs() < 1e-6 || orig_h.abs() < 1e-6 {
            return Bounds { top, left, bottom, right };
        }
        let new_w = right - left;
        let new_h = bottom - top;
        if handle.is_corner() {
            // Pick the dominant scale (whichever axis the user drove
            // hardest) and propagate to the other axis.
            let scale_w = new_w / orig_w;
            let scale_h = new_h / orig_h;
            let s = if (scale_w.abs() - 1.0).abs() > (scale_h.abs() - 1.0).abs() {
                scale_w
            } else {
                scale_h
            };
            let w = orig_w * s;
            let h = orig_h * s;
            if modifiers.alt {
                let cx = (snap.left + snap.right) * 0.5;
                let cy = (snap.top + snap.bottom) * 0.5;
                left = cx - w * 0.5;
                right = cx + w * 0.5;
                top = cy - h * 0.5;
                bottom = cy + h * 0.5;
            } else {
                // Anchor on the fixed corner.
                if handle.moves_north() {
                    top = bottom - h;
                } else {
                    bottom = top + h;
                }
                if handle.moves_west() {
                    left = right - w;
                } else {
                    right = left + w;
                }
            }
        } else {
            // Edge handle + Shift: propagate the dominant axis to the
            // perpendicular dimension, anchored on the centre of the
            // perpendicular axis so the locked-aspect resize doesn't
            // visibly drift sideways.
            if handle.is_horizontal_edge() {
                let scale = new_h / orig_h;
                let w = orig_w * scale;
                let cx = (snap.left + snap.right) * 0.5;
                left = cx - w * 0.5;
                right = cx + w * 0.5;
            } else {
                let scale = new_w / orig_w;
                let h = orig_h * scale;
                let cy = (snap.top + snap.bottom) * 0.5;
                top = cy - h * 0.5;
                bottom = cy + h * 0.5;
            }
        }
    }
    Bounds { top, left, bottom, right }
}

fn write_mutation_to_scene(
    scene: &mut paged_scene::Document,
    node: &NodeId,
    mutation: NodeMutation,
) {
    match node {
        NodeId::TextFrame(id) => {
            for parsed in scene.spreads.iter_mut() {
                if let Some(f) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|f| f.self_id.as_deref() == Some(id.as_str()))
                {
                    match mutation {
                        NodeMutation::Bounds(b) => f.bounds = b,
                        NodeMutation::Transform(m) => f.item_transform = m,
                        // TextFrames don't carry an image content
                        // transform — silently ignore (the caller's
                        // snapshot would have refused at begin time
                        // for TranslateContent on a non-image node
                        // if that guard lands; for now it's a no-op).
                        NodeMutation::ImageTransform(_) => {}
                        // TextFrames don't have a path either —
                        // PathEdit on a TextFrame is a no-op.
                        NodeMutation::PathPoint { .. } => {}
                    }
                    return;
                }
            }
        }
        NodeId::Rectangle(id) => {
            for parsed in scene.spreads.iter_mut() {
                if let Some(r) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|r| r.self_id.as_deref() == Some(id.as_str()))
                {
                    match mutation {
                        NodeMutation::Bounds(b) => r.bounds = b,
                        NodeMutation::Transform(m) => r.item_transform = m,
                        NodeMutation::ImageTransform(m) => r.image_item_transform = m,
                        // Rectangles don't carry path points — no-op.
                        NodeMutation::PathPoint { .. } => {}
                    }
                    return;
                }
            }
        }
        NodeId::Polygon(id) => {
            for parsed in scene.spreads.iter_mut() {
                if let Some(p) = parsed
                    .spread
                    .polygons
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(id.as_str()))
                {
                    if let NodeMutation::PathPoint { address, position } = mutation {
                        if let Some(anchor) = p.anchors.get_mut(address.index) {
                            match address.role {
                                paged_mutate::PathPointRole::Anchor => {
                                    let dx = position[0] - anchor.anchor.0;
                                    let dy = position[1] - anchor.anchor.1;
                                    anchor.anchor = (position[0], position[1]);
                                    anchor.left = (anchor.left.0 + dx, anchor.left.1 + dy);
                                    anchor.right = (anchor.right.0 + dx, anchor.right.1 + dy);
                                }
                                paged_mutate::PathPointRole::Left => {
                                    anchor.left = (position[0], position[1]);
                                }
                                paged_mutate::PathPointRole::Right => {
                                    anchor.right = (position[0], position[1]);
                                }
                            }
                        }
                    }
                    return;
                }
            }
        }
        NodeId::Group(id) => {
            for parsed in scene.spreads.iter_mut() {
                if let Some(g) = parsed
                    .spread
                    .groups
                    .iter_mut()
                    .find(|g| g.self_id.as_deref() == Some(id.as_str()))
                {
                    if let NodeMutation::Transform(m) = mutation {
                        g.item_transform = m;
                    }
                    return;
                }
            }
        }
        // Phase D only mutates TextFrame + Rectangle. Other shapes
        // resolve as ElementNotFound at snapshot time. Phase H added
        // Polygon for path-point editing (handled above); Track L
        // added Group above.
        _ => {}
    }
}

fn restore_snapshot_in_scene(scene: &mut paged_scene::Document, snap: &NodeSnapshot) {
    match &snap.node_id {
        NodeId::TextFrame(id) => {
            for parsed in scene.spreads.iter_mut() {
                if let Some(f) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|f| f.self_id.as_deref() == Some(id.as_str()))
                {
                    f.bounds = snap.bounds;
                    f.item_transform = snap.item_transform;
                    return;
                }
            }
        }
        NodeId::Rectangle(id) => {
            for parsed in scene.spreads.iter_mut() {
                if let Some(r) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|r| r.self_id.as_deref() == Some(id.as_str()))
                {
                    r.bounds = snap.bounds;
                    r.item_transform = snap.item_transform;
                    r.image_item_transform = snap.image_item_transform;
                    return;
                }
            }
        }
        NodeId::Polygon(id) => {
            for parsed in scene.spreads.iter_mut() {
                if let Some(p) = parsed
                    .spread
                    .polygons
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(id.as_str()))
                {
                    p.bounds = snap.bounds;
                    p.item_transform = snap.item_transform;
                    p.anchors = snap.path_anchors.clone();
                    return;
                }
            }
        }
        NodeId::Group(id) => {
            for parsed in scene.spreads.iter_mut() {
                if let Some(g) = parsed
                    .spread
                    .groups
                    .iter_mut()
                    .find(|g| g.self_id.as_deref() == Some(id.as_str()))
                {
                    g.item_transform = snap.item_transform;
                    return;
                }
            }
        }
        _ => {}
    }
}

/// Test helper — convenience wrapper around `compute_node_mutation`
/// that lets Phase B/C unit tests stay focused on the bounds math
/// without constructing a full `GestureSession`. Returns the new
/// bounds only; transform-emitting gestures (rotate / scale / rotated
/// translate) need the full `compute_node_mutation` path.
#[cfg(test)]
pub(crate) fn compute_new_bounds(
    snapshot: Bounds,
    gesture: GestureType,
    delta: (f32, f32),
    modifiers: GestureModifiers,
) -> Bounds {
    let snap = NodeSnapshot {
        id: ElementId::TextFrame("test".to_string()),
        node_id: NodeId::TextFrame("test".to_string()),
        bounds: snapshot,
        item_transform: None,
        image_item_transform: None,
        path_anchors: Vec::new(),
    };
    let session = GestureSession {
        handle: GestureHandle(0),
        gesture,
        snapshots: vec![snap.clone()],
        current_delta: Some(delta),
        modifiers,
        anchor_spread: None,
        pivot_spread: None,
        camera_scale: None,
    };
    match compute_node_mutation(&snap, &session, delta) {
        NodeMutation::Bounds(b) => b,
        NodeMutation::Transform(_)
        | NodeMutation::ImageTransform(_)
        | NodeMutation::PathPoint { .. } => snapshot,
    }
}

/// Phase H — Alt+Translate emits one `InsertNode(CloneTranslate)` per
/// member, so the originals stay put and a new copy lands at the
/// dragged position. The new self_ids derive from the source id +
/// the time-based suffix below so successive duplicates don't
/// collide. Note: this is the gesture-level path; scripted clones
/// pick their own self_ids when calling `paged_mutate::apply`.
///
/// Track K — `destination_spread_id` carries the spread the
/// pointer is currently over at commit time. When `Some` and
/// different from the source's spread, the apply layer routes
/// the clone there with a corrected delta accounting for the
/// spread-origin offset. `None` preserves the Phase H behaviour
/// (clone into the source's spread).
fn build_alt_duplicate_ops(
    snapshots: &[NodeSnapshot],
    delta: (f32, f32),
    destination_spread_id: Option<String>,
) -> Vec<Operation> {
    let suffix = duplicate_suffix();
    snapshots
        .iter()
        .enumerate()
        .map(|(i, snap)| {
            let source_raw = snap.id.raw_id();
            let new_self_id = format!("{source_raw}_dup_{suffix}_{i}");
            Operation::InsertNode {
                parent: NodeId::Spread(String::new()), // placeholder; the apply path uses the source's spread implicitly via find/replace, but the channel needs SOME parent.
                position: usize::MAX,                  // appended; apply.rs will clamp.
                node: NodeSpec::CloneTranslate {
                    self_id: new_self_id,
                    source: snap.node_id.clone(),
                    dx: delta.0,
                    dy: delta.1,
                    destination_spread_id: destination_spread_id.clone(),
                },
                z_slot: None, // duplicates stack on top
            }
        })
        .collect()
}

/// Phase H — produce a short, monotone-ish suffix for cloned node
/// self_ids. Uses millisecond timestamp on native, a sequence
/// counter on wasm32 (no wall clock).
fn duplicate_suffix() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        // wasm-bindgen js_sys provides Date::now() in the canvas
        // wasm binding, but paged-canvas is pure Rust and doesn't
        // link js-sys. Use a thread-local atomic counter — good
        // enough for uniqueness within a session.
        use core::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let v = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{v}")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        format!("{ms}")
    }
}

fn build_op_from_mutation(snap: &NodeSnapshot, mutation: NodeMutation) -> Operation {
    match mutation {
        NodeMutation::Bounds(b) => Operation::SetProperty {
            node: snap.node_id.clone(),
            path: PropertyPath::FrameBounds,
            value: Value::Bounds([b.top, b.left, b.bottom, b.right]),
        },
        NodeMutation::Transform(m) => Operation::SetProperty {
            node: snap.node_id.clone(),
            path: PropertyPath::FrameTransform,
            value: Value::Transform(m),
        },
        NodeMutation::ImageTransform(m) => Operation::SetProperty {
            node: snap.node_id.clone(),
            path: PropertyPath::ImageContentTransform,
            value: Value::Transform(m),
        },
        NodeMutation::PathPoint { address, position } => Operation::SetProperty {
            node: snap.node_id.clone(),
            path: PropertyPath::FramePathPoint,
            value: Value::PathPoint { address, position },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gesture_sab_layout_constants_match_spec() {
        // Locked-down byte size + u32-word offsets. The TS-side mirror
        // in `packages/client/src/sab/gesture.ts` reads
        // `gestureSabBytes()` from wasm at worker init and asserts its
        // own `OFFSET_*` constants against
        // `GestureSabLayout::canonical()` — any drift here will fire
        // a `protocolMismatch` warning on the canvas.
        assert_eq!(GESTURE_SAB_BYTES, 32);
        assert_eq!(GESTURE_OFFSET_HANDLE_LO, 0);
        assert_eq!(GESTURE_OFFSET_HANDLE_HI, 1);
        assert_eq!(GESTURE_OFFSET_DX, 2);
        assert_eq!(GESTURE_OFFSET_DY, 3);
        assert_eq!(GESTURE_OFFSET_MODIFIERS, 4);
        assert_eq!(GESTURE_OFFSET_SEQ, 5);
        assert_eq!(GESTURE_OFFSET_GEN_LO, 6);
        assert_eq!(GESTURE_OFFSET_GEN_HI, 7);
        assert_eq!(GESTURE_MODIFIER_SHIFT, 0b001);
        assert_eq!(GESTURE_MODIFIER_ALT, 0b010);
        assert_eq!(GESTURE_MODIFIER_DISABLE_SNAP, 0b100);
        // Total bytes = 8 u32 words.
        assert_eq!(GESTURE_SAB_BYTES, 8 * std::mem::size_of::<u32>());
    }

    #[test]
    fn gesture_sab_layout_canonical_matches_constants() {
        let lo = GestureSabLayout::canonical();
        assert_eq!(lo.bytes, GESTURE_SAB_BYTES as u32);
        assert_eq!(lo.offset_handle_lo, GESTURE_OFFSET_HANDLE_LO as u32);
        assert_eq!(lo.offset_handle_hi, GESTURE_OFFSET_HANDLE_HI as u32);
        assert_eq!(lo.offset_dx, GESTURE_OFFSET_DX as u32);
        assert_eq!(lo.offset_dy, GESTURE_OFFSET_DY as u32);
        assert_eq!(lo.offset_modifiers, GESTURE_OFFSET_MODIFIERS as u32);
        assert_eq!(lo.offset_seq, GESTURE_OFFSET_SEQ as u32);
        assert_eq!(lo.offset_gen_lo, GESTURE_OFFSET_GEN_LO as u32);
        assert_eq!(lo.offset_gen_hi, GESTURE_OFFSET_GEN_HI as u32);
        assert_eq!(lo.modifier_shift, GESTURE_MODIFIER_SHIFT);
        assert_eq!(lo.modifier_alt, GESTURE_MODIFIER_ALT);
        assert_eq!(lo.modifier_disable_snap, GESTURE_MODIFIER_DISABLE_SNAP);
    }

    fn b(top: f32, left: f32, bottom: f32, right: f32) -> Bounds {
        Bounds {
            top,
            left,
            bottom,
            right,
        }
    }

    fn assert_close(a: Bounds, e: Bounds) {
        let d = |x: f32, y: f32| (x - y).abs() < 1e-3;
        assert!(d(a.top, e.top), "top: got {} expected {}", a.top, e.top);
        assert!(d(a.left, e.left), "left: got {} expected {}", a.left, e.left);
        assert!(
            d(a.bottom, e.bottom),
            "bottom: got {} expected {}",
            a.bottom, e.bottom
        );
        assert!(
            d(a.right, e.right),
            "right: got {} expected {}",
            a.right, e.right
        );
    }

    const NONE: GestureModifiers = GestureModifiers {
        shift: false,
        alt: false,
        disable_snap: false,
    };
    const ALT: GestureModifiers = GestureModifiers {
        shift: false,
        alt: true,
        disable_snap: false,
    };
    const SHIFT: GestureModifiers = GestureModifiers {
        shift: true,
        alt: false,
        disable_snap: false,
    };

    #[test]
    fn inverse_rotate_delta_passes_through_identity() {
        let d = inverse_rotate_delta(None, (3.0, -5.0));
        assert!((d.0 - 3.0).abs() < 1e-4);
        assert!((d.1 - -5.0).abs() < 1e-4);
        let d2 = inverse_rotate_delta(Some(IDENTITY), (10.0, 7.0));
        assert!((d2.0 - 10.0).abs() < 1e-4);
        assert!((d2.1 - 7.0).abs() < 1e-4);
    }

    #[test]
    fn inverse_rotate_delta_undoes_90_rotation() {
        // 90° rotation: (1, 0) → (0, 1); inverse takes (0, 1) → (1, 0).
        let m = [0.0, 1.0, -1.0, 0.0, 50.0, 50.0];
        let d = inverse_rotate_delta(Some(m), (0.0, 1.0));
        assert!((d.0 - 1.0).abs() < 1e-3, "x={}", d.0);
        assert!(d.1.abs() < 1e-3, "y={}", d.1);
    }

    #[test]
    fn shift_constrains_translate_to_dominant_axis() {
        // |dx| > |dy| → y is dropped.
        let snap = b(0.0, 0.0, 100.0, 100.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Translate,
            (30.0, 5.0),
            GestureModifiers { shift: true, alt: false, disable_snap: false },
        );
        assert_close(out, b(0.0, 30.0, 100.0, 130.0));
        // |dy| > |dx| → x is dropped.
        let out = compute_new_bounds(
            snap,
            GestureType::Translate,
            (3.0, -40.0),
            GestureModifiers { shift: true, alt: false, disable_snap: false },
        );
        assert_close(out, b(-40.0, 0.0, 60.0, 100.0));
    }

    #[test]
    fn translate_shifts_all_four_edges() {
        let snap = b(0.0, 0.0, 100.0, 100.0);
        let out = compute_new_bounds(snap, GestureType::Translate, (5.0, 7.0), NONE);
        assert_close(out, b(7.0, 5.0, 107.0, 105.0));
    }

    #[test]
    fn resize_north_moves_only_top() {
        let snap = b(100.0, 100.0, 200.0, 200.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Resize { handle: ResizeHandle::North },
            (15.0, -20.0),
            NONE,
        );
        // Only top changed. dx is ignored for a cardinal vertical edge
        // — that's industry convention; cardinal handles only move
        // their own edge regardless of perpendicular motion.
        assert_close(out, b(80.0, 100.0, 200.0, 200.0));
    }

    #[test]
    fn resize_east_moves_only_right() {
        let snap = b(0.0, 0.0, 100.0, 100.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Resize { handle: ResizeHandle::East },
            (25.0, 10.0),
            NONE,
        );
        assert_close(out, b(0.0, 0.0, 100.0, 125.0));
    }

    #[test]
    fn resize_southeast_moves_two_edges() {
        let snap = b(0.0, 0.0, 100.0, 100.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Resize { handle: ResizeHandle::SouthEast },
            (10.0, 20.0),
            NONE,
        );
        assert_close(out, b(0.0, 0.0, 120.0, 110.0));
    }

    #[test]
    fn resize_northwest_moves_two_edges() {
        let snap = b(50.0, 50.0, 150.0, 150.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Resize { handle: ResizeHandle::NorthWest },
            (-5.0, -10.0),
            NONE,
        );
        assert_close(out, b(40.0, 45.0, 150.0, 150.0));
    }

    #[test]
    fn alt_resize_mirrors_about_centre() {
        // SE handle + Alt: right and bottom each move by (dx, dy);
        // left and top each move by (-dx, -dy). The centre stays put.
        let snap = b(0.0, 0.0, 100.0, 100.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Resize { handle: ResizeHandle::SouthEast },
            (10.0, 20.0),
            ALT,
        );
        assert_close(out, b(-20.0, -10.0, 120.0, 110.0));
    }

    #[test]
    fn alt_north_edge_mirrors_top_and_bottom() {
        let snap = b(0.0, 0.0, 100.0, 100.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Resize { handle: ResizeHandle::North },
            (0.0, -15.0),
            ALT,
        );
        // North edge moves up by 15; bottom mirrors down by 15.
        assert_close(out, b(-15.0, 0.0, 115.0, 100.0));
    }

    #[test]
    fn shift_corner_locks_aspect_ratio() {
        // Original is 100×100 (square). Drag SE by (40, 20): without
        // Shift you'd get 140×120 (non-square). With Shift the
        // dominant axis wins → 140×140.
        let snap = b(0.0, 0.0, 100.0, 100.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Resize { handle: ResizeHandle::SouthEast },
            (40.0, 20.0),
            SHIFT,
        );
        assert_close(out, b(0.0, 0.0, 140.0, 140.0));
    }

    #[test]
    fn shift_corner_preserves_aspect_for_non_square_snapshot() {
        // 100 wide × 50 tall. Drag SE by (50, 10): scale_w = 1.5,
        // scale_h = 1.2. Dominant is scale_w; new bounds = 150 × 75.
        let snap = b(0.0, 0.0, 50.0, 100.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Resize { handle: ResizeHandle::SouthEast },
            (50.0, 10.0),
            SHIFT,
        );
        // The aspect ratio is preserved (100:50 = 2:1).
        let w = out.right - out.left;
        let h = out.bottom - out.top;
        assert!((w / h - 2.0).abs() < 1e-3, "w/h={}", w / h);
        // Anchored at NW: top + left unchanged.
        assert!(out.top.abs() < 1e-3);
        assert!(out.left.abs() < 1e-3);
    }

    #[test]
    fn shift_corner_with_alt_anchors_centre() {
        let snap = b(0.0, 0.0, 100.0, 100.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Resize { handle: ResizeHandle::SouthEast },
            (50.0, 30.0),
            GestureModifiers { shift: true, alt: true, disable_snap: false },
        );
        // Centre stays at (50, 50); dominant scale_w = 1.5 → 150×150.
        // Centre-anchored: bounds = (-25, -25, 125, 125).
        let cx = (out.left + out.right) * 0.5;
        let cy = (out.top + out.bottom) * 0.5;
        assert!((cx - 50.0).abs() < 1e-3);
        assert!((cy - 50.0).abs() < 1e-3);
        let w = out.right - out.left;
        let h = out.bottom - out.top;
        assert!((w - h).abs() < 1e-3);
    }

    #[test]
    fn shift_edge_propagates_to_perpendicular_axis() {
        // East edge with Shift. Drag right by 50 (scale_w = 1.5).
        // Height also scales by 1.5 → 150, anchored on centre.
        let snap = b(0.0, 0.0, 100.0, 100.0);
        let out = compute_new_bounds(
            snap,
            GestureType::Resize { handle: ResizeHandle::East },
            (50.0, 0.0),
            SHIFT,
        );
        let h = out.bottom - out.top;
        assert!((h - 150.0).abs() < 1e-3, "h={h}");
        // Centre on the perpendicular axis stays at 50.
        let cy = (out.top + out.bottom) * 0.5;
        assert!((cy - 50.0).abs() < 1e-3, "cy={cy}");
    }
}
