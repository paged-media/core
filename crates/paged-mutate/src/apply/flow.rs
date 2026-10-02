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
use crate::operation::{AppliedOperation, InvalidationHint, Operation};

// ---------------------------------------------------------------------------
// ADR 026 — growing frame chains
// ---------------------------------------------------------------------------

/// Set (or clear, `None`) a story's grow rule. The pages it generates are
/// derived at layout, so the rule IS the whole edit: the inverse restores the
/// prior rule and the next build regrows or shrinks to match.
pub(super) fn apply_set_flow_grow_rule(
    doc: &mut Document,
    story_id: &str,
    rule: &Option<paged_model::FlowGrowRule>,
) -> Result<AppliedOperation, OperationError> {
    let story = doc
        .stories
        .iter_mut()
        .find(|s| s.self_id == story_id)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "stories".to_string(),
            id: story_id.to_string(),
        })?;
    let prev = std::mem::replace(&mut story.story.grow, rule.clone());
    Ok(AppliedOperation {
        op: Operation::SetFlowGrowRule {
            story_id: story_id.to_string(),
            rule: rule.clone(),
        },
        inverse: Operation::SetFlowGrowRule {
            story_id: story_id.to_string(),
            rule: prev,
        },
        // The page set changes: rebuild the whole document.
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}
