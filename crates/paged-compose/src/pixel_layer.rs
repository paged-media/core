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

//! Plugin PIXEL-layer IR (C-1 Stage B — the per-drag GPU preview door).
//!
//! PROTOCOL STATUS — LANDED IN CORE at `PROTOCOL_VERSION` v50. This type
//! is the wire shape for the canvas messages
//! `MainToWorkerKind::SubmitPixelLayer` / `ClearPixelLayer` (the streaming
//! sibling of `SubmitSceneLayer`); the worker routes it through
//! `CanvasModel::set_pixel_layer`, which lowers it via
//! [`PixelLayer::into_scene_layer`] into the SAME per-frame scene-layer
//! registry. The save-back side is `Mutation::ReplaceImageBytes` →
//! `Operation::ReplaceImageBytes`. The canvas-wasm publish + editor pin
//! sync are still downstream (consumer side, not this repo).
//!
//! Where [`crate::SceneLayer`]'s `SceneItem::Image` (Stage A, v41) carries
//! ONE whole-frame RGBA8 buffer re-sent on every adjust commit, a
//! [`PixelLayer`] carries a SET OF TILES, each independently positioned —
//! the granularity an interactive *drag* needs: a slider drag re-streams
//! only the dirtied tiles onto the frame, not the whole image, every
//! frame. The lowering to the display list is the same
//! [`crate::SceneItem::Image`] CPU/GPU lane (no new rasterizer path); the
//! distinction is purely the wire/streaming shape, so this is forward-
//! compatible with the eventual zero-copy shared-`GPUTexture` Stage B
//! (the tiles become texture-pool handles; the IR keeps the same dest
//! rects).

use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

use crate::scene_layer::{SceneItem, SceneLayer};

/// A plugin-submitted pixel layer: a set of independently-positioned RGBA8
/// tiles to composite INSIDE a frame during an interactive gesture. All
/// coordinates are frame-content points (the same space as
/// [`SceneLayer`]); core applies the frame transform + content-box clip at
/// compose time. Ephemeral — re-streamed per drag, never a document
/// mutation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct PixelLayer {
    pub tiles: Vec<PixelTile>,
}

/// One RGBA8 tile of a [`PixelLayer`]. `rgba` is tightly packed
/// (`width*height*4`, row-major); the tile is placed at the `(x, y)`
/// frame-content point with display size `(w, h)` points. A malformed tile
/// (length ≠ `width*height*4`, or a zero-area buffer/dest) lowers to
/// nothing, never a panic — identical to the `SceneItem::Image` contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Tsify)]
#[serde(rename_all = "camelCase")]
pub struct PixelTile {
    #[tsify(type = "number[]")]
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl PixelTile {
    /// Is this tile renderable (buffer length matches the declared extent
    /// and both the source and dest have positive area)? A non-renderable
    /// tile is dropped by [`PixelLayer::into_scene_layer`] rather than
    /// emitting a torn image.
    fn is_renderable(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.w > 0.0
            && self.h > 0.0
            && self.rgba.len() == (self.width as usize) * (self.height as usize) * 4
    }
}

impl PixelLayer {
    /// Lower the pixel layer to an ordinary [`SceneLayer`] of
    /// `SceneItem::Image` items, so it composites through the EXISTING
    /// Stage-A image lane (CPU/GPU) with no new rasterizer path. Malformed
    /// tiles are dropped. This is the whole integration: a pixel layer IS a
    /// scene layer of image tiles, distinguished only by its streaming
    /// wire shape.
    pub fn into_scene_layer(self) -> SceneLayer {
        let items = self
            .tiles
            .into_iter()
            .filter(PixelTile::is_renderable)
            .map(|t| SceneItem::Image {
                rgba: t.rgba,
                width: t.width,
                height: t.height,
                x: t.x,
                y: t.y,
                w: t.w,
                h: t.h,
            })
            .collect();
        SceneLayer { items }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, fill: u8) -> PixelTile {
        PixelTile {
            rgba: vec![fill; (width * height * 4) as usize],
            width,
            height,
            x,
            y,
            w,
            h,
        }
    }

    #[test]
    fn pixel_layer_lowers_each_renderable_tile_to_an_image_item() {
        let layer = PixelLayer {
            tiles: vec![
                tile(2, 2, 0.0, 0.0, 10.0, 10.0, 200),
                tile(2, 2, 10.0, 0.0, 10.0, 10.0, 100),
            ],
        };
        let scene = layer.into_scene_layer();
        assert_eq!(scene.items.len(), 2, "both tiles lower to image items");
        for item in &scene.items {
            assert!(matches!(item, SceneItem::Image { .. }));
        }
    }

    #[test]
    fn pixel_layer_drops_malformed_tiles() {
        let mut bad_len = tile(2, 2, 0.0, 0.0, 10.0, 10.0, 1);
        bad_len.rgba.truncate(3); // length no longer matches width*height*4
        let zero_src = tile(0, 0, 0.0, 0.0, 10.0, 10.0, 1);
        let zero_dest = tile(2, 2, 0.0, 0.0, 0.0, 10.0, 1);
        let good = tile(1, 1, 5.0, 5.0, 4.0, 4.0, 255);
        let layer = PixelLayer {
            tiles: vec![bad_len, zero_src, zero_dest, good],
        };
        let scene = layer.into_scene_layer();
        assert_eq!(scene.items.len(), 1, "only the renderable tile survives");
    }

    #[test]
    fn pixel_layer_serde_roundtrips_through_json() {
        let layer = PixelLayer {
            tiles: vec![tile(1, 1, 1.0, 2.0, 3.0, 4.0, 42)],
        };
        let json = serde_json::to_string(&layer).expect("serialize");
        // camelCase fields cross the wire.
        assert!(json.contains("\"tiles\""));
        let back: PixelLayer = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, layer);
    }
}
