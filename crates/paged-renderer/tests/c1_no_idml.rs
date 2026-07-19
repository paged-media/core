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

//! Migration slice **S5 — honour C1 in code** (`renderer.md` §C1: *"construct
//! a scene graph by hand in Rust, with no IDML involved, and render it"*).
//!
//! This exercises the **neutral composition pipeline** end to end with **zero
//! IDML**: a hand-built `paged_flow` region-chain + a non-IDML content engine
//! (coloured blocks) → `run_flow` fragments the content across the regions →
//! each fragment's `SceneLayer` lowers into a `DisplayList` at the region's
//! page position (`paged_compose::emit_scene_layer`) → `paged_gpu::rasterize`
//! composites it to pixels. No `Document`, no `paged_scene`, no `.idml`
//! anywhere — proving the composition model can render a native content part
//! that is not IDML (the C1 discipline the migration must keep passing).

use paged_compose::{
    emit_scene_layer, Color, DisplayList, SceneItem, SceneLayer, ScenePaint, ScenePathSeg,
    SceneTextItem, Transform,
};
use paged_flow::{
    run_flow, FlowContent, FlowId, Overset, Placement, Region, RegionChain, RegionGeometry,
};
use paged_gpu::{rasterize, RasterOptions};

/// A non-IDML content engine: a sequence of solid-colour blocks, one per
/// region. Each `place` fills the region's content box with the next block's
/// colour and advances — a genuine content flow across the region-chain, with
/// no text engine and no IDML.
struct BlockFlow {
    /// sRGB colours, one per block.
    colors: Vec<[f32; 4]>,
}

fn rect_path(w: f32, h: f32) -> Vec<ScenePathSeg> {
    vec![
        ScenePathSeg::MoveTo { x: 0.0, y: 0.0 },
        ScenePathSeg::LineTo { x: w, y: 0.0 },
        ScenePathSeg::LineTo { x: w, y: h },
        ScenePathSeg::LineTo { x: 0.0, y: h },
        ScenePathSeg::Close,
    ]
}

impl FlowContent for BlockFlow {
    type Fragment = SceneLayer;
    type Cursor = usize;

    fn start(&self) -> usize {
        0
    }

    fn place(&self, region: &Region, cursor: usize) -> Placement<SceneLayer, usize> {
        let mut items = Vec::new();
        if let Some(&[r, g, b, a]) = self.colors.get(cursor) {
            items.push(SceneItem::FillPath {
                path: rect_path(region.geometry.width_pt, region.geometry.height_pt),
                paint: ScenePaint { r, g, b, a },
            });
        }
        let next = if cursor + 1 < self.colors.len() {
            Some(cursor + 1)
        } else {
            None
        };
        Placement {
            fragment: SceneLayer { items },
            next,
        }
    }
}

/// A region's placement on the page (position + size). Positioning is a
/// separate concern from flow (composition-format §4), so the test supplies
/// page positions alongside the flow geometry.
struct PlacedRegion {
    id: &'static str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
}

#[test]
fn c1_render_a_composition_with_no_idml() {
    // Two regions, side by side, each hosting one coloured block.
    let placed = [
        PlacedRegion {
            id: "r0",
            x: 10.0,
            y: 10.0,
            w: 100.0,
            h: 60.0,
            color: [0.90, 0.10, 0.10, 1.0],
        },
        PlacedRegion {
            id: "r1",
            x: 200.0,
            y: 10.0,
            w: 120.0,
            h: 80.0,
            color: [0.10, 0.30, 0.80, 1.0],
        },
    ];

    // The composition's region-chain (sizes only) + the content engine.
    let chain = RegionChain::new(
        FlowId::new("blocks"),
        placed
            .iter()
            .map(|p| Region::new(p.id, RegionGeometry::new(p.w, p.h)))
            .collect(),
    );
    let content = BlockFlow {
        colors: placed.iter().map(|p| p.color).collect(),
    };

    // Fragment the content across the chain (S1 driver).
    let run = run_flow(&content, &chain);
    assert_eq!(run.overset, Overset::Fits);
    assert_eq!(run.placements.len(), 2);

    // Lower each region's SceneLayer into one shared DisplayList at its page
    // position — the compositor path, no IDML.
    let mut list = DisplayList::new();
    for (region_id, layer) in &run.placements {
        let p = placed.iter().find(|p| p.id == region_id.as_str()).unwrap();
        emit_scene_layer(
            &mut list,
            layer,
            Transform::translate(p.x, p.y),
            (p.w, p.h),
            // No text items in this composition; the closure is never called.
            |_: &mut DisplayList, _: &SceneTextItem, _: Transform| {},
        );
    }

    // Rasterise at 72 dpi (1 px per pt) on a white page.
    let opts = RasterOptions {
        page_width_pt: 400.0,
        page_height_pt: 200.0,
        dpi: 72.0,
        background: Color::WHITE,
    };
    let img = rasterize(&list, &opts);

    // Each region's centre pixel is its block colour; the gap between them is
    // the white background. (sRGB round-trips through the linear compositor to
    // within a couple of levels; sample centres, away from clip edges.)
    let near = |got: u8, want: f32| (got as f32 - want * 255.0).abs() <= 4.0;
    for p in &placed {
        let px = img
            .get_pixel((p.x + p.w / 2.0) as u32, (p.y + p.h / 2.0) as u32)
            .0;
        assert!(
            near(px[0], p.color[0]) && near(px[1], p.color[1]) && near(px[2], p.color[2]),
            "region {} centre = {:?}, expected ~{:?}",
            p.id,
            px,
            p.color
        );
    }
    // Background between the two regions stays white.
    let bg = img.get_pixel(150, 40).0;
    assert!(
        bg[0] > 250 && bg[1] > 250 && bg[2] > 250,
        "background should be white, got {bg:?}"
    );
}
