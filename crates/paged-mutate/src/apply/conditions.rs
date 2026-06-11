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
    AppliedOperation, InvalidationHint, Operation,
};

// ---------------------------------------------------------------------------
// W0.5 — conditions
// ---------------------------------------------------------------------------

pub(super) fn apply_set_condition_visible(
    doc: &mut Document,
    condition: &str,
    visible: bool,
) -> Result<AppliedOperation, OperationError> {
    let cond = doc.styles.conditions.get_mut(condition).ok_or_else(|| {
        OperationError::CollectionEntryNotFound {
            collection: "conditions".to_string(),
            id: condition.to_string(),
        }
    })?;
    // `None` ⇒ visible (IDML default), so capture the resolved prior.
    let prev = cond.visible.unwrap_or(true);
    cond.visible = Some(visible);
    Ok(AppliedOperation {
        op: Operation::SetConditionVisible {
            condition: condition.to_string(),
            visible,
        },
        inverse: Operation::SetConditionVisible {
            condition: condition.to_string(),
            visible: prev,
        },
        // Conditional text changes which runs render → reflow the
        // whole document (advisory: structural).
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

pub(super) fn apply_activate_condition_set(
    doc: &mut Document,
    set: &str,
) -> Result<AppliedOperation, OperationError> {
    let members: Vec<String> = doc
        .styles
        .condition_sets
        .get(set)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "conditionSets".to_string(),
            id: set.to_string(),
        })?
        .conditions
        .clone();
    // Capture every condition's prior visibility so the inverse is a
    // single RestoreConditionVisibility.
    let mut states: Vec<(String, bool)> = Vec::with_capacity(doc.styles.conditions.len());
    for (id, def) in doc.styles.conditions.iter() {
        states.push((id.clone(), def.visible.unwrap_or(true)));
    }
    // Activate: members visible, everyone else hidden.
    let member_set: std::collections::HashSet<&str> = members.iter().map(String::as_str).collect();
    for (id, def) in doc.styles.conditions.iter_mut() {
        def.visible = Some(member_set.contains(id.as_str()));
    }
    Ok(AppliedOperation {
        op: Operation::ActivateConditionSet {
            set: set.to_string(),
        },
        inverse: Operation::RestoreConditionVisibility { states },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

pub(super) fn apply_restore_condition_visibility(
    doc: &mut Document,
    states: &[(String, bool)],
) -> Result<AppliedOperation, OperationError> {
    // Capture the current state for THIS op's own inverse so a
    // restore is itself undoable (redo of ActivateConditionSet).
    let mut prior: Vec<(String, bool)> = Vec::with_capacity(states.len());
    for (id, vis) in states {
        if let Some(def) = doc.styles.conditions.get_mut(id) {
            prior.push((id.clone(), def.visible.unwrap_or(true)));
            def.visible = Some(*vis);
        }
    }
    Ok(AppliedOperation {
        op: Operation::RestoreConditionVisibility {
            states: states.to_vec(),
        },
        inverse: Operation::RestoreConditionVisibility { states: prior },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

