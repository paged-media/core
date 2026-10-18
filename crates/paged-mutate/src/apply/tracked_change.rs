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

//! v74 — tracked deletions accepted and rejected (D-01).
//!
//! A tracked deletion is its place, a [`TRACKED_DELETION_MARKER`] character
//! of its paragraph, and the paragraph's `tracked_deletions` entry of the
//! same order. Accepting one keeps the text deleted: the place and the
//! entry go. Rejecting one puts the deleted text back where its place
//! stood, as InDesign 20.0.1 rejects a deletion (`track-changes-probe`:
//! "Alpha beta INSERTED delta." reads "Alpha beta INSERTED gamma delta."
//! once its deletion is rejected). Both address the change by its place's
//! contiguous story char offset, as the `trackedChanges` collection lists
//! it; `RestoreTrackedChange` is their inverse.

use paged_model::{TrackedChange, TRACKED_DELETION_MARKER as MARK};
use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{AppliedOperation, Operation};

use super::endnote::{insert_str, remove_chars};
use super::footnote::{insert_char, locate, story_index};
use super::path_topology::reflow_hint_for_story;

/// The paragraph, the paragraph-local char and the entry index of the
/// tracked deletion whose place stands at story char `offset`.
fn find(
    doc: &Document,
    story_id: &str,
    offset: u32,
) -> Result<(usize, usize, u32, usize), OperationError> {
    let si = story_index(doc, story_id)?;
    let story = &doc.stories[si].story;
    let not_found = || OperationError::CollectionEntryNotFound {
        collection: "trackedChanges".to_string(),
        id: format!("{story_id}@{offset}"),
    };
    let (pi, at) = locate(story, offset).ok_or_else(not_found)?;
    let para = &story.paragraphs[pi];
    let chars: Vec<char> = para.runs.iter().flat_map(|r| r.text.chars()).collect();
    if chars.get(at as usize) != Some(&MARK) {
        return Err(not_found());
    }
    let k = chars[..at as usize].iter().filter(|c| **c == MARK).count();
    if k >= para.tracked_deletions.len() {
        return Err(not_found());
    }
    Ok((si, pi, at, k))
}

/// Take a deletion's place and its entry out.
fn take(doc: &mut Document, si: usize, pi: usize, at: u32, k: usize) -> TrackedChange {
    let para = &mut doc.stories[si].story.paragraphs[pi];
    remove_chars(para, at, at + 1);
    para.tracked_deletions.remove(k)
}

pub(super) fn apply_accept(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
) -> Result<AppliedOperation, OperationError> {
    let (si, pi, at, k) = find(doc, story_id, offset)?;
    let change = take(doc, si, pi, at, k);
    Ok(AppliedOperation {
        op: Operation::AcceptTrackedChange {
            story_id: story_id.to_string(),
            offset,
        },
        inverse: Operation::RestoreTrackedChange {
            story_id: story_id.to_string(),
            offset,
            remove: 0,
            change: serde_json::to_string(&change).unwrap_or_default(),
        },
        invalidation: reflow_hint_for_story(doc, story_id),
    })
}

pub(super) fn apply_reject(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
) -> Result<AppliedOperation, OperationError> {
    let (si, pi, at, k) = find(doc, story_id, offset)?;
    let change = take(doc, si, pi, at, k);
    // The deleted text, its paragraph marks as the model keeps them in a
    // run's text.
    let text = change
        .deleted_text
        .clone()
        .unwrap_or_default()
        .replace('\r', "\n");
    insert_str(&mut doc.stories[si].story.paragraphs[pi], at, &text);
    Ok(AppliedOperation {
        op: Operation::RejectTrackedChange {
            story_id: story_id.to_string(),
            offset,
        },
        inverse: Operation::RestoreTrackedChange {
            story_id: story_id.to_string(),
            offset,
            remove: text.chars().count() as u32,
            change: serde_json::to_string(&change).unwrap_or_default(),
        },
        invalidation: reflow_hint_for_story(doc, story_id),
    })
}

/// Undo of accept / reject: take out the `remove` chars at `offset` (the
/// text a reject put back) and put the deletion's place and its entry
/// back there.
pub(super) fn apply_restore(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    remove: u32,
    change: &str,
) -> Result<AppliedOperation, OperationError> {
    let entry: TrackedChange =
        serde_json::from_str(change).map_err(|e| OperationError::CollectionEntryNotFound {
            collection: "trackedChanges".to_string(),
            id: format!("{story_id}@{offset}: malformed restore payload: {e}"),
        })?;
    let si = story_index(doc, story_id)?;
    let (pi, at) = locate(&doc.stories[si].story, offset).ok_or_else(|| {
        OperationError::CollectionEntryNotFound {
            collection: "trackedChanges".to_string(),
            id: format!("{story_id}@{offset}"),
        }
    })?;
    let para = &mut doc.stories[si].story.paragraphs[pi];
    if remove > 0 {
        remove_chars(para, at, at + remove);
    }
    let k = para
        .runs
        .iter()
        .flat_map(|r| r.text.chars())
        .take(at as usize)
        .filter(|c| *c == MARK)
        .count()
        .min(para.tracked_deletions.len());
    para.shift_anchors(at, 1);
    insert_char(para, at, MARK);
    para.tracked_deletions.insert(k, entry);
    Ok(AppliedOperation {
        op: Operation::RestoreTrackedChange {
            story_id: story_id.to_string(),
            offset,
            remove,
            change: change.to_string(),
        },
        inverse: if remove > 0 {
            Operation::RejectTrackedChange {
                story_id: story_id.to_string(),
                offset,
            }
        } else {
            Operation::AcceptTrackedChange {
                story_id: story_id.to_string(),
                offset,
            }
        },
        invalidation: reflow_hint_for_story(doc, story_id),
    })
}
