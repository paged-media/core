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

//! `corners.idml` — the five IDML corner options.
//!
//! One A4-portrait page per `CornerOption` (Rounded, Inverse Rounded,
//! Bevel, Inset, Fancy), each a single filled+stroked rectangle whose
//! `CornerOption` + `CornerRadius` exercise the corner-path emitter. The
//! page label names the option so a per-page diff reads
//! "corners · inset" on failure.
//!
//! This sample carries no paired InDesign-exported PDF and is therefore
//! NOT in `fidelity-thresholds.json` — the hard fidelity gate skips it
//! (the gate's fixture list is driven by that JSON). It exists so the
//! corner emitter is exercised end-to-end through `build_document`
//! alongside the pure-geometry unit tests.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "corners";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const DEMO_W_PT: f32 = 320.0;
const DEMO_H_PT: f32 = 220.0;
const CORNER_RADIUS_PT: f32 = 48.0;

/// `(label, IDML CornerOption enum value)` per page.
fn variants() -> Vec<(&'static str, &'static str)> {
    vec![
        ("corners · rounded", "Rounded"),
        ("corners · inverse-rounded", "InverseRounded"),
        ("corners · bevel", "Beveled"),
        ("corners · inset", "Inset"),
        ("corners · fancy", "Fancy"),
    ]
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let variants = variants();

    let mut master_spreads = Vec::with_capacity(variants.len());
    let mut spreads = Vec::with_capacity(variants.len());
    let mut master_refs = Vec::with_capacity(variants.len());
    let mut spread_refs = Vec::with_capacity(variants.len());

    for (i, (name, corner_option)) in variants.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let rect_id = self_id(SAMPLE, "Rectangle", seq);

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

        // One filled + stroked rectangle centred on the page, carrying
        // the page's `CornerOption` + a uniform `CornerRadius`. The
        // stroke makes the corner shape legible against the fill.
        let demo = Rect {
            self_id: rect_id,
            width_pt: DEMO_W_PT,
            height_pt: DEMO_H_PT,
            item_transform: translate(
                (PAGE_W_PT - DEMO_W_PT) * 0.5,
                (PAGE_H_PT - DEMO_H_PT) * 0.5,
            ),
            fill_color: Some("Color/Paper".into()),
            stroke_color: Some("Color/Black".into()),
            stroke_weight_pt: Some(3.0),
            parent_story: None,
            extra_attrs: vec![
                ("CornerOption".to_string(), (*corner_option).to_string()),
                ("CornerRadius".to_string(), CORNER_RADIUS_PT.to_string()),
            ],
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
        };

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![demo.into()],
                override_list: Vec::new(),
            }),
        ));
        spread_refs.push(spread_id);
    }

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: master_refs,
        spreads: spread_refs,
        stories: Vec::new(),
    });

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories: Vec::new(),
    }
}
