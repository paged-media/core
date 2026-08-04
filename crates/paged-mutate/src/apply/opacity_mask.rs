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

//! C-23 opacity masks: `Operation::ApplyOpacityMask` /
//! `Operation::ReleaseOpacityMask` — move a top-level page item into a
//! target's [`Spread::opacity_masks`] entry as its mask ARTWORK, and
//! back out again.
//!
//! Modelled on the B-18 [`super::nested`] pair (`PasteInto` /
//! `ReleaseFrom`), which is the closest existing gesture: both consume
//! a top-level item into another item's side map, both preserve
//! spread-space geometry untouched, and both are pure bookkeeping
//! between the z-table (`frames_in_order`) and a side map. The
//! difference is only what the consumed item MEANS — a pasted child is
//! painted clipped by its host, a mask item is not painted at all; its
//! coverage modulates the target's alpha.

use paged_model::{FrameRef, OpacityMask, OpacityMaskType, Spread};

use super::insert_node::ensure_frames_in_order;
use super::nested::leaf_ref_in_spread;
use crate::error::OperationError;
use crate::operation::{
    AppliedOperation, InvalidationHint, NodeId, OpacityMaskMode, Operation, PropertyPath,
};

fn invalid(node: &NodeId, reason: String) -> OperationError {
    OperationError::InvalidValue {
        node: node.clone(),
        path: PropertyPath::FrameTransform,
        reason,
    }
}

/// C-23 — the kinds that may take part in a mask, on either side.
///
/// Deliberately excludes `TextFrame`. A text frame's GLYPHS are
/// emitted by a later story pass, not by the frame-body pass the mask
/// bracket wraps, so a TextFrame target would paint its box masked and
/// its type unmasked, and a TextFrame mask would contribute a shape
/// with no letters in it. Rather than half-wire that (the B-18 route —
/// `StoryEmitter::apply_container_clip` splices the bracket over the
/// glyph range — is a real option, just a separate piece of work) both
/// sides are gated to the graphic kinds, and a TextFrame is rejected
/// with an error that says exactly why.
fn maskable(node: &NodeId) -> bool {
    matches!(
        node,
        NodeId::Rectangle(_) | NodeId::Oval(_) | NodeId::GraphicLine(_) | NodeId::Polygon(_)
    )
}

fn reject_text_frame(node: &NodeId) -> OperationError {
    invalid(
        node,
        format!(
            "C-23: a {} cannot take part in an opacity mask (Rectangle / Oval / \
             GraphicLine / Polygon only — a text frame's glyphs are emitted by the \
             story pass, outside the mask bracket)",
            node.kind()
        ),
    )
}

/// True when `node` is currently serving as some target's mask
/// artwork. `apply_remove_node` gates on this for the same reason it
/// gates on [`super::nested::is_nested_child`]: deleting a mask item
/// directly would produce a `RemoveNode` inverse that restores it
/// TOP-LEVEL, silently breaking the mutate-then-undo identity.
/// Callers release the mask first.
pub(super) fn is_mask_item(doc: &paged_scene::Document, node: &NodeId) -> bool {
    let Some(id) = node_self_id(node) else {
        return false;
    };
    doc.spreads
        .iter()
        .any(|p| p.spread.opacity_masks.values().any(|m| m.mask_item == id))
}

/// True when `node` currently carries an opacity mask.
pub(super) fn is_masked_target(doc: &paged_scene::Document, node: &NodeId) -> bool {
    let Some(id) = node_self_id(node) else {
        return false;
    };
    doc.spreads
        .iter()
        .any(|p| p.spread.opacity_masks.contains_key(&id))
}

fn node_self_id(node: &NodeId) -> Option<String> {
    maskable(node).then(|| node.self_id().to_string())
}

/// Resolve a leaf id to its `FrameRef` and confirm it lives on this
/// spread.
fn ref_here(spread: &Spread, node: &NodeId) -> Option<FrameRef> {
    leaf_ref_in_spread(spread, node)
}

/// C-23 — `Operation::ApplyOpacityMask`. The mask item leaves the
/// z-table (its slot is captured into the inverse) and becomes the
/// target's mask artwork. **Geometry is untouched on both sides**: the
/// mask covers whatever it geometrically overlaps, exactly like
/// Illustrator's Make Opacity Mask.
///
/// Validation: both ids exist on the SAME spread, both are maskable
/// kinds, they are not the same item, the mask item is currently
/// top-level (not grouped, not pasted-in, not already a mask), and the
/// target does not already carry a mask (release it first — an
/// implicit replace would lose the old mask item's z slot).
pub(super) fn apply_opacity_mask(
    doc: &mut paged_scene::Document,
    target: &NodeId,
    mask: &NodeId,
    mask_type: OpacityMaskType,
    invert: bool,
) -> Result<AppliedOperation, OperationError> {
    if !maskable(target) {
        return Err(reject_text_frame(target));
    }
    if !maskable(mask) {
        return Err(reject_text_frame(mask));
    }
    if target.self_id() == mask.self_id() {
        return Err(invalid(mask, "C-23: an item cannot mask itself".into()));
    }
    let target_id = target.self_id().to_string();

    for parsed in doc.spreads.iter_mut() {
        let spread = &mut parsed.spread;
        let Some(mask_ref) = ref_here(spread, mask) else {
            continue;
        };
        if ref_here(spread, target).is_none() {
            return Err(invalid(
                target,
                "C-23: the mask and the masked item must live on the same spread".into(),
            ));
        }
        if spread.opacity_masks.contains_key(&target_id) {
            return Err(invalid(
                target,
                "C-23: the item already carries an opacity mask (release it first)".into(),
            ));
        }
        if spread
            .opacity_masks
            .values()
            .any(|m| m.mask_item == mask.self_id())
        {
            return Err(invalid(
                mask,
                "C-23: the item is already serving as a mask (release it first)".into(),
            ));
        }
        if spread
            .nested_children
            .values()
            .any(|v| v.contains(&mask_ref))
        {
            return Err(invalid(
                mask,
                "C-23: a pasted-in item cannot become a mask (release it first)".into(),
            ));
        }
        if spread.groups.iter().any(|g| g.members.contains(&mask_ref)) {
            return Err(invalid(
                mask,
                "C-23: a grouped item cannot become a mask (ungroup first)".into(),
            ));
        }
        ensure_frames_in_order(spread);
        let Some(slot) = spread.frames_in_order.iter().position(|r| *r == mask_ref) else {
            return Err(invalid(mask, "C-23: the mask item is not top-level".into()));
        };
        spread.frames_in_order.remove(slot);
        spread.opacity_masks.insert(
            target_id,
            OpacityMask {
                mask_item: mask.self_id().to_string(),
                mask_type,
                invert,
            },
        );
        return Ok(AppliedOperation {
            op: Operation::ApplyOpacityMask {
                target: target.clone(),
                mask: mask.clone(),
                mask_type: OpacityMaskMode::from_model(mask_type),
                invert,
            },
            inverse: Operation::ReleaseOpacityMask {
                target: target.clone(),
                restore_slot: Some(slot),
            },
            invalidation: InvalidationHint {
                structural: true,
                frame_geometry: vec![target.clone(), mask.clone()],
                ..Default::default()
            },
        });
    }
    Err(OperationError::NodeNotFound(mask.clone()))
}

/// C-23 — `Operation::ReleaseOpacityMask`: drop the relation and pop
/// the mask artwork back to top level. World transform preserved (the
/// stored transform was never touched), so nothing moves on canvas —
/// the artwork simply becomes visible again as its own object, which
/// is what Illustrator's Release Opacity Mask does.
///
/// `restore_slot` is **inverse-only**: undo-of-apply restores the mask
/// item's exact stacking position; a user-initiated release passes
/// `None` and the artwork stacks on top.
pub(super) fn apply_release_opacity_mask(
    doc: &mut paged_scene::Document,
    target: &NodeId,
    restore_slot: Option<usize>,
) -> Result<AppliedOperation, OperationError> {
    if !maskable(target) {
        return Err(reject_text_frame(target));
    }
    let target_id = target.self_id().to_string();
    for parsed in doc.spreads.iter_mut() {
        let spread = &mut parsed.spread;
        let Some(entry) = spread.opacity_masks.get(&target_id).cloned() else {
            continue;
        };
        // Resolve the mask item's NodeId by finding which backing vec
        // owns the id. An orphaned entry (the artwork was deleted out
        // from under the relation) still releases — that is the
        // recovery path for a trapped target — but the map entry is
        // simply dropped, since there is nothing to restore.
        let mask_node = mask_node_id(spread, &entry.mask_item);
        spread.opacity_masks.remove(&target_id);
        let Some(mask_node) = mask_node else {
            return Ok(AppliedOperation {
                op: Operation::ReleaseOpacityMask {
                    target: target.clone(),
                    restore_slot,
                },
                // The artwork is gone, so there is no faithful inverse
                // to offer: re-applying would need an item that no
                // longer exists. We hand back a release of the same
                // target, which now FAILS honestly ("carries no
                // opacity mask") rather than silently mis-restoring.
                // Unreachable through the wire — `RemoveNode` refuses
                // to delete a live mask item — so this is the
                // recovery path for a hand-authored model only.
                inverse: Operation::ReleaseOpacityMask {
                    target: target.clone(),
                    restore_slot: None,
                },
                invalidation: InvalidationHint {
                    structural: true,
                    frame_geometry: vec![target.clone()],
                    ..Default::default()
                },
            });
        };
        let Some(mask_ref) = ref_here(spread, &mask_node) else {
            return Err(OperationError::NodeNotFound(mask_node));
        };
        ensure_frames_in_order(spread);
        let len = spread.frames_in_order.len();
        let slot = restore_slot.unwrap_or(len).min(len);
        spread.frames_in_order.insert(slot, mask_ref);
        return Ok(AppliedOperation {
            op: Operation::ReleaseOpacityMask {
                target: target.clone(),
                restore_slot,
            },
            inverse: Operation::ApplyOpacityMask {
                target: target.clone(),
                mask: mask_node.clone(),
                mask_type: OpacityMaskMode::from_model(entry.mask_type),
                invert: entry.invert,
            },
            invalidation: InvalidationHint {
                structural: true,
                frame_geometry: vec![target.clone(), mask_node],
                ..Default::default()
            },
        });
    }
    Err(invalid(
        target,
        "C-23: the item carries no opacity mask".into(),
    ))
}

/// Which backing vec owns `id` — the mask item's kind, recovered for
/// the inverse's `NodeId`.
fn mask_node_id(spread: &Spread, id: &str) -> Option<NodeId> {
    if spread
        .rectangles
        .iter()
        .any(|r| r.self_id.as_deref() == Some(id))
    {
        Some(NodeId::Rectangle(id.to_string()))
    } else if spread
        .ovals
        .iter()
        .any(|o| o.self_id.as_deref() == Some(id))
    {
        Some(NodeId::Oval(id.to_string()))
    } else if spread
        .graphic_lines
        .iter()
        .any(|l| l.self_id.as_deref() == Some(id))
    {
        Some(NodeId::GraphicLine(id.to_string()))
    } else if spread
        .polygons
        .iter()
        .any(|p| p.self_id.as_deref() == Some(id))
    {
        Some(NodeId::Polygon(id.to_string()))
    } else {
        None
    }
}
