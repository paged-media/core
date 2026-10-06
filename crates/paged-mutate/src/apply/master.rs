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
        | Operation::OnMaster { .. }
        | Operation::CreateMaster { .. }
        | Operation::DeleteMaster { .. }
        | Operation::RestoreMaster { .. }
        | Operation::RenameMaster { .. } => true,
        Operation::Batch { ops } => ops.iter().any(page_level),
        _ => false,
    }
}

/// v70 — `OnMaster`: run `op` with master `master_id` standing in as the
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
        name,
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
            name,
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

fn bare(master_id: &str) -> &str {
    master_id.rsplit_once('/').map_or(master_id, |(_, id)| id)
}

fn master_error(id: &str, reason: String) -> OperationError {
    OperationError::InvalidValue {
        node: NodeId::Page(id.to_string()),
        path: PropertyPath::PageBounds,
        reason,
    }
}

/// v70 — create a master: one page of the given size, or a fresh-id copy
/// of `duplicate_of`.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_create_master(
    doc: &mut Document,
    master_id: &str,
    name: Option<&str>,
    width_pt: f32,
    height_pt: f32,
    duplicate_of: Option<&str>,
    restore_json: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let id = bare(master_id).to_string();
    if doc.master_spreads.contains_key(&id) {
        return Err(OperationError::DuplicateNodeId { id });
    }
    let master = match restore_json {
        Some(json) => serde_json::from_str::<paged_scene::ParsedMasterSpread>(json)
            .map_err(|e| master_error(&id, format!("master restore failed: {e}")))?,
        None => {
            let mut next = crate::ids::highest_u_hex_id(doc) + 1;
            let mut spread = match duplicate_of {
                Some(src) => {
                    let src = bare(src);
                    let Some(m) = doc.master_spreads.get(src) else {
                        return Err(OperationError::NodeNotFound(NodeId::Page(src.to_string())));
                    };
                    let mut copy = m.spread.clone();
                    let _ = super::duplicate_page::fresh_ids(&mut copy, &mut next);
                    copy
                }
                None => {
                    if !(width_pt > 0.0 && height_pt > 0.0) {
                        return Err(master_error(
                            &id,
                            "a master's page needs a positive size".into(),
                        ));
                    }
                    paged_model::Spread {
                        pages: vec![paged_model::Page {
                            self_id: Some(format!("u{next:x}")),
                            bounds: paged_model::Bounds {
                                top: 0.0,
                                left: 0.0,
                                bottom: height_pt,
                                right: width_pt,
                            },
                            applied_master: None,
                            item_transform: None,
                            master_page_transform: None,
                            override_list: Vec::new(),
                            name: None,
                            show_master_items: None,
                        }],
                        ..paged_model::Spread::default()
                    }
                }
            };
            spread.self_id = Some(id.clone());
            paged_scene::ParsedMasterSpread {
                src: format!("MasterSpreads/MasterSpread_{id}.xml"),
                self_id: id.clone(),
                spread,
                name: name.map(str::to_string).or_else(|| {
                    duplicate_of.and_then(|d| doc.master_spreads.get(bare(d))?.name.clone())
                }),
            }
        }
    };
    let json = serde_json::to_string(&master)
        .map_err(|e| master_error(&id, format!("master capture failed: {e}")))?;
    doc.master_spreads.insert(id.clone(), master);
    Ok(AppliedOperation {
        op: Operation::CreateMaster {
            master_id: id.clone(),
            name: name.map(str::to_string),
            width_pt,
            height_pt,
            duplicate_of: duplicate_of.map(str::to_string),
            restore_json: Some(json),
        },
        inverse: Operation::DeleteMaster { master_id: id },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// Whether any page (or master page) applies master `id`.
fn master_in_use(doc: &Document, id: &str) -> bool {
    let applies = |p: &paged_model::Page| p.applied_master.as_deref().map(bare) == Some(id);
    doc.spreads
        .iter()
        .any(|s| s.spread.pages.iter().any(applies))
        || doc
            .master_spreads
            .values()
            .any(|m| m.spread.pages.iter().any(applies))
}

/// v70 — remove a master no page applies.
pub(super) fn apply_delete_master(
    doc: &mut Document,
    master_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let id = bare(master_id).to_string();
    if !doc.master_spreads.contains_key(&id) {
        return Err(OperationError::NodeNotFound(NodeId::Page(id)));
    }
    if master_in_use(doc, &id) {
        return Err(master_error(
            &id,
            "the master is applied to a page; apply another first".into(),
        ));
    }
    let master = doc.master_spreads.remove(&id).expect("checked above");
    let json = serde_json::to_string(&master)
        .map_err(|e| master_error(&id, format!("master capture failed: {e}")))?;
    Ok(AppliedOperation {
        op: Operation::DeleteMaster { master_id: id },
        inverse: Operation::RestoreMaster { master_json: json },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// v70 — `DeleteMaster`'s inverse.
pub(super) fn apply_restore_master(
    doc: &mut Document,
    master_json: &str,
) -> Result<AppliedOperation, OperationError> {
    let master: paged_scene::ParsedMasterSpread = serde_json::from_str(master_json)
        .map_err(|e| master_error("", format!("master restore failed: {e}")))?;
    let id = master.self_id.clone();
    if doc.master_spreads.contains_key(&id) {
        return Err(OperationError::DuplicateNodeId { id });
    }
    doc.master_spreads.insert(id.clone(), master);
    Ok(AppliedOperation {
        op: Operation::RestoreMaster {
            master_json: master_json.to_string(),
        },
        inverse: Operation::DeleteMaster { master_id: id },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// v70 — set or clear a master's name.
pub(super) fn apply_rename_master(
    doc: &mut Document,
    master_id: &str,
    name: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let id = bare(master_id).to_string();
    let Some(master) = doc.master_spreads.get_mut(&id) else {
        return Err(OperationError::NodeNotFound(NodeId::Page(id)));
    };
    let prev = std::mem::replace(&mut master.name, name.map(str::to_string));
    Ok(AppliedOperation {
        op: Operation::RenameMaster {
            master_id: id.clone(),
            name: name.map(str::to_string),
        },
        inverse: Operation::RenameMaster {
            master_id: id,
            name: prev,
        },
        invalidation: InvalidationHint::default(),
    })
}
