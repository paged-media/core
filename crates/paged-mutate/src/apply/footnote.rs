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

//! v0.71.13 — inserting and deleting a footnote (ADR 163).
//!
//! A footnote is two things in the model, as InDesign writes it: its
//! reference, a [`FOOTNOTE_REFERENCE_MARKER`] character in its host
//! paragraph's text (the renderer prints the note's number there), and the
//! note itself, a [`paged_model::Footnote`] on that paragraph whose own
//! text opens with the same marker (the note's number) and the separator.
//! `InsertFootnote` writes both at a CONTIGUOUS story char offset (the
//! address space of `apply::character` and `apply::hyperlink`); every
//! anchor after it in the paragraph moves on by the one character.
//! `RemoveFootnote` takes both out again and keeps the note's paragraphs
//! for its inverse.

use paged_model::FOOTNOTE_REFERENCE_MARKER as MARK;
use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{AppliedOperation, NodeId, Operation};

use super::path_topology::reflow_hint_for_story;

pub(super) fn story_index(doc: &Document, story_id: &str) -> Result<usize, OperationError> {
    doc.stories
        .iter()
        .position(|s| s.self_id == story_id)
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Story(story_id.to_string())))
}

pub(super) fn chars(para: &paged_model::Paragraph) -> u32 {
    para.runs
        .iter()
        .map(|r| r.text.chars().count() as u32)
        .sum()
}

/// The paragraph holding contiguous story char `offset`, and the offset
/// within it; past the end, the last paragraph's end.
pub(super) fn locate(story: &paged_model::Story, offset: u32) -> Option<(usize, u32)> {
    let mut start = 0u32;
    for (i, para) in story.paragraphs.iter().enumerate() {
        let n = chars(para);
        if offset < start + n || i + 1 == story.paragraphs.len() {
            return Some((i, offset.saturating_sub(start).min(n)));
        }
        start += n;
    }
    None
}

pub(super) fn paragraph_start(story: &paged_model::Story, index: usize) -> u32 {
    story.paragraphs[..index].iter().map(chars).sum()
}

/// Insert `ch` at paragraph-local char `at`, into the run that holds it
/// (or ends there), so it takes that run's formatting.
pub(super) fn insert_char(para: &mut paged_model::Paragraph, at: u32, ch: char) {
    let mut start = 0u32;
    let last = para.runs.len().saturating_sub(1);
    for (i, run) in para.runs.iter_mut().enumerate() {
        let n = run.text.chars().count() as u32;
        if at < start + n || i == last {
            let local = (at - start).min(n) as usize;
            let byte = run
                .text
                .char_indices()
                .nth(local)
                .map_or(run.text.len(), |(b, _)| b);
            run.text.insert(byte, ch);
            return;
        }
        start += n;
    }
    para.runs.push(paged_model::CharacterRun {
        text: ch.to_string(),
        ..Default::default()
    });
}

/// Remove the character at paragraph-local char `at`.
pub(super) fn remove_char(para: &mut paged_model::Paragraph, at: u32) {
    let mut start = 0u32;
    for run in para.runs.iter_mut() {
        let n = run.text.chars().count() as u32;
        if at < start + n {
            let (byte, _) = run.text.char_indices().nth((at - start) as usize).unwrap();
            run.text.remove(byte);
            return;
        }
        start += n;
    }
}

/// The paragraph-local char of each reference marker, in order.
fn marker_chars(para: &paged_model::Paragraph) -> Vec<u32> {
    para.runs
        .iter()
        .flat_map(|r| r.text.chars())
        .enumerate()
        .filter(|(_, c)| *c == MARK)
        .map(|(i, _)| i as u32)
        .collect()
}

/// The note's own text, as InDesign makes one: its number (the marker),
/// the document's separator, the text.
fn note_paragraphs(doc: &Document, text: &str) -> Vec<paged_model::Paragraph> {
    let separator = doc
        .designmap
        .footnote_options
        .separator_text
        .clone()
        .unwrap_or_else(|| "\t".to_string());
    vec![paged_model::Paragraph {
        runs: vec![paged_model::CharacterRun {
            text: format!("{MARK}{separator}{text}"),
            ..Default::default()
        }],
        ..Default::default()
    }]
}

fn find_note(doc: &Document, footnote_id: &str) -> bool {
    doc.stories.iter().any(|s| {
        s.story
            .paragraphs
            .iter()
            .flat_map(|p| &p.footnotes)
            .any(|f| f.self_id.as_deref() == Some(footnote_id))
    })
}

pub(super) fn apply_insert_footnote(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    paragraph: Option<u32>,
    footnote_id: &str,
    text: &str,
    restore: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    if find_note(doc, footnote_id) {
        return Err(OperationError::DuplicateNodeId {
            id: footnote_id.to_string(),
        });
    }
    let paragraphs = match restore {
        Some(json) => serde_json::from_str(json).map_err(|e| OperationError::InvalidParent {
            parent: NodeId::Story(story_id.to_string()),
            child_kind: format!("footnote whose saved text does not read ({e})"),
        })?,
        None => note_paragraphs(doc, text),
    };
    let si = story_index(doc, story_id)?;
    let story = &mut doc.stories[si].story;
    let (pi, at) = match paragraph {
        // The paragraph named outright: `offset` clamped into it.
        Some(index) if (index as usize) < story.paragraphs.len() => {
            let index = index as usize;
            let start = paragraph_start(story, index);
            (
                index,
                offset
                    .saturating_sub(start)
                    .min(chars(&story.paragraphs[index])),
            )
        }
        Some(index) => {
            return Err(OperationError::InvalidPosition {
                parent: NodeId::Story(story_id.to_string()),
                position: index as usize,
                len: story.paragraphs.len(),
            })
        }
        None => locate(story, offset)
            .ok_or_else(|| OperationError::NodeNotFound(NodeId::Story(story_id.to_string())))?,
    };
    let para = &mut story.paragraphs[pi];
    para.shift_anchors(at, 1);
    insert_char(para, at, MARK);
    para.footnotes.push(paged_model::Footnote {
        self_id: Some(footnote_id.to_string()),
        anchor: Some(at),
        paragraphs,
    });
    para.footnotes.sort_by_key(|f| f.anchor.unwrap_or(0));
    Ok(AppliedOperation {
        op: Operation::InsertFootnote {
            story_id: story_id.to_string(),
            offset,
            paragraph,
            footnote_id: footnote_id.to_string(),
            text: text.to_string(),
            restore: restore.map(str::to_string),
        },
        inverse: Operation::RemoveFootnote {
            story_id: story_id.to_string(),
            footnote_id: footnote_id.to_string(),
        },
        invalidation: reflow_hint_for_story(doc, story_id),
    })
}

pub(super) fn apply_remove_footnote(
    doc: &mut Document,
    story_id: &str,
    footnote_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let missing = || OperationError::CollectionEntryNotFound {
        collection: "footnotes".to_string(),
        id: footnote_id.to_string(),
    };
    let si = story_index(doc, story_id)?;
    let story = &mut doc.stories[si].story;
    let (pi, k) = story
        .paragraphs
        .iter()
        .enumerate()
        .find_map(|(pi, p)| {
            p.footnotes
                .iter()
                .position(|f| f.self_id.as_deref() == Some(footnote_id))
                .map(|k| (pi, k))
        })
        .ok_or_else(missing)?;
    let start = paragraph_start(story, pi);
    let para = &mut story.paragraphs[pi];
    // The note's reference: the k-th marker when the paragraph holds one
    // per note (what the renderer reads), its anchor otherwise.
    let markers = marker_chars(para);
    let at = if markers.len() == para.footnotes.len() {
        markers[k]
    } else {
        para.footnotes[k].anchor.unwrap_or(0)
    };
    let note = para.footnotes.remove(k);
    if markers.contains(&at) {
        remove_char(para, at);
        para.shift_anchors(at, -1);
    }
    let restore = serde_json::to_string(&note.paragraphs).ok();
    Ok(AppliedOperation {
        op: Operation::RemoveFootnote {
            story_id: story_id.to_string(),
            footnote_id: footnote_id.to_string(),
        },
        inverse: Operation::InsertFootnote {
            story_id: story_id.to_string(),
            offset: start + at,
            paragraph: Some(pi as u32),
            footnote_id: footnote_id.to_string(),
            text: String::new(),
            restore,
        },
        invalidation: reflow_hint_for_story(doc, story_id),
    })
}
