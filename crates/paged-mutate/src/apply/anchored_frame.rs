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

//! v52 — the anchored-frame CREATE/REMOVE door.
//!
//! The anchored-object model (`paged_model::AnchoredFrame` on
//! `Paragraph::anchored_frames`), its inline render path
//! (`paged-renderer` `anchored.rs`), and its property edits
//! (`AnchoredPosition` / `AnchoredXOffset` / …) all predate this. What was
//! missing was a way to *create* an anchored frame with a mutation — the door
//! a content plugin (paged.doc) needs to place an inline image in the text
//! flow. `InsertAnchoredFrame` pushes an image-bearing anchored Rectangle onto
//! the paragraph containing a story character offset; leaving `setting = None`
//! makes the renderer default `anchored_position` to `"InlinePosition"`, so the
//! frame is drawn inline at the paragraph origin (its `image_link` painted via
//! the existing deferred-image path).

use crate::error::OperationError;
use crate::operation::{AppliedOperation, NodeId, Operation};
use paged_scene::Document;

use super::path_topology::reflow_hint_for_story;

/// The paragraph index in `story` whose text range contains story character
/// `offset`; clamps to the last paragraph when `offset` is past the end.
/// Paragraph boundaries count one break character between paragraphs (the same
/// running-offset convention as `apply::paragraph`).
fn paragraph_for_offset(story: &paged_model::Story, offset: u32) -> Option<usize> {
    if story.paragraphs.is_empty() {
        return None;
    }
    let mut char_offset: u32 = 0;
    for (i, para) in story.paragraphs.iter().enumerate() {
        let chars: u32 = para
            .runs
            .iter()
            .map(|r| r.text.chars().count() as u32)
            .sum();
        let end = char_offset + chars;
        if offset <= end {
            return Some(i);
        }
        char_offset = end + 1; // the inter-paragraph break
    }
    Some(story.paragraphs.len() - 1)
}

/// Find the (story index, paragraph index) of the anchored frame `self_id`.
fn locate_anchored_frame(doc: &Document, self_id: &str) -> Option<(usize, usize)> {
    for (si, ps) in doc.stories.iter().enumerate() {
        for (pi, para) in ps.story.paragraphs.iter().enumerate() {
            if para
                .anchored_frames
                .iter()
                .any(|af| af.self_id.as_deref() == Some(self_id))
            {
                return Some((si, pi));
            }
        }
    }
    None
}

/// v52 — insert an image-bearing anchored Rectangle at the paragraph holding
/// `offset` in `story_id`.
pub(super) fn apply_insert_anchored_frame(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    width: f32,
    height: f32,
    image_uri: Option<&str>,
    self_id: &str,
) -> Result<AppliedOperation, OperationError> {
    if locate_anchored_frame(doc, self_id).is_some() {
        return Err(OperationError::DuplicateNodeId {
            id: self_id.to_string(),
        });
    }
    let story_idx = doc
        .stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Story(story_id.to_string())))?;

    let para_idx = paragraph_for_offset(&doc.stories[story_idx].story, offset)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Story(story_id.to_string())))?;

    doc.stories[story_idx].story.paragraphs[para_idx]
        .anchored_frames
        .push(paged_model::AnchoredFrame {
            frame_kind: paged_model::AnchoredFrameKind::Rectangle,
            self_id: Some(self_id.to_string()),
            bounds: Some(paged_model::Bounds {
                top: 0.0,
                left: 0.0,
                bottom: height,
                right: width,
            }),
            item_transform: None,
            parent_story: None,
            // `setting: None` ⇒ the renderer defaults `anchored_position` to
            // "InlinePosition", so the frame draws inline at the paragraph origin.
            setting: None,
            fill_color: None,
            stroke_color: None,
            stroke_weight: None,
            fill_tint: None,
            gradient_fill_angle: None,
            applied_object_style: None,
            image_link: image_uri.map(str::to_string),
            image_item_transform: None,
            children: Vec::new(),
        });

    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::InsertAnchoredFrame {
            story_id: story_id.to_string(),
            offset,
            width,
            height,
            image_uri: image_uri.map(str::to_string),
            self_id: self_id.to_string(),
        },
        inverse: Operation::RemoveAnchoredFrame {
            story_id: story_id.to_string(),
            self_id: self_id.to_string(),
        },
        invalidation,
    })
}

/// v52 — remove the anchored frame `self_id` (the `InsertAnchoredFrame` inverse).
pub(super) fn apply_remove_anchored_frame(
    doc: &mut Document,
    story_id: &str,
    self_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let (story_idx, para_idx) = locate_anchored_frame(doc, self_id)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Story(story_id.to_string())))?;

    let frames = &mut doc.stories[story_idx].story.paragraphs[para_idx].anchored_frames;
    let pos = frames
        .iter()
        .position(|af| af.self_id.as_deref() == Some(self_id))
        .expect("locate_anchored_frame just found it");
    let removed = frames.remove(pos);

    let width = removed.bounds.map(|b| b.right - b.left).unwrap_or(0.0);
    let height = removed.bounds.map(|b| b.bottom - b.top).unwrap_or(0.0);
    let invalidation = reflow_hint_for_story(doc, story_id);
    Ok(AppliedOperation {
        op: Operation::RemoveAnchoredFrame {
            story_id: story_id.to_string(),
            self_id: self_id.to_string(),
        },
        inverse: Operation::InsertAnchoredFrame {
            story_id: story_id.to_string(),
            offset: 0,
            width,
            height,
            image_uri: removed.image_link.clone(),
            self_id: self_id.to_string(),
        },
        invalidation,
    })
}
