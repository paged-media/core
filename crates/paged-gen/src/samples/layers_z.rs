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

//! `layers-z.idml` — cross-shape z-ordering.
//!
//! One page with two overlapping rectangles bound to two `<Layer>`s. The
//! back rect sits on the lower layer (declared second in the designmap,
//! so painted first); the front rect on the upper layer. The renderer
//! z-sorts by layer order then XML order, so the front rect must occlude
//! the back rect's overlap region.

use crate::builders::{
    designmap::{write_designmap_with_markers, DesignMap, LayerDef, MarkerResources},
    master::{write_master, Master},
    page_item::Rect,
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml, ExtraColor,
    },
    spread::{write_spread, Spread},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "layers-z";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const RECT_W_PT: f32 = 220.0;
const RECT_H_PT: f32 = 220.0;

fn extra_colors() -> Vec<ExtraColor> {
    vec![ExtraColor {
        self_id: "Color/RGBRed".to_string(),
        name: "RGB Red".to_string(),
        space: "RGB",
        value: "220 40 40".to_string(),
    }]
}

pub fn build() -> Sample {
    // Upper layer declared first, lower layer second — matching InDesign's
    // top-to-bottom layer panel order.
    let layer_front = "ud81".to_string();
    let layer_back = "ud82".to_string();

    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);

    // Back rect (black) on the lower layer, top-left.
    let back = Rect {
        self_id: self_id(SAMPLE, "RectBack", 0),
        width_pt: RECT_W_PT,
        height_pt: RECT_H_PT,
        item_transform: translate(140.0, 240.0),
        fill_color: Some("Color/Black".to_string()),
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: None,
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: vec![("ItemLayer".to_string(), layer_back.clone())],
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    };
    // Front rect (red) on the upper layer, offset so it overlaps the back.
    let front = Rect {
        self_id: self_id(SAMPLE, "RectFront", 0),
        width_pt: RECT_W_PT,
        height_pt: RECT_H_PT,
        item_transform: translate(235.0, 335.0),
        fill_color: Some("Color/RGBRed".to_string()),
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: None,
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: vec![("ItemLayer".to_string(), layer_front.clone())],
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    };

    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });

    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: page_id,
        page_name: "layers-z · overlap · front-occludes-back".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        // XML order: back then front (front later → higher within a layer);
        // the layer binding is the primary z-key.
        page_items: vec![back.into(), front.into()],
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

    let markers = MarkerResources {
        layers: vec![
            LayerDef {
                self_id: layer_front,
                name: "Foreground".to_string(),
            },
            LayerDef {
                self_id: layer_back,
                name: "Background".to_string(),
            },
        ],
        ..MarkerResources::default()
    };
    let designmap = write_designmap_with_markers(
        &DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id.clone()],
            spreads: vec![spread_id],
            stories: Vec::new(),
        },
        &markers,
    );

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
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id_for_refs(), spread)],
        stories: Vec::new(),
    }
}

fn spread_id_for_refs() -> String {
    self_id(SAMPLE, "Spread", 0)
}
