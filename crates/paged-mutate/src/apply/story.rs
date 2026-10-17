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

//! Removing and restoring a whole story — the inverse steps of a text
//! frame insert that minted its story. Internal: no wire mutation
//! produces these ops; they ride the undo log only.

use paged_scene::{Document, ParsedStory};

use crate::error::OperationError;
use crate::operation::{AppliedOperation, InvalidationHint, Operation};

fn structural() -> InvalidationHint {
    InvalidationHint {
        structural: true,
        ..Default::default()
    }
}

/// Remove `story_id`, capturing it whole (and its index) for the inverse.
pub(super) fn apply_remove_story(
    doc: &mut Document,
    story_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let position = doc
        .stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| OperationError::CollectionEntryNotFound {
            collection: "stories".to_string(),
            id: story_id.to_string(),
        })?;
    // A parsed story is plain data: serializing it cannot fail.
    let story_json =
        serde_json::to_string(&doc.stories[position]).expect("a parsed story serializes");
    doc.stories.remove(position);
    Ok(AppliedOperation {
        op: Operation::RemoveStory {
            story_id: story_id.to_string(),
        },
        inverse: Operation::RestoreStory {
            position,
            story_json,
        },
        invalidation: structural(),
    })
}

/// Put a captured story back at `position` (clamped to the list's end).
pub(super) fn apply_restore_story(
    doc: &mut Document,
    position: usize,
    story_json: &str,
) -> Result<AppliedOperation, OperationError> {
    let story: ParsedStory =
        serde_json::from_str(story_json).map_err(|e| OperationError::CollectionEntryNotFound {
            collection: "stories (captured story does not decode)".to_string(),
            id: e.to_string(),
        })?;
    if doc.stories.iter().any(|s| s.self_id == story.self_id) {
        return Err(OperationError::DuplicateNodeId { id: story.self_id });
    }
    let story_id = story.self_id.clone();
    let at = position.min(doc.stories.len());
    doc.stories.insert(at, story);
    Ok(AppliedOperation {
        op: Operation::RestoreStory {
            position,
            story_json: story_json.to_string(),
        },
        inverse: Operation::RemoveStory { story_id },
        invalidation: structural(),
    })
}

/// v0.71.13 — set a story's paragraphs to `paragraphs_json` (a JSON
/// `Vec<Paragraph>`), as Update Table of Contents writes a TOC story
/// (ADR 165). The inverse carries the paragraphs it replaced.
pub(super) fn apply_replace_story_paragraphs(
    doc: &mut Document,
    story_id: &str,
    paragraphs_json: &str,
) -> Result<AppliedOperation, OperationError> {
    let paragraphs: Vec<paged_model::Paragraph> =
        serde_json::from_str(paragraphs_json).map_err(|e| OperationError::InvalidParent {
            parent: crate::operation::NodeId::Story(story_id.to_string()),
            child_kind: format!("paragraphs that do not read ({e})"),
        })?;
    let parsed = doc
        .stories
        .iter_mut()
        .find(|s| s.self_id == story_id)
        .ok_or_else(|| {
            OperationError::NodeNotFound(crate::operation::NodeId::Story(story_id.to_string()))
        })?;
    let before = std::mem::replace(&mut parsed.story.paragraphs, paragraphs);
    // Plain data: serializing it cannot fail.
    let before_json = serde_json::to_string(&before).expect("paragraphs serialize");
    let invalidation = super::path_topology::reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::ReplaceStoryParagraphs {
            story_id: story_id.to_string(),
            paragraphs_json: paragraphs_json.to_string(),
        },
        inverse: Operation::ReplaceStoryParagraphs {
            story_id: story_id.to_string(),
            paragraphs_json: before_json,
        },
        invalidation,
    })
}
