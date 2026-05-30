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

//! Phase-2 mega-file: `transparency.idml`.
//!
//! Pages exercise IDML's transparency model — the cross-product of
//! `BlendMode`, `Opacity`, and `DropShadowSetting` — with one shape
//! per page composited atop a coloured backdrop so the variant is
//! visually meaningful in the rendered PDF:
//!
//!   * opacity sweep (100 / 50 / 25)
//!   * blend modes (Multiply, Screen, Lighten, Darken, Overlay) each
//!     paired with the backdrop colour that makes the mode legible
//!   * drop shadow (default — relies on the §IDML Defaults Table 84
//!     fall-throughs — and an explicit-attribute variant)
//!   * combos (BlendMode + Opacity, DropShadow + Opacity)
//!
//! Layout is uniform across all pages: a ~200×150 backdrop centred at
//! (200, 300), a ~150×100 test rectangle centred at (250, 350) so it
//! overlaps the backdrop on three sides, and a label at (36, 36)
//! identifying the variant. The rendered PDF reads top-to-bottom as
//! `<label>` / shape over backdrop, which keeps human review legible
//! when paired with an InDesign reference export.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{Blending, DropShadow, Rect},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml,
        ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "transparency";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

const BACKDROP_W_PT: f32 = 200.0;
const BACKDROP_H_PT: f32 = 150.0;
/// Backdrop centre — top-left corner is (CX - W/2, CY - H/2).
const BACKDROP_CX_PT: f32 = 200.0;
const BACKDROP_CY_PT: f32 = 300.0;

const TEST_W_PT: f32 = 150.0;
const TEST_H_PT: f32 = 100.0;
const TEST_CX_PT: f32 = 250.0;
const TEST_CY_PT: f32 = 350.0;

const LABEL_W_PT: f32 = 360.0;
const LABEL_H_PT: f32 = 24.0;

/// One page-spec — describes the test rectangle's fill, the backdrop
/// colour painted underneath it, and the optional blending /
/// drop-shadow that's the actual subject of the page.
struct Variant {
    name: &'static str,
    /// Backdrop fill colour (`Color/<id>`). `None` ⇒ no backdrop is
    /// drawn (used for the opacity-100 baseline page where there is
    /// nothing to blend onto).
    backdrop: Option<&'static str>,
    /// Test-shape fill colour (`Color/<id>`).
    fill_color: &'static str,
    blending: Option<Blending>,
    drop_shadow: Option<DropShadow>,
}

fn variants() -> Vec<Variant> {
    vec![
        // ── opacity sweep ─────────────────────────────────────────
        // Baseline: 100 % opacity, no backdrop. Anything visible
        // here is just the solid black rect — confirms that
        // `<BlendingSetting Opacity="100"/>` is a no-op.
        Variant {
            name: "transparency · opacity · 100",
            backdrop: None,
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: None,
            }),
            drop_shadow: None,
        },
        // 50 % over a yellow backdrop — the test rect should pick
        // up the underlying yellow so it lightens.
        Variant {
            name: "transparency · opacity · 50",
            backdrop: Some("Color/CMYKYellow"),
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(50.0),
                blend_mode: None,
            }),
            drop_shadow: None,
        },
        // 25 % — backdrop dominates, test rect is a faint grey film.
        Variant {
            name: "transparency · opacity · 25",
            backdrop: Some("Color/CMYKYellow"),
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(25.0),
                blend_mode: None,
            }),
            drop_shadow: None,
        },
        // ── blend modes ──────────────────────────────────────────
        // Multiply: black on yellow = black (where the rect overlaps);
        // yellow elsewhere. Standard "darken-only" sanity check.
        Variant {
            name: "transparency · blend · multiply",
            backdrop: Some("Color/CMYKYellow"),
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: Some("Multiply"),
            }),
            drop_shadow: None,
        },
        // Screen: white on black = white over the overlap. The
        // inverse of Multiply.
        Variant {
            name: "transparency · blend · screen",
            backdrop: Some("Color/Black"),
            fill_color: "Color/Paper",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: Some("Screen"),
            }),
            drop_shadow: None,
        },
        // Lighten: per-channel max(rect, backdrop). Black rect over
        // magenta should leave the magenta unchanged where they
        // overlap (since magenta is brighter than black on every
        // channel) — handy "no-op" check for the operator.
        Variant {
            name: "transparency · blend · lighten",
            backdrop: Some("Color/CMYKMagenta"),
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: Some("Lighten"),
            }),
            drop_shadow: None,
        },
        // Darken: per-channel min. White rect over cyan leaves cyan
        // unchanged (cyan is darker than white on every channel) —
        // matches the Lighten-with-black symmetry.
        Variant {
            name: "transparency · blend · darken",
            backdrop: Some("Color/CMYKCyan"),
            fill_color: "Color/Paper",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: Some("Darken"),
            }),
            drop_shadow: None,
        },
        // Overlay: contrast-preserving blend. 50 % grey over yellow
        // is the canonical demo — the highlights stay yellow, the
        // shadows go orange-ish.
        Variant {
            name: "transparency · blend · overlay",
            backdrop: Some("Color/CMYKYellow"),
            fill_color: "Color/CMYKGray50",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: Some("Overlay"),
            }),
            drop_shadow: None,
        },
        // ── drop shadow ──────────────────────────────────────────
        // Default-attrs variant: emits `<DropShadowSetting
        // Mode="Drop"/>` with no other attrs so the parser pulls
        // XOffset=7, YOffset=7, Size=5, Opacity=75 from §IDML
        // Defaults Table 84. Rectangles a renderer regression that
        // pinned these to zero before commit 9f98738.
        Variant {
            name: "transparency · drop-shadow · default",
            backdrop: None,
            fill_color: "Color/Black",
            blending: None,
            drop_shadow: Some(DropShadow::default_drop()),
        },
        // Explicit-attrs variant: every parameter pinned. Lets the
        // diff harness watch for a regression in the offset / size /
        // opacity parsing path independent of the defaults path.
        Variant {
            name: "transparency · drop-shadow · explicit",
            backdrop: None,
            fill_color: "Color/Black",
            blending: None,
            drop_shadow: Some(DropShadow {
                mode: "Drop",
                x_offset: Some(12.0),
                y_offset: Some(12.0),
                size: Some(8.0),
                opacity_pct: Some(50.0),
                effect_color: Some("Color/Black".to_string()),
            }),
        },
        // ── combos ───────────────────────────────────────────────
        // BlendMode + Opacity stacked on the same rect. Multiply
        // first (against the yellow backdrop), then 50 % opacity —
        // the result is the multiply-blended value composited 50/50
        // with the backdrop.
        Variant {
            name: "transparency · combo · blend+opacity",
            backdrop: Some("Color/CMYKYellow"),
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(50.0),
                blend_mode: Some("Multiply"),
            }),
            drop_shadow: None,
        },
        // DropShadow + Opacity. The shadow renders against paper
        // (no backdrop), then 50 % opacity scales both the rect's
        // fill and the shadow alpha by half — confirming the shadow
        // composites through the frame's transparency rather than
        // being painted at its own absolute alpha.
        Variant {
            name: "transparency · combo · shadow+opacity",
            backdrop: None,
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(50.0),
                blend_mode: None,
            }),
            drop_shadow: Some(DropShadow {
                mode: "Drop",
                x_offset: Some(8.0),
                y_offset: Some(8.0),
                size: Some(6.0),
                opacity_pct: Some(75.0),
                effect_color: Some("Color/Black".to_string()),
            }),
        },
    ]
}

/// Custom CMYK swatches the variants above reference. Pinned CMYK
/// (rather than RGB) because the IDML BlendMode equations operate on
/// the device colour space — keeping everything in CMYK avoids a
/// CMYK→RGB conversion shifting the demo colour mid-blend.
fn extra_colors() -> Vec<ExtraColor> {
    vec![
        ExtraColor {
            self_id: "Color/CMYKYellow".to_string(),
            name: "Yellow".to_string(),
            space: "CMYK",
            value: "0 0 100 0".to_string(),
        },
        ExtraColor {
            self_id: "Color/CMYKMagenta".to_string(),
            name: "Magenta".to_string(),
            space: "CMYK",
            value: "0 100 0 0".to_string(),
        },
        ExtraColor {
            self_id: "Color/CMYKCyan".to_string(),
            name: "Cyan".to_string(),
            space: "CMYK",
            value: "100 0 0 0".to_string(),
        },
        ExtraColor {
            self_id: "Color/CMYKGray50".to_string(),
            name: "Gray 50".to_string(),
            space: "CMYK",
            value: "0 0 0 50".to_string(),
        },
    ]
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let variants = variants();

    let mut master_spreads = Vec::with_capacity(variants.len());
    let mut spreads = Vec::with_capacity(variants.len());
    let mut stories = Vec::with_capacity(variants.len());
    let mut master_refs = Vec::with_capacity(variants.len());
    let mut spread_refs = Vec::with_capacity(variants.len());
    let mut story_refs = Vec::with_capacity(variants.len());

    for (i, variant) in variants.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let story_id = self_id(SAMPLE, "Story", seq);
        let label_frame_id = self_id(SAMPLE, "TextFrame", seq);
        let backdrop_id = self_id(SAMPLE, "Backdrop", seq);
        let test_id = self_id(SAMPLE, "Rectangle", seq);

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: master_page_id.clone(),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
            }),
        ));
        master_refs.push(master_id.clone());

        stories.push((
            story_id.clone(),
            write_story(&Story {
                self_id: story_id.clone(),
                paragraphs: vec![Paragraph::plain(variant.name)],
            }),
        ));
        story_refs.push(story_id.clone());

        let label = Rect {
            self_id: label_frame_id,
            width_pt: LABEL_W_PT,
            height_pt: LABEL_H_PT,
            item_transform: translate(36.0, 36.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(story_id.clone()),
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
        };

        let backdrop_x = BACKDROP_CX_PT - BACKDROP_W_PT * 0.5;
        let backdrop_y = BACKDROP_CY_PT - BACKDROP_H_PT * 0.5;
        let test_x = TEST_CX_PT - TEST_W_PT * 0.5;
        let test_y = TEST_CY_PT - TEST_H_PT * 0.5;

        let test_rect = Rect {
            self_id: test_id,
            width_pt: TEST_W_PT,
            height_pt: TEST_H_PT,
            item_transform: compose_translate(test_x, test_y),
            fill_color: Some(variant.fill_color.to_string()),
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: None,
            extra_attrs: Vec::new(),
            blending: variant.blending.clone(),
            drop_shadow: variant.drop_shadow.clone(),
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
        };

        let mut page_items = Vec::with_capacity(3);
        page_items.push(label.into());
        // Backdrop drawn first so the test rect composites on top
        // (back-to-front emit order = Z-order in IDML).
        if let Some(backdrop_color) = variant.backdrop {
            page_items.push(
                Rect {
                    self_id: backdrop_id,
                    width_pt: BACKDROP_W_PT,
                    height_pt: BACKDROP_H_PT,
                    item_transform: compose_translate(backdrop_x, backdrop_y),
                    fill_color: Some(backdrop_color.to_string()),
                    stroke_color: None,
                    stroke_weight_pt: None,
                    parent_story: None,
                    extra_attrs: Vec::new(),
                    blending: None,
                    drop_shadow: None,
                    placed_image: None,
                    text_wrap: None,
                    anchored_setting: None,
                }
                .into(),
            );
        }
        page_items.push(test_rect.into());

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: variant.name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items,
            }),
        ));
        spread_refs.push(spread_id);
    }

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: master_refs,
        spreads: spread_refs,
        stories: story_refs,
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic_xml_with_extras(&extra_colors()),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}

/// Identity matrix translated to (`tx`, `ty`). Same helper that
/// lives in the other samples — kept private here so its scope is
/// obvious to a reader following one file at a time.
fn compose_translate(tx: f32, ty: f32) -> Matrix {
    let mut m = IDENTITY;
    m[4] = tx;
    m[5] = ty;
    m
}
