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

//! Operation algebra: for every `Operation` shape, the matching
//! inverse shape. The functions are tiny — they encode the *algebraic*
//! relationship between an op and its undo, separate from the
//! per-variant *application* logic in `apply.rs`.
//!
//! Per the briefing:
//! - `SetProperty(new)` → `SetProperty(prev)`.
//! - `InsertNode(spec)` → `RemoveNode(spec.node_id())`.
//! - `RemoveNode(node)` → `InsertNode(captured_parent, captured_pos, captured_spec)`.
//! - `MoveNode(new_parent, new_pos)` → `MoveNode(prev_parent, prev_pos)`.
//! - `Batch(ops)` → `Batch(ops.iter().rev().map(invert))`.
//!
//! Apply orchestrates: it captures the "before" state as it walks the
//! op, hands the captured pieces to these helpers, and stores the
//! returned inverse on the `AppliedOperation`.

use crate::operation::{NodeId, NodeSpec, Operation, PropertyPath, Value};

pub fn invert_set_property(node: NodeId, path: PropertyPath, previous_value: Value) -> Operation {
    Operation::SetProperty {
        node,
        path,
        value: previous_value,
    }
}

pub fn invert_insert_node(spec: &NodeSpec) -> Operation {
    Operation::RemoveNode {
        node: spec.node_id(),
    }
}

pub fn invert_remove_node(
    parent: NodeId,
    position: usize,
    captured: NodeSpec,
    z_slot: Option<usize>,
) -> Operation {
    Operation::InsertNode {
        parent,
        position,
        node: captured,
        // Restore the exact `frames_in_order` stacking slot the node
        // occupied, not just its kind-vec position.
        z_slot,
    }
}

pub fn invert_move_node(
    node: NodeId,
    previous_parent: NodeId,
    previous_position: usize,
) -> Operation {
    Operation::MoveNode {
        node,
        new_parent: previous_parent,
        position: previous_position,
    }
}

/// Combine the per-child inverses into the batch's inverse. Children
/// are reversed so the undo applies them in the opposite order from
/// the forward batch — the only order that recovers the original state.
pub fn invert_batch(child_inverses: Vec<Operation>) -> Operation {
    let reversed: Vec<Operation> = child_inverses.into_iter().rev().collect();
    Operation::Batch { ops: reversed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_property_inverse_carries_previous_value() {
        let node = NodeId::TextFrame("TextFrame/u1".to_string());
        let inv = invert_set_property(
            node.clone(),
            PropertyPath::FrameBounds,
            Value::Bounds([1.0, 2.0, 3.0, 4.0]),
        );
        assert_eq!(
            inv,
            Operation::SetProperty {
                node,
                path: PropertyPath::FrameBounds,
                value: Value::Bounds([1.0, 2.0, 3.0, 4.0]),
            }
        );
    }

    #[test]
    fn insert_inverse_is_remove_with_matching_id() {
        let spec = NodeSpec::TextFrame {
            stroke_color: None,
            stroke_weight: None,
            self_id: "TextFrame/new".to_string(),
            bounds: [0.0, 0.0, 10.0, 10.0],
            fill_color: None,
            item_transform: None,
        };
        assert_eq!(
            invert_insert_node(&spec),
            Operation::RemoveNode {
                node: NodeId::TextFrame("TextFrame/new".to_string())
            }
        );
    }

    #[test]
    fn batch_inverse_reverses_child_order() {
        let a = Operation::RemoveNode {
            node: NodeId::TextFrame("a".into()),
        };
        let b = Operation::RemoveNode {
            node: NodeId::TextFrame("b".into()),
        };
        let inv = invert_batch(vec![a.clone(), b.clone()]);
        // Forward batch was [insert(a), insert(b)]; per-child inverses
        // computed at apply time were [remove(a), remove(b)]; the
        // batch inverse must apply remove(b) first then remove(a).
        assert_eq!(inv, Operation::Batch { ops: vec![b, a] });
    }
}
