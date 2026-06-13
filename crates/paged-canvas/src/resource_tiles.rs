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

//! Worker-side image-resource tile store (C-6 / I-06).
//!
//! Backs `PipelineOptions::resource_providers`: the worker registers a
//! claim per frame (`ClaimImageResource`), fills a budgeted LRU tile cache
//! as the host submits tiles (`SubmitResourceTiles`), and drops both on
//! release (`ReleaseImageResource`). Keyed by the frame `Self` id (parsed
//! out of the claim's `x-paged-image:<frame>` namespace) so the renderer's
//! per-frame splice can find it.
//!
//! Implements [`paged_renderer::ImageResourceProvider`] so a single store
//! serves every claimed image during one build. `tile()` is `&self` (the
//! build only reads), so LRU recency uses interior mutability — a read
//! during compose bumps the tile's recency so the working set survives
//! eviction.

use std::cell::Cell;
use std::collections::HashMap;

use paged_renderer::{ImageResourceProvider, ProviderTile, ResourcePyramid};

/// Default tile-cache byte budget: 128 MiB. Large enough to hold the
/// working set of a 50 MP composition's visible mip level at 256 px tiles
/// (a 50 MP image at one level is ~16 K tiles worst case, but only the
/// on-screen window is ever pulled — a few hundred tiles, well under the
/// budget) while bounding the worker's footprint. Tuned against real M2
/// telemetry is a follow-up (design §13 / out-of-scope).
pub const DEFAULT_TILE_BUDGET_BYTES: usize = 128 * 1024 * 1024;

/// One frame's image-resource claim: pyramid geometry + the claimed
/// `image_id` (`x-paged-image:<frame>`) + the damage revision.
#[derive(Debug, Clone)]
struct Claim {
    image_id: String,
    pyramid: ResourcePyramid,
    revision: u64,
}

/// A cached tile plus its LRU recency stamp.
struct CachedTile {
    rgba: std::sync::Arc<[u8]>,
    width: u32,
    height: u32,
    /// Last-access stamp (monotone clock). Bumped on submit AND on read.
    last_used: Cell<u64>,
    /// Cached byte cost (`rgba.len()`), so eviction sums without
    /// re-measuring.
    bytes: usize,
}

/// Cache key: frame id + pyramid coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TileKey {
    frame_id: String,
    level: u8,
    x: u32,
    y: u32,
}

/// Budgeted LRU tile cache + claim registry. One per [`crate::model::
/// CanvasModel`].
pub struct ResourceTileStore {
    /// frame id → claim.
    claims: HashMap<String, Claim>,
    /// (frame, level, x, y) → tile.
    tiles: HashMap<TileKey, CachedTile>,
    /// Sum of `tiles[*].bytes`. Maintained incrementally so eviction is
    /// O(evicted), not O(cache).
    used_bytes: usize,
    /// Byte budget; insertions past it evict the least-recently-used.
    budget_bytes: usize,
    /// Monotone access clock (interior mutability so `&self` reads tick).
    clock: Cell<u64>,
}

impl Default for ResourceTileStore {
    fn default() -> Self {
        Self::new(DEFAULT_TILE_BUDGET_BYTES)
    }
}

impl ResourceTileStore {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            claims: HashMap::new(),
            tiles: HashMap::new(),
            used_bytes: 0,
            budget_bytes,
            clock: Cell::new(0),
        }
    }

    fn tick(&self) -> u64 {
        let t = self.clock.get().wrapping_add(1);
        self.clock.set(t);
        t
    }

    /// Register (or replace) the claim for `frame_id`. A replace drops the
    /// old claim's cached tiles (the pyramid may have changed shape).
    pub fn claim(
        &mut self,
        frame_id: String,
        image_id: String,
        pyramid: ResourcePyramid,
        revision: u64,
    ) {
        if self.claims.contains_key(&frame_id) {
            self.drop_tiles_for(&frame_id);
        }
        self.claims.insert(
            frame_id,
            Claim {
                image_id,
                pyramid,
                revision,
            },
        );
    }

    /// Drop the claim for `frame_id` and its cached tiles (no-op if none).
    /// Returns `true` when something was removed.
    pub fn release(&mut self, frame_id: &str) -> bool {
        let had = self.claims.remove(frame_id).is_some();
        if had {
            self.drop_tiles_for(frame_id);
        }
        had
    }

    /// Resolve the frame id a claimed `image_id` belongs to. The claim is
    /// stored by frame id; submits/needs name the `image_id`. Linear scan
    /// over claims (a handful at most).
    pub fn frame_for_image(&self, image_id: &str) -> Option<&str> {
        self.claims
            .iter()
            .find(|(_, c)| c.image_id == image_id)
            .map(|(f, _)| f.as_str())
    }

    /// Fill the cache with submitted tiles for `image_id` at `level`.
    /// `generation` must match the claim's current revision, else the
    /// submit is a stale reply and is dropped (returns `false`). A
    /// malformed tile (length ≠ `w*h*4`, zero area) is skipped. Returns
    /// `true` when at least the claim matched (tiles inserted, page should
    /// be dirtied).
    pub fn submit(
        &mut self,
        image_id: &str,
        level: u8,
        tiles: Vec<crate::channel::ProviderTileWire>,
        generation: u64,
    ) -> bool {
        let Some(frame_id) = self.frame_for_image(image_id).map(str::to_string) else {
            return false;
        };
        // Stale-reply guard: the pyramid moved on since the request.
        let claim_rev = self.claims.get(&frame_id).map(|c| c.revision).unwrap_or(0);
        if generation != claim_rev {
            return false;
        }
        for t in tiles {
            if t.width == 0
                || t.height == 0
                || t.rgba.as_slice().len() != (t.width as usize) * (t.height as usize) * 4
            {
                continue;
            }
            let bytes = t.rgba.as_slice().len();
            let key = TileKey {
                frame_id: frame_id.clone(),
                level,
                x: t.x,
                y: t.y,
            };
            let stamp = self.tick();
            if let Some(old) = self.tiles.insert(
                key,
                CachedTile {
                    rgba: std::sync::Arc::from(t.rgba.into_vec()),
                    width: t.width,
                    height: t.height,
                    last_used: Cell::new(stamp),
                    bytes,
                },
            ) {
                self.used_bytes -= old.bytes;
            }
            self.used_bytes += bytes;
        }
        self.evict_to_budget();
        true
    }

    /// True when `frame_id` carries an active claim.
    pub fn is_claimed(&self, frame_id: &str) -> bool {
        self.claims.contains_key(frame_id)
    }

    /// Current cache footprint in bytes (test/introspection aid).
    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    /// Number of cached tiles (test/introspection aid).
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Build the renderer-facing provider entry map keyed by frame id,
    /// borrowing `self` as the shared provider. One entry per active
    /// claim; all entries point at the same `&self` provider.
    pub fn provider_entries(
        &self,
    ) -> HashMap<String, paged_renderer::pipeline::ResourceProviderEntry<'_>> {
        self.claims
            .iter()
            .map(|(frame_id, c)| {
                (
                    frame_id.clone(),
                    paged_renderer::pipeline::ResourceProviderEntry {
                        image_id: c.image_id.as_str(),
                        pyramid: c.pyramid,
                        provider: self,
                    },
                )
            })
            .collect()
    }

    fn drop_tiles_for(&mut self, frame_id: &str) {
        let mut freed = 0;
        self.tiles.retain(|k, v| {
            if k.frame_id == frame_id {
                freed += v.bytes;
                false
            } else {
                true
            }
        });
        self.used_bytes -= freed;
    }

    /// Evict least-recently-used tiles until under budget. Cheap when the
    /// working set fits (the common case): the loop body runs zero times.
    fn evict_to_budget(&mut self) {
        while self.used_bytes > self.budget_bytes && !self.tiles.is_empty() {
            // Find the LRU key (min last_used). O(n) per eviction; n is the
            // tile count and evictions are rare (only past budget).
            let victim = self
                .tiles
                .iter()
                .min_by_key(|(_, v)| v.last_used.get())
                .map(|(k, _)| k.clone());
            let Some(victim) = victim else { break };
            if let Some(v) = self.tiles.remove(&victim) {
                self.used_bytes -= v.bytes;
            }
        }
    }
}

impl ImageResourceProvider for ResourceTileStore {
    fn tile(&self, image_id: &str, level: u8, x: u32, y: u32) -> Option<ProviderTile> {
        let frame_id = self.frame_for_image(image_id)?;
        let key = TileKey {
            frame_id: frame_id.to_string(),
            level,
            x,
            y,
        };
        let cached = self.tiles.get(&key)?;
        // Bump recency on read so the visible working set survives
        // eviction (true LRU, not just insert-order).
        cached.last_used.set(self.tick());
        Some(ProviderTile {
            rgba: cached.rgba.clone(),
            width: cached.width,
            height: cached.height,
            dest: [x, y],
        })
    }

    fn revision(&self, image_id: &str) -> u64 {
        self.frame_for_image(image_id)
            .and_then(|f| self.claims.get(f))
            .map(|c| c.revision)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ProviderTileWire;

    fn wire(x: u32, y: u32, w: u32, h: u32) -> ProviderTileWire {
        ProviderTileWire {
            x,
            y,
            width: w,
            height: h,
            rgba: vec![200u8; (w * h * 4) as usize].into(),
        }
    }

    fn pyramid() -> ResourcePyramid {
        ResourcePyramid {
            base_width: 512,
            base_height: 512,
            levels: 3,
            tile_size: 256,
        }
    }

    #[test]
    fn claim_submit_then_provider_serves_the_tile() {
        let mut store = ResourceTileStore::default();
        store.claim(
            "frame-1".into(),
            "x-paged-image:frame-1".into(),
            pyramid(),
            7,
        );
        assert!(store.is_claimed("frame-1"));
        assert!(store.submit("x-paged-image:frame-1", 0, vec![wire(0, 0, 256, 256)], 7));
        // Provider serves it back by image_id.
        let t = store
            .tile("x-paged-image:frame-1", 0, 0, 0)
            .expect("tile served");
        assert_eq!((t.width, t.height), (256, 256));
        assert_eq!(t.dest, [0, 0]);
        assert_eq!(store.revision("x-paged-image:frame-1"), 7);
    }

    #[test]
    fn stale_generation_submit_is_dropped() {
        let mut store = ResourceTileStore::default();
        store.claim("f".into(), "img".into(), pyramid(), 5);
        // generation 4 != claim revision 5 → dropped.
        assert!(!store.submit("img", 0, vec![wire(0, 0, 256, 256)], 4));
        assert_eq!(store.tile_count(), 0);
    }

    #[test]
    fn release_drops_claim_and_tiles() {
        let mut store = ResourceTileStore::default();
        store.claim("f".into(), "img".into(), pyramid(), 0);
        store.submit("img", 0, vec![wire(0, 0, 256, 256)], 0);
        assert_eq!(store.tile_count(), 1);
        assert!(store.release("f"));
        assert!(!store.is_claimed("f"));
        assert_eq!(store.tile_count(), 0);
        assert_eq!(store.used_bytes(), 0);
        // The provider no longer serves it.
        assert!(store.tile("img", 0, 0, 0).is_none());
    }

    #[test]
    fn lru_evicts_least_recently_used_when_over_budget() {
        // Budget = exactly two 1×1 tiles (4 bytes each → 8 bytes).
        let mut store = ResourceTileStore::new(8);
        store.claim("f".into(), "img".into(), pyramid(), 0);
        store.submit("img", 0, vec![wire(0, 0, 1, 1)], 0); // A
        store.submit("img", 0, vec![wire(1, 0, 1, 1)], 0); // B
        assert_eq!(store.tile_count(), 2);
        // Touch A so B becomes the LRU.
        assert!(store.tile("img", 0, 0, 0).is_some());
        // Insert C → over budget → evict B (the least-recently-used).
        store.submit("img", 0, vec![wire(2, 0, 1, 1)], 0); // C
        assert_eq!(store.tile_count(), 2);
        assert!(store.tile("img", 0, 0, 0).is_some(), "A retained (touched)");
        assert!(store.tile("img", 0, 2, 0).is_some(), "C retained (newest)");
        assert!(store.tile("img", 0, 1, 0).is_none(), "B evicted (LRU)");
        assert!(store.used_bytes() <= 8);
    }

    #[test]
    fn provider_entries_one_per_claim() {
        let mut store = ResourceTileStore::default();
        store.claim("f1".into(), "img1".into(), pyramid(), 0);
        store.claim("f2".into(), "img2".into(), pyramid(), 0);
        let entries = store.provider_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries.get("f1").unwrap().image_id, "img1");
        assert_eq!(entries.get("f2").unwrap().image_id, "img2");
    }

    #[test]
    fn malformed_tile_is_skipped_on_submit() {
        let mut store = ResourceTileStore::default();
        store.claim("f".into(), "img".into(), pyramid(), 0);
        let bad = ProviderTileWire {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            rgba: vec![1, 2, 3].into(), // not 2*2*4
        };
        assert!(store.submit("img", 0, vec![bad], 0)); // claim matched
        assert_eq!(store.tile_count(), 0); // but the tile was skipped
    }
}
