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

//! Driving the compositor from a [`paged_composition::Composition`]
//! (`document.pgd`, S6 slice 2). Walks the model's regions, asks a
//! [`RegionRenderer`] for each region's `SceneLayer`, lowers it at the
//! region's page position (`paged_compose::emit_scene_layer`), and rasterizes
//! (`paged_gpu::rasterize`).
//!
//! This is the reusable form of the S5 C1 test: **render a `document.pgd`
//! composition to pixels with no IDML.** The content itself comes from a
//! `RegionRenderer` (a content engine, or a test stub) — the composition owns
//! only the arrangement.
//!
//! It lives one layer up from `paged-composition` (which stays pure, so
//! `paged-scene` can depend on it) because the render stack
//! (`paged-compose`/`paged-gpu`) itself depends on `paged-scene`.
//!
//! Slice-2 scope: single page, `PageRelative` positions;
//! `FrameRelative`/`Anchor`/`GridCell` resolution, multi-page, and cross-region
//! flow fragmentation are later slices.

use paged_compose::{Color, DisplayList, SceneLayer, SceneTextItem, Transform};
use paged_composition::{Composition, Position, Region};
use paged_flow::RegionGeometry;
use paged_gpu::{rasterize, RasterOptions};

/// Produces the `SceneLayer` a region's bound content paints. The composition
/// is content-agnostic; this is where a content engine (or a test) turns a
/// `Region`'s `bind` + `geometry` into pixels-to-be. Returning `None` skips the
/// region (e.g. an unbound decoration the renderer doesn't handle).
pub trait RegionRenderer {
    fn render_region(&self, region: &Region) -> Option<SceneLayer>;
}

/// Render one page of a composition to an RGBA image, driving `renderer` for
/// each region positioned on that page. Returns `None` if `page_id` is not in
/// the composition. Regions whose position is not `PageRelative` on this page
/// are skipped (later slices resolve the other constraint kinds).
pub fn render_page<R: RegionRenderer>(
    comp: &Composition,
    page_id: &str,
    renderer: &R,
    dpi: f32,
    background: Color,
) -> Option<image::RgbaImage> {
    let page = comp.pages.iter().find(|p| p.id == page_id)?;

    let mut list = DisplayList::new();
    for region in comp.regions() {
        if let Position::PageRelative { page: pg, at } = &region.position {
            if pg == page_id {
                if let Some(layer) = renderer.render_region(region) {
                    emit_region(&mut list, &layer, at, region.geometry);
                }
            }
        }
    }

    let opts = RasterOptions {
        page_width_pt: page.size[0],
        page_height_pt: page.size[1],
        dpi,
        background,
    };
    Some(rasterize(&list, &opts))
}

fn emit_region(list: &mut DisplayList, layer: &SceneLayer, at: &[f32; 2], geom: RegionGeometry) {
    paged_compose::emit_scene_layer(
        list,
        layer,
        Transform::translate(at[0], at[1]),
        (geom.width_pt, geom.height_pt),
        // The composition carries no fonts; a content engine that emits text
        // supplies its own outliner. Slice-2 renderers paint vector/fill only.
        |_: &mut DisplayList, _: &SceneTextItem, _: Transform| {},
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_compose::{SceneItem, ScenePaint, ScenePathSeg};
    use paged_composition::{Bind, Node, Page, PartRef, Surface, SurfaceKind};
    use paged_flow::RegionId;

    /// A test content engine: paints each region's bound `selector` as a solid
    /// colour block (the colour encoded in the selector `"color:RRGGBB"`).
    struct BlockRenderer;

    fn rect(w: f32, h: f32) -> Vec<ScenePathSeg> {
        vec![
            ScenePathSeg::MoveTo { x: 0.0, y: 0.0 },
            ScenePathSeg::LineTo { x: w, y: 0.0 },
            ScenePathSeg::LineTo { x: w, y: h },
            ScenePathSeg::LineTo { x: 0.0, y: h },
            ScenePathSeg::Close,
        ]
    }

    impl RegionRenderer for BlockRenderer {
        fn render_region(&self, region: &Region) -> Option<SceneLayer> {
            let Bind::Part { selector, .. } = &region.bind else {
                return None;
            };
            let hex = selector.strip_prefix("color:")?;
            let n = u32::from_str_radix(hex, 16).ok()?;
            let paint = ScenePaint {
                r: ((n >> 16) & 0xff) as f32 / 255.0,
                g: ((n >> 8) & 0xff) as f32 / 255.0,
                b: (n & 0xff) as f32 / 255.0,
                a: 1.0,
            };
            Some(SceneLayer {
                items: vec![SceneItem::FillPath {
                    path: rect(region.geometry.width_pt, region.geometry.height_pt),
                    paint,
                }],
            })
        }
    }

    fn block(id: &str, hex: &str, at: [f32; 2], w: f32, h: f32) -> Node {
        Node::Region(Region {
            id: RegionId::new(id),
            bind: Bind::Part {
                part: PartRef::new("blocks"),
                selector: format!("color:{hex}"),
            },
            position: Position::PageRelative {
                page: "p1".to_string(),
                at,
            },
            geometry: RegionGeometry::new(w, h),
            layer: None,
            flow: None,
            visible_on: Vec::new(),
        })
    }

    #[test]
    fn render_page_composites_regions_with_no_idml() {
        let mut comp = Composition::new(1);
        comp.surfaces = vec![Surface {
            id: "print".to_string(),
            kind: SurfaceKind::Print,
        }];
        comp.pages = vec![Page {
            id: "p1".to_string(),
            size: [400.0, 200.0],
            spread: None,
        }];
        comp.nodes = vec![
            block("r0", "e51a1a", [10.0, 10.0], 100.0, 60.0), // red
            block("r1", "1a4de5", [200.0, 10.0], 120.0, 80.0), // blue
        ];

        let img =
            render_page(&comp, "p1", &BlockRenderer, 72.0, Color::WHITE).expect("page p1 renders");

        // Region centres carry their block colour; the gap is the white page.
        let near = |got: u8, want: u8| (got as i32 - want as i32).abs() <= 4;
        let c0 = img.get_pixel(60, 40).0; // r0 centre (10+50, 10+30)
        assert!(
            near(c0[0], 0xe5) && near(c0[1], 0x1a) && near(c0[2], 0x1a),
            "r0 = {c0:?}"
        );
        let c1 = img.get_pixel(260, 50).0; // r1 centre (200+60, 10+40)
        assert!(
            near(c1[0], 0x1a) && near(c1[1], 0x4d) && near(c1[2], 0xe5),
            "r1 = {c1:?}"
        );
        let bg = img.get_pixel(150, 40).0;
        assert!(bg[0] > 250 && bg[1] > 250 && bg[2] > 250, "bg = {bg:?}");
    }

    #[test]
    fn render_page_returns_none_for_unknown_page() {
        let comp = Composition::new(1);
        assert!(render_page(&comp, "nope", &BlockRenderer, 72.0, Color::WHITE).is_none());
    }

    /// S7 — a composition survives the on-disk `document.pgd` JSON form and
    /// renders **pixel-identically** after the round-trip. This is the render
    /// side of composition persistence: whatever `CanvasModel` writes into the
    /// container part reconstructs the same page.
    #[test]
    fn persisted_composition_renders_identically() {
        let mut comp = Composition::new(1);
        comp.surfaces = vec![Surface {
            id: "print".to_string(),
            kind: SurfaceKind::Print,
        }];
        comp.pages = vec![Page {
            id: "p1".to_string(),
            size: [400.0, 200.0],
            spread: None,
        }];
        comp.nodes = vec![
            block("r0", "e51a1a", [10.0, 10.0], 100.0, 60.0),
            block("r1", "1a4de5", [200.0, 10.0], 120.0, 80.0),
        ];

        // Round-trip through the exact bytes the `document.pgd` part carries.
        let bytes = serde_json::to_vec(&comp).expect("serialize document.pgd");
        let restored: Composition =
            serde_json::from_slice(&bytes).expect("deserialize document.pgd");
        assert_eq!(
            restored, comp,
            "composition changed across the JSON round-trip"
        );

        // The persisted bytes reconstruct a pixel-identical page.
        let a =
            render_page(&comp, "p1", &BlockRenderer, 72.0, Color::WHITE).expect("render original");
        let b = render_page(&restored, "p1", &BlockRenderer, 72.0, Color::WHITE)
            .expect("render restored");
        assert_eq!(
            a.as_raw(),
            b.as_raw(),
            "persisted composition renders differently"
        );
    }
}
