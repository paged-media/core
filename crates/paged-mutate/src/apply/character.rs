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
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, Operation, PropertyPath, Value,
};

// ---------------------------------------------------------------------------
// SDK Phase 3 — character properties addressed by `NodeId::StoryRange`
// ---------------------------------------------------------------------------
//
// The forward op walks `doc.stories[story_id].story.paragraphs`,
// computing the running character offset across all `CharacterRun.text`
// fields in order. Runs whose `[run_start, run_end)` intersect
// `[start, end)` receive the new property value; an inverse `Batch`
// of restorations is built per affected run.
//
// Constraint (this commit): the range must align with whole-run
// boundaries. If `start` or `end` cuts inside a `CharacterRun.text`,
// the apply returns `OperationError::Unimplemented`. Run-splitting
// at arbitrary character offsets is a Phase 3.x follow-up — it
// needs a story-snapshot inverse strategy (clone the affected
// paragraphs' run lists pre-mutation, restore on undo) to round-
// trip bytewise, which in turn needs `CharacterRun` to derive
// Deserialize/PartialEq/Tsify. Out of scope for this commit;
// today's editor-binding-flow can target catalog-bound writes that
// already snap to run boundaries.

pub(super) fn apply_character_property(
    doc: &mut Document,
    story_id: &str,
    start: u32,
    end: u32,
    node: &NodeId,
    path: PropertyPath,
    value: &Value,
) -> Result<AppliedOperation, OperationError> {
    if start >= end {
        return Err(OperationError::InvalidValue {
            node: node.clone(),
            path,
            reason: format!("empty range: start={start} >= end={end}"),
        });
    }

    let story_idx = doc
        .stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| OperationError::NodeNotFound(node.clone()))?;

    let story = &mut doc.stories[story_idx].story;
    let mut inverse_ops: Vec<Operation> = Vec::new();
    let mut char_offset: u32 = 0;

    // Per-paragraph walk. Runs that intersect [start, end) are
    // split as needed and the "middle" piece (the one fully inside
    // [start, end)) receives the new property value. Inverse is a
    // Batch of per-(now-split-)run SetProperty restorations
    // addressed at the post-split range — undo restores each
    // affected run's previous value without re-merging the splits.
    // A future "merge consecutive runs with identical properties"
    // pass can canonicalize the document; today's correctness is
    // bytewise even with extra boundaries.
    for para in story.paragraphs.iter_mut() {
        let para_chars: u32 = para
            .runs
            .iter()
            .map(|r| r.text.chars().count() as u32)
            .sum();
        let para_start = char_offset;
        let para_end = char_offset + para_chars;
        char_offset = para_end;

        // Skip paragraphs entirely outside [start, end).
        if para_end <= start || para_start >= end {
            continue;
        }

        // Rebuild this paragraph's runs vec, splitting as needed.
        let original_runs: Vec<paged_model::CharacterRun> = para.runs.drain(..).collect();
        let mut new_runs: Vec<paged_model::CharacterRun> =
            Vec::with_capacity(original_runs.len() * 2);
        let mut local_offset: u32 = 0;

        for run in original_runs {
            let run_len = run.text.chars().count() as u32;
            let run_start = para_start + local_offset;
            let run_end = run_start + run_len;
            local_offset += run_len;

            let intersects = run_end > start && run_start < end;
            if !intersects {
                new_runs.push(run);
                continue;
            }

            // Local split offsets within the run (in characters):
            // - left split at `local_left` if run starts BEFORE the
            //   requested range — everything before it stays as the
            //   pre-mutation value.
            // - right split at `local_right` if run ends AFTER the
            //   range — everything past it stays as well.
            let local_left = if run_start < start {
                Some(start - run_start)
            } else {
                None
            };
            let local_right = if run_end > end {
                Some(end - run_start)
            } else {
                None
            };

            match (local_left, local_right) {
                (None, None) => {
                    // Whole run in range. Mutate in place.
                    let mut mutated = run;
                    let (prev_value, _new_set) =
                        apply_character_field_on_run(&mut mutated, path, value)?;
                    inverse_ops.push(Operation::SetProperty {
                        node: NodeId::StoryRange {
                            story_id: story_id.to_string(),
                            start: run_start,
                            end: run_end,
                        },
                        path,
                        value: prev_value,
                    });
                    new_runs.push(mutated);
                }
                (Some(split_at), None) => {
                    // Run starts before the range; one split at
                    // `start`. Left piece stays; right piece gets
                    // mutated.
                    let (left, mut right) = split_run_at(run, split_at);
                    let mid_start = run_start + split_at;
                    let mid_end = run_end;
                    let (prev_value, _) = apply_character_field_on_run(&mut right, path, value)?;
                    inverse_ops.push(Operation::SetProperty {
                        node: NodeId::StoryRange {
                            story_id: story_id.to_string(),
                            start: mid_start,
                            end: mid_end,
                        },
                        path,
                        value: prev_value,
                    });
                    new_runs.push(left);
                    new_runs.push(right);
                }
                (None, Some(split_at)) => {
                    // Run ends after the range; one split at `end`.
                    // Left piece gets mutated; right piece stays.
                    let (mut left, right) = split_run_at(run, split_at);
                    let mid_start = run_start;
                    let mid_end = run_start + split_at;
                    let (prev_value, _) = apply_character_field_on_run(&mut left, path, value)?;
                    inverse_ops.push(Operation::SetProperty {
                        node: NodeId::StoryRange {
                            story_id: story_id.to_string(),
                            start: mid_start,
                            end: mid_end,
                        },
                        path,
                        value: prev_value,
                    });
                    new_runs.push(left);
                    new_runs.push(right);
                }
                (Some(left_at), Some(right_at)) => {
                    // Run straddles both ends of the range; two
                    // splits — three pieces. Middle gets mutated.
                    let (left, rest) = split_run_at(run, left_at);
                    let (mut mid, right) = split_run_at(rest, right_at - left_at);
                    let mid_start = run_start + left_at;
                    let mid_end = run_start + right_at;
                    let (prev_value, _) = apply_character_field_on_run(&mut mid, path, value)?;
                    inverse_ops.push(Operation::SetProperty {
                        node: NodeId::StoryRange {
                            story_id: story_id.to_string(),
                            start: mid_start,
                            end: mid_end,
                        },
                        path,
                        value: prev_value,
                    });
                    new_runs.push(left);
                    new_runs.push(mid);
                    new_runs.push(right);
                }
            }
        }

        para.runs = new_runs;
    }

    if inverse_ops.is_empty() {
        // No runs in the range — empty story or pre/post the
        // entire content. Return a no-op AppliedOperation so the
        // caller's undo stack stays consistent.
        return Ok(AppliedOperation {
            op: Operation::SetProperty {
                node: node.clone(),
                path,
                value: value.clone(),
            },
            inverse: Operation::SetProperty {
                node: node.clone(),
                path,
                value: value.clone(),
            },
            invalidation: InvalidationHint::default(),
        });
    }

    // Build an InvalidationHint targeting the host text frame so the
    // renderer's cache invalidates the right page. Most character
    // properties remeasure the line (font / size / scale / kerning /
    // ligatures …), so they invalidate `text_reflow`. Paint-only
    // decorations (underline / strikethru) don't change the line's
    // geometry — they invalidate `frame_style` instead so the
    // renderer repaints without re-running layout. The story-to-frame
    // index is built at document open; if it's empty (shouldn't happen
    // for parsed docs) we leave the hint default.
    let paint_only = matches!(
        path,
        PropertyPath::CharacterUnderline | PropertyPath::CharacterStrikethru
    );
    let invalidation = match doc.frame_for_story.get(story_id) {
        Some(frame) => {
            if let Some(self_id) = &frame.self_id {
                let frame_node = NodeId::TextFrame(self_id.clone());
                if paint_only {
                    InvalidationHint {
                        frame_style: vec![frame_node],
                        ..Default::default()
                    }
                } else {
                    InvalidationHint {
                        text_reflow: vec![frame_node],
                        ..Default::default()
                    }
                }
            } else {
                InvalidationHint::default()
            }
        }
        None => InvalidationHint::default(),
    };

    // The forward op's recorded form is the original (caller-provided)
    // node/path/value. The inverse is a Batch of per-run restorations
    // — even if there's only one affected run, wrapping in Batch keeps
    // the inverse shape stable across the cardinality of the range.
    let inverse = if inverse_ops.len() == 1 {
        inverse_ops.into_iter().next().unwrap()
    } else {
        Operation::Batch { ops: inverse_ops }
    };

    Ok(AppliedOperation {
        op: Operation::SetProperty {
            node: node.clone(),
            path,
            value: value.clone(),
        },
        inverse,
        invalidation,
    })
}
