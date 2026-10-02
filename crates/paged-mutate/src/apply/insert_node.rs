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
use paged_model::{FrameRef, GraphicLine, Oval, Polygon, Rectangle, Spread, TextFrame};
use paged_scene::Document;

use crate::error::OperationError;
use crate::invert::invert_insert_node;
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, NodeSpec, Operation, PathAnchorSpec,
};

// ---------------------------------------------------------------------------
// InsertNode
// ---------------------------------------------------------------------------

/// `frames_in_order` bookkeeping for structural inserts/removals.
///
/// The renderer, hit-tester, and scene-tree all walk a spread's
/// `frames_in_order` (cross-shape z-order) whenever it is non-empty —
/// a page item present in its kind vec but absent from the table is
/// invisible AND unclickable, and inserting/removing mid-vec shifts
/// every later same-kind `FrameRef` index. These helpers keep the
/// table consistent. On spreads whose table is EMPTY they do nothing:
/// the consumers' legacy fallback synthesises the walk order from the
/// kind vecs directly, and making the table non-empty with a single
/// entry would hide every other frame.
pub(super) fn fr_index(fr: &FrameRef) -> usize {
    match fr {
        FrameRef::TextFrame(i)
        | FrameRef::Rectangle(i)
        | FrameRef::Oval(i)
        | FrameRef::GraphicLine(i)
        | FrameRef::Polygon(i)
        | FrameRef::Group(i) => *i,
    }
}

pub(super) fn fr_with_index(fr: &FrameRef, i: usize) -> FrameRef {
    match fr {
        FrameRef::TextFrame(_) => FrameRef::TextFrame(i),
        FrameRef::Rectangle(_) => FrameRef::Rectangle(i),
        FrameRef::Oval(_) => FrameRef::Oval(i),
        FrameRef::GraphicLine(_) => FrameRef::GraphicLine(i),
        FrameRef::Polygon(_) => FrameRef::Polygon(i),
        FrameRef::Group(_) => FrameRef::Group(i),
    }
}

pub(super) fn fr_same_kind(a: &FrameRef, b: &FrameRef) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Materialise a spread's `frames_in_order` z-table from the per-kind
/// vecs when it is EMPTY (the legacy fallback order the renderer,
/// hit-tester, and scene-tree synthesise on the fly — see
/// `build_engine`'s `frames_ordered`). Ops that need an *authoritative*
/// z-table (e.g. `CreateGroup`, whose top-level membership check and
/// z-slot lookups index into the table) call this first.
///
/// Why it's needed: a spread built up entirely via `InsertNode` — most
/// notably a synthesised blank document — keeps an empty table, because
/// [`register_frame_ref`] deliberately no-ops on an empty one (a single
/// partial entry would hide every other frame). A COMPLETE
/// materialisation is render-neutral: the order equals the synthesised
/// fallback, so nothing moves; it just makes the implicit order explicit
/// so order-dependent mutations can run.
///
/// Order mirrors `build_engine`'s legacy concatenation
/// (text → rect → oval → line → polygon) and additionally appends any
/// existing groups, so a group member that is itself a group still
/// resolves.
pub(super) fn ensure_frames_in_order(spread: &mut Spread) {
    if !spread.frames_in_order.is_empty() {
        return;
    }
    let mut v: Vec<FrameRef> = Vec::new();
    v.extend((0..spread.text_frames.len()).map(FrameRef::TextFrame));
    v.extend((0..spread.rectangles.len()).map(FrameRef::Rectangle));
    v.extend((0..spread.ovals.len()).map(FrameRef::Oval));
    v.extend((0..spread.graphic_lines.len()).map(FrameRef::GraphicLine));
    v.extend((0..spread.polygons.len()).map(FrameRef::Polygon));
    v.extend((0..spread.groups.len()).map(FrameRef::Group));
    spread.frames_in_order = v;
}

/// The z table a spread would have had just before `template` was
/// pushed into its kind vec — every existing item, in the same order
/// [`ensure_frames_in_order`] synthesises, minus the one being
/// registered.
///
/// The caller has already grown the kind vec, so the template's own
/// kind is one shorter here; the shift-and-insert that follows then
/// operates on pre-insert indices, which is what it expects.
fn z_table_before_insert(spread: &Spread, template: &FrameRef) -> Vec<FrameRef> {
    let mut v: Vec<FrameRef> = Vec::new();
    let mut push = |count: usize, make: fn(usize) -> FrameRef| {
        let sample = make(0);
        let before = if fr_same_kind(&sample, template) {
            count.saturating_sub(1)
        } else {
            count
        };
        v.extend((0..before).map(make));
    };
    push(spread.text_frames.len(), FrameRef::TextFrame);
    push(spread.rectangles.len(), FrameRef::Rectangle);
    push(spread.ovals.len(), FrameRef::Oval);
    push(spread.graphic_lines.len(), FrameRef::GraphicLine);
    push(spread.polygons.len(), FrameRef::Polygon);
    push(spread.groups.len(), FrameRef::Group);
    v
}

/// Register a page item inserted at `vec_pos` of its kind vec:
/// same-kind refs at `>= vec_pos` shift up by one, then the new ref
/// lands at `z_slot` (or on top when `None` — new creations stack
/// like InDesign's draw tools).
pub(super) fn register_frame_ref(
    spread: &mut Spread,
    template: FrameRef,
    vec_pos: usize,
    z_slot: Option<usize>,
) {
    // B-18: nested-children refs live outside `frames_in_order` but
    // index the same backing vecs — shift them with everything else
    // (and regardless of the legacy empty-z-table fallback below).
    for children in spread.nested_children.values_mut() {
        for fr in children.iter_mut() {
            if fr_same_kind(fr, &template) {
                let i = fr_index(fr);
                if i >= vec_pos {
                    *fr = fr_with_index(fr, i + 1);
                }
            }
        }
    }
    // C-74: so do every group's members. They were left out, and an
    // insert below a grouped item of the same kind re-seated the group
    // onto its neighbours — the mirror of the removal fault in
    // `unregister_frame_ref`, and the reason undoing a delete could not
    // repair what the delete had done.
    for group in spread.groups.iter_mut() {
        for fr in group.members.iter_mut() {
            if fr_same_kind(fr, &template) {
                let i = fr_index(fr);
                if i >= vec_pos {
                    *fr = fr_with_index(fr, i + 1);
                }
            }
        }
    }
    if spread.frames_in_order.is_empty() {
        // A spread BORN empty — every page an editor session authors
        // from nothing — used to return here, and so never acquired a
        // z table at all: the first insert declined to start one, and
        // every insert after it found the table still empty and
        // declined too. Downstream that left the renderer, the
        // hit-tester and the scene tree on their synthetic
        // kind-by-kind walk (all text frames, then all rectangles, …),
        // which is not paint order and, until it learned to sort,
        // painted a Background rectangle over Content text.
        //
        // Start the table from the items that were here BEFORE this
        // insert; the shift and the insert below then place the new
        // one exactly as they would have on a parsed spread.
        spread.frames_in_order = z_table_before_insert(spread, &template);
    }
    for fr in spread.frames_in_order.iter_mut() {
        if fr_same_kind(fr, &template) {
            let i = fr_index(fr);
            if i >= vec_pos {
                *fr = fr_with_index(fr, i + 1);
            }
        }
    }
    let len = spread.frames_in_order.len();
    let slot = z_slot.unwrap_or(len).min(len);
    spread
        .frames_in_order
        .insert(slot, fr_with_index(&template, vec_pos));
}

/// The list that names a page item, and the item's position in it.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum RefHome {
    /// `Spread::frames_in_order`.
    Root(usize),
    /// `Spread::groups[group].members`.
    Group(usize, usize),
    /// `Spread::nested_children[host]`.
    Nested(String, usize),
}

/// Where `r` is listed, if anywhere. A page item is named by exactly
/// one list: the spread's z-order, one group's members, or one
/// container's pasted-in children (a mask item is named by none).
pub(super) fn ref_home(spread: &Spread, r: FrameRef) -> Option<RefHome> {
    if let Some(i) = spread.frames_in_order.iter().position(|x| *x == r) {
        return Some(RefHome::Root(i));
    }
    for (gi, g) in spread.groups.iter().enumerate() {
        if let Some(i) = g.members.iter().position(|x| *x == r) {
            return Some(RefHome::Group(gi, i));
        }
    }
    // Deterministic across runs: a `HashMap` walk has no stable order,
    // and an item is listed by at most one host anyway.
    let mut hosts: Vec<&String> = spread.nested_children.keys().collect();
    hosts.sort();
    for host in hosts {
        if let Some(i) = spread.nested_children[host].iter().position(|x| *x == r) {
            return Some(RefHome::Nested(host.clone(), i));
        }
    }
    None
}

/// Unregister a page item removed from `vec_pos` of its kind vec;
/// returns the z slot it occupied so the `RemoveNode` inverse can
/// restore the exact stacking position.
///
/// The item's ref is dropped from EVERY list that can name it — the
/// z-table, a group's members, a container's pasted-in children — and
/// same-kind refs above it step down by one in all three. C-74: the
/// members were not touched at all, so removing an item re-seated every
/// group holding a later item of its kind (`r1, group[r2, r3], r4` →
/// `group[r3, r4], r4`) while the op reported success. A caller that
/// needs to know WHICH list named the item (to put it back there) asks
/// [`ref_home`] before it calls this.
pub(super) fn unregister_frame_ref(
    spread: &mut Spread,
    template: FrameRef,
    vec_pos: usize,
) -> Option<usize> {
    let target = fr_with_index(&template, vec_pos);
    let step_down = |fr: &mut FrameRef| {
        if fr_same_kind(fr, &template) {
            let i = fr_index(fr);
            if i > vec_pos {
                *fr = fr_with_index(fr, i - 1);
            }
        }
    };
    for children in spread.nested_children.values_mut() {
        children.retain(|fr| *fr != target);
        children.iter_mut().for_each(step_down);
    }
    for group in spread.groups.iter_mut() {
        group.members.retain(|fr| *fr != target);
        group.members.iter_mut().for_each(step_down);
    }
    if spread.frames_in_order.is_empty() {
        return None;
    }
    let slot = spread.frames_in_order.iter().position(|fr| *fr == target);
    if let Some(s) = slot {
        spread.frames_in_order.remove(s);
    }
    spread.frames_in_order.iter_mut().for_each(step_down);
    slot
}

/// Move a just-registered ref out of the z-table and into a group's
/// members at `slot` (the end when `None`). `InsertNode` with a
/// `NodeId::Group` parent — what the inverse of removing a member is.
pub(super) fn seat_in_group(
    spread: &mut Spread,
    r: FrameRef,
    group_idx: usize,
    slot: Option<usize>,
) {
    spread.frames_in_order.retain(|fr| *fr != r);
    let members = &mut spread.groups[group_idx].members;
    let at = slot.unwrap_or(members.len()).min(members.len());
    members.insert(at, r);
}

/// Where an insert lands: `(spread id, group id)`. The parent is a
/// spread (the item joins its z-order) or — C-74 — a GROUP on some
/// spread (the item joins that group's members, with `z_slot` naming the
/// member slot). The second is what puts a removed member back where it
/// was; before it the inverse of that removal could only say "the
/// spread", and the member came back as a second top-level entry.
fn resolve_parent(
    doc: &Document,
    parent: &NodeId,
    spec: &NodeSpec,
) -> Result<(String, Option<String>), OperationError> {
    match parent {
        NodeId::Spread(id) => Ok((id.clone(), None)),
        NodeId::Group(gid) => {
            let host = doc
                .spreads
                .iter()
                .find(|p| {
                    p.spread
                        .groups
                        .iter()
                        .any(|g| g.self_id.as_deref() == Some(gid.as_str()))
                })
                .and_then(|p| p.spread.self_id.clone())
                .ok_or_else(|| OperationError::NodeNotFound(parent.clone()))?;
            Ok((host, Some(gid.clone())))
        }
        _ => Err(OperationError::InvalidParent {
            parent: parent.clone(),
            child_kind: spec.node_id().kind().to_string(),
        }),
    }
}

/// Make sure the story a text frame names exists, creating the empty
/// story when it does not (the fresh-insert case, and the redo of an
/// undone insert). An existing story is left alone — that is how a
/// re-inserted frame gets its text back.
fn ensure_story(doc: &mut Document, id: &str) {
    if doc.stories.iter().any(|s| s.self_id == id) {
        return;
    }
    let mut story = paged_model::Story::default();
    // One empty paragraph + run — the shape an empty parsed story has;
    // the text ops' `locate()` needs ≥1 paragraph.
    story.paragraphs.push(paged_model::Paragraph {
        runs: vec![paged_model::CharacterRun::default()],
        ..Default::default()
    });
    doc.stories.push(paged_scene::ParsedStory {
        // No source entry — minted post-parse. The empty src is the
        // writer's mint signal: `paged-write` (C-8) emits a full
        // `Stories/Story_<sanitized-id>.xml` part + designmap ref for it
        // on export.
        src: String::new(),
        self_id: id.to_string(),
        story,
    });
}

/// C-75 — re-insert a node `RemoveNode` captured whole
/// ([`NodeSpec::Captured`]): the model struct goes back verbatim, with
/// its image bytes and the side-map rows that left the spread with it.
fn apply_insert_captured(
    doc: &mut Document,
    parent: &NodeId,
    position: usize,
    z_slot: Option<usize>,
    spec: &NodeSpec,
) -> Result<AppliedOperation, OperationError> {
    let NodeSpec::Captured {
        node,
        json,
        image_bytes,
    } = spec
    else {
        unreachable!("apply_insert_captured called with another spec");
    };
    let malformed = |what: String| OperationError::InvalidValue {
        node: node.clone(),
        path: crate::operation::PropertyPath::FrameTransform,
        reason: format!("malformed captured node: {what}"),
    };
    let (parent_id, group_home) = resolve_parent(doc, parent, spec)?;
    if node_exists(doc, node) {
        return Err(OperationError::DuplicateNodeId {
            id: node.self_id().to_string(),
        });
    }
    let mut envelope: serde_json::Value =
        serde_json::from_str(json).map_err(|e| malformed(e.to_string()))?;
    let item = envelope
        .get_mut("item")
        .map(serde_json::Value::take)
        .ok_or_else(|| malformed("no item".to_string()))?;
    // Decode BEFORE touching the document, so a capture that does not
    // decode leaves it untouched.
    enum Item {
        TextFrame(Box<TextFrame>),
        Rectangle(Box<Rectangle>),
        Oval(Box<Oval>),
        GraphicLine(Box<GraphicLine>),
        Polygon(Box<Polygon>),
    }
    let decode = |e: serde_json::Error| malformed(e.to_string());
    let item = match node {
        NodeId::TextFrame(_) => Item::TextFrame(serde_json::from_value(item).map_err(decode)?),
        NodeId::Rectangle(_) => {
            let mut r: Box<Rectangle> = serde_json::from_value(item).map_err(decode)?;
            r.image_bytes = image_bytes.clone();
            Item::Rectangle(r)
        }
        NodeId::Oval(_) => {
            let mut o: Box<Oval> = serde_json::from_value(item).map_err(decode)?;
            o.image_bytes = image_bytes.clone();
            Item::Oval(o)
        }
        NodeId::GraphicLine(_) => Item::GraphicLine(serde_json::from_value(item).map_err(decode)?),
        NodeId::Polygon(_) => {
            let mut p: Box<Polygon> = serde_json::from_value(item).map_err(decode)?;
            p.image_bytes = image_bytes.clone();
            Item::Polygon(p)
        }
        other => return Err(malformed(format!("{other:?} is not a page item"))),
    };
    let labels: Option<Vec<(String, String)>> = envelope
        .get_mut("labels")
        .map(serde_json::Value::take)
        .map(serde_json::from_value)
        .transpose()
        .map_err(decode)?
        .flatten();
    let image_metadata: Option<paged_model::ImageMetadata> = envelope
        .get_mut("imageMetadata")
        .map(serde_json::Value::take)
        .map(serde_json::from_value)
        .transpose()
        .map_err(decode)?
        .flatten();

    if let Item::TextFrame(frame) = &item {
        if let Some(story) = frame.parent_story.clone() {
            ensure_story(doc, &story);
        }
    }
    let spread = find_spread_mut(doc, &parent_id)
        .ok_or_else(|| OperationError::NodeNotFound(parent.clone()))?;
    let s = &mut spread.spread;
    let len = match &item {
        Item::TextFrame(_) => s.text_frames.len(),
        Item::Rectangle(_) => s.rectangles.len(),
        Item::Oval(_) => s.ovals.len(),
        Item::GraphicLine(_) => s.graphic_lines.len(),
        Item::Polygon(_) => s.polygons.len(),
    };
    if position > len {
        return Err(OperationError::InvalidPosition {
            parent: parent.clone(),
            position,
            len,
        });
    }
    let new_ref = match item {
        Item::TextFrame(f) => {
            s.text_frames.insert(position, *f);
            FrameRef::TextFrame(position)
        }
        Item::Rectangle(r) => {
            s.rectangles.insert(position, *r);
            FrameRef::Rectangle(position)
        }
        Item::Oval(o) => {
            s.ovals.insert(position, *o);
            FrameRef::Oval(position)
        }
        Item::GraphicLine(l) => {
            s.graphic_lines.insert(position, *l);
            FrameRef::GraphicLine(position)
        }
        Item::Polygon(p) => {
            s.polygons.insert(position, *p);
            FrameRef::Polygon(position)
        }
    };
    register_frame_ref(s, new_ref, position, z_slot);
    if let Some(gid) = &group_home {
        let group_idx = s
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(gid.as_str()))
            .expect("resolved above: the group is on this spread");
        seat_in_group(s, new_ref, group_idx, z_slot);
    }
    let id = node.self_id().to_string();
    if let Some(labels) = labels {
        s.labels.insert(id.clone(), labels);
    }
    if let Some(meta) = image_metadata {
        s.image_metadata.insert(id, meta);
    }

    Ok(AppliedOperation {
        op: Operation::InsertNode {
            parent: parent.clone(),
            position,
            node: spec.clone(),
            z_slot,
        },
        inverse: invert_insert_node(spec),
        invalidation: InvalidationHint {
            structural: true,
            ..Default::default()
        },
    })
}

pub(super) fn apply_insert_node(
    doc: &mut Document,
    parent: &NodeId,
    position: usize,
    z_slot: Option<usize>,
    spec: &NodeSpec,
) -> Result<AppliedOperation, OperationError> {
    // Phase H — CloneTranslate is a special "find the source, copy
    // it into its own spread" path. It ignores `parent` and uses the
    // source's host spread, so the gesture-spine caller doesn't have
    // to discover the spread itself.
    if let NodeSpec::CloneTranslate { .. } = spec {
        return apply_insert_clone_translate(doc, position, spec);
    }
    // S-03 — a table is in-story content, not a page item. It targets a
    // `NodeId::Story` parent and nests under `Paragraph::table`, so it
    // takes a wholly different path from the spread-bound shape inserts.
    if let NodeSpec::Table { .. } = spec {
        return apply_insert_table(doc, parent, position, spec);
    }
    // C-75 — a node captured whole by `RemoveNode` goes back verbatim.
    if let NodeSpec::Captured { .. } = spec {
        return apply_insert_captured(doc, parent, position, z_slot, spec);
    }
    let (parent_id, group_home) = resolve_parent(doc, parent, spec)?;
    let parent_id = &parent_id;

    // Uniqueness across the document — IDML Self IDs must be unique.
    let new_self_id = spec.node_id();
    if node_exists(doc, &new_self_id) {
        return Err(OperationError::DuplicateNodeId {
            id: new_self_id.self_id().to_string(),
        });
    }

    // A TextFrame spec may name a `ParentStory` (InDesign's model — the
    // wire's InsertTextFrame mapping MINTS one so a fresh frame's story
    // is immediately addressable: `hitTest` resolved `storyId: null`
    // and no caller could pour text into a new frame, found live by the
    // sheets K-1 e2e). `Some(id)` attaches; an id with no parsed story
    // yet CREATES the empty story (the fresh-insert case, and the redo
    // of an undone insert). `None` attaches nothing — the legacy
    // story-less shape stays byte-identical across remove → undo (the
    // kernel invariant). Runs BEFORE the spread borrow (`doc.stories`).
    let text_frame_story: Option<String> = match spec {
        NodeSpec::TextFrame {
            parent_story: Some(id),
            ..
        } => {
            ensure_story(doc, id);
            Some(id.clone())
        }
        _ => None,
    };

    let spread = find_spread_mut(doc, parent_id)
        .ok_or_else(|| OperationError::NodeNotFound(parent.clone()))?;

    let invalidation = InvalidationHint {
        structural: true,
        ..Default::default()
    };

    match spec {
        NodeSpec::TextFrame {
            self_id,
            bounds,
            fill_color,
            stroke_color,
            stroke_weight,
            item_transform,
            parent_story: _,
        } => {
            let len = spread.spread.text_frames.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut frame = new_text_frame(
                self_id.clone(),
                bounds_from_array(*bounds),
                fill_color.clone(),
            );
            frame.stroke_color = stroke_color.clone();
            frame.stroke_weight = *stroke_weight;
            frame.item_transform = *item_transform;
            // Minted or reattached above (before the spread borrow).
            frame.parent_story = text_frame_story.clone();
            spread.spread.text_frames.insert(position, frame);
            register_frame_ref(&mut spread.spread, FrameRef::TextFrame(0), position, z_slot);
        }
        NodeSpec::Rectangle {
            self_id,
            bounds,
            fill_color,
            stroke_color,
            stroke_weight,
            item_transform,
        } => {
            let len = spread.spread.rectangles.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut rect = new_rectangle(
                self_id.clone(),
                bounds_from_array(*bounds),
                fill_color.clone(),
            );
            rect.stroke_color = stroke_color.clone();
            rect.stroke_weight = *stroke_weight;
            rect.item_transform = *item_transform;
            spread.spread.rectangles.insert(position, rect);
            register_frame_ref(&mut spread.spread, FrameRef::Rectangle(0), position, z_slot);
        }
        NodeSpec::Oval {
            self_id,
            bounds,
            fill_color,
            stroke_color,
            stroke_weight,
            item_transform,
        } => {
            let len = spread.spread.ovals.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut oval = new_oval(
                self_id.clone(),
                bounds_from_array(*bounds),
                fill_color.clone(),
            );
            oval.stroke_color = stroke_color.clone();
            oval.stroke_weight = *stroke_weight;
            oval.item_transform = *item_transform;
            spread.spread.ovals.insert(position, oval);
            register_frame_ref(&mut spread.spread, FrameRef::Oval(0), position, z_slot);
        }
        NodeSpec::GraphicLine {
            self_id,
            bounds,
            anchors,
            subpath_starts,
            subpath_open,
            stroke_color,
            stroke_weight,
            item_transform,
        } => {
            let len = spread.spread.graphic_lines.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            let mut line = new_graphic_line(
                self_id.clone(),
                bounds_from_array(*bounds),
                anchors.iter().map(PathAnchorSpec::to_parse).collect(),
                subpath_starts.clone(),
                subpath_open.clone(),
                stroke_color.clone(),
                *stroke_weight,
            );
            line.item_transform = *item_transform;
            spread.spread.graphic_lines.insert(position, line);
            register_frame_ref(
                &mut spread.spread,
                FrameRef::GraphicLine(0),
                position,
                z_slot,
            );
        }
        NodeSpec::Polygon {
            self_id,
            bounds,
            anchors,
            subpath_starts,
            subpath_open,
            fill_color,
            stroke_color,
            stroke_weight,
            item_transform,
        } => {
            let len = spread.spread.polygons.len();
            if position > len {
                return Err(OperationError::InvalidPosition {
                    parent: parent.clone(),
                    position,
                    len,
                });
            }
            // The path is the truth: its own box, not the one the wire
            // handed us beside it (see `path_topology::anchors_bounds`).
            let parsed_anchors: Vec<paged_model::PathAnchor> =
                anchors.iter().map(PathAnchorSpec::to_parse).collect();
            let bounds = super::path_topology::anchors_bounds(&parsed_anchors)
                .unwrap_or_else(|| bounds_from_array(*bounds));
            let mut poly = new_polygon(
                self_id.clone(),
                bounds,
                parsed_anchors,
                subpath_starts.clone(),
                subpath_open.clone(),
                fill_color.clone(),
                stroke_color.clone(),
                *stroke_weight,
            );
            poly.item_transform = *item_transform;
            spread.spread.polygons.insert(position, poly);
            register_frame_ref(&mut spread.spread, FrameRef::Polygon(0), position, z_slot);
        }
        NodeSpec::CloneTranslate { .. } => {
            // Handled by `apply_insert_clone_translate` above.
            unreachable!("CloneTranslate routed via the early-return");
        }
        NodeSpec::Table { .. } => {
            // S-03 — handled by `apply_insert_table` via the early-return
            // (a table targets a `NodeId::Story`, not this spread path).
            unreachable!("Table insert routed via the early-return");
        }
        NodeSpec::Captured { .. } => {
            unreachable!("Captured routed via the early-return");
        }
    }

    // C-74 — a group parent: the arms above registered the item in the
    // z-table like any other insert (which also renumbered every list);
    // re-seat it in the group's members instead.
    if let Some(gid) = &group_home {
        let new_ref = match spec {
            NodeSpec::TextFrame { .. } => FrameRef::TextFrame(position),
            NodeSpec::Rectangle { .. } => FrameRef::Rectangle(position),
            NodeSpec::Oval { .. } => FrameRef::Oval(position),
            NodeSpec::GraphicLine { .. } => FrameRef::GraphicLine(position),
            NodeSpec::Polygon { .. } => FrameRef::Polygon(position),
            NodeSpec::CloneTranslate { .. }
            | NodeSpec::Table { .. }
            | NodeSpec::Captured { .. } => {
                unreachable!("routed via the early-returns")
            }
        };
        let group_idx = spread
            .spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(gid.as_str()))
            .expect("resolved above: the group is on this spread");
        seat_in_group(&mut spread.spread, new_ref, group_idx, z_slot);
    }

    let inverse = invert_insert_node(spec);
    Ok(AppliedOperation {
        op: Operation::InsertNode {
            parent: parent.clone(),
            position,
            node: spec.clone(),
            z_slot,
        },
        inverse,
        invalidation,
    })
}
