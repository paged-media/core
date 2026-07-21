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
use crate::invert::invert_batch;
use crate::operation::{
    AppliedOperation, ColorGroupSpec, GradientSpec, GradientStopSpec, GroupSpec, InvalidationHint,
    NodeId, NumberingListSpec, Operation, PropertyPath, StyleCollection, SwatchSpec, Value,
};

// ---------------------------------------------------------------------------
// Track M — structural layer ops
// ---------------------------------------------------------------------------

pub(super) fn apply_move_layer(
    doc: &mut Document,
    layer_id: &str,
    new_index: usize,
) -> Result<AppliedOperation, OperationError> {
    let layers = &mut doc.designmap.layers;
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

pub(super) fn apply_insert_layer(
    doc: &mut Document,
    position: usize,
    name: &str,
    requested_self_id: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let layers = &mut doc.designmap.layers;
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

pub(super) fn apply_remove_layer(
    doc: &mut Document,
    layer_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let layers = &mut doc.designmap.layers;
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
pub(super) fn color_entry_from_spec(self_id: String, spec: &SwatchSpec) -> paged_parse::ColorEntry {
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
pub(super) fn swatch_spec_from_entry(entry: &paged_parse::ColorEntry) -> SwatchSpec {
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

pub(super) fn apply_create_swatch(
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

pub(super) fn apply_edit_swatch(
    doc: &mut Document,
    swatch_id: &str,
    spec: &SwatchSpec,
) -> Result<AppliedOperation, OperationError> {
    let colors = &mut doc.palette.colors;
    let existing =
        colors
            .get(swatch_id)
            .ok_or_else(|| OperationError::CollectionEntryNotFound {
                collection: "swatch".to_string(),
                id: swatch_id.to_string(),
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

pub(super) fn apply_delete_swatch(
    doc: &mut Document,
    swatch_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let colors = &mut doc.palette.colors;
    let captured =
        colors
            .remove(swatch_id)
            .ok_or_else(|| OperationError::CollectionEntryNotFound {
                collection: "swatch".to_string(),
                id: swatch_id.to_string(),
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

pub(super) fn gradient_kind_from_attr(s: &str) -> paged_parse::GradientKind {
    match s {
        "Linear" => paged_parse::GradientKind::Linear,
        "Radial" => paged_parse::GradientKind::Radial,
        _ => paged_parse::GradientKind::Unknown,
    }
}

pub(super) fn gradient_kind_as_attr(k: paged_parse::GradientKind) -> &'static str {
    match k {
        paged_parse::GradientKind::Linear => "Linear",
        paged_parse::GradientKind::Radial => "Radial",
        paged_parse::GradientKind::Unknown => "Unknown",
    }
}

pub(super) fn gradient_entry_from_spec(
    self_id: String,
    spec: &GradientSpec,
) -> paged_parse::GradientEntry {
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

pub(super) fn gradient_spec_from_entry(entry: &paged_parse::GradientEntry) -> GradientSpec {
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

/// B-04 — mint a page-item id (`u<hex>`) unique across every page
/// item in the document, groups included. Mutate-side twin of the
/// canvas `mint_page_item_id_with_offset` scanner.
pub(super) fn mint_group_id(doc: &paged_scene::Document) -> String {
    fn scan(max: &mut u64, id: Option<&str>) {
        if let Some(rest) = id.and_then(|s| s.strip_prefix('u')) {
            if let Ok(n) = u64::from_str_radix(rest, 16) {
                *max = (*max).max(n);
            }
        }
    }
    let mut max: u64 = 0;
    for parsed in &doc.spreads {
        let s = &parsed.spread;
        for f in &s.text_frames {
            scan(&mut max, f.self_id.as_deref());
        }
        for r in &s.rectangles {
            scan(&mut max, r.self_id.as_deref());
        }
        for o in &s.ovals {
            scan(&mut max, o.self_id.as_deref());
        }
        for l in &s.graphic_lines {
            scan(&mut max, l.self_id.as_deref());
        }
        for p in &s.polygons {
            scan(&mut max, p.self_id.as_deref());
        }
        for g in &s.groups {
            scan(&mut max, g.self_id.as_deref());
        }
    }
    format!("u{:x}", max + 1)
}

/// Resolve a leaf-member NodeId to its `FrameRef` within `spread`.
pub(super) fn leaf_frame_ref(
    spread: &paged_parse::Spread,
    node: &NodeId,
) -> Option<paged_parse::FrameRef> {
    use paged_parse::FrameRef;
    let find = |id: &str, ids: Vec<Option<&str>>| -> Option<usize> {
        ids.iter().position(|s| *s == Some(id))
    };
    match node {
        NodeId::TextFrame(id) => find(
            id,
            spread
                .text_frames
                .iter()
                .map(|f| f.self_id.as_deref())
                .collect(),
        )
        .map(FrameRef::TextFrame),
        NodeId::Rectangle(id) => find(
            id,
            spread
                .rectangles
                .iter()
                .map(|f| f.self_id.as_deref())
                .collect(),
        )
        .map(FrameRef::Rectangle),
        NodeId::Oval(id) => find(
            id,
            spread.ovals.iter().map(|f| f.self_id.as_deref()).collect(),
        )
        .map(FrameRef::Oval),
        NodeId::GraphicLine(id) => find(
            id,
            spread
                .graphic_lines
                .iter()
                .map(|f| f.self_id.as_deref())
                .collect(),
        )
        .map(FrameRef::GraphicLine),
        NodeId::Polygon(id) => find(
            id,
            spread
                .polygons
                .iter()
                .map(|f| f.self_id.as_deref())
                .collect(),
        )
        .map(FrameRef::Polygon),
        _ => None,
    }
}

/// W1.20 (groups v2) — resolve a member NodeId to its `FrameRef`,
/// extending `leaf_frame_ref` with `NodeId::Group` so `createGroup`
/// can nest an existing group (group-of-groups).
pub(super) fn member_frame_ref(
    spread: &paged_parse::Spread,
    node: &NodeId,
) -> Option<paged_parse::FrameRef> {
    use paged_parse::FrameRef;
    match node {
        NodeId::Group(id) => spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(id.as_str()))
            .map(FrameRef::Group),
        other => leaf_frame_ref(spread, other),
    }
}

/// W1.20 — map a `FrameRef` back to the `NodeId` of the page item it
/// addresses (leaf shapes AND `Group`s, unlike the leaf-only inline
/// resolver `apply_dissolve_group` used in v1). Returns `None` for an
/// id-less frame.
pub(super) fn node_for_frame_ref(
    spread: &paged_parse::Spread,
    r: paged_parse::FrameRef,
) -> Option<NodeId> {
    use paged_parse::FrameRef;
    Some(match r {
        FrameRef::TextFrame(i) => NodeId::TextFrame(spread.text_frames.get(i)?.self_id.clone()?),
        FrameRef::Rectangle(i) => NodeId::Rectangle(spread.rectangles.get(i)?.self_id.clone()?),
        FrameRef::Oval(i) => NodeId::Oval(spread.ovals.get(i)?.self_id.clone()?),
        FrameRef::GraphicLine(i) => {
            NodeId::GraphicLine(spread.graphic_lines.get(i)?.self_id.clone()?)
        }
        FrameRef::Polygon(i) => NodeId::Polygon(spread.polygons.get(i)?.self_id.clone()?),
        FrameRef::Group(i) => NodeId::Group(spread.groups.get(i)?.self_id.clone()?),
    })
}

/// Plugin-metadata write cap: keeps documents loadable and the Label
/// mechanism friendly to other IDML consumers (facility design §2).
pub(super) const PLUGIN_METADATA_MAX_BYTES: usize = 64 * 1024;

/// Locate the spread holding a leaf page item by NodeId. Returns the
/// spread index so the caller can borrow mutably afterwards.
pub(super) fn find_spread_for_leaf(doc: &Document, node: &NodeId) -> Option<usize> {
    fn has<'a>(mut ids: impl Iterator<Item = Option<&'a str>>, id: &str) -> bool {
        ids.any(|s| s == Some(id))
    }
    for (si, parsed) in doc.spreads.iter().enumerate() {
        let s = &parsed.spread;
        let found = match node {
            NodeId::TextFrame(id) => has(s.text_frames.iter().map(|f| f.self_id.as_deref()), id),
            NodeId::Rectangle(id) => has(s.rectangles.iter().map(|f| f.self_id.as_deref()), id),
            NodeId::Oval(id) => has(s.ovals.iter().map(|f| f.self_id.as_deref()), id),
            NodeId::GraphicLine(id) => {
                has(s.graphic_lines.iter().map(|f| f.self_id.as_deref()), id)
            }
            NodeId::Polygon(id) => has(s.polygons.iter().map(|f| f.self_id.as_deref()), id),
            _ => false,
        };
        if found {
            return Some(si);
        }
    }
    None
}

/// Plugin-metadata carrier (decision 9 facility) — set / replace /
/// delete one Label `KeyValuePair` in the reserved `x-paged:`
/// namespace. Gates BEFORE any mutation: key prefix, 64 KiB cap, and
/// the JSON envelope `{ v: number >= 1, data: object, … }`. The
/// inverse carries the prev snapshot so undo restores exactly
/// (including "was absent").
pub(super) fn apply_plugin_metadata(
    doc: &mut Document,
    node: &NodeId,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    let invalid = |reason: String| OperationError::InvalidValue {
        node: node.clone(),
        path: PropertyPath::PluginMetadata,
        reason,
    };
    let Value::PluginMetadata {
        key,
        value: new_value,
        caller,
        ..
    } = value
    else {
        return Err(invalid("expected Value::PluginMetadata".into()));
    };
    if !key.starts_with("x-paged:") || key.len() <= "x-paged:".len() {
        return Err(invalid(format!(
            "metadata keys live in the reserved namespace: expected \"x-paged:<plugin>\", got \"{key}\""
        )));
    }
    // B-16 — caller-identity gate (additive). When the request names a
    // calling plugin, the engine enforces that the key lives in THAT
    // plugin's namespace, mirroring the SDK door's `foreignMetadataKey`.
    // A bundle holding the raw handle can no longer write another
    // plugin's `x-paged:<other>` by passing `caller`. `None` (the
    // editor / pre-B-16 callers) keeps the prior behaviour.
    if let Some(caller) = caller {
        let own = format!("x-paged:{caller}");
        if key != &own {
            return Err(invalid(format!(
                "caller \"{caller}\" may only write its own namespace \"{own}\", not \"{key}\""
            )));
        }
    }
    if let Some(v) = new_value {
        if v.len() > PLUGIN_METADATA_MAX_BYTES {
            return Err(invalid(format!(
                "metadata value is {} bytes; the cap is {PLUGIN_METADATA_MAX_BYTES} (assets belong in the asset store, not inline)",
                v.len()
            )));
        }
        let parsed: serde_json::Value = serde_json::from_str(v)
            .map_err(|e| invalid(format!("metadata value must be the JSON envelope: {e}")))?;
        let envelope_ok = parsed.as_object().is_some_and(|o| {
            o.get("v")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|n| n >= 1)
                && o.get("data").is_some_and(serde_json::Value::is_object)
        });
        if !envelope_ok {
            return Err(invalid(
                "metadata envelope must be { v: <int >= 1>, data: {…}, engine?: {…} }".into(),
            ));
        }
    }
    let Some(si) = find_spread_for_leaf(doc, node) else {
        return Err(OperationError::NodeNotFound(node.clone()));
    };

    // ---- mutation ----
    let self_id = node.self_id().to_string();
    let labels = &mut doc.spreads[si].spread.labels;
    let prev: Option<String> = labels
        .get(&self_id)
        .and_then(|entries| entries.iter().find(|(k, _)| k == key))
        .map(|(_, v)| v.clone());
    match new_value {
        Some(v) => {
            let entries = labels.entry(self_id).or_default();
            match entries.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = v.clone(),
                None => entries.push((key.clone(), v.clone())),
            }
        }
        None => {
            if let Some(entries) = labels.get_mut(&self_id) {
                entries.retain(|(k, _)| k != key);
                if entries.is_empty() {
                    labels.remove(&self_id);
                }
            }
        }
    }

    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::PluginMetadata,
            value: Value::PluginMetadata {
                key: key.clone(),
                value: new_value.clone(),
                caller: caller.clone(),
                prev: Some(prev.clone()),
            },
        },
        inverse: Operation::SetProperty {
            node: node.clone(),
            path: PropertyPath::PluginMetadata,
            // The inverse is an engine-authoritative restore, not a
            // plugin call — no caller gate (B-16).
            value: Value::PluginMetadata {
                key: key.clone(),
                value: prev,
                caller: None,
                prev: Some(new_value.clone()),
            },
        },
        // Metadata is invisible to the renderer — no invalidation.
        invalidation: InvalidationHint::default(),
    })
}

/// B-04 / W1.20 — group page items (leaf shapes OR existing groups,
/// the latter producing a nested group-of-groups). Fully validated
/// BEFORE any mutation (atomicity invariant). Members contiguous in
/// z-order group paint-neutrally (the group ref takes the earliest
/// member's `frames_in_order` slot, paint recursion emits members
/// there in stored order); scattered members deterministically
/// collect at the earliest slot. The inverse carries the original
/// slots so undo restores z-order EXACTLY either way.
pub(super) fn apply_create_group(
    doc: &mut paged_scene::Document,
    spec: &GroupSpec,
) -> Result<AppliedOperation, OperationError> {
    use paged_parse::FrameRef;

    let invalid = |reason: String| OperationError::InvalidValue {
        node: NodeId::Group(spec.self_id.clone().unwrap_or_default()),
        path: PropertyPath::FrameTransform,
        reason,
    };

    if spec.members.is_empty() {
        return Err(invalid("a group needs at least one member".into()));
    }
    // Duplicate member ids.
    {
        let mut seen = std::collections::HashSet::new();
        for m in &spec.members {
            if !seen.insert(m.self_id().to_string()) {
                return Err(invalid(format!("duplicate member \"{}\"", m.self_id())));
            }
        }
    }
    // Locate the ONE spread holding every member; resolve FrameRefs.
    // `member_frame_ref` resolves Group members too (v2 nesting).
    let mut located: Option<(usize, Vec<FrameRef>)> = None;
    for (si, parsed) in doc.spreads.iter().enumerate() {
        let spread = &parsed.spread;
        let refs: Vec<Option<FrameRef>> = spec
            .members
            .iter()
            .map(|m| member_frame_ref(spread, m))
            .collect();
        if refs.iter().all(|r| r.is_some()) {
            located = Some((si, refs.into_iter().flatten().collect()));
            break;
        }
        if refs.iter().any(|r| r.is_some()) {
            return Err(invalid("all members must live on the same spread".into()));
        }
    }
    let Some((spread_idx, member_refs)) = located else {
        return Err(invalid("member not found in any spread".into()));
    };
    // Materialise the z-table when this spread never carried one (a
    // synthesised blank document built up via InsertNode keeps an empty
    // `frames_in_order` — register_frame_ref no-ops on the empty table).
    // The top-level membership check + z-slot lookups below require an
    // authoritative order; a COMPLETE materialisation equals the
    // renderer's legacy fallback, so it's render-neutral. `member_refs`
    // are kind+index FrameRefs, unaffected by this.
    super::insert_node::ensure_frames_in_order(&mut doc.spreads[spread_idx].spread);
    let spread = &doc.spreads[spread_idx].spread;
    // W1.20 — nested re-create (inverse-only): the new group nests
    // inside `parent` at a captured slot. Its members are expected to
    // be DIRECT members of that parent (they were spliced in by the
    // dissolve this inverts), not top-level `frames_in_order` entries —
    // so the placement / validation differs from a fresh top-level
    // create. Resolve the parent now.
    let nested_parent: Option<usize> = match &spec.parent {
        Some(p) => match spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(p.group_id.as_str()))
        {
            Some(gi) => Some(gi),
            None => {
                return Err(invalid(format!(
                    "parent group \"{}\" not found",
                    p.group_id
                )));
            }
        },
        None => None,
    };
    // Already grouped? A member may not already belong to a DIFFERENT
    // group. For the nested re-create the members legitimately sit in
    // the named parent (we're about to move them out of it), so skip
    // that one group in the scan.
    for (gi, g) in spread.groups.iter().enumerate() {
        if nested_parent == Some(gi) {
            continue;
        }
        for r in &member_refs {
            if g.members.contains(r) {
                return Err(invalid("a member already belongs to another group".into()));
            }
        }
    }
    if let Some(parent_idx) = nested_parent {
        // Nested path: every member must currently be a direct member
        // of the parent group.
        for r in &member_refs {
            if !spread.groups[parent_idx].members.contains(r) {
                return Err(invalid(
                    "nested-create member is not a direct child of the named parent".into(),
                ));
            }
        }
    } else {
        // Top-level path: every member must sit in frames_in_order.
        for r in &member_refs {
            if !spread.frames_in_order.contains(r) {
                return Err(invalid("member is not a top-level spread item".into()));
            }
        }
    }
    // Mint or validate the id.
    let self_id = match &spec.self_id {
        Some(s) => {
            if spread
                .groups
                .iter()
                .any(|g| g.self_id.as_deref() == Some(s))
            {
                return Err(OperationError::DuplicateNodeId { id: s.clone() });
            }
            s.clone()
        }
        None => mint_group_id(doc),
    };

    // ---- mutation (validated; cannot fail past this point) ----
    let spread = &mut doc.spreads[spread_idx].spread;
    let new_group_idx = spread.groups.len();

    // Inverse-only for the TOP-LEVEL create: the members' exact
    // pre-group `frames_in_order` slots, so undo restores scattered
    // z-order bytewise. Nested re-creates leave it `None` (the
    // dissolve they invert re-nests via its own captured inverse).
    let top_level_restore_slots: Option<Vec<u32>> = if let Some(parent_idx) = nested_parent {
        // Nested re-create: order members by their position in the
        // parent's `members` (the order the dissolve spliced them in),
        // splice them OUT, and reference the new group from the parent
        // at the recorded index.
        let parent_members = &spread.groups[parent_idx].members;
        let mut ordered: Vec<(usize, FrameRef)> = member_refs
            .iter()
            .map(|r| {
                let pos = parent_members
                    .iter()
                    .position(|x| x == r)
                    .expect("validated: member is a direct child of the parent");
                (pos, *r)
            })
            .collect();
        ordered.sort_by_key(|(pos, _)| *pos);
        let members_in_order: Vec<FrameRef> = ordered.iter().map(|(_, r)| *r).collect();
        spread.groups.push(paged_parse::Group {
            self_id: Some(self_id.clone()),
            members: members_in_order.clone(),
            transparency: Default::default(),
            item_transform: spec.item_transform,
        });
        // The recorded slot is where `FrameRef::Group(new_group_idx)`
        // goes in the parent's members. Remove the member entries (may
        // be `FrameRef::Group`s — sub-groups) FIRST, then insert the new
        // wrapper at the recorded slot (clamped post-removal).
        spread.groups[parent_idx]
            .members
            .retain(|r| !members_in_order.contains(r));
        let at = spec
            .parent
            .as_ref()
            .map(|p| p.index as usize)
            .unwrap_or(0)
            .min(spread.groups[parent_idx].members.len());
        spread.groups[parent_idx]
            .members
            .insert(at, FrameRef::Group(new_group_idx));
        None
    } else {
        // Top-level create: members ordered by frames_in_order slots;
        // the group ref takes the earliest member's slot (the InDesign
        // "group adopts the topmost member's z-position" rule).
        let mut ordered: Vec<(usize, FrameRef)> = member_refs
            .iter()
            .map(|r| {
                let pos = spread
                    .frames_in_order
                    .iter()
                    .position(|x| x == r)
                    .expect("validated above");
                (pos, *r)
            })
            .collect();
        ordered.sort_by_key(|(pos, _)| *pos);
        let earliest = ordered[0].0;
        let members_doc_order: Vec<FrameRef> = ordered.iter().map(|(_, r)| *r).collect();
        let slots: Vec<u32> = ordered.iter().map(|(pos, _)| *pos as u32).collect();
        spread.groups.push(paged_parse::Group {
            self_id: Some(self_id.clone()),
            members: members_doc_order.clone(),
            transparency: Default::default(),
            item_transform: spec.item_transform,
        });
        // Remove the member entries (which MAY be `FrameRef::Group`s in
        // v2 nested create), THEN re-insert the new group ref at the
        // earliest member's former slot. Removing first avoids the
        // ambiguity of "is this group ref a member or the new wrapper?".
        spread
            .frames_in_order
            .retain(|r| !members_doc_order.contains(r));
        let insert_at = earliest.min(spread.frames_in_order.len());
        spread
            .frames_in_order
            .insert(insert_at, FrameRef::Group(new_group_idx));
        Some(slots)
    };

    let mut resolved = spec.clone();
    resolved.self_id = Some(self_id.clone());
    Ok(AppliedOperation {
        op: Operation::CreateGroup { spec: resolved },
        inverse: Operation::DissolveGroup {
            group_id: self_id,
            restore_slots: top_level_restore_slots,
        },
        invalidation: InvalidationHint {
            structural: true,
            frame_geometry: spec.members.clone(),
            ..Default::default()
        },
    })
}

/// B-04 / W1.20 — dissolve a group; members are spliced back at the
/// group's slot in stored order — or, for an undo inverse carrying
/// `restore_slots`, at their exact pre-group indices.
///
/// v2: a NESTED group (one that is a member of a parent group) splices
/// its members into the PARENT's `members` at the group's slot, rather
/// than rejecting (v1) or surfacing them at the spread root. Member
/// EFFECTIVE transforms are pre-baked (they already compose the
/// dissolved group's `ItemTransform`), so the rendered geometry is
/// unchanged — the dissolve only removes the wrapper. The inverse
/// `CreateGroup` carries the captured parent link + the group's own
/// `ItemTransform` so re-grouping restores the exact prior structure.
/// `FrameRef::Group` indices above the removed entry are fixed up
/// across the spread (frames_in_order AND every group's members).
pub(super) fn apply_dissolve_group(
    doc: &mut paged_scene::Document,
    group_id: &str,
    restore_slots: Option<&[u32]>,
) -> Result<AppliedOperation, OperationError> {
    use paged_parse::FrameRef;

    let node = NodeId::Group(group_id.to_string());
    let invalid = |reason: String| OperationError::InvalidValue {
        node: node.clone(),
        path: PropertyPath::FrameTransform,
        reason,
    };

    // Locate the group.
    let mut found: Option<(usize, usize)> = None;
    for (si, parsed) in doc.spreads.iter().enumerate() {
        if let Some(gi) = parsed
            .spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(group_id))
        {
            found = Some((si, gi));
            break;
        }
    }
    let Some((spread_idx, group_idx)) = found else {
        return Err(OperationError::NodeNotFound(node));
    };
    let spread = &doc.spreads[spread_idx].spread;

    // Where does this group live? Either nested inside a parent group
    // (splice members into the parent) or at the spread root (splice
    // into frames_in_order). v2 handles both.
    let parent_idx: Option<usize> = spread
        .groups
        .iter()
        .position(|g| g.members.contains(&FrameRef::Group(group_idx)));

    enum Site {
        /// `frames_in_order` slot (top-level group).
        Root(usize),
        /// (parent group index, position within parent.members).
        Nested(usize, usize),
    }
    let site = match parent_idx {
        Some(pi) => {
            let pos = spread.groups[pi]
                .members
                .iter()
                .position(|r| *r == FrameRef::Group(group_idx))
                .expect("parent contains this group (just found)");
            Site::Nested(pi, pos)
        }
        None => {
            let Some(slot) = spread
                .frames_in_order
                .iter()
                .position(|r| *r == FrameRef::Group(group_idx))
            else {
                return Err(invalid("group is not a top-level spread item".into()));
            };
            Site::Root(slot)
        }
    };

    // Members → NodeIds for the inverse spec. v2 resolves nested
    // `FrameRef::Group` members too (a group-of-groups dissolves, its
    // sub-groups splicing up one level).
    let member_nodes: Option<Vec<NodeId>> = spread.groups[group_idx]
        .members
        .iter()
        .map(|r| node_for_frame_ref(spread, *r))
        .collect();
    let Some(member_nodes) = member_nodes else {
        return Err(invalid(
            "group has an id-less member that cannot round-trip".into(),
        ));
    };
    // Capture the parent-group id NOW (before the index fix-up shifts
    // `parent_idx`), for the inverse's nested re-create.
    let parent_link = match &site {
        Site::Nested(pi, pos) => {
            spread.groups[*pi]
                .self_id
                .clone()
                .map(|gid| crate::operation::NestedParent {
                    group_id: gid,
                    index: *pos as u32,
                })
        }
        Site::Root(_) => None,
    };

    // ---- mutation ----
    let spread = &mut doc.spreads[spread_idx].spread;
    let group = spread.groups.remove(group_idx);

    match &site {
        Site::Root(slot) => {
            spread.frames_in_order.remove(*slot);
            match restore_slots {
                // Undo path: members back at their exact pre-group
                // indices (captured ascending, paired with member order).
                Some(slots) if slots.len() == group.members.len() => {
                    for (r, s) in group.members.iter().zip(slots) {
                        let at = (*s as usize).min(spread.frames_in_order.len());
                        spread.frames_in_order.insert(at, *r);
                    }
                }
                // User-initiated ungroup: members stay together at the
                // group's slot (the InDesign semantic).
                _ => {
                    for (k, r) in group.members.iter().enumerate() {
                        spread.frames_in_order.insert(slot + k, *r);
                    }
                }
            }
        }
        Site::Nested(parent_orig, pos) => {
            // Removing `group_idx` from `spread.groups` shifts every
            // later index down by one — including the parent's, if it
            // sat after the dissolved group. Recompute the parent index
            // post-removal.
            let parent_now = if *parent_orig > group_idx {
                *parent_orig - 1
            } else {
                *parent_orig
            };
            // Drop the group ref from the parent, splice the members in
            // at the same slot in stored order (geometry invariant: the
            // members keep their pre-baked effective transforms).
            spread.groups[parent_now].members.remove(*pos);
            for (k, r) in group.members.iter().enumerate() {
                spread.groups[parent_now].members.insert(pos + k, *r);
            }
        }
    }

    // Index fix-up: every FrameRef::Group(j) with j > group_idx
    // decrements, in frames_in_order AND in remaining groups' members.
    let fix = |r: &mut FrameRef| {
        if let FrameRef::Group(j) = r {
            if *j > group_idx {
                *j -= 1;
            }
        }
    };
    for r in spread.frames_in_order.iter_mut() {
        fix(r);
    }
    for g in spread.groups.iter_mut() {
        for r in g.members.iter_mut() {
            fix(r);
        }
    }

    Ok(AppliedOperation {
        op: Operation::DissolveGroup {
            group_id: group_id.to_string(),
            restore_slots: restore_slots.map(<[u32]>::to_vec),
        },
        inverse: Operation::CreateGroup {
            spec: GroupSpec {
                self_id: group.self_id.clone(),
                members: member_nodes,
                parent: parent_link,
                item_transform: group.item_transform,
            },
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// W1.20 (groups v2) — move/scale/rotate a group as a unit. Sets the
/// group's own `ItemTransform` to `transform` and rebases every
/// descendant's pre-baked EFFECTIVE transform by the delta
/// `transform * inv(prev)` so the members follow rigidly. The renderer
/// and the hit-tester both read each leaf's effective `item_transform`,
/// so they agree by construction. Nested child groups' own transforms
/// ride the delta too (keeping the stored "un-composed group
/// transform" field consistent for re-serialization). Inverse: the
/// same op carrying the captured `prev` as the new transform.
pub(super) fn apply_set_group_transform(
    doc: &mut paged_scene::Document,
    group_id: &str,
    transform: Option<[f32; 6]>,
    prev_arg: Option<[f32; 6]>,
) -> Result<AppliedOperation, OperationError> {
    use crate::path_math::{affine_multiply, group_rebase_delta, AFFINE_IDENTITY};
    use paged_parse::FrameRef;

    let node = NodeId::Group(group_id.to_string());

    // Locate the group + its spread.
    let mut found: Option<(usize, usize)> = None;
    for (si, parsed) in doc.spreads.iter().enumerate() {
        if let Some(gi) = parsed
            .spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(group_id))
        {
            found = Some((si, gi));
            break;
        }
    }
    let Some((spread_idx, group_idx)) = found else {
        return Err(OperationError::NodeNotFound(node));
    };

    let spread = &mut doc.spreads[spread_idx].spread;
    let g_old = spread.groups[group_idx].item_transform;
    let g_new = transform.unwrap_or(AFFINE_IDENTITY);
    // The captured previous value (for the inverse). On the inverse
    // pass the caller passes `prev_arg = None` and we recapture from the
    // live state, which is the just-applied `g_new` — exactly what the
    // inverse needs.
    let captured_prev = g_old;
    let _ = prev_arg;

    // Delta that maps the old effective geometry to the new:
    // `delta = g_new * inv(g_old)`. A singular old transform (never in
    // well-formed IDML) makes the rebase a no-op rather than panicking.
    let Some(delta) = group_rebase_delta(g_old, g_new) else {
        // Degenerate prior transform: still store the new own-transform
        // (no member rebase possible) so at least the group field is set.
        spread.groups[group_idx].item_transform = transform;
        return Ok(AppliedOperation {
            op: Operation::SetGroupTransform {
                group: group_id.to_string(),
                transform,
                prev: captured_prev,
            },
            inverse: Operation::SetGroupTransform {
                group: group_id.to_string(),
                transform: captured_prev,
                prev: transform,
            },
            invalidation: InvalidationHint {
                frame_geometry: vec![node],
                ..Default::default()
            },
        });
    };

    // Collect every descendant FrameRef (leaves + nested groups),
    // descending through nested groups. We then rebase each.
    let mut leaves: Vec<FrameRef> = Vec::new();
    let mut nested_groups: Vec<usize> = Vec::new();
    fn walk(
        spread: &paged_parse::Spread,
        gi: usize,
        leaves: &mut Vec<FrameRef>,
        nested_groups: &mut Vec<usize>,
    ) {
        for m in &spread.groups[gi].members {
            match *m {
                FrameRef::Group(child) => {
                    nested_groups.push(child);
                    walk(spread, child, leaves, nested_groups);
                }
                leaf => leaves.push(leaf),
            }
        }
    }
    walk(spread, group_idx, &mut leaves, &mut nested_groups);

    // Rebase a leaf's effective transform: `leaf' = delta * leaf`
    // (`None` ⇒ identity leaf, so `leaf' = delta`). An exact-identity
    // result collapses back to `None` so an inverse op (`inv(delta)`
    // applied to the rebased value) restores the original `None`
    // bytewise — the common translate/undo path round-trips exactly.
    let rebase = |cur: Option<[f32; 6]>| -> Option<[f32; 6]> {
        let m = affine_multiply(delta, cur.unwrap_or(AFFINE_IDENTITY));
        if m == AFFINE_IDENTITY {
            None
        } else {
            Some(m)
        }
    };
    for leaf in &leaves {
        match *leaf {
            FrameRef::TextFrame(i) => {
                if let Some(f) = spread.text_frames.get_mut(i) {
                    f.item_transform = rebase(f.item_transform);
                }
            }
            FrameRef::Rectangle(i) => {
                if let Some(f) = spread.rectangles.get_mut(i) {
                    f.item_transform = rebase(f.item_transform);
                }
            }
            FrameRef::Oval(i) => {
                if let Some(f) = spread.ovals.get_mut(i) {
                    f.item_transform = rebase(f.item_transform);
                }
            }
            FrameRef::GraphicLine(i) => {
                if let Some(f) = spread.graphic_lines.get_mut(i) {
                    f.item_transform = rebase(f.item_transform);
                }
            }
            FrameRef::Polygon(i) => {
                if let Some(f) = spread.polygons.get_mut(i) {
                    f.item_transform = rebase(f.item_transform);
                }
            }
            FrameRef::Group(_) => unreachable!("groups collected separately"),
        }
    }
    // Nested child groups: their own (un-composed) transform also rides
    // the delta, keeping the stored field consistent with the rebased
    // member geometry on re-serialization. Same exact-identity → `None`
    // collapse as the leaf rebase so undo round-trips bytewise.
    for child in &nested_groups {
        if let Some(g) = spread.groups.get_mut(*child) {
            g.item_transform = rebase(g.item_transform);
        }
    }
    // Finally set THIS group's own transform.
    spread.groups[group_idx].item_transform = transform;

    // Invalidation: every leaf node id that moved (frame geometry).
    let mut moved: Vec<NodeId> = vec![node.clone()];
    for leaf in &leaves {
        if let Some(nid) = node_for_frame_ref(spread, *leaf) {
            moved.push(nid);
        }
    }

    Ok(AppliedOperation {
        op: Operation::SetGroupTransform {
            group: group_id.to_string(),
            transform,
            prev: captured_prev,
        },
        inverse: Operation::SetGroupTransform {
            group: group_id.to_string(),
            transform: captured_prev,
            prev: transform,
        },
        invalidation: InvalidationHint {
            frame_geometry: moved,
            ..Default::default()
        },
    })
}

pub(super) fn apply_create_gradient(
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
    gradients.insert(
        self_id.clone(),
        gradient_entry_from_spec(self_id.clone(), spec),
    );
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

pub(super) fn apply_edit_gradient(
    doc: &mut Document,
    gradient_id: &str,
    spec: &GradientSpec,
) -> Result<AppliedOperation, OperationError> {
    let gradients = &mut doc.palette.gradients;
    let existing =
        gradients
            .get(gradient_id)
            .ok_or_else(|| OperationError::CollectionEntryNotFound {
                collection: "gradient".to_string(),
                id: gradient_id.to_string(),
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

pub(super) fn apply_delete_gradient(
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

pub(super) fn color_group_entry_from_spec(
    self_id: String,
    spec: &ColorGroupSpec,
) -> paged_parse::graphic::ColorGroupEntry {
    paged_parse::graphic::ColorGroupEntry {
        self_id,
        name: spec.name.clone(),
        members: spec.members.clone(),
    }
}

pub(super) fn apply_create_color_group(
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

pub(super) fn apply_edit_color_group(
    doc: &mut Document,
    group_id: &str,
    spec: &ColorGroupSpec,
) -> Result<AppliedOperation, OperationError> {
    let groups = &mut doc.palette.color_groups;
    let existing = groups
        .get(group_id)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "color group".to_string(),
            id: group_id.to_string(),
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

pub(super) fn apply_delete_color_group(
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

// ── Numbering-list collection mutations (W1.22, engine gap 22) ─────
//
// `<NumberingList>` resources live in `doc.styles.numbering_lists`
// (BTreeMap keyed by `Self` id). CRUD mirrors the colour-group ops
// exactly. A list's `continue_across_stories` flag drives cross-story
// numbering continuity in the renderer, so the invalidation is the
// conservative `structural` (a change can reflow numbering across the
// whole document).

pub(super) fn numbering_list_def_from_spec(
    self_id: String,
    spec: &NumberingListSpec,
) -> paged_parse::styles::NumberingListDef {
    paged_parse::styles::NumberingListDef {
        self_id,
        name: spec.name.clone(),
        continue_across_stories: spec.continue_across_stories,
        continue_across_documents: spec.continue_across_documents,
    }
}

pub(super) fn numbering_list_spec_from_def(
    def: &paged_parse::styles::NumberingListDef,
) -> NumberingListSpec {
    NumberingListSpec {
        self_id: Some(def.self_id.clone()),
        name: def.name.clone(),
        continue_across_stories: def.continue_across_stories,
        continue_across_documents: def.continue_across_documents,
    }
}

pub(super) fn apply_create_numbering_list(
    doc: &mut Document,
    spec: &NumberingListSpec,
) -> Result<AppliedOperation, OperationError> {
    let lists = &mut doc.styles.numbering_lists;
    let self_id = match &spec.self_id {
        Some(s) => {
            if lists.contains_key(s) {
                return Err(OperationError::DuplicateNodeId { id: s.clone() });
            }
            s.clone()
        }
        None => {
            let mut n = lists.len();
            let mut id = format!("NumberingList/u{n}");
            while lists.contains_key(&id) {
                n += 1;
                id = format!("NumberingList/u{n}");
            }
            id
        }
    };
    lists.insert(
        self_id.clone(),
        numbering_list_def_from_spec(self_id.clone(), spec),
    );
    let mut resolved = spec.clone();
    resolved.self_id = Some(self_id.clone());
    Ok(AppliedOperation {
        op: Operation::CreateNumberingList { spec: resolved },
        inverse: Operation::DeleteNumberingList { list_id: self_id },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

pub(super) fn apply_edit_numbering_list(
    doc: &mut Document,
    list_id: &str,
    spec: &NumberingListSpec,
) -> Result<AppliedOperation, OperationError> {
    let lists = &mut doc.styles.numbering_lists;
    let existing = lists
        .get(list_id)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "numbering list".to_string(),
            id: list_id.to_string(),
        })?;
    let prior = numbering_list_spec_from_def(existing);
    lists.insert(
        list_id.to_string(),
        numbering_list_def_from_spec(list_id.to_string(), spec),
    );
    Ok(AppliedOperation {
        op: Operation::EditNumberingList {
            list_id: list_id.to_string(),
            spec: spec.clone(),
        },
        inverse: Operation::EditNumberingList {
            list_id: list_id.to_string(),
            spec: prior,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

pub(super) fn apply_delete_numbering_list(
    doc: &mut Document,
    list_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let captured = doc.styles.numbering_lists.remove(list_id).ok_or_else(|| {
        OperationError::CollectionEntryNotFound {
            collection: "numbering list".to_string(),
            id: list_id.to_string(),
        }
    })?;
    Ok(AppliedOperation {
        op: Operation::DeleteNumberingList {
            list_id: list_id.to_string(),
        },
        inverse: Operation::CreateNumberingList {
            spec: numbering_list_spec_from_def(&captured),
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
        pub(super) fn $create_fn(
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
                let def: $def =
                    serde_json::from_str(json).map_err(|e| OperationError::InvalidValue {
                        node: NodeId::Layer(String::new()),
                        path: PropertyPath::LayerName,
                        reason: format!("malformed {} restore payload: {e}", $label),
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

        pub(super) fn $rename_fn(
            doc: &mut Document,
            style_id: &str,
            name: &str,
        ) -> Result<AppliedOperation, OperationError> {
            let map = &mut doc.styles.$map;
            let def =
                map.get_mut(style_id)
                    .ok_or_else(|| OperationError::CollectionEntryNotFound {
                        collection: $label.to_string(),
                        id: style_id.to_string(),
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

        pub(super) fn $delete_fn(
            doc: &mut Document,
            style_id: &str,
        ) -> Result<AppliedOperation, OperationError> {
            let map = &mut doc.styles.$map;
            let captured =
                map.remove(style_id)
                    .ok_or_else(|| OperationError::CollectionEntryNotFound {
                        collection: $label.to_string(),
                        id: style_id.to_string(),
                    })?;
            // Serialize the captured def for a lossless create-inverse.
            let json =
                serde_json::to_string(&captured).map_err(|e| OperationError::InvalidValue {
                    node: NodeId::Layer(String::new()),
                    path: PropertyPath::LayerName,
                    reason: format!("failed to capture {} for undo: {e}", $label),
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
pub(super) fn style_node_marker(style_id: &str) -> NodeId {
    NodeId::Layer(style_id.to_string())
}

pub(super) fn set_paragraph_style_field(
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
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.point_size);
            def.point_size = *n;
            Ok(prior)
        }
        PropertyPath::CharacterTracking => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.tracking);
            def.tracking = *n;
            Ok(prior)
        }
        PropertyPath::CharacterFillColor => {
            let Value::ColorRef(c) = value else {
                return Err(type_err());
            };
            let prior = Value::ColorRef(def.fill_color.clone());
            def.fill_color = c.clone();
            Ok(prior)
        }
        PropertyPath::ParagraphSpaceBefore => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.space_before);
            def.space_before = *n;
            Ok(prior)
        }
        PropertyPath::ParagraphSpaceAfter => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.space_after);
            def.space_after = *n;
            Ok(prior)
        }
        PropertyPath::ParagraphFirstLineIndent => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.first_line_indent);
            def.first_line_indent = *n;
            Ok(prior)
        }
        PropertyPath::ParagraphJustification => {
            let Value::Text(s) = value else {
                return Err(type_err());
            };
            let prior = Value::Text(
                def.justification
                    .map(|j| j.as_idml().to_string())
                    .unwrap_or_default(),
            );
            def.justification = paged_parse::story::Justification::from_idml(s);
            Ok(prior)
        }
        // styles.next-style (W1.22) — set the paragraph style's
        // `NextStyle` chain. Value is the next style's self id; the
        // empty string clears it (`None`). Prior is captured as the
        // current ref (or empty) so the inverse round-trips.
        PropertyPath::ParagraphStyleNextStyle => {
            let Value::Text(s) = value else {
                return Err(type_err());
            };
            let prior = Value::Text(def.next_style.clone().unwrap_or_default());
            def.next_style = if s.is_empty() { None } else { Some(s.clone()) };
            Ok(prior)
        }
        _ => Err(OperationError::UnsupportedProperty {
            node: style_node_marker(style_id),
            path,
        }),
    }
}

pub(super) fn set_character_style_field(
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
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.point_size);
            def.point_size = *n;
            Ok(prior)
        }
        PropertyPath::CharacterTracking => {
            let Value::Length(n) = value else {
                return Err(type_err());
            };
            let prior = Value::Length(def.tracking);
            def.tracking = *n;
            Ok(prior)
        }
        PropertyPath::CharacterFillColor => {
            let Value::ColorRef(c) = value else {
                return Err(type_err());
            };
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

pub(super) fn apply_set_style_property(
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

pub(super) fn apply_batch(
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
