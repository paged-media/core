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
use paged_model::Spread;
use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation, PropertyPath};

// ---------------------------------------------------------------------------
// Batch
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Editor-ops — Page tool (InsertPage / RemovePage / PageBounds)
// ---------------------------------------------------------------------------

/// Lossless undo capture for `RemovePage`: the whole hosting spread
/// (every page item included) plus its position in `doc.spreads` and
/// its manifest src. Serialized to JSON inside the inverse Operation
/// so the op stays wire-shaped; `paged_model::Spread` derives
/// `Serialize`+`Deserialize` for exactly this round-trip.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct SpreadRestore {
    pub(super) index: usize,
    pub(super) src: String,
    pub(super) spread: Spread,
}

/// Letter portrait — the fallback page size when there is no
/// reference page to clone (matches the renderer's empty-document
/// fallback).
pub(super) const FALLBACK_PAGE_BOUNDS: [f32; 4] = [0.0, 0.0, 792.0, 612.0];
/// Pasteboard gap between an inserted spread and everything above it.
pub(super) const SPREAD_STACK_GAP_PT: f32 = 72.0;

/// Mint two fresh `u<hex>` ids (spread + page), unique across every
/// self id in the document — page items, spreads, and pages alike.
pub(super) fn mint_spread_page_ids(doc: &Document) -> (String, String) {
    let mut max: u64 = 0;
    let mut scan = |id: Option<&str>| {
        let Some(id) = id else { return };
        let Some(hex) = id.strip_prefix('u') else {
            return;
        };
        if hex.is_empty() || hex.len() > 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return;
        }
        if let Ok(v) = u64::from_str_radix(hex, 16) {
            max = max.max(v);
        }
    };
    for parsed in &doc.spreads {
        let s = &parsed.spread;
        scan(s.self_id.as_deref());
        for p in &s.pages {
            scan(p.self_id.as_deref());
        }
        for f in &s.text_frames {
            scan(f.self_id.as_deref());
        }
        for r in &s.rectangles {
            scan(r.self_id.as_deref());
        }
        for o in &s.ovals {
            scan(o.self_id.as_deref());
        }
        for l in &s.graphic_lines {
            scan(l.self_id.as_deref());
        }
        for p in &s.polygons {
            scan(p.self_id.as_deref());
        }
        for g in &s.groups {
            scan(g.self_id.as_deref());
        }
    }
    (format!("u{:x}", max + 1), format!("u{:x}", max + 2))
}

pub(super) fn find_page_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut paged_model::Page> {
    for parsed in &mut doc.spreads {
        if let Some(p) = parsed
            .spread
            .pages
            .iter_mut()
            .find(|p| p.self_id.as_deref() == Some(self_id))
        {
            return Some(p);
        }
    }
    None
}

pub(super) fn apply_insert_page(
    doc: &mut Document,
    after_page_id: Option<&str>,
    master_id: Option<&str>,
    spread_self_id: Option<String>,
    page_self_id: Option<String>,
    restore_spread_json: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let invalidation = InvalidationHint {
        structural: true,
        ..Default::default()
    };

    // Undo path — reinsert the captured spread verbatim at its
    // original index.
    if let Some(json) = restore_spread_json {
        let restore: SpreadRestore =
            serde_json::from_str(json).map_err(|e| OperationError::InvalidValue {
                node: NodeId::Spread(String::new()),
                path: PropertyPath::PageBounds,
                reason: format!("malformed spread restore payload: {e}"),
            })?;
        let page_id = restore
            .spread
            .pages
            .first()
            .and_then(|p| p.self_id.clone())
            .unwrap_or_default();
        let index = restore.index.min(doc.spreads.len());
        doc.spreads.insert(
            index,
            paged_scene::ParsedSpread {
                src: restore.src,
                spread: restore.spread,
            },
        );
        return Ok(AppliedOperation {
            op: Operation::InsertPage {
                after_page_id: after_page_id.map(str::to_string),
                master_id: master_id.map(str::to_string),
                spread_self_id,
                page_self_id,
                restore_spread_json: Some(json.to_string()),
            },
            inverse: Operation::RemovePage { page_id },
            invalidation,
        });
    }

    // Fresh insert — resolve the host spread (after_page_id's, else
    // the last) and clone its page size.
    let (host_idx, ref_bounds) = match after_page_id {
        Some(pid) => {
            let mut found = None;
            for (i, parsed) in doc.spreads.iter().enumerate() {
                if let Some(p) = parsed
                    .spread
                    .pages
                    .iter()
                    .find(|p| p.self_id.as_deref() == Some(pid))
                {
                    found = Some((i, bounds_to_array(p.bounds)));
                    break;
                }
            }
            found.ok_or_else(|| OperationError::NodeNotFound(NodeId::Page(pid.to_string())))?
        }
        None => {
            let last = doc.spreads.len().saturating_sub(1);
            let bounds = doc
                .spreads
                .last()
                .and_then(|parsed| parsed.spread.pages.first())
                .map(|p| bounds_to_array(p.bounds))
                .unwrap_or(FALLBACK_PAGE_BOUNDS);
            (last, bounds)
        }
    };

    // Stack the new spread below everything on the pasteboard so the
    // spread-origin consumers (hit-test, marquee, snap) never see
    // overlapping spread AABBs. Pure-translate transforms are the
    // universal case; rotated pages are out of scope (flagged).
    let mut max_bottom: f32 = 0.0;
    for parsed in &doc.spreads {
        let sty = parsed.spread.item_transform.map(|m| m[5]).unwrap_or(0.0);
        for p in &parsed.spread.pages {
            let pty = p.item_transform.map(|m| m[5]).unwrap_or(0.0);
            max_bottom = max_bottom.max(sty + pty + p.bounds.bottom);
        }
    }

    // Redo re-applies the echoed op, so prefer ids it carries; only
    // mint on the first application.
    let (sid, pid) = match (spread_self_id, page_self_id) {
        (Some(s), Some(p)) => (s, p),
        _ => mint_spread_page_ids(doc),
    };
    if doc.spreads.iter().any(|parsed| {
        parsed.spread.self_id.as_deref() == Some(sid.as_str())
            || parsed
                .spread
                .pages
                .iter()
                .any(|p| p.self_id.as_deref() == Some(pid.as_str()))
    }) {
        return Err(OperationError::DuplicateNodeId { id: sid });
    }

    let page = paged_model::Page {
        self_id: Some(pid.clone()),
        bounds: bounds_from_array(ref_bounds),
        applied_master: master_id.map(str::to_string),
        item_transform: None,
        master_page_transform: None,
        override_list: Vec::new(),
        name: None,
        show_master_items: None,
    };
    let spread = Spread {
        self_id: Some(sid.clone()),
        item_transform: Some([1.0, 0.0, 0.0, 1.0, 0.0, max_bottom + SPREAD_STACK_GAP_PT]),
        pages: vec![page],
        ..Spread::default()
    };
    doc.spreads.insert(
        host_idx + 1,
        paged_scene::ParsedSpread {
            src: format!("Spreads/Spread_{sid}.xml"),
            spread,
        },
    );

    Ok(AppliedOperation {
        op: Operation::InsertPage {
            after_page_id: after_page_id.map(str::to_string),
            master_id: master_id.map(str::to_string),
            spread_self_id: Some(sid),
            page_self_id: Some(pid.clone()),
            restore_spread_json: None,
        },
        inverse: Operation::RemovePage { page_id: pid },
        invalidation,
    })
}

pub(super) fn apply_remove_page(
    doc: &mut Document,
    page_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let node = NodeId::Page(page_id.to_string());
    let Some(idx) = doc.spreads.iter().position(|parsed| {
        parsed
            .spread
            .pages
            .iter()
            .any(|p| p.self_id.as_deref() == Some(page_id))
    }) else {
        return Err(OperationError::NodeNotFound(node));
    };
    let page_count = doc.spreads[idx].spread.pages.len();
    if page_count > 1 {
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::PageBounds,
            reason: "page belongs to a multi-page spread; per-page deletion within a spread \
                     is not supported in v1"
                .to_string(),
        });
    }
    if doc.spreads.len() == 1 {
        return Err(OperationError::InvalidValue {
            node,
            path: PropertyPath::PageBounds,
            reason: "cannot delete the document's only page".to_string(),
        });
    }
    let parsed = doc.spreads.remove(idx);
    let spread_self_id = parsed.spread.self_id.clone();
    let restore = SpreadRestore {
        index: idx,
        src: parsed.src,
        spread: parsed.spread,
    };
    let json = serde_json::to_string(&restore).map_err(|e| OperationError::InvalidValue {
        node: NodeId::Page(page_id.to_string()),
        path: PropertyPath::PageBounds,
        reason: format!("spread capture failed: {e}"),
    })?;
    Ok(AppliedOperation {
        op: Operation::RemovePage {
            page_id: page_id.to_string(),
        },
        inverse: Operation::InsertPage {
            after_page_id: None,
            master_id: None,
            spread_self_id,
            page_self_id: Some(page_id.to_string()),
            restore_spread_json: Some(json),
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}
