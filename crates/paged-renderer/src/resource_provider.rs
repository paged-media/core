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

//! Renderer image-resource provider (C-6 · I-06) — pyramid-tile pull.
//!
//! The native image lane is whole-image: one `DecodedImage` per placed
//! asset, decoded once at the resolution it happens to carry. A plugin
//! that owns a *tiled mip pyramid* (paged.image's image-graph) can serve
//! any `(level, x, y)` window of a large composition, but core had no seam
//! to ASK for tiles. This module is that seam: a *pull* contract the host
//! wires, keyed by a provider-claimed image id (the `x-paged-image:<frame>`
//! namespace).
//!
//! The renderer queries [`ImageResourceProvider::tile`] at the mip level
//! matching the current scale ([`mip_level_for_scale`]) and places each
//! returned tile as an ordinary [`DisplayCommand::Image`] — the SAME lane
//! placed assets use, so the tiles rasterise through tiny-skia (CPU) and
//! Vello (GPU) with no new path and no Vello fork. When the provider lacks
//! tiles at the chosen level the renderer records a
//! [`ResourceTilesNeeded`] signal and assembles whatever coarser level IS
//! cached (or nothing): compose NEVER blocks on a tile fetch. The host
//! fills the cache asynchronously (over the wire) and the next build
//! sharpens the frame.
//!
//! The zero-copy GPU-resident tile path (a `GPUTexture` variant of
//! [`ProviderTile`]) is Stage B (v45) — same trait, grown later.

use std::sync::Arc;

use paged_compose::{DecodedImage, DisplayCommand, DisplayList, Rect, Transform};

/// A host-wired source of pyramid tiles for one or more claimed images.
///
/// Implemented main-thread-side by the host (the SDK plumbs a plugin's
/// `source` callback into it); on the worker the canvas model implements
/// it over a budgeted LRU cache filled across the channel. Core treats
/// tiles as opaque RGBA8 rects — it never decodes, never owns the pyramid.
pub trait ImageResourceProvider {
    /// One tile of `image_id` at pyramid `level` (0 = full res; each level
    /// halves both dimensions). `x`/`y` is the tile's grid origin in
    /// level-space px — i.e. `dest` of the returned tile. `None` means the
    /// provider has no tile at that `(level, x, y)` *yet*; the renderer
    /// records the gap as [`ResourceTilesNeeded`] and falls back to a
    /// coarser cached level (or the whole-image lane) without blocking.
    fn tile(&self, image_id: &str, level: u8, x: u32, y: u32) -> Option<ProviderTile>;

    /// Monotonic revision of `image_id`. Core re-pulls when it changes —
    /// the damage signal, the same etag discipline the data provider uses.
    /// An unknown id returns `0`.
    fn revision(&self, image_id: &str) -> u64;
}

/// One opaque RGBA8 tile placed by `dest` in level-space px.
#[derive(Debug, Clone)]
pub struct ProviderTile {
    /// Tightly packed RGBA8, row-major. Length must be `width*height*4`.
    pub rgba: Arc<[u8]>,
    /// Pixel width of the buffer.
    pub width: u32,
    /// Pixel height of the buffer.
    pub height: u32,
    /// Tile origin in level-space px (top-left), `[x, y]`.
    pub dest: [u32; 2],
}

/// Static geometry of a claimed image's pyramid — the renderer needs the
/// base (level-0) pixel extent + tile size to (a) enumerate the tile grid
/// at a level and (b) map a tile's level-space px rect into the frame's
/// content box. Mirrors the claim the host sends over the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcePyramid {
    /// Level-0 (full-resolution) pixel width.
    pub base_width: u32,
    /// Level-0 (full-resolution) pixel height.
    pub base_height: u32,
    /// Number of pyramid levels (`>= 1`). Level `levels - 1` is the
    /// coarsest the provider can serve.
    pub levels: u8,
    /// Square tile edge in px (the grid cell size at every level).
    pub tile_size: u32,
}

impl ResourcePyramid {
    /// Pixel dimensions of `level` (`base >> level`, floored at 1).
    pub fn level_dims(&self, level: u8) -> (u32, u32) {
        let w = (self.base_width >> level).max(1);
        let h = (self.base_height >> level).max(1);
        (w, h)
    }

    /// Highest valid level index (`levels - 1`, clamped to `>= 0`).
    pub fn max_level(&self) -> u8 {
        self.levels.saturating_sub(1)
    }

    /// The tile-grid origins `[x, y]` (level-space px) that tile `level`,
    /// row-major, top-left first. A partial edge tile keeps the grid
    /// origin; its served `width`/`height` may be smaller than `tile_size`.
    pub fn tile_grid(&self, level: u8) -> Vec<[u32; 2]> {
        let ts = self.tile_size.max(1);
        let (lw, lh) = self.level_dims(level);
        let mut out = Vec::new();
        let mut y = 0;
        while y < lh {
            let mut x = 0;
            while x < lw {
                out.push([x, y]);
                x += ts;
            }
            y += ts;
        }
        out
    }
}

/// A worker→main request: a claimed image lacks tiles at `level`. The host
/// fetches the listed tile origins and replies with `SubmitResourceTiles`.
/// Emitted during compose; compose proceeds with the best cached level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceTilesNeeded {
    /// The provider-claimed image id (`x-paged-image:<frame>`).
    pub image_id: String,
    /// The mip level the renderer wanted (matching current scale).
    pub level: u8,
    /// Tile grid origins `[x, y]` (level-space px) that were missing.
    pub tiles: Vec<[u32; 2]>,
    /// The pyramid revision the request was computed against — the host
    /// echoes it back so a stale reply (revision moved on) can be dropped.
    pub generation: u64,
}

/// Pick the pyramid level for `scale` (rasteriser px-per-content-px).
///
/// `floor(log2(1/scale))` clamped to `[0, max_level]`: at `scale >= 1`
/// (the frame is shown at full res or zoomed in) we want level 0; halving
/// the scale steps one level coarser. A non-finite or non-positive scale
/// degrades to the coarsest level (the cheapest safe choice).
pub fn mip_level_for_scale(scale: f32, max_level: u8) -> u8 {
    if !scale.is_finite() || scale <= 0.0 {
        return max_level;
    }
    if scale >= 1.0 {
        return 0;
    }
    // 1/scale > 1, so log2 is >= 0; floor then clamp.
    let lvl = (1.0 / scale).log2().floor();
    let lvl = lvl.max(0.0).min(max_level as f32);
    lvl as u8
}

/// Assemble the cached tiles of one claimed image into `list` as ordinary
/// [`DisplayCommand::Image`] entries, clipped to the frame's content box.
///
/// `content_outer` maps frame-content coords (origin = content-box
/// top-left, pt) into page space — exactly the transform the C-1 scene
/// layer + native placed-image lanes use, so the assembled tiles
/// colour-manage and transform identically. `content_size` is the content
/// box `(w, h)` in pt; the WHOLE level-`level` image maps onto it, and
/// each tile occupies the proportional sub-rect.
///
/// Returns the tile grid origins the provider DIDN'T have at `level` (the
/// caller turns these into a [`ResourceTilesNeeded`]). When the provider
/// has nothing at `level`, the caller should retry at a coarser level;
/// this function only ever assembles tiles the provider actually returned,
/// so a fully-cold image emits no `Image` command (the whole-image
/// fallback lane stays responsible for first paint).
///
/// Coordinates: a tile at level-space px rect `[tx, ty, tw, th]` maps to
/// the content rect `[tx, ty, tw, th] * (content_dim / level_dim)`, then
/// `content_outer` carries it to page space via [`Transform::for_rect_in`].
pub fn assemble_resource_tiles(
    list: &mut DisplayList,
    provider: &dyn ImageResourceProvider,
    image_id: &str,
    pyramid: &ResourcePyramid,
    level: u8,
    content_outer: Transform,
    content_size: (f32, f32),
) -> Vec<[u32; 2]> {
    let (content_w, content_h) = content_size;
    let (level_w, level_h) = pyramid.level_dims(level);
    let mut missing = Vec::new();
    if content_w <= 0.0 || content_h <= 0.0 || level_w == 0 || level_h == 0 {
        return missing;
    }
    // pt-per-level-px on each axis: the whole level image fills the box.
    let sx = content_w / level_w as f32;
    let sy = content_h / level_h as f32;

    for [gx, gy] in pyramid.tile_grid(level) {
        let Some(tile) = provider.tile(image_id, level, gx, gy) else {
            missing.push([gx, gy]);
            continue;
        };
        // Defend against a malformed tile (length mismatch, zero area)
        // rather than panicking the whole render — the bytes are the
        // host's to get right; a bad one is simply skipped (and not
        // re-requested: the provider claims to have it).
        if tile.width == 0
            || tile.height == 0
            || tile.rgba.len() != (tile.width as usize) * (tile.height as usize) * 4
        {
            continue;
        }
        let [tx, ty] = tile.dest;
        let dest = Rect {
            x: tx as f32 * sx,
            y: ty as f32 * sy,
            w: tile.width as f32 * sx,
            h: tile.height as f32 * sy,
        };
        let image_id_in_pool = list.push_image(DecodedImage {
            width: tile.width,
            height: tile.height,
            encoded: bytes::Bytes::new(),
            rgba: bytes::Bytes::from(tile.rgba.to_vec()),
            icc: None,
        });
        list.push(DisplayCommand::Image {
            image_id: image_id_in_pool,
            transform: Transform::for_rect_in(dest, content_outer),
        });
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A test provider backed by an in-memory `(id, level, x, y) -> tile`
    /// map plus a per-id revision.
    #[derive(Default)]
    struct MapProvider {
        tiles: HashMap<(String, u8, u32, u32), ProviderTile>,
        revs: HashMap<String, u64>,
    }
    impl MapProvider {
        fn put(&mut self, id: &str, level: u8, x: u32, y: u32, w: u32, h: u32) {
            self.tiles.insert(
                (id.to_string(), level, x, y),
                ProviderTile {
                    rgba: vec![255u8; (w * h * 4) as usize].into(),
                    width: w,
                    height: h,
                    dest: [x, y],
                },
            );
        }
    }
    impl ImageResourceProvider for MapProvider {
        fn tile(&self, image_id: &str, level: u8, x: u32, y: u32) -> Option<ProviderTile> {
            self.tiles
                .get(&(image_id.to_string(), level, x, y))
                .cloned()
        }
        fn revision(&self, image_id: &str) -> u64 {
            self.revs.get(image_id).copied().unwrap_or(0)
        }
    }

    #[test]
    fn mip_pick_is_floor_log2_inv_scale_clamped() {
        // scale >= 1 → level 0 (full res / zoomed in).
        assert_eq!(mip_level_for_scale(1.0, 5), 0);
        assert_eq!(mip_level_for_scale(2.0, 5), 0);
        // scale 0.5 → 1/0.5 = 2 → log2 = 1 → level 1.
        assert_eq!(mip_level_for_scale(0.5, 5), 1);
        // scale 0.25 → 1/0.25 = 4 → log2 = 2 → level 2.
        assert_eq!(mip_level_for_scale(0.25, 5), 2);
        // 0.3 → 1/0.3 ≈ 3.33 → log2 ≈ 1.74 → floor 1.
        assert_eq!(mip_level_for_scale(0.3, 5), 1);
        // Clamp to max_level.
        assert_eq!(mip_level_for_scale(0.001, 3), 3);
        // Degenerate scales → coarsest.
        assert_eq!(mip_level_for_scale(0.0, 4), 4);
        assert_eq!(mip_level_for_scale(-1.0, 4), 4);
        assert_eq!(mip_level_for_scale(f32::NAN, 4), 4);
    }

    #[test]
    fn level_dims_halve_and_floor_at_one() {
        let p = ResourcePyramid {
            base_width: 1024,
            base_height: 768,
            levels: 11,
            tile_size: 256,
        };
        assert_eq!(p.level_dims(0), (1024, 768));
        assert_eq!(p.level_dims(1), (512, 384));
        assert_eq!(p.level_dims(2), (256, 192));
        // Far down the pyramid both dims floor at 1.
        assert_eq!(p.level_dims(10), (1, 1));
        assert_eq!(p.max_level(), 10);
    }

    #[test]
    fn tile_grid_covers_the_level_in_row_major_order() {
        let p = ResourcePyramid {
            base_width: 512,
            base_height: 300,
            levels: 4,
            tile_size: 256,
        };
        // Level 0 is 512×300 → 2 cols × 2 rows (300 needs a partial row).
        let grid = p.tile_grid(0);
        assert_eq!(grid, vec![[0, 0], [256, 0], [0, 256], [256, 256]]);
        // Level 1 is 256×150 → exactly one tile.
        assert_eq!(p.tile_grid(1), vec![[0, 0]]);
    }

    #[test]
    fn assemble_emits_an_image_per_cached_tile_and_reports_the_missing() {
        let p = ResourcePyramid {
            base_width: 512,
            base_height: 512,
            levels: 2,
            tile_size: 256,
        };
        // Level 0 grid is [0,0],[256,0],[0,256],[256,256]. Cache three.
        let mut prov = MapProvider::default();
        prov.put("img", 0, 0, 0, 256, 256);
        prov.put("img", 0, 256, 0, 256, 256);
        prov.put("img", 0, 0, 256, 256, 256);
        // [256,256] intentionally absent → reported missing.
        let mut list = DisplayList::new();
        let missing = assemble_resource_tiles(
            &mut list,
            &prov,
            "img",
            &p,
            0,
            Transform::IDENTITY,
            (512.0, 512.0),
        );
        // Three Image commands, three pooled images.
        let images = list
            .commands
            .iter()
            .filter(|c| matches!(c, DisplayCommand::Image { .. }))
            .count();
        assert_eq!(images, 3);
        assert_eq!(list.images.len(), 3);
        assert_eq!(missing, vec![[256, 256]]);
    }

    #[test]
    fn assemble_maps_a_tile_into_the_content_box_proportionally() {
        // One 256×256 tile at grid origin [256,0] in a 512×512 level placed
        // into a 100×100 pt content box at page origin (10,20). The whole
        // level maps onto the box, so the tile occupies the box's right
        // half top quarter: content rect [50,0,50,50] → page [60,20,...].
        let p = ResourcePyramid {
            base_width: 512,
            base_height: 512,
            levels: 1,
            tile_size: 256,
        };
        let mut prov = MapProvider::default();
        prov.put("img", 0, 256, 0, 256, 256);
        // Only request the one tile by claiming a single-tile grid: easier
        // to assert geometry on one command. Use a pyramid whose level-0
        // is exactly one tile wide is not possible here (512/256=2), so
        // assemble all and find the emitted command.
        let mut list = DisplayList::new();
        let _ = assemble_resource_tiles(
            &mut list,
            &prov,
            "img",
            &p,
            0,
            Transform::translate(10.0, 20.0),
            (100.0, 100.0),
        );
        let cmd = list
            .commands
            .iter()
            .find_map(|c| match c {
                DisplayCommand::Image { transform, .. } => Some(*transform),
                _ => None,
            })
            .expect("one image command");
        // Tile unit-square corners map: (0,0) → content (50,0) → page
        // (60,20); (1,1) → content (100,50) → page (110,70).
        let (tlx, tly) = cmd.apply(0.0, 0.0);
        let (brx, bry) = cmd.apply(1.0, 1.0);
        assert!(
            (tlx - 60.0).abs() < 1e-3 && (tly - 20.0).abs() < 1e-3,
            "tl=({tlx},{tly})"
        );
        assert!(
            (brx - 110.0).abs() < 1e-3 && (bry - 70.0).abs() < 1e-3,
            "br=({brx},{bry})"
        );
    }

    #[test]
    fn malformed_tile_is_skipped_not_panicked() {
        let p = ResourcePyramid {
            base_width: 256,
            base_height: 256,
            levels: 1,
            tile_size: 256,
        };
        let mut prov = MapProvider::default();
        // Claim a tile but give it a length-mismatched buffer.
        prov.tiles.insert(
            ("img".to_string(), 0, 0, 0),
            ProviderTile {
                rgba: vec![1, 2, 3].into(), // not 256*256*4
                width: 256,
                height: 256,
                dest: [0, 0],
            },
        );
        let mut list = DisplayList::new();
        let missing = assemble_resource_tiles(
            &mut list,
            &prov,
            "img",
            &p,
            0,
            Transform::IDENTITY,
            (10.0, 10.0),
        );
        // Tile was "present" (so not missing) but malformed → no Image.
        assert!(missing.is_empty());
        assert!(!list
            .commands
            .iter()
            .any(|c| matches!(c, DisplayCommand::Image { .. })));
        assert!(list.images.is_empty());
    }
}
