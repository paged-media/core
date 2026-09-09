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

//! Element-level selection model (application state).
//!
//! Distinct from `ContentSelection` (text caret / range): this is the
//! set of *page items* — text frames, rectangles, ovals, polygons,
//! graphic lines, and groups — the user has selected. Selection lives
//! in application state, never enters the Operation log, and Cmd-Z
//! never changes it.
//!
//! `ElementId`'s variants mirror `paged-mutate::NodeId` so a future
//! `From<ElementId> for NodeId` can bridge them when Phase B lands the
//! gesture → Operation pipeline. The duplication is deliberate: Phase A
//! avoids the cross-crate dep so element selection can ship before the
//! mutate-log bridge.

//! Re-export: `ElementId` and the selection shapes moved to
//! `paged-wire`, the leaf crate that carries the wire vocabulary. They
//! never needed the canvas model, and the capability catalog needs to
//! see them without it.

pub use paged_wire::{ElementId, ElementSelection, SelectionMode};
