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
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation};

// ---------------------------------------------------------------------------
// W0.5 — master application
// ---------------------------------------------------------------------------

pub(super) fn apply_master_to_page(
    doc: &mut Document,
    page: &str,
    master: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let page_ref = find_page_mut(doc, page)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Page(page.to_string())))?;
    let prev = page_ref.applied_master.clone();
    page_ref.applied_master = master.map(str::to_string);
    Ok(AppliedOperation {
        op: Operation::ApplyMasterToPage {
            page: page.to_string(),
            master: master.map(str::to_string),
        },
        inverse: Operation::ApplyMasterToPage {
            page: page.to_string(),
            master: prev,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// Operations that address pages or the spread list, which mean nothing
/// inside one master.
fn page_level(op: &Operation) -> bool {
    match op {
        Operation::InsertPage { .. }
        | Operation::RemovePage { .. }
        | Operation::MovePage { .. }
        | Operation::SetSpreadOrder { .. }
        | Operation::DuplicatePage { .. }
        | Operation::ApplyMasterToPage { .. }
        | Operation::InsertSection { .. }
        | Operation::EditSection { .. }
        | Operation::DeleteSection { .. }
        | Operation::OnMaster { .. } => true,
        Operation::Batch { ops } => ops.iter().any(page_level),
        _ => false,
    }
}

/// v69 — `OnMaster`: run `op` with master `master_id` standing in as the
/// document's only spread, then put both back.
pub(super) fn apply_on_master(
    doc: &mut Document,
    master_id: &str,
    op: &Operation,
) -> Result<AppliedOperation, OperationError> {
    let key = master_id.rsplit_once('/').map_or(master_id, |(_, id)| id);
    if page_level(op) {
        return Err(OperationError::InvalidValue {
            node: NodeId::Page(key.to_string()),
            path: PropertyPath::PageBounds,
            reason: "page and spread operations do not apply inside a master".to_string(),
        });
    }
    let Some(master) = doc.master_spreads.remove(key) else {
        return Err(OperationError::NodeNotFound(NodeId::Page(key.to_string())));
    };
    let paged_scene::ParsedMasterSpread {
        src,
        self_id,
        spread,
    } = master;
    let pages = std::mem::replace(
        &mut doc.spreads,
        vec![paged_scene::ParsedSpread {
            src: src.clone(),
            spread,
        }],
    );
    let result = super::apply_inner(doc, op);
    let mut swapped = std::mem::replace(&mut doc.spreads, pages);
    let spread = swapped.pop().map(|p| p.spread).unwrap_or_default();
    doc.master_spreads.insert(
        key.to_string(),
        paged_scene::ParsedMasterSpread {
            src,
            self_id,
            spread,
        },
    );
    let applied = result?;
    Ok(AppliedOperation {
        op: Operation::OnMaster {
            master_id: key.to_string(),
            op: Box::new(applied.op),
        },
        inverse: Operation::OnMaster {
            master_id: key.to_string(),
            op: Box::new(applied.inverse),
        },
        // Every page the master is applied to repaints.
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}
