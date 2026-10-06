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

use std::collections::HashMap;

use super::*;
use paged_scene::{Document, ParsedStory};

use super::duplicate_nodes::{next_story_number, relink_copied_story, story_blocker};
use crate::error::OperationError;
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation, PropertyPath};

// ---------------------------------------------------------------------------
// W0.5 — duplicate page
// ---------------------------------------------------------------------------
//
// paged.data D-23: a duplicate is a COPY, as in InDesign. Each text frame
// on the page gets a copy of its story (fresh `Story/u<n>`, hyperlink
// sources re-minted with their `<Hyperlink>`s), so editing the copy never
// edits the original; frames threaded to each other on the page stay
// threaded to each other in the copy, and a thread that leaves the page is
// cut (the copy holds the whole story, as InDesign's does). Every side map
// keyed by an item's or the page's `Self` — margins, labels, image
// metadata, pasted-in children, opacity masks — is re-keyed to the clone's
// ids; before, the page came back without its margins.
//
// A story holding something with an id of its own (a table, an anchored
// object, a footnote) is refused, by the same rule `duplicateElements`
// applies: a copy would put that id in the document twice.

/// The redo payload: the cloned spread plus the stories and hyperlinks
/// the clone owns. `flatten` keeps a payload written before D-23 (a bare
/// [`SpreadRestore`]) decodable.
#[derive(serde::Serialize, serde::Deserialize)]
struct PageClone {
    #[serde(flatten)]
    spread: SpreadRestore,
    #[serde(default)]
    stories: Vec<ParsedStory>,
    #[serde(default)]
    hyperlinks: Vec<paged_model::Hyperlink>,
}

fn invalid(page: &str, reason: impl Into<String>) -> OperationError {
    OperationError::InvalidValue {
        node: NodeId::Page(page.to_string()),
        path: PropertyPath::PageBounds,
        reason: reason.into(),
    }
}

fn first_page_id(spread: &paged_model::Spread) -> String {
    spread
        .pages
        .first()
        .and_then(|p| p.self_id.clone())
        .unwrap_or_default()
}

/// Re-materialise a captured clone (redo, and the undo of its removal).
fn restore_clone(
    doc: &mut Document,
    page: &str,
    json: &str,
) -> Result<AppliedOperation, OperationError> {
    let clone: PageClone = serde_json::from_str(json)
        .map_err(|e| invalid(page, format!("malformed duplicate-page payload: {e}")))?;
    let cloned_page_id = first_page_id(&clone.spread.spread);
    if let Some(s) = clone
        .stories
        .iter()
        .find(|s| doc.stories.iter().any(|d| d.self_id == s.self_id))
    {
        return Err(OperationError::DuplicateNodeId {
            id: s.self_id.clone(),
        });
    }
    let story_ids: Vec<String> = clone.stories.iter().map(|s| s.self_id.clone()).collect();
    let hyperlink_ids: Vec<String> = clone.hyperlinks.iter().map(|h| h.self_id.clone()).collect();
    let index = clone.spread.index.min(doc.spreads.len());
    doc.spreads.insert(
        index,
        paged_scene::ParsedSpread {
            src: clone.spread.src,
            spread: clone.spread.spread,
        },
    );
    doc.stories.extend(clone.stories);
    doc.designmap.hyperlinks.extend(clone.hyperlinks);
    Ok(AppliedOperation {
        op: Operation::DuplicatePage {
            page: page.to_string(),
            clone_spread_json: Some(json.to_string()),
        },
        inverse: Operation::RemovePageClone {
            page: page.to_string(),
            cloned_page: cloned_page_id,
            story_ids,
            hyperlink_ids,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

pub(super) fn apply_duplicate_page(
    doc: &mut Document,
    page: &str,
    clone_spread_json: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    // Redo path — re-materialise the captured clone verbatim.
    if let Some(json) = clone_spread_json {
        return restore_clone(doc, page, json);
    }

    let src_idx = doc
        .spreads
        .iter()
        .position(|p| {
            p.spread
                .pages
                .iter()
                .any(|pg| pg.self_id.as_deref() == Some(page))
        })
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Page(page.to_string())))?;
    if doc.spreads[src_idx].spread.pages.len() != 1 {
        return Err(invalid(
            page,
            "duplicating a page out of a multi-page spread is not supported in v1",
        ));
    }

    // The stories the copy needs, in frame order, each validated before
    // anything is written.
    let mut story_sources: Vec<String> = Vec::new();
    for f in &doc.spreads[src_idx].spread.text_frames {
        let Some(sid) = f.parent_story.as_deref() else {
            continue;
        };
        if story_sources.iter().any(|s| s == sid) {
            continue;
        }
        let Some(parsed) = doc.stories.iter().find(|s| s.self_id == sid) else {
            continue;
        };
        if let Some(what) = story_blocker(&parsed.story) {
            return Err(invalid(
                page,
                format!(
                    "text frame {:?}'s story holds {what}, which carries an id of its own — \
                     a copy of the page would put that id in the document twice",
                    f.self_id.as_deref().unwrap_or_default()
                ),
            ));
        }
        story_sources.push(sid.to_string());
    }

    // Deep-clone the source spread, then give every `Self` a fresh id.
    // The floor is the document's whole shared `u<hex>` line — hyperlinks
    // and sections included — not the spreads alone, which handed out ids
    // a designmap object already held.
    let mut clone = doc.spreads[src_idx].clone();
    let mut next = crate::ids::highest_u_hex_id(doc) + 1;
    let renamed = fresh_ids(&mut clone.spread, &mut next);

    // The stories: one copy per distinct story, threads kept inside the
    // page and cut where they leave it.
    let mut story_n = next_story_number(doc);
    let mut new_story: HashMap<String, String> = HashMap::new();
    let mut stories: Vec<ParsedStory> = Vec::new();
    let mut hyperlinks: Vec<String> = Vec::new();
    for sid in &story_sources {
        let id = format!("Story/u{story_n}");
        story_n += 1;
        let mut story = doc
            .stories
            .iter()
            .find(|s| s.self_id == *sid)
            .expect("validated above")
            .story
            .clone();
        hyperlinks.extend(relink_copied_story(&mut doc.designmap, &mut story, &id));
        stories.push(ParsedStory {
            // No source part — minted post-parse, the writer's signal to
            // emit a full story.
            src: String::new(),
            self_id: id.clone(),
            story,
        });
        new_story.insert(sid.clone(), id);
    }
    for f in &mut clone.spread.text_frames {
        if let Some(sid) = f.parent_story.as_ref() {
            if let Some(id) = new_story.get(sid) {
                f.parent_story = Some(id.clone());
            }
        }
        f.next_text_frame = match f.next_text_frame.as_deref() {
            Some(t) if !t.is_empty() && t != "n" => renamed.get(t).cloned(),
            other => other.map(str::to_string),
        };
    }

    // Stack the clone below everything on the pasteboard (same rule as
    // InsertPage) so spread AABBs never overlap.
    let mut max_bottom: f32 = 0.0;
    for parsed in &doc.spreads {
        let sty = parsed.spread.item_transform.map(|m| m[5]).unwrap_or(0.0);
        for p in &parsed.spread.pages {
            let pty = p.item_transform.map(|m| m[5]).unwrap_or(0.0);
            max_bottom = max_bottom.max(sty + pty + p.bounds.bottom);
        }
    }
    clone.spread.item_transform = Some([1.0, 0.0, 0.0, 1.0, 0.0, max_bottom + SPREAD_STACK_GAP_PT]);
    let cloned_spread_self = clone.spread.self_id.clone().unwrap_or_default();
    clone.src = format!("Spreads/Spread_{cloned_spread_self}.xml");
    let cloned_page_id = first_page_id(&clone.spread);

    let insert_index = src_idx + 1;
    let added_links: Vec<paged_model::Hyperlink> = doc
        .designmap
        .hyperlinks
        .iter()
        .filter(|h| hyperlinks.contains(&h.self_id))
        .cloned()
        .collect();
    // Capture the materialised clone so redo re-creates the exact ids.
    let capture = PageClone {
        spread: SpreadRestore {
            index: insert_index,
            src: clone.src.clone(),
            spread: clone.spread.clone(),
        },
        stories: stories.clone(),
        hyperlinks: added_links,
    };
    let json = serde_json::to_string(&capture)
        .map_err(|e| invalid(page, format!("duplicate-page capture failed: {e}")))?;

    let story_ids: Vec<String> = stories.iter().map(|s| s.self_id.clone()).collect();
    doc.spreads.insert(insert_index, clone);
    doc.stories.extend(stories);

    Ok(AppliedOperation {
        op: Operation::DuplicatePage {
            page: page.to_string(),
            clone_spread_json: Some(json),
        },
        inverse: Operation::RemovePageClone {
            page: page.to_string(),
            cloned_page: cloned_page_id,
            story_ids,
            hyperlink_ids: hyperlinks,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// Give `spread`, its pages and all its items fresh `u<hex>` ids from
/// `next` on, and move the spread's id-keyed side maps (labels with
/// plugin metadata, page margins, placed-image metadata, pasted-in
/// children, opacity masks and a mask's item reference) to the new ids.
/// They used to keep the old ones, so a duplicate lost its metadata.
/// Returns the renames, old id → new.
pub(super) fn fresh_ids(
    spread: &mut paged_model::Spread,
    next: &mut u64,
) -> HashMap<String, String> {
    let mut renamed: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut remap = |slot: &mut Option<String>, next: &mut u64| {
        let fresh = format!("u{:x}", *next);
        if let Some(old) = slot.take() {
            renamed.insert(old, fresh.clone());
        }
        *slot = Some(fresh);
        *next += 1;
    };
    remap(&mut spread.self_id, next);
    for pg in &mut spread.pages {
        remap(&mut pg.self_id, next);
    }
    for f in &mut spread.text_frames {
        remap(&mut f.self_id, next);
    }
    for r in &mut spread.rectangles {
        remap(&mut r.self_id, next);
    }
    for o in &mut spread.ovals {
        remap(&mut o.self_id, next);
    }
    for l in &mut spread.graphic_lines {
        remap(&mut l.self_id, next);
    }
    for p in &mut spread.polygons {
        remap(&mut p.self_id, next);
    }
    for g in &mut spread.groups {
        remap(&mut g.self_id, next);
    }
    fn rekey<V>(
        map: &mut std::collections::HashMap<String, V>,
        renamed: &std::collections::HashMap<String, String>,
    ) {
        *map = std::mem::take(map)
            .into_iter()
            .map(|(k, v)| (renamed.get(&k).cloned().unwrap_or(k), v))
            .collect();
    }
    rekey(&mut spread.labels, &renamed);
    rekey(&mut spread.page_margins, &renamed);
    rekey(&mut spread.image_metadata, &renamed);
    rekey(&mut spread.nested_children, &renamed);
    rekey(&mut spread.opacity_masks, &renamed);
    for mask in spread.opacity_masks.values_mut() {
        if let Some(fresh) = renamed.get(&mask.mask_item) {
            mask.mask_item = fresh.clone();
        }
    }
    renamed
}

/// The undo of a `DuplicatePage`: remove the cloned page's spread, the
/// stories the clone owns and their hyperlinks. Its inverse re-applies
/// the duplicate from the capture.
pub(super) fn apply_remove_page_clone(
    doc: &mut Document,
    page: &str,
    cloned_page: &str,
    story_ids: &[String],
    hyperlink_ids: &[String],
) -> Result<AppliedOperation, OperationError> {
    let idx = doc
        .spreads
        .iter()
        .position(|p| {
            p.spread
                .pages
                .iter()
                .any(|pg| pg.self_id.as_deref() == Some(cloned_page))
        })
        .ok_or_else(|| OperationError::NodeNotFound(NodeId::Page(cloned_page.to_string())))?;
    if let Some(missing) = story_ids
        .iter()
        .find(|id| !doc.stories.iter().any(|s| s.self_id == **id))
    {
        return Err(OperationError::NodeNotFound(NodeId::Story(missing.clone())));
    }
    let parsed = doc.spreads.remove(idx);
    let mut stories = Vec::with_capacity(story_ids.len());
    for id in story_ids {
        let at = doc
            .stories
            .iter()
            .position(|s| s.self_id == *id)
            .expect("validated above");
        stories.push(doc.stories.remove(at));
    }
    let mut hyperlinks = Vec::new();
    doc.designmap.hyperlinks.retain(|h| {
        if hyperlink_ids.contains(&h.self_id) {
            hyperlinks.push(h.clone());
            false
        } else {
            true
        }
    });
    let capture = PageClone {
        spread: SpreadRestore {
            index: idx,
            src: parsed.src,
            spread: parsed.spread,
        },
        stories,
        hyperlinks,
    };
    let json = serde_json::to_string(&capture)
        .map_err(|e| invalid(cloned_page, format!("page-clone capture failed: {e}")))?;
    Ok(AppliedOperation {
        op: Operation::RemovePageClone {
            page: page.to_string(),
            cloned_page: cloned_page.to_string(),
            story_ids: story_ids.to_vec(),
            hyperlink_ids: hyperlink_ids.to_vec(),
        },
        inverse: Operation::DuplicatePage {
            page: page.to_string(),
            clone_spread_json: Some(json),
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}
