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

//! Work counters: how much expensive work the engine did, as numbers a
//! test can assert.
//!
//! Wall-clock time is noisy on a shared CI runner; the AMOUNT of work is
//! not. Each counter names one piece of work that has, at some point, been
//! done far more often than it needed to be:
//!
//! * `font_bytes_hashed` — the per-paragraph whole-font hash that cost
//!   ~17 s per rebuild on the annual (now memoised; this proves it stays so);
//! * `pipeline_image_decodes` — images decoded by the build;
//! * `scene_image_decodes` — images materialised into a GPU page scene
//!   (decoded from their bytes on wasm, copied on native): the repaint
//!   after each write used to redo this for every visible photo;
//! * `page_scenes_built` — GPU page scenes encoded;
//! * `grow_passes` / `story_emits` — layout passes, the multipliers on
//!   everything above.
//!
//! The engine is single-threaded, so the counters are per THREAD: tests
//! running in parallel each see only their own work. Counting is one
//! thread-local add — cheap enough to leave on in every build.

use std::cell::Cell;

/// A snapshot of the counters (see the module docs for each field).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PerfCounters {
    pub font_bytes_hashed: u64,
    pub pipeline_image_decodes: u64,
    pub scene_image_decodes: u64,
    pub page_scenes_built: u64,
    pub grow_passes: u64,
    pub story_emits: u64,
}

impl PerfCounters {
    /// The work done between two snapshots.
    pub fn since(&self, earlier: &PerfCounters) -> PerfCounters {
        PerfCounters {
            font_bytes_hashed: self.font_bytes_hashed - earlier.font_bytes_hashed,
            pipeline_image_decodes: self.pipeline_image_decodes - earlier.pipeline_image_decodes,
            scene_image_decodes: self.scene_image_decodes - earlier.scene_image_decodes,
            page_scenes_built: self.page_scenes_built - earlier.page_scenes_built,
            grow_passes: self.grow_passes - earlier.grow_passes,
            story_emits: self.story_emits - earlier.story_emits,
        }
    }
}

thread_local! {
    static COUNTERS: Cell<PerfCounters> = const { Cell::new(PerfCounters {
        font_bytes_hashed: 0,
        pipeline_image_decodes: 0,
        scene_image_decodes: 0,
        page_scenes_built: 0,
        grow_passes: 0,
        story_emits: 0,
    }) };
}

/// The current thread's counters.
pub fn snapshot() -> PerfCounters {
    COUNTERS.with(Cell::get)
}

/// Add to one counter.
pub fn count(f: impl FnOnce(&mut PerfCounters)) {
    COUNTERS.with(|c| {
        let mut v = c.get();
        f(&mut v);
        c.set(v);
    });
}
