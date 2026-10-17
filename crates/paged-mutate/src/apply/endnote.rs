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

//! v73 — endnotes inserted and deleted, and the endnote story a first
//! endnote makes (ADR 174), as InDesign 20.0.1 does it (`endnote-probe`).
//!
//! An endnote is two things in the model (ADR 170): its reference, an
//! [`REF`] character in the host text with its
//! [`paged_model::EndnoteReference`], and its range in the endnote story,
//! `\u{FEFF}` + the number marker [`NUM`] + the separator + the text +
//! `\u{FEFF}`, with its [`paged_model::EndnoteRangeDef`]. The endnote
//! story keeps the ranges in reference order, each after a break from the
//! one before; InDesign inserts a new note's range after the range of
//! the reference before it, and deleting a note takes its range and the
//! break before it, leaving the title.
//!
//! The first endnote of a document makes the endnote story: InDesign adds
//! a page at the end holding an `<EndnoteTextFrame>` over its margin box,
//! the story in it holding the title (`CreateEndnoteStory`).

use paged_model::{
    EndnoteRangeDef, EndnoteReference, ENDNOTE_NUMBER_MARKER as NUM,
    ENDNOTE_REFERENCE_MARKER as REF,
};
use paged_scene::Document;
use serde::{Deserialize, Serialize};

use crate::error::OperationError;
use crate::operation::{AppliedOperation, NodeId, NodeSpec, Operation};

use super::footnote::{chars, insert_char, locate, paragraph_start, remove_char, story_index};
use super::path_topology::reflow_hint_for_story;

/// The byte-order mark InDesign opens and closes an endnote's range with.
const BOM: char = '\u{FEFF}';

/// What `DeleteEndnote` keeps for its inverse.
#[derive(Serialize, Deserialize)]
struct Captured {
    reference: EndnoteReference,
    range: EndnoteRangeDef,
    /// The range's text: the opening mark through the closing one.
    text: String,
}

fn missing(endnote_id: &str) -> OperationError {
    OperationError::CollectionEntryNotFound {
        collection: "endnotes".to_string(),
        id: endnote_id.to_string(),
    }
}

/// The document's endnote story (the first one marked so).
pub(crate) fn endnote_story(doc: &Document) -> Option<usize> {
    doc.stories.iter().position(|s| s.story.is_endnote_story)
}

fn text_of(para: &paged_model::Paragraph) -> Vec<char> {
    para.runs.iter().flat_map(|r| r.text.chars()).collect()
}

/// The paragraph-local chars of `mark` in `para`, in order.
fn marks(para: &paged_model::Paragraph, mark: char) -> Vec<u32> {
    text_of(para)
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == mark)
        .map(|(i, _)| i as u32)
        .collect()
}

/// Insert `s` at paragraph-local char `at`, into the run holding it (or
/// ending there), moving every anchor after it on.
fn insert_str(para: &mut paged_model::Paragraph, at: u32, s: &str) {
    para.shift_anchors(at, s.chars().count() as i64);
    for (i, ch) in s.chars().enumerate() {
        insert_char(para, at + i as u32, ch);
    }
}

/// Remove paragraph-local chars `[from, to)`.
fn remove_chars(para: &mut paged_model::Paragraph, from: u32, to: u32) {
    for _ in from..to {
        remove_char(para, from);
    }
    para.shift_anchors(from, -i64::from(to - from));
}

/// One range in the endnote story: its paragraph, its order there, and
/// its chars `[start, end)` (the opening mark through the closing one).
struct RangeAt {
    paragraph: usize,
    k: usize,
    start: u32,
    end: u32,
}

/// The endnote story's ranges, in story order, by their number markers.
fn ranges(story: &paged_model::Story) -> Vec<(RangeAt, Option<EndnoteRangeDef>)> {
    let mut out = Vec::new();
    for (pi, para) in story.paragraphs.iter().enumerate() {
        let text = text_of(para);
        for (k, &m) in marks(para, NUM).iter().enumerate() {
            let m = m as usize;
            let start = if m > 0 && text[m - 1] == BOM {
                m - 1
            } else {
                m
            };
            let end = text[m + 1..]
                .iter()
                .position(|c| *c == BOM)
                .map_or(text.len(), |p| m + 1 + p + 1);
            out.push((
                RangeAt {
                    paragraph: pi,
                    k,
                    start: start as u32,
                    end: end as u32,
                },
                para.endnote_ranges.get(k).cloned(),
            ));
        }
    }
    out
}

/// Every endnote reference in reference order (stories in document order),
/// with its `Self`.
fn references_in_order(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    for s in &doc.stories {
        for para in &s.story.paragraphs {
            let n = marks(para, REF).len();
            out.extend(
                para.endnote_references
                    .iter()
                    .take(n)
                    .map(|r| r.self_id.clone()),
            );
        }
    }
    out
}

/// The note's range text as InDesign writes it: the opening mark, the
/// number, the separator, the text, the closing mark.
fn range_text(doc: &Document, text: &str) -> String {
    let separator = doc
        .designmap
        .endnote_options
        .as_ref()
        .and_then(|o| o.separator_text.clone())
        .unwrap_or_else(|| "\t".to_string());
    format!("{BOM}{NUM}{separator}{text}{BOM}")
}

/// Put `range` (its text `text`) into the endnote story after the range
/// of the reference before `endnote_id` in reference order, or first,
/// after the title.
fn place_range(
    doc: &mut Document,
    si: usize,
    endnote_id: &str,
    range: EndnoteRangeDef,
    text: &str,
) {
    let order = references_in_order(doc);
    let at = order.iter().position(|id| id == endnote_id).unwrap_or(0);
    let story = &mut doc.stories[si].story;
    let existing = ranges(story);
    let before = order[..at].iter().rev().find_map(|id| {
        existing
            .iter()
            .find(|(_, def)| def.as_ref().and_then(|d| d.source.as_deref()) == Some(id))
    });
    match before {
        Some((r, _)) => {
            let para = &mut story.paragraphs[r.paragraph];
            insert_str(para, r.end, &format!("\n{text}"));
            para.endnote_ranges.insert(r.k + 1, range);
        }
        None => match existing.first() {
            // Before the first range: in the break that opens it.
            Some((first, _)) => {
                let para = &mut story.paragraphs[first.paragraph];
                let at = first.start.saturating_sub(1);
                insert_str(para, at, &format!("\n{text}"));
                para.endnote_ranges.insert(0, range);
            }
            // The first note: after the title.
            None => {
                let last = story.paragraphs.len().saturating_sub(1);
                if story.paragraphs.is_empty() {
                    story.paragraphs.push(paged_model::Paragraph::default());
                }
                let para = &mut story.paragraphs[last];
                let end = chars(para);
                let lead = if end > 0 { "\n" } else { "" };
                insert_str(para, end, &format!("{lead}{text}"));
                para.endnote_ranges.push(range);
            }
        },
    }
}

pub(super) struct NewEndnote<'a> {
    pub endnote_id: &'a str,
    pub range_id: &'a str,
    pub text: &'a str,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_insert_endnote(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    paragraph: Option<u32>,
    new: NewEndnote<'_>,
    restore: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    let NewEndnote {
        endnote_id,
        range_id,
        text,
    } = new;
    let exists = doc.stories.iter().any(|s| {
        s.story
            .paragraphs
            .iter()
            .flat_map(|p| &p.endnote_references)
            .any(|r| r.self_id == endnote_id)
    });
    if exists {
        return Err(OperationError::DuplicateNodeId {
            id: endnote_id.to_string(),
        });
    }
    let ei = endnote_story(doc).ok_or_else(|| OperationError::InvalidParent {
        parent: NodeId::Story(story_id.to_string()),
        child_kind: "endnote in a document with no endnote story".to_string(),
    })?;
    let (reference, range, body) = match restore {
        Some(json) => {
            let c: Captured =
                serde_json::from_str(json).map_err(|e| OperationError::InvalidParent {
                    parent: NodeId::Story(story_id.to_string()),
                    child_kind: format!("endnote whose saved state does not read ({e})"),
                })?;
            (c.reference, c.range, c.text)
        }
        None => (
            EndnoteReference {
                self_id: endnote_id.to_string(),
                text_range: Some(range_id.to_string()),
            },
            EndnoteRangeDef {
                self_id: range_id.to_string(),
                source: Some(endnote_id.to_string()),
            },
            range_text(doc, text),
        ),
    };
    // The reference, at its place in the host text.
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
    let k = marks(para, REF).iter().filter(|&&m| m < at).count();
    para.shift_anchors(at, 1);
    insert_char(para, at, REF);
    let k = k.min(para.endnote_references.len());
    para.endnote_references.insert(k, reference);
    // Its range, in the endnote story.
    place_range(doc, ei, endnote_id, range, &body);
    let endnote_story_id = doc.stories[ei].self_id.clone();
    let mut invalidation = reflow_hint_for_story(doc, story_id);
    invalidation.merge(reflow_hint_for_story(doc, &endnote_story_id));
    Ok(AppliedOperation {
        op: Operation::InsertEndnote {
            story_id: story_id.to_string(),
            offset,
            paragraph,
            endnote_id: endnote_id.to_string(),
            range_id: range_id.to_string(),
            text: text.to_string(),
            restore: restore.map(str::to_string),
        },
        inverse: Operation::DeleteEndnote {
            endnote_id: endnote_id.to_string(),
        },
        invalidation,
    })
}

pub(super) fn apply_delete_endnote(
    doc: &mut Document,
    endnote_id: &str,
) -> Result<AppliedOperation, OperationError> {
    // The reference: its story, paragraph and order there.
    let (si, pi, k) = doc
        .stories
        .iter()
        .enumerate()
        .find_map(|(si, s)| {
            s.story.paragraphs.iter().enumerate().find_map(|(pi, p)| {
                p.endnote_references
                    .iter()
                    .position(|r| r.self_id == endnote_id)
                    .map(|k| (si, pi, k))
            })
        })
        .ok_or_else(|| missing(endnote_id))?;
    let story_id = doc.stories[si].self_id.clone();
    let story = &mut doc.stories[si].story;
    let start = paragraph_start(story, pi);
    let para = &mut story.paragraphs[pi];
    let at = marks(para, REF).get(k).copied();
    let reference = para.endnote_references.remove(k);
    if let Some(at) = at {
        remove_char(para, at);
        para.shift_anchors(at, -1);
    }
    // Its range, with the break before it.
    let mut captured_range = None;
    let mut endnote_story_id = None;
    if let Some(ei) = endnote_story(doc) {
        endnote_story_id = Some(doc.stories[ei].self_id.clone());
        let story = &mut doc.stories[ei].story;
        let found = ranges(story)
            .into_iter()
            .find(|(_, def)| def.as_ref().and_then(|d| d.source.as_deref()) == Some(endnote_id));
        if let Some((r, Some(def))) = found {
            let para = &mut story.paragraphs[r.paragraph];
            let text: String = text_of(para)[r.start as usize..r.end as usize]
                .iter()
                .collect();
            let all = text_of(para);
            let (from, to) = if r.start > 0 && all[r.start as usize - 1] == '\n' {
                (r.start - 1, r.end)
            } else if all.get(r.end as usize) == Some(&'\n') {
                (r.start, r.end + 1)
            } else {
                (r.start, r.end)
            };
            remove_chars(para, from, to);
            para.endnote_ranges.remove(r.k);
            captured_range = Some((def, text));
        }
    }
    let (range, text) = captured_range.unwrap_or_else(|| {
        (
            EndnoteRangeDef {
                self_id: reference.text_range.clone().unwrap_or_default(),
                source: Some(endnote_id.to_string()),
            },
            String::new(),
        )
    });
    let range_id = range.self_id.clone();
    let restore = serde_json::to_string(&Captured {
        reference,
        range,
        text,
    })
    .ok();
    let mut invalidation = reflow_hint_for_story(doc, &story_id);
    if let Some(id) = endnote_story_id {
        invalidation.merge(reflow_hint_for_story(doc, &id));
    }
    Ok(AppliedOperation {
        op: Operation::DeleteEndnote {
            endnote_id: endnote_id.to_string(),
        },
        inverse: Operation::InsertEndnote {
            story_id,
            offset: start + at.unwrap_or(0),
            paragraph: Some(pi as u32),
            endnote_id: endnote_id.to_string(),
            range_id,
            text: String::new(),
            restore,
        },
        invalidation,
    })
}

/// The ids `CreateEndnoteStory` makes the endnote story with.
pub(super) struct EndnoteStoryIds<'a> {
    pub spread_id: Option<&'a str>,
    pub page_id: Option<&'a str>,
    pub frame_id: &'a str,
    pub story_id: &'a str,
}

/// The endnote story a first endnote makes, as InDesign 20.0.1 makes it
/// (`endnote-probe`): a page added at the end (on the last page's master),
/// an `<EndnoteTextFrame>` over its margin box, and the story in it holding
/// the title (`EndnoteTitle`, in `EndnoteTitleStyle`). Inverse: the
/// frame's removal (which takes the story it made) and the page's.
pub(super) fn apply_create_endnote_story(
    doc: &mut Document,
    ids: EndnoteStoryIds<'_>,
) -> Result<AppliedOperation, OperationError> {
    if let Some(ei) = endnote_story(doc) {
        return Err(OperationError::DuplicateNodeId {
            id: doc.stories[ei].self_id.clone(),
        });
    }
    let last_page = doc
        .spreads
        .iter()
        .flat_map(|p| p.spread.pages.iter())
        .next_back()
        .cloned();
    let master = last_page.as_ref().and_then(|p| p.applied_master.clone());
    let after = last_page.as_ref().and_then(|p| p.self_id.clone());
    let page = super::batch_page::apply_insert_page(
        doc,
        after.as_deref(),
        master.as_deref(),
        ids.spread_id.map(str::to_string),
        ids.page_id.map(str::to_string),
        None,
    )?;
    let Operation::InsertPage {
        spread_self_id: Some(spread_id),
        page_self_id: Some(page_id),
        ..
    } = &page.op
    else {
        return Err(OperationError::InvalidParent {
            parent: NodeId::Document(String::new()),
            child_kind: "endnote page".to_string(),
        });
    };
    let (spread_id, page_id) = (spread_id.clone(), page_id.clone());
    // The frame: the new page's margin box, in spread coordinates.
    let (bounds, margins) = {
        let spread = &doc
            .spreads
            .iter()
            .find(|p| p.spread.self_id.as_deref() == Some(spread_id.as_str()))
            .expect("the page just inserted")
            .spread;
        let page = spread.pages.first().expect("one page");
        (page.bounds, spread.page_margins.get(&page_id).cloned())
    };
    let m = margins.unwrap_or_default();
    let frame_bounds = [
        bounds.top + m.top,
        bounds.left + m.left,
        bounds.bottom - m.bottom,
        bounds.right - m.right,
    ];
    let frame = super::insert_node::apply_insert_node(
        doc,
        &NodeId::Spread(spread_id.clone()),
        0,
        None,
        &NodeSpec::TextFrame {
            self_id: ids.frame_id.to_string(),
            bounds: frame_bounds,
            fill_color: None,
            stroke_color: None,
            stroke_weight: None,
            item_transform: None,
            parent_story: Some(ids.story_id.to_string()),
        },
    )?;
    // The story: InDesign's endnote story, holding the title.
    let options = doc.designmap.endnote_options.clone().unwrap_or_default();
    if let Some(s) = doc.stories.iter_mut().find(|s| s.self_id == ids.story_id) {
        s.story.is_endnote_story = true;
        s.story.paragraphs = vec![paged_model::Paragraph {
            paragraph_style: options.title_style.clone(),
            runs: vec![paged_model::CharacterRun {
                text: options
                    .title
                    .clone()
                    .unwrap_or_else(|| "Endnotes".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        }];
    }
    if let Some(p) = doc
        .spreads
        .iter_mut()
        .find(|p| p.spread.self_id.as_deref() == Some(spread_id.as_str()))
    {
        p.spread.endnote_frames.push(ids.frame_id.to_string());
    }
    let mut invalidation = page.invalidation;
    invalidation.merge(frame.invalidation);
    Ok(AppliedOperation {
        op: Operation::CreateEndnoteStory {
            spread_id: Some(spread_id),
            page_id: Some(page_id),
            frame_id: ids.frame_id.to_string(),
            story_id: ids.story_id.to_string(),
        },
        inverse: Operation::Batch {
            ops: vec![frame.inverse, page.inverse],
        },
        invalidation,
    })
}
