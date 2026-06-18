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

//! C-1 Stage B (pixel save-back) — `Operation::ReplaceImageBytes`: commit
//! a placed graphic frame's processed pixels as the frame's INLINE
//! `image_bytes` (the decoded payload the renderer prefers over a
//! `<Link>` uri — the same lane parsed inline-CDATA images use). The
//! companion to the ephemeral per-drag `SubmitPixelLayer` preview: that
//! composites tiles over the frame DURING a gesture; this is the single
//! undoable mutation that COMMITS the result into the document.
//!
//! Mirrors `apply_place_image`'s self-inverse shape: the inverse is
//! another `ReplaceImageBytes` carrying the prior bytes + the prior
//! `has_image_element` flag, so undo restores both losslessly (was-absent
//! → absent). It deliberately does NOT touch `image_link` /
//! `image_item_transform`: bytes outrank the link in the renderer, and
//! keeping the transform lands the new pixels in the same place.

use paged_scene::Document;

use super::helpers::{find_oval_mut, find_polygon_mut, find_rectangle_mut};
use crate::error::OperationError;
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation, PropertyPath};

pub(super) fn apply_replace_image_bytes(
    doc: &mut Document,
    frame: &NodeId,
    bytes: Option<&[u8]>,
    // Inverse-only on the incoming op: when `Some`, this is the prior flag
    // an undo must restore. On a forward op it's `None` and the apply
    // layer sets `has_image_element = true` (installing bytes makes the
    // frame an image element; clearing them does NOT auto-clear the flag —
    // a frame that was an image element with an unreachable link stays one).
    prior_has_image_element_override: Option<bool>,
) -> Result<AppliedOperation, OperationError> {
    let prior_bytes: Option<Vec<u8>>;
    let prior_has_image_element: bool;
    // `has_image_element` is set true on any forward op (the frame now
    // carries bytes, even a clear leaves it an image element); the inverse
    // restores whatever it was. `prior_has_image_element_override` lets the
    // inverse force the captured prior value back.
    let new_bytes = bytes.map(<[u8]>::to_vec);
    match frame {
        NodeId::Rectangle(id) => {
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(frame.clone()))?;
            prior_bytes = rect.image_bytes.take();
            prior_has_image_element = rect.has_image_element;
            rect.image_bytes = new_bytes;
            rect.has_image_element = prior_has_image_element_override.unwrap_or(true);
        }
        NodeId::Oval(id) => {
            let oval = find_oval_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(frame.clone()))?;
            prior_bytes = oval.image_bytes.take();
            prior_has_image_element = oval.has_image_element;
            oval.image_bytes = new_bytes;
            oval.has_image_element = prior_has_image_element_override.unwrap_or(true);
        }
        NodeId::Polygon(id) => {
            let poly = find_polygon_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(frame.clone()))?;
            prior_bytes = poly.image_bytes.take();
            prior_has_image_element = poly.has_image_element;
            poly.image_bytes = new_bytes;
            poly.has_image_element = prior_has_image_element_override.unwrap_or(true);
        }
        other => {
            return Err(OperationError::InvalidValue {
                node: other.clone(),
                path: PropertyPath::FrameFittingType,
                reason: "ReplaceImageBytes targets Rectangle / Oval / Polygon frames".to_string(),
            })
        }
    }

    let invalidation = InvalidationHint {
        frame_geometry: vec![frame.clone()],
        ..Default::default()
    };
    Ok(AppliedOperation {
        op: Operation::ReplaceImageBytes {
            frame: frame.clone(),
            bytes: bytes.map(<[u8]>::to_vec),
            // The echoed forward op carries the (true) flag it set, so a
            // redo reproduces the placement exactly.
            prior_has_image_element: Some(prior_has_image_element_override.unwrap_or(true)),
        },
        inverse: Operation::ReplaceImageBytes {
            frame: frame.clone(),
            bytes: prior_bytes,
            prior_has_image_element: Some(prior_has_image_element),
        },
        invalidation,
    })
}
