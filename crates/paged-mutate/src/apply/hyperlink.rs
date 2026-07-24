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

//! v53 — the hyperlink CREATE/REMOVE door.
//!
//! The IDML hyperlink model has three cooperating pieces, all of which the
//! parser + renderer already understood; what was missing was a way to
//! *create* them with a mutation:
//!
//! 1. a `HyperlinkTextSource` span — a run tagged `hyperlink_source = <id>`;
//! 2. a document-level `Hyperlink { source, destination }` in the designmap;
//! 3. a `HyperlinkDestination::Url(..)` in the designmap.
//!
//! The renderer's `links.rs` resolves a run's `hyperlink_source` →
//! `Hyperlink.source` → `Hyperlink.destination` → the destination URL to make
//! the span clickable. `InsertHyperlink` walks `[start, end)` (the same
//! CONTIGUOUS char-offset address space as `apply::character` — NOT the
//! `mutate::locate` byte+`\n` space; the range-styling ops this door sits
//! beside all walk char-contiguous), splitting runs at the boundaries and
//! tagging the middle pieces, then pushes the two designmap resources. The
//! inverse, `RemoveHyperlink`, untags by source id and drops the resources.

use crate::error::OperationError;
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation};
use paged_scene::Document;

use super::paragraph::split_run_at;

/// Paint-only frame-style invalidation for `story_id`'s host frame — tagging a
/// hyperlink source changes clickability + resolved paint, never line geometry,
/// so it repaints without re-running layout (mirrors the underline/strikethru
/// branch in `apply::character`).
fn frame_style_hint(doc: &Document, story_id: &str) -> InvalidationHint {
    match doc
        .frame_for_story
        .get(story_id)
        .and_then(|f| f.self_id.clone())
    {
        Some(self_id) => InvalidationHint {
            frame_style: vec![NodeId::TextFrame(self_id)],
            ..Default::default()
        },
        None => InvalidationHint::default(),
    }
}

/// Tag every run of `story` whose `[run_start, run_end)` falls inside
/// `[start, end)` with `hyperlink_source`, splitting runs at the boundaries.
/// Contiguous char offsets. Returns the number of runs that ended up tagged.
fn tag_source_over_range(
    story: &mut paged_model::Story,
    start: u32,
    end: u32,
    source_id: &str,
) -> u32 {
    let mut tagged = 0u32;
    let mut char_offset: u32 = 0;
    for para in story.paragraphs.iter_mut() {
        let para_chars: u32 = para
            .runs
            .iter()
            .map(|r| r.text.chars().count() as u32)
            .sum();
        let para_start = char_offset;
        let para_end = char_offset + para_chars;
        char_offset = para_end;

        if para_end <= start || para_start >= end {
            continue;
        }

        let original_runs: Vec<paged_model::CharacterRun> = para.runs.drain(..).collect();
        let mut new_runs: Vec<paged_model::CharacterRun> =
            Vec::with_capacity(original_runs.len() * 2);
        let mut local_offset: u32 = 0;

        for run in original_runs {
            let run_len = run.text.chars().count() as u32;
            let run_start = para_start + local_offset;
            let run_end = run_start + run_len;
            local_offset += run_len;

            if run_end <= start || run_start >= end {
                new_runs.push(run);
                continue;
            }

            let local_left = (run_start < start).then(|| start - run_start);
            let local_right = (run_end > end).then(|| end - run_start);

            match (local_left, local_right) {
                (None, None) => {
                    let mut mid = run;
                    mid.hyperlink_source = Some(source_id.to_string());
                    tagged += 1;
                    new_runs.push(mid);
                }
                (Some(split_at), None) => {
                    let (left, mut right) = split_run_at(run, split_at);
                    right.hyperlink_source = Some(source_id.to_string());
                    tagged += 1;
                    new_runs.push(left);
                    new_runs.push(right);
                }
                (None, Some(split_at)) => {
                    let (mut left, right) = split_run_at(run, split_at);
                    left.hyperlink_source = Some(source_id.to_string());
                    tagged += 1;
                    new_runs.push(left);
                    new_runs.push(right);
                }
                (Some(left_at), Some(right_at)) => {
                    let (left, rest) = split_run_at(run, left_at);
                    let (mut mid, right) = split_run_at(rest, right_at - left_at);
                    mid.hyperlink_source = Some(source_id.to_string());
                    tagged += 1;
                    new_runs.push(left);
                    new_runs.push(mid);
                    new_runs.push(right);
                }
            }
        }

        para.runs = new_runs;
    }
    tagged
}

/// v53 — make the story range `[start, end)` a native clickable link to `url`.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_insert_hyperlink(
    doc: &mut Document,
    story_id: &str,
    start: u32,
    end: u32,
    url: &str,
    source_id: &str,
    dest_id: &str,
    hyperlink_id: &str,
) -> Result<AppliedOperation, OperationError> {
    if start >= end {
        return Err(OperationError::InvalidPosition {
            parent: NodeId::Story(story_id.to_string()),
            position: start as usize,
            len: end as usize,
        });
    }
    let story_idx = doc
        .stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Story(story_id.to_string())))?;

    tag_source_over_range(&mut doc.stories[story_idx].story, start, end, source_id);

    doc.designmap
        .hyperlink_destinations
        .push(paged_model::HyperlinkDestination {
            self_id: dest_id.to_string(),
            kind: paged_model::HyperlinkDestinationKind::Url(url.to_string()),
        });
    doc.designmap.hyperlinks.push(paged_model::Hyperlink {
        self_id: hyperlink_id.to_string(),
        name: None,
        source: Some(source_id.to_string()),
        destination: Some(dest_id.to_string()),
    });

    let invalidation = frame_style_hint(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::InsertHyperlink {
            story_id: story_id.to_string(),
            start,
            end,
            url: url.to_string(),
            source_id: source_id.to_string(),
            dest_id: dest_id.to_string(),
            hyperlink_id: hyperlink_id.to_string(),
        },
        inverse: Operation::RemoveHyperlink {
            story_id: story_id.to_string(),
            start,
            end,
            url: url.to_string(),
            source_id: source_id.to_string(),
            dest_id: dest_id.to_string(),
            hyperlink_id: hyperlink_id.to_string(),
        },
        invalidation,
    })
}

/// v53 — the `InsertHyperlink` inverse: untag the source + drop the resources.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_remove_hyperlink(
    doc: &mut Document,
    story_id: &str,
    start: u32,
    end: u32,
    url: &str,
    source_id: &str,
    dest_id: &str,
    hyperlink_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let story_idx = doc
        .stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Story(story_id.to_string())))?;

    // Untag every run carrying this source (the range was already split on
    // insert, so a by-source sweep restores the semantic state exactly).
    for para in doc.stories[story_idx].story.paragraphs.iter_mut() {
        for run in para.runs.iter_mut() {
            if run.hyperlink_source.as_deref() == Some(source_id) {
                run.hyperlink_source = None;
            }
        }
    }
    doc.designmap
        .hyperlinks
        .retain(|h| h.self_id != hyperlink_id);
    doc.designmap
        .hyperlink_destinations
        .retain(|d| d.self_id != dest_id);

    let invalidation = frame_style_hint(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::RemoveHyperlink {
            story_id: story_id.to_string(),
            start,
            end,
            url: url.to_string(),
            source_id: source_id.to_string(),
            dest_id: dest_id.to_string(),
            hyperlink_id: hyperlink_id.to_string(),
        },
        inverse: Operation::InsertHyperlink {
            story_id: story_id.to_string(),
            start,
            end,
            url: url.to_string(),
            source_id: source_id.to_string(),
            dest_id: dest_id.to_string(),
            hyperlink_id: hyperlink_id.to_string(),
        },
        invalidation,
    })
}
