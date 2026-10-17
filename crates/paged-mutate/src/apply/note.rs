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

//! v72 — editorial notes inserted, edited and deleted (ADR 172).
//!
//! A note is two things in the model, as InDesign keeps it: its place, a
//! [`NOTE_MARKER`] character of its paragraph's text (InDesign's U+FEFF,
//! which the story's length counts), and the [`paged_model::Note`] on that
//! paragraph. `InsertNote` writes both at a CONTIGUOUS story char offset
//! (the `InsertFootnote` address space); every anchor after it in the
//! paragraph moves on by the one character. `DeleteNote` takes both out
//! and keeps the note for its inverse.

use paged_model::NOTE_MARKER as MARK;
use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation};

use super::footnote::{chars, insert_char, locate, paragraph_start, remove_char, story_index};
use super::path_topology::reflow_hint_for_story;

/// What `InsertNote` writes into a new note.
pub(super) struct NewNote<'a> {
    pub text: &'a str,
    pub user: Option<&'a str>,
    pub date: Option<&'a str>,
}

/// The story, paragraph and index in it of note `note_id`.
fn find_note(doc: &Document, note_id: &str) -> Option<(usize, usize, usize)> {
    doc.stories.iter().enumerate().find_map(|(si, s)| {
        s.story.paragraphs.iter().enumerate().find_map(|(pi, p)| {
            p.notes
                .iter()
                .position(|n| n.id == note_id)
                .map(|k| (si, pi, k))
        })
    })
}

fn missing(note_id: &str) -> OperationError {
    OperationError::CollectionEntryNotFound {
        collection: "notes".to_string(),
        id: note_id.to_string(),
    }
}

/// The paragraph-local char of each note's place, in order.
fn marker_chars(para: &paged_model::Paragraph) -> Vec<u32> {
    para.runs
        .iter()
        .flat_map(|r| r.text.chars())
        .enumerate()
        .filter(|(_, c)| *c == MARK)
        .map(|(i, _)| i as u32)
        .collect()
}

/// Where note `k` of `para` stands: its marker when the paragraph holds
/// one per note (what the renderer and the writer read), its anchor
/// otherwise.
pub(crate) fn note_place(para: &paged_model::Paragraph, k: usize) -> u32 {
    let markers = marker_chars(para);
    if markers.len() == para.notes.len() {
        markers[k]
    } else {
        para.notes[k].anchor
    }
}

pub(super) fn apply_insert_note(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    paragraph: Option<u32>,
    note_id: &str,
    new: NewNote<'_>,
    restore: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    if find_note(doc, note_id).is_some() {
        return Err(OperationError::DuplicateNodeId {
            id: note_id.to_string(),
        });
    }
    let note = match restore {
        Some(json) => serde_json::from_str::<paged_model::Note>(json).map_err(|e| {
            OperationError::InvalidParent {
                parent: NodeId::Story(story_id.to_string()),
                child_kind: format!("note whose saved state does not read ({e})"),
            }
        })?,
        None => paged_model::Note {
            id: note_id.to_string(),
            text: new.text.to_string(),
            user_name: new.user.map(str::to_string),
            creation_date: new.date.map(str::to_string),
            modification_date: new.date.map(str::to_string),
            ..Default::default()
        },
    };
    let si = story_index(doc, story_id)?;
    let story = &mut doc.stories[si].story;
    let (pi, at) = match paragraph {
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
    para.notes.push(paged_model::Note {
        id: note_id.to_string(),
        anchor: at,
        ..note
    });
    para.notes.sort_by_key(|n| n.anchor);
    Ok(AppliedOperation {
        op: Operation::InsertNote {
            story_id: story_id.to_string(),
            offset,
            paragraph,
            note_id: note_id.to_string(),
            text: new.text.to_string(),
            user: new.user.map(str::to_string),
            date: new.date.map(str::to_string),
            restore: restore.map(str::to_string),
        },
        inverse: Operation::DeleteNote {
            note_id: note_id.to_string(),
        },
        invalidation: reflow_hint_for_story(doc, story_id),
    })
}

pub(super) fn apply_edit_note(
    doc: &mut Document,
    note_id: &str,
    text: &str,
    modification_date: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let (si, pi, k) = find_note(doc, note_id).ok_or_else(|| missing(note_id))?;
    let note = &mut doc.stories[si].story.paragraphs[pi].notes[k];
    let old_text = std::mem::replace(&mut note.text, text.to_string());
    let old_date = std::mem::replace(
        &mut note.modification_date,
        modification_date.map(str::to_string),
    );
    Ok(AppliedOperation {
        op: Operation::EditNote {
            note_id: note_id.to_string(),
            text: text.to_string(),
            modification_date: modification_date.map(str::to_string),
        },
        inverse: Operation::EditNote {
            note_id: note_id.to_string(),
            text: old_text,
            modification_date: old_date,
        },
        // A note prints nothing: its text moves no line.
        invalidation: InvalidationHint::default(),
    })
}

pub(super) fn apply_delete_note(
    doc: &mut Document,
    note_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let (si, pi, k) = find_note(doc, note_id).ok_or_else(|| missing(note_id))?;
    let story_id = doc.stories[si].self_id.clone();
    let story = &mut doc.stories[si].story;
    let start = paragraph_start(story, pi);
    let para = &mut story.paragraphs[pi];
    let at = note_place(para, k);
    let has_marker = marker_chars(para).contains(&at);
    let note = para.notes.remove(k);
    if has_marker {
        remove_char(para, at);
        para.shift_anchors(at, -1);
    }
    let restore = serde_json::to_string(&note).ok();
    Ok(AppliedOperation {
        op: Operation::DeleteNote {
            note_id: note_id.to_string(),
        },
        inverse: Operation::InsertNote {
            story_id: story_id.clone(),
            offset: start + at,
            paragraph: Some(pi as u32),
            note_id: note_id.to_string(),
            text: String::new(),
            user: None,
            date: None,
            restore,
        },
        invalidation: reflow_hint_for_story(doc, &story_id),
    })
}
