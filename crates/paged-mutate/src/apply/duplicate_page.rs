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
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation, PropertyPath};

// ---------------------------------------------------------------------------
// W0.5 — duplicate page
// ---------------------------------------------------------------------------

pub(super) fn apply_duplicate_page(
    doc: &mut Document,
    page: &str,
    clone_spread_json: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    // Redo path — re-materialise the captured clone verbatim.
    if let Some(json) = clone_spread_json {
        let restore: SpreadRestore =
            serde_json::from_str(json).map_err(|e| OperationError::InvalidValue {
                node: NodeId::Page(page.to_string()),
                path: PropertyPath::PageBounds,
                reason: format!("malformed duplicate-page payload: {e}"),
            })?;
        let cloned_page_id = restore
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
            op: Operation::DuplicatePage {
                page: page.to_string(),
                clone_spread_json: Some(json.to_string()),
            },
            inverse: Operation::RemovePage {
                page_id: cloned_page_id,
            },
            invalidation: InvalidationHint {
                structural: true,
                ..Default::default()
            },
        });
    }

    let src_idx = doc
        .spreads
        .iter()
        .position(|p| {
            p.spread
                .pages
                .iter()
                .any(|pg| pg.self_id.as_deref() == Some(page))
        })
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Page(page.to_string())))?;
    if doc.spreads[src_idx].spread.pages.len() != 1 {
        return Err(OperationError::InvalidValue {
            node: NodeId::Page(page.to_string()),
            path: PropertyPath::PageBounds,
            reason: "duplicating a page out of a multi-page spread is not supported in v1"
                .to_string(),
        });
    }

    // Deep-clone the source spread, then remap every Self id to a fresh
    // one (spread, page, and all page items) so the clone is a distinct
    // document object. Reuse the id-minting convention used by
    // InsertPage / id scans (`u<hex>`).
    let mut clone = doc.spreads[src_idx].clone();
    let mut next = next_id_seed(doc);
    let remap = |slot: &mut Option<String>, next: &mut u64| {
        *slot = Some(format!("u{:x}", *next));
        *next += 1;
    };
    remap(&mut clone.spread.self_id, &mut next);
    for pg in &mut clone.spread.pages {
        remap(&mut pg.self_id, &mut next);
    }
    for f in &mut clone.spread.text_frames {
        remap(&mut f.self_id, &mut next);
    }
    for r in &mut clone.spread.rectangles {
        remap(&mut r.self_id, &mut next);
    }
    for o in &mut clone.spread.ovals {
        remap(&mut o.self_id, &mut next);
    }
    for l in &mut clone.spread.graphic_lines {
        remap(&mut l.self_id, &mut next);
    }
    for p in &mut clone.spread.polygons {
        remap(&mut p.self_id, &mut next);
    }
    for g in &mut clone.spread.groups {
        remap(&mut g.self_id, &mut next);
    }

    // Stack the clone below everything on the pasteboard (same rule as
    // InsertPage) so spread AABBs never overlap.
    let mut max_bottom: f32 = 0.0;
    for parsed in &doc.spreads {
        let sty = parsed.spread.item_transform.map(|m| m[5]).unwrap_or(0.0);
        for p in &parsed.spread.pages {
            let pty = p.item_transform.map(|m| m[5]).unwrap_or(0.0);
            max_bottom = max_bottom.max(sty + pty + p.bounds.bottom);
        }
    }
    clone.spread.item_transform = Some([1.0, 0.0, 0.0, 1.0, 0.0, max_bottom + SPREAD_STACK_GAP_PT]);
    let cloned_spread_self = clone.spread.self_id.clone().unwrap_or_default();
    clone.src = format!("Spreads/Spread_{cloned_spread_self}.xml");
    let cloned_page_id = clone
        .spread
        .pages
        .first()
        .and_then(|p| p.self_id.clone())
        .unwrap_or_default();

    let insert_index = src_idx + 1;
    // Capture the materialised clone so redo re-creates the exact ids.
    let restore = SpreadRestore {
        index: insert_index,
        src: clone.src.clone(),
        spread: clone.spread.clone(),
    };
    let json = serde_json::to_string(&restore).map_err(|e| OperationError::InvalidValue {
        node: NodeId::Page(page.to_string()),
        path: PropertyPath::PageBounds,
        reason: format!("duplicate-page capture failed: {e}"),
    })?;

    doc.spreads.insert(insert_index, clone);

    Ok(AppliedOperation {
        op: Operation::DuplicatePage {
            page: page.to_string(),
            clone_spread_json: Some(json),
        },
        inverse: Operation::RemovePage {
            page_id: cloned_page_id,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// Highest `u<hex>` id seen across the document + 1, as a raw counter
/// for minting a run of fresh ids (DuplicatePage needs many at once).
pub(super) fn next_id_seed(doc: &Document) -> u64 {
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
    max + 1
}
