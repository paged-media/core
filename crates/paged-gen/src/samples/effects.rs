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

//! Phase-1 mega-file: `effects.idml`.
//!
//! Pages exercise transparency + drop-shadow rendering paths the
//! strokes-fills sample intentionally left untouched:
//!   * BlendingSetting Opacity (25 / 50 / 75 / 100)
//!   * BlendMode (Multiply / Screen / Overlay / Darken / Lighten)
//!   * DropShadowSetting (default + offset + size sweep)
//!
//! Each variant lives on its own A4 page with the descriptor as both
//! `Page.Name` and the visible label, so the diff harness can
//! attribute failure per page.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{Blending, DropShadow, Rect},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml, ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "effects";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const DEMO_W_PT: f32 = 240.0;
const DEMO_H_PT: f32 = 140.0;
const LABEL_W_PT: f32 = 360.0;
const LABEL_H_PT: f32 = 24.0;

struct Variant {
    name: &'static str,
    fill_color: &'static str,
    blending: Option<Blending>,
    drop_shadow: Option<DropShadow>,
    /// When set, draw a second underlay rectangle behind the demo so
    /// blend-mode variants have something coloured to blend onto.
    /// Stored as `(fill_color, dx_pt, dy_pt)`.
    underlay: Option<(&'static str, f32, f32)>,
}

fn variants() -> Vec<Variant> {
    let cyan_underlay = Some(("Color/RGBCyan", -36.0, -24.0));
    vec![
        // Opacity sweep on a black-fill rectangle. Frame-level
        // BlendingSetting Opacity scales every paint's alpha.
        Variant {
            name: "effects · opacity · 100",
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: None,
            }),
            drop_shadow: None,
            underlay: None,
        },
        Variant {
            name: "effects · opacity · 75",
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(75.0),
                blend_mode: None,
            }),
            drop_shadow: None,
            underlay: cyan_underlay,
        },
        Variant {
            name: "effects · opacity · 50",
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(50.0),
                blend_mode: None,
            }),
            drop_shadow: None,
            underlay: cyan_underlay,
        },
        Variant {
            name: "effects · opacity · 25",
            fill_color: "Color/Black",
            blending: Some(Blending {
                opacity_pct: Some(25.0),
                blend_mode: None,
            }),
            drop_shadow: None,
            underlay: cyan_underlay,
        },
        // Blend modes — each over a cyan underlay so the blend is
        // visible. Opacity stays 100% to isolate the mode itself.
        Variant {
            name: "effects · blend · multiply",
            fill_color: "Color/Magenta50",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: Some("Multiply"),
            }),
            drop_shadow: None,
            underlay: cyan_underlay,
        },
        Variant {
            name: "effects · blend · screen",
            fill_color: "Color/Magenta50",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: Some("Screen"),
            }),
            drop_shadow: None,
            underlay: cyan_underlay,
        },
        Variant {
            name: "effects · blend · overlay",
            fill_color: "Color/Magenta50",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: Some("Overlay"),
            }),
            drop_shadow: None,
            underlay: cyan_underlay,
        },
        Variant {
            name: "effects · blend · darken",
            fill_color: "Color/Magenta50",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: Some("Darken"),
            }),
            drop_shadow: None,
            underlay: cyan_underlay,
        },
        Variant {
            name: "effects · blend · lighten",
            fill_color: "Color/Magenta50",
            blending: Some(Blending {
                opacity_pct: Some(100.0),
                blend_mode: Some("Lighten"),
            }),
            drop_shadow: None,
            underlay: cyan_underlay,
        },
        // Drop shadows — vary offset and blur to verify the renderer
        // honours each independently.
        Variant {
            name: "effects · drop-shadow · default",
            fill_color: "Color/Paper",
            blending: None,
            drop_shadow: Some(DropShadow {
                mode: "Drop",
                x_offset: Some(6.0),
                y_offset: Some(6.0),
                size: Some(6.0),
                opacity_pct: Some(75.0),
                effect_color: Some("Color/Black".to_string()),
            }),
            underlay: None,
        },
        Variant {
            name: "effects · drop-shadow · large-offset",
            fill_color: "Color/Paper",
            blending: None,
            drop_shadow: Some(DropShadow {
                mode: "Drop",
                x_offset: Some(18.0),
                y_offset: Some(18.0),
                size: Some(6.0),
                opacity_pct: Some(75.0),
                effect_color: Some("Color/Black".to_string()),
            }),
            underlay: None,
        },
        Variant {
            name: "effects · drop-shadow · large-blur",
            fill_color: "Color/Paper",
            blending: None,
            drop_shadow: Some(DropShadow {
                mode: "Drop",
                x_offset: Some(6.0),
                y_offset: Some(6.0),
                size: Some(24.0),
                opacity_pct: Some(75.0),
                effect_color: Some("Color/Black".to_string()),
            }),
            underlay: None,
        },
    ]
}

fn extra_colors() -> Vec<ExtraColor> {
    vec![
        ExtraColor {
            self_id: "Color/RGBCyan".to_string(),
            name: "RGB Cyan".to_string(),
            space: "RGB",
            value: "0 200 220".to_string(),
        },
        // 50% magenta CMYK swatch — chosen so the blend variants
        // produce an obviously different colour against the cyan
        // underlay (cyan + 50% magenta = blue under Multiply).
        ExtraColor {
            self_id: "Color/Magenta50".to_string(),
            name: "Magenta 50".to_string(),
            space: "CMYK",
            value: "0 50 0 0".to_string(),
        },
    ]
}

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
        let demo_id = self_id(SAMPLE, "Rectangle", seq);
        let underlay_id = self_id(SAMPLE, "Underlay", seq);

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: master_page_id.clone(),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
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
            next_text_frame: None,
            previous_text_frame: None,
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
            text_frame_pref: None,
        };

        let demo_x = (PAGE_W_PT - DEMO_W_PT) * 0.5;
        let demo_y = (PAGE_H_PT - DEMO_H_PT) * 0.5;
        let demo = Rect {
            self_id: demo_id,
            width_pt: DEMO_W_PT,
            height_pt: DEMO_H_PT,
            item_transform: compose_translate(demo_x, demo_y),
            fill_color: Some(variant.fill_color.to_string()),
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: None,
            next_text_frame: None,
            previous_text_frame: None,
            extra_attrs: Vec::new(),
            blending: variant.blending.clone(),
            drop_shadow: variant.drop_shadow.clone(),
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
            text_frame_pref: None,
        };

        let mut page_items = Vec::with_capacity(3);
        page_items.push(label.into());
        if let Some((color, dx, dy)) = variant.underlay {
            // Underlay drawn first so the demo composites on top.
            page_items.push(
                Rect {
                    self_id: underlay_id,
                    width_pt: DEMO_W_PT,
                    height_pt: DEMO_H_PT,
                    item_transform: compose_translate(demo_x + dx, demo_y + dy),
                    fill_color: Some(color.to_string()),
                    stroke_color: None,
                    stroke_weight_pt: None,
                    parent_story: None,
                    next_text_frame: None,
                    previous_text_frame: None,
                    extra_attrs: Vec::new(),
                    blending: None,
                    drop_shadow: None,
                    placed_image: None,
                    text_wrap: None,
                    anchored_setting: None,
                    text_frame_pref: None,
                }
                .into(),
            );
        }
        page_items.push(demo.into());

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
                override_list: Vec::new(),
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

fn compose_translate(tx: f32, ty: f32) -> Matrix {
    let mut m = IDENTITY;
    m[4] = tx;
    m[5] = ty;
    m
}
