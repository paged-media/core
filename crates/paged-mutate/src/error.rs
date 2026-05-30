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

//! Errors surfaced by [`apply`](crate::apply::apply) and the
//! [`Project`](crate::Project) wrappers around it. Operations are
//! validated lazily at apply time; mismatches between a property's
//! declared kind and the carried value, or a node that doesn't exist
//! at the moment of apply, both produce `OperationError`.
//!
//! `BatchFailed` is the variant that distinguishes itself: it carries
//! the index of the child that failed plus the underlying cause, so a
//! caller can pinpoint *which* op in a Batch broke the atomicity
//! contract without re-running. Per the briefing, a failed batch is
//! rolled back before `apply` returns this error.

use serde::{Deserialize, Serialize};

use crate::operation::{NodeId, PropertyPath};

#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OperationError {
    #[error("node not found: {0:?}")]
    NodeNotFound(NodeId),

    #[error("property {path:?} is not supported on {node:?}")]
    UnsupportedProperty { node: NodeId, path: PropertyPath },

    #[error("value type for property {path:?} doesn't match (expected {expected})")]
    TypeMismatch {
        path: PropertyPath,
        expected: String,
    },

    /// SDK Phase 3 — the carried `Value` was the right kind for the
    /// path but is semantically invalid for the addressed node. For
    /// example, `(NodeId::StoryRange, CharacterFontSize, Length(_))`
    /// is type-correct but fails if the range is empty or cuts inside
    /// a `CharacterRun` (whole-run-only constraint in this build).
    #[error("invalid value for {path:?} on {node:?}: {reason}")]
    InvalidValue {
        node: NodeId,
        path: PropertyPath,
        reason: String,
    },

    #[error("parent {parent:?} cannot host a {child_kind} child")]
    InvalidParent {
        parent: NodeId,
        child_kind: String,
    },

    #[error("position {position} out of range for parent {parent:?} (len {len})")]
    InvalidPosition {
        parent: NodeId,
        position: usize,
        len: usize,
    },

    #[error("duplicate self_id {id:?} — IDML node IDs must be unique")]
    DuplicateNodeId { id: String },

    #[error("batch failed at index {failed_at}: {source}")]
    BatchFailed {
        failed_at: usize,
        source: Box<OperationError>,
    },
}
