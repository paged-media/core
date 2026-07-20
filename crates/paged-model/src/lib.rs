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

//! The Paged document **model** — the Paged-owned data types and their pure
//! value logic (geometry, IDML token maps), with **no XML/ZIP parsing**.
//!
//! This is the foundation of the fork: the model is Paged's, not IDML's. The
//! IDML parser (`paged-parse` today, destined for the import/export adapter)
//! *depends on* this crate and imports into it; the render/mutate stack speaks
//! these types. Serde-serializable (it backs the native `.paged` codec).
//!
//! N5: the model is being lifted out of the parser crate incrementally — the
//! split axis is "touches quick-xml/zip" (stays in the parser) vs "pure value
//! logic" (moves here). This is the first slice: the foundational geometry
//! primitive. `paged-parse` re-exports everything moved here, so its dependents
//! compile unchanged.

use serde::{Deserialize, Serialize};

/// An axis-aligned bounding box in points: `top`, `left`, `bottom`, `right`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Bounds {
    pub top: f32,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
}

impl Bounds {
    pub const ZERO: Bounds = Bounds {
        top: 0.0,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    };
    pub fn width(&self) -> f32 {
        self.right - self.left
    }
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
}
