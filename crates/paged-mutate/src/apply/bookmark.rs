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

//! v72 — bookmarks created, renamed, moved and deleted (ADR 173), as
//! InDesign 20.0.1 does it (`bookmarks-probe`).
//!
//! The document's bookmarks are a tree: `DesignMap::bookmarks` in document
//! order (a parent before its children, a subtree contiguous) and
//! `DesignMap::bookmark_parents`. Every op keeps that order, and captures
//! the bookmarks, their nesting and the destinations for its inverse,
//! `RestoreBookmarks`, so undo lands on the exact prior state.
//!
//! A bookmark to a page goes through a page destination of its own, as
//! InDesign makes one; deleting a bookmark deletes its children and the
//! page destinations nothing else goes to, and keeps a text destination.

use std::collections::BTreeMap;

use paged_model::{Bookmark, HyperlinkDestination, HyperlinkDestinationKind};
use paged_scene::Document;
use serde::{Deserialize, Serialize};

use crate::error::OperationError;
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation};

/// What `RestoreBookmarks` puts back.
#[derive(Serialize, Deserialize)]
struct Captured {
    bookmarks: Vec<Bookmark>,
    parents: BTreeMap<String, String>,
    destinations: Vec<HyperlinkDestination>,
}

fn capture(doc: &Document) -> String {
    let dm = &doc.designmap;
    serde_json::to_string(&Captured {
        bookmarks: dm.bookmarks.clone(),
        parents: dm.bookmark_parents.clone(),
        destinations: dm.hyperlink_destinations.clone(),
    })
    .expect("bookmarks serialize")
}

fn missing(bookmark_id: &str) -> OperationError {
    OperationError::CollectionEntryNotFound {
        collection: "bookmarks".to_string(),
        id: bookmark_id.to_string(),
    }
}

fn position(doc: &Document, bookmark_id: &str) -> Result<usize, OperationError> {
    doc.designmap
        .bookmarks
        .iter()
        .position(|b| b.self_id == bookmark_id)
        .ok_or_else(|| missing(bookmark_id))
}

/// True when `id` is `ancestor` or stands under it.
fn is_within(parents: &BTreeMap<String, String>, id: &str, ancestor: &str) -> bool {
    let mut at = Some(id);
    while let Some(cur) = at {
        if cur == ancestor {
            return true;
        }
        at = parents.get(cur).map(String::as_str);
    }
    false
}

/// The end (exclusive) of the subtree starting at `i`: the bookmarks after
/// it that stand under it.
fn subtree_end(bookmarks: &[Bookmark], parents: &BTreeMap<String, String>, i: usize) -> usize {
    let root = &bookmarks[i].self_id;
    let mut end = i + 1;
    while end < bookmarks.len() && is_within(parents, &bookmarks[end].self_id, root) {
        end += 1;
    }
    end
}

/// Where a bookmark goes in document order to stand at `index` among the
/// children of `parent` (the top level for `None`; past the last for
/// `None` or an index past the end).
fn insertion_point(
    bookmarks: &[Bookmark],
    parents: &BTreeMap<String, String>,
    parent: Option<&str>,
    index: Option<u32>,
) -> usize {
    let siblings: Vec<usize> = (0..bookmarks.len())
        .filter(|&i| parents.get(&bookmarks[i].self_id).map(String::as_str) == parent)
        .collect();
    match index.map(|k| k as usize) {
        Some(k) if k < siblings.len() => siblings[k],
        _ => match siblings.last() {
            Some(&last) => subtree_end(bookmarks, parents, last),
            // The parent's first child: right after it. The top level's
            // first bookmark: at the start.
            None => parent
                .and_then(|p| bookmarks.iter().position(|b| b.self_id == p))
                .map_or(0, |p| p + 1),
        },
    }
}

fn applied(op: Operation, before: String) -> Result<AppliedOperation, OperationError> {
    Ok(AppliedOperation {
        op,
        inverse: Operation::RestoreBookmarks { state: before },
        // Bookmarks print nothing; a PDF export reads them when it runs.
        invalidation: InvalidationHint::default(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_create_bookmark(
    doc: &mut Document,
    bookmark_id: &str,
    name: &str,
    parent: Option<&str>,
    index: Option<u32>,
    destination: Option<&str>,
    page: Option<&str>,
    dest_id: &str,
) -> Result<AppliedOperation, OperationError> {
    if doc
        .designmap
        .bookmarks
        .iter()
        .any(|b| b.self_id == bookmark_id)
    {
        return Err(OperationError::DuplicateNodeId {
            id: bookmark_id.to_string(),
        });
    }
    if let Some(p) = parent {
        position(doc, p)?;
    }
    let before = capture(doc);
    let target = match (destination, page) {
        (Some(d), _) => {
            if !doc
                .designmap
                .hyperlink_destinations
                .iter()
                .any(|x| x.self_id == d)
            {
                return Err(OperationError::CollectionEntryNotFound {
                    collection: "hyperlinkDestinations".to_string(),
                    id: d.to_string(),
                });
            }
            d.to_string()
        }
        (None, Some(pid)) => {
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
            // A page destination of its own, as InDesign makes one.
            doc.designmap
                .hyperlink_destinations
                .push(HyperlinkDestination {
                    self_id: dest_id.to_string(),
                    kind: HyperlinkDestinationKind::Page(pid.to_string()),
                });
            dest_id.to_string()
        }
        (None, None) => {
            return Err(OperationError::InvalidParent {
                parent: NodeId::Document(String::new()),
                child_kind: "bookmark with neither a page nor a destination".to_string(),
            })
        }
    };
    let dm = &mut doc.designmap;
    let at = insertion_point(&dm.bookmarks, &dm.bookmark_parents, parent, index);
    dm.bookmarks.insert(
        at,
        Bookmark {
            self_id: bookmark_id.to_string(),
            name: Some(name.to_string()),
            destination: Some(target),
        },
    );
    if let Some(p) = parent {
        dm.bookmark_parents
            .insert(bookmark_id.to_string(), p.to_string());
    }
    applied(
        Operation::CreateBookmark {
            bookmark_id: bookmark_id.to_string(),
            name: name.to_string(),
            parent: parent.map(str::to_string),
            index,
            destination: destination.map(str::to_string),
            page: page.map(str::to_string),
            dest_id: dest_id.to_string(),
        },
        before,
    )
}

pub(super) fn apply_rename_bookmark(
    doc: &mut Document,
    bookmark_id: &str,
    name: &str,
) -> Result<AppliedOperation, OperationError> {
    let i = position(doc, bookmark_id)?;
    let before = capture(doc);
    doc.designmap.bookmarks[i].name = Some(name.to_string());
    applied(
        Operation::RenameBookmark {
            bookmark_id: bookmark_id.to_string(),
            name: name.to_string(),
        },
        before,
    )
}

pub(super) fn apply_move_bookmark(
    doc: &mut Document,
    bookmark_id: &str,
    parent: Option<&str>,
    index: Option<u32>,
) -> Result<AppliedOperation, OperationError> {
    let i = position(doc, bookmark_id)?;
    if let Some(p) = parent {
        position(doc, p)?;
        if is_within(&doc.designmap.bookmark_parents, p, bookmark_id) {
            return Err(OperationError::InvalidParent {
                parent: NodeId::Document(String::new()),
                child_kind: format!("bookmark {bookmark_id} moved under itself"),
            });
        }
    }
    let before = capture(doc);
    let dm = &mut doc.designmap;
    let end = subtree_end(&dm.bookmarks, &dm.bookmark_parents, i);
    let moved: Vec<Bookmark> = dm.bookmarks.drain(i..end).collect();
    match parent {
        Some(p) => {
            dm.bookmark_parents
                .insert(bookmark_id.to_string(), p.to_string());
        }
        None => {
            dm.bookmark_parents.remove(bookmark_id);
        }
    }
    let at = insertion_point(&dm.bookmarks, &dm.bookmark_parents, parent, index);
    dm.bookmarks.splice(at..at, moved);
    applied(
        Operation::MoveBookmark {
            bookmark_id: bookmark_id.to_string(),
            parent: parent.map(str::to_string),
            index,
        },
        before,
    )
}

pub(super) fn apply_delete_bookmark(
    doc: &mut Document,
    bookmark_id: &str,
) -> Result<AppliedOperation, OperationError> {
    let i = position(doc, bookmark_id)?;
    let before = capture(doc);
    let dm = &mut doc.designmap;
    let end = subtree_end(&dm.bookmarks, &dm.bookmark_parents, i);
    let gone: Vec<Bookmark> = dm.bookmarks.drain(i..end).collect();
    for b in &gone {
        dm.bookmark_parents.remove(&b.self_id);
    }
    // The page destinations the deleted bookmarks went to, when nothing
    // else goes there: InDesign deletes them with the bookmark. A text
    // destination is an anchor in the text and stays.
    let used = |id: &str| {
        dm.bookmarks
            .iter()
            .any(|b| b.destination.as_deref() == Some(id))
            || dm
                .hyperlinks
                .iter()
                .any(|h| h.destination.as_deref() == Some(id))
            || dm
                .cross_references
                .iter()
                .any(|x| x.destination.as_deref() == Some(id))
    };
    let orphaned: Vec<String> = gone
        .iter()
        .filter_map(|b| b.destination.clone())
        .filter(|d| !used(d))
        .collect();
    dm.hyperlink_destinations.retain(|d| {
        !(orphaned.contains(&d.self_id) && matches!(d.kind, HyperlinkDestinationKind::Page(_)))
    });
    applied(
        Operation::DeleteBookmark {
            bookmark_id: bookmark_id.to_string(),
        },
        before,
    )
}

pub(super) fn apply_restore_bookmarks(
    doc: &mut Document,
    state: &str,
) -> Result<AppliedOperation, OperationError> {
    let captured: Captured =
        serde_json::from_str(state).map_err(|e| OperationError::InvalidParent {
            parent: NodeId::Document(String::new()),
            child_kind: format!("bookmarks whose saved state does not read ({e})"),
        })?;
    let before = capture(doc);
    let dm = &mut doc.designmap;
    dm.bookmarks = captured.bookmarks;
    dm.bookmark_parents = captured.parents;
    dm.hyperlink_destinations = captured.destinations;
    applied(
        Operation::RestoreBookmarks {
            state: state.to_string(),
        },
        before,
    )
}
