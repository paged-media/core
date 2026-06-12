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
use paged_parse::{FrameRef, Spread};
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
    if spread.frames_in_order.is_empty() {
        return; // legacy vec-walk fallback covers this spread
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

/// Unregister a page item removed from `vec_pos` of its kind vec;
/// returns the z slot it occupied so the `RemoveNode` inverse can
/// restore the exact stacking position.
pub(super) fn unregister_frame_ref(spread: &mut Spread, template: FrameRef, vec_pos: usize) -> Option<usize> {
    if spread.frames_in_order.is_empty() {
        return None;
    }
    let target = fr_with_index(&template, vec_pos);
    let slot = spread
        .frames_in_order
        .iter()
        .position(|fr| fr_same_kind(fr, &target) && fr_index(fr) == vec_pos);
    if let Some(s) = slot {
        spread.frames_in_order.remove(s);
    }
    for fr in spread.frames_in_order.iter_mut() {
        if fr_same_kind(fr, &template) {
            let i = fr_index(fr);
            if i > vec_pos {
                *fr = fr_with_index(fr, i - 1);
            }
        }
    }
    slot
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
    let parent_id = match parent {
        NodeId::Spread(id) => id,
        _ => {
            return Err(OperationError::InvalidParent {
                parent: parent.clone(),
                child_kind: spec.node_id().kind().to_string(),
            })
        }
    };

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
            if !doc.stories.iter().any(|s| s.self_id == *id) {
                let mut story = paged_parse::Story::default();
                // One empty paragraph + run — the shape an empty parsed
                // story has; the text ops' `locate()` needs ≥1 paragraph.
                story.paragraphs.push(paged_parse::Paragraph {
                    runs: vec![paged_parse::CharacterRun::default()],
                    ..Default::default()
                });
                doc.stories.push(paged_scene::ParsedStory {
                    // No source entry — minted post-parse. NOTE (honest
                    // gap, RFI'd): `paged-write` only PATCHES existing
                    // entries, so a minted story does not survive IDML
                    // export yet — new-entry emission is the v43 batch's
                    // write-side companion.
                    src: String::new(),
                    self_id: id.clone(),
                    story,
                });
            }
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
            let mut poly = new_polygon(
                self_id.clone(),
                bounds_from_array(*bounds),
                anchors.iter().map(PathAnchorSpec::to_parse).collect(),
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

