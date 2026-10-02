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

//! C-64 — duplicate page items.
//!
//! Until this op the only way to copy an element was the Alt-drag
//! gesture's `NodeSpec::CloneTranslate`, and that path is a HALF clone:
//! it reaches a TextFrame or a Rectangle and nothing else; a text
//! frame's copy keeps the source's `ParentStory`, so two head frames
//! share one story; a container's pasted-in children stay behind (the
//! `nested_children` side map is keyed by the SOURCE id); and its label
//! and image-metadata side maps are not copied either. This op clones
//! the whole thing or refuses, by name.
//!
//! ## What a clone is
//!
//! * the item's own struct, verbatim — every field the model carries,
//!   present or future, because the struct is CLONED, not re-listed;
//! * its `labels` and `image_metadata` side-map rows (keyed by `Self`);
//! * for a container, its pasted-in children, each cloned in turn;
//! * for a group, every member, each cloned in turn — nested groups
//!   too — and the group's own transparency and transform;
//! * for a text frame, a copy of its STORY under a fresh id.
//!
//! Each clone gets a fresh `Self` id and is translated by `(dx, dy)` in
//! spread space, through its `ItemTransform`: that is the one move that
//! is right for every kind (a path's anchors, a placed image's inner
//! transform and a gradient's axis are all in the item's own
//! coordinates, and shifting `bounds` alone would leave them behind).
//!
//! ## Where it goes
//!
//! Directly ABOVE its source: one slot after it in whichever list names
//! the source — the spread's z-order, a group's members, or a
//! container's pasted-in children. So a duplicate made inside a group
//! stays inside that group.
//!
//! ## What is refused
//!
//! Everything is validated before anything is written, so a refusal
//! leaves the document untouched:
//!
//! * a node that is not a page item, or is not on a body spread;
//! * an object anchored in a story (it travels with its text);
//! * an item serving as an opacity mask, or carrying one — the mask
//!   relation is keyed by id and has no clone semantics yet;
//! * a THREADED text frame (it shares its story with other frames, and
//!   a copy of "this frame's part of the story" is not defined);
//! * a text frame whose story holds something with an id of its own — a
//!   table, an anchored object, a footnote, a hyperlink source. A plain
//!   copy would put the same id in the document twice.
//!
//! ## Undo
//!
//! The inverse is [`Operation::RemoveDuplicates`], which removes exactly
//! the ids this op minted (items and stories) with a full reference
//! fix-up. It is not a batch of `RemoveNode`s: `RemoveNode` cannot
//! remove a group or a pasted-in child, and its inverse re-inserts a
//! hand-listed subset of fields. Redo re-applies this op with the ids
//! it resolved, against sources that are still there, so the clones
//! come back under the same ids.

use std::collections::HashSet;

use paged_model::{FrameRef, Spread};
use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation, PropertyPath};

use super::insert_node::{
    ensure_frames_in_order, fr_index, ref_home, unregister_frame_ref, RefHome,
};

/// How many ids a duplicate of `sources` mints: one per page item and
/// group in the cloned subtrees, and one per story copied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DuplicateDemand {
    pub items: usize,
    pub stories: usize,
}

fn refused(node: NodeId, reason: impl Into<String>) -> OperationError {
    OperationError::InvalidValue {
        node,
        // The path is unused; `InvalidValue` is the kernel's "the node
        // exists but this op does not apply to it" carrier.
        path: PropertyPath::FrameTransform,
        reason: reason.into(),
    }
}

fn ref_self_id(spread: &Spread, r: FrameRef) -> Option<&str> {
    match r {
        FrameRef::TextFrame(i) => spread.text_frames.get(i)?.self_id.as_deref(),
        FrameRef::Rectangle(i) => spread.rectangles.get(i)?.self_id.as_deref(),
        FrameRef::Oval(i) => spread.ovals.get(i)?.self_id.as_deref(),
        FrameRef::GraphicLine(i) => spread.graphic_lines.get(i)?.self_id.as_deref(),
        FrameRef::Polygon(i) => spread.polygons.get(i)?.self_id.as_deref(),
        FrameRef::Group(i) => spread.groups.get(i)?.self_id.as_deref(),
    }
}

fn node_for(spread: &Spread, r: FrameRef) -> NodeId {
    let id = ref_self_id(spread, r).unwrap_or_default().to_string();
    match r {
        FrameRef::TextFrame(_) => NodeId::TextFrame(id),
        FrameRef::Rectangle(_) => NodeId::Rectangle(id),
        FrameRef::Oval(_) => NodeId::Oval(id),
        FrameRef::GraphicLine(_) => NodeId::GraphicLine(id),
        FrameRef::Polygon(_) => NodeId::Polygon(id),
        FrameRef::Group(_) => NodeId::Group(id),
    }
}

/// `node`'s `FrameRef` on `spread`, for the six page-item kinds.
fn ref_in_spread(spread: &Spread, node: &NodeId) -> Option<FrameRef> {
    fn at<'a>(mut ids: impl Iterator<Item = Option<&'a str>>, want: &str) -> Option<usize> {
        ids.position(|s| s == Some(want))
    }
    match node {
        NodeId::TextFrame(id) => {
            at(spread.text_frames.iter().map(|f| f.self_id.as_deref()), id).map(FrameRef::TextFrame)
        }
        NodeId::Rectangle(id) => {
            at(spread.rectangles.iter().map(|f| f.self_id.as_deref()), id).map(FrameRef::Rectangle)
        }
        NodeId::Oval(id) => {
            at(spread.ovals.iter().map(|f| f.self_id.as_deref()), id).map(FrameRef::Oval)
        }
        NodeId::GraphicLine(id) => at(
            spread.graphic_lines.iter().map(|f| f.self_id.as_deref()),
            id,
        )
        .map(FrameRef::GraphicLine),
        NodeId::Polygon(id) => {
            at(spread.polygons.iter().map(|f| f.self_id.as_deref()), id).map(FrameRef::Polygon)
        }
        NodeId::Group(id) => {
            at(spread.groups.iter().map(|f| f.self_id.as_deref()), id).map(FrameRef::Group)
        }
        _ => None,
    }
}

fn is_anchored_in_a_story(doc: &Document, id: &str) -> bool {
    fn hit(a: &paged_model::AnchoredFrame, id: &str) -> bool {
        a.self_id.as_deref() == Some(id) || a.children.iter().any(|c| hit(c, id))
    }
    doc.stories.iter().any(|s| {
        s.story
            .paragraphs
            .iter()
            .any(|p| p.anchored_frames.iter().any(|a| hit(a, id)))
    })
}

/// Why `story` cannot be copied as it stands, if it cannot: the first
/// thing in it that carries an id of its own.
fn story_blocker(story: &paged_model::Story) -> Option<&'static str> {
    fn in_paragraphs(paragraphs: &[paged_model::Paragraph]) -> Option<&'static str> {
        for p in paragraphs {
            if p.table.is_some() {
                return Some("a table");
            }
            if !p.anchored_frames.is_empty() {
                return Some("an anchored object");
            }
            if !p.footnotes.is_empty() {
                return Some("a footnote");
            }
            if p.runs.iter().any(|r| r.hyperlink_source.is_some()) {
                return Some("a hyperlink");
            }
        }
        None
    }
    in_paragraphs(&story.paragraphs)
}

/// A `NextTextFrame` that names a frame. IDML spells "none" as `n`.
fn links_on(next: &Option<String>) -> bool {
    matches!(next.as_deref(), Some(s) if !s.is_empty() && s != "n")
}

/// Validate one cloned subtree and count the ids it needs.
fn check_subtree(
    doc: &Document,
    spread: &Spread,
    r: FrameRef,
    demand: &mut DuplicateDemand,
) -> Result<(), OperationError> {
    let node = node_for(spread, r);
    let id = ref_self_id(spread, r);
    if let Some(id) = id {
        if spread.opacity_masks.contains_key(id) {
            return Err(refused(
                node,
                "the item carries an opacity mask — release the mask before duplicating",
            ));
        }
    }
    demand.items += 1;
    match r {
        FrameRef::Group(gi) => {
            let group = spread.groups.get(gi).ok_or_else(|| {
                refused(node.clone(), "the group names a member that does not exist")
            })?;
            for m in &group.members {
                check_subtree(doc, spread, *m, demand)?;
            }
        }
        FrameRef::TextFrame(i) => {
            let frame = &spread.text_frames[i];
            if frame.is_anchored {
                return Err(refused(
                    node,
                    "the frame is anchored in a story — an inline object travels with its text",
                ));
            }
            let frame_id = frame.self_id.as_deref();
            let threaded = links_on(&frame.next_text_frame)
                || doc.spreads.iter().any(|p| {
                    p.spread.text_frames.iter().any(|other| {
                        other.self_id.as_deref() != frame_id
                            && ((frame_id.is_some()
                                && other.next_text_frame.as_deref() == frame_id)
                                || (frame.parent_story.is_some()
                                    && other.parent_story == frame.parent_story))
                    })
                });
            if threaded {
                return Err(refused(
                    node,
                    "the text frame is threaded — it shares its story with other frames, \
                     and a copy of one frame's part of a story is not defined",
                ));
            }
            if let Some(story) = frame
                .parent_story
                .as_deref()
                .and_then(|sid| doc.stories.iter().find(|s| s.self_id == sid))
            {
                if let Some(what) = story_blocker(&story.story) {
                    return Err(refused(
                        node,
                        format!(
                            "the text frame's story holds {what}, which carries an id of its \
                             own — a copy would put that id in the document twice"
                        ),
                    ));
                }
                demand.stories += 1;
            }
        }
        FrameRef::Rectangle(i) => {
            if spread.rectangles[i].is_anchored {
                return Err(refused(
                    node,
                    "the frame is anchored in a story — an inline object travels with its text",
                ));
            }
        }
        FrameRef::Oval(_) | FrameRef::GraphicLine(_) | FrameRef::Polygon(_) => {}
    }
    // Pasted-in children (a group hosts none; a leaf of any kind may).
    if let Some(children) = id.and_then(|id| spread.nested_children.get(id)) {
        for c in children {
            check_subtree(doc, spread, *c, demand)?;
        }
    }
    Ok(())
}

/// Locate and validate one source. Returns its spread and ref.
fn locate(
    doc: &Document,
    source: &NodeId,
    demand: &mut DuplicateDemand,
) -> Result<(usize, FrameRef), OperationError> {
    if !matches!(
        source,
        NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::GraphicLine(_)
            | NodeId::Polygon(_)
            | NodeId::Group(_)
    ) {
        return Err(refused(
            source.clone(),
            "only a page item (frame, shape, line, path or group) can be duplicated",
        ));
    }
    let found = doc
        .spreads
        .iter()
        .enumerate()
        .find_map(|(si, p)| ref_in_spread(&p.spread, source).map(|r| (si, r)));
    let Some((si, r)) = found else {
        if is_anchored_in_a_story(doc, source.self_id()) {
            return Err(refused(
                source.clone(),
                "the item is anchored in a story — an inline object travels with its text",
            ));
        }
        return Err(OperationError::NodeNotFound(source.clone()));
    };
    let spread = &doc.spreads[si].spread;
    // A spread that never built a z-table lists nothing; the clone pass
    // materialises one (render-neutral) before it looks for the source,
    // so only an item that a COMPLETE table would still not name is a
    // refusal here.
    let listed = spread.frames_in_order.is_empty() || ref_home(spread, r).is_some();
    if !listed {
        let id = source.self_id();
        if spread.opacity_masks.values().any(|m| m.mask_item == id) {
            return Err(refused(
                source.clone(),
                "the item is serving as an opacity mask — release the mask before duplicating",
            ));
        }
        return Err(refused(
            source.clone(),
            "the item is not in its spread's stacking order",
        ));
    }
    check_subtree(doc, spread, r, demand)?;
    Ok((si, r))
}

/// Every ref strictly inside the subtree at `r`.
fn descendants(spread: &Spread, r: FrameRef, out: &mut Vec<FrameRef>) {
    let push = |c: FrameRef, out: &mut Vec<FrameRef>| {
        out.push(c);
        descendants(spread, c, out);
    };
    if let FrameRef::Group(gi) = r {
        if let Some(g) = spread.groups.get(gi) {
            for m in &g.members {
                push(*m, out);
            }
        }
    }
    if let Some(children) = ref_self_id(spread, r).and_then(|id| spread.nested_children.get(id)) {
        for c in children {
            push(*c, out);
        }
    }
}

/// The op's whole validation: every source located and cleared, and the
/// ids the clones need. Writes nothing.
fn validate(
    doc: &Document,
    sources: &[NodeId],
) -> Result<(Vec<(usize, FrameRef)>, DuplicateDemand), OperationError> {
    if sources.is_empty() {
        return Err(refused(
            NodeId::Spread(String::new()),
            "no elements to duplicate",
        ));
    }
    let mut demand = DuplicateDemand::default();
    let mut located = Vec::with_capacity(sources.len());
    for s in sources {
        located.push(locate(doc, s, &mut demand)?);
    }
    // A source inside another source: the outer clone already carries a
    // copy of the inner one, and the inner one's own clone would land
    // inside the outer SOURCE — two answers to one request, and the
    // count of ids would depend on the order they were named in.
    for (si, r) in &located {
        let mut inner = Vec::new();
        descendants(&doc.spreads[*si].spread, *r, &mut inner);
        if let Some(pos) = located
            .iter()
            .position(|(sj, other)| sj == si && inner.contains(other))
        {
            return Err(refused(
                sources[pos].clone(),
                "the element is inside another element of the same duplicate — \
                 duplicate the outer one (its clone carries a copy of this) or this one alone",
            ));
        }
    }
    Ok((located, demand))
}

/// C-64 — what a `DuplicateNodes` over `sources` would mint, or the
/// refusal it would answer. Runs the op's whole validation and writes
/// nothing, so a caller that mints ids ahead of the apply (the canvas,
/// whose id floor also covers the source package) knows how many.
pub fn duplicate_demand(
    doc: &Document,
    sources: &[NodeId],
) -> Result<DuplicateDemand, OperationError> {
    validate(doc, sources).map(|(_, demand)| demand)
}

fn translated(m: Option<[f32; 6]>, dx: f32, dy: f32) -> Option<[f32; 6]> {
    match m {
        Some(mut m) => {
            m[4] += dx;
            m[5] += dy;
            Some(m)
        }
        None if dx == 0.0 && dy == 0.0 => None,
        None => Some([1.0, 0.0, 0.0, 1.0, dx, dy]),
    }
}

/// The running state of one apply: the offset, and the ids as they are
/// handed out.
struct Cloner<'a> {
    dx: f32,
    dy: f32,
    ids: std::slice::Iter<'a, String>,
    story_ids: std::slice::Iter<'a, String>,
}

impl Cloner<'_> {
    fn next_id(&mut self) -> String {
        self.ids
            .next()
            .cloned()
            .expect("validated: one id per item")
    }

    /// Clone the subtree at `r`, pushing every new item into its kind
    /// vec, and return the ref of the clone's root. The root is NOT
    /// listed anywhere yet — the caller decides where it goes.
    fn clone_ref(
        &mut self,
        spread: &mut Spread,
        stories: &mut Vec<paged_scene::ParsedStory>,
        r: FrameRef,
    ) -> FrameRef {
        let source_id = ref_self_id(spread, r).map(str::to_string);
        // Ids are handed out PRE-order (a group before its members), so
        // the first id a source consumes names its root.
        let new_id = self.next_id();
        let (dx, dy) = (self.dx, self.dy);
        let new_ref = match r {
            FrameRef::Group(gi) => {
                let source = spread.groups[gi].clone();
                // Members first: `group_pass` brackets innermost-first by
                // walking `groups` in reverse, which relies on a nested
                // group sitting BEFORE its outer in the vec.
                let members: Vec<FrameRef> = source
                    .members
                    .iter()
                    .map(|m| self.clone_ref(spread, stories, *m))
                    .collect();
                spread.groups.push(paged_model::Group {
                    self_id: Some(new_id.clone()),
                    members,
                    item_transform: translated(source.item_transform, dx, dy),
                    ..source
                });
                FrameRef::Group(spread.groups.len() - 1)
            }
            FrameRef::TextFrame(i) => {
                let mut clone = spread.text_frames[i].clone();
                clone.self_id = Some(new_id.clone());
                clone.item_transform = translated(clone.item_transform, dx, dy);
                clone.parent_story = match clone
                    .parent_story
                    .as_deref()
                    .and_then(|sid| stories.iter().find(|s| s.self_id == sid))
                    .map(|s| s.story.clone())
                {
                    Some(story) => {
                        let story_id = self
                            .story_ids
                            .next()
                            .cloned()
                            .expect("validated: one id per story");
                        stories.push(paged_scene::ParsedStory {
                            // No source part — minted post-parse, the
                            // writer's signal to emit a full story.
                            src: String::new(),
                            self_id: story_id.clone(),
                            story,
                        });
                        Some(story_id)
                    }
                    // A frame with no story, or naming one that is not
                    // in the document: the copy holds no text either.
                    None => None,
                };
                spread.text_frames.push(clone);
                FrameRef::TextFrame(spread.text_frames.len() - 1)
            }
            FrameRef::Rectangle(i) => {
                let mut clone = spread.rectangles[i].clone();
                clone.self_id = Some(new_id.clone());
                clone.item_transform = translated(clone.item_transform, dx, dy);
                spread.rectangles.push(clone);
                FrameRef::Rectangle(spread.rectangles.len() - 1)
            }
            FrameRef::Oval(i) => {
                let mut clone = spread.ovals[i].clone();
                clone.self_id = Some(new_id.clone());
                clone.item_transform = translated(clone.item_transform, dx, dy);
                spread.ovals.push(clone);
                FrameRef::Oval(spread.ovals.len() - 1)
            }
            FrameRef::GraphicLine(i) => {
                let mut clone = spread.graphic_lines[i].clone();
                clone.self_id = Some(new_id.clone());
                clone.item_transform = translated(clone.item_transform, dx, dy);
                spread.graphic_lines.push(clone);
                FrameRef::GraphicLine(spread.graphic_lines.len() - 1)
            }
            FrameRef::Polygon(i) => {
                let mut clone = spread.polygons[i].clone();
                clone.self_id = Some(new_id.clone());
                clone.item_transform = translated(clone.item_transform, dx, dy);
                spread.polygons.push(clone);
                FrameRef::Polygon(spread.polygons.len() - 1)
            }
        };
        // The side maps keyed by `Self`.
        if let Some(source_id) = source_id {
            if let Some(labels) = spread.labels.get(&source_id).cloned() {
                spread.labels.insert(new_id.clone(), labels);
            }
            if let Some(meta) = spread.image_metadata.get(&source_id).cloned() {
                spread.image_metadata.insert(new_id.clone(), meta);
            }
            if let Some(children) = spread.nested_children.get(&source_id).cloned() {
                let cloned: Vec<FrameRef> = children
                    .iter()
                    .map(|c| self.clone_ref(spread, stories, *c))
                    .collect();
                spread.nested_children.insert(new_id, cloned);
            }
        }
        new_ref
    }
}

/// The `u<hex>` ids and `Story/u<n>` ids a direct kernel caller did not
/// supply. Mirrors the canvas minter's two number lines.
fn mint_missing(doc: &Document, demand: DuplicateDemand) -> (Vec<String>, Vec<String>) {
    let base = crate::ids::highest_u_hex_id(doc) + 1;
    let ids = (0..demand.items as u64)
        .map(|k| format!("u{:x}", base + k))
        .collect();
    let mut n = doc.stories.len();
    for s in &doc.stories {
        if let Some(v) = s
            .self_id
            .strip_prefix("Story/u")
            .filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|d| d.parse::<usize>().ok())
        {
            n = n.max(v + 1);
        }
    }
    let story_ids = (0..demand.stories)
        .map(|k| format!("Story/u{}", n + k))
        .collect();
    (ids, story_ids)
}

pub(super) fn apply_duplicate_nodes(
    doc: &mut Document,
    sources: &[NodeId],
    dx: f32,
    dy: f32,
    ids: &[String],
    story_ids: &[String],
) -> Result<AppliedOperation, OperationError> {
    // ---- validation: nothing is written before this block ends ----
    let (located, demand) = validate(doc, sources)?;
    if !dx.is_finite() || !dy.is_finite() {
        return Err(refused(sources[0].clone(), "the offset is not a number"));
    }
    let (ids, story_ids): (Vec<String>, Vec<String>) = if ids.is_empty() && story_ids.is_empty() {
        mint_missing(doc, demand)
    } else {
        (ids.to_vec(), story_ids.to_vec())
    };
    if ids.len() != demand.items || story_ids.len() != demand.stories {
        return Err(refused(
            sources[0].clone(),
            format!(
                "the duplicate needs {} item id(s) and {} story id(s); {} and {} were supplied",
                demand.items,
                demand.stories,
                ids.len(),
                story_ids.len()
            ),
        ));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for id in &ids {
        let taken = !seen.insert(id.as_str())
            || doc.spreads.iter().any(|p| {
                let s = &p.spread;
                s.text_frames
                    .iter()
                    .map(|f| f.self_id.as_deref())
                    .chain(s.rectangles.iter().map(|f| f.self_id.as_deref()))
                    .chain(s.ovals.iter().map(|f| f.self_id.as_deref()))
                    .chain(s.graphic_lines.iter().map(|f| f.self_id.as_deref()))
                    .chain(s.polygons.iter().map(|f| f.self_id.as_deref()))
                    .chain(s.groups.iter().map(|f| f.self_id.as_deref()))
                    .any(|x| x == Some(id.as_str()))
            });
        if taken {
            return Err(OperationError::DuplicateNodeId { id: id.clone() });
        }
    }
    let mut seen_stories: HashSet<&str> = HashSet::new();
    for id in &story_ids {
        if !seen_stories.insert(id.as_str()) || doc.stories.iter().any(|s| s.self_id == *id) {
            return Err(OperationError::DuplicateNodeId { id: id.clone() });
        }
    }

    // ---- mutation (validated; cannot fail past this point) ----
    let mut cloner = Cloner {
        dx,
        dy,
        ids: ids.iter(),
        story_ids: story_ids.iter(),
    };
    let mut touched_spreads: Vec<usize> = Vec::new();
    for (source, (si, _)) in sources.iter().zip(&located) {
        let (spreads, stories) = (&mut doc.spreads, &mut doc.stories);
        let spread = &mut spreads[*si].spread;
        if !touched_spreads.contains(si) {
            // An authoritative z-table, so "directly above the source"
            // has a slot to mean. A complete materialisation equals the
            // renderer's own fallback order, so it paints the same.
            ensure_frames_in_order(spread);
            touched_spreads.push(*si);
        }
        // Re-resolved per source: an earlier source's clone may have
        // been inserted into the same list, and kind-vec indices of the
        // SOURCES never move (clones are appended).
        let r = ref_in_spread(spread, source).expect("validated: the source is on this spread");
        let clone = cloner.clone_ref(spread, stories, r);
        match ref_home(spread, r) {
            Some(RefHome::Root(i)) => spread.frames_in_order.insert(i + 1, clone),
            Some(RefHome::Group(gi, i)) => spread.groups[gi].members.insert(i + 1, clone),
            Some(RefHome::Nested(host, i)) => {
                if let Some(list) = spread.nested_children.get_mut(&host) {
                    list.insert(i + 1, clone);
                }
            }
            // Unreachable after `ensure_frames_in_order` + validation;
            // on top is the safe place for a clone nothing lists.
            None => spread.frames_in_order.push(clone),
        }
    }

    Ok(AppliedOperation {
        op: Operation::DuplicateNodes {
            sources: sources.to_vec(),
            dx,
            dy,
            ids: ids.clone(),
            story_ids: story_ids.clone(),
        },
        inverse: Operation::RemoveDuplicates {
            sources: sources.to_vec(),
            dx,
            dy,
            ids,
            story_ids,
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

/// The root clone of each source of a `DuplicateNodes`, in source order
/// — what the wire reports as minted. A clone takes its source's kind,
/// and ids are handed out pre-order, so a source's root is the first id
/// its subtree consumes: the answer is a walk of the SOURCES, which are
/// in `doc` before the op is applied and after it alike. Empty when the
/// op carries no resolved ids (it has not been applied or translated
/// with them) or names a source that is not there.
pub fn duplicate_roots(doc: &Document, op: &Operation) -> Vec<NodeId> {
    let Operation::DuplicateNodes { sources, ids, .. } = op else {
        return Vec::new();
    };
    let mut roots = Vec::with_capacity(sources.len());
    let mut next = 0usize;
    for source in sources {
        let (Some(id), Some((spread, r))) = (
            ids.get(next),
            doc.spreads
                .iter()
                .find_map(|p| ref_in_spread(&p.spread, source).map(|r| (&p.spread, r))),
        ) else {
            return Vec::new();
        };
        roots.push(match source {
            NodeId::TextFrame(_) => NodeId::TextFrame(id.clone()),
            NodeId::Rectangle(_) => NodeId::Rectangle(id.clone()),
            NodeId::Oval(_) => NodeId::Oval(id.clone()),
            NodeId::GraphicLine(_) => NodeId::GraphicLine(id.clone()),
            NodeId::Polygon(_) => NodeId::Polygon(id.clone()),
            _ => NodeId::Group(id.clone()),
        });
        next += subtree_len(spread, r);
    }
    roots
}

/// Items (and groups) in the subtree at `r` — the ids it consumed.
fn subtree_len(spread: &Spread, r: FrameRef) -> usize {
    let mut n = 1;
    if let FrameRef::Group(gi) = r {
        if let Some(g) = spread.groups.get(gi) {
            n += g
                .members
                .iter()
                .map(|m| subtree_len(spread, *m))
                .sum::<usize>();
        }
    }
    if let Some(children) = ref_self_id(spread, r).and_then(|id| spread.nested_children.get(id)) {
        n += children
            .iter()
            .map(|c| subtree_len(spread, *c))
            .sum::<usize>();
    }
    n
}

/// Remove the item at `r` from its kind vec and from every list that
/// can name it, closing the gap in all of them (the shared
/// `unregister_frame_ref`, which C-74 made complete).
fn remove_ref_everywhere(spread: &mut Spread, r: FrameRef) {
    match r {
        FrameRef::TextFrame(i) => {
            spread.text_frames.remove(i);
        }
        FrameRef::Rectangle(i) => {
            spread.rectangles.remove(i);
        }
        FrameRef::Oval(i) => {
            spread.ovals.remove(i);
        }
        FrameRef::GraphicLine(i) => {
            spread.graphic_lines.remove(i);
        }
        FrameRef::Polygon(i) => {
            spread.polygons.remove(i);
        }
        FrameRef::Group(i) => {
            spread.groups.remove(i);
        }
    }
    unregister_frame_ref(spread, r, fr_index(&r));
}

pub(super) fn apply_remove_duplicates(
    doc: &mut Document,
    sources: &[NodeId],
    dx: f32,
    dy: f32,
    ids: &[String],
    story_ids: &[String],
) -> Result<AppliedOperation, OperationError> {
    // Validate first: every id this op is about to remove must be here.
    let find = |doc: &Document, id: &str| -> Option<(usize, FrameRef)> {
        doc.spreads.iter().enumerate().find_map(|(si, p)| {
            [
                NodeId::TextFrame(id.to_string()),
                NodeId::Rectangle(id.to_string()),
                NodeId::Oval(id.to_string()),
                NodeId::GraphicLine(id.to_string()),
                NodeId::Polygon(id.to_string()),
                NodeId::Group(id.to_string()),
            ]
            .iter()
            .find_map(|n| ref_in_spread(&p.spread, n))
            .map(|r| (si, r))
        })
    };
    for id in ids {
        if find(doc, id).is_none() {
            return Err(OperationError::NodeNotFound(NodeId::Rectangle(id.clone())));
        }
    }
    for id in story_ids {
        if !doc.stories.iter().any(|s| s.self_id == *id) {
            return Err(OperationError::NodeNotFound(NodeId::Story(id.clone())));
        }
    }
    // Newest first, each located afresh (a removal renumbers its kind).
    for id in ids.iter().rev() {
        let (si, r) = find(doc, id).expect("validated above");
        let spread = &mut doc.spreads[si].spread;
        remove_ref_everywhere(spread, r);
        spread.labels.remove(id);
        spread.image_metadata.remove(id);
        spread.nested_children.remove(id);
    }
    doc.stories.retain(|s| !story_ids.contains(&s.self_id));

    Ok(AppliedOperation {
        op: Operation::RemoveDuplicates {
            sources: sources.to_vec(),
            dx,
            dy,
            ids: ids.to_vec(),
            story_ids: story_ids.to_vec(),
        },
        inverse: Operation::DuplicateNodes {
            sources: sources.to_vec(),
            dx,
            dy,
            ids: ids.to_vec(),
            story_ids: story_ids.to_vec(),
        },
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}
