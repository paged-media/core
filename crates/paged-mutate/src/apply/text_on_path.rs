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

//! C-29 type-on-a-path: `Operation::AttachTextToPath` /
//! `Operation::DetachTextFromPath` — create and remove the
//! [`paged_model::TextPath`] the renderer already consumes.
//!
//! The engine has rendered text-on-a-path since the parser learned
//! `<TextPath>`, but nothing could ever CREATE one: every mutate-side
//! constructor initialises `text_paths: Vec::new()` and no operation
//! ever pushed to it. A plugin could therefore render type on a path
//! it loaded from an IDML, but not author one. These two ops are that
//! gap, and nothing else — the renderer is untouched.

use paged_model::TextPath;

use crate::error::OperationError;
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, Operation, PropertyPath, TextPathSpec,
};

fn invalid(node: &NodeId, reason: String) -> OperationError {
    OperationError::InvalidValue {
        node: node.clone(),
        path: PropertyPath::FrameTransform,
        reason,
    }
}

/// The three kinds that carry `text_paths` in the model AND are walked
/// by the renderer's text-on-path pass. `Oval` is deliberately absent:
/// `paged_model::Oval` has no `text_paths` field and the pass never
/// looks at ovals, so accepting one would create a link nothing draws.
fn path_host_kind(node: &NodeId) -> bool {
    matches!(
        node,
        NodeId::Rectangle(_) | NodeId::GraphicLine(_) | NodeId::Polygon(_)
    )
}

/// Borrow the host's `text_paths` vec, whichever backing vec owns it.
fn host_text_paths<'a>(
    spread: &'a mut paged_model::Spread,
    node: &NodeId,
) -> Option<&'a mut Vec<TextPath>> {
    match node {
        NodeId::Rectangle(id) => spread
            .rectangles
            .iter_mut()
            .find(|r| r.self_id.as_deref() == Some(id.as_str()))
            .map(|r| &mut r.text_paths),
        NodeId::GraphicLine(id) => spread
            .graphic_lines
            .iter_mut()
            .find(|l| l.self_id.as_deref() == Some(id.as_str()))
            .map(|l| &mut l.text_paths),
        NodeId::Polygon(id) => spread
            .polygons
            .iter_mut()
            .find(|p| p.self_id.as_deref() == Some(id.as_str()))
            .map(|p| &mut p.text_paths),
        _ => None,
    }
}

/// C-29 — `Operation::AttachTextToPath`: link an EXISTING story to an
/// EXISTING path element, producing the `<TextPath>` the renderer
/// consumes.
///
/// Validation: the host is a Rectangle / GraphicLine / Polygon that
/// exists; the story exists in the document; the story is not already
/// flowing somewhere else (a text frame's `parent_story`, or another
/// path's `parent_story`) — InDesign likewise refuses to place one
/// story in two flows, and allowing it would double-render the text.
///
/// Inverse: `DetachTextFromPath { host, index: Some(idx), spec }`.
pub(super) fn apply_attach_text_to_path(
    doc: &mut paged_scene::Document,
    host: &NodeId,
    story_id: &str,
    spec: &TextPathSpec,
) -> Result<AppliedOperation, OperationError> {
    if !path_host_kind(host) {
        return Err(invalid(
            host,
            format!(
                "C-29: a {} cannot host text-on-a-path (Rectangle / GraphicLine / \
                 Polygon only — those are the kinds the renderer's text-path pass walks)",
                host.kind()
            ),
        ));
    }
    if !doc.stories.iter().any(|s| s.self_id == story_id) {
        return Err(invalid(
            host,
            format!("C-29: no story `{story_id}` in this document"),
        ));
    }
    // Already flowing into a frame chain?
    for parsed in &doc.spreads {
        if parsed
            .spread
            .text_frames
            .iter()
            .any(|f| f.parent_story.as_deref() == Some(story_id))
        {
            return Err(invalid(
                host,
                format!(
                    "C-29: story `{story_id}` already flows into a text frame \
                     (a story belongs to exactly one flow)"
                ),
            ));
        }
    }

    for parsed in doc.spreads.iter_mut() {
        let spread = &mut parsed.spread;
        // Already on some path (this one or another)?
        let taken = spread
            .rectangles
            .iter()
            .flat_map(|r| r.text_paths.iter())
            .chain(
                spread
                    .graphic_lines
                    .iter()
                    .flat_map(|l| l.text_paths.iter()),
            )
            .chain(spread.polygons.iter().flat_map(|p| p.text_paths.iter()))
            .any(|tp| tp.parent_story == story_id);
        if taken {
            return Err(invalid(
                host,
                format!("C-29: story `{story_id}` is already attached to a path"),
            ));
        }
        let Some(paths) = host_text_paths(spread, host) else {
            continue;
        };
        let index = paths.len();
        paths.push(TextPath {
            // IDML mints a `Self` for the element; the writer supplies
            // one on export, and the renderer keys off `parent_story`
            // rather than this id, so leaving it `None` is honest
            // rather than inventing an id shape InDesign owns.
            self_id: None,
            parent_story: story_id.to_string(),
            // The legacy non-standard `PathAlignment`; the renderer
            // documents that it ignores this field in favour of
            // `path_type_alignment`, so we never write it.
            path_alignment: None,
            path_type_alignment: spec.path_type_alignment.clone(),
            // See `TextPathSpec` — only Rainbow renders, so we leave
            // the attribute absent (which IS Rainbow) instead of
            // pretending to offer the other four.
            path_effect: None,
            flip_path_effect: spec.flip_path_effect.clone(),
            start_bracket: spec.start_bracket,
            end_bracket: spec.end_bracket,
        });
        return Ok(AppliedOperation {
            op: Operation::AttachTextToPath {
                host: host.clone(),
                story_id: story_id.to_string(),
                spec: spec.clone(),
            },
            inverse: Operation::DetachTextFromPath {
                host: host.clone(),
                index: Some(index),
                restore: None,
            },
            invalidation: InvalidationHint {
                structural: true,
                frame_geometry: vec![host.clone()],
                text_reflow: vec![NodeId::Story(story_id.to_string())],
                ..Default::default()
            },
        });
    }
    Err(OperationError::NodeNotFound(host.clone()))
}

/// C-29 — `Operation::DetachTextFromPath`: remove a `<TextPath>` link
/// from its host.
///
/// # Why removing the LINK is the faithful inverse
///
/// InDesign's "Delete Type from Path" deletes the text as well. That
/// is the right *gesture* but the wrong *inverse*: `AttachTextToPath`
/// takes an already-existing story and does nothing but link it, so
/// the exact undo of that is to unlink it — the story object is
/// restored to precisely the state it was in before (present in the
/// document, unflowed). Deleting the story would destroy content the
/// apply never created, and would need to carry the whole story in
/// the inverse to get it back. So: detach unlinks, the story survives,
/// and one Cmd-Z round-trips exactly.
///
/// `index` and `restore` are **inverse-only**: `index` names the slot
/// (hosts may carry more than one `<TextPath>`) and `restore` carries
/// the removed entry's knobs so redo re-creates it identically. A
/// user-initiated detach passes `None` for both and takes slot 0.
pub(super) fn apply_detach_text_from_path(
    doc: &mut paged_scene::Document,
    host: &NodeId,
    index: Option<usize>,
    restore: Option<&TextPathSpec>,
) -> Result<AppliedOperation, OperationError> {
    if !path_host_kind(host) {
        return Err(invalid(
            host,
            format!("C-29: a {} never hosts text-on-a-path", host.kind()),
        ));
    }
    for parsed in doc.spreads.iter_mut() {
        let spread = &mut parsed.spread;
        let Some(paths) = host_text_paths(spread, host) else {
            continue;
        };
        if paths.is_empty() {
            return Err(invalid(
                host,
                "C-29: the element hosts no text-on-a-path".into(),
            ));
        }
        let idx = index.unwrap_or(0);
        if idx >= paths.len() {
            return Err(invalid(
                host,
                format!("C-29: text-path index {idx} out of range ({})", paths.len()),
            ));
        }
        let removed = paths.remove(idx);
        // `restore` is honoured on REDO: the apply that produced this
        // op captured the knobs, and re-attaching must reproduce them
        // exactly. When absent we fall back to what we just removed
        // (the user-initiated path), which is the same values.
        let spec = restore
            .cloned()
            .unwrap_or_else(|| TextPathSpec::from_model(&removed));
        return Ok(AppliedOperation {
            op: Operation::DetachTextFromPath {
                host: host.clone(),
                index: Some(idx),
                restore: Some(TextPathSpec::from_model(&removed)),
            },
            inverse: Operation::AttachTextToPath {
                host: host.clone(),
                story_id: removed.parent_story.clone(),
                spec,
            },
            invalidation: InvalidationHint {
                structural: true,
                frame_geometry: vec![host.clone()],
                text_reflow: vec![NodeId::Story(removed.parent_story.clone())],
                ..Default::default()
            },
        });
    }
    Err(OperationError::NodeNotFound(host.clone()))
}
