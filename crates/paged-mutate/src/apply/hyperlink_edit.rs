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

//! v73 — deleting and editing a hyperlink of any origin (ADR 164).
//!
//! `InsertHyperlink`'s own inverse knows the span and URL it made; a link
//! read from an InDesign document carries neither on an op. These doors
//! address a link by its `Hyperlink` `Self` alone and work out the rest:
//! the runs its source tags (in any story), and whether its destination is
//! its own. Both are undone by [`Operation::RestoreHyperlink`], which puts
//! a captured [`HyperlinkState`] back: the link record where it stood, the
//! destinations as they were, and the source tag over the same runs.
//!
//! A destination goes with its link only when nothing else points at it
//! (no other hyperlink, bookmark or cross-reference) and it is a URL or
//! page destination. A text destination is an anchor in its story's text
//! and outlives every link to it, as in InDesign.

use paged_model::{Hyperlink, HyperlinkDestination, HyperlinkDestinationKind};
use paged_scene::Document;
use serde::{Deserialize, Serialize};

use crate::error::OperationError;
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation};

use super::hyperlink::{frame_style_hint, tag_source_over_range};

/// One hyperlink as it stood: what [`Operation::RestoreHyperlink`] puts
/// back, and what it captures for its own inverse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HyperlinkState {
    pub hyperlink_id: String,
    /// The link's source (`HyperlinkTextSource/…`): the run tag.
    pub source_id: Option<String>,
    /// The record and its index in the designmap; `None` when the link
    /// does not exist in this state.
    pub hyperlink: Option<(usize, Hyperlink)>,
    /// Each destination this state pins: its record and index, or `None`
    /// when it does not exist in this state.
    pub destinations: Vec<(String, Option<(usize, HyperlinkDestination)>)>,
    /// The runs the source tags, per story, as contiguous char ranges.
    /// Only read when `hyperlink` is present.
    pub spans: Vec<(String, u32, u32)>,
}

fn not_found(id: &str) -> OperationError {
    OperationError::CollectionEntryNotFound {
        collection: "hyperlinks".to_string(),
        id: id.to_string(),
    }
}

/// Every run `source_id` tags, as `(story, start, end)` contiguous char
/// ranges, adjacent runs merged.
fn source_spans(doc: &Document, source_id: &str) -> Vec<(String, u32, u32)> {
    let mut out: Vec<(String, u32, u32)> = Vec::new();
    for parsed in &doc.stories {
        let mut offset = 0u32;
        for para in &parsed.story.paragraphs {
            for run in &para.runs {
                let len = run.text.chars().count() as u32;
                if run.hyperlink_source.as_deref() == Some(source_id) && len > 0 {
                    match out.last_mut() {
                        Some((story, _, end)) if *story == parsed.self_id && *end == offset => {
                            *end = offset + len;
                        }
                        _ => out.push((parsed.self_id.clone(), offset, offset + len)),
                    }
                }
                offset += len;
            }
        }
    }
    out
}

/// Does anything other than hyperlink `except` point at `dest_id`?
fn destination_shared(doc: &Document, dest_id: &str, except: &str) -> bool {
    let dm = &doc.designmap;
    dm.hyperlinks
        .iter()
        .any(|h| h.self_id != except && h.destination.as_deref() == Some(dest_id))
        || dm
            .bookmarks
            .iter()
            .any(|b| b.destination.as_deref() == Some(dest_id))
        || dm
            .cross_references
            .iter()
            .any(|x| x.destination.as_deref() == Some(dest_id))
}

fn destination_entry(doc: &Document, dest_id: &str) -> Option<(usize, HyperlinkDestination)> {
    doc.designmap
        .hyperlink_destinations
        .iter()
        .position(|d| d.self_id == dest_id)
        .map(|i| (i, doc.designmap.hyperlink_destinations[i].clone()))
}

/// The current state of `hyperlink_id` over the destinations `dest_ids`.
fn capture(
    doc: &Document,
    hyperlink_id: &str,
    source_id: Option<&str>,
    dest_ids: &[String],
) -> HyperlinkState {
    let hyperlink = doc
        .designmap
        .hyperlinks
        .iter()
        .position(|h| h.self_id == hyperlink_id)
        .map(|i| (i, doc.designmap.hyperlinks[i].clone()));
    let spans = match (&hyperlink, source_id) {
        (Some(_), Some(src)) => source_spans(doc, src),
        _ => Vec::new(),
    };
    HyperlinkState {
        hyperlink_id: hyperlink_id.to_string(),
        source_id: source_id.map(str::to_string),
        hyperlink,
        destinations: dest_ids
            .iter()
            .map(|d| (d.clone(), destination_entry(doc, d)))
            .collect(),
        spans,
    }
}

fn hint_for_spans(doc: &Document, spans: &[(String, u32, u32)]) -> InvalidationHint {
    let mut hint = InvalidationHint::default();
    for (story, _, _) in spans {
        for node in frame_style_hint(doc, story).frame_style {
            if !hint.frame_style.contains(&node) {
                hint.frame_style.push(node);
            }
        }
    }
    hint
}

fn untag(doc: &mut Document, source_id: &str) {
    for parsed in doc.stories.iter_mut() {
        for para in parsed.story.paragraphs.iter_mut() {
            for run in para.runs.iter_mut() {
                if run.hyperlink_source.as_deref() == Some(source_id) {
                    run.hyperlink_source = None;
                }
            }
        }
    }
}

/// Delete hyperlink `hyperlink_id`: its source runs stop linking, the
/// record goes, and so does its destination when nothing else needs it.
pub(super) fn apply_delete_hyperlink(
    doc: &mut Document,
    hyperlink_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let link = doc
        .designmap
        .hyperlinks
        .iter()
        .find(|h| h.self_id == hyperlink_id)
        .cloned()
        .ok_or_else(|| not_found(hyperlink_id))?;
    let dest_goes = link.destination.as_deref().filter(|d| {
        matches!(
            destination_entry(doc, d).map(|(_, e)| e.kind),
            Some(HyperlinkDestinationKind::Url(_) | HyperlinkDestinationKind::Page(_))
        ) && !destination_shared(doc, d, hyperlink_id)
    });
    let dest_ids: Vec<String> = dest_goes.map(str::to_string).into_iter().collect();
    let before = capture(doc, hyperlink_id, link.source.as_deref(), &dest_ids);

    if let Some(src) = link.source.as_deref() {
        untag(doc, src);
    }
    doc.designmap
        .hyperlinks
        .retain(|h| h.self_id != hyperlink_id);
    if let Some(d) = dest_goes {
        doc.designmap
            .hyperlink_destinations
            .retain(|x| x.self_id != d);
    }

    let invalidation = hint_for_spans(doc, &before.spans);
    Ok(AppliedOperation {
        op: Operation::DeleteHyperlink {
            hyperlink_id: hyperlink_id.to_string(),
        },
        inverse: Operation::RestoreHyperlink {
            state: serde_json::to_string(&before).unwrap_or_default(),
        },
        invalidation,
    })
}

/// Edit hyperlink `hyperlink_id`: its name, and where it goes. A new URL
/// or page rewrites the link's own destination of that kind in place;
/// otherwise the link moves to a new destination `dest_id`, and the one it
/// leaves goes by the delete rule (a shared one or a text anchor stays).
/// `page` wins over `url`, as in `InsertHyperlink`.
pub(super) fn apply_edit_hyperlink(
    doc: &mut Document,
    hyperlink_id: &str,
    name: Option<&str>,
    url: Option<&str>,
    page: Option<&str>,
    dest_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let link = doc
        .designmap
        .hyperlinks
        .iter()
        .find(|h| h.self_id == hyperlink_id)
        .cloned()
        .ok_or_else(|| not_found(hyperlink_id))?;
    if let Some(pid) = page {
        let exists = doc.spreads.iter().any(|parsed| {
            parsed
                .spread
                .pages
                .iter()
                .any(|p| p.self_id.as_deref() == Some(pid))
        });
        if !exists {
            return Err(OperationError::NodeNotFound(NodeId::Page(pid.to_string())));
        }
    }
    let new_kind = match (page, url) {
        (Some(pid), _) => Some(HyperlinkDestinationKind::Page(pid.to_string())),
        (None, Some(u)) => Some(HyperlinkDestinationKind::Url(u.to_string())),
        (None, None) => None,
    };

    // Where the new destination goes: the link's own (same kind, not
    // shared) in place, or a fresh one under `dest_id`.
    let current = link
        .destination
        .as_deref()
        .and_then(|d| destination_entry(doc, d));
    let in_place = match (&new_kind, &current) {
        (Some(kind), Some((_, cur))) => {
            std::mem::discriminant(kind) == std::mem::discriminant(&cur.kind)
                && !matches!(cur.kind, HyperlinkDestinationKind::TextAnchor(_))
                && !destination_shared(doc, &cur.self_id, hyperlink_id)
        }
        _ => false,
    };
    let mut dest_ids: Vec<String> = Vec::new();
    if new_kind.is_some() {
        if in_place {
            if let Some((_, cur)) = &current {
                dest_ids.push(cur.self_id.clone());
            }
        } else {
            if destination_entry(doc, dest_id).is_some() {
                return Err(OperationError::DuplicateNodeId {
                    id: dest_id.to_string(),
                });
            }
            dest_ids.push(dest_id.to_string());
        }
    }
    // The destination the link leaves goes when it was the link's own (the
    // delete rule): nothing else points at it and it is a URL or a page.
    let left_behind = current
        .as_ref()
        .filter(|(_, cur)| {
            !in_place
                && new_kind.is_some()
                && matches!(
                    cur.kind,
                    HyperlinkDestinationKind::Url(_) | HyperlinkDestinationKind::Page(_)
                )
                && !destination_shared(doc, &cur.self_id, hyperlink_id)
        })
        .map(|(_, cur)| cur.self_id.clone());
    dest_ids.extend(left_behind.iter().cloned());
    let before = capture(doc, hyperlink_id, None, &dest_ids);

    if let Some(kind) = new_kind {
        if in_place {
            let cur_id = current.map(|(_, c)| c.self_id).unwrap_or_default();
            if let Some(d) = doc
                .designmap
                .hyperlink_destinations
                .iter_mut()
                .find(|d| d.self_id == cur_id)
            {
                d.kind = kind;
            }
        } else {
            doc.designmap
                .hyperlink_destinations
                .push(HyperlinkDestination {
                    self_id: dest_id.to_string(),
                    kind,
                });
            if let Some(h) = doc
                .designmap
                .hyperlinks
                .iter_mut()
                .find(|h| h.self_id == hyperlink_id)
            {
                h.destination = Some(dest_id.to_string());
            }
            if let Some(old) = &left_behind {
                doc.designmap
                    .hyperlink_destinations
                    .retain(|d| &d.self_id != old);
            }
        }
    }
    if let Some(n) = name {
        if let Some(h) = doc
            .designmap
            .hyperlinks
            .iter_mut()
            .find(|h| h.self_id == hyperlink_id)
        {
            h.name = (!n.is_empty()).then(|| n.to_string());
        }
    }

    Ok(AppliedOperation {
        op: Operation::EditHyperlink {
            hyperlink_id: hyperlink_id.to_string(),
            name: name.map(str::to_string),
            url: url.map(str::to_string),
            page: page.map(str::to_string),
            dest_id: dest_id.to_string(),
        },
        inverse: Operation::RestoreHyperlink {
            state: serde_json::to_string(&before).unwrap_or_default(),
        },
        // A destination is not drawn: nothing to repaint.
        invalidation: InvalidationHint::default(),
    })
}

/// Inverse-only: put a captured [`HyperlinkState`] back. Its own inverse
/// is the state it replaced, over the same keys.
pub(super) fn apply_restore_hyperlink(
    doc: &mut Document,
    state_json: &str,
) -> Result<AppliedOperation, OperationError> {
    let state: HyperlinkState =
        serde_json::from_str(state_json).map_err(|e| OperationError::CollectionEntryNotFound {
            collection: "hyperlinks (captured link does not decode)".to_string(),
            id: e.to_string(),
        })?;
    let dest_ids: Vec<String> = state.destinations.iter().map(|(d, _)| d.clone()).collect();
    // The source the current link carries, so undoing a restore untags it.
    let source_now = doc
        .designmap
        .hyperlinks
        .iter()
        .find(|h| h.self_id == state.hyperlink_id)
        .and_then(|h| h.source.clone())
        .or_else(|| state.source_id.clone());
    let now = capture(doc, &state.hyperlink_id, source_now.as_deref(), &dest_ids);

    // Destinations first: every pinned one out, then the present ones back
    // in ascending index order, which rebuilds the captured order exactly.
    let list = &mut doc.designmap.hyperlink_destinations;
    list.retain(|d| !dest_ids.contains(&d.self_id));
    let mut back: Vec<&(usize, HyperlinkDestination)> = state
        .destinations
        .iter()
        .filter_map(|(_, entry)| entry.as_ref())
        .collect();
    back.sort_by_key(|(index, _)| *index);
    for (index, record) in back {
        let at = (*index).min(list.len());
        list.insert(at, record.clone());
    }
    // Then the link record.
    let links = &mut doc.designmap.hyperlinks;
    links.retain(|h| h.self_id != state.hyperlink_id);
    if let Some((index, record)) = &state.hyperlink {
        let at = (*index).min(links.len());
        links.insert(at, record.clone());
    }
    // Then the source tag.
    match (&state.hyperlink, state.source_id.as_deref()) {
        (Some(_), Some(src)) => {
            untag(doc, src);
            for (story, start, end) in &state.spans {
                if let Some(parsed) = doc.stories.iter_mut().find(|s| &s.self_id == story) {
                    tag_source_over_range(&mut parsed.story, *start, *end, src);
                }
            }
        }
        (None, Some(src)) => untag(doc, src),
        _ => {}
    }

    let mut spans = state.spans.clone();
    spans.extend(now.spans.iter().cloned());
    let invalidation = hint_for_spans(doc, &spans);
    Ok(AppliedOperation {
        op: Operation::RestoreHyperlink {
            state: state_json.to_string(),
        },
        inverse: Operation::RestoreHyperlink {
            state: serde_json::to_string(&now).unwrap_or_default(),
        },
        invalidation,
    })
}
