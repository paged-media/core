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

//! v59 — `Operation::ReorderNode` (Arrange: bring to front / send to
//! back / bring forward / send backward / restack to an exact slot).
//!
//! One list, permuted in place. The op carries no parent: the sibling
//! list is DERIVED from where the node already lives, which is what
//! makes "a reorder cannot smuggle an item out of a group or into a
//! B-18 container" a structural property rather than a validation rule
//! somebody has to remember to keep in sync with `PasteInto`'s.

use paged_model::{FrameRef, Spread};

use super::insert_node::ensure_frames_in_order;
use super::nested::leaf_ref_in_spread;
use crate::error::OperationError;
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, Operation, PropertyPath, ZOrderTarget,
};

/// Which list holds the node's siblings — i.e. what a reorder is
/// allowed to touch. Each variant names the list's OWNER so an
/// out-of-range `Index` can report against it.
enum Scope {
    /// The spread's cross-shape z table.
    TopLevel,
    /// `Spread::groups[i].members`.
    Group(usize),
    /// `Spread::nested_children[host]` (B-18 paste-into).
    Nested(String),
}

fn invalid(node: &NodeId, reason: String) -> OperationError {
    OperationError::InvalidValue {
        node: node.clone(),
        // The z table is frame-stacking state; `FrameTransform` is the
        // same stand-in path the B-18 pair uses for structural errors
        // (`PropertyPath` has no "structure" member).
        path: PropertyPath::FrameTransform,
        reason,
    }
}

/// Resolve a page-item `NodeId` to its `FrameRef` in one spread —
/// [`leaf_ref_in_spread`] plus `Group`, which is a first-class z-table
/// entry and therefore reorderable like any leaf.
fn item_ref_in_spread(spread: &Spread, node: &NodeId) -> Option<FrameRef> {
    match node {
        NodeId::Group(id) => spread
            .groups
            .iter()
            .position(|g| g.self_id.as_deref() == Some(id.as_str()))
            .map(FrameRef::Group),
        other => leaf_ref_in_spread(spread, other),
    }
}

/// The list `scope` names, mutably.
fn list_mut<'a>(spread: &'a mut Spread, scope: &Scope) -> &'a mut Vec<FrameRef> {
    match scope {
        Scope::TopLevel => &mut spread.frames_in_order,
        Scope::Group(gi) => &mut spread.groups[*gi].members,
        Scope::Nested(host) => spread
            .nested_children
            .get_mut(host)
            .expect("nested host resolved moments ago"),
    }
}

/// The `NodeId` that OWNS the sibling list — the "parent" an
/// out-of-range `Index` is reported against.
fn scope_owner(spread: &Spread, scope: &Scope) -> NodeId {
    match scope {
        Scope::TopLevel => NodeId::Spread(spread.self_id.clone().unwrap_or_default()),
        Scope::Group(gi) => NodeId::Group(
            spread.groups[*gi]
                .self_id
                .clone()
                .unwrap_or_else(|| format!("Group/#{gi}")),
        ),
        // The host kind is Rectangle / Oval / Polygon (PasteInto's
        // gate); the id alone identifies it for an error message, and
        // Rectangle is the overwhelmingly common host.
        Scope::Nested(host) => NodeId::Rectangle(host.clone()),
    }
}

pub(super) fn apply_reorder_node(
    doc: &mut paged_scene::Document,
    node: &NodeId,
    target: ZOrderTarget,
) -> Result<AppliedOperation, OperationError> {
    // Kind gate up front so a Story / Table / StoryRange / Layer /
    // Spread / Page target fails with a message that says WHY, rather
    // than falling through every spread into a bare `NodeNotFound`.
    if !matches!(
        node,
        NodeId::TextFrame(_)
            | NodeId::Rectangle(_)
            | NodeId::Oval(_)
            | NodeId::GraphicLine(_)
            | NodeId::Polygon(_)
            | NodeId::Group(_)
    ) {
        return Err(invalid(
            node,
            format!(
                "a {} has no stacking position (page items and groups only)",
                node.kind()
            ),
        ));
    }

    for parsed in doc.spreads.iter_mut() {
        let spread = &mut parsed.spread;
        let Some(node_ref) = item_ref_in_spread(spread, node) else {
            continue;
        };

        // Locate the sibling list. Group membership and B-18 nesting
        // are checked BEFORE the z table, because a grouped / nested
        // item is deliberately absent from `frames_in_order` — and
        // `ensure_frames_in_order` must not run for those (it only
        // materialises top-level order and is a no-op once the table
        // is populated, but the intent is clearer in this order).
        let scope = if let Some(gi) = spread
            .groups
            .iter()
            .position(|g| g.members.contains(&node_ref))
        {
            Scope::Group(gi)
        } else if let Some(host) = spread
            .nested_children
            .iter()
            .find_map(|(h, kids)| kids.contains(&node_ref).then(|| h.clone()))
        {
            Scope::Nested(host)
        } else if spread
            .opacity_masks
            .values()
            .any(|m| Some(m.mask_item.as_str()) == item_self_id(spread, node_ref))
        {
            // C-28 — mask artwork is consumed BY the mask and painted
            // from no list at all. There is nothing to reorder it
            // against; releasing the mask puts it back on the z table.
            return Err(invalid(
                node,
                "C-28: the item is an opacity mask — it is painted from no stacking list \
                 (release the mask first)"
                    .into(),
            ));
        } else {
            // Render-neutral: materialising an empty z table reproduces
            // exactly the order the renderer already synthesises from
            // the kind vecs, so nothing moves — it just makes the
            // implicit order explicit so a reorder can address it. This
            // is what makes `ReorderNode` work on a synthesised blank
            // document (File ▸ New), where the parse never filled it.
            ensure_frames_in_order(spread);
            if !spread.frames_in_order.contains(&node_ref) {
                return Err(invalid(node, "the item is not in any stacking list".into()));
            }
            Scope::TopLevel
        };

        let owner = scope_owner(spread, &scope);
        let list = list_mut(spread, &scope);
        let len = list.len();
        let from = list
            .iter()
            .position(|r| *r == node_ref)
            .expect("membership established above");

        // `to` is the node's FINAL slot in the resulting list, so
        // `remove(from) + insert(to)` and its mirror are exact
        // inverses of each other. `len >= 1` here (the node is in it).
        let last = len - 1;
        let to = match target {
            ZOrderTarget::Front => last,
            ZOrderTarget::Back => 0,
            ZOrderTarget::Forward => (from + 1).min(last),
            ZOrderTarget::Backward => from.saturating_sub(1),
            ZOrderTarget::Index(i) => {
                if i > last {
                    // Loud, not clamped: an absolute index this far off
                    // means the caller's model is stale, and silently
                    // restacking to "as close as we could" would hide
                    // that. The relative verbs exist precisely so a
                    // caller need not hold a fresh index.
                    return Err(OperationError::InvalidPosition {
                        parent: owner,
                        position: i,
                        len,
                    });
                }
                i
            }
        };

        // A no-op reorder (already frontmost + `Front`, `Index(from)`,
        // …) still applies successfully and still logs an inverse. It
        // costs one undo step and restores the same order — cheaper
        // than making every caller pre-check, and matching InDesign,
        // which also pushes an undo entry for a no-op Arrange.
        list.remove(from);
        list.insert(to, node_ref);

        return Ok(AppliedOperation {
            op: Operation::ReorderNode {
                node: node.clone(),
                target,
            },
            // EXACT inverse for every verb: go back to the slot we
            // came from. Same property `RemoveNode`'s `z_slot` gives
            // undo-of-delete.
            inverse: Operation::ReorderNode {
                node: node.clone(),
                target: ZOrderTarget::Index(from),
            },
            invalidation: InvalidationHint {
                structural: true,
                ..Default::default()
            },
        });
    }
    Err(OperationError::NodeNotFound(node.clone()))
}

/// The `Self` id behind a `FrameRef`, for the opacity-mask check.
fn item_self_id(spread: &Spread, r: FrameRef) -> Option<&str> {
    match r {
        FrameRef::TextFrame(i) => spread.text_frames.get(i)?.self_id.as_deref(),
        FrameRef::Rectangle(i) => spread.rectangles.get(i)?.self_id.as_deref(),
        FrameRef::Oval(i) => spread.ovals.get(i)?.self_id.as_deref(),
        FrameRef::GraphicLine(i) => spread.graphic_lines.get(i)?.self_id.as_deref(),
        FrameRef::Polygon(i) => spread.polygons.get(i)?.self_id.as_deref(),
        FrameRef::Group(i) => spread.groups.get(i)?.self_id.as_deref(),
    }
}
