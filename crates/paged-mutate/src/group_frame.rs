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

//! Where a group's own coordinate space sits.
//!
//! The model stores the two kinds of transform in two spaces, as the IDML
//! parser builds them:
//!
//! - a LEAF's `item_transform` is composed all the way into spread space
//!   (every enclosing group's `ItemTransform` already folded in);
//! - a GROUP's `item_transform` is its raw `ItemTransform`, relative to
//!   the group that holds it (spread space only for a top-level group).
//!
//! So anything that moves page items by a spread-space matrix `M` moves a
//! leaf by `M ∘ leaf`, but a group by `P⁻¹ ∘ M ∘ P ∘ G` (its parent's
//! frame `P` conjugating the move), and a group whose parent moves with it
//! not at all: its frame rides the parent's. Writing `M ∘ G` into a nested
//! group instead doubles a translation and applies a rotation twice.

use paged_model::{FrameRef, Spread};

use crate::path_math::{affine_inverse, affine_multiply, Affine, AFFINE_IDENTITY};

/// The group that holds the group at `gi` as a member, if any.
pub fn parent_group(spread: &Spread, gi: usize) -> Option<usize> {
    spread
        .groups
        .iter()
        .position(|g| g.members.contains(&FrameRef::Group(gi)))
}

/// The spread-space frame the group at `gi` is placed in: every enclosing
/// group's stored transform composed outermost first. `None` is the
/// spread itself (a top-level group, or ancestors that carry none).
pub fn parent_frame(spread: &Spread, gi: usize) -> Option<Affine> {
    let mut acc: Option<Affine> = None;
    let mut current = gi;
    // The cap stops a malformed member list that names an ancestor twice
    // from spinning.
    for _ in 0..spread.groups.len() {
        let Some(parent) = parent_group(spread, current) else {
            break;
        };
        acc = compose(spread.groups[parent].item_transform, acc);
        current = parent;
    }
    acc
}

/// The spread-space transform of the group at `gi` itself: its parent
/// frame composed with its own stored transform.
pub fn composed_transform(spread: &Spread, gi: usize) -> Option<Affine> {
    compose(
        parent_frame(spread, gi),
        spread.groups.get(gi)?.item_transform,
    )
}

/// `outer ∘ inner`, `None` standing for the identity on either side.
pub fn compose(outer: Option<Affine>, inner: Option<Affine>) -> Option<Affine> {
    match (outer, inner) {
        (None, x) => x,
        (Some(o), None) => Some(o),
        (Some(o), Some(i)) => Some(affine_multiply(o, i)),
    }
}

/// A spread-space matrix `m` expressed in `frame`'s space:
/// `frame⁻¹ ∘ m ∘ frame`. `None` for a singular frame.
pub fn in_frame(frame: Option<Affine>, m: Affine) -> Option<Affine> {
    match frame {
        None => Some(m),
        Some(f) => Some(affine_multiply(affine_inverse(f)?, affine_multiply(m, f))),
    }
}

/// A transform within `frame` (`frame`-relative) moved to spread space:
/// `frame ∘ m ∘ frame⁻¹`. `None` for a singular frame.
pub fn out_of_frame(frame: Option<Affine>, m: Affine) -> Option<Affine> {
    match frame {
        None => Some(m),
        Some(f) => Some(affine_multiply(f, affine_multiply(m, affine_inverse(f)?))),
    }
}

/// A group's new stored transform after its spread-space transform became
/// `composed`, placed in `frame`: `frame⁻¹ ∘ composed`. `None` for a
/// singular frame.
pub fn stored_in_frame(frame: Option<Affine>, composed: Affine) -> Option<Affine> {
    match frame {
        None => Some(composed),
        Some(f) => Some(affine_multiply(affine_inverse(f)?, composed)),
    }
}

/// A spread-space shift `(dx, dy)` seen from inside `frame`: the shift
/// along the frame's own axes that moves its contents by `(dx, dy)` on
/// the spread (`frame⁻¹ ∘ T(dx, dy) ∘ frame` is a pure translation by
/// it). A zero shift stays exactly zero. `None` for a singular frame.
pub fn translation_in_frame(frame: Option<Affine>, dx: f32, dy: f32) -> Option<(f32, f32)> {
    let Some([a, b, c, d, _, _]) = frame else {
        return Some((dx, dy));
    };
    let [ia, ib, ic, id, _, _] = affine_inverse([a, b, c, d, 0.0, 0.0])?;
    Some((ia * dx + ic * dy, ib * dx + id * dy))
}

/// `Some(m)` unless `m` is exactly the identity, so a write that lands on
/// the identity stores nothing, as the parser stores an identity.
pub fn none_if_identity(m: Affine) -> Option<Affine> {
    (m != AFFINE_IDENTITY).then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_model::Group;

    fn group(members: Vec<FrameRef>, t: Option<Affine>) -> Group {
        Group {
            members,
            item_transform: t,
            ..Group::default()
        }
    }

    #[test]
    fn a_nested_group_is_placed_in_its_ancestors_frames_outermost_first() {
        let mut spread = Spread::default();
        // groups[0] holds groups[1], which holds groups[2].
        let outer = [1.0, 0.0, 0.0, 1.0, 100.0, 0.0];
        let middle = [0.0, 1.0, -1.0, 0.0, 10.0, 0.0];
        spread.groups = vec![
            group(vec![FrameRef::Group(1)], Some(outer)),
            group(vec![FrameRef::Group(2)], Some(middle)),
            group(vec![], None),
        ];
        assert_eq!(parent_frame(&spread, 0), None);
        assert_eq!(parent_frame(&spread, 1), Some(outer));
        assert_eq!(
            parent_frame(&spread, 2),
            Some(affine_multiply(outer, middle))
        );
        assert_eq!(
            composed_transform(&spread, 1),
            Some(affine_multiply(outer, middle))
        );
    }

    #[test]
    fn a_spread_space_move_round_trips_through_a_frame() {
        let frame = Some([0.0, 1.0, -1.0, 0.0, 30.0, 40.0]);
        let shift = [1.0, 0.0, 0.0, 1.0, 5.0, 7.0];
        let local = in_frame(frame, shift).unwrap();
        // A spread-space shift seen from a frame rotated 90° is a shift
        // along the frame's own axes.
        assert!((local[4] - 7.0).abs() < 1e-5 && (local[5] + 5.0).abs() < 1e-5);
        let back = out_of_frame(frame, local).unwrap();
        for (a, b) in back.iter().zip(shift) {
            assert!((a - b).abs() < 1e-5);
        }
    }
}
