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

//! C-77 — a locked page item stays where it is.
//!
//! `elementLocked` used to be paint-neutral bookkeeping the engine never
//! read: `moveFrame` and `deleteFrame` applied to a locked frame, so the
//! lock held only as far as a host chose to honour it, and a script or a
//! plugin went straight through it. The rule is InDesign's, asked of it
//! by script (`tools/indesign-export/object-integrity-probe.json`):
//!
//! - a locked item cannot be moved, resized, rotated, scaled, reshaped
//!   or restacked, nor removed — each refuses with "the object is locked"
//!   (a path-point write is silently ignored there; here it is refused
//!   like every other reshape, so a caller learns why nothing changed);
//! - its ATTRIBUTES still change — fill, stroke, opacity, corners,
//!   effects, nonprinting, text-frame options — and the text in a locked
//!   text frame is edited normally; it can be duplicated, and unlocked;
//! - the content of a locked container is locked too, and does not move.
//!
//! So the engine refuses exactly the operations that would move, reshape,
//! restack or remove a locked item, with [`OperationError::Locked`], and
//! everything else applies. The write that unlocks it is a property
//! write like any other.
//!
//! Undo is never refused by this: an inverse replays against the state
//! its forward op left, which a lock that op did not set cannot be part
//! of — a lock set later is undone first.

use paged_model::{FrameRef, Spread};
use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{NodeId, Operation, PropertyPath};

/// The properties whose write moves or reshapes the item: its box, its
/// transform and the parts of it, its path, and the placed content's
/// position inside it. Corner options are NOT here — InDesign changes
/// them on a locked item.
pub(super) fn moves_geometry(path: PropertyPath) -> bool {
    use PropertyPath as P;
    matches!(
        path,
        P::FrameBounds
            | P::FrameTransform
            | P::FrameRotationAngle
            | P::FrameScaleX
            | P::FrameScaleY
            | P::FrameFlipH
            | P::FrameFlipV
            | P::FramePath
            | P::FramePathPoint
            | P::PathPointInsert
            | P::PathPointRemove
            | P::PathPointCurveType
            | P::PathOpenAt
            | P::OffsetPath
            | P::SimplifyPath
            | P::ClosePath
            | P::ImageContentTransform
    )
}

/// Whether the page item `node` is locked: its own `elementLocked`, or
/// that of the container it is pasted into (InDesign reads the content of
/// a locked container as locked). `false` for anything that is not a
/// leaf page item, and for an item that does not exist — the op that
/// names it reports that.
pub fn is_locked(doc: &Document, node: &NodeId) -> bool {
    doc.spreads.iter().any(|parsed| {
        super::nested::leaf_ref_in_spread(&parsed.spread, node)
            .is_some_and(|r| ref_locked(&parsed.spread, r))
    })
}

/// The lock of the item `r` names, through its pasted-into containers.
fn ref_locked(spread: &Spread, r: FrameRef) -> bool {
    let mut current = r;
    // A container chain is short; the bound only stops a malformed
    // cycle in `nested_children` from looping.
    for _ in 0..64 {
        if own_lock(spread, current) {
            return true;
        }
        match host_of(spread, current) {
            Some(host) => current = host,
            None => return false,
        }
    }
    false
}

fn own_lock(spread: &Spread, r: FrameRef) -> bool {
    match r {
        FrameRef::TextFrame(i) => spread.text_frames.get(i).is_some_and(|f| f.locked),
        FrameRef::Rectangle(i) => spread.rectangles.get(i).is_some_and(|f| f.locked),
        FrameRef::Oval(i) => spread.ovals.get(i).is_some_and(|f| f.locked),
        FrameRef::GraphicLine(i) => spread.graphic_lines.get(i).is_some_and(|f| f.locked),
        FrameRef::Polygon(i) => spread.polygons.get(i).is_some_and(|f| f.locked),
        FrameRef::Group(_) => false,
    }
}

/// The container `r` is pasted into, if any.
fn host_of(spread: &Spread, r: FrameRef) -> Option<FrameRef> {
    let (host_id, _) = spread
        .nested_children
        .iter()
        .find(|(_, children)| children.contains(&r))?;
    let host = Some(host_id.as_str());
    if let Some(i) = spread
        .rectangles
        .iter()
        .position(|f| f.self_id.as_deref() == host)
    {
        return Some(FrameRef::Rectangle(i));
    }
    if let Some(i) = spread
        .ovals
        .iter()
        .position(|f| f.self_id.as_deref() == host)
    {
        return Some(FrameRef::Oval(i));
    }
    if let Some(i) = spread
        .polygons
        .iter()
        .position(|f| f.self_id.as_deref() == host)
    {
        return Some(FrameRef::Polygon(i));
    }
    if let Some(i) = spread
        .text_frames
        .iter()
        .position(|f| f.self_id.as_deref() == host)
    {
        return Some(FrameRef::TextFrame(i));
    }
    spread
        .graphic_lines
        .iter()
        .position(|f| f.self_id.as_deref() == host)
        .map(FrameRef::GraphicLine)
}

/// The first locked page item inside group `group_id`, at any depth.
fn locked_inside_group(doc: &Document, group_id: &str) -> Option<NodeId> {
    for parsed in &doc.spreads {
        let spread = &parsed.spread;
        let Some(gi) = spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(group_id))
        else {
            continue;
        };
        let mut stack: Vec<FrameRef> = spread.groups[gi].members.clone();
        let mut seen = 0usize;
        while let Some(r) = stack.pop() {
            seen += 1;
            if seen > 100_000 {
                break;
            }
            match r {
                FrameRef::Group(i) => {
                    if let Some(g) = spread.groups.get(i) {
                        stack.extend(g.members.iter().copied());
                    }
                }
                leaf if ref_locked(spread, leaf) => {
                    return super::layer::node_for_frame_ref(spread, leaf);
                }
                _ => {}
            }
        }
        return None;
    }
    None
}

fn locked(node: &NodeId, action: &str) -> OperationError {
    OperationError::Locked {
        node: node.clone(),
        action: action.to_string(),
    }
}

fn check_leaf(doc: &Document, node: &NodeId, action: &str) -> Result<(), OperationError> {
    if is_locked(doc, node) {
        return Err(locked(node, action));
    }
    Ok(())
}

fn check_group(doc: &Document, group_id: &str, action: &str) -> Result<(), OperationError> {
    match locked_inside_group(doc, group_id) {
        Some(member) => Err(locked(&member, action)),
        None => Ok(()),
    }
}

/// Refuse `op` when it would move, reshape, restack or remove a locked
/// page item. Called once per op (and per batch child) at the top of the
/// dispatch, so every surface — the wire, `paged invoke`, `paged.*`, a
/// plugin — meets the same rule.
pub(super) fn refuse_if_locked(doc: &Document, op: &Operation) -> Result<(), OperationError> {
    match op {
        Operation::RemoveNode { node } => check_leaf(doc, node, "delete it"),
        Operation::SetProperty { node, path, .. } if moves_geometry(*path) => match node {
            NodeId::Group(gid) => check_group(doc, gid, "move or reshape the group that holds it"),
            _ => check_leaf(doc, node, "move or reshape it"),
        },
        Operation::MoveNode { node, .. } => check_leaf(doc, node, "move it to another spread"),
        // The drag-a-copy gesture: InDesign does not drag a locked item at
        // all. (The copy would carry the lock, and its undo — removing
        // it — would then be refused.) `DuplicateNodes` stays allowed, as
        // Edit > Duplicate is, and its undo does not go through here.
        Operation::InsertNode {
            node: crate::operation::NodeSpec::CloneTranslate { source, .. },
            ..
        } => check_leaf(doc, source, "drag a copy of it"),
        Operation::ReorderNode { node, .. } => check_leaf(doc, node, "change its stacking order"),
        Operation::SetGroupTransform { group, .. } => {
            check_group(doc, group, "move or transform the group that holds it")
        }
        Operation::PathfinderBoolean { kept, others, .. } => std::iter::once(kept)
            .chain(others)
            .try_for_each(|n| check_leaf(doc, n, "combine it with a pathfinder")),
        Operation::JoinPaths { kept, other } => [kept, other]
            .into_iter()
            .try_for_each(|n| check_leaf(doc, n, "join it")),
        Operation::PathfinderRegion { elements, .. }
        | Operation::PathfinderFaces { elements, .. } => elements
            .iter()
            .try_for_each(|n| check_leaf(doc, n, "combine it with a pathfinder")),
        _ => Ok(()),
    }
}
