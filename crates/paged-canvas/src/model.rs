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

//! Worker-side canvas data model.
//!
//! `CanvasModel` is the single owner of parsed document state, the
//! built display lists, and (in later phases) the salsa database
//! that memoises Tiers 2–4. Phase-1 implementation is intentionally
//! synchronous and non-incremental: `load()` parses + builds the
//! whole document, and every mutation triggers a fresh rebuild.
//! The point of this phase is to nail down the **API surface** the
//! main thread depends on; incrementality is Phase 3's problem.

use std::collections::HashMap;

use paged_renderer::{
    pipeline, BuiltDocument, BuiltPage, BytesResolver, DisplayList, Document, PageId,
    PipelineOptions,
};
use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

use crate::channel::{LoadError, Mutation};

/// Options that flow through to `paged-renderer::PipelineOptions`.
/// Mirrors the subset of the renderer's options the worker needs
/// to surface to the main thread on `LoadDocument`.
#[derive(Debug, Clone, Default)]
pub struct CanvasOptions {
    /// Default-font fallback. First-entry-still-wins semantics: the
    /// renderer's `PipelineOptions::font` receives this byte slice and
    /// uses it for any `AppliedFont` that doesn't resolve via the
    /// family registry. Kept as a `Vec<Vec<u8>>` so callers that don't
    /// know about the registry (e.g. the dev shell auto-loading
    /// Inter.ttf) can still drop bytes in here without naming them.
    pub fonts: Vec<Vec<u8>>,
    /// Named font registry. Each entry binds an `AppliedFont` family
    /// (and optionally a style like "Bold") to a font payload, mirroring
    /// `paged-inspect --font-family "Family=path"`. Translates 1:1 to
    /// `BytesResolver::add_font` entries on every build/rebuild.
    pub font_registry: Vec<FontEntry>,
    /// CMYK ICC profile bytes for accurate colour. Optional; the
    /// renderer falls back to naive conversion when absent.
    pub cmyk_icc_profile: Option<Vec<u8>>,
    /// Concept 2 — named ICC profiles registered before load (the
    /// `RegisterColorProfile` registry, font-registry pattern).
    /// `SetColorSettings` resolves working-space names against
    /// these; a designmap naming one of them activates it at load
    /// when no explicit `cmyk_icc_profile` is supplied.
    pub color_profiles: Vec<ColorProfileEntry>,
}

/// Concept 2 — one named ICC profile payload.
#[derive(Debug, Clone)]
pub struct ColorProfileEntry {
    /// Display / lookup name, e.g. "Coated FOGRA39 (ISO 12647-2:2004)".
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Concept 2 — the document's active colour-management settings
/// (mirrors InDesign's Color Settings). Written whole by
/// `Mutation::SetColorSettings`; surfaced through `DocumentMeta`.
/// `Default` reproduces the pre-Concept-2 hardcoded behaviour.
#[derive(Debug, Clone)]
pub struct ColorSettingsState {
    /// Name of the ACTIVE registered profile, `None` when running on
    /// the load-time bytes (or no profile at all).
    pub cmyk_profile_name: Option<String>,
    /// Concept-3 seam — "preserve" | "convertToWorkingSpace" | "off".
    pub rgb_policy: Option<String>,
    pub intent: paged_color::Intent,
    pub bpc: bool,
}

/// Concept 2 (Ink Manager) — one ink's output-time settings.
#[derive(Debug, Clone, Default)]
pub struct InkSetting {
    pub convert_to_process: bool,
    pub alias_to: Option<String>,
}

/// Concept 2 — resolved soft-proof condition. The bytes are cloned
/// out of the profile registry at SetProofSetup time so a later
/// re-registration doesn't silently change the active proof.
#[derive(Debug, Clone)]
struct ProofState {
    name: String,
    bytes: Vec<u8>,
    intent: paged_color::Intent,
    simulate_paper_white: bool,
}

impl Default for ColorSettingsState {
    fn default() -> Self {
        Self {
            cmyk_profile_name: None,
            rgb_policy: None,
            intent: paged_color::Intent::RelativeColorimetric,
            bpc: true,
        }
    }
}

/// Named font payload used to populate the renderer's per-family
/// asset resolver.
#[derive(Debug, Clone)]
pub struct FontEntry {
    /// IDML family name as it appears in `AppliedFont` (e.g.
    /// `"Poppins"`).
    pub family: String,
    /// IDML style string when known (`"Regular"`, `"Bold Italic"`).
    /// `None` registers the family bare and matches every style via
    /// the bare-family fall-through in `BytesResolver::resolve_font`.
    pub style: Option<String>,
    pub bytes: Vec<u8>,
}

/// One-time facts about a loaded document. Sent to the main thread
/// on a successful `LoadDocument` so the navigator + page count UI
/// can render before the first page is rasterised.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct DocumentHandle {
    /// Stable id assigned by the worker; used by the main thread when
    /// addressing operations to a specific document (the worker may
    /// hold more than one document open in the future).
    pub doc_id: String,
    /// Total page count. Stable for the life of the document unless
    /// a mutation explicitly inserts / deletes pages.
    pub page_count: usize,
    /// Page ids in document order. The navigator displays them as
    /// "page N" with `N = 1 + index`; the canvas uses the ids
    /// directly for cache keys.
    pub page_ids: Vec<PageId>,
    /// Per-page dimensions in points. Same length as `page_ids`.
    /// The navigator needs these to size thumbnails before any
    /// rasterisation has happened.
    pub page_sizes_pt: Vec<(f32, f32)>,
    /// Aggregate counts for debugging / UI badges.
    pub stats: DocumentStats,
    /// Plan-2 §8.3 — ruler guides per page. The overlay renders
    /// these and the snap pass treats them as targets. Total volume
    /// is small (real docs ship a few dozen at most) so we ship them
    /// inline on the handle rather than paging via a separate
    /// request.
    #[serde(default)]
    pub ruler_guides: Vec<RulerGuideWire>,
}

/// Plan-2 §8.3 — wire shape of a ruler guide. `page_id` matches one
/// of `DocumentHandle::page_ids`. `orientation` is "vertical" (snaps
/// on x) or "horizontal" (snaps on y); `location` is the page-local
/// coord on the perpendicular axis.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct RulerGuideWire {
    pub page_id: PageId,
    pub orientation: GuideOrientationWire,
    pub location: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Tsify, PartialEq, Eq)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub enum GuideOrientationWire {
    Vertical,
    Horizontal,
}

/// Structural counts. The main thread surfaces these in the debug
/// HUD. Mirrors `paged-renderer::PipelineStats` but lives in serde-
/// friendly form so it can cross the message channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
pub struct DocumentStats {
    pub spreads: usize,
    pub pages: usize,
    pub frames: usize,
    pub stories: usize,
    pub paragraphs: usize,
    pub runs: usize,
    pub glyphs: usize,
    pub lines: usize,
    /// panels.md gap 1 — number of distinct stories whose text
    /// overflows the last frame in its chain (overset). Derived from
    /// the build's `OversetTextDropped` diagnostics, not from
    /// `PipelineStats`, so `DocumentStats::from(&PipelineStats)`
    /// leaves this 0 and the `handle()` builder backfills it from the
    /// document's render diagnostics. Drives the Preflight panel's
    /// "N overset stories" badge.
    #[serde(default)]
    pub overset_stories: usize,
}

/// What `CanvasModel::apply_mutation` returns on success. The
/// `applied_seq` is the monotone id the worker assigns; `page_ids`
/// lists pages the caller must invalidate in its LOD cache; the
/// `inverse` is the op to push onto the undo log.
#[derive(Debug, Clone)]
pub struct MutationOutcome {
    pub applied_seq: u64,
    pub page_ids: Vec<PageId>,
    pub inverse: crate::mutate::TextOp,
    /// Editor-ops — the element a structural insert created (or the
    /// new page id for `InsertPage`); `None` for every other kind.
    /// Threaded into the `MutationApplied` reply so the editor can
    /// select the fresh element.
    pub created_id: Option<crate::element_selection::ElementId>,
    /// Editor-ops — `true` when the page LIST changed (insert /
    /// delete / resize page); the reply then carries the refreshed
    /// per-page sizes.
    pub page_structure_changed: bool,
}

/// Editor-ops — document defaults for newly-created objects. The
/// whole triple is replaced by `Mutation::SetDocumentDefaults`;
/// `None` means no fill / no stroke / engine-default weight.
#[derive(Debug, Clone, Default)]
pub struct DocumentDefaults {
    pub fill_color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
}

/// Editor-ops — the element a structural-insert Operation created,
/// mapped to the channel's `ElementId` so the `MutationApplied` reply
/// can carry it (the editor selects the fresh element).
/// B-04 — leaf ElementId -> NodeId (groups and non-page-item ids map
/// to None and reject wire-side). Used by the plugin-metadata carrier,
/// which targets leaf page items only.
fn element_to_leaf_node_id(
    id: &crate::element_selection::ElementId,
) -> Option<paged_mutate::NodeId> {
    use crate::element_selection::ElementId;
    Some(match id {
        ElementId::TextFrame(s) => paged_mutate::NodeId::TextFrame(s.clone()),
        ElementId::Rectangle(s) => paged_mutate::NodeId::Rectangle(s.clone()),
        ElementId::Oval(s) => paged_mutate::NodeId::Oval(s.clone()),
        ElementId::GraphicLine(s) => paged_mutate::NodeId::GraphicLine(s.clone()),
        ElementId::Polygon(s) => paged_mutate::NodeId::Polygon(s.clone()),
        _ => return None,
    })
}

/// B-04 / W1.20 — group-member ElementId -> NodeId. Leaf page items
/// AND `Group`s (the v2 nested-create case) map through; non-page-item
/// ids (StoryRange / Table / TableCell) map to None and reject
/// wire-side.
fn element_to_member_node_id(
    id: &crate::element_selection::ElementId,
) -> Option<paged_mutate::NodeId> {
    use crate::element_selection::ElementId;
    Some(match id {
        ElementId::Group(s) => paged_mutate::NodeId::Group(s.clone()),
        other => return element_to_leaf_node_id(other),
    })
}

fn created_element_id(op: &paged_mutate::Operation) -> Option<crate::element_selection::ElementId> {
    use crate::element_selection::ElementId;
    // B-04 — group creation reports the minted group id.
    if let paged_mutate::Operation::CreateGroup { spec } = op {
        return spec.self_id.clone().map(ElementId::Group);
    }
    // Protocol v34 — a batch reports its LAST creating child (the id
    // the batch-created sentinel resolved to), so insert-with-
    // metadata flows still get a `createdId` to select.
    if let paged_mutate::Operation::Batch { ops } = op {
        return ops.iter().rev().find_map(created_element_id);
    }
    if let paged_mutate::Operation::InsertNode { node, .. } = op {
        return match node.node_id() {
            paged_mutate::NodeId::TextFrame(id) => Some(ElementId::TextFrame(id)),
            paged_mutate::NodeId::Rectangle(id) => Some(ElementId::Rectangle(id)),
            paged_mutate::NodeId::GraphicLine(id) => Some(ElementId::GraphicLine(id)),
            paged_mutate::NodeId::Polygon(id) => Some(ElementId::Polygon(id)),
            paged_mutate::NodeId::Oval(id) => Some(ElementId::Oval(id)),
            paged_mutate::NodeId::Group(id) => Some(ElementId::Group(id)),
            _ => None,
        };
    }
    None
}

/// Editor-ops — id-minting scan helper: track the max `u<hex>` suffix.
fn scan_page_item_id(max: &mut u64, id: Option<&str>) {
    let Some(id) = id else { return };
    let Some(hex) = id.strip_prefix('u') else {
        return;
    };
    if hex.is_empty() || hex.len() > 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return;
    }
    if let Ok(v) = u64::from_str_radix(hex, 16) {
        *max = (*max).max(v);
    }
}

/// Phase B — frame-mutation analogue of [`MutationOutcome`]. Carries
/// the full `AppliedOperation` (op + inverse + invalidation hint)
/// rather than just the inverse text op, since frame mutations come
/// from the canonical `paged_mutate` log.
#[derive(Debug, Clone)]
pub struct FrameMutationOutcome {
    pub applied_seq: u64,
    pub page_ids: Vec<PageId>,
    pub applied: paged_mutate::AppliedOperation,
}

/// What `CanvasModel::undo` / `redo` return.
#[derive(Debug, Clone)]
pub struct UndoOutcome {
    /// `applied_seq` of the mutation being reversed (undo) or
    /// re-applied (redo).
    pub undone_seq: u64,
    /// Newly assigned `applied_seq` for the undo/redo operation
    /// itself.
    pub applied_seq: u64,
    pub page_ids: Vec<PageId>,
    /// Phase 4 Step 3 — story id touched by the undo/redo, used by
    /// the wasm dispatch to scope GPU cache invalidation. None when
    /// the op carries no story id (frame moves, page inserts) —
    /// reserved for later mutation types.
    pub affected_story_id: Option<String>,
}

/// Track J fan-out — map an `ElementId` carried on the wire to the
/// matching `paged_mutate::NodeId` for the apply layer. Ovals + Groups
/// don't carry editable PathPointArrays so they translate to `None`
/// and the calling Mutation falls through to the non-frame handler.
fn path_node_id_for(id: &crate::element_selection::ElementId) -> Option<paged_mutate::NodeId> {
    use crate::element_selection::ElementId;
    use paged_mutate::NodeId;
    match id {
        ElementId::Polygon(s) => Some(NodeId::Polygon(s.clone())),
        ElementId::TextFrame(s) => Some(NodeId::TextFrame(s.clone())),
        ElementId::Rectangle(s) => Some(NodeId::Rectangle(s.clone())),
        ElementId::GraphicLine(s) => Some(NodeId::GraphicLine(s.clone())),
        // Path-bearing only — Oval / Group / StoryRange / Table /
        // TableCell have no `<PathPointArray>`, so the path-edit
        // gestures never target them.
        ElementId::Oval(_)
        | ElementId::Group(_)
        | ElementId::StoryRange { .. }
        | ElementId::Table { .. }
        | ElementId::TableCell { .. } => None,
    }
}

/// Inspector P1 — map a wire `ElementId` to the apply layer's
/// `NodeId`. Unlike `path_node_id_for`, this version handles every
/// element kind including Oval / Group; the apply layer may refuse
/// some `(NodeId, PropertyPath)` combinations, but routing decides
/// that at apply time.
fn element_to_node_id(id: &crate::element_selection::ElementId) -> paged_mutate::NodeId {
    use crate::element_selection::ElementId;
    use paged_mutate::NodeId;
    match id {
        ElementId::TextFrame(s) => NodeId::TextFrame(s.clone()),
        ElementId::Rectangle(s) => NodeId::Rectangle(s.clone()),
        ElementId::Oval(s) => NodeId::Oval(s.clone()),
        ElementId::Polygon(s) => NodeId::Polygon(s.clone()),
        ElementId::GraphicLine(s) => NodeId::GraphicLine(s.clone()),
        ElementId::Group(s) => NodeId::Group(s.clone()),
        ElementId::StoryRange {
            story_id,
            start,
            end,
        } => NodeId::StoryRange {
            story_id: story_id.clone(),
            start: *start,
            end: *end,
        },
        ElementId::Table { story_id, table_id } => NodeId::Table {
            story_id: story_id.clone(),
            table_id: table_id.clone(),
        },
        ElementId::TableCell {
            story_id,
            table_id,
            row,
            col,
        } => NodeId::TableCell {
            story_id: story_id.clone(),
            table_id: table_id.clone(),
            row: *row,
            col: *col,
        },
    }
}

/// Inspector P1 — translate a `FrameRef` into a tree node for the
/// scene-tree outline. Recurses through Groups so nested members
/// appear as children. Returns `None` for empty / unresolvable
/// references.
fn frame_to_tree_node(
    spread: &paged_parse::Spread,
    fr: paged_parse::FrameRef,
) -> Option<crate::channel::SceneTreeNode> {
    use crate::channel::SceneTreeNode;
    use crate::element_selection::ElementId;
    use paged_parse::FrameRef;
    match fr {
        FrameRef::TextFrame(i) => spread.text_frames.get(i).and_then(|f| {
            let id = f.self_id.clone()?;
            Some(SceneTreeNode {
                id: Some(ElementId::TextFrame(id.clone())),
                kind: "TextFrame".to_string(),
                label: format!("TextFrame {id}"),
                children: Vec::new(),
            })
        }),
        FrameRef::Rectangle(i) => spread.rectangles.get(i).and_then(|f| {
            let id = f.self_id.clone()?;
            Some(SceneTreeNode {
                id: Some(ElementId::Rectangle(id.clone())),
                kind: "Rectangle".to_string(),
                label: format!("Rectangle {id}"),
                children: Vec::new(),
            })
        }),
        FrameRef::Oval(i) => spread.ovals.get(i).and_then(|f| {
            let id = f.self_id.clone()?;
            Some(SceneTreeNode {
                id: Some(ElementId::Oval(id.clone())),
                kind: "Oval".to_string(),
                label: format!("Oval {id}"),
                children: Vec::new(),
            })
        }),
        FrameRef::Polygon(i) => spread.polygons.get(i).and_then(|f| {
            let id = f.self_id.clone()?;
            Some(SceneTreeNode {
                id: Some(ElementId::Polygon(id.clone())),
                kind: "Polygon".to_string(),
                label: format!("Polygon {id}"),
                children: Vec::new(),
            })
        }),
        FrameRef::GraphicLine(i) => spread.graphic_lines.get(i).and_then(|f| {
            let id = f.self_id.clone()?;
            Some(SceneTreeNode {
                id: Some(ElementId::GraphicLine(id.clone())),
                kind: "GraphicLine".to_string(),
                label: format!("GraphicLine {id}"),
                children: Vec::new(),
            })
        }),
        FrameRef::Group(i) => spread.groups.get(i).map(|g| {
            let id = g.self_id.clone().unwrap_or_else(|| format!("group#{i}"));
            let children = g
                .members
                .iter()
                .filter_map(|m| frame_to_tree_node(spread, *m))
                .collect();
            SceneTreeNode {
                id: Some(ElementId::Group(id.clone())),
                kind: "Group".to_string(),
                label: format!("Group {id}"),
                children,
            }
        }),
    }
}

/// W1.20 (groups v2) — the spread-space axis-aligned bounding box of
/// every leaf reachable from `group_idx` (descending through nested
/// groups), each leaf's `[bounds × effective item_transform]` oriented
/// box folded in. `None` when the group is empty / unresolvable. Used
/// by the Group read-side descriptor so the layers/tree panel can show
/// a group's extent and the inspector can pivot scale/rotate gestures.
fn group_union_aabb(spread: &paged_parse::Spread, group_idx: usize) -> Option<paged_parse::Bounds> {
    use paged_parse::{Bounds, FrameRef};
    fn leaf_box(spread: &paged_parse::Spread, r: FrameRef) -> Option<Bounds> {
        let (bounds, it) = match r {
            FrameRef::TextFrame(i) => spread
                .text_frames
                .get(i)
                .map(|f| (f.bounds, f.item_transform)),
            FrameRef::Rectangle(i) => spread
                .rectangles
                .get(i)
                .map(|f| (f.bounds, f.item_transform)),
            FrameRef::Oval(i) => spread.ovals.get(i).map(|f| (f.bounds, f.item_transform)),
            FrameRef::GraphicLine(i) => spread
                .graphic_lines
                .get(i)
                .map(|f| (f.bounds, f.item_transform)),
            FrameRef::Polygon(i) => spread.polygons.get(i).map(|f| (f.bounds, f.item_transform)),
            FrameRef::Group(_) => None,
        }?;
        // `transform_bbox` returns the axis-aligned bbox of the
        // oriented (bounds × item_transform) box in spread space.
        Some(crate::hit::transform_bbox(bounds, it))
    }
    fn walk(spread: &paged_parse::Spread, gi: usize, acc: &mut Option<Bounds>) {
        let Some(group) = spread.groups.get(gi) else {
            return;
        };
        for m in &group.members {
            match *m {
                FrameRef::Group(child) => walk(spread, child, acc),
                leaf => {
                    if let Some(b) = leaf_box(spread, leaf) {
                        *acc = Some(match acc.take() {
                            Some(a) => Bounds {
                                left: a.left.min(b.left),
                                top: a.top.min(b.top),
                                right: a.right.max(b.right),
                                bottom: a.bottom.max(b.bottom),
                            },
                            None => b,
                        });
                    }
                }
            }
        }
    }
    let mut acc: Option<Bounds> = None;
    walk(spread, group_idx, &mut acc);
    acc
}

/// SDK Phase 3 — uniform-collapse helper for the StoryRange
/// snapshot. Returns `Some(common)` when all values match (every
/// run agrees, including the "all agree on None" case);
/// `None` when values diverge. The wrapping `PropertyEntry.value:
/// Option<Value>` then carries `Some(Value::Length(uniform))` /
/// `None` (mixed) at the wire boundary.
fn collapse_uniform<T: Clone + PartialEq>(values: &[T]) -> Option<T> {
    let first = values.first()?;
    if values.iter().all(|v| v == first) {
        Some(first.clone())
    } else {
        None
    }
}

fn story_id_of_text_op(op: &crate::mutate::TextOp) -> &str {
    match op {
        crate::mutate::TextOp::InsertText { story_id, .. } => story_id,
        crate::mutate::TextOp::DeleteRange { story_id, .. } => story_id,
    }
}

// ---- W0.3 — read-side enum→IDML-string mirrors + transform
// decompose. The mutate layer owns the write-side `*_as_idml`
// (private there); the inspector needs the same canonical strings so
// the snapshot round-trips through `apply`. ----------------------

fn vertical_justification_idml(v: paged_parse::VerticalJustification) -> &'static str {
    use paged_parse::VerticalJustification as V;
    match v {
        V::Top => "TopAlign",
        V::Center => "CenterAlign",
        V::Bottom => "BottomAlign",
        V::Justify => "JustifyAlign",
    }
}

fn auto_sizing_idml(v: paged_parse::AutoSizingType) -> &'static str {
    use paged_parse::AutoSizingType as A;
    match v {
        A::Off => "Off",
        A::HeightOnly => "HeightOnly",
        A::WidthOnly => "WidthOnly",
        A::HeightAndWidth => "HeightAndWidth",
        A::HeightAndWidthProportionally => "HeightAndWidthProportionally",
    }
}

fn first_baseline_idml(v: paged_parse::FirstBaselineOffset) -> &'static str {
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

fn corner_option_idml(v: paged_parse::CornerOption) -> &'static str {
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

fn decompose_angle(m: Option<[f32; 6]>) -> f32 {
    paged_mutate::operation::decompose_transform(m).angle_deg
}
fn decompose_scale_x(m: Option<[f32; 6]>) -> f32 {
    paged_mutate::operation::decompose_transform(m).scale_x
}
fn decompose_scale_y(m: Option<[f32; 6]>) -> f32 {
    paged_mutate::operation::decompose_transform(m).scale_y
}
fn decompose_flip_h(m: Option<[f32; 6]>) -> bool {
    paged_mutate::operation::decompose_transform(m).flip_h
}
fn decompose_flip_v(m: Option<[f32; 6]>) -> bool {
    paged_mutate::operation::decompose_transform(m).flip_v
}

/// W0.4 — read-side mirror of the transparency-effect per-field paths
/// (gap 18). Emits one `PropertyEntry` per effect field, sourcing each
/// from the parsed `effects: Option<FrameEffects>` block (a `None`
/// effect surfaces the field's "empty" value: `false` for the
/// `*Enabled` toggle, `Length(None)` / `ColorRef(None)` / `Text("")`
/// for the rest). Shared by the TextFrame and Rectangle property
/// blocks so the inventory stays in lockstep across kinds. The
/// object-level `frame.blendMode` path reads the page item's own
/// `blend_mode` slot (the `<BlendingSetting>` Opacity half is already
/// surfaced as `FrameOpacity`).
fn effect_property_entries(
    effects: Option<&paged_parse::FrameEffects>,
    blend_mode: Option<&str>,
) -> Vec<crate::channel::PropertyEntry> {
    use crate::channel::PropertyEntry;
    use paged_mutate::{PropertyPath as P, Value as V};
    let inner_shadow = effects.and_then(|e| e.inner_shadow.as_ref());
    let outer_glow = effects.and_then(|e| e.outer_glow.as_ref());
    let inner_glow = effects.and_then(|e| e.inner_glow.as_ref());
    let bevel = effects.and_then(|e| e.bevel.as_ref());
    let satin = effects.and_then(|e| e.satin.as_ref());
    let feather = effects.and_then(|e| e.feather.as_ref());
    let dfeather = effects.and_then(|e| e.directional_feather.as_ref());
    let entry = |path, value| PropertyEntry {
        path,
        value: Some(value),
    };
    let txt = |s: Option<&String>| V::Text(s.cloned().unwrap_or_default());
    let col = |s: Option<&String>| V::ColorRef(s.cloned());
    vec![
        // Inner shadow.
        entry(P::FrameInnerShadowEnabled, V::Bool(inner_shadow.is_some())),
        entry(
            P::FrameInnerShadowBlendMode,
            txt(inner_shadow.and_then(|e| e.blend_mode.as_ref())),
        ),
        entry(
            P::FrameInnerShadowColor,
            col(inner_shadow.and_then(|e| e.effect_color.as_ref())),
        ),
        entry(
            P::FrameInnerShadowOpacity,
            V::Length(inner_shadow.and_then(|e| e.opacity_pct)),
        ),
        entry(
            P::FrameInnerShadowAngle,
            V::Length(inner_shadow.and_then(|e| e.angle_deg)),
        ),
        entry(
            P::FrameInnerShadowDistance,
            V::Length(inner_shadow.and_then(|e| e.distance)),
        ),
        entry(
            P::FrameInnerShadowSize,
            V::Length(inner_shadow.and_then(|e| e.size)),
        ),
        entry(
            P::FrameInnerShadowChoke,
            V::Length(inner_shadow.and_then(|e| e.choke_pct)),
        ),
        entry(
            P::FrameInnerShadowNoise,
            V::Length(inner_shadow.and_then(|e| e.noise_pct)),
        ),
        // Outer glow.
        entry(P::FrameOuterGlowEnabled, V::Bool(outer_glow.is_some())),
        entry(
            P::FrameOuterGlowBlendMode,
            txt(outer_glow.and_then(|e| e.blend_mode.as_ref())),
        ),
        entry(
            P::FrameOuterGlowColor,
            col(outer_glow.and_then(|e| e.effect_color.as_ref())),
        ),
        entry(
            P::FrameOuterGlowOpacity,
            V::Length(outer_glow.and_then(|e| e.opacity_pct)),
        ),
        entry(
            P::FrameOuterGlowSpread,
            V::Length(outer_glow.and_then(|e| e.spread_pct)),
        ),
        entry(
            P::FrameOuterGlowSize,
            V::Length(outer_glow.and_then(|e| e.size)),
        ),
        entry(
            P::FrameOuterGlowNoise,
            V::Length(outer_glow.and_then(|e| e.noise_pct)),
        ),
        // Inner glow.
        entry(P::FrameInnerGlowEnabled, V::Bool(inner_glow.is_some())),
        entry(
            P::FrameInnerGlowBlendMode,
            txt(inner_glow.and_then(|e| e.blend_mode.as_ref())),
        ),
        entry(
            P::FrameInnerGlowColor,
            col(inner_glow.and_then(|e| e.effect_color.as_ref())),
        ),
        entry(
            P::FrameInnerGlowOpacity,
            V::Length(inner_glow.and_then(|e| e.opacity_pct)),
        ),
        entry(
            P::FrameInnerGlowChoke,
            V::Length(inner_glow.and_then(|e| e.choke_pct)),
        ),
        entry(
            P::FrameInnerGlowSize,
            V::Length(inner_glow.and_then(|e| e.size)),
        ),
        entry(
            P::FrameInnerGlowSource,
            txt(inner_glow.and_then(|e| e.source.as_ref())),
        ),
        entry(
            P::FrameInnerGlowNoise,
            V::Length(inner_glow.and_then(|e| e.noise_pct)),
        ),
        // Bevel / emboss.
        entry(P::FrameBevelEnabled, V::Bool(bevel.is_some())),
        entry(
            P::FrameBevelStyle,
            txt(bevel.and_then(|e| e.style.as_ref())),
        ),
        entry(
            P::FrameBevelTechnique,
            txt(bevel.and_then(|e| e.technique.as_ref())),
        ),
        entry(
            P::FrameBevelDepth,
            V::Length(bevel.and_then(|e| e.depth_pct)),
        ),
        entry(
            P::FrameBevelDirection,
            txt(bevel.and_then(|e| e.direction.as_ref())),
        ),
        entry(P::FrameBevelSize, V::Length(bevel.and_then(|e| e.size))),
        entry(P::FrameBevelSoften, V::Length(bevel.and_then(|e| e.soften))),
        entry(
            P::FrameBevelAngle,
            V::Length(bevel.and_then(|e| e.angle_deg)),
        ),
        entry(
            P::FrameBevelAltitude,
            V::Length(bevel.and_then(|e| e.altitude_deg)),
        ),
        entry(
            P::FrameBevelHighlightColor,
            col(bevel.and_then(|e| e.highlight_color.as_ref())),
        ),
        entry(
            P::FrameBevelShadowColor,
            col(bevel.and_then(|e| e.shadow_color.as_ref())),
        ),
        entry(
            P::FrameBevelHighlightOpacity,
            V::Length(bevel.and_then(|e| e.highlight_opacity_pct)),
        ),
        entry(
            P::FrameBevelShadowOpacity,
            V::Length(bevel.and_then(|e| e.shadow_opacity_pct)),
        ),
        // Satin.
        entry(P::FrameSatinEnabled, V::Bool(satin.is_some())),
        entry(
            P::FrameSatinBlendMode,
            txt(satin.and_then(|e| e.blend_mode.as_ref())),
        ),
        entry(
            P::FrameSatinColor,
            col(satin.and_then(|e| e.effect_color.as_ref())),
        ),
        entry(
            P::FrameSatinOpacity,
            V::Length(satin.and_then(|e| e.opacity_pct)),
        ),
        entry(
            P::FrameSatinAngle,
            V::Length(satin.and_then(|e| e.angle_deg)),
        ),
        entry(
            P::FrameSatinDistance,
            V::Length(satin.and_then(|e| e.distance)),
        ),
        entry(P::FrameSatinSize, V::Length(satin.and_then(|e| e.size))),
        entry(
            P::FrameSatinInvert,
            V::Bool(satin.and_then(|e| e.invert).unwrap_or(false)),
        ),
        // Feather (basic).
        entry(P::FrameFeatherEnabled, V::Bool(feather.is_some())),
        entry(
            P::FrameFeatherWidth,
            V::Length(feather.and_then(|e| e.width)),
        ),
        entry(
            P::FrameFeatherCornerType,
            txt(feather.and_then(|e| e.corner_type.as_ref())),
        ),
        entry(
            P::FrameFeatherNoise,
            V::Length(feather.and_then(|e| e.noise_pct)),
        ),
        entry(
            P::FrameFeatherChoke,
            V::Length(feather.and_then(|e| e.choke_pct)),
        ),
        // Directional feather.
        entry(
            P::FrameDirectionalFeatherEnabled,
            V::Bool(dfeather.is_some()),
        ),
        entry(
            P::FrameDirectionalFeatherLeftWidth,
            V::Length(dfeather.and_then(|e| e.left_width)),
        ),
        entry(
            P::FrameDirectionalFeatherRightWidth,
            V::Length(dfeather.and_then(|e| e.right_width)),
        ),
        entry(
            P::FrameDirectionalFeatherTopWidth,
            V::Length(dfeather.and_then(|e| e.top_width)),
        ),
        entry(
            P::FrameDirectionalFeatherBottomWidth,
            V::Length(dfeather.and_then(|e| e.bottom_width)),
        ),
        entry(
            P::FrameDirectionalFeatherAngle,
            V::Length(dfeather.and_then(|e| e.angle_deg)),
        ),
        entry(
            P::FrameDirectionalFeatherNoise,
            V::Length(dfeather.and_then(|e| e.noise_pct)),
        ),
        entry(
            P::FrameDirectionalFeatherChoke,
            V::Length(dfeather.and_then(|e| e.choke_pct)),
        ),
        // Object-level transparency blend mode.
        entry(
            P::FrameBlendMode,
            V::Text(blend_mode.unwrap_or_default().to_string()),
        ),
    ]
}

impl From<&pipeline::PipelineStats> for DocumentStats {
    fn from(s: &pipeline::PipelineStats) -> Self {
        Self {
            spreads: s.spreads,
            pages: s.pages,
            frames: s.frames,
            stories: s.stories,
            paragraphs: s.paragraphs,
            runs: s.runs,
            glyphs: s.glyphs,
            lines: s.lines,
            // Backfilled by `CanvasModel::handle()` from the build's
            // overset diagnostics (PipelineStats doesn't track which
            // *stories* overflowed, only the dropped-line count).
            overset_stories: 0,
        }
    }
}

/// The worker's view of a single loaded document plus all derived
/// canvas state.
///
/// Today this is a thin wrapper: store the parsed scene + the most
/// recent `BuiltDocument`. Tomorrow (Phase 3) the `BuiltDocument`
/// becomes a salsa-tracked derived value and incremental Tier 2
/// runs against checkpoints stored alongside `scene`.
pub struct CanvasModel {
    doc_id: String,
    pub(crate) scene: Document,
    /// W3.B2 — the IDML bytes this model was parsed from, retained so
    /// `export_idml` can hand them to `paged_write::write_idml` as the
    /// carry-through source package (it patches only the model-owned
    /// Spreads/Stories and copies every other entry verbatim, so it
    /// needs the original ZIP container, not just the parsed scene).
    /// Memory cost: one *compressed* IDML package copy — typically a
    /// few MB at most. Cleared/replaced on every `load` (a fresh
    /// `CanvasModel` ⇒ fresh bytes), so it never accumulates.
    pub(crate) source_idml: Vec<u8>,
    pub(crate) built: BuiltDocument,
    /// Index from `PageId` to `BuiltDocument::pages` position. Built
    /// once at load and refreshed after every rebuild. Worker callers
    /// (display-list-for-page, snapshot rendering, hit-test) all key
    /// by id; the linear-scan fallback on `BuiltDocument::page` is
    /// fine in absolute terms but salsa-shaped lookups should be O(1).
    page_index: HashMap<PageId, usize>,
    /// Owned option inputs. `PipelineOptions` borrows from these on
    /// every rebuild; storing them owned keeps the worker self-contained.
    font_bytes: Option<Vec<u8>>,
    /// Named per-family payloads consulted via `BytesResolver` during
    /// every (re)build. Owned by the model so the assets resolver
    /// borrowed in `PipelineOptions` doesn't need lifetimes leaking out.
    font_registry: Vec<FontEntry>,
    icc_bytes: Option<Vec<u8>>,
    /// Concept 2 — the load-time profile bytes (explicit
    /// `CanvasOptions::cmyk_icc_profile` or the designmap-name
    /// registry hit). `SetColorSettings { cmyk_profile_name: None }`
    /// restores these.
    initial_icc_bytes: Option<Vec<u8>>,
    /// Concept 2 — named profile registry (name → ICC bytes).
    color_profiles: std::collections::BTreeMap<String, Vec<u8>>,
    /// Concept 2 — active colour-management settings.
    pub color_settings: ColorSettingsState,
    /// Concept 2 — active soft-proof state (`None` = proofing off).
    proof_state: Option<ProofState>,
    /// Concept 2 (Ink Manager) — per-spot output-time settings,
    /// keyed by the spot swatch's `Color/<id>`. Never edits the
    /// swatch (AC-8); consumed by Concept 3's separations encoding.
    ink_settings: std::collections::BTreeMap<String, InkSetting>,
    /// Concept 2 (Ink Manager) — prefer spot Lab primaries over
    /// CMYK alternates in preview resolution.
    use_standard_lab_for_spots: bool,
    /// Concept 2 — lazily-built CMM for preview/compute reads
    /// (transform creation is expensive; the SwatchPicker previews
    /// every swatch per refresh). Cleared by `SetColorSettings`.
    cmm_cache: std::cell::RefCell<Option<std::rc::Rc<paged_color::IccCmm>>>,
    /// Phase 3 Item 6 — content hash of the scene at load time.
    /// Drives determinism tests: replaying the recorded mutation log
    /// against the same `initial_state_hash` must produce a matching
    /// post-state hash.
    initial_state_hash: [u8; 32],
    /// Monotone counter assigned by the worker for each successfully
    /// applied mutation. The main thread matches against its own
    /// `client_seq` via the `MutationApplied` reply.
    last_applied_seq: u64,
    /// Active text selection mirrored from the main thread.
    pub current_selection: Option<crate::selection::ContentSelection>,
    /// Editor-ops — document defaults for newly-created objects
    /// (`InsertFrame` / `InsertLine` / `InsertPath`). App-level state
    /// written by `Mutation::SetDocumentDefaults`: not undoable, never
    /// enters the Operation log, surfaced through `DocumentMeta`.
    pub document_defaults: DocumentDefaults,
    /// Phase A — active element selection (frames, images, vectors).
    /// Mirrored from the main thread, never enters the Operation log.
    /// Survives mutations and re-layout; the main thread is responsible
    /// for clearing entries whose target was removed.
    pub element_selection: crate::element_selection::ElementSelection,
    /// Phase B — in-flight gesture (translate / future resize / rotate).
    /// Only one gesture is active at a time; `begin_gesture` errors on
    /// re-entry. None when no drag is happening.
    pub(crate) active_gesture: Option<crate::gesture::GestureSession>,
    /// Monotone counter for `GestureHandle` allocation. Bumped on
    /// every successful `begin_gesture`. Persists across cancels so a
    /// stale handle from a previous gesture can never collide with a
    /// fresh one.
    pub(crate) next_gesture_handle: u64,
    /// Phase 3 Item 7 — undo log. Each entry holds the op + inverse
    /// + the applied_seq that was assigned at apply time.
    applied_log: Vec<AppliedRecord>,
    /// Phase 3 Item 7 — redo stack. Populated by `undo()`; consumed
    /// by `redo()`. Cleared when a new mutation lands (standard
    /// editor convention).
    redo_log: Vec<AppliedRecord>,
    /// Phase 4 Step 1 — persistent per-paragraph layout cache.
    /// Installed on every rebuild via `paged_text::cache::with_layout_cache`
    /// so unchanged paragraphs short-circuit Knuth-Plass on
    /// mutation-driven rebuilds. Survives across mutations.
    layout_cache: paged_text::LayoutCache,
    /// Phase 4 Step 3 — map of story id → page ids the story's
    /// frame chain touches. Built after every rebuild. Used by
    /// `apply_mutation` to compute the dirty page set for the GPU
    /// scene cache invalidation hint.
    story_pages: HashMap<String, Vec<PageId>>,
    /// Perf-S — persistent `URI → DecodedImage` cache shared across
    /// rebuilds. Threaded into `PipelineOptions::image_decode_cache`
    /// on every `rebuild_after_mutation` so gesture-driven rebuilds
    /// don't re-decode placed images. Per the corrected perf
    /// investigation, image decoding isn't actually the bottleneck
    /// in the current test harness; this stays as forward-looking
    /// infra for when asset resolvers wire up.
    image_decode_cache: std::cell::RefCell<HashMap<String, paged_compose::DecodedImage>>,
    /// Perf-FontTable — pre-built shaping table reused across every
    /// `rebuild_after_mutation`. The `FontTable::build` walk costs
    /// ~225ms on a multi-spread fixture (harvests every paragraph's
    /// cascade-resolved font key, then resolver-fetches bytes per
    /// key). The document's font registry only changes at
    /// loadDocument boundaries — fresh CanvasModel ⇒ fresh table —
    /// so we never need to invalidate mid-lifetime.
    font_table: paged_renderer::FontTable,
    /// Perf-MasterText — per-(master_frame_self_id, page_idx) cache
    /// of the DisplayList delta the master-text pass appends to a
    /// page. The COLD build populates this; every gesture-driven
    /// rebuild hits and skips the emit. Structural mutations clear
    /// it (handled in `apply_operation`) because the master+frame
    /// pass's path-buffer state changes when frames are added/
    /// removed and the cached relative-path-id rebase would
    /// produce visually-correct but order-divergent output. ~161ms
    /// savings per rebuild on a multi-spread fixture.
    master_text_emit_cache:
        std::cell::RefCell<HashMap<(String, usize), paged_renderer::MasterTextEmitDelta>>,
    /// Perf-BodyStory — per-(story_self_id, signature) cache of
    /// the multi-page body-story emission delta. Signature
    /// hashes the chain's frames + wrap_rects on chain pages, so
    /// a story whose chain doesn't see a change keeps hitting
    /// through a drag. Body-story emission is the largest single
    /// cost in `build_document` on a multi-spread fixture
    /// (~613ms); most stories are unaffected by any given gesture
    /// so the hit ratio is high. Cleared by `apply_operation` on
    /// structural commits.
    body_story_emit_cache:
        std::cell::RefCell<HashMap<(String, u64), paged_renderer::BodyStoryEmissionDelta>>,
    /// W1.24 (audit B18) — stats for the most recent rebuild. Refreshed
    /// by `rebuild_after_mutation` (build timing + sizes) and by the
    /// mutation entrypoints (op-apply timing). Read via
    /// `last_rebuild_stats`.
    rebuild_stats: RebuildStats,
    /// W1.24 (audit B18) — op-apply duration (ms) staged by a mutation
    /// entrypoint just before it calls `rebuild_after_mutation`, which
    /// folds it into `rebuild_stats`. Reset to 0 by every rebuild after
    /// it is consumed, so a view-state rebuild (no preceding edit) reads
    /// 0 rather than a stale value.
    pending_op_apply_ms: f64,
}

/// W1.24 (audit B19) — hard cap on the undo log's length.
///
/// `applied_log` is the **undo stack**: each entry pairs a forward op
/// with the pre-captured inverse that reverses it (see `undo` / `redo`).
/// It is NOT the save-back source (that is `source_idml` + the live
/// scene; `export_idml` re-serialises the current scene, never replays
/// the log) and NOT the determinism replay source (the determinism
/// tests build their own op list). So the only correctness contract the
/// log carries is: *the most recent N mutations can be undone.* Bounding
/// it to the N freshest entries therefore costs only the ability to undo
/// past the cap — a deliberate, documented trade most editors make
/// (InDesign itself bounds undo). We evict from the FRONT (oldest first)
/// so the freshest `CAP` mutations always stay undoable; the redo stack
/// is a transient of in-session undo and is not separately capped (it can
/// never exceed what was undone, which is bounded by the same cap).
///
/// 10_000 entries: at a generous ~1 KiB/entry (a `TextOp` inverse holds a
/// deleted-text `String`; a `Frame` inverse holds an `AppliedOperation`)
/// that is ~10 MiB worst case — well under the multi-MB `source_idml`
/// copy the model already holds, and far more undo depth than any human
/// session reaches. Pragmatic count cap over a byte-accounting scheme:
/// the per-entry size is bounded in practice and a count is O(1) to
/// enforce without walking the payloads.
pub const MAX_APPLIED_LOG: usize = 10_000;

/// One entry in the applied / redo logs.
///
/// Phase B — generalized to hold both text edits (legacy `TextOp`
/// path) and frame mutations (canonical `paged_mutate::AppliedOperation`)
/// so a single Cmd-Z timeline covers both. The full convergence
/// (folding `TextOp` into `paged_mutate::Operation`) is tracked
/// separately and is **out of scope** for Phase B per the plan §3.5.
#[derive(Debug, Clone)]
pub struct AppliedRecord {
    pub applied_seq: u64,
    pub kind: LoggedMutation,
}

/// W1.24 (audit B18) — per-rebuild timing + size instrumentation.
///
/// Captured by `rebuild_after_mutation` (and the initial `load`) on every
/// pipeline run so native callers can read "the last relayout took X ms
/// over N pages" without wiring a `Clock` of their own, and the wasm
/// dispatch can fold the breakdown onto the wire `LayoutCacheStats`
/// additively. Timing uses the same monotonic-on-native /
/// `js_sys::Date::now`-on-wasm source the crate's `phase_*` helpers use,
/// so it compiles and runs on `wasm32` (no `std::time::Instant`, which
/// panics there).
///
/// `op_apply_ms` is filled by the mutation entrypoints (`apply_mutation`
/// / `apply_operation` / `undo` / `redo`) which own the scene edit that
/// precedes the rebuild; `rebuild_after_mutation` only knows `build_ms`,
/// so it leaves `op_apply_ms` at its prior value. Read the pair together
/// via `last_rebuild_stats`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RebuildStats {
    /// Wall-clock of the `pipeline::build_document` call inside the most
    /// recent rebuild, in milliseconds.
    pub build_ms: f64,
    /// Wall-clock of the scene-edit (`paged_mutate::apply` /
    /// `crate::mutate::apply`) that preceded the rebuild, in
    /// milliseconds. 0.0 for the initial `load` (no edit) and for
    /// rebuilds triggered by view-state changes (colour settings).
    pub op_apply_ms: f64,
    /// Pages in the freshly built document.
    pub pages: usize,
    /// `PipelineStats::paragraphs` — relayout cost scales with this.
    pub paragraphs: usize,
    /// Monotone count of rebuilds this model has run (initial load = 1).
    /// Lets a HUD show "rebuild #K" and tests assert monotonicity.
    pub rebuilds: u64,
    /// Undo-log depth right after this rebuild (the B19 cap is visible
    /// here: it never exceeds `MAX_APPLIED_LOG`).
    pub applied_log_len: usize,
}

// Boxing the large `Frame` payload would change this public, re-exported
// enum's variant signature and every construction/match site — out of
// scope for a lint pass. The undo log is short-lived, not bulk-allocated.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum LoggedMutation {
    /// Legacy text edit. `op` is the forward action; `inverse` is the
    /// pre-captured `TextOp` that reverses it. Both live in
    /// `crate::mutate`.
    Text {
        op: crate::mutate::TextOp,
        inverse: crate::mutate::TextOp,
    },
    /// Frame / structural mutation routed through `paged_mutate::apply`.
    /// The `AppliedOperation` already pairs op + inverse + invalidation.
    Frame(paged_mutate::AppliedOperation),
}

impl CanvasModel {
    /// Parse `bytes` and run the renderer pipeline to produce the
    /// initial `BuiltDocument`. Returns a `CanvasModel` the worker
    /// uses to serve all subsequent queries.
    pub fn load(
        doc_id: impl Into<String>,
        bytes: &[u8],
        opts: CanvasOptions,
    ) -> Result<Self, LoadError> {
        let doc_id = doc_id.into();
        let t_parse = phase_now();
        let scene = Document::open(bytes).map_err(|e| LoadError::Parse(e.to_string()))?;
        phase_log("CanvasModel::load parse", t_parse);

        // Honour the first font and the ICC profile. Take ownership
        // up-front so the model is self-contained — no caller-managed
        // lifetimes leaking through.
        let font_bytes = opts.fonts.into_iter().next();
        let font_registry = opts.font_registry;
        // Concept 2 — profile registry + activation precedence:
        // explicit CanvasOptions::cmyk_icc_profile wins; else a
        // registered profile whose name matches the designmap's
        // CMYKProfile attribute activates automatically (the
        // document "names" its working space and the editor shipped
        // it); else no profile (naive conversion, as before).
        let color_profiles: std::collections::BTreeMap<String, Vec<u8>> = opts
            .color_profiles
            .into_iter()
            .map(|p| (p.name, p.bytes))
            .collect();
        // The ACTIVE intent stays RelativeColorimetric+BPC until an
        // explicit SetColorSettings: poppler (the fidelity
        // reference) hardcodes RelCol, and our lcms2 setup is
        // calibrated against pdftoppm — honouring a designmap-
        // declared Perceptual at load would silently diverge the
        // default render from the reference. The declared name is
        // still surfaced via DocumentMeta for the settings dialog.
        let mut color_settings = ColorSettingsState::default();
        let designmap_settings = scene.container.designmap.color_settings.clone();
        let icc_bytes = match opts.cmyk_icc_profile {
            Some(bytes) => Some(bytes),
            None => designmap_settings.cmyk_profile.as_ref().and_then(|name| {
                let hit = color_profiles.get(name).cloned();
                if hit.is_some() {
                    color_settings.cmyk_profile_name = Some(name.clone());
                }
                hit
            }),
        };
        let resolver = build_font_resolver(&font_registry, font_bytes.as_deref());

        let t_build = phase_now();
        // Perf-S — image-decode cache populated by the initial
        // build_document then stored on Self below for subsequent
        // rebuilds. The cold load pays the full decode cost; every
        // mutation-driven rebuild after that reuses.
        let image_decode_cache: std::cell::RefCell<HashMap<String, paged_compose::DecodedImage>> =
            std::cell::RefCell::new(HashMap::new());
        // Perf-FontTable — pre-build the shaping table once so the
        // initial build_document + every subsequent
        // rebuild_after_mutation skips the harvest walk
        // (~225ms/call on a multi-spread fixture).
        let font_table_options = PipelineOptions {
            font: font_bytes.as_deref(),
            assets: resolver
                .as_ref()
                .map(|r| r as &dyn paged_renderer::AssetResolver),
            cmyk_icc_profile: icc_bytes.as_deref(),
            ..PipelineOptions::default()
        };
        let font_table = paged_renderer::FontTable::build(&scene, &font_table_options);
        // Perf-MasterText — empty cache; the initial build_document
        // below populates it as each master-text emit runs.
        let master_text_emit_cache: std::cell::RefCell<
            HashMap<(String, usize), paged_renderer::MasterTextEmitDelta>,
        > = std::cell::RefCell::new(HashMap::new());
        // Perf-BodyStory — same pattern; populated by the initial
        // build, reused by every subsequent rebuild.
        let body_story_emit_cache: std::cell::RefCell<
            HashMap<(String, u64), paged_renderer::BodyStoryEmissionDelta>,
        > = std::cell::RefCell::new(HashMap::new());
        let (built_result, layout_cache) = {
            let options = PipelineOptions {
                font: font_bytes.as_deref(),
                assets: resolver
                    .as_ref()
                    .map(|r| r as &dyn paged_renderer::AssetResolver),
                cmyk_icc_profile: icc_bytes.as_deref(),
                image_decode_cache: Some(&image_decode_cache),
                pre_built_font_table: Some(&font_table),
                master_text_emit_cache: Some(&master_text_emit_cache),
                body_story_emit_cache: Some(&body_story_emit_cache),
                ..PipelineOptions::default()
            };
            // Phase 4 Step 1 — install an empty cache for the initial
            // build. Every paragraph misses; the cache fills up so
            // subsequent mutation-driven rebuilds can hit.
            paged_text::cache::with_layout_cache(paged_text::LayoutCache::default(), || {
                pipeline::build_document(&scene, &options)
            })
        };
        // W1.24 (audit B18) — capture the cold build duration before
        // the `?` so the seeded `rebuild_stats` carries real timing even
        // on the first build. `phase_elapsed_ms` reads the same monotone
        // source `phase_log` does, so this is one extra clock read.
        let load_build_ms = phase_elapsed_ms(t_build);
        let built = built_result.map_err(|e| LoadError::Build(e.to_string()))?;
        phase_log("CanvasModel::load build", t_build);

        // W1.24 (audit B18) — snapshot the sizes before `built` moves
        // into Self so the seeded `rebuild_stats` reads the cold build.
        let load_pages = built.pages.len();
        let load_paragraphs = built.stats.paragraphs;

        let t_post = phase_now();
        let page_index = built
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();

        let initial_state_hash = scene.canonical_hash();
        let story_pages = compute_story_pages(&built);
        phase_log("CanvasModel::load post (index+hash+story_pages)", t_post);
        Ok(Self {
            doc_id,
            scene,
            // W3.B2 — retain the source package for save-back. One
            // compressed copy; replaced wholesale on the next load.
            source_idml: bytes.to_vec(),
            built,
            page_index,
            font_bytes,
            font_registry,
            initial_icc_bytes: icc_bytes.clone(),
            icc_bytes,
            color_profiles,
            color_settings,
            proof_state: None,
            ink_settings: Default::default(),
            use_standard_lab_for_spots: false,
            cmm_cache: std::cell::RefCell::new(None),
            initial_state_hash,
            last_applied_seq: 0,
            current_selection: None,
            document_defaults: DocumentDefaults::default(),
            element_selection: crate::element_selection::ElementSelection::new(),
            active_gesture: None,
            next_gesture_handle: 0,
            applied_log: Vec::new(),
            redo_log: Vec::new(),
            layout_cache,
            story_pages,
            // Perf-S — cache populated by the initial build_document
            // above; subsequent rebuild_after_mutation calls share
            // this RefCell so decode cost amortises.
            image_decode_cache,
            // Perf-FontTable — built once above and reused by every
            // rebuild_after_mutation.
            font_table,
            // Perf-MasterText — cache populated by the initial
            // build above; subsequent rebuilds reuse + structural
            // mutations clear it.
            master_text_emit_cache,
            // Perf-BodyStory — same lifecycle as master_text.
            body_story_emit_cache,
            // W1.24 (audit B18) — seeded with the cold-build sizes +
            // timing so a caller that never mutates still sees a
            // populated, plausible stat. `rebuilds` starts at 1: the
            // initial build counts as the model's first rebuild.
            rebuild_stats: RebuildStats {
                build_ms: load_build_ms,
                op_apply_ms: 0.0,
                pages: load_pages,
                paragraphs: load_paragraphs,
                rebuilds: 1,
                applied_log_len: 0,
            },
            pending_op_apply_ms: 0.0,
        })
    }

    /// Initial canonical hash captured at load. Phase 3 Item 6 —
    /// determinism tests assert that replaying the mutation log
    /// against the same `initial_state_hash` reproduces a known
    /// post-state hash.
    pub fn initial_state_hash(&self) -> [u8; 32] {
        self.initial_state_hash
    }

    /// Current canonical hash of the (possibly-mutated) scene.
    pub fn current_state_hash(&self) -> [u8; 32] {
        self.scene.canonical_hash()
    }

    /// Most-recently-assigned `applied_seq`. 0 if no mutations have
    /// landed.
    pub fn last_applied_seq(&self) -> u64 {
        self.last_applied_seq
    }

    /// Increment + return the next applied_seq. Worker calls this
    /// when assigning ordering to a mutation that successfully
    /// applied.
    pub fn bump_applied_seq(&mut self) -> u64 {
        self.last_applied_seq += 1;
        self.last_applied_seq
    }

    pub fn doc_id(&self) -> &str {
        &self.doc_id
    }

    pub fn handle(&self) -> DocumentHandle {
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        let page_sizes_pt: Vec<(f32, f32)> = self
            .built
            .pages
            .iter()
            .map(|p| (p.width_pt, p.height_pt))
            .collect();
        // Plan-2 §8.3 — flatten per-spread `<Guide>` parses into a
        // page-id-keyed list. IDML's `PageIndex` is 1-based per
        // spread (single-page spreads use `PageIndex="1"` for their
        // only page); we subtract 1 and clamp to the last valid
        // page so off-by-one or absent attributes still surface the
        // guide on the spread's first page.
        let mut ruler_guides: Vec<RulerGuideWire> = Vec::new();
        for parsed in &self.scene.spreads {
            let pages = &parsed.spread.pages;
            if pages.is_empty() {
                continue;
            }
            for g in &parsed.spread.guides {
                let idx = if g.page_index == 0 {
                    0
                } else {
                    ((g.page_index as usize) - 1).min(pages.len() - 1)
                };
                let Some(p) = pages.get(idx) else {
                    continue;
                };
                let Some(sid) = p.self_id.clone() else {
                    continue;
                };
                ruler_guides.push(RulerGuideWire {
                    page_id: PageId(sid),
                    orientation: match g.orientation {
                        paged_parse::GuideOrientation::Vertical => GuideOrientationWire::Vertical,
                        paged_parse::GuideOrientation::Horizontal => {
                            GuideOrientationWire::Horizontal
                        }
                    },
                    location: g.location,
                });
            }
        }
        let mut stats = DocumentStats::from(&self.built.stats);
        // panels.md gap 1 — count distinct overset stories from the
        // build's render diagnostics (the `From<&PipelineStats>` path
        // can't see them).
        stats.overset_stories = self.built.diagnostics.overset_story_ids().len();
        DocumentHandle {
            doc_id: self.doc_id.clone(),
            page_count: self.built.pages.len(),
            page_ids,
            page_sizes_pt,
            stats,
            ruler_guides,
        }
    }

    pub fn page_count(&self) -> usize {
        self.built.pages.len()
    }

    pub fn page_ids(&self) -> impl Iterator<Item = &PageId> {
        self.built.page_ids()
    }

    pub fn page(&self, id: &PageId) -> Option<&BuiltPage> {
        self.page_index.get(id).map(|&i| &self.built.pages[i])
    }

    /// Tier 4 seam: the per-page display list the worker hands to
    /// the GPU rasterizer (Vello in `apps/canvas/`, tiny-skia in
    /// headless tests).
    pub fn display_list_for_page(&self, id: &PageId) -> Option<&DisplayList> {
        self.page(id).map(|p| &p.list)
    }

    /// Apply a `Mutation` from the main thread. Phase 3 — InsertText
    /// and DeleteRange route through `crate::mutate`; other variants
    /// (style, frame, page, structural) still return `NotImplemented`
    /// until Items 5b/c + 8 land.
    ///
    /// On success: bumps `last_applied_seq`, returns the
    /// `applied_seq` + the inverse op (for the caller's undo log) +
    /// the list of affected page ids (caller invalidates the LOD
    /// cache for them). Rebuild is full + synchronous —
    /// "correctness, not pessimisation" (see plan §Item 5).
    pub fn apply_mutation(
        &mut self,
        mutation: &Mutation,
    ) -> Result<MutationOutcome, crate::channel::WorkerError> {
        // Editor-ops — document defaults are app-level state, not a
        // scene edit: no rebuild, no undo entry, no pixel change. The
        // editor reads the triple back via `DocumentMeta`.
        if let Mutation::SetDocumentDefaults {
            fill_color,
            stroke_color,
            stroke_weight,
        } = mutation
        {
            self.document_defaults = DocumentDefaults {
                fill_color: fill_color.clone(),
                stroke_color: stroke_color.clone(),
                stroke_weight: *stroke_weight,
            };
            let applied_seq = self.bump_applied_seq();
            return Ok(MutationOutcome {
                applied_seq,
                page_ids: Vec::new(),
                inverse: crate::mutate::TextOp::InsertText {
                    story_id: String::new(),
                    offset: 0,
                    text: String::new(),
                    cell: None,
                },
                created_id: None,
                page_structure_changed: false,
            });
        }
        // Concept 2 — colour-management settings: whole-state app
        // config like SetDocumentDefaults (not undoable, no log
        // entry) but with a FORCED full rebuild — the working space
        // / intent / BPC change what every CMYK swatch resolves to
        // on screen (AC-3).
        if let Mutation::SetColorSettings {
            cmyk_profile_name,
            rgb_policy,
            intent,
            bpc,
        } = mutation
        {
            let next_bytes = match cmyk_profile_name {
                Some(name) => Some(self.color_profiles.get(name).cloned().ok_or_else(
                    || crate::channel::WorkerError::NotImplemented {
                        what: format!(
                            "unknown color profile {name:?} — register it via RegisterColorProfile first"
                        ),
                    },
                )?),
                None => None,
            };
            self.icc_bytes = match next_bytes {
                Some(bytes) => Some(bytes),
                // Name cleared → back to the load-time profile.
                None => self.initial_icc_bytes.clone(),
            };
            self.color_settings = ColorSettingsState {
                cmyk_profile_name: cmyk_profile_name.clone(),
                rgb_policy: rgb_policy.clone(),
                intent: intent
                    .as_deref()
                    .and_then(paged_color::Intent::from_name)
                    .unwrap_or(paged_color::Intent::RelativeColorimetric),
                bpc: bpc.unwrap_or(true),
            };
            // The transform inputs changed out from under every
            // cached paint — full rebuild, everything repaints. The
            // preview CMM rebuilds lazily from the new state.
            self.cmm_cache.borrow_mut().take();
            self.rebuild_after_mutation().map_err(|e| {
                crate::channel::WorkerError::NotImplemented {
                    what: format!("rebuild after SetColorSettings: {e}"),
                }
            })?;
            let applied_seq = self.bump_applied_seq();
            let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
            return Ok(MutationOutcome {
                applied_seq,
                page_ids,
                inverse: crate::mutate::TextOp::InsertText {
                    story_id: String::new(),
                    offset: 0,
                    text: String::new(),
                    cell: None,
                },
                created_id: None,
                page_structure_changed: false,
            });
        }
        // Concept 2 — soft-proof toggle/setup: swap the display
        // transform's inputs (proof profile + absolute intent for
        // paper white) and repaint. Not undoable; view-state like
        // colour settings.
        if let Mutation::SetProofSetup {
            profile_name,
            simulate_paper_white,
            intent,
        } = mutation
        {
            self.proof_state = match profile_name {
                Some(name) => {
                    let bytes = self.color_profiles.get(name).cloned().ok_or_else(|| {
                        crate::channel::WorkerError::NotImplemented {
                            what: format!(
                                "unknown proof profile {name:?} — register it via RegisterColorProfile first"
                            ),
                        }
                    })?;
                    Some(ProofState {
                        name: name.clone(),
                        bytes,
                        intent: intent
                            .as_deref()
                            .and_then(paged_color::Intent::from_name)
                            .unwrap_or(paged_color::Intent::RelativeColorimetric),
                        simulate_paper_white: *simulate_paper_white,
                    })
                }
                None => None,
            };
            self.rebuild_after_mutation().map_err(|e| {
                crate::channel::WorkerError::NotImplemented {
                    what: format!("rebuild after SetProofSetup: {e}"),
                }
            })?;
            let applied_seq = self.bump_applied_seq();
            let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
            return Ok(MutationOutcome {
                applied_seq,
                page_ids,
                inverse: crate::mutate::TextOp::InsertText {
                    story_id: String::new(),
                    offset: 0,
                    text: String::new(),
                    cell: None,
                },
                created_id: None,
                page_structure_changed: false,
            });
        }
        // Concept 2 (Ink Manager) — output-time ink settings.
        // Whole-row replace; not undoable; swatch identity untouched.
        if let Mutation::SetInkSetting {
            spot_id,
            convert_to_process,
            alias_to,
        } = mutation
        {
            let is_spot = self
                .scene
                .palette
                .colors
                .get(spot_id)
                .is_some_and(|c| c.model == paged_parse::graphic::ColorModel::Spot);
            if !is_spot {
                return Err(crate::channel::WorkerError::NotImplemented {
                    what: format!("{spot_id:?} is not a spot swatch"),
                });
            }
            self.ink_settings.insert(
                spot_id.clone(),
                InkSetting {
                    convert_to_process: *convert_to_process,
                    alias_to: alias_to.clone(),
                },
            );
            let applied_seq = self.bump_applied_seq();
            return Ok(MutationOutcome {
                applied_seq,
                page_ids: Vec::new(),
                inverse: crate::mutate::TextOp::InsertText {
                    story_id: String::new(),
                    offset: 0,
                    text: String::new(),
                    cell: None,
                },
                created_id: None,
                page_structure_changed: false,
            });
        }
        if let Mutation::SetUseStandardLabForSpots { enabled } = mutation {
            self.use_standard_lab_for_spots = *enabled;
            // Preview resolution changes for Lab-primary spots; the
            // canvas itself repaints with Concept 3's separations
            // threading. Clear the preview CMM so reads see the flag.
            self.cmm_cache.borrow_mut().take();
            let applied_seq = self.bump_applied_seq();
            return Ok(MutationOutcome {
                applied_seq,
                page_ids: Vec::new(),
                inverse: crate::mutate::TextOp::InsertText {
                    story_id: String::new(),
                    offset: 0,
                    text: String::new(),
                    cell: None,
                },
                created_id: None,
                page_structure_changed: false,
            });
        }
        // Phase B — route frame-shape mutations through the canonical
        // `paged_mutate::apply` path; only the text mutations stay on
        // the legacy `TextOp` log. The `MutationOutcome` is text-only
        // (carries an inverse `TextOp`), so frame-shape mutations
        // synthesise an empty text op into the response. Future
        // convergence folds both into one shape.
        if let Some(op) = self.try_translate_frame_mutation_to_operation(mutation, &mut 0) {
            let outcome = self.apply_operation(op)?;
            let created_id = created_element_id(&outcome.applied.op);
            // Page-list mutations (M7 extends this set with ResizePage)
            // require the editor to rebuild its page grid.
            let page_structure_changed = matches!(
                mutation,
                Mutation::InsertPage { .. }
                    | Mutation::DeletePage { .. }
                    | Mutation::ResizePage { .. }
                    // W0.5 — DuplicatePage adds a spread; the others
                    // change page labels/masters but keep the page
                    // count, so they don't force a grid rebuild.
                    | Mutation::DuplicatePage { .. }
            );
            return Ok(MutationOutcome {
                applied_seq: outcome.applied_seq,
                page_ids: outcome.page_ids,
                // No text op to send back — frame mutations carry their
                // inverse in the AppliedOperation stored on the log.
                inverse: crate::mutate::TextOp::InsertText {
                    story_id: String::new(),
                    offset: 0,
                    text: String::new(),
                    cell: None,
                },
                created_id,
                page_structure_changed,
            });
        }
        let text_op: crate::mutate::TextOp = match mutation {
            Mutation::InsertText {
                story_id,
                offset,
                text,
                cell,
            } => crate::mutate::TextOp::InsertText {
                story_id: story_id.clone(),
                offset: *offset,
                text: text.clone(),
                cell: cell.clone(),
            },
            Mutation::DeleteRange {
                story_id,
                start,
                end,
                cell,
            } => crate::mutate::TextOp::DeleteRange {
                story_id: story_id.clone(),
                start: *start,
                end: *end,
                recovered: String::new(),
                cell: cell.clone(),
            },
            other => {
                return Err(crate::channel::WorkerError::NotImplemented {
                    what: format!("Mutation::{}", other.discriminant()),
                })
            }
        };
        // W1.24 (audit B18) — time the scene edit; the next rebuild
        // folds it into RebuildStats.op_apply_ms.
        let t_op = phase_now();
        let applied = crate::mutate::apply(&mut self.scene, &text_op).map_err(|e| {
            crate::channel::WorkerError::NotImplemented {
                what: format!("text mutation failed: {e}"),
            }
        })?;
        self.stage_op_apply_ms(phase_elapsed_ms(t_op));
        // Perf-BodyStory — text edits change the *content* of a story
        // but not its frame chain, so the body-story signature would
        // wrongly match and the edit would never display. Blow the
        // cache; the rebuild repopulates from the new content. We
        // also clear master_text for symmetry with apply_operation —
        // text in a master is rare but if it happens we want the
        // same invariant.
        self.master_text_emit_cache.borrow_mut().clear();
        self.body_story_emit_cache.borrow_mut().clear();
        self.rebuild_after_mutation()
            .map_err(|e| crate::channel::WorkerError::NotImplemented {
                what: format!("rebuild after mutation: {e}"),
            })?;
        let applied_seq = self.bump_applied_seq();
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        // Shift the active selection through the mutation so caret
        // tracking survives the edit (AC-E-9).
        if let Some(sel) = self.current_selection.take() {
            let shifted = match &text_op {
                crate::mutate::TextOp::InsertText {
                    story_id,
                    offset,
                    text,
                    cell,
                } => sel.shift_for_insert(story_id, cell, *offset, text.chars().count() as u32),
                crate::mutate::TextOp::DeleteRange {
                    story_id,
                    start,
                    end,
                    cell,
                    ..
                } => sel.shift_for_delete(story_id, cell, *start, *end),
            };
            self.current_selection = Some(shifted);
        }
        // Phase 3 Item 7 — push to undo log; clear redo log (any
        // pending redo is invalidated by a fresh mutation). W1.24
        // (B19) — routed through `push_applied` so the MAX_APPLIED_LOG
        // cap is enforced.
        self.push_applied(AppliedRecord {
            applied_seq,
            kind: LoggedMutation::Text {
                op: text_op,
                inverse: applied.inverse.clone(),
            },
        });
        self.redo_log.clear();
        Ok(MutationOutcome {
            applied_seq,
            page_ids,
            inverse: applied.inverse,
            created_id: None,
            page_structure_changed: false,
        })
    }

    /// Phase B — convert a channel `Mutation` into an
    /// `paged_mutate::Operation` when the mutation is a frame-shape
    /// edit. Returns `None` for text edits + any mutation kind not yet
    /// bridged (MoveFrame, InsertFrame, etc.).
    /// `mint_offset` (FINDING #6) tracks how many page-item ids this
    /// translation pass has already minted, so a `Batch` of N inserts
    /// gets N distinct ids instead of N copies of `u<max+1>`. Top-level
    /// callers pass `&mut 0`; the `Batch` arm threads the SAME counter
    /// through every child so the ids stay unique across the batch.
    fn try_translate_frame_mutation_to_operation(
        &self,
        mutation: &Mutation,
        mint_offset: &mut u64,
    ) -> Option<paged_mutate::Operation> {
        use paged_mutate::{NodeId, Operation, PropertyPath, Value};
        match mutation {
            Mutation::ResizeFrame { frame_id, bounds } => {
                let node = self.resolve_frame_node_id(frame_id)?;
                Some(Operation::SetProperty {
                    node,
                    path: PropertyPath::FrameBounds,
                    // Channel carries `(top, left, bottom, right)`;
                    // `Value::Bounds` is `[top, left, bottom, right]`.
                    value: Value::Bounds([bounds.0, bounds.1, bounds.2, bounds.3]),
                })
            }
            // W1.2 — un-stub `MoveFrame`. The stub deferred to "use the
            // translate gesture"; the translate gesture commits its move
            // by SETTING the frame's `ItemTransform` (the Phase D
            // `FrameTransform` path), so `moveFrame` is semantically a
            // whole-`ItemTransform` write to the same matrix the gesture
            // would land. The Phase D apply arm captures the prior
            // transform as the inverse, so apply→invert→reapply
            // round-trips without any new inverse machinery.
            Mutation::MoveFrame {
                frame_id,
                transform,
            } => {
                let node = self.resolve_frame_node_id(frame_id)?;
                Some(Operation::SetProperty {
                    node,
                    path: PropertyPath::FrameTransform,
                    value: Value::Transform(Some(*transform)),
                })
            }
            Mutation::InsertFrame { page_id, bounds } => {
                let (spread_id, (ox, oy), idx) = self.page_insert_context(page_id)?;
                // Append to the kind vec = top of the z-order.
                let position = self.scene.spreads[idx].spread.rectangles.len();
                let d = &self.document_defaults;
                Some(Operation::InsertNode {
                    parent: NodeId::Spread(spread_id),
                    position,
                    node: paged_mutate::NodeSpec::Rectangle {
                        self_id: self.mint_page_item_id_with_offset(mint_offset),
                        // Page-local (top, left, bottom, right) →
                        // spread coords (the marquee_hits rule: y axes
                        // shift by origin.y, x axes by origin.x).
                        bounds: [bounds.0 + oy, bounds.1 + ox, bounds.2 + oy, bounds.3 + ox],
                        fill_color: d.fill_color.clone(),
                        stroke_color: d.stroke_color.clone(),
                        stroke_weight: d.stroke_weight,
                        // fresh creations carry no transform
                        item_transform: None,
                    },
                    z_slot: None,
                })
            }
            Mutation::InsertTextFrame { page_id, bounds } => {
                let (spread_id, (ox, oy), idx) = self.page_insert_context(page_id)?;
                let position = self.scene.spreads[idx].spread.text_frames.len();
                Some(Operation::InsertNode {
                    parent: NodeId::Spread(spread_id),
                    position,
                    node: paged_mutate::NodeSpec::TextFrame {
                        self_id: self.mint_page_item_id_with_offset(mint_offset),
                        bounds: [bounds.0 + oy, bounds.1 + ox, bounds.2 + oy, bounds.3 + ox],
                        // Text frames carry no fill/stroke by default —
                        // an empty threading/Type-tool target.
                        fill_color: None,
                        stroke_color: None,
                        stroke_weight: None,
                        item_transform: None,
                    },
                    z_slot: None,
                })
            }
            Mutation::InsertLine {
                page_id,
                start,
                end,
            } => {
                let (spread_id, (ox, oy), idx) = self.page_insert_context(page_id)?;
                let position = self.scene.spreads[idx].spread.graphic_lines.len();
                let s = [start.0 + ox, start.1 + oy];
                let e = [end.0 + ox, end.1 + oy];
                let corner = |p: [f32; 2]| paged_mutate::operation::PathAnchorSpec {
                    anchor: p,
                    left: p,
                    right: p,
                };
                let d = &self.document_defaults;
                Some(Operation::InsertNode {
                    parent: NodeId::Spread(spread_id),
                    position,
                    node: paged_mutate::NodeSpec::GraphicLine {
                        self_id: self.mint_page_item_id_with_offset(mint_offset),
                        bounds: [
                            s[1].min(e[1]),
                            s[0].min(e[0]),
                            s[1].max(e[1]),
                            s[0].max(e[0]),
                        ],
                        anchors: vec![corner(s), corner(e)],
                        subpath_starts: vec![0],
                        subpath_open: vec![true],
                        // A strokeless line is invisible — fall back to
                        // a 1pt black stroke when no default is set.
                        stroke_color: d
                            .stroke_color
                            .clone()
                            .or_else(|| Some("Color/Black".to_string())),
                        stroke_weight: d.stroke_weight.or(Some(1.0)),
                        // fresh creations carry no transform
                        item_transform: None,
                    },
                    z_slot: None,
                })
            }
            Mutation::InsertPath {
                page_id,
                anchors,
                open,
                smooth,
            } => {
                let (spread_id, (ox, oy), idx) = self.page_insert_context(page_id)?;
                let position = self.scene.spreads[idx].spread.polygons.len();
                let mut conv: Vec<paged_mutate::operation::PathAnchorSpec> = anchors
                    .iter()
                    .map(|a| paged_mutate::operation::PathAnchorSpec {
                        anchor: [a.anchor[0] + ox, a.anchor[1] + oy],
                        left: [a.left[0] + ox, a.left[1] + oy],
                        right: [a.right[0] + ox, a.right[1] + oy],
                    })
                    .collect();
                if *smooth {
                    conv = paged_mutate::fit_polyline_to_anchors(&conv);
                }
                if conv.is_empty() {
                    return None;
                }
                // Bounding box over anchors + handles (handles cover
                // the curve's extent after smoothing).
                let mut top = f32::MAX;
                let mut left = f32::MAX;
                let mut bottom = f32::MIN;
                let mut right = f32::MIN;
                for a in &conv {
                    for p in [a.anchor, a.left, a.right] {
                        left = left.min(p[0]);
                        right = right.max(p[0]);
                        top = top.min(p[1]);
                        bottom = bottom.max(p[1]);
                    }
                }
                let d = &self.document_defaults;
                Some(Operation::InsertNode {
                    parent: NodeId::Spread(spread_id),
                    position,
                    node: paged_mutate::NodeSpec::Polygon {
                        self_id: self.mint_page_item_id_with_offset(mint_offset),
                        bounds: [top, left, bottom, right],
                        anchors: conv,
                        subpath_starts: vec![0],
                        subpath_open: vec![*open],
                        fill_color: d.fill_color.clone(),
                        // Pencil strokes need a visible stroke too.
                        stroke_color: d
                            .stroke_color
                            .clone()
                            .or_else(|| Some("Color/Black".to_string())),
                        stroke_weight: d.stroke_weight.or(Some(1.0)),
                        // fresh creations carry no transform
                        item_transform: None,
                    },
                    z_slot: None,
                })
            }
            Mutation::InsertPage {
                after_page_id,
                master_id,
            } => Some(Operation::InsertPage {
                after_page_id: after_page_id.as_ref().map(|p| p.0.clone()),
                master_id: master_id.clone(),
                spread_self_id: None,
                page_self_id: None,
                restore_spread_json: None,
            }),
            Mutation::DeletePage { page_id } => Some(Operation::RemovePage {
                page_id: page_id.0.clone(),
            }),
            Mutation::ResizePage { page_id, bounds } => Some(Operation::SetProperty {
                node: NodeId::Page(page_id.0.clone()),
                path: PropertyPath::PageBounds,
                value: Value::Bounds([bounds.0, bounds.1, bounds.2, bounds.3]),
            }),
            Mutation::DeleteFrame { frame_id } => {
                let node = self.resolve_frame_node_id(frame_id)?;
                Some(Operation::RemoveNode { node })
            }
            Mutation::PathfinderBoolean { kept, others, kind } => {
                let kept_node = path_node_id_for(kept)?;
                let other_nodes: Vec<NodeId> = others
                    .iter()
                    .map(path_node_id_for)
                    .collect::<Option<Vec<_>>>()?;
                Some(Operation::PathfinderBoolean {
                    kept: kept_node,
                    others: other_nodes,
                    op_kind: *kind,
                })
            }
            Mutation::PathPointInsert {
                element_id,
                index,
                anchor,
                prev_subpath_starts,
            } => Some(Operation::SetProperty {
                node: path_node_id_for(element_id)?,
                path: PropertyPath::PathPointInsert,
                value: Value::PathPointInsert {
                    index: *index as usize,
                    anchor: *anchor,
                    prev_subpath_starts: prev_subpath_starts
                        .as_ref()
                        .map(|v| v.iter().map(|&n| n as usize).collect()),
                },
            }),
            Mutation::PathPointRemove { element_id, index } => Some(Operation::SetProperty {
                node: path_node_id_for(element_id)?,
                path: PropertyPath::PathPointRemove,
                value: Value::PathPointRemove {
                    index: *index as usize,
                    prev_subpath_starts: None,
                },
            }),
            Mutation::PathOpenAt { element_id, index } => Some(Operation::SetProperty {
                node: path_node_id_for(element_id)?,
                path: PropertyPath::PathOpenAt,
                value: Value::PathOpenAt {
                    index: *index as usize,
                    prev_anchors: None,
                    prev_subpath_starts: None,
                    prev_subpath_open: None,
                },
            }),
            Mutation::OutlineStroke {
                element_id,
                width,
                cap,
                join,
                miter_limit,
            } => Some(Operation::SetProperty {
                node: path_node_id_for(element_id)?,
                path: PropertyPath::OutlineStroke,
                value: Value::OutlineStroke {
                    width: *width,
                    cap: cap.clone(),
                    join: join.clone(),
                    miter_limit: *miter_limit,
                    prev_anchors: None,
                    prev_subpath_starts: None,
                    prev_subpath_open: None,
                },
            }),
            Mutation::OffsetPath {
                element_id,
                delta,
                join,
                miter_limit,
            } => Some(Operation::SetProperty {
                node: path_node_id_for(element_id)?,
                path: PropertyPath::OffsetPath,
                value: Value::OffsetPath {
                    delta: *delta,
                    join: join.clone(),
                    miter_limit: *miter_limit,
                    prev_anchors: None,
                    prev_subpath_starts: None,
                    prev_subpath_open: None,
                },
            }),
            Mutation::SimplifyPath {
                element_id,
                tolerance,
            } => Some(Operation::SetProperty {
                node: path_node_id_for(element_id)?,
                path: PropertyPath::SimplifyPath,
                value: Value::SimplifyPath {
                    tolerance: *tolerance,
                    prev_anchors: None,
                    prev_subpath_starts: None,
                    prev_subpath_open: None,
                },
            }),
            // W1.20 (groups v2) — members may include existing groups
            // (group-of-groups); `element_to_member_node_id` resolves
            // Group ids too. Fresh top-level create carries no parent /
            // own-transform (those are inverse-only).
            Mutation::CreateGroup { member_ids } => Some(Operation::CreateGroup {
                spec: paged_mutate::GroupSpec {
                    self_id: None,
                    members: member_ids
                        .iter()
                        .map(element_to_member_node_id)
                        .collect::<Option<Vec<_>>>()?,
                    parent: None,
                    item_transform: None,
                },
            }),
            Mutation::SetGroupTransform {
                group_id,
                transform,
            } => Some(Operation::SetGroupTransform {
                group: group_id.clone(),
                transform: *transform,
                prev: None,
            }),
            Mutation::DissolveGroup { group_id } => Some(Operation::DissolveGroup {
                group_id: group_id.clone(),
                restore_slots: None,
            }),
            Mutation::SetPluginMetadata {
                element_id,
                key,
                value,
            } => Some(Operation::SetProperty {
                node: element_to_leaf_node_id(element_id)?,
                path: PropertyPath::PluginMetadata,
                value: Value::PluginMetadata {
                    key: key.clone(),
                    value: value.clone(),
                    prev: None,
                },
            }),
            Mutation::PathPointCurveType {
                element_id,
                index,
                smooth,
            } => Some(Operation::SetProperty {
                node: path_node_id_for(element_id)?,
                path: PropertyPath::PathPointCurveType,
                value: Value::PathPointCurveType {
                    index: *index as usize,
                    smooth: *smooth,
                    prev: None,
                },
            }),
            Mutation::PathPointSet {
                element_id,
                index,
                role,
                position,
            } => Some(Operation::SetProperty {
                node: path_node_id_for(element_id)?,
                path: PropertyPath::FramePathPoint,
                value: Value::PathPoint {
                    address: paged_mutate::PathPointAddress {
                        index: *index as usize,
                        role: *role,
                    },
                    position: *position,
                },
            }),
            Mutation::Batch { ops } => {
                // Recursive: every child mutation must translate
                // through this same dispatch. If any child fails
                // (text mutation, unimplemented variant) the whole
                // batch falls through to the non-frame handler so
                // the worker can return a coherent error.
                let mut translated = Vec::with_capacity(ops.len());
                // Protocol v34 — the batch-created sentinel: a child
                // whose element id is `$created` addresses the element
                // minted by the most recent CREATING child of this
                // same (flat) batch. This is what lets a plugin
                // create an object and attach metadata/properties in
                // ONE undo step (apply_batch is atomic + rolls back).
                let mut last_created: Option<crate::element_selection::ElementId> = None;
                for child in ops {
                    let substituted;
                    let child = match (child, last_created.as_ref()) {
                        (
                            Mutation::SetPluginMetadata {
                                element_id,
                                key,
                                value,
                            },
                            Some(created),
                        ) if element_id.raw_id() == "$created" => {
                            substituted = Mutation::SetPluginMetadata {
                                element_id: created.clone(),
                                key: key.clone(),
                                value: value.clone(),
                            };
                            &substituted
                        }
                        (
                            Mutation::SetElementProperty {
                                element_id,
                                path,
                                value,
                            },
                            Some(created),
                        ) if element_id.raw_id() == "$created" => {
                            substituted = Mutation::SetElementProperty {
                                element_id: created.clone(),
                                path: *path,
                                value: value.clone(),
                            };
                            &substituted
                        }
                        _ => child,
                    };
                    // FINDING #6 — thread the SAME mint counter so each
                    // child insert in the batch mints a distinct self_id.
                    let op = self.try_translate_frame_mutation_to_operation(child, mint_offset)?;
                    if let Some(id) = created_element_id(&op) {
                        last_created = Some(id);
                    }
                    translated.push(op);
                }
                Some(Operation::Batch { ops: translated })
            }
            Mutation::LayerSetVisible { layer_id, visible } => Some(Operation::SetProperty {
                node: NodeId::Layer(layer_id.clone()),
                path: PropertyPath::LayerVisible,
                value: Value::Bool(*visible),
            }),
            Mutation::LayerSetLocked { layer_id, locked } => Some(Operation::SetProperty {
                node: NodeId::Layer(layer_id.clone()),
                path: PropertyPath::LayerLocked,
                value: Value::Bool(*locked),
            }),
            Mutation::LayerSetPrintable {
                layer_id,
                printable,
            } => Some(Operation::SetProperty {
                node: NodeId::Layer(layer_id.clone()),
                path: PropertyPath::LayerPrintable,
                value: Value::Bool(*printable),
            }),
            Mutation::SetElementProperty {
                element_id,
                path,
                value,
            } => Some(Operation::SetProperty {
                node: element_to_node_id(element_id),
                path: *path,
                value: value.clone(),
            }),
            Mutation::LayerSetName { layer_id, name } => Some(Operation::SetProperty {
                node: NodeId::Layer(layer_id.clone()),
                path: PropertyPath::LayerName,
                value: Value::Text(name.clone()),
            }),
            Mutation::LayerMove {
                layer_id,
                new_index,
            } => Some(Operation::MoveLayer {
                layer_id: layer_id.clone(),
                new_index: *new_index as usize,
            }),
            Mutation::LayerInsert { position, name } => Some(Operation::InsertLayer {
                position: *position as usize,
                name: name.clone(),
                self_id: None,
            }),
            Mutation::LayerRemove { layer_id } => Some(Operation::RemoveLayer {
                layer_id: layer_id.clone(),
            }),
            // ── Collection mutations — 1:1 with the matching Operation
            //    (no NodeId resolution; they target collections by id).
            Mutation::CreateSwatch { spec } => Some(Operation::CreateSwatch { spec: spec.clone() }),
            // Concept 2 — one undoable Batch: every .ase colour as a
            // CreateSwatch (+ one CreateColorGroup per .ase group).
            // Ids are minted HERE (translate time) so the group
            // member lists are known before apply; the Batch's
            // inverse removes the whole import in one Cmd-Z.
            Mutation::ImportSwatchLibrary { bytes, group_name } => {
                let lib = match paged_color::ase::parse_ase(bytes.as_slice()) {
                    Ok(lib) => lib,
                    Err(e) => {
                        tracing::warn!(error = %e, "ImportSwatchLibrary: bad .ase payload");
                        return None;
                    }
                };
                let mut ops: Vec<Operation> = Vec::new();
                let mut next_n = self.scene.palette.colors.len();
                let mut mint = |taken: &mut std::collections::HashSet<String>| -> String {
                    let mut id = format!("Color/u{next_n}");
                    while self.scene.palette.colors.contains_key(&id) || taken.contains(&id) {
                        next_n += 1;
                        id = format!("Color/u{next_n}");
                    }
                    taken.insert(id.clone());
                    id
                };
                let mut taken: std::collections::HashSet<String> = Default::default();
                let spec_of = |entry: &paged_color::ase::AseEntry, id: String| {
                    use paged_color::ase::{AseKind, AseSpace};
                    paged_mutate::operation::SwatchSpec {
                        self_id: Some(id),
                        name: Some(entry.name.clone()),
                        space: match entry.space {
                            AseSpace::Rgb => "RGB".into(),
                            AseSpace::Cmyk => "CMYK".into(),
                            AseSpace::Lab => "LAB".into(),
                            AseSpace::Gray => "Gray".into(),
                        },
                        value: entry.value.clone(),
                        model: Some(match entry.kind {
                            AseKind::Spot => "Spot".into(),
                            _ => "Process".into(),
                        }),
                        alternate_space: None,
                        alternate_value: Vec::new(),
                        tint: None,
                        alpha: None,
                    }
                };
                let mut groups: Vec<(String, Vec<String>)> = Vec::new();
                for g in &lib.groups {
                    let mut members = Vec::new();
                    for entry in &g.entries {
                        let id = mint(&mut taken);
                        members.push(id.clone());
                        ops.push(Operation::CreateSwatch {
                            spec: spec_of(entry, id),
                        });
                    }
                    groups.push((g.name.clone(), members));
                }
                let mut loose_members = Vec::new();
                for entry in &lib.loose {
                    let id = mint(&mut taken);
                    loose_members.push(id.clone());
                    ops.push(Operation::CreateSwatch {
                        spec: spec_of(entry, id),
                    });
                }
                if let (Some(name), false) = (group_name.as_ref(), loose_members.is_empty()) {
                    groups.push((name.clone(), loose_members));
                }
                for (name, members) in groups {
                    ops.push(Operation::CreateColorGroup {
                        spec: paged_mutate::operation::ColorGroupSpec {
                            self_id: None,
                            name: Some(name),
                            members,
                        },
                    });
                }
                if ops.is_empty() {
                    return None;
                }
                Some(Operation::Batch { ops })
            }
            Mutation::EditSwatch { swatch_id, spec } => Some(Operation::EditSwatch {
                swatch_id: swatch_id.clone(),
                spec: spec.clone(),
            }),
            Mutation::DeleteSwatch { swatch_id } => Some(Operation::DeleteSwatch {
                swatch_id: swatch_id.clone(),
            }),
            Mutation::CreateGradient { spec } => {
                Some(Operation::CreateGradient { spec: spec.clone() })
            }
            Mutation::EditGradient { gradient_id, spec } => Some(Operation::EditGradient {
                gradient_id: gradient_id.clone(),
                spec: spec.clone(),
            }),
            Mutation::DeleteGradient { gradient_id } => Some(Operation::DeleteGradient {
                gradient_id: gradient_id.clone(),
            }),
            Mutation::CreateColorGroup { spec } => {
                Some(Operation::CreateColorGroup { spec: spec.clone() })
            }
            Mutation::EditColorGroup { group_id, spec } => Some(Operation::EditColorGroup {
                group_id: group_id.clone(),
                spec: spec.clone(),
            }),
            Mutation::DeleteColorGroup { group_id } => Some(Operation::DeleteColorGroup {
                group_id: group_id.clone(),
            }),
            // W1.22 (engine gap 22) — numbering-list CRUD, 1:1 with the
            // matching Operation (collection-by-id, no NodeId resolve).
            Mutation::CreateNumberingList { spec } => {
                Some(Operation::CreateNumberingList { spec: spec.clone() })
            }
            Mutation::EditNumberingList { list_id, spec } => Some(Operation::EditNumberingList {
                list_id: list_id.clone(),
                spec: spec.clone(),
            }),
            Mutation::DeleteNumberingList { list_id } => Some(Operation::DeleteNumberingList {
                list_id: list_id.clone(),
            }),
            Mutation::CreateParagraphStyle {
                self_id,
                name,
                based_on,
            } => Some(Operation::CreateParagraphStyle {
                self_id: self_id.clone(),
                name: name.clone(),
                based_on: based_on.clone(),
                restore_json: None,
            }),
            Mutation::RenameParagraphStyle { style_id, name } => {
                Some(Operation::RenameParagraphStyle {
                    style_id: style_id.clone(),
                    name: name.clone(),
                })
            }
            Mutation::DeleteParagraphStyle { style_id } => Some(Operation::DeleteParagraphStyle {
                style_id: style_id.clone(),
            }),
            Mutation::CreateCharacterStyle {
                self_id,
                name,
                based_on,
            } => Some(Operation::CreateCharacterStyle {
                self_id: self_id.clone(),
                name: name.clone(),
                based_on: based_on.clone(),
                restore_json: None,
            }),
            Mutation::RenameCharacterStyle { style_id, name } => {
                Some(Operation::RenameCharacterStyle {
                    style_id: style_id.clone(),
                    name: name.clone(),
                })
            }
            Mutation::DeleteCharacterStyle { style_id } => Some(Operation::DeleteCharacterStyle {
                style_id: style_id.clone(),
            }),
            Mutation::CreateObjectStyle {
                self_id,
                name,
                based_on,
            } => Some(Operation::CreateObjectStyle {
                self_id: self_id.clone(),
                name: name.clone(),
                based_on: based_on.clone(),
                restore_json: None,
            }),
            Mutation::RenameObjectStyle { style_id, name } => Some(Operation::RenameObjectStyle {
                style_id: style_id.clone(),
                name: name.clone(),
            }),
            Mutation::DeleteObjectStyle { style_id } => Some(Operation::DeleteObjectStyle {
                style_id: style_id.clone(),
            }),
            Mutation::CreateCellStyle {
                self_id,
                name,
                based_on,
            } => Some(Operation::CreateCellStyle {
                self_id: self_id.clone(),
                name: name.clone(),
                based_on: based_on.clone(),
                restore_json: None,
            }),
            Mutation::RenameCellStyle { style_id, name } => Some(Operation::RenameCellStyle {
                style_id: style_id.clone(),
                name: name.clone(),
            }),
            Mutation::DeleteCellStyle { style_id } => Some(Operation::DeleteCellStyle {
                style_id: style_id.clone(),
            }),
            Mutation::CreateTableStyle {
                self_id,
                name,
                based_on,
            } => Some(Operation::CreateTableStyle {
                self_id: self_id.clone(),
                name: name.clone(),
                based_on: based_on.clone(),
                restore_json: None,
            }),
            Mutation::RenameTableStyle { style_id, name } => Some(Operation::RenameTableStyle {
                style_id: style_id.clone(),
                name: name.clone(),
            }),
            Mutation::DeleteTableStyle { style_id } => Some(Operation::DeleteTableStyle {
                style_id: style_id.clone(),
            }),
            Mutation::SetStyleProperty {
                collection,
                style_id,
                path,
                value,
            } => Some(Operation::SetStyleProperty {
                collection: *collection,
                style_id: style_id.clone(),
                path: *path,
                value: value.clone(),
            }),
            // ── W0.5 wire-expansion ─────────────────────────────────
            Mutation::InsertOval { page_id, bounds } => {
                let (spread_id, (ox, oy), idx) = self.page_insert_context(page_id)?;
                let position = self.scene.spreads[idx].spread.ovals.len();
                let d = &self.document_defaults;
                Some(Operation::InsertNode {
                    parent: NodeId::Spread(spread_id),
                    position,
                    node: paged_mutate::NodeSpec::Oval {
                        self_id: self.mint_page_item_id_with_offset(mint_offset),
                        // Page-local (top, left, bottom, right) → spread
                        // coords (same rule as InsertFrame).
                        bounds: [bounds.0 + oy, bounds.1 + ox, bounds.2 + oy, bounds.3 + ox],
                        fill_color: d.fill_color.clone(),
                        stroke_color: d.stroke_color.clone(),
                        stroke_weight: d.stroke_weight,
                        item_transform: None,
                    },
                    z_slot: None,
                })
            }
            Mutation::LinkFrames { from, to } => Some(Operation::LinkFrames {
                from: from.clone(),
                to: to.clone(),
            }),
            Mutation::UnlinkFrames { frame } => Some(Operation::UnlinkFrames {
                frame: frame.clone(),
                prev_next: None,
            }),
            Mutation::ApplyStyle {
                story_id,
                start,
                end,
                style,
                scope,
            } => Some(Operation::ApplyStyle {
                story_id: story_id.clone(),
                start: *start,
                end: *end,
                style: style.clone(),
                scope: *scope,
            }),
            Mutation::InsertField {
                story_id,
                offset,
                field,
            } => Some(Operation::InsertField {
                story_id: story_id.clone(),
                offset: *offset,
                field: *field,
            }),
            Mutation::InsertGuide {
                spread_id,
                orientation,
                position,
                page_index,
            } => Some(Operation::InsertGuide {
                spread_id: spread_id.clone(),
                orientation: *orientation,
                position: *position,
                page_index: *page_index,
                guide_id: None,
            }),
            Mutation::MoveGuide { guide_id, position } => Some(Operation::MoveGuide {
                guide_id: guide_id.clone(),
                position: *position,
            }),
            Mutation::DeleteGuide { guide_id } => Some(Operation::DeleteGuide {
                guide_id: guide_id.clone(),
            }),
            Mutation::SetConditionVisible { condition, visible } => {
                Some(Operation::SetConditionVisible {
                    condition: condition.clone(),
                    visible: *visible,
                })
            }
            Mutation::ActivateConditionSet { set } => {
                Some(Operation::ActivateConditionSet { set: set.clone() })
            }
            Mutation::ApplyMasterToPage { page, master } => Some(Operation::ApplyMasterToPage {
                page: page.0.clone(),
                master: master.clone(),
            }),
            Mutation::DuplicatePage { page } => Some(Operation::DuplicatePage {
                page: page.0.clone(),
                clone_spread_json: None,
            }),
            Mutation::InsertSection {
                at_page,
                prefix,
                numbering_style,
                start_at,
            } => Some(Operation::InsertSection {
                at_page: at_page.0.clone(),
                prefix: prefix.clone(),
                numbering_style: numbering_style.clone(),
                start_at: *start_at,
                self_id: None,
            }),
            Mutation::EditSection {
                section_id,
                prefix,
                numbering_style,
                start_at,
            } => Some(Operation::EditSection {
                section_id: section_id.clone(),
                prefix: prefix.clone(),
                numbering_style: numbering_style.clone(),
                start_at: *start_at,
            }),
            Mutation::DeleteSection { section_id } => Some(Operation::DeleteSection {
                section_id: section_id.clone(),
            }),
            // ── W3.A1 table structure — 1:1 with the Operation. ──────
            Mutation::SetRowHeight {
                story_id,
                table_id,
                row,
                height,
            } => Some(Operation::SetRowHeight {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
                row: *row,
                height: *height,
            }),
            Mutation::SetColumnWidth {
                story_id,
                table_id,
                col,
                width,
            } => Some(Operation::SetColumnWidth {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
                col: *col,
                width: *width,
            }),
            Mutation::InsertTableRow {
                story_id,
                table_id,
                at,
            } => Some(Operation::InsertTableRow {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
                at: *at,
                restore: None,
            }),
            Mutation::DeleteTableRow {
                story_id,
                table_id,
                at,
            } => Some(Operation::DeleteTableRow {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
                at: *at,
            }),
            Mutation::InsertTableColumn {
                story_id,
                table_id,
                at,
            } => Some(Operation::InsertTableColumn {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
                at: *at,
                restore: None,
            }),
            Mutation::DeleteTableColumn {
                story_id,
                table_id,
                at,
            } => Some(Operation::DeleteTableColumn {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
                at: *at,
            }),
            // W1.12a — header / footer row inserts. The `restore` blob is
            // engine-internal (the Remove* inverse fills it); wire callers
            // always insert empties, so `restore: None`.
            Mutation::InsertHeaderRow { story_id, table_id } => Some(Operation::InsertHeaderRow {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
                restore: None,
            }),
            Mutation::RemoveHeaderRow { story_id, table_id } => Some(Operation::RemoveHeaderRow {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
            }),
            Mutation::InsertFooterRow { story_id, table_id } => Some(Operation::InsertFooterRow {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
                restore: None,
            }),
            Mutation::RemoveFooterRow { story_id, table_id } => Some(Operation::RemoveFooterRow {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
            }),
            // W1.12b — merge / split spans.
            Mutation::SetCellSpan {
                story_id,
                table_id,
                row,
                col,
                row_span,
                column_span,
            } => Some(Operation::SetCellSpan {
                story_id: story_id.clone(),
                table_id: table_id.clone(),
                row: *row,
                col: *col,
                row_span: *row_span,
                column_span: *column_span,
            }),
            _ => None,
        }
    }

    /// Editor-ops — mint a fresh, document-wide-unique page-item
    /// `Self` id (`u<hex>`, the bare style real IDML page items use).
    /// Scans every page-item id across all spreads for the highest
    /// `u<hex>` suffix and returns the successor — collision-safe by
    /// construction.
    ///
    /// FINDING #6 — batch-safe id minting. The scene is only mutated at
    /// `apply` time, so every translate-time `mint_page_item_id()` within
    /// one `Operation::Batch` saw the SAME unmutated max and minted the
    /// SAME `u<max+1>` for all N inserts — `paged_mutate::apply` then
    /// rejected the 2nd insert with "duplicate self_id". `offset` is the
    /// number of ids already minted earlier in the same translation pass;
    /// the caller bumps it by one after each mint so N successive calls
    /// yield `u<max+1>, u<max+2>, …`. Standalone inserts pass `&mut 0`.
    pub(crate) fn mint_page_item_id_with_offset(&self, offset: &mut u64) -> String {
        let mut max: u64 = 0;
        for parsed in &self.scene.spreads {
            let s = &parsed.spread;
            for f in &s.text_frames {
                scan_page_item_id(&mut max, f.self_id.as_deref());
            }
            for r in &s.rectangles {
                scan_page_item_id(&mut max, r.self_id.as_deref());
            }
            for o in &s.ovals {
                scan_page_item_id(&mut max, o.self_id.as_deref());
            }
            for l in &s.graphic_lines {
                scan_page_item_id(&mut max, l.self_id.as_deref());
            }
            for p in &s.polygons {
                scan_page_item_id(&mut max, p.self_id.as_deref());
            }
            for g in &s.groups {
                scan_page_item_id(&mut max, g.self_id.as_deref());
            }
        }
        let id = format!("u{:x}", max + 1 + *offset);
        *offset += 1;
        id
    }

    /// Editor-ops — resolve the spread hosting `page_id` plus the
    /// page's spread-origin (for the page-local → spread-coordinate
    /// conversion the structural inserts need; same rule as
    /// `marquee_hits`). Returns `(spread self_id, origin, spread idx)`.
    pub(crate) fn page_insert_context(
        &self,
        page_id: &PageId,
    ) -> Option<(String, (f32, f32), usize)> {
        let origin = self.page(page_id)?.spread_origin;
        let (idx, parsed) = self.scene.spreads.iter().enumerate().find(|(_, parsed)| {
            parsed
                .spread
                .pages
                .iter()
                .any(|p| p.self_id.as_deref() == Some(page_id.as_str()) || p.self_id.is_none())
        })?;
        // Well-formed IDMLs always carry a spread self_id; synthetic
        // docs fall back to the manifest src (mirrors spread_parent_id
        // on the apply side).
        let spread_id = parsed
            .spread
            .self_id
            .clone()
            .unwrap_or_else(|| parsed.src.clone());
        Some((spread_id, origin, idx))
    }

    pub(crate) fn resolve_frame_node_id(&self, frame_id: &str) -> Option<paged_mutate::NodeId> {
        for parsed in &self.scene.spreads {
            let s = &parsed.spread;
            if s.text_frames
                .iter()
                .any(|f| f.self_id.as_deref() == Some(frame_id))
            {
                return Some(paged_mutate::NodeId::TextFrame(frame_id.to_string()));
            }
            if s.rectangles
                .iter()
                .any(|f| f.self_id.as_deref() == Some(frame_id))
            {
                return Some(paged_mutate::NodeId::Rectangle(frame_id.to_string()));
            }
            // Editor-ops — the insert ops create lines/polygons, so
            // ResizeFrame/DeleteFrame must resolve them too.
            if s.graphic_lines
                .iter()
                .any(|l| l.self_id.as_deref() == Some(frame_id))
            {
                return Some(paged_mutate::NodeId::GraphicLine(frame_id.to_string()));
            }
            if s.polygons
                .iter()
                .any(|p| p.self_id.as_deref() == Some(frame_id))
            {
                return Some(paged_mutate::NodeId::Polygon(frame_id.to_string()));
            }
            if s.ovals
                .iter()
                .any(|o| o.self_id.as_deref() == Some(frame_id))
            {
                return Some(paged_mutate::NodeId::Oval(frame_id.to_string()));
            }
        }
        None
    }

    /// Phase B — apply a canonical `paged_mutate::Operation` (frame
    /// mutation, fill, etc.), rebuild, push to the unified undo log.
    /// The bridge from `Mutation::MoveFrame` / `ResizeFrame` (channel
    /// envelope) lands here.
    ///
    /// Returns the dirty page set + the underlying `AppliedOperation`
    /// so the caller can also feed the LOD-cache invalidation hint.
    pub fn apply_operation(
        &mut self,
        op: paged_mutate::Operation,
    ) -> Result<FrameMutationOutcome, crate::channel::WorkerError> {
        // W1.24 (audit B18) — time the scene edit; the rebuild folds it
        // into RebuildStats.op_apply_ms.
        let t_op = phase_now();
        let applied = paged_mutate::apply(&mut self.scene, &op).map_err(|e| {
            crate::channel::WorkerError::NotImplemented {
                what: format!("frame mutation failed: {e}"),
            }
        })?;
        self.stage_op_apply_ms(phase_elapsed_ms(t_op));
        // Perf-MasterText + Perf-BodyStory — committed mutations
        // can shift the per-page pool state (Alt-duplicate inserts
        // a new frame whose path the frame pass emits earlier,
        // path-topology mutations change anchor counts, etc.).
        // The cached relative-path-id rebase only stays valid
        // when those earlier passes produce the same pool state.
        // Clear both caches; the post-rebuild fresh capture
        // re-pins. Gesture-driven update_gesture mutates the
        // scene directly without going through apply_operation,
        // so the cache survives the whole drag.
        self.master_text_emit_cache.borrow_mut().clear();
        self.body_story_emit_cache.borrow_mut().clear();
        self.rebuild_after_mutation()
            .map_err(|e| crate::channel::WorkerError::NotImplemented {
                what: format!("rebuild after frame mutation: {e}"),
            })?;
        let applied_seq = self.bump_applied_seq();
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        // W1.24 (B19) — capped push (oldest-evicted).
        self.push_applied(AppliedRecord {
            applied_seq,
            kind: LoggedMutation::Frame(applied.clone()),
        });
        self.redo_log.clear();
        Ok(FrameMutationOutcome {
            applied_seq,
            page_ids,
            applied,
        })
    }

    /// Undo the most recent applied mutation. Phase 3 Item 7 —
    /// applies the cached inverse + rebuilds + pushes onto the redo
    /// stack. Phase B — handles both text and frame variants of the
    /// unified log.
    pub fn undo(&mut self) -> Option<UndoOutcome> {
        let rec = self.applied_log.pop()?;
        let affected_story_id = match &rec.kind {
            LoggedMutation::Text { op: _, inverse } => {
                let _ = crate::mutate::apply(&mut self.scene, inverse).ok()?;
                Some(story_id_of_text_op(inverse).to_string())
            }
            LoggedMutation::Frame(applied) => {
                let _ = paged_mutate::apply(&mut self.scene, &applied.inverse).ok()?;
                None
            }
        };
        // Perf-MasterText + Perf-BodyStory — undo replays an inverse
        // through the same scene paths as the forward commit, so the
        // same invariant applies: a content-only text inverse keeps the
        // body-story signature matching (the stale pre-undo emit would
        // splice back in), and a structural inverse (page remove, frame
        // re-insert) shifts page indices under the cached per-page
        // deltas. Mirror apply_mutation / apply_operation and blow both
        // caches before the rebuild.
        self.master_text_emit_cache.borrow_mut().clear();
        self.body_story_emit_cache.borrow_mut().clear();
        self.rebuild_after_mutation().ok()?;
        let undone_seq = rec.applied_seq;
        let applied_seq = self.bump_applied_seq();
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        self.redo_log.push(rec);
        Some(UndoOutcome {
            undone_seq,
            applied_seq,
            page_ids,
            affected_story_id,
        })
    }

    /// Redo the most-recently-undone mutation. Phase 3 Item 7. Phase B
    /// — handles both text and frame variants.
    pub fn redo(&mut self) -> Option<UndoOutcome> {
        let rec = self.redo_log.pop()?;
        let (new_kind, affected_story_id) = match &rec.kind {
            LoggedMutation::Text { op, inverse: _ } => {
                let applied = crate::mutate::apply(&mut self.scene, op).ok()?;
                let sid = Some(story_id_of_text_op(op).to_string());
                (
                    LoggedMutation::Text {
                        op: op.clone(),
                        inverse: applied.inverse,
                    },
                    sid,
                )
            }
            LoggedMutation::Frame(prev_applied) => {
                let applied = paged_mutate::apply(&mut self.scene, &prev_applied.op).ok()?;
                (LoggedMutation::Frame(applied), None)
            }
        };
        // Perf-MasterText + Perf-BodyStory — same invariant as undo():
        // the replayed op mutates content/structure under the caches.
        self.master_text_emit_cache.borrow_mut().clear();
        self.body_story_emit_cache.borrow_mut().clear();
        self.rebuild_after_mutation().ok()?;
        let redone_seq = rec.applied_seq;
        let applied_seq = self.bump_applied_seq();
        let page_ids: Vec<PageId> = self.built.pages.iter().map(|p| p.id.clone()).collect();
        // W1.24 (B19) — capped push (oldest-evicted). A redo can only
        // re-grow the log up to the cap; the front-eviction here is the
        // same as the forward paths.
        self.push_applied(AppliedRecord {
            applied_seq: redone_seq,
            kind: new_kind,
        });
        Some(UndoOutcome {
            undone_seq: redone_seq,
            applied_seq,
            page_ids,
            affected_story_id,
        })
    }

    /// Number of mutations in the undo log (read-only inspection
    /// for debug UI + tests).
    pub fn applied_log_len(&self) -> usize {
        self.applied_log.len()
    }
    pub fn redo_log_len(&self) -> usize {
        self.redo_log.len()
    }

    /// Read-only inspection of the most recently logged mutation.
    /// Used by Phase B integration tests to confirm the bridge routed
    /// a frame mutation onto the canonical `LoggedMutation::Frame`
    /// variant rather than the legacy text path.
    pub fn applied_log_back(&self) -> Option<&AppliedRecord> {
        self.applied_log.last()
    }

    /// Expose the inner scene for read-only inspection. Used by the
    /// inspector devtools wasm + tests. Mutating consumers should
    /// route through `apply_mutation`.
    pub fn scene(&self) -> &Document {
        &self.scene
    }

    /// Mutable accessor for the parsed scene. Phase 3 — used by the
    /// `mutate` module to apply text edits in place. Callers must
    /// follow up with `rebuild_after_mutation` so the `BuiltDocument`
    /// and page index stay in sync; bypassing the rebuild leaves the
    /// canvas painting stale pixels.
    pub fn scene_mut(&mut self) -> &mut Document {
        &mut self.scene
    }

    /// W3.B2 — re-serialize the (possibly-mutated) scene back into an
    /// IDML package for save-back. Hands the retained source bytes to
    /// the carry-through writer (`paged_write::write_idml`), which
    /// patches only the model-owned Spreads/Stories and copies every
    /// other entry verbatim — so an unmutated document round-trips
    /// byte-identically and a mutated one differs only in the entries
    /// the edit touched. The model holds a `paged_scene::Document`,
    /// which is exactly the `&Document` the writer takes.
    pub fn export_idml(&self) -> Result<Vec<u8>, paged_write::WriteError> {
        paged_write::write_idml(&self.scene, &self.source_idml)
    }

    /// Phase 4 Step 3 — return the pages whose frame chains touch
    /// `story_id`. Used by the wasm dispatch to scope GPU scene-cache
    /// invalidation after a mutation: instead of clearing every page's
    /// cached Vello scene, only invalidate the affected ones. Returns
    /// an empty slice when the story id is unknown (e.g. the mutation
    /// failed validation, or the story has no on-page frames).
    pub fn pages_for_story(&self, story_id: &str) -> &[PageId] {
        self.story_pages
            .get(story_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Phase A — oriented geometry for the requested element ids.
    /// Skips ids that don't resolve (unknown / removed / not on a body
    /// page). Used by the overlay layer to draw element-selection
    /// chrome without re-deriving the affine math in TS.
    /// Phase H — return every leaf `ElementId` (no groups) reachable
    /// from the named group, descending through nested groups. Used
    /// by the canvas's double-click-to-enter-group gesture so the
    /// user can select a whole group as a unit and translate / scale
    /// it via the existing union handles.
    /// Track M — flatten the loaded document's designmap layers into
    /// the wire-shape `LayerSummary` list. Top-first order matches
    /// the renderer's `layer_z_index` (designmap[0] = topmost in the
    /// IDML wire convention).
    pub fn layers(&self) -> Vec<crate::channel::LayerSummary> {
        use crate::channel::LayerSummary;
        self.scene
            .container
            .designmap
            .layers
            .iter()
            .enumerate()
            .map(|(z, l)| LayerSummary {
                self_id: l.self_id.clone(),
                name: l.name.clone(),
                visible: l.visible,
                locked: l.locked,
                printable: l.printable,
                z: z as u32,
            })
            .collect()
    }

    /// Inspector P1 — typed property snapshot for the named element.
    /// Returns `None` when the id doesn't resolve. Frame-level
    /// properties: bounds, item_transform, fill, stroke, stroke
    /// weight, opacity. SDK Phase 3 — character properties when
    /// `id` is `ElementId::StoryRange`: walks the story's
    /// `CharacterRun`s within `[start, end)`, collapses uniform
    /// values, emits `None` for "mixed" so the binding renderer
    /// can show an em-dash placeholder.
    /// W1.16 — read entries for an anchored frame's
    /// `AnchoredObjectSetting`. Anchored frames live on a story's
    /// `CharacterRun.anchored_frames` (recursing into anchored Group
    /// children), addressed by their own `Self` id. Returns `None` when
    /// no anchored frame carries the requested id (so `element_properties`
    /// can fall through to the spread page-item lookup). The ten entries
    /// mirror the W1.16 mutation surface; `None`-valued Option fields read
    /// back as the clear sentinel (empty `Text`), matching the apply arm.
    fn anchored_frame_properties(
        &self,
        id: &crate::element_selection::ElementId,
    ) -> Option<crate::channel::ElementProperties> {
        use crate::channel::{ElementProperties, PropertyEntry};
        use paged_mutate::{PropertyPath, Value};
        use paged_parse::{AnchoredFrame, AnchoredObjectSetting};

        let raw = id.raw_id();

        fn find<'a>(frames: &'a [AnchoredFrame], raw: &str) -> Option<&'a AnchoredFrame> {
            for frame in frames {
                if frame.self_id.as_deref() == Some(raw) {
                    return Some(frame);
                }
                if let Some(found) = find(&frame.children, raw) {
                    return Some(found);
                }
            }
            None
        }

        let frame = self.scene.stories.iter().find_map(|parsed| {
            parsed
                .story
                .paragraphs
                .iter()
                .find_map(|p| find(&p.anchored_frames, raw))
        })?;

        // Read against the materialised-or-default setting so the
        // inspector always shows every row (an anchored frame with no
        // `<AnchoredObjectSetting>` reads the IDML defaults).
        let default = AnchoredObjectSetting::default();
        let s = frame.setting.as_ref().unwrap_or(&default);
        let text = |v: &Option<String>| Value::Text(v.clone().unwrap_or_default());
        let entries = vec![
            PropertyEntry {
                path: PropertyPath::AnchoredPosition,
                value: Some(text(&s.anchored_position)),
            },
            PropertyEntry {
                path: PropertyPath::AnchorPoint,
                value: Some(text(&s.anchor_point)),
            },
            PropertyEntry {
                path: PropertyPath::AnchoredXOffset,
                value: Some(Value::Length(Some(s.anchor_x_offset))),
            },
            PropertyEntry {
                path: PropertyPath::AnchoredYOffset,
                value: Some(Value::Length(Some(s.anchor_y_offset))),
            },
            PropertyEntry {
                path: PropertyPath::AnchoredHorizontalReference,
                value: Some(text(&s.horizontal_reference_point)),
            },
            PropertyEntry {
                path: PropertyPath::AnchoredVerticalReference,
                value: Some(text(&s.vertical_reference_point)),
            },
            PropertyEntry {
                path: PropertyPath::AnchoredHorizontalAlignment,
                value: Some(text(&s.horizontal_alignment)),
            },
            PropertyEntry {
                path: PropertyPath::AnchoredVerticalAlignment,
                value: Some(text(&s.vertical_alignment)),
            },
            PropertyEntry {
                path: PropertyPath::AnchoredSpineRelative,
                value: Some(Value::Bool(s.spine_relative)),
            },
            PropertyEntry {
                path: PropertyPath::AnchoredLockPosition,
                value: Some(Value::Bool(s.lock_position)),
            },
        ];
        Some(ElementProperties {
            id: id.clone(),
            kind: id.kind_label().to_string(),
            name: frame.self_id.clone(),
            entries,
        })
    }

    pub fn element_properties(
        &self,
        id: &crate::element_selection::ElementId,
    ) -> Option<crate::channel::ElementProperties> {
        use crate::channel::{ElementProperties, PropertyEntry};
        use crate::element_selection::ElementId;
        use paged_mutate::{PropertyPath, Value};

        // SDK Phase 3 — StoryRange snapshot. Story lives in
        // `self.scene.stories`, not in spreads, so this branch
        // returns early before the spread loop below.
        if let ElementId::StoryRange {
            story_id,
            start,
            end,
        } = id
        {
            return self.story_range_properties(story_id, *start, *end, id);
        }
        // W3.A1 — tables / cells likewise live in stories, not spreads.
        if let ElementId::Table { story_id, table_id } = id {
            return self.table_properties(story_id, table_id, id);
        }
        if let ElementId::TableCell {
            story_id,
            table_id,
            row,
            col,
        } = id
        {
            return self.cell_properties(story_id, table_id, *row, *col, id);
        }

        // W1.16 — an anchored frame is addressed by its own page-item
        // id, but it lives in a story's run (not the spread page-item
        // vecs), so resolve it here before the spread loop. The
        // `AnchoredObjectSetting` read entries surface its placement.
        if matches!(
            id,
            ElementId::TextFrame(_) | ElementId::Rectangle(_) | ElementId::Group(_)
        ) {
            if let Some(props) = self.anchored_frame_properties(id) {
                return Some(props);
            }
            // Fall through: a non-anchored TextFrame / Rectangle / Group
            // resolves against the spreads below as usual.
        }

        let raw = id.raw_id();
        for parsed in &self.scene.spreads {
            let spread = &parsed.spread;
            let entries: Option<Vec<PropertyEntry>> = match id {
                ElementId::TextFrame(_) => spread
                    .text_frames
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(raw))
                    .map(|f| {
                        let mut entries = vec![
                            PropertyEntry {
                                path: PropertyPath::FrameBounds,
                                value: Some(Value::Bounds([
                                    f.bounds.top,
                                    f.bounds.left,
                                    f.bounds.bottom,
                                    f.bounds.right,
                                ])),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameTransform,
                                value: Some(Value::Transform(f.item_transform)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameFillColor,
                                value: Some(Value::ColorRef(f.fill_color.clone())),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeColor,
                                value: Some(Value::ColorRef(f.stroke_color.clone())),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeWeight,
                                value: Some(Value::Length(f.stroke_weight)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameOpacity,
                                value: Some(Value::Length(f.opacity)),
                            },
                            PropertyEntry {
                                path: PropertyPath::AppliedObjectStyle,
                                value: Some(Value::Text(
                                    f.applied_object_style.clone().unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameTextWrapMode,
                                value: Some(Value::Text(
                                    f.text_wrap
                                        .map(|t| t.mode.as_idml().to_string())
                                        .unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameTextWrapOffsets,
                                value: Some(Value::Bounds(
                                    f.text_wrap
                                        .map(|t| t.offsets)
                                        .unwrap_or([0.0; 4]),
                                )),
                            },
                            // W2.5 — text-wrap contour options.
                            PropertyEntry {
                                path: PropertyPath::FrameTextWrapContourType,
                                value: Some(Value::Text(
                                    f.text_wrap
                                        .and_then(|t| t.contour_type)
                                        .map(|c| c.as_idml().to_string())
                                        .unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameTextWrapContourIncludeInside,
                                value: Some(Value::Bool(
                                    f.text_wrap
                                        .and_then(|t| t.include_inside_edges)
                                        .unwrap_or(false),
                                )),
                            },
                            // W2.5 — element-level visibility / lock.
                            PropertyEntry {
                                path: PropertyPath::ElementVisible,
                                value: Some(Value::Bool(f.visible)),
                            },
                            PropertyEntry {
                                path: PropertyPath::ElementLocked,
                                value: Some(Value::Bool(f.locked)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadow,
                                value: Some(Value::Bool(
                                    f.drop_shadow.is_some(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameGradientFillAngle,
                                value: Some(Value::Length(f.gradient_fill_angle)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameGradientFillLength,
                                value: Some(Value::Length(f.gradient_fill_length)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameGradientStrokeAngle,
                                value: Some(Value::Length(f.gradient_stroke_angle)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameGradientStrokeLength,
                                value: Some(Value::Length(f.gradient_stroke_length)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameGradientFeather,
                                value: Some(Value::GradientFeather(
                                    f.effects
                                        .as_ref()
                                        .and_then(|e| e.gradient_feather.as_ref())
                                        .map(
                                            paged_mutate::operation::GradientFeatherSpec::from_parse,
                                        ),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowMode,
                                value: Some(Value::Text(
                                    f.drop_shadow
                                        .as_ref()
                                        .map(|s| s.mode.clone())
                                        .unwrap_or_else(|| "Drop".to_string()),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowXOffset,
                                value: Some(Value::Length(
                                    f.drop_shadow.as_ref().map(|s| s.x_offset),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowYOffset,
                                value: Some(Value::Length(
                                    f.drop_shadow.as_ref().map(|s| s.y_offset),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowSize,
                                value: Some(Value::Length(
                                    f.drop_shadow.as_ref().map(|s| s.size),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowOpacity,
                                value: Some(Value::Length(
                                    f.drop_shadow.as_ref().map(|s| s.opacity_pct),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowColor,
                                value: Some(Value::ColorRef(
                                    f.drop_shadow
                                        .as_ref()
                                        .and_then(|s| s.effect_color.clone()),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameFillTint,
                                value: Some(Value::Length(f.fill_tint)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameNonprinting,
                                value: Some(Value::Bool(f.nonprinting)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameInsetSpacing,
                                value: Some(Value::Bounds(
                                    f.inset_spacing.unwrap_or([0.0; 4]),
                                )),
                            },
                            // W0.3 — text-frame prefs.
                            PropertyEntry {
                                path: PropertyPath::TextFrameColumnCount,
                                value: Some(Value::Length(
                                    f.column_count.map(|c| c as f32),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::TextFrameColumnGutter,
                                value: Some(Value::Length(f.column_gutter)),
                            },
                            PropertyEntry {
                                path: PropertyPath::TextFrameColumnBalance,
                                value: Some(Value::Bool(
                                    f.column_balance.unwrap_or(false),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::TextFrameVerticalJustification,
                                value: Some(Value::Text(
                                    f.vertical_justification
                                        .map(vertical_justification_idml)
                                        .unwrap_or_default()
                                        .to_string(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::TextFrameAutoSizing,
                                value: Some(Value::Text(
                                    f.auto_sizing
                                        .map(auto_sizing_idml)
                                        .unwrap_or_default()
                                        .to_string(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::TextFrameFirstBaseline,
                                value: Some(Value::Text(
                                    f.first_baseline_offset
                                        .map(first_baseline_idml)
                                        .unwrap_or_default()
                                        .to_string(),
                                )),
                            },
                            // W0.3 — stroke type / gap, wrap invert,
                            // overprint, transform-decompose (shared).
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeType,
                                value: Some(Value::Text(
                                    f.stroke_type.clone().unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeGapColor,
                                value: Some(Value::ColorRef(
                                    f.stroke_gap_color.clone(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeGapTint,
                                value: Some(Value::Length(f.stroke_gap_tint)),
                            },
                            // W1.1 — per-frame dash override (empty vec =
                            // no override; stroke uses its StrokeType).
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeDashArray,
                                value: Some(Value::Lengths(f.stroke_dash.clone())),
                            },
                            PropertyEntry {
                                path: PropertyPath::TextWrapInvert,
                                value: Some(Value::Bool(
                                    f.text_wrap.and_then(|t| t.invert).unwrap_or(false),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameOverprintFill,
                                value: Some(Value::Bool(f.overprint_fill)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameOverprintStroke,
                                value: Some(Value::Bool(f.overprint_stroke)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameRotationAngle,
                                value: Some(Value::Length(Some(
                                    decompose_angle(f.item_transform),
                                ))),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameScaleX,
                                value: Some(Value::Length(Some(
                                    decompose_scale_x(f.item_transform),
                                ))),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameScaleY,
                                value: Some(Value::Length(Some(
                                    decompose_scale_y(f.item_transform),
                                ))),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameFlipH,
                                value: Some(Value::Bool(
                                    decompose_flip_h(f.item_transform),
                                )),
                            },
                            // W3.A0 — read-side mirror of the W0.3
                            // `FrameFlipV` apply arm (decompose already
                            // computes the V flip).
                            PropertyEntry {
                                path: PropertyPath::FrameFlipV,
                                value: Some(Value::Bool(
                                    decompose_flip_v(f.item_transform),
                                )),
                            },
                            // W3.A0 — thread-chain read (READ-ONLY).
                            // `nextTextFrame` is this frame's own
                            // `NextTextFrame` link (empty ⇒ end of
                            // chain). Authored via `LinkFrames` /
                            // `UnlinkFrames`, never `SetProperty`.
                            PropertyEntry {
                                path: PropertyPath::NextTextFrame,
                                value: Some(Value::Text(
                                    f.next_text_frame.clone().unwrap_or_default(),
                                )),
                            },
                            // `previousTextFrame` is derived: the frame
                            // whose `NextTextFrame` points at this one
                            // (empty ⇒ start of chain). Scans the
                            // spread's text frames — threads don't cross
                            // spreads in the parse model.
                            PropertyEntry {
                                path: PropertyPath::PreviousTextFrame,
                                value: Some(Value::Text(
                                    spread
                                        .text_frames
                                        .iter()
                                        .find(|other| {
                                            other.next_text_frame.as_deref() == Some(raw)
                                        })
                                        .and_then(|other| other.self_id.clone())
                                        .unwrap_or_default(),
                                )),
                            },
                        ];
                        // W0.4 — transparency effects (gap 18).
                        entries.extend(effect_property_entries(
                            f.effects.as_ref(),
                            f.blend_mode.as_deref(),
                        ));
                        entries
                    }),
                ElementId::Rectangle(_) => spread
                    .rectangles
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(raw))
                    .map(|f| {
                        let mut entries = vec![
                            PropertyEntry {
                                path: PropertyPath::FrameBounds,
                                value: Some(Value::Bounds([
                                    f.bounds.top,
                                    f.bounds.left,
                                    f.bounds.bottom,
                                    f.bounds.right,
                                ])),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameTransform,
                                value: Some(Value::Transform(f.item_transform)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameFillColor,
                                value: Some(Value::ColorRef(f.fill_color.clone())),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeColor,
                                value: Some(Value::ColorRef(f.stroke_color.clone())),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeWeight,
                                value: Some(Value::Length(f.stroke_weight)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameOpacity,
                                value: Some(Value::Length(f.opacity)),
                            },
                            PropertyEntry {
                                path: PropertyPath::AppliedObjectStyle,
                                value: Some(Value::Text(
                                    f.applied_object_style.clone().unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameTextWrapMode,
                                value: Some(Value::Text(
                                    f.text_wrap
                                        .map(|t| t.mode.as_idml().to_string())
                                        .unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameTextWrapOffsets,
                                value: Some(Value::Bounds(
                                    f.text_wrap
                                        .map(|t| t.offsets)
                                        .unwrap_or([0.0; 4]),
                                )),
                            },
                            // W2.5 — text-wrap contour options.
                            PropertyEntry {
                                path: PropertyPath::FrameTextWrapContourType,
                                value: Some(Value::Text(
                                    f.text_wrap
                                        .and_then(|t| t.contour_type)
                                        .map(|c| c.as_idml().to_string())
                                        .unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameTextWrapContourIncludeInside,
                                value: Some(Value::Bool(
                                    f.text_wrap
                                        .and_then(|t| t.include_inside_edges)
                                        .unwrap_or(false),
                                )),
                            },
                            // W2.5 — element-level visibility / lock.
                            PropertyEntry {
                                path: PropertyPath::ElementVisible,
                                value: Some(Value::Bool(f.visible)),
                            },
                            PropertyEntry {
                                path: PropertyPath::ElementLocked,
                                value: Some(Value::Bool(f.locked)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadow,
                                value: Some(Value::Bool(
                                    f.drop_shadow.is_some(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameGradientFillAngle,
                                value: Some(Value::Length(f.gradient_fill_angle)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameGradientFillLength,
                                value: Some(Value::Length(f.gradient_fill_length)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameGradientStrokeAngle,
                                value: Some(Value::Length(f.gradient_stroke_angle)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameGradientStrokeLength,
                                value: Some(Value::Length(f.gradient_stroke_length)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameGradientFeather,
                                value: Some(Value::GradientFeather(
                                    f.effects
                                        .as_ref()
                                        .and_then(|e| e.gradient_feather.as_ref())
                                        .map(
                                            paged_mutate::operation::GradientFeatherSpec::from_parse,
                                        ),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowMode,
                                value: Some(Value::Text(
                                    f.drop_shadow
                                        .as_ref()
                                        .map(|s| s.mode.clone())
                                        .unwrap_or_else(|| "Drop".to_string()),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowXOffset,
                                value: Some(Value::Length(
                                    f.drop_shadow.as_ref().map(|s| s.x_offset),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowYOffset,
                                value: Some(Value::Length(
                                    f.drop_shadow.as_ref().map(|s| s.y_offset),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowSize,
                                value: Some(Value::Length(
                                    f.drop_shadow.as_ref().map(|s| s.size),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowOpacity,
                                value: Some(Value::Length(
                                    f.drop_shadow.as_ref().map(|s| s.opacity_pct),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameDropShadowColor,
                                value: Some(Value::ColorRef(
                                    f.drop_shadow
                                        .as_ref()
                                        .and_then(|s| s.effect_color.clone()),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameFillTint,
                                value: Some(Value::Length(f.fill_tint)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameNonprinting,
                                value: Some(Value::Bool(f.nonprinting)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeEndCap,
                                value: Some(Value::Text(
                                    f.end_cap.clone().unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameFittingCrops,
                                value: Some(Value::Bounds(
                                    f.frame_fitting
                                        .as_ref()
                                        .map(|ff| {
                                            [
                                                ff.top_crop.unwrap_or(0.0),
                                                ff.left_crop.unwrap_or(0.0),
                                                ff.bottom_crop.unwrap_or(0.0),
                                                ff.right_crop.unwrap_or(0.0),
                                            ]
                                        })
                                        .unwrap_or([0.0; 4]),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameFittingType,
                                value: Some(Value::Text(
                                    f.frame_fitting
                                        .as_ref()
                                        .and_then(|ff| {
                                            ff.fitting_on_empty_frame.clone()
                                        })
                                        .unwrap_or_default(),
                                )),
                            },
                            // W0.3 — frame fitting alignment / auto-fit.
                            PropertyEntry {
                                path: PropertyPath::FrameFittingReferencePoint,
                                value: Some(Value::Text(
                                    f.frame_fitting
                                        .as_ref()
                                        .and_then(|ff| ff.reference_point.clone())
                                        .unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameAutoFit,
                                value: Some(Value::Bool(
                                    f.frame_fitting
                                        .as_ref()
                                        .and_then(|ff| ff.auto_fit)
                                        .unwrap_or(false),
                                )),
                            },
                            // W0.3 — stroke type / join / miter /
                            // alignment / gap.
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeType,
                                value: Some(Value::Text(
                                    f.stroke_type.clone().unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeJoin,
                                value: Some(Value::Text(
                                    f.end_join.clone().unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeMiterLimit,
                                value: Some(Value::Length(f.miter_limit)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeAlignment,
                                value: Some(Value::Text(
                                    f.stroke_alignment.clone().unwrap_or_default(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeGapColor,
                                value: Some(Value::ColorRef(
                                    f.stroke_gap_color.clone(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeGapTint,
                                value: Some(Value::Length(f.stroke_gap_tint)),
                            },
                            // W1.1 — per-frame dash override (empty vec =
                            // no override; stroke uses its StrokeType).
                            PropertyEntry {
                                path: PropertyPath::FrameStrokeDashArray,
                                value: Some(Value::Lengths(f.stroke_dash.clone())),
                            },
                            // W0.3 — per-corner option + radius. Order
                            // matches IDML `corners[4]`:
                            // [top_left, top_right, bottom_right, bottom_left].
                            PropertyEntry {
                                path: PropertyPath::FrameCornerOptionTopLeft,
                                value: Some(Value::Text(
                                    f.corners[0]
                                        .option
                                        .map(corner_option_idml)
                                        .unwrap_or_default()
                                        .to_string(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameCornerRadiusTopLeft,
                                value: Some(Value::Length(f.corners[0].radius)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameCornerOptionTopRight,
                                value: Some(Value::Text(
                                    f.corners[1]
                                        .option
                                        .map(corner_option_idml)
                                        .unwrap_or_default()
                                        .to_string(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameCornerRadiusTopRight,
                                value: Some(Value::Length(f.corners[1].radius)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameCornerOptionBottomRight,
                                value: Some(Value::Text(
                                    f.corners[2]
                                        .option
                                        .map(corner_option_idml)
                                        .unwrap_or_default()
                                        .to_string(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameCornerRadiusBottomRight,
                                value: Some(Value::Length(f.corners[2].radius)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameCornerOptionBottomLeft,
                                value: Some(Value::Text(
                                    f.corners[3]
                                        .option
                                        .map(corner_option_idml)
                                        .unwrap_or_default()
                                        .to_string(),
                                )),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameCornerRadiusBottomLeft,
                                value: Some(Value::Length(f.corners[3].radius)),
                            },
                            // W0.3 — overprint + transform-decompose.
                            PropertyEntry {
                                path: PropertyPath::FrameOverprintFill,
                                value: Some(Value::Bool(f.overprint_fill)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameOverprintStroke,
                                value: Some(Value::Bool(f.overprint_stroke)),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameRotationAngle,
                                value: Some(Value::Length(Some(
                                    decompose_angle(f.item_transform),
                                ))),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameScaleX,
                                value: Some(Value::Length(Some(
                                    decompose_scale_x(f.item_transform),
                                ))),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameScaleY,
                                value: Some(Value::Length(Some(
                                    decompose_scale_y(f.item_transform),
                                ))),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameFlipH,
                                value: Some(Value::Bool(
                                    decompose_flip_h(f.item_transform),
                                )),
                            },
                            // W3.A0 — read-side mirror of the W0.3
                            // `FrameFlipV` apply arm.
                            PropertyEntry {
                                path: PropertyPath::FrameFlipV,
                                value: Some(Value::Bool(
                                    decompose_flip_v(f.item_transform),
                                )),
                            },
                        ];
                        // W3.A0 — image-bearing rectangles surface the
                        // inner `<Image>` transform so the content
                        // grabber's `ImageContentTransform` apply arm
                        // has a read counterpart. Gated on
                        // `has_image_element` (a plain colour swatch has
                        // no inner image to grab); `None` ⇒ the legacy
                        // "stretch to bounds" frame with no parsed inner
                        // matrix.
                        if f.has_image_element {
                            entries.push(PropertyEntry {
                                path: PropertyPath::ImageContentTransform,
                                value: Some(Value::Transform(
                                    f.image_item_transform,
                                )),
                            });
                        }
                        // W0.4 — transparency effects (gap 18).
                        entries.extend(effect_property_entries(
                            f.effects.as_ref(),
                            f.blend_mode.as_deref(),
                        ));
                        entries
                    }),
                // W1.20 (groups v2) — a Group surfaces its own
                // `ItemTransform` (what `SetGroupTransform` writes) plus
                // its content's union AABB in spread space (so the
                // inspector/layers panel can show the group's extent and
                // pivot transform gestures). Per-member properties are
                // read by selecting the members themselves.
                ElementId::Group(_) => spread
                    .groups
                    .iter()
                    .position(|g| g.self_id.as_deref() == Some(raw))
                    .map(|gi| {
                        let g = &spread.groups[gi];
                        let bounds = group_union_aabb(spread, gi).unwrap_or(paged_parse::Bounds {
                            top: 0.0,
                            left: 0.0,
                            bottom: 0.0,
                            right: 0.0,
                        });
                        vec![
                            PropertyEntry {
                                path: PropertyPath::FrameBounds,
                                value: Some(Value::Bounds([
                                    bounds.top,
                                    bounds.left,
                                    bounds.bottom,
                                    bounds.right,
                                ])),
                            },
                            PropertyEntry {
                                path: PropertyPath::FrameTransform,
                                value: Some(Value::Transform(g.item_transform)),
                            },
                        ]
                    }),
                // Oval / Polygon / GraphicLine: v1 surfaces only the
                // geometry common to all kinds (bounds + transform).
                // Per-kind authored properties (fill, stroke) follow
                // once the apply arms cover them.
                _ => None,
            };
            if let Some(mut entries) = entries {
                // Plugin-metadata carrier (protocol v33) — surface the
                // item's reserved-namespace Label entries. Vendor
                // (non-`x-paged:`) labels stay engine-internal.
                if let Some(labels) = spread.labels.get(raw) {
                    for (key, value) in labels {
                        if key.starts_with("x-paged:") {
                            entries.push(PropertyEntry {
                                path: PropertyPath::PluginMetadata,
                                value: Some(Value::PluginMetadata {
                                    key: key.clone(),
                                    value: Some(value.clone()),
                                    prev: None,
                                }),
                            });
                        }
                    }
                }
                let kind = id.kind_label().to_string();
                let name = None; // TextFrame/Rectangle don't carry a Name attr today.
                return Some(ElementProperties {
                    id: id.clone(),
                    kind,
                    name,
                    entries,
                });
            }
        }
        None
    }

    /// SDK Phase 3 — `(StoryRange, Character*)` snapshot. Walks the
    /// story's runs that intersect `[start, end)`, collects the
    /// resolved per-run value for each character path, and collapses:
    ///
    /// - All runs share the same value → `PropertyEntry.value =
    ///   Some(Value::Length(uniform))` (or `Some(Value::ColorRef(uniform))`).
    /// - Runs disagree → `PropertyEntry.value = None` (the catalog
    ///   renderer shows "—").
    ///
    /// Note: a uniform `None` (every run has `point_size: None`,
    /// i.e. inherits) collapses to `Some(Value::Length(None))` — the
    /// "they all agree on inherit" case, distinct from "they
    /// disagree". This matches the apply layer's semantics where
    /// `Value::Length(None)` is a meaningful "clear the override"
    /// payload, not just absence.
    fn story_range_properties(
        &self,
        story_id: &str,
        start: u32,
        end: u32,
        id: &crate::element_selection::ElementId,
    ) -> Option<crate::channel::ElementProperties> {
        use crate::channel::{ElementProperties, PropertyEntry};
        use paged_mutate::{PropertyPath, Value};

        let story_idx = self
            .scene
            .stories
            .iter()
            .position(|s| s.self_id == story_id)?;
        let story = &self.scene.stories[story_idx].story;

        // Collect per-run + per-paragraph values for each path the
        // apply layer covers today. Character paths walk per-run;
        // paragraph paths walk per-paragraph (each paragraph that
        // intersects [start, end) contributes one value).
        let mut font_sizes: Vec<Option<f32>> = Vec::new();
        let mut leadings: Vec<Option<f32>> = Vec::new();
        let mut trackings: Vec<Option<f32>> = Vec::new();
        let mut fill_colors: Vec<Option<String>> = Vec::new();
        let mut applied_character_styles: Vec<String> = Vec::new();
        // W0.1 — character formatting read-side. One bucket per
        // mutable run field so the toolbar can populate its current
        // state (and show "mixed" via collapse_uniform) before
        // writing. String-valued fields collapse on the raw IDML
        // attribute string; Option<bool> fields collapse on the
        // `Some(_)` value (None ⇒ inherit, surfaced as no override).
        let mut font_families: Vec<Option<String>> = Vec::new();
        let mut font_styles: Vec<Option<String>> = Vec::new();
        let mut kerning_methods: Vec<Option<String>> = Vec::new();
        let mut capitalizations: Vec<Option<String>> = Vec::new();
        let mut positions: Vec<Option<String>> = Vec::new();
        let mut languages: Vec<Option<String>> = Vec::new();
        let mut otf_features: Vec<Option<String>> = Vec::new();
        let mut baseline_shifts: Vec<Option<f32>> = Vec::new();
        let mut horizontal_scales: Vec<Option<f32>> = Vec::new();
        let mut vertical_scales: Vec<Option<f32>> = Vec::new();
        let mut skews: Vec<Option<f32>> = Vec::new();
        let mut underlines: Vec<Option<bool>> = Vec::new();
        let mut strikethrus: Vec<Option<bool>> = Vec::new();
        let mut ligatures: Vec<Option<bool>> = Vec::new();
        let mut space_before: Vec<Option<f32>> = Vec::new();
        let mut space_after: Vec<Option<f32>> = Vec::new();
        let mut first_line_indent: Vec<Option<f32>> = Vec::new();
        let mut applied_paragraph_styles: Vec<String> = Vec::new();
        let mut justifications: Vec<String> = Vec::new();
        // W0.2 — paragraph formatting read-side. One bucket per
        // mutable paragraph field so the toolbar can populate its
        // current state (and show "mixed" via collapse_uniform)
        // before writing. Drop-cap counts collapse on the raw `u32`;
        // bool fields collapse on the `Some(_)` value (None ⇒ inherit).
        let mut left_indent: Vec<Option<f32>> = Vec::new();
        let mut right_indent: Vec<Option<f32>> = Vec::new();
        let mut drop_cap_characters: Vec<u32> = Vec::new();
        let mut drop_cap_lines: Vec<u32> = Vec::new();
        let mut hyphenations: Vec<Option<bool>> = Vec::new();
        let mut keep_lines_together: Vec<Option<bool>> = Vec::new();
        let mut keep_with_next: Vec<Option<u32>> = Vec::new();
        let mut list_types: Vec<Option<String>> = Vec::new();
        let mut bullet_characters: Vec<Option<u32>> = Vec::new();
        let mut numbering_formats: Vec<Option<String>> = Vec::new();
        let mut rule_aboves: Vec<paged_parse::styles::ParagraphRule> = Vec::new();
        let mut rule_belows: Vec<paged_parse::styles::ParagraphRule> = Vec::new();
        let mut tab_lists: Vec<Vec<paged_parse::TabStop>> = Vec::new();

        let mut char_offset: u32 = 0;
        for para in &story.paragraphs {
            let para_chars: u32 = para
                .runs
                .iter()
                .map(|r| r.text.chars().count() as u32)
                .sum();
            let para_start = char_offset;
            let para_end = char_offset + para_chars;

            // Paragraph intersects iff (para_end > start AND
            // para_start < end). Collect paragraph-level values for
            // every intersecting paragraph.
            if para_end > start && para_start < end {
                space_before.push(para.space_before);
                space_after.push(para.space_after);
                first_line_indent.push(para.first_line_indent);
                applied_paragraph_styles.push(para.paragraph_style.clone().unwrap_or_default());
                justifications.push(
                    para.justification
                        .map(|j| j.as_idml().to_string())
                        .unwrap_or_default(),
                );
                // W0.2 — paragraph formatting.
                left_indent.push(para.left_indent);
                right_indent.push(para.right_indent);
                drop_cap_characters.push(para.drop_cap_characters);
                drop_cap_lines.push(para.drop_cap_lines);
                hyphenations.push(para.hyphenation);
                keep_lines_together.push(para.keep_lines_together);
                keep_with_next.push(para.keep_with_next);
                list_types.push(para.bullets_list_type.clone());
                bullet_characters.push(para.bullet_character);
                numbering_formats.push(para.numbering_format.clone());
                rule_aboves.push(para.rule_above.clone());
                rule_belows.push(para.rule_below.clone());
                tab_lists.push(para.tab_list.clone());
            }

            // Character-level walk: per run inside the paragraph.
            for run in &para.runs {
                let run_len = run.text.chars().count() as u32;
                let run_start = char_offset;
                let run_end = char_offset + run_len;
                char_offset = run_end;
                if run_end <= start || run_start >= end {
                    continue;
                }
                font_sizes.push(run.point_size);
                leadings.push(run.leading);
                trackings.push(run.tracking);
                fill_colors.push(run.fill_color.clone());
                applied_character_styles.push(run.character_style.clone().unwrap_or_default());
                font_families.push(run.font.clone());
                font_styles.push(run.font_style.clone());
                kerning_methods.push(run.kerning_method.clone());
                capitalizations.push(run.capitalization.clone());
                positions.push(run.position.clone());
                languages.push(run.applied_language.clone());
                otf_features.push(run.otf_features.clone());
                baseline_shifts.push(run.baseline_shift);
                horizontal_scales.push(run.horizontal_scale);
                vertical_scales.push(run.vertical_scale);
                skews.push(run.skew);
                underlines.push(run.underline);
                strikethrus.push(run.strikethru);
                ligatures.push(run.ligatures_on);
            }
        }

        if font_sizes.is_empty() && space_before.is_empty() {
            // No runs and no paragraphs intersect — well-formed but
            // empty address. Return empty entries so the UI shows
            // the (empty) StoryRange panel rather than treating the
            // address as "not found".
            return Some(ElementProperties {
                id: id.clone(),
                kind: "StoryRange".to_string(),
                name: None,
                entries: Vec::new(),
            });
        }

        let entries = vec![
            PropertyEntry {
                path: PropertyPath::CharacterFontSize,
                value: collapse_uniform(&font_sizes).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::CharacterLeading,
                value: collapse_uniform(&leadings).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::CharacterTracking,
                value: collapse_uniform(&trackings).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::CharacterFillColor,
                value: collapse_uniform(&fill_colors).map(Value::ColorRef),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphSpaceBefore,
                value: collapse_uniform(&space_before).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphSpaceAfter,
                value: collapse_uniform(&space_after).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphFirstLineIndent,
                value: collapse_uniform(&first_line_indent).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::AppliedParagraphStyle,
                value: collapse_uniform(&applied_paragraph_styles).map(Value::Text),
            },
            PropertyEntry {
                path: PropertyPath::AppliedCharacterStyle,
                value: collapse_uniform(&applied_character_styles).map(Value::Text),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphJustification,
                value: collapse_uniform(&justifications).map(Value::Text),
            },
            // W0.1 — character formatting. String fields surface the
            // raw IDML attribute (empty string ⇒ no override, matching
            // the apply-side empty-clears convention); bool fields
            // surface `Some(_)` collapsed (None ⇒ inherit ⇒ false).
            PropertyEntry {
                path: PropertyPath::CharacterFontFamily,
                value: collapse_uniform(&font_families).map(|o| Value::Text(o.unwrap_or_default())),
            },
            PropertyEntry {
                path: PropertyPath::CharacterFontStyle,
                value: collapse_uniform(&font_styles).map(|o| Value::Text(o.unwrap_or_default())),
            },
            PropertyEntry {
                path: PropertyPath::CharacterKerningMethod,
                value: collapse_uniform(&kerning_methods)
                    .map(|o| Value::Text(o.unwrap_or_default())),
            },
            PropertyEntry {
                path: PropertyPath::CharacterCase,
                value: collapse_uniform(&capitalizations)
                    .map(|o| Value::Text(o.unwrap_or_default())),
            },
            PropertyEntry {
                path: PropertyPath::CharacterPosition,
                value: collapse_uniform(&positions).map(|o| Value::Text(o.unwrap_or_default())),
            },
            PropertyEntry {
                path: PropertyPath::CharacterLanguage,
                value: collapse_uniform(&languages).map(|o| Value::Text(o.unwrap_or_default())),
            },
            PropertyEntry {
                path: PropertyPath::CharacterOtfFeatures,
                value: collapse_uniform(&otf_features).map(|o| Value::Text(o.unwrap_or_default())),
            },
            PropertyEntry {
                path: PropertyPath::CharacterBaselineShift,
                value: collapse_uniform(&baseline_shifts).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::CharacterHorizontalScale,
                value: collapse_uniform(&horizontal_scales).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::CharacterVerticalScale,
                value: collapse_uniform(&vertical_scales).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::CharacterSkew,
                value: collapse_uniform(&skews).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::CharacterUnderline,
                value: collapse_uniform(&underlines).map(|o| Value::Bool(o.unwrap_or(false))),
            },
            PropertyEntry {
                path: PropertyPath::CharacterStrikethru,
                value: collapse_uniform(&strikethrus).map(|o| Value::Bool(o.unwrap_or(false))),
            },
            PropertyEntry {
                path: PropertyPath::CharacterLigatures,
                value: collapse_uniform(&ligatures).map(|o| Value::Bool(o.unwrap_or(false))),
            },
            // W0.2 — paragraph formatting. Length fields surface the
            // `Option<f32>` collapsed; drop-cap counts surface as
            // integer-Length; bool fields surface `Some(_)` collapsed
            // (None ⇒ inherit ⇒ the field's IDML default); string
            // fields surface the raw IDML attribute (empty ⇒ no
            // override). Rule structs / the tab list surface as their
            // whole-struct / whole-list wire forms.
            PropertyEntry {
                path: PropertyPath::ParagraphLeftIndent,
                value: collapse_uniform(&left_indent).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphRightIndent,
                value: collapse_uniform(&right_indent).map(Value::Length),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphDropCapCharacters,
                value: collapse_uniform(&drop_cap_characters)
                    .map(|n| Value::Length(Some(n as f32))),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphDropCapLines,
                value: collapse_uniform(&drop_cap_lines).map(|n| Value::Length(Some(n as f32))),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphHyphenation,
                value: collapse_uniform(&hyphenations).map(|o| Value::Bool(o.unwrap_or(true))),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphKeepLinesTogether,
                value: collapse_uniform(&keep_lines_together)
                    .map(|o| Value::Bool(o.unwrap_or(false))),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphKeepWithNext,
                value: collapse_uniform(&keep_with_next)
                    .map(|o| Value::Length(o.map(|n| n as f32))),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphListType,
                value: collapse_uniform(&list_types).map(|o| Value::Text(o.unwrap_or_default())),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphBulletCharacter,
                value: collapse_uniform(&bullet_characters).map(|o| {
                    Value::Text(
                        o.and_then(char::from_u32)
                            .map(|c| c.to_string())
                            .unwrap_or_default(),
                    )
                }),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphNumberingFormat,
                value: collapse_uniform(&numbering_formats)
                    .map(|o| Value::Text(o.unwrap_or_default())),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphRuleAbove,
                value: collapse_uniform(&rule_aboves).map(|r| {
                    Value::ParagraphRule(Some(
                        paged_mutate::operation::ParagraphRuleSpec::from_parse(&r),
                    ))
                }),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphRuleBelow,
                value: collapse_uniform(&rule_belows).map(|r| {
                    Value::ParagraphRule(Some(
                        paged_mutate::operation::ParagraphRuleSpec::from_parse(&r),
                    ))
                }),
            },
            PropertyEntry {
                path: PropertyPath::ParagraphTabStops,
                value: collapse_uniform(&tab_lists).map(|stops| {
                    Value::TabStops(
                        stops
                            .iter()
                            .map(paged_mutate::operation::TabStopSpec::from_parse)
                            .collect(),
                    )
                }),
            },
        ];

        Some(ElementProperties {
            id: id.clone(),
            kind: "StoryRange".to_string(),
            name: None,
            entries,
        })
    }

    /// W3.A1 — locate a `<Table>` by `(story_id, table_id)` for the
    /// inspector read-side. Returns the host paragraph's table ref.
    fn find_table_read(&self, story_id: &str, table_id: &str) -> Option<&paged_parse::Table> {
        let story = self.scene.stories.iter().find(|s| s.self_id == story_id)?;
        story.story.paragraphs.iter().find_map(|p| {
            p.table
                .as_ref()
                .filter(|t| t.self_id.as_deref() == Some(table_id))
        })
    }

    /// W3.A1 — table-scoped property snapshot (the `AppliedTableStyle`
    /// entry). Mirrors `story_range_properties`'s return shape.
    fn table_properties(
        &self,
        story_id: &str,
        table_id: &str,
        id: &crate::element_selection::ElementId,
    ) -> Option<crate::channel::ElementProperties> {
        use crate::channel::{ElementProperties, PropertyEntry};
        use paged_mutate::{PropertyPath, Value};

        let table = self.find_table_read(story_id, table_id)?;
        // Aftercare-A — total physical rows = header + body + footer
        // (the three IDML `*RowCount` attributes). Read-only, carried
        // as the integer-as-`Length` convention drop-cap counts use.
        let row_count = table.header_row_count + table.body_row_count + table.footer_row_count;
        let entries = vec![
            PropertyEntry {
                path: PropertyPath::AppliedTableStyle,
                value: Some(Value::Text(
                    table.applied_table_style.clone().unwrap_or_default(),
                )),
            },
            PropertyEntry {
                path: PropertyPath::TableRowCount,
                value: Some(Value::Length(Some(row_count as f32))),
            },
            PropertyEntry {
                path: PropertyPath::TableColumnCount,
                value: Some(Value::Length(Some(table.column_count as f32))),
            },
        ];
        Some(ElementProperties {
            id: id.clone(),
            kind: "Table".to_string(),
            name: None,
            entries,
        })
    }

    /// W3.A1 — cell-scoped property snapshot (fill / insets / vertical
    /// justify / applied cell style). Backs the Cell panel's read so it
    /// can populate before a `SetElementProperty` write.
    fn cell_properties(
        &self,
        story_id: &str,
        table_id: &str,
        row: u32,
        col: u32,
        id: &crate::element_selection::ElementId,
    ) -> Option<crate::channel::ElementProperties> {
        use crate::channel::{ElementProperties, PropertyEntry};
        use paged_mutate::{PropertyPath, Value};

        let table = self.find_table_read(story_id, table_id)?;
        let cell = table
            .cells
            .iter()
            .find(|c| c.coords() == Some((col, row)))?;
        let entries = vec![
            PropertyEntry {
                path: PropertyPath::CellFillColor,
                value: Some(Value::ColorRef(cell.fill_color.clone())),
            },
            PropertyEntry {
                path: PropertyPath::CellInsetTop,
                value: Some(Value::Length(Some(cell.text_top_inset))),
            },
            PropertyEntry {
                path: PropertyPath::CellInsetLeft,
                value: Some(Value::Length(Some(cell.text_left_inset))),
            },
            PropertyEntry {
                path: PropertyPath::CellInsetBottom,
                value: Some(Value::Length(Some(cell.text_bottom_inset))),
            },
            PropertyEntry {
                path: PropertyPath::CellInsetRight,
                value: Some(Value::Length(Some(cell.text_right_inset))),
            },
            PropertyEntry {
                path: PropertyPath::CellVerticalJustification,
                value: Some(Value::Text(
                    cell.vertical_justification.clone().unwrap_or_default(),
                )),
            },
            PropertyEntry {
                path: PropertyPath::AppliedCellStyle,
                value: Some(Value::Text(
                    cell.applied_cell_style.clone().unwrap_or_default(),
                )),
            },
            // W1.11b — per-cell edge strokes (colour / weight / tint per
            // edge). Colour reads back as a `ColorRef`, weight + tint as
            // `Length`; an absent inline override reports `None` (the
            // value falls through to the cell-style cascade at render).
            PropertyEntry {
                path: PropertyPath::CellTopEdgeStrokeColor,
                value: Some(Value::ColorRef(cell.top_edge_stroke_color.clone())),
            },
            PropertyEntry {
                path: PropertyPath::CellTopEdgeStrokeWeight,
                value: Some(Value::Length(cell.top_edge_stroke_weight)),
            },
            PropertyEntry {
                path: PropertyPath::CellTopEdgeStrokeTint,
                value: Some(Value::Length(cell.top_edge_stroke_tint)),
            },
            PropertyEntry {
                path: PropertyPath::CellBottomEdgeStrokeColor,
                value: Some(Value::ColorRef(cell.bottom_edge_stroke_color.clone())),
            },
            PropertyEntry {
                path: PropertyPath::CellBottomEdgeStrokeWeight,
                value: Some(Value::Length(cell.bottom_edge_stroke_weight)),
            },
            PropertyEntry {
                path: PropertyPath::CellBottomEdgeStrokeTint,
                value: Some(Value::Length(cell.bottom_edge_stroke_tint)),
            },
            PropertyEntry {
                path: PropertyPath::CellLeftEdgeStrokeColor,
                value: Some(Value::ColorRef(cell.left_edge_stroke_color.clone())),
            },
            PropertyEntry {
                path: PropertyPath::CellLeftEdgeStrokeWeight,
                value: Some(Value::Length(cell.left_edge_stroke_weight)),
            },
            PropertyEntry {
                path: PropertyPath::CellLeftEdgeStrokeTint,
                value: Some(Value::Length(cell.left_edge_stroke_tint)),
            },
            PropertyEntry {
                path: PropertyPath::CellRightEdgeStrokeColor,
                value: Some(Value::ColorRef(cell.right_edge_stroke_color.clone())),
            },
            PropertyEntry {
                path: PropertyPath::CellRightEdgeStrokeWeight,
                value: Some(Value::Length(cell.right_edge_stroke_weight)),
            },
            PropertyEntry {
                path: PropertyPath::CellRightEdgeStrokeTint,
                value: Some(Value::Length(cell.right_edge_stroke_tint)),
            },
        ];
        Some(ElementProperties {
            id: id.clone(),
            kind: "TableCell".to_string(),
            name: None,
            entries,
        })
    }

    /// SDK Phase 3 — list every swatch in the document's palette.
    /// One entry per `<Color>` in `graphic.colors`, classified by
    /// model (process / spot) with the built-in specials (None /
    /// Paper / Black / Registration) carrying their canonical
    /// `kind` label so the swatch grid can badge them.
    ///
    /// Backs `documentCollection:swatches` per
    /// `docs/paged/panel-catalog-and-sdk-extension.md` §5.1.
    pub fn swatches(&self) -> Vec<crate::channel::SwatchSummary> {
        use crate::channel::SwatchSummary;
        use paged_parse::ColorModel;
        let mut out = Vec::with_capacity(self.scene.palette.colors.len());
        for (self_id, color) in self.scene.palette.colors.iter() {
            let kind = match paged_parse::graphic::ReservedSwatch::classify(self_id) {
                Some(r) => r.label(),
                None => match color.model {
                    ColorModel::Process => "process",
                    ColorModel::Spot => "spot",
                    ColorModel::MixedInk => "mixedInk",
                    ColorModel::Unknown => "unknown",
                },
            };
            let name = color.name.clone().unwrap_or_else(|| self_id.clone());
            out.push(SwatchSummary {
                self_id: self_id.clone(),
                name,
                kind: kind.to_string(),
            });
        }
        out
    }

    /// SDK Phase 3 — list every paragraph style in the document.
    /// Backs `documentCollection:paragraphStyles` per
    /// `docs/paged/panel-catalog-and-sdk-extension.md` §5.1.
    pub fn paragraph_styles(&self) -> Vec<crate::channel::ParagraphStyleSummary> {
        use crate::channel::ParagraphStyleSummary;
        self.scene
            .styles
            .paragraph_styles
            .iter()
            .map(|(self_id, style)| ParagraphStyleSummary {
                self_id: self_id.clone(),
                name: style.name.clone().unwrap_or_else(|| self_id.clone()),
                based_on: style.based_on.clone(),
                next_style: style.next_style.clone(),
            })
            .collect()
    }

    /// SDK Phase 3 — list every character style in the document.
    pub fn character_styles(&self) -> Vec<crate::channel::CharacterStyleSummary> {
        use crate::channel::CharacterStyleSummary;
        self.scene
            .styles
            .character_styles
            .iter()
            .map(|(self_id, style)| CharacterStyleSummary {
                self_id: self_id.clone(),
                name: style.name.clone().unwrap_or_else(|| self_id.clone()),
                based_on: style.based_on.clone(),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — resolved colour readout for a
    /// single swatch. Returns CMYK percentages when the swatch's
    /// effective colour is in CMYK space, plus a display RGB hex
    /// string the panel can render directly. None when the swatch
    /// id doesn't resolve.
    pub fn color_preview(&self, swatch_id: &str) -> Option<crate::channel::ColorPreview> {
        use paged_parse::graphic::ColorModel;
        let color = self.scene.palette.colors.get(swatch_id)?;
        let model = match color.model {
            ColorModel::Process => "process",
            ColorModel::Spot => "spot",
            ColorModel::MixedInk => "mixedInk",
            ColorModel::Unknown => "unknown",
        };
        let model_str = match paged_parse::graphic::ReservedSwatch::classify(swatch_id) {
            Some(r) => r.label(),
            None => model,
        };
        // `effective_cmyk` already returns IDML percentages
        // (0..=100); the old `* 100.0` here clamped every mid-tone
        // channel to 100% in the preview readout (pre-existing bug,
        // caught by the Concept-2 mixer work).
        let cmyk = color.effective_cmyk().map(|c| {
            [
                c[0].clamp(0.0, 100.0),
                c[1].clamp(0.0, 100.0),
                c[2].clamp(0.0, 100.0),
                c[3].clamp(0.0, 100.0),
            ]
        });
        // Concept 2 — resolve through the active CMM: ICC-accurate
        // when a CMYK working profile is configured (the unconfigured
        // path inside IccCmm is the exact pre-existing naive math),
        // analytic Lab instead of the 50% grey placeholder, plus the
        // out-of-gamut verdict for the mixer/swatch badges.
        let cmm = self.active_cmm();
        let working = working_color_of_with(color, self.use_standard_lab_for_spots);
        let rgb = match working {
            Some(w) => {
                use paged_color::Cmm as _;
                let paged_color::LinearRgb(rgb) = cmm.resolve_display(w);
                rgb
            }
            None => [0.5, 0.5, 0.5],
        };
        let out_of_gamut = match working {
            Some(w) => {
                use paged_color::Cmm as _;
                !matches!(cmm.check_gamut(w), paged_color::GamutStatus::InGamut)
            }
            None => false,
        };
        Some(crate::channel::ColorPreview {
            self_id: swatch_id.to_string(),
            name: color.name.clone().unwrap_or_else(|| swatch_id.to_string()),
            model: model_str.to_string(),
            cmyk,
            rgb_hex: rgb_to_hex(rgb),
            out_of_gamut,
            space: Some(
                match color.space {
                    paged_parse::graphic::ColorSpace::Cmyk => "CMYK",
                    paged_parse::graphic::ColorSpace::Rgb => "RGB",
                    paged_parse::graphic::ColorSpace::Lab => "LAB",
                    paged_parse::graphic::ColorSpace::Gray => "Gray",
                    paged_parse::graphic::ColorSpace::Unknown => "Unknown",
                }
                .to_string(),
            ),
            value: Some(color.value.clone()),
        })
    }

    /// SDK Phase 5 (v1 sweep) — list every spread in the document.
    ///
    /// W3.A0 — each summary carries the spread's *live* guide set
    /// (`SpreadSummary.guides`), rebuilt from `spread.guides` on every
    /// call. This is the read-side counterpart to the guide-CRUD
    /// mutations: `DocumentHandle.ruler_guides` is load-time-only, so
    /// the editor re-queries `collection("spreads")` after an undo to
    /// re-sync its overlay mirror.
    pub fn spreads(&self) -> Vec<crate::channel::SpreadSummary> {
        use crate::channel::{GuideSummary, SpreadSummary};
        use crate::model::GuideOrientationWire;
        self.scene
            .spreads
            .iter()
            .map(|parsed| {
                let self_id = parsed
                    .spread
                    .self_id
                    .clone()
                    .unwrap_or_else(|| parsed.src.clone());
                let page_count = parsed.spread.pages.len() as u32;
                // Positional ids mirror `apply::guide_id_for` —
                // `Guide/<spreadSelf>/<index>`. The spread-self half is
                // empty when the parse carried no `Self`, exactly as the
                // mutation layer mints it, so the ids stay addressable.
                let spread_self = parsed.spread.self_id.as_deref().unwrap_or_default();
                let guides = parsed
                    .spread
                    .guides
                    .iter()
                    .enumerate()
                    .map(|(gi, g)| GuideSummary {
                        id: format!("Guide/{spread_self}/{gi}"),
                        orientation: match g.orientation {
                            paged_parse::GuideOrientation::Vertical => {
                                GuideOrientationWire::Vertical
                            }
                            paged_parse::GuideOrientation::Horizontal => {
                                GuideOrientationWire::Horizontal
                            }
                        },
                        position: g.location,
                        page_index: g.page_index,
                    })
                    .collect();
                SpreadSummary {
                    label: self_id.clone(),
                    self_id,
                    page_count,
                    guides,
                }
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every built page (the same
    /// surface the Navigator panel reads from). Index is 1-based
    /// (matches the "Go to page #" convention).
    pub fn pages(&self) -> Vec<crate::channel::PageSummary> {
        use crate::channel::PageSummary;
        // panels.md gap 10 — document bleed is document-level
        // (`<DocumentPreference>`); carry it on every page so the
        // overlay has it without a second request.
        let bleed = self.scene.container.designmap.document_preference;
        // Build page-self-id → MarginPreference once across all
        // spreads (each spread carries its own pages' margins).
        let mut margins: std::collections::HashMap<&str, &paged_parse::MarginPreference> =
            std::collections::HashMap::new();
        for parsed in &self.scene.spreads {
            for (pid, m) in &parsed.spread.page_margins {
                margins.insert(pid.as_str(), m);
            }
        }
        self.built
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let m = margins.get(p.id.as_str()).copied();
                PageSummary {
                    self_id: p.id.as_str().to_string(),
                    index: (i + 1) as u32,
                    size_pt: [p.width_pt, p.height_pt],
                    margin_top_pt: m.map(|m| m.top).unwrap_or(0.0),
                    margin_left_pt: m.map(|m| m.left).unwrap_or(0.0),
                    margin_bottom_pt: m.map(|m| m.bottom).unwrap_or(0.0),
                    margin_right_pt: m.map(|m| m.right).unwrap_or(0.0),
                    column_count: m.map(|m| m.column_count).unwrap_or(1),
                    column_gutter_pt: m.map(|m| m.column_gutter).unwrap_or(0.0),
                    bleed_top_pt: bleed.bleed_top,
                    bleed_left_pt: bleed.bleed_inside_or_left,
                    bleed_bottom_pt: bleed.bleed_bottom,
                    bleed_right_pt: bleed.bleed_outside_or_right,
                }
            })
            .collect()
    }

    /// panels.md gaps 9/10/19 — list every `<Section>` with its
    /// prefix, numbering style, and the flat body-page range it
    /// spans. `start_page_index` is resolved by matching the
    /// section's `PageStart` against the built pages; `page_count`
    /// runs from this section's start up to the next section's start
    /// (sections are in document order) or the document end.
    pub fn sections(&self) -> Vec<crate::channel::SectionSummary> {
        use crate::channel::SectionSummary;
        let sections = &self.scene.container.designmap.sections;
        // page Self id → flat body-page index.
        let page_index: HashMap<&str, u32> = self
            .built
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.as_str(), i as u32))
            .collect();
        let total_pages = self.built.pages.len() as u32;
        // Resolve each section's start index up-front so page_count
        // can look at the following section's start.
        let starts: Vec<Option<u32>> = sections
            .iter()
            .map(|s| {
                s.page_start
                    .as_deref()
                    .and_then(|pid| page_index.get(pid).copied())
            })
            .collect();
        sections
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let start = starts[i];
                // The next section with a resolved start bounds this
                // section's span; fall back to the document end.
                let next_start = starts[i + 1..]
                    .iter()
                    .find_map(|s| *s)
                    .unwrap_or(total_pages);
                let page_count = match start {
                    Some(st) => next_start.saturating_sub(st),
                    None => 0,
                };
                SectionSummary {
                    self_id: s.self_id.clone(),
                    prefix: if s.include_prefix {
                        s.section_prefix.clone().unwrap_or_default()
                    } else {
                        String::new()
                    },
                    label_style: s.numbering_style.as_str().to_string(),
                    start_page_index: start,
                    page_count,
                }
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every master spread. Stable
    /// HashMap order is undefined; the accessor sorts by self_id
    /// so panel rendering is deterministic across reloads.
    pub fn master_pages(&self) -> Vec<crate::channel::MasterPageSummary> {
        use crate::channel::MasterPageSummary;
        let mut out: Vec<MasterPageSummary> = self
            .scene
            .master_spreads
            .iter()
            .map(|(self_id, ms)| MasterPageSummary {
                label: self_id.clone(),
                self_id: self_id.clone(),
                page_count: ms.spread.pages.len() as u32,
            })
            .collect();
        out.sort_by(|a, b| a.self_id.cmp(&b.self_id));
        out
    }

    /// SDK Phase 5 (v1 sweep) — list every cell style.
    pub fn cell_styles(&self) -> Vec<crate::channel::CellStyleSummary> {
        use crate::channel::CellStyleSummary;
        self.scene
            .styles
            .cell_styles
            .iter()
            .map(|(self_id, style)| CellStyleSummary {
                self_id: self_id.clone(),
                name: style.name.clone().unwrap_or_else(|| self_id.clone()),
                based_on: style.based_on.clone(),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every table style.
    pub fn table_styles(&self) -> Vec<crate::channel::TableStyleSummary> {
        use crate::channel::TableStyleSummary;
        self.scene
            .styles
            .table_styles
            .iter()
            .map(|(self_id, style)| TableStyleSummary {
                self_id: self_id.clone(),
                name: style.name.clone().unwrap_or_else(|| self_id.clone()),
                based_on: style.based_on.clone(),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every font family used in the
    /// document. Walks character runs + paragraph-style defaults +
    /// character-style defaults; dedupes; sums per-family
    /// reference counts. The parse layer doesn't carry a font
    /// registry, so this is derived from content rather than
    /// declared — the panel surfaces "fonts in use" rather than
    /// "fonts installed".
    pub fn fonts(&self) -> Vec<crate::channel::FontSummary> {
        use crate::channel::FontSummary;
        use std::collections::{BTreeMap, BTreeSet};
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        // W1.23 — distinct style strings per family. The parse layer
        // has no Fonts.xml registry (fonts are referenced by name from
        // runs + style defaults), so the honest source for "styles in
        // this family" is the `FontStyle` strings the document itself
        // carries, unioned with the styles the worker has registered
        // face bytes for via `RegisterFont`.
        let mut styles: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut note_style = |family: &str, style: &Option<String>| {
            if let Some(style) = style {
                if !style.is_empty() {
                    styles
                        .entry(family.to_string())
                        .or_default()
                        .insert(style.clone());
                }
            }
        };
        for story in &self.scene.stories {
            for para in &story.story.paragraphs {
                for run in &para.runs {
                    if let Some(family) = &run.font {
                        *counts.entry(family.clone()).or_default() += 1;
                        note_style(family, &run.font_style);
                    }
                }
            }
        }
        for (_, style) in self.scene.styles.paragraph_styles.iter() {
            if let Some(family) = &style.font {
                *counts.entry(family.clone()).or_default() += 1;
                note_style(family, &style.font_style);
            }
        }
        for (_, style) in self.scene.styles.character_styles.iter() {
            if let Some(family) = &style.font {
                *counts.entry(family.clone()).or_default() += 1;
                note_style(family, &style.font_style);
            }
        }
        // Union in the styles the worker registered face bytes for, so
        // the panel can list a registered style even if no content uses
        // it yet. Family match is exact here (the registry is keyed by
        // the family string as the editor registered it).
        for entry in &self.font_registry {
            if let Some(style) = &entry.style {
                if !style.is_empty() {
                    styles
                        .entry(entry.family.clone())
                        .or_default()
                        .insert(style.clone());
                }
            }
        }
        // panels.md gap 4 — a family is "missing" when the worker's
        // font registry has no face for it (the renderer then either
        // substitutes the document-wide default font or drops glyphs).
        // We check the registry directly rather than the resolver,
        // because the resolver's default-font fall-through would mask
        // every missing family as "resolved". Family match is
        // case-insensitive to tolerate `"Open Sans"` vs `"open sans"`
        // registration drift.
        let registered: std::collections::BTreeSet<String> = self
            .font_registry
            .iter()
            .map(|e| e.family.to_lowercase())
            .collect();
        counts
            .into_iter()
            .map(|(family, reference_count)| {
                let is_missing = !registered.contains(&family.to_lowercase());
                let styles = styles
                    .get(&family)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default();
                FontSummary {
                    family,
                    reference_count,
                    is_missing,
                    styles,
                }
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every `<Article>` defined.
    pub fn articles(&self) -> Vec<crate::channel::ArticleSummary> {
        use crate::channel::ArticleSummary;
        self.scene
            .container
            .designmap
            .articles
            .iter()
            .map(|a| ArticleSummary {
                self_id: a.self_id.clone(),
                name: a.name.clone().unwrap_or_else(|| a.self_id.clone()),
                members: a.members.clone(),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every `<Hyperlink>` defined.
    pub fn hyperlinks(&self) -> Vec<crate::channel::HyperlinkSummary> {
        use crate::channel::HyperlinkSummary;
        self.scene
            .container
            .designmap
            .hyperlinks
            .iter()
            .map(|h| HyperlinkSummary {
                self_id: h.self_id.clone(),
                name: h.name.clone().unwrap_or_else(|| h.self_id.clone()),
                source: h.source.clone().unwrap_or_default(),
                destination: h.destination.clone().unwrap_or_default(),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every `<Bookmark>` defined.
    pub fn bookmarks(&self) -> Vec<crate::channel::BookmarkSummary> {
        use crate::channel::BookmarkSummary;
        self.scene
            .container
            .designmap
            .bookmarks
            .iter()
            .map(|b| BookmarkSummary {
                self_id: b.self_id.clone(),
                name: b.name.clone().unwrap_or_else(|| b.self_id.clone()),
                destination: b.destination.clone().unwrap_or_default(),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every `<CrossReferenceSource>`.
    pub fn cross_references(&self) -> Vec<crate::channel::CrossReferenceSummary> {
        use crate::channel::CrossReferenceSummary;
        self.scene
            .container
            .designmap
            .cross_references
            .iter()
            .map(|x| CrossReferenceSummary {
                self_id: x.self_id.clone(),
                name: x.name.clone().unwrap_or_else(|| x.self_id.clone()),
                format: x.format.clone().unwrap_or_default(),
                destination: x.destination.clone().unwrap_or_default(),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every `<Topic>` for the index.
    pub fn index_topics(&self) -> Vec<crate::channel::IndexTopicSummary> {
        use crate::channel::IndexTopicSummary;
        self.scene
            .container
            .designmap
            .index_topics
            .iter()
            .map(|t| IndexTopicSummary {
                self_id: t.self_id.clone(),
                name: t.name.clone().unwrap_or_else(|| t.self_id.clone()),
                sort_order: t.sort_order.clone().unwrap_or_default(),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every `<ConditionSet>` defined.
    /// Backs `documentCollection:conditionSets`.
    pub fn condition_sets(&self) -> Vec<crate::channel::ConditionSetSummary> {
        use crate::channel::ConditionSetSummary;
        self.scene
            .styles
            .condition_sets
            .iter()
            .map(|(self_id, set)| ConditionSetSummary {
                self_id: self_id.clone(),
                name: set.name.clone().unwrap_or_else(|| self_id.clone()),
                conditions: set.conditions.clone(),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every `<ColorGroup>` defined.
    /// Backs `documentCollection:colorGroups`.
    pub fn color_groups(&self) -> Vec<crate::channel::ColorGroupSummary> {
        use crate::channel::ColorGroupSummary;
        self.scene
            .palette
            .color_groups
            .iter()
            .map(|(self_id, group)| ColorGroupSummary {
                self_id: self_id.clone(),
                name: group.name.clone().unwrap_or_else(|| self_id.clone()),
                members: group.members.clone(),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every `<Condition>` defined
    /// in the document. Backs `documentCollection:conditions` per
    /// `panel-catalog-and-sdk-extension.md` §5.1. The Conditions
    /// panel renders this for inspection. Per-condition visibility
    /// toggle lands when `Operation::SetConditionVisible` ships.
    pub fn conditions(&self) -> Vec<crate::channel::ConditionSummary> {
        use crate::channel::ConditionSummary;
        self.scene
            .styles
            .conditions
            .iter()
            .map(|(self_id, cond)| ConditionSummary {
                self_id: self_id.clone(),
                name: cond.name.clone().unwrap_or_else(|| self_id.clone()),
                visible: cond.visible.unwrap_or(true),
                indicator_method: cond.indicator_method.clone().unwrap_or_default(),
            })
            .collect()
    }

    /// W1.22 (engine gap 22) — list every `<NumberingList>` resource
    /// in the document. Backs `documentCollection:numberingLists`.
    /// The editor's list-definitions surface renders this;
    /// `continue_across_stories` drives cross-story numbering
    /// continuity in the renderer.
    pub fn numbering_lists(&self) -> Vec<crate::channel::NumberingListSummary> {
        use crate::channel::NumberingListSummary;
        self.scene
            .styles
            .numbering_lists
            .iter()
            .map(|(self_id, list)| NumberingListSummary {
                self_id: self_id.clone(),
                name: list.name.clone().unwrap_or_else(|| self_id.clone()),
                continue_across_stories: list.continue_across_stories.unwrap_or(false),
                continue_across_documents: list.continue_across_documents.unwrap_or(false),
            })
            .collect()
    }

    /// SDK Phase 5 (v1 sweep) — list every placed-image link in the
    /// document. Walks every page-item kind that can host an image
    /// (Rectangle / Oval / Polygon) and collects `image_link`
    /// entries. Top-first per-spread order; within a spread, the
    /// rectangles → ovals → polygons walk order. The Links panel
    /// consumes this through `useCollection<LinkSummary>("links")`.
    pub fn links(&self) -> Vec<crate::channel::LinkSummary> {
        use crate::channel::LinkSummary;
        // panels.md gap 2 — frames whose image the build couldn't
        // resolve/decode (drew the placeholder) are "missing".
        let missing = self.built.diagnostics.missing_image_frame_ids();
        // panels.md gap 3 — placed-image colour space + ppi lives in
        // the per-spread side map keyed by host-frame self_id.
        let summary = |self_id: String,
                       host_kind: &str,
                       uri: String,
                       meta: Option<&paged_parse::ImageMetadata>| {
            LinkSummary {
                status: if missing.contains(&self_id) {
                    "missing"
                } else {
                    "ok"
                }
                .to_string(),
                colorspace: meta.and_then(|m| m.space.clone()),
                effective_ppi: meta.and_then(|m| m.effective_ppi),
                host_self_id: self_id,
                host_kind: host_kind.to_string(),
                uri,
            }
        };
        let mut out = Vec::new();
        for parsed in &self.scene.spreads {
            let meta_map = &parsed.spread.image_metadata;
            for r in &parsed.spread.rectangles {
                if let (Some(self_id), Some(uri)) = (r.self_id.clone(), r.image_link.clone()) {
                    let meta = meta_map.get(&self_id);
                    out.push(summary(self_id, "Rectangle", uri, meta));
                }
            }
            for o in &parsed.spread.ovals {
                if let (Some(self_id), Some(uri)) = (o.self_id.clone(), o.image_link.clone()) {
                    let meta = meta_map.get(&self_id);
                    out.push(summary(self_id, "Oval", uri, meta));
                }
            }
            for p in &parsed.spread.polygons {
                if let (Some(self_id), Some(uri)) = (p.self_id.clone(), p.image_link.clone()) {
                    let meta = meta_map.get(&self_id);
                    out.push(summary(self_id, "Polygon", uri, meta));
                }
            }
        }
        out
    }

    /// SDK Phase 5 (v1 sweep) — list every object style in the
    /// document. Backs `documentCollection:objectStyles` per
    /// `panel-catalog-and-sdk-extension.md` §5.1. The Object Styles
    /// panel consumes this through `useCollection<ObjectStyleSummary>
    /// ("objectStyles")` and applies the chosen `selfId` via the
    /// existing `AppliedObjectStyle` apply arm.
    pub fn object_styles(&self) -> Vec<crate::channel::ObjectStyleSummary> {
        use crate::channel::ObjectStyleSummary;
        self.scene
            .styles
            .object_styles
            .iter()
            .map(|(self_id, style)| ObjectStyleSummary {
                self_id: self_id.clone(),
                name: style.name.clone().unwrap_or_else(|| self_id.clone()),
                based_on: style.based_on.clone(),
            })
            .collect()
    }

    /// SDK Phase 3 — list every gradient swatch in the palette.
    /// `kind` is the IDML `Type` attribute — `"linear"` / `"radial"`.
    pub fn gradients(&self) -> Vec<crate::channel::GradientSummary> {
        use crate::channel::GradientSummary;
        use paged_parse::GradientKind;
        self.scene
            .palette
            .gradients
            .iter()
            .map(|(self_id, g)| {
                let kind = match g.kind {
                    GradientKind::Linear => "linear",
                    GradientKind::Radial => "radial",
                    GradientKind::Unknown => "unknown",
                };
                GradientSummary {
                    self_id: self_id.clone(),
                    name: g.name.clone().unwrap_or_else(|| self_id.clone()),
                    kind: kind.to_string(),
                }
            })
            .collect()
    }

    /// Concept 2 — full stop detail for one gradient. Stops carry
    /// the swatch REF (the model identity) plus a display hex
    /// resolved through the active CMM, so the ramp editor paints
    /// faithfully under the current working space.
    pub fn gradient_detail(&self, gradient_id: &str) -> Option<crate::channel::GradientDetail> {
        use paged_parse::GradientKind;
        let g = self.scene.palette.gradients.get(gradient_id)?;
        let kind = match g.kind {
            GradientKind::Linear => "linear",
            GradientKind::Radial => "radial",
            GradientKind::Unknown => "unknown",
        };
        let cmm = self.active_cmm();
        let stops = g
            .stops
            .iter()
            .map(|s| {
                let resolved_rgb_hex = self
                    .scene
                    .palette
                    .resolve(&s.stop_color)
                    .and_then(working_color_of)
                    .map(|w| {
                        use paged_color::Cmm as _;
                        let paged_color::LinearRgb(rgb) = cmm.resolve_display(w);
                        rgb_to_hex(rgb)
                    })
                    .unwrap_or_else(|| "#808080".to_string());
                crate::channel::GradientStopWire {
                    stop_color_ref: s.stop_color.clone(),
                    resolved_rgb_hex,
                    location_pct: s.location_pct,
                    midpoint_pct: s.midpoint_pct,
                }
            })
            .collect();
        Some(crate::channel::GradientDetail {
            self_id: gradient_id.to_string(),
            name: g.name.clone().unwrap_or_else(|| gradient_id.to_string()),
            kind: kind.to_string(),
            stops,
        })
    }

    /// Concept 2 — serialise swatches to `.ase` bytes ("Save
    /// .ase…"). `group_id: Some` exports one ColorGroup's members;
    /// `None` exports every user colour, grouped by the document's
    /// ColorGroups (ungrouped colours land loose). Reserved
    /// swatches and unknown spaces are skipped.
    pub fn export_ase(&self, group_id: Option<&str>) -> Vec<u8> {
        use paged_color::ase::{AseEntry, AseGroup, AseKind, AseLibrary, AseSpace};
        use paged_parse::graphic::{ColorModel, ColorSpace};
        let entry_of = |id: &str| -> Option<AseEntry> {
            // Reserved swatches aren't exchange material.
            if matches!(
                id,
                "Color/None" | "Color/Paper" | "Color/Black" | "Color/Registration"
            ) {
                return None;
            }
            let c = self.scene.palette.colors.get(id)?;
            let space = match c.space {
                ColorSpace::Rgb => AseSpace::Rgb,
                ColorSpace::Cmyk => AseSpace::Cmyk,
                ColorSpace::Lab => AseSpace::Lab,
                ColorSpace::Gray => AseSpace::Gray,
                ColorSpace::Unknown => return None,
            };
            Some(AseEntry {
                name: c.name.clone().unwrap_or_else(|| id.to_string()),
                space,
                value: c.value.clone(),
                kind: match c.model {
                    ColorModel::Spot => AseKind::Spot,
                    _ => AseKind::Process,
                },
            })
        };
        let mut lib = AseLibrary::default();
        match group_id {
            Some(gid) => {
                if let Some(g) = self.scene.palette.color_groups.get(gid) {
                    lib.groups.push(AseGroup {
                        name: g.name.clone().unwrap_or_else(|| gid.to_string()),
                        entries: g.members.iter().filter_map(|m| entry_of(m)).collect(),
                    });
                }
            }
            None => {
                let mut grouped: std::collections::HashSet<&str> = Default::default();
                for (gid, g) in &self.scene.palette.color_groups {
                    let entries: Vec<AseEntry> =
                        g.members.iter().filter_map(|m| entry_of(m)).collect();
                    for m in &g.members {
                        grouped.insert(m.as_str());
                    }
                    if !entries.is_empty() {
                        lib.groups.push(AseGroup {
                            name: g.name.clone().unwrap_or_else(|| gid.clone()),
                            entries,
                        });
                    }
                }
                for id in self.scene.palette.colors.keys() {
                    if !grouped.contains(id.as_str()) {
                        if let Some(e) = entry_of(id) {
                            lib.loose.push(e);
                        }
                    }
                }
            }
        }
        paged_color::ase::write_ase(&lib)
    }

    /// Concept 2 (Ink Manager) — the ink list: one row per spot
    /// swatch, settings folded in.
    pub fn inks(&self) -> Vec<crate::channel::InkSummary> {
        self.scene
            .palette
            .colors
            .iter()
            .filter(|(_, c)| c.model == paged_parse::graphic::ColorModel::Spot)
            .map(|(id, c)| {
                let setting = self.ink_settings.get(id);
                crate::channel::InkSummary {
                    spot_id: id.clone(),
                    name: c.name.clone().unwrap_or_else(|| id.clone()),
                    convert_to_process: setting.is_some_and(|s| s.convert_to_process),
                    alias_to: setting.and_then(|s| s.alias_to.clone()),
                }
            })
            .collect()
    }

    /// SDK Phase 3 — list every story's self_id + character count.
    /// Used by `paged.stories()` (the script host fn) and by tests
    /// that need a valid story id to address a StoryRange edit.
    pub fn stories(&self) -> Vec<crate::channel::StorySummary> {
        // panels.md gap 1 — stories the build flagged overset (text
        // dropped past the last frame in their chain).
        let overset = self.built.diagnostics.overset_story_ids();
        self.scene
            .stories
            .iter()
            .map(|s| {
                let mut chars: u32 = 0;
                for para in &s.story.paragraphs {
                    for run in &para.runs {
                        chars += run.text.chars().count() as u32;
                    }
                }
                crate::channel::StorySummary {
                    self_id: s.self_id.clone(),
                    character_count: chars,
                    paragraph_count: s.story.paragraphs.len() as u32,
                    overset: overset.contains(&s.self_id),
                }
            })
            .collect()
    }

    /// SDK Phase 5 (D1) — generic document-collection dispatcher per
    /// `panel-catalog-and-sdk-extension.md` §5.1 + plan Task B.
    /// Routes the requested `CollectionName` to the per-collection
    /// accessor and returns the serialized array as a
    /// `serde_json::Value` — one wire shape that handles all 21
    /// collections without per-name plumbing in the channel envelope.
    ///
    /// Consumers deserialize against the typed `*Summary` matching
    /// the `documentCollection:<name>` ReadSpec they declared. Names
    /// without a backing accessor return `Value::Array(vec![])` — the
    /// audit pass per §10 flags these as "wire-shape exists, accessor
    /// pending."
    pub fn collection(&self, name: crate::channel::CollectionName) -> serde_json::Value {
        use crate::channel::CollectionName::*;
        match name {
            Swatches => serde_json::to_value(self.swatches()).unwrap_or_default(),
            Gradients => serde_json::to_value(self.gradients()).unwrap_or_default(),
            ParagraphStyles => serde_json::to_value(self.paragraph_styles()).unwrap_or_default(),
            CharacterStyles => serde_json::to_value(self.character_styles()).unwrap_or_default(),
            Layers => serde_json::to_value(self.layers()).unwrap_or_default(),
            ObjectStyles => serde_json::to_value(self.object_styles()).unwrap_or_default(),
            Links => serde_json::to_value(self.links()).unwrap_or_default(),
            Conditions => serde_json::to_value(self.conditions()).unwrap_or_default(),
            Spreads => serde_json::to_value(self.spreads()).unwrap_or_default(),
            Pages => serde_json::to_value(self.pages()).unwrap_or_default(),
            MasterPages => serde_json::to_value(self.master_pages()).unwrap_or_default(),
            CellStyles => serde_json::to_value(self.cell_styles()).unwrap_or_default(),
            TableStyles => serde_json::to_value(self.table_styles()).unwrap_or_default(),
            Fonts => serde_json::to_value(self.fonts()).unwrap_or_default(),
            ConditionSets => serde_json::to_value(self.condition_sets()).unwrap_or_default(),
            ColorGroups => serde_json::to_value(self.color_groups()).unwrap_or_default(),
            Articles => serde_json::to_value(self.articles()).unwrap_or_default(),
            Hyperlinks => serde_json::to_value(self.hyperlinks()).unwrap_or_default(),
            Bookmarks => serde_json::to_value(self.bookmarks()).unwrap_or_default(),
            CrossReferences => serde_json::to_value(self.cross_references()).unwrap_or_default(),
            IndexTopics => serde_json::to_value(self.index_topics()).unwrap_or_default(),
            Inks => serde_json::to_value(self.inks()).unwrap_or_default(),
            Sections => serde_json::to_value(self.sections()).unwrap_or_default(),
            Stories => serde_json::to_value(self.stories()).unwrap_or_default(),
            NumberingLists => serde_json::to_value(self.numbering_lists()).unwrap_or_default(),
        }
    }

    /// SDK Phase 5 (D1) — singleton document-state snapshot per
    /// `panel-catalog-and-sdk-extension.md` §5.6. Powers the Info
    /// panel + status-bar + any chrome that reflects whole-document
    /// state (vs. selection state). Scalar reads; the six fields
    /// cover v1 panel needs.
    pub fn document_meta(&self) -> crate::channel::DocumentMeta {
        // W2.5 — baseline-grid settings live on the parsed designmap.
        let gp = &self.scene.container.designmap.grid_preference;
        crate::channel::DocumentMeta {
            page_count: self.built.pages.len() as u32,
            // `active_page` is application state (canvas-side camera
            // focus + Pages panel selection). The worker doesn't
            // track it; consumers fold their own active-page state
            // into the binding renderer when they need it.
            active_page: None,
            units: String::new(),
            color_mode: String::new(),
            document_name: String::new(),
            dirty: false,
            default_fill_color: self.document_defaults.fill_color.clone(),
            default_stroke_color: self.document_defaults.stroke_color.clone(),
            default_stroke_weight: self.document_defaults.stroke_weight,
            // Concept 2 — active colour-management settings. The
            // profile NAME falls back to the designmap's declared
            // working space so the settings dialog can show what the
            // document asks for even before a registered profile
            // activates it.
            cmyk_profile_name: self.color_settings.cmyk_profile_name.clone().or_else(|| {
                self.scene
                    .container
                    .designmap
                    .color_settings
                    .cmyk_profile
                    .clone()
            }),
            rgb_policy: self.color_settings.rgb_policy.clone(),
            cmyk_profile_active: self.icc_bytes.is_some(),
            rendering_intent: Some(self.color_settings.intent.name().to_string()),
            black_point_compensation: Some(self.color_settings.bpc),
            proof_profile_name: self.proof_state.as_ref().map(|p| p.name.clone()),
            proof_simulate_paper_white: self.proof_state.as_ref().map(|p| p.simulate_paper_white),
            use_standard_lab_for_spots: Some(self.use_standard_lab_for_spots),
            // W2.5 — read-only baseline-grid settings from
            // `<GridPreference>`. All `None` when the document carried no
            // such element (the editor then shows InDesign's defaults).
            baseline_grid_start: gp.baseline_start,
            baseline_grid_division: gp.baseline_division,
            baseline_grid_shown: gp.baseline_grid_shown,
            baseline_grid_relative_to: gp.baseline_grid_relative_option.clone(),
            baseline_grid_color: gp.baseline_color.clone(),
        }
    }

    /// Inspector P1 — build the scene-tree outline. Spread → Page →
    /// (group leaves OR top-level frames). Light enough to send
    /// eagerly; the panel can re-fetch on `mutationApplied`.
    pub fn scene_tree(&self) -> Vec<crate::channel::SceneTreeNode> {
        use crate::channel::SceneTreeNode;
        use paged_parse::FrameRef;
        let mut spread_nodes = Vec::with_capacity(self.scene.spreads.len());
        for (si, parsed) in self.scene.spreads.iter().enumerate() {
            let spread = &parsed.spread;
            let spread_label = spread
                .self_id
                .clone()
                .map(|id| format!("Spread {id}"))
                .unwrap_or_else(|| format!("Spread {}", si + 1));
            let mut page_nodes = Vec::with_capacity(spread.pages.len().max(1));
            // Single page node for now — the parser doesn't carry a
            // per-page frame map; all frames belong to the spread.
            // Per-page partitioning is a v2 refinement.
            let order: Vec<FrameRef> = if !spread.frames_in_order.is_empty() {
                spread.frames_in_order.clone()
            } else {
                let mut v: Vec<FrameRef> = Vec::new();
                v.extend((0..spread.text_frames.len()).map(FrameRef::TextFrame));
                v.extend((0..spread.rectangles.len()).map(FrameRef::Rectangle));
                v.extend((0..spread.ovals.len()).map(FrameRef::Oval));
                v.extend((0..spread.graphic_lines.len()).map(FrameRef::GraphicLine));
                v.extend((0..spread.polygons.len()).map(FrameRef::Polygon));
                v
            };
            let frame_nodes: Vec<SceneTreeNode> = order
                .into_iter()
                .filter_map(|fr| frame_to_tree_node(spread, fr))
                .collect();
            let label = spread
                .pages
                .first()
                .and_then(|p| p.name.clone())
                .unwrap_or_else(|| "Page".to_string());
            page_nodes.push(SceneTreeNode {
                id: None,
                kind: "Page".to_string(),
                label,
                children: frame_nodes,
            });
            spread_nodes.push(SceneTreeNode {
                id: None,
                kind: "Spread".to_string(),
                label: spread_label,
                children: page_nodes,
            });
        }
        spread_nodes
    }

    pub fn group_leaves(&self, group_self_id: &str) -> Vec<crate::element_selection::ElementId> {
        use crate::element_selection::ElementId;
        let mut out = Vec::new();
        for parsed in &self.scene.spreads {
            let spread = &parsed.spread;
            let Some(group) = spread
                .groups
                .iter()
                .find(|g| g.self_id.as_deref() == Some(group_self_id))
            else {
                continue;
            };
            // Recurse via a worklist instead of true recursion so we
            // don't blow the stack on pathological group nesting.
            let mut stack: Vec<&paged_parse::Group> = vec![group];
            while let Some(g) = stack.pop() {
                for member in &g.members {
                    match *member {
                        paged_parse::FrameRef::TextFrame(i) => {
                            if let Some(f) = spread.text_frames.get(i) {
                                if let Some(id) = f.self_id.as_deref() {
                                    out.push(ElementId::TextFrame(id.to_string()));
                                }
                            }
                        }
                        paged_parse::FrameRef::Rectangle(i) => {
                            if let Some(f) = spread.rectangles.get(i) {
                                if let Some(id) = f.self_id.as_deref() {
                                    out.push(ElementId::Rectangle(id.to_string()));
                                }
                            }
                        }
                        paged_parse::FrameRef::Oval(i) => {
                            if let Some(f) = spread.ovals.get(i) {
                                if let Some(id) = f.self_id.as_deref() {
                                    out.push(ElementId::Oval(id.to_string()));
                                }
                            }
                        }
                        paged_parse::FrameRef::Polygon(i) => {
                            if let Some(f) = spread.polygons.get(i) {
                                if let Some(id) = f.self_id.as_deref() {
                                    out.push(ElementId::Polygon(id.to_string()));
                                }
                            }
                        }
                        paged_parse::FrameRef::GraphicLine(i) => {
                            if let Some(f) = spread.graphic_lines.get(i) {
                                if let Some(id) = f.self_id.as_deref() {
                                    out.push(ElementId::GraphicLine(id.to_string()));
                                }
                            }
                        }
                        paged_parse::FrameRef::Group(i) => {
                            if let Some(nested) = spread.groups.get(i) {
                                stack.push(nested);
                            }
                        }
                    }
                }
            }
            break;
        }
        out
    }

    /// Aftercare-A — `RequestWordBounds` accessor. Reconstructs the
    /// story's byte-text (paragraphs joined by the synthetic
    /// inter-paragraph `\n`, matching the offset convention the caret /
    /// line-bounds / hit-test paths share) and returns the
    /// `[start, end)` byte span of the word (per UAX-29 word
    /// segmentation) containing `offset`. The editor's double-click
    /// word-selection drives this. `None` when the story id doesn't
    /// resolve or the story has no text. See [`geometry::word_bounds`]
    /// for the whitespace / boundary / clamp behaviour.
    pub fn word_bounds(
        &self,
        story_id: &str,
        cell: Option<&crate::selection::TextCellAddr>,
        offset: u32,
    ) -> Option<crate::geometry::WordBounds> {
        let text = self.stream_byte_text(story_id, cell)?;
        crate::geometry::word_bounds(&text, offset)
    }

    /// W1.23 — the `[start, end)` byte span of the paragraph containing
    /// `offset`, per the stream's reconstructed byte string (paragraphs
    /// joined by the synthetic inter-paragraph `\n`). Backs the caret's
    /// triple-click paragraph-selection. Mirrors [`Self::word_bounds`]
    /// exactly: same reconstruction, same byte-offset address space.
    pub fn paragraph_bounds(
        &self,
        story_id: &str,
        cell: Option<&crate::selection::TextCellAddr>,
        offset: u32,
    ) -> Option<crate::geometry::ParagraphBounds> {
        let text = self.stream_byte_text(story_id, cell)?;
        crate::geometry::paragraph_bounds(&text, offset)
    }

    /// W1.13 — reconstruct the byte-text of a paragraph stream: the
    /// story body when `cell` is `None`, or the named cell's paragraphs
    /// when `cell` is `Some`. Paragraphs are joined with the single
    /// synthetic `\n` that the offset contract accounts for, so the
    /// string is index-compatible with every stream offset on the wire.
    /// `None` when the story / table / cell address doesn't resolve.
    fn stream_byte_text(
        &self,
        story_id: &str,
        cell: Option<&crate::selection::TextCellAddr>,
    ) -> Option<String> {
        let story = self.scene.stories.iter().find(|s| s.self_id == story_id)?;
        let paragraphs: &[paged_parse::Paragraph] = match cell {
            None => &story.story.paragraphs,
            Some(addr) => {
                let table = story
                    .story
                    .paragraphs
                    .iter()
                    .filter_map(|p| p.table.as_ref())
                    .find(|t| t.self_id.as_deref() == Some(addr.table_id.as_str()))?;
                &table
                    .cells
                    .iter()
                    .find(|c| c.coords() == Some((addr.col, addr.row)))?
                    .paragraphs
            }
        };
        let mut text = String::new();
        for (i, para) in paragraphs.iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            for run in &para.runs {
                text.push_str(&run.text);
            }
        }
        Some(text)
    }

    pub fn element_geometry(
        &self,
        ids: &[crate::element_selection::ElementId],
    ) -> Vec<crate::channel::ElementGeometryItem> {
        use crate::element_selection::ElementId;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            // Aftercare-A — per-cell geometry. A `TableCell` resolves
            // against the BuiltPage's retained `cell_rects` (the W3.A1
            // hit-test map), not a spread page-item vec. The rect is
            // already page-local pt (`(0,0)` = page top-left, the
            // `LineLayout` frame), so it needs no `item_transform`:
            // the overlay consumes `bounds` directly. `bounds` follows
            // the `[top, left, bottom, right]` convention by mapping
            // the cell rect `[x, y, w, h]` → `[y, x, y+h, x+w]`. The
            // editor's cell overlay upgrades from the table AABB to
            // this precise cell rect.
            if let ElementId::TableCell {
                story_id,
                table_id,
                row,
                col,
            } = id
            {
                for bp in &self.built().pages {
                    if let Some(cr) = bp.cell_rects.iter().find(|cr| {
                        cr.story_id == *story_id
                            && cr.table_id == *table_id
                            && cr.row == *row
                            && cr.col == *col
                    }) {
                        let [x, y, w, h] = cr.rect;
                        out.push(crate::channel::ElementGeometryItem {
                            id: id.clone(),
                            page_id: bp.id.clone(),
                            bounds: [y, x, y + h, x + w],
                            item_transform: None,
                            has_image: false,
                        });
                        break;
                    }
                }
                continue;
            }
            let raw = id.raw_id();
            for parsed in &self.scene().spreads {
                let spread = &parsed.spread;
                let resolved: Option<(paged_parse::Bounds, Option<[f32; 6]>, bool)> = match id {
                    ElementId::TextFrame(_) => spread
                        .text_frames
                        .iter()
                        .find(|f| f.self_id.as_deref() == Some(raw))
                        .map(|f| (f.bounds, f.item_transform, false)),
                    ElementId::Rectangle(_) => spread
                        .rectangles
                        .iter()
                        .find(|f| f.self_id.as_deref() == Some(raw))
                        .map(|f| (f.bounds, f.item_transform, f.has_image_element)),
                    ElementId::Oval(_) => spread
                        .ovals
                        .iter()
                        .find(|f| f.self_id.as_deref() == Some(raw))
                        .map(|f| (f.bounds, f.item_transform, false)),
                    ElementId::Polygon(_) => spread
                        .polygons
                        .iter()
                        .find(|f| f.self_id.as_deref() == Some(raw))
                        .map(|f| (f.bounds, f.item_transform, false)),
                    ElementId::GraphicLine(_) => spread
                        .graphic_lines
                        .iter()
                        .find(|f| f.self_id.as_deref() == Some(raw))
                        .map(|f| (f.bounds, f.item_transform, false)),
                    // Groups themselves are not directly addressable
                    // through geometry today — the overlay draws the
                    // leaves. A future revision can compute the group's
                    // union bbox once Phase B introduces "enter group".
                    ElementId::Group(_) => None,
                    // StoryRange is a property-address, not a
                    // geometric element — its bounds are the host
                    // frame's. The caret/selection-rect surface
                    // (RequestCaretGeometry / RequestSelectionGeometry)
                    // is the right read path for text-range visuals.
                    ElementId::StoryRange { .. } => None,
                    // W3.A1 — Table / TableCell geometry comes from the
                    // retained `cell_rects` on the BuiltPage (the hit
                    // path), not the spread's page-item vecs. No
                    // spread-frame geometry to resolve here.
                    ElementId::Table { .. } | ElementId::TableCell { .. } => None,
                };
                let Some((bounds, item_transform, has_image)) = resolved else {
                    continue;
                };
                // Locate the page the element sits on by checking
                // which of its PARENT SPREAD's pages contains the
                // transformed centroid, then resolve the BuiltPage by
                // matching self_id. Restricting the search to the
                // parent spread is what makes cross-spread duplicates
                // route correctly: a clone in spread B's `text_frames`
                // vec carries bounds in spread-B-local coords, and
                // those coords would alias spread-A's page-bounds
                // rectangles in a flat all-pages search (Track K
                // open follow-up).
                let aabb = crate::hit::transform_bbox(bounds, item_transform);
                let cx = (aabb.left + aabb.right) * 0.5;
                let cy = (aabb.top + aabb.bottom) * 0.5;
                let host_page = parsed.spread.pages.iter().find(|p| {
                    let pb = crate::hit::transform_bbox(p.bounds, p.item_transform);
                    cx >= pb.left && cx <= pb.right && cy >= pb.top && cy <= pb.bottom
                });
                let built_page = host_page
                    .and_then(|p| p.self_id.as_deref())
                    .and_then(|sid| self.built().pages.iter().find(|bp| bp.id.as_str() == sid));
                if let Some(bp) = built_page {
                    out.push(crate::channel::ElementGeometryItem {
                        id: id.clone(),
                        page_id: bp.id.clone(),
                        bounds: [bounds.top, bounds.left, bounds.bottom, bounds.right],
                        item_transform,
                        has_image,
                    });
                }
                break;
            }
        }
        out
    }

    /// Step 5 — `RequestPathAnchors` accessor. Returns the anchor
    /// list + subpath markers for a single Polygon / Rectangle / Oval
    /// / TextFrame, alongside its page and composed transform.
    /// Rectangles / Ovals declared via `GeometricBounds` only (no
    /// `<PathGeometry>`) come back with an empty `anchors` vector —
    /// callers treat that as "nothing to draw".
    pub fn path_anchors(
        &self,
        id: &crate::element_selection::ElementId,
    ) -> Option<crate::channel::PathAnchorsResult> {
        use crate::channel::{PathAnchorTriple, PathAnchorsResult};
        use crate::element_selection::ElementId;
        use paged_parse::PathAnchor;

        // Resolved per-frame path geometry borrowed from the scene:
        // (bounds, item_transform, anchors, subpath_starts, subpath_open).
        type FramePathGeom<'a> = (
            paged_parse::Bounds,
            Option<[f32; 6]>,
            &'a [PathAnchor],
            &'a [usize],
            &'a [bool],
        );

        let raw = id.raw_id();
        for parsed in &self.scene().spreads {
            let spread = &parsed.spread;
            let resolved: Option<FramePathGeom> = match id {
                ElementId::TextFrame(_) => spread
                    .text_frames
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(raw))
                    .map(|f| {
                        (
                            f.bounds,
                            f.item_transform,
                            f.anchors.as_slice(),
                            f.subpath_starts.as_slice(),
                            f.subpath_open.as_slice(),
                        )
                    }),
                ElementId::Rectangle(_) => spread
                    .rectangles
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(raw))
                    .map(|f| {
                        (
                            f.bounds,
                            f.item_transform,
                            f.anchors.as_slice(),
                            f.subpath_starts.as_slice(),
                            f.subpath_open.as_slice(),
                        )
                    }),
                // Ovals don't carry a parsed PathGeometry — they're
                // declared by GeometricBounds; render shows them as
                // ellipses. Drop them silently rather than synthesise
                // 4 fake anchors.
                ElementId::Oval(_) => None,
                ElementId::Polygon(_) => spread
                    .polygons
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(raw))
                    .map(|f| {
                        (
                            f.bounds,
                            f.item_transform,
                            f.anchors.as_slice(),
                            f.subpath_starts.as_slice(),
                            f.subpath_open.as_slice(),
                        )
                    }),
                ElementId::GraphicLine(_) => spread
                    .graphic_lines
                    .iter()
                    .find(|f| f.self_id.as_deref() == Some(raw))
                    .map(|f| {
                        (
                            f.bounds,
                            f.item_transform,
                            f.anchors.as_slice(),
                            f.subpath_starts.as_slice(),
                            f.subpath_open.as_slice(),
                        )
                    }),
                ElementId::Group(_) => None,
                // StoryRange doesn't carry path anchors.
                ElementId::StoryRange { .. } => None,
                // W3.A1 — tables / cells have no path-anchor array.
                ElementId::Table { .. } | ElementId::TableCell { .. } => None,
            };
            let Some((bounds, item_transform, anchors, subpath_starts, subpath_open)) = resolved
            else {
                continue;
            };
            // Same spread-scoped page-resolution as element_geometry:
            // search the PARENT spread's pages so a clone in another
            // spread's vec doesn't alias the source spread's page
            // bounds (Track K cross-spread fix).
            let aabb = crate::hit::transform_bbox(bounds, item_transform);
            let cx = (aabb.left + aabb.right) * 0.5;
            let cy = (aabb.top + aabb.bottom) * 0.5;
            let host_page = parsed.spread.pages.iter().find(|p| {
                let pb = crate::hit::transform_bbox(p.bounds, p.item_transform);
                cx >= pb.left && cx <= pb.right && cy >= pb.top && cy <= pb.bottom
            })?;
            let page = self
                .built()
                .pages
                .iter()
                .find(|bp| Some(bp.id.as_str()) == host_page.self_id.as_deref())?;
            let anchors_out: Vec<PathAnchorTriple> = anchors
                .iter()
                .map(|a| PathAnchorTriple {
                    anchor: [a.anchor.0, a.anchor.1],
                    left: [a.left.0, a.left.1],
                    right: [a.right.0, a.right.1],
                })
                .collect();
            return Some(PathAnchorsResult {
                id: id.clone(),
                page_id: page.id.clone(),
                anchors: anchors_out,
                subpath_starts: subpath_starts.iter().map(|&n| n as u32).collect(),
                subpath_open: subpath_open.to_vec(),
                item_transform,
            });
        }
        None
    }

    /// Same as `pages_for_story` but returns page *indices* into
    /// `built().pages`. Convenient for the GPU scene cache which
    /// keys by index. Indices not currently in `page_index` (stale
    /// after a rebuild that removed pages) are skipped.
    /// B-06 — `RequestNearestPathPoint` accessor: closest on-curve
    /// point via the kurbo kernel. `point` is element-local (the
    /// `path_anchors` space).
    pub fn nearest_path_point(
        &self,
        id: &crate::element_selection::ElementId,
        point: [f32; 2],
    ) -> Option<crate::channel::NearestPathPointResult> {
        let table = self.path_anchors(id)?;
        if table.anchors.is_empty() {
            return None;
        }
        let anchors: Vec<paged_parse::PathAnchor> = table
            .anchors
            .iter()
            .map(|a| paged_parse::PathAnchor {
                anchor: (a.anchor[0], a.anchor[1]),
                left: (a.left[0], a.left[1]),
                right: (a.right[0], a.right[1]),
            })
            .collect();
        let starts: Vec<usize> = table.subpath_starts.iter().map(|s| *s as usize).collect();
        let hit = paged_mutate::kurbo_kernel::nearest_point_on_path(
            &anchors,
            &starts,
            &table.subpath_open,
            (point[0], point[1]),
        )?;
        Some(crate::channel::NearestPathPointResult {
            seg_start: hit.seg_start as u32,
            seg_end: hit.seg_end as u32,
            t: hit.t,
            point: [hit.point.0, hit.point.1],
            distance: hit.distance,
        })
    }

    pub fn page_indices_for_story(&self, story_id: &str) -> Vec<usize> {
        self.pages_for_story(story_id)
            .iter()
            .filter_map(|id| self.page_index.get(id).copied())
            .collect()
    }

    /// Rebuild the `BuiltDocument` from the (possibly-mutated) scene.
    /// Phase 4 Step 1 — installs the persistent `layout_cache` so
    /// paragraphs whose `(text, style, width, font)` signature didn't
    /// change short-circuit Knuth-Plass. The first build (cold cache)
    /// pays the full layout cost; subsequent mutation rebuilds only
    /// recompose the touched paragraph(s).
    /// Concept 3 — the export-time ONE-SHOT build: glyph side-channel
    /// ON, splice caches OFF. The splice caches (master-text / body-
    /// story emit deltas) re-append cached command RANGES, which would
    /// desync the glyph table's `command_index` parallelism; the
    /// export pays the full build cost instead (it happens once per
    /// export, not per gesture frame). The live canvas build and its
    /// caches are untouched. Proof simulation is deliberately ignored
    /// — export always renders the WORKING space.
    pub fn build_for_export(
        &self,
    ) -> Result<paged_renderer::BuiltDocument, crate::channel::LoadError> {
        let resolver = build_font_resolver(&self.font_registry, self.font_bytes.as_deref());
        let options = PipelineOptions {
            font: self.font_bytes.as_deref(),
            assets: resolver
                .as_ref()
                .map(|r| r as &dyn paged_renderer::AssetResolver),
            cmyk_icc_profile: self.icc_bytes.as_deref(),
            cmyk_intent: self.color_settings.intent,
            cmyk_bpc: self.color_settings.bpc,
            collect_glyph_runs: true,
            // Image decode cache is content-addressed (URI →
            // DecodedImage), not positional — safe to share.
            image_decode_cache: Some(&self.image_decode_cache),
            pre_built_font_table: Some(&self.font_table),
            ..PipelineOptions::default()
        };
        pipeline::build_document(&self.scene, &options)
            .map_err(|e| crate::channel::LoadError::Build(e.to_string()))
    }

    /// Concept 3 — read accessors for the export session's begin.
    pub fn font_table(&self) -> &paged_renderer::FontTable {
        &self.font_table
    }

    pub fn active_cmyk_profile(&self) -> Option<&[u8]> {
        self.icc_bytes.as_deref()
    }

    pub fn registered_profile(&self, name: &str) -> Option<&[u8]> {
        self.color_profiles.get(name).map(|b| b.as_slice())
    }

    pub fn color_settings_state(&self) -> &ColorSettingsState {
        &self.color_settings
    }

    pub fn ink_settings_map(&self) -> &std::collections::BTreeMap<String, InkSetting> {
        &self.ink_settings
    }

    pub fn use_standard_lab_for_spots(&self) -> bool {
        self.use_standard_lab_for_spots
    }

    pub fn document_preference(&self) -> paged_parse::DocumentPreference {
        self.scene.container.designmap.document_preference
    }

    pub fn palette(&self) -> &paged_parse::graphic::Graphic {
        &self.scene.palette
    }

    pub fn rebuild_after_mutation(&mut self) -> Result<(), crate::channel::LoadError> {
        let resolver = build_font_resolver(&self.font_registry, self.font_bytes.as_deref());
        let options = PipelineOptions {
            font: self.font_bytes.as_deref(),
            assets: resolver
                .as_ref()
                .map(|r| r as &dyn paged_renderer::AssetResolver),
            // Soft-proof active => CMYK renders through the PROOF
            // condition (paper white = absolute colorimetric);
            // otherwise the working space + document settings.
            cmyk_icc_profile: match &self.proof_state {
                Some(p) => Some(p.bytes.as_slice()),
                None => self.icc_bytes.as_deref(),
            },
            cmyk_intent: match &self.proof_state {
                Some(p) if p.simulate_paper_white => paged_color::Intent::AbsoluteColorimetric,
                Some(p) => p.intent,
                None => self.color_settings.intent,
            },
            cmyk_bpc: match &self.proof_state {
                // Paper-white simulation wants the true media white:
                // BPC would re-anchor the black point and dilute it.
                Some(p) => !p.simulate_paper_white && self.color_settings.bpc,
                None => self.color_settings.bpc,
            },
            // Perf-S — reuse the persistent image-decode cache so
            // placed images don't re-decode on every gesture rebuild.
            image_decode_cache: Some(&self.image_decode_cache),
            // Perf-FontTable — reuse the shaping table built at
            // load. Saves the ~225ms harvest+resolve walk that
            // FontTable::build does internally.
            pre_built_font_table: Some(&self.font_table),
            // Perf-MasterText — reuse the per-page master-text
            // emit deltas captured at load. Saves the ~161ms
            // emission walk for footers/headers across body pages.
            // `apply_operation` clears this when a structural
            // mutation lands so the next rebuild repopulates.
            master_text_emit_cache: Some(&self.master_text_emit_cache),
            // Perf-BodyStory — reuse the per-story body emit
            // deltas. Signature-keyed so stories whose frame
            // chain isn't affected by the active gesture keep
            // hitting; the dragged frame's story misses and
            // re-emits. Largest single perf opportunity in
            // build_document; ~613ms ceiling on a multi-spread
            // fixture.
            body_story_emit_cache: Some(&self.body_story_emit_cache),
            ..PipelineOptions::default()
        };
        let mut cache = std::mem::take(&mut self.layout_cache);
        cache.reset_stats();
        // W1.24 (audit B18) — time just the pipeline build (the
        // dominant cost; op-apply is staged separately by the caller).
        let t_build = phase_now();
        let (build_result, cache) = paged_text::cache::with_layout_cache(cache, || {
            pipeline::build_document(&self.scene, &options)
        });
        let build_ms = phase_elapsed_ms(t_build);
        self.layout_cache = cache;
        let built = build_result.map_err(|e| crate::channel::LoadError::Build(e.to_string()))?;
        self.page_index = built
            .pages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();
        self.story_pages = compute_story_pages(&built);
        // W1.24 (audit B18) — refresh the per-rebuild stats. `rebuilds`
        // is monotone; `op_apply_ms` consumes whatever a mutation
        // entrypoint staged (0 for a view-state rebuild). Done before
        // `self.built` is moved so the sizes read the fresh document.
        self.rebuild_stats = RebuildStats {
            build_ms,
            op_apply_ms: std::mem::take(&mut self.pending_op_apply_ms),
            pages: built.pages.len(),
            paragraphs: built.stats.paragraphs,
            rebuilds: self.rebuild_stats.rebuilds.saturating_add(1),
            // Snapshotted pre-push (the caller pushes after this
            // returns); `last_rebuild_stats` recomputes it live so the
            // reported value is the true post-mutation depth.
            applied_log_len: self.applied_log.len(),
        };
        self.built = built;
        Ok(())
    }

    /// W1.24 (audit B18) — stats for the most recent rebuild (or the
    /// initial `load`). Native callers (the editor's worker, tests) read
    /// this directly; the wasm dispatch folds the breakdown onto the wire
    /// `LayoutCacheStats` additively. Always populated.
    ///
    /// `applied_log_len` is read LIVE from the log here rather than from
    /// the value snapshotted during the rebuild: the forward-mutation
    /// paths push onto `applied_log` *after* `rebuild_after_mutation`
    /// returns, so the snapshot would always trail by one. Reading it at
    /// stats-read time reports the true post-mutation undo depth (and the
    /// B19 cap, which lives here).
    pub fn last_rebuild_stats(&self) -> RebuildStats {
        RebuildStats {
            applied_log_len: self.applied_log.len(),
            ..self.rebuild_stats
        }
    }

    /// W1.24 (audit B18) — stage the op-apply duration (ms) a mutation
    /// entrypoint measured for the scene edit that precedes its rebuild.
    /// Consumed (and zeroed) by the next `rebuild_after_mutation`. Kept
    /// `pub(crate)` so only the in-crate mutation paths feed it.
    pub(crate) fn stage_op_apply_ms(&mut self, ms: f64) {
        self.pending_op_apply_ms = ms;
    }

    /// W1.24 (audit B19) — push an `AppliedRecord` onto the undo log,
    /// enforcing the `MAX_APPLIED_LOG` cap. Evicts from the FRONT
    /// (oldest first) so the freshest `MAX_APPLIED_LOG` mutations stay
    /// undoable. Every forward-mutation path (`apply_mutation` /
    /// `apply_operation` / `redo`) routes its push here so the cap is
    /// enforced in exactly one place. `undo` does NOT use this — it
    /// `pop`s (shrinks) the log — and the redo stack is a transient of
    /// in-session undo, bounded by the same cap, so it needs no
    /// separate cap.
    fn push_applied(&mut self, rec: AppliedRecord) {
        self.applied_log.push(rec);
        if self.applied_log.len() > MAX_APPLIED_LOG {
            // O(n) on the eviction only; eviction happens at most once
            // per push past the cap, so amortised O(1). A VecDeque would
            // make the front-pop O(1) but `applied_log` is read as a
            // slice elsewhere (`applied_log_back`); keep the Vec and pay
            // the rare shift.
            let overflow = self.applied_log.len() - MAX_APPLIED_LOG;
            self.applied_log.drain(0..overflow);
        }
    }

    /// Phase 4 instrumentation — last rebuild's layout cache stats.
    /// Hits / misses reflect the most recent `rebuild_after_mutation`
    /// (or initial `load`) so callers can verify incremental wins on
    /// a typing test.
    pub fn layout_cache_stats(&self) -> paged_text::CacheStats {
        self.layout_cache.stats()
    }

    /// Expose the inner built document for tests and the wasm
    /// renderer-on-demand path that needs to read display lists.
    pub fn built(&self) -> &BuiltDocument {
        &self.built
    }

    pub fn font_bytes(&self) -> Option<&[u8]> {
        self.font_bytes.as_deref()
    }

    pub fn icc_bytes(&self) -> Option<&[u8]> {
        self.icc_bytes.as_deref()
    }

    /// Concept 2 — add (or replace) a named ICC profile in the live
    /// registry. Post-load registrations become resolvable by the
    /// next `SetColorSettings`; they do NOT retroactively activate.
    pub fn register_color_profile(&mut self, name: String, bytes: Vec<u8>) {
        self.color_profiles.insert(name, bytes);
    }

    /// Concept 2 — the CMM matching the active colour settings,
    /// built lazily and cached until `SetColorSettings` changes the
    /// inputs.
    fn active_cmm(&self) -> std::rc::Rc<paged_color::IccCmm> {
        if let Some(cmm) = self.cmm_cache.borrow().as_ref() {
            return cmm.clone();
        }
        let cmm = std::rc::Rc::new(paged_color::IccCmm::new(
            self.icc_bytes.as_deref(),
            paged_color::DisplaySetup {
                intent: self.color_settings.intent,
                bpc: self.color_settings.bpc,
            },
        ));
        *self.cmm_cache.borrow_mut() = Some(cmm.clone());
        cmm
    }

    /// Concept 2 — resolve an ARBITRARY colour value (mixer slider
    /// state, not a swatch ref) through the active colour
    /// management. Returns display hex + the effective CMYK (when
    /// the value resolves through CMYK) + the gamut verdict.
    pub fn color_compute(
        &self,
        space: &str,
        value: &[f32],
        tint: Option<f32>,
        model: Option<&str>,
        alternate_space: Option<&str>,
        alternate_value: Option<&[f32]>,
    ) -> (String, Option<[f32; 4]>, bool) {
        use paged_parse::graphic::{ColorEntry, ColorModel, ColorSpace};
        let parse_space = |s: &str| match s {
            "CMYK" | "cmyk" => ColorSpace::Cmyk,
            "RGB" | "rgb" => ColorSpace::Rgb,
            "LAB" | "Lab" | "lab" => ColorSpace::Lab,
            "Gray" | "gray" | "GRAY" => ColorSpace::Gray,
            _ => ColorSpace::Unknown,
        };
        // Ephemeral ColorEntry so spot/tint folding reuses the
        // exact swatch semantics (`effective_cmyk`).
        let entry = ColorEntry {
            self_id: String::new(),
            name: None,
            space: parse_space(space),
            value: value.to_vec(),
            model: match model {
                Some("Spot" | "spot") => ColorModel::Spot,
                _ => ColorModel::Process,
            },
            alternate_space: alternate_space.map(parse_space),
            alternate_value: alternate_value.map(|v| v.to_vec()).unwrap_or_default(),
            tint,
            alpha: None,
        };
        let cmm = self.active_cmm();
        let working = working_color_of_with(&entry, self.use_standard_lab_for_spots);
        let rgb = match working {
            Some(w) => {
                use paged_color::Cmm as _;
                let paged_color::LinearRgb(rgb) = cmm.resolve_display(w);
                rgb
            }
            None => [0.5, 0.5, 0.5],
        };
        let out_of_gamut = match working {
            Some(w) => {
                use paged_color::Cmm as _;
                !matches!(cmm.check_gamut(w), paged_color::GamutStatus::InGamut)
            }
            None => false,
        };
        // `effective_cmyk` returns IDML percentages (0..=100).
        let cmyk = entry.effective_cmyk().map(|c| {
            [
                c[0].clamp(0.0, 100.0),
                c[1].clamp(0.0, 100.0),
                c[2].clamp(0.0, 100.0),
                c[3].clamp(0.0, 100.0),
            ]
        });
        (rgb_to_hex(rgb), cmyk, out_of_gamut)
    }
}

/// Phase 4 Step 3 — build `story_id → Vec<PageId>` from the freshly
/// built document by walking every page's `story_layout` and grouping
/// LineLayout entries by story. Pages preserve their order; each
/// story's `Vec<PageId>` is in first-appearance order without
/// duplicates.
/// Cheap-but-coarse perf instrumentation. On wasm32 we use
/// `js_sys::Date::now()` because std::time::Instant panics. On native
/// we use Instant. Output goes to `tracing::info!` (web console on
/// wasm via `tracing-subscriber`'s wasm hook) and to a `console.log`
/// fallback so the line is also visible in DevTools.
#[cfg(target_arch = "wasm32")]
fn phase_now() -> f64 {
    js_sys::Date::now()
}
#[cfg(not(target_arch = "wasm32"))]
fn phase_now() -> std::time::Instant {
    std::time::Instant::now()
}

#[cfg(target_arch = "wasm32")]
fn phase_log(label: &str, start: f64) {
    let ms = js_sys::Date::now() - start;
    web_sys::console::log_1(&format!("[paged-canvas perf] {label}: {ms:.0} ms").into());
}
#[cfg(not(target_arch = "wasm32"))]
fn phase_log(label: &str, start: std::time::Instant) {
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    tracing::info!("[paged-canvas perf] {label}: {ms:.0} ms");
}

/// W1.24 (audit B18) — milliseconds elapsed since `start`, read from the
/// same monotone source as `phase_log` but RETURNED (not logged) so the
/// `RebuildStats` capture can store it. Wasm uses `js_sys::Date::now`
/// (sub-ms resolution is fine for HUD-grade timing); native uses the
/// monotonic `Instant`. Never panics on `wasm32`.
#[cfg(target_arch = "wasm32")]
fn phase_elapsed_ms(start: f64) -> f64 {
    js_sys::Date::now() - start
}
#[cfg(not(target_arch = "wasm32"))]
fn phase_elapsed_ms(start: std::time::Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// Build a `BytesResolver` from a font registry. Returns `None` when
/// the registry is empty AND no default font is provided — the
/// pipeline already handles `assets: None` cleanly, so we save the
/// allocation in the common single-font dev path.
/// Concept 2 — adapt a parsed swatch to the CMM's input. Spot
/// alternates + swatch-level tints fold via `effective_cmyk` (the
/// exact swatch semantics the renderer uses); non-CMYK spaces map
/// directly. `None` = unresolvable (e.g. a spot with no CMYK
/// alternate and a non-Lab primary).
fn working_color_of(entry: &paged_parse::graphic::ColorEntry) -> Option<paged_color::WorkingColor> {
    working_color_of_with(entry, false)
}

/// Like [`working_color_of`] but honouring the Ink Manager's "Use
/// Standard Lab Values for Spots": a spot whose PRIMARY space is
/// Lab resolves device-independently instead of via its CMYK
/// alternate.
fn working_color_of_with(
    entry: &paged_parse::graphic::ColorEntry,
    use_standard_lab_for_spots: bool,
) -> Option<paged_color::WorkingColor> {
    use paged_parse::graphic::ColorSpace;
    if use_standard_lab_for_spots
        && entry.model == paged_parse::graphic::ColorModel::Spot
        && entry.space == ColorSpace::Lab
        && entry.value.len() == 3
    {
        // Swatch-level tint scales toward paper white in Lab: only
        // L* lightens, chroma fades proportionally.
        let t = entry
            .tint
            .map(|v| (v / 100.0).clamp(0.0, 1.0))
            .unwrap_or(1.0);
        return Some(paged_color::WorkingColor::Lab {
            l: 100.0 - (100.0 - entry.value[0]) * t,
            a: entry.value[1] * t,
            b: entry.value[2] * t,
        });
    }
    if let Some([c, m, y, k]) = entry.effective_cmyk() {
        return Some(paged_color::WorkingColor::Cmyk(paged_color::Cmyk {
            c,
            m,
            y,
            k,
        }));
    }
    match entry.space {
        ColorSpace::Rgb if entry.value.len() == 3 => Some(paged_color::WorkingColor::Rgb([
            entry.value[0] / 255.0,
            entry.value[1] / 255.0,
            entry.value[2] / 255.0,
        ])),
        ColorSpace::Lab if entry.value.len() == 3 => Some(paged_color::WorkingColor::Lab {
            l: entry.value[0],
            a: entry.value[1],
            b: entry.value[2],
        }),
        ColorSpace::Gray if entry.value.len() == 1 => {
            Some(paged_color::WorkingColor::Gray(entry.value[0]))
        }
        _ => None,
    }
}

/// Linear RGB → `#rrggbb` (sRGB-encoded, the wire convention).
fn rgb_to_hex(rgb: [f32; 3]) -> String {
    let to_byte = |v: f32| -> u8 {
        let s = if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (s.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        to_byte(rgb[0]),
        to_byte(rgb[1]),
        to_byte(rgb[2])
    )
}

fn build_font_resolver(
    registry: &[FontEntry],
    default_font: Option<&[u8]>,
) -> Option<BytesResolver> {
    if registry.is_empty() && default_font.is_none() {
        return None;
    }
    let mut r = BytesResolver::new();
    for entry in registry {
        r.add_font(&entry.family, entry.style.as_deref(), entry.bytes.clone());
    }
    if let Some(bytes) = default_font {
        r.default_font = Some(bytes.to_vec().into());
    }
    Some(r)
}

fn compute_story_pages(built: &BuiltDocument) -> HashMap<String, Vec<PageId>> {
    let mut out: HashMap<String, Vec<PageId>> = HashMap::new();
    for page in &built.pages {
        for line in &page.story_layout {
            let entry = out.entry(line.story_id.clone()).or_default();
            if entry.last().map(|p| p != &page.id).unwrap_or(true) {
                entry.push(page.id.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimum-viable IDML the canvas can load. Hand-rolled so the
    // model test stays independent of the heavier `paged-gen` fixture
    // generator. Single Letter-sized page, no stories, no styles —
    // just the package files `Document::open` needs to parse.
    fn minimal_idml_bytes() -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            // mimetype must be the first file, stored uncompressed.
            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();

            // META-INF/container.xml
            zip.start_file("META-INF/container.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
            )
            .unwrap();

            // designmap.xml — references one spread.
            zip.start_file("designmap.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<?aid style="50" type="document" readerVersion="13.0" featureSet="513" product="13.1(255)"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
            )
            .unwrap();

            // Spreads/Spread_s1.xml — one Letter-sized page.
            zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
            )
            .unwrap();

            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn load_minimal_document_produces_one_page_with_stable_id() {
        let bytes = minimal_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default())
            .expect("minimal IDML parses + builds");
        assert_eq!(model.page_count(), 1);
        let ids: Vec<PageId> = model.page_ids().cloned().collect();
        assert_eq!(ids.len(), 1);
        // The page carried Self="p1" in the IDML — the renderer
        // surfaces that directly as PageId. If parsing falls back to
        // a synthetic id, the spec contract is broken.
        assert_eq!(ids[0].as_str(), "p1");
        // Display list seam is reachable.
        let list = model
            .display_list_for_page(&ids[0])
            .expect("page exists, display list returns Some");
        // No stories or frames yet => no commands. Just confirm we
        // returned a borrow on the in-place list, not a clone.
        assert!(list.commands.is_empty());
    }

    #[test]
    fn handle_exposes_page_dimensions() {
        let bytes = minimal_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        let handle = model.handle();
        assert_eq!(handle.page_count, 1);
        assert_eq!(handle.page_sizes_pt.len(), 1);
        let (w, h) = handle.page_sizes_pt[0];
        assert!((w - 612.0).abs() < 0.01, "expected Letter width, got {w}");
        assert!((h - 792.0).abs() < 0.01, "expected Letter height, got {h}");
    }

    #[test]
    fn unknown_page_id_returns_none() {
        let bytes = minimal_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        assert!(model.page(&PageId("does-not-exist".into())).is_none());
        assert!(model
            .display_list_for_page(&PageId("nope".into()))
            .is_none());
    }

    #[test]
    fn canonical_hash_is_stable_across_loads() {
        let bytes = minimal_idml_bytes();
        let a = CanvasModel::load("a", &bytes, CanvasOptions::default()).unwrap();
        let b = CanvasModel::load("b", &bytes, CanvasOptions::default()).unwrap();
        assert_eq!(
            a.initial_state_hash(),
            b.initial_state_hash(),
            "same bytes → same canonical hash (doc_id is not part of content)"
        );
        assert_eq!(a.initial_state_hash(), a.current_state_hash());
    }

    /// panels.md gaps 9/10/19 — a fixture with two `<Section>`s,
    /// per-page `<MarginPreference>`, document bleed, and a placed
    /// image link carrying colour space + ppi. Drives the W0.6 panel
    /// summary accessors.
    fn panels_idml_bytes() -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();
            zip.start_file("META-INF/container.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
            )
            .unwrap();
            // Two sections: A- prefix (lowerRoman) at p1, plain arabic at p2.
            zip.start_file("designmap.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<DocumentPreference DocumentBleedTopOffset="9" DocumentBleedBottomOffset="9" DocumentBleedInsideOrLeftOffset="9" DocumentBleedOutsideOrRightOffset="9"/>
<Section Self="sec1" PageStart="p1" PageNumberStyle="LowerRoman" SectionPrefix="A-" IncludeSectionPrefix="true"/>
<Section Self="sec2" PageStart="p2" PageNumberStyle="Arabic"/>
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
            )
            .unwrap();
            zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="2">
<Page Self="p1" Name="i" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0">
<MarginPreference Top="36" Bottom="48" Left="54" Right="54" ColumnCount="2" ColumnGutter="12"/>
</Page>
<Page Self="p2" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 700 0"/>
<Rectangle Self="r1" GeometricBounds="0 0 100 100" ItemTransform="1 0 0 1 50 50">
<Image Self="img1" Space="$ID/CMYK" ActualPpi="(300 300)" EffectivePpi="(150 150)" LinkResourceURI="file:///missing.tif"/>
</Rectangle>
</Spread></idPkg:Spread>"#,
            )
            .unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn sections_summary_reports_prefix_style_and_page_ranges() {
        use crate::channel::CollectionName;
        let bytes = panels_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        let sections = model.sections();
        assert_eq!(sections.len(), 2);
        // First section: A- prefix, lowerRoman, starts at page index 0,
        // spans 1 page (up to the next section's start).
        assert_eq!(sections[0].self_id, "sec1");
        assert_eq!(sections[0].prefix, "A-");
        assert_eq!(sections[0].label_style, "lowerRoman");
        assert_eq!(sections[0].start_page_index, Some(0));
        assert_eq!(sections[0].page_count, 1);
        // Second section: no prefix, arabic, page index 1 to doc end.
        assert_eq!(sections[1].prefix, "");
        assert_eq!(sections[1].label_style, "arabic");
        assert_eq!(sections[1].start_page_index, Some(1));
        assert_eq!(sections[1].page_count, 1);
        // The generic dispatcher routes "sections" to the same shape.
        let via_named = serde_json::to_value(model.sections()).unwrap();
        let via_dispatch = model.collection(CollectionName::Sections);
        assert_eq!(via_named, via_dispatch);
    }

    #[test]
    fn page_summary_carries_margins_columns_and_bleed() {
        let bytes = panels_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        let pages = model.pages();
        assert_eq!(pages.len(), 2);
        let p1 = &pages[0];
        assert!((p1.margin_top_pt - 36.0).abs() < 1e-3);
        assert!((p1.margin_bottom_pt - 48.0).abs() < 1e-3);
        assert!((p1.margin_left_pt - 54.0).abs() < 1e-3);
        assert!((p1.margin_right_pt - 54.0).abs() < 1e-3);
        assert_eq!(p1.column_count, 2);
        assert!((p1.column_gutter_pt - 12.0).abs() < 1e-3);
        // Bleed is document-level: present on every page.
        assert!((p1.bleed_top_pt - 9.0).abs() < 1e-3);
        assert!((p1.bleed_right_pt - 9.0).abs() < 1e-3);
        // p2 declared no margins → all 0, column_count defaults to 1.
        let p2 = &pages[1];
        assert_eq!(p2.margin_top_pt, 0.0);
        assert_eq!(p2.column_count, 1);
        assert!((p2.bleed_bottom_pt - 9.0).abs() < 1e-3);
    }

    #[test]
    fn overset_story_is_flagged_on_summary_and_stats() {
        use std::io::Write;
        let font_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus/fonts/Inter.ttf");
        let Ok(font) = std::fs::read(&font_path) else {
            // Font fixture absent in this checkout — skip rather than
            // fail (mirrors the corpus-optional convention).
            eprintln!("skip: Inter.ttf not present");
            return;
        };
        let mut buf = Vec::new();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#).unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><Document Self="d1"><idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/><idPkg:Story src="Stories/Story_u10.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/></Document>"#).unwrap();
        // A 40pt-tall frame can't hold 12 lines → overset.
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Spread Self="s1"><Page Self="p1" GeometricBounds="0 0 800 400"/><TextFrame Self="f1" ParentStory="u10" GeometricBounds="20 20 60 200"/></Spread></idPkg:Spread>"#).unwrap();
        let mut story = String::from(
            r#"<?xml version="1.0"?><idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Story Self="u10">"#,
        );
        for i in 0..12 {
            story.push_str(&format!(r#"<ParagraphStyleRange><CharacterStyleRange AppliedFont="Inter" PointSize="10"><Content>Line {i}</Content></CharacterStyleRange></ParagraphStyleRange>"#));
        }
        story.push_str("</Story></idPkg:Story>");
        zip.start_file("Stories/Story_u10.xml", opts).unwrap();
        zip.write_all(story.as_bytes()).unwrap();
        zip.finish().unwrap();
        let bytes = buf;

        let opts = CanvasOptions {
            fonts: vec![font],
            ..Default::default()
        };
        let model = CanvasModel::load("doc-overset", &bytes, opts).unwrap();
        // The story overflowed its single short frame → overset.
        let story = model
            .stories()
            .into_iter()
            .find(|s| s.self_id == "u10")
            .expect("story u10");
        assert!(story.overset, "u10 should be flagged overset");
        // Document stats count it.
        assert_eq!(model.handle().stats.overset_stories, 1);
    }

    #[test]
    fn font_summary_flags_unregistered_families_missing() {
        let bytes = panels_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        // The panels fixture has no text, so no font references — but
        // the simple text fixture does. Re-load one with a run that
        // names "Inter", registered with no matching face.
        let _ = model; // panels fixture has no fonts; covered below.
        let text_bytes = {
            use std::io::Write;
            let mut buf = Vec::new();
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();
            zip.start_file("META-INF/container.xml", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#).unwrap();
            zip.start_file("designmap.xml", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0"?><Document Self="d1"><idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/><idPkg:Story src="Stories/Story_u10.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/></Document>"#).unwrap();
            zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0"?><idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Spread Self="s1"><Page Self="p1" GeometricBounds="0 0 792 612"/><TextFrame Self="f1" ParentStory="u10" GeometricBounds="40 40 700 500"/></Spread></idPkg:Spread>"#).unwrap();
            zip.start_file("Stories/Story_u10.xml", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0"?><idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Story Self="u10"><ParagraphStyleRange><CharacterStyleRange AppliedFont="Nonexistent Face" PointSize="12"><Content>Hi</Content></CharacterStyleRange></ParagraphStyleRange></Story></idPkg:Story>"#).unwrap();
            zip.finish().unwrap();
            buf
        };
        let model = CanvasModel::load("doc-2", &text_bytes, CanvasOptions::default()).unwrap();
        let fonts = model.fonts();
        let f = fonts
            .iter()
            .find(|f| f.family == "Nonexistent Face")
            .expect("font row");
        assert!(f.is_missing, "unregistered family must be flagged missing");
    }

    #[test]
    fn link_summary_carries_status_colorspace_and_ppi() {
        let bytes = panels_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        let links = model.links();
        assert_eq!(links.len(), 1);
        let l = &links[0];
        assert_eq!(l.host_self_id, "r1");
        assert_eq!(l.colorspace.as_deref(), Some("CMYK"));
        assert!((l.effective_ppi.unwrap() - 150.0).abs() < 1e-3);
        // The link points at a non-existent asset and no resolver was
        // configured, so the build drew the placeholder → "missing".
        assert_eq!(l.status, "missing");
    }

    /// SDK Phase 5 (D1) — the generic `collection(name)` dispatcher
    /// returns the same JSON shape the named accessor would, so
    /// callers can switch from `paged.swatches()` to
    /// `paged.collection("swatches")` without re-decoding.
    #[test]
    fn collection_dispatch_matches_named_accessor() {
        use crate::channel::CollectionName;
        let bytes = minimal_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        // Swatches: the named accessor returns the IDML defaults
        // (None / Paper / Black / Registration); the dispatcher must
        // serialize to the same Value::Array.
        let via_named = serde_json::to_value(model.swatches()).unwrap();
        let via_dispatch = model.collection(CollectionName::Swatches);
        assert_eq!(via_named, via_dispatch);

        // Layers: the minimal fixture has no `<Layer>` rows; both
        // return an empty array.
        let via_named = serde_json::to_value(model.layers()).unwrap();
        let via_dispatch = model.collection(CollectionName::Layers);
        assert_eq!(via_named, via_dispatch);

        // ParagraphStyles: dispatcher matches the typed accessor.
        let via_named = serde_json::to_value(model.paragraph_styles()).unwrap();
        let via_dispatch = model.collection(CollectionName::ParagraphStyles);
        assert_eq!(via_named, via_dispatch);
    }

    /// SDK Phase 5 (D1) — unwired collections (§5.1 enum entries
    /// without a backing accessor) return an empty array — never
    /// null — so consumers' typed `useCollection<T>` arrays stay
    /// valid.
    #[test]
    fn collection_dispatch_empty_array_for_unwired_collections() {
        use crate::channel::CollectionName;
        let bytes = minimal_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        for name in [
            CollectionName::ObjectStyles,
            CollectionName::CellStyles,
            CollectionName::TableStyles,
            CollectionName::Links,
            CollectionName::Articles,
            CollectionName::Hyperlinks,
            CollectionName::Bookmarks,
            CollectionName::Conditions,
            CollectionName::Fonts,
        ] {
            let v = model.collection(name);
            assert_eq!(v, serde_json::Value::Array(Vec::new()), "{:?}", name);
        }
    }

    /// SDK Phase 5 (D1) — `document_meta()` reports page count;
    /// other fields stay at their v1 defaults.
    #[test]
    fn document_meta_reports_page_count() {
        let bytes = minimal_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        let meta = model.document_meta();
        assert_eq!(meta.page_count, 1);
        assert_eq!(meta.active_page, None);
        assert!(meta.units.is_empty());
        assert!(meta.color_mode.is_empty());
        assert!(meta.document_name.is_empty());
        assert!(!meta.dirty);
    }

    #[test]
    fn applied_seq_starts_at_zero_and_bumps() {
        let bytes = minimal_idml_bytes();
        let mut m = CanvasModel::load("a", &bytes, CanvasOptions::default()).unwrap();
        assert_eq!(m.last_applied_seq(), 0);
        assert_eq!(m.bump_applied_seq(), 1);
        assert_eq!(m.bump_applied_seq(), 2);
        assert_eq!(m.last_applied_seq(), 2);
    }

    /// An IDML whose story carries two runs in the same family
    /// ("Open Sans") with distinct `FontStyle`s ("Regular", "Bold"),
    /// so `fonts()` can surface the styles-per-family list.
    fn idml_with_font_styles() -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();
            zip.start_file("META-INF/container.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
            )
            .unwrap();
            zip.start_file("designmap.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_st1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
            )
            .unwrap();
            zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="st1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
            )
            .unwrap();
            zip.start_file("Stories/Story_st1.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="st1">
<ParagraphStyleRange>
<CharacterStyleRange AppliedFont="Open Sans" FontStyle="Regular"><Content>Hello </Content></CharacterStyleRange>
<CharacterStyleRange AppliedFont="Open Sans" FontStyle="Bold"><Content>world</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
            )
            .unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn fonts_reports_styles_per_family() {
        let bytes = idml_with_font_styles();
        let model = CanvasModel::load("doc-fonts", &bytes, CanvasOptions::default()).unwrap();
        let fonts = model.fonts();
        let open_sans = fonts
            .iter()
            .find(|f| f.family == "Open Sans")
            .expect("Open Sans family present");
        // Both runs reference the family → reference_count 2.
        assert_eq!(open_sans.reference_count, 2);
        // The styles list is the deduped, sorted set of FontStyles seen.
        assert_eq!(
            open_sans.styles,
            vec!["Bold".to_string(), "Regular".to_string()]
        );
        // No font registered → missing, but styles still populate from
        // the document content (the honest "styles in use" source).
        assert!(open_sans.is_missing);
    }

    // ── W1.22 (engine gap 22) — numbering-list read surface + next-style ──

    /// Minimal IDML carrying a `<NumberingList>` resource and a
    /// paragraph style with `NextStyle`, so the canvas read surfaces
    /// can be exercised.
    fn numbering_list_idml_bytes() -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();
            zip.start_file("designmap.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Styles src="Resources/Styles.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
            )
            .unwrap();
            zip.start_file("Resources/Styles.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<RootNumberingListGroup>
<NumberingList Self="NumberingList/Shared" Name="Shared" ContinueNumbersAcrossStories="true"/>
</RootNumberingListGroup>
<RootParagraphStyleGroup>
<ParagraphStyle Self="ParagraphStyle/Head" Name="Head" NextStyle="ParagraphStyle/Body"/>
<ParagraphStyle Self="ParagraphStyle/Body" Name="Body"/>
</RootParagraphStyleGroup>
</idPkg:Styles>"#,
            )
            .unwrap();
            zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
            )
            .unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn numbering_lists_collection_reports_continuity_flags() {
        use crate::channel::CollectionName;
        let bytes = numbering_list_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        let lists = model.numbering_lists();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].self_id, "NumberingList/Shared");
        assert_eq!(lists[0].name, "Shared");
        assert!(lists[0].continue_across_stories);
        assert!(!lists[0].continue_across_documents);
        // The generic dispatcher routes "numberingLists" to the same shape.
        let via_named = serde_json::to_value(model.numbering_lists()).unwrap();
        let via_dispatch = model.collection(CollectionName::NumberingLists);
        assert_eq!(via_named, via_dispatch);
        // Round-trips through the CollectionName string form too.
        assert_eq!(
            CollectionName::from_str("numberingLists"),
            Some(CollectionName::NumberingLists)
        );
    }

    #[test]
    fn paragraph_style_summary_exposes_next_style() {
        let bytes = numbering_list_idml_bytes();
        let model = CanvasModel::load("doc-1", &bytes, CanvasOptions::default()).unwrap();
        let styles = model.paragraph_styles();
        let head = styles
            .iter()
            .find(|s| s.self_id == "ParagraphStyle/Head")
            .expect("Head style present");
        assert_eq!(head.next_style.as_deref(), Some("ParagraphStyle/Body"));
        let body = styles
            .iter()
            .find(|s| s.self_id == "ParagraphStyle/Body")
            .expect("Body style present");
        assert_eq!(body.next_style, None, "Body declares no NextStyle");
    }

    // ---- W1.24 (audit B19) — applied_log cap ----------------------------

    /// A throwaway `AppliedRecord` carrying its `applied_seq` as an
    /// identity tag. The `LoggedMutation::Text` payload is empty so the
    /// push is allocation-cheap — this lets us drive the log 10k+ entries
    /// past the cap in microseconds (no rebuilds), exercising the
    /// eviction logic directly rather than through `apply_mutation`.
    fn dummy_record(seq: u64) -> AppliedRecord {
        AppliedRecord {
            applied_seq: seq,
            kind: LoggedMutation::Text {
                op: crate::mutate::TextOp::InsertText {
                    story_id: String::new(),
                    offset: 0,
                    text: String::new(),
                    cell: None,
                },
                inverse: crate::mutate::TextOp::DeleteRange {
                    story_id: String::new(),
                    start: 0,
                    end: 0,
                    recovered: String::new(),
                    cell: None,
                },
            },
        }
    }

    #[test]
    fn applied_log_caps_at_max_and_evicts_oldest_first() {
        let bytes = minimal_idml_bytes();
        let mut model = CanvasModel::load("doc-cap", &bytes, CanvasOptions::default()).unwrap();

        let overflow = 7usize;
        let total = MAX_APPLIED_LOG + overflow;
        // applied_seq is 1-based and increases by one per push.
        for seq in 1..=total as u64 {
            model.push_applied(dummy_record(seq));
            // Invariant at EVERY step, not just the end.
            assert!(
                model.applied_log.len() <= MAX_APPLIED_LOG,
                "log exceeded cap at push {seq}: {}",
                model.applied_log.len()
            );
        }

        // Saturated exactly at the cap.
        assert_eq!(model.applied_log.len(), MAX_APPLIED_LOG);
        // Oldest-first eviction: the first `overflow` seqs (1..=overflow)
        // were dropped, so the FRONT now holds seq == overflow + 1 and
        // the BACK holds the freshest seq == total.
        assert_eq!(
            model.applied_log.first().unwrap().applied_seq,
            overflow as u64 + 1,
            "oldest survivor must be seq overflow+1 (older ones evicted)"
        );
        assert_eq!(
            model.applied_log.last().unwrap().applied_seq,
            total as u64,
            "freshest entry must always be retained"
        );
        // And the surviving window is exactly the freshest MAX entries:
        // contiguous seqs [overflow+1 ..= total].
        for (i, rec) in model.applied_log.iter().enumerate() {
            assert_eq!(rec.applied_seq, overflow as u64 + 1 + i as u64);
        }
    }

    #[test]
    fn push_under_cap_never_evicts() {
        let bytes = minimal_idml_bytes();
        let mut model = CanvasModel::load("doc-under", &bytes, CanvasOptions::default()).unwrap();
        for seq in 1..=100u64 {
            model.push_applied(dummy_record(seq));
        }
        assert_eq!(model.applied_log.len(), 100, "no eviction under the cap");
        assert_eq!(
            model.applied_log.first().unwrap().applied_seq,
            1,
            "seq 1 kept"
        );
        assert_eq!(model.applied_log.last().unwrap().applied_seq, 100);
    }
}
