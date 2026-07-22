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

//! v43 (D-14) — `Operation::PlaceImage`: set/clear a graphic frame's
//! `image_link` (the parsed `LinkResourceURI` lane the renderer
//! resolves through `AssetResolver::resolve_image`) plus, on
//! Rectangles, the `FittingOnEmptyFrame` fit. The op deliberately does
//! NOT touch `has_image_element` / `image_bytes` /
//! `image_item_transform`: a uri the resolver can't serve leaves the
//! frame rendering exactly as before (no missing-image badge for a
//! plugin-placed link), and a frame whose IDML embedded inline image
//! bytes keeps rendering those (inline bytes outrank the link in the
//! renderer — the same precedence parsed documents get).

use paged_scene::Document;

use super::helpers::{find_oval_mut, find_polygon_mut, find_rectangle_mut};
use crate::error::OperationError;
use crate::operation::{AppliedOperation, InvalidationHint, NodeId, Operation, PropertyPath};

pub(super) fn apply_place_image(
    doc: &mut Document,
    frame: &NodeId,
    image_uri: Option<&str>,
    fit: Option<&str>,
) -> Result<AppliedOperation, OperationError> {
    // `fit` rides IDML's `<FrameFittingOption>`, which only nests in
    // Rectangles — reject early so the inverse never has to restore a
    // fit we couldn't have written.
    if fit.is_some() && !matches!(frame, NodeId::Rectangle(_)) {
        return Err(OperationError::UnsupportedProperty {
            node: frame.clone(),
            path: PropertyPath::FrameFittingType,
        });
    }

    let prior_link: Option<String>;
    let mut prior_fit: Option<String> = None;
    match frame {
        NodeId::Rectangle(id) => {
            let rect = find_rectangle_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(frame.clone()))?;
            prior_link = rect.image_link.clone();
            rect.image_link = image_uri.map(|u| u.to_string());
            if let Some(fit) = fit {
                let prev_type = rect
                    .frame_fitting
                    .as_ref()
                    .and_then(|f| f.fitting_on_empty_frame.clone());
                // Inverse vocabulary: `Some("")` = "fit was unset"
                // (mirrors the FrameFittingType property arm's
                // empty-string-clears convention).
                prior_fit = Some(prev_type.unwrap_or_default());
                let new_type = (!fit.is_empty()).then(|| fit.to_string());
                match rect.frame_fitting.as_mut() {
                    Some(f) => f.fitting_on_empty_frame = new_type,
                    None => {
                        if new_type.is_some() {
                            rect.frame_fitting = Some(paged_model::FrameFittingOption {
                                fitting_on_empty_frame: new_type,
                                ..Default::default()
                            });
                        }
                    }
                }
            }
        }
        NodeId::Oval(id) => {
            let oval = find_oval_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(frame.clone()))?;
            prior_link = oval.image_link.clone();
            oval.image_link = image_uri.map(|u| u.to_string());
        }
        NodeId::Polygon(id) => {
            let poly = find_polygon_mut(doc, id)
                .ok_or_else(|| OperationError::NodeNotFound(frame.clone()))?;
            prior_link = poly.image_link.clone();
            poly.image_link = image_uri.map(|u| u.to_string());
        }
        other => {
            return Err(OperationError::InvalidValue {
                node: other.clone(),
                path: PropertyPath::FrameFittingType,
                reason: "PlaceImage targets Rectangle / Oval / Polygon frames".to_string(),
            })
        }
    }

    let invalidation = InvalidationHint {
        frame_geometry: vec![frame.clone()],
        ..Default::default()
    };
    Ok(AppliedOperation {
        op: Operation::PlaceImage {
            frame: frame.clone(),
            image_uri: image_uri.map(|u| u.to_string()),
            fit: fit.map(|f| f.to_string()),
        },
        inverse: Operation::PlaceImage {
            frame: frame.clone(),
            image_uri: prior_link,
            // `None` when the forward op didn't touch the fit.
            fit: prior_fit,
        },
        invalidation,
    })
}
