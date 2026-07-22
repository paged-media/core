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

use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{
    AppliedOperation, GuideOrientationSpec, InvalidationHint, NodeId, Operation,
};

// ---------------------------------------------------------------------------
// W0.5 — guide CRUD
// ---------------------------------------------------------------------------

/// Guides carry no id in the parse struct, so we address them
/// positionally as `Guide/<spread_self_id>/<index>`. Resolve such an id
/// to its `(spread_index, guide_index)`.
pub(super) fn resolve_guide(doc: &Document, guide_id: &str) -> Option<(usize, usize)> {
    let rest = guide_id.strip_prefix("Guide/")?;
    let (spread_self, idx_str) = rest.rsplit_once('/')?;
    let gi: usize = idx_str.parse().ok()?;
    let si = doc
        .spreads
        .iter()
        .position(|p| p.spread.self_id.as_deref() == Some(spread_self))?;
    if gi < doc.spreads[si].spread.guides.len() {
        Some((si, gi))
    } else {
        None
    }
}

/// The positional id for the guide at `(spread_index, guide_index)`.
pub(super) fn guide_id_for(doc: &Document, si: usize, gi: usize) -> String {
    let spread_self = doc.spreads[si]
        .spread
        .self_id
        .as_deref()
        .unwrap_or_default();
    format!("Guide/{spread_self}/{gi}")
}

pub(super) fn apply_insert_guide(
    doc: &mut Document,
    spread_id: &str,
    orientation: GuideOrientationSpec,
    position: f32,
    page_index: u32,
    _guide_id: Option<String>,
) -> Result<AppliedOperation, OperationError> {
    let si = doc
        .spreads
        .iter()
        .position(|p| p.spread.self_id.as_deref() == Some(spread_id))
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Spread(spread_id.to_string())))?;
    let guide = paged_model::RulerGuide {
        orientation: orientation.to_parse(),
        location: position,
        page_index,
    };
    doc.spreads[si].spread.guides.push(guide);
    let gi = doc.spreads[si].spread.guides.len() - 1;
    let id = guide_id_for(doc, si, gi);
    Ok(AppliedOperation {
        op: Operation::InsertGuide {
            spread_id: spread_id.to_string(),
            orientation,
            position,
            page_index,
            guide_id: Some(id.clone()),
        },
        inverse: Operation::DeleteGuide { guide_id: id },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

pub(super) fn apply_move_guide(
    doc: &mut Document,
    guide_id: &str,
    position: f32,
) -> Result<AppliedOperation, OperationError> {
    let (si, gi) =
        resolve_guide(doc, guide_id).ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "guides".to_string(),
            id: guide_id.to_string(),
        })?;
    let prev = doc.spreads[si].spread.guides[gi].location;
    doc.spreads[si].spread.guides[gi].location = position;
    Ok(AppliedOperation {
        op: Operation::MoveGuide {
            guide_id: guide_id.to_string(),
            position,
        },
        inverse: Operation::MoveGuide {
            guide_id: guide_id.to_string(),
            position: prev,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

pub(super) fn apply_delete_guide(
    doc: &mut Document,
    guide_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let (si, gi) =
        resolve_guide(doc, guide_id).ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "guides".to_string(),
            id: guide_id.to_string(),
        })?;
    let removed = doc.spreads[si].spread.guides.remove(gi);
    let spread_id = doc.spreads[si].spread.self_id.clone().unwrap_or_default();
    Ok(AppliedOperation {
        op: Operation::DeleteGuide {
            guide_id: guide_id.to_string(),
        },
        // Undo re-inserts at the tail; positional ids past this index
        // are stable because deletion shifts only higher indices and we
        // re-append. (v1: a delete-then-undo restores geometry, not the
        // exact mid-vec slot — acceptable since guides are unordered.)
        inverse: Operation::InsertGuide {
            spread_id,
            orientation: GuideOrientationSpec::from_parse(removed.orientation),
            position: removed.location,
            page_index: removed.page_index,
            guide_id: None,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}
