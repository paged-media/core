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

//! `masters.idml` — master-page item inheritance + per-item override
//! suppression (Q-14).
//!
//! Three A4-portrait body pages, each applying its own one-page master
//! that carries two master rectangles:
//!
//!   * a *shared* navy rectangle (top-left) — always inherited onto the
//!     body page, never overridden.
//!   * an *overridable* black rectangle (bottom-right). On page 2 the
//!     body page lists this master rectangle's `Self` id in its
//!     `OverrideList` and carries its own red replacement rectangle at
//!     the same position. The renderer must suppress the master copy so
//!     the red override isn't double-painted under the black master
//!     placeholder.
//!
//! Page variants:
//!   * page 1 — both master items inherited, ShowMasterItems default.
//!   * page 2 — overridable master item suppressed via OverrideList; a
//!     red replacement frame on the body page takes its place.
//!   * page 3 — ShowMasterItems="false": every master item hidden.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "masters";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

/// Shared (inherited, never overridden) master rectangle — top-left.
const SHARED_X: f32 = 48.0;
const SHARED_Y: f32 = 48.0;
const SHARED_W: f32 = 160.0;
const SHARED_H: f32 = 80.0;

/// Overridable master rectangle — bottom-right. Page 2 replaces it.
const OVR_X: f32 = 360.0;
const OVR_Y: f32 = 700.0;
const OVR_W: f32 = 180.0;
const OVR_H: f32 = 90.0;

/// One of the three body-page variants.
enum Variant {
    /// Both master items inherited (no override, master shows items).
    InheritBoth,
    /// Overridable master item suppressed via OverrideList; a body
    /// replacement frame stands in for it.
    OverrideOne,
    /// `ShowMasterItems="false"` — every master item hidden.
    HideAll,
}

fn variants() -> Vec<(&'static str, Variant)> {
    vec![
        ("masters · inherit-both", Variant::InheritBoth),
        ("masters · override-one", Variant::OverrideOne),
        ("masters · hide-all", Variant::HideAll),
    ]
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let variants = variants();

    let mut master_spreads = Vec::with_capacity(variants.len());
    let mut spreads = Vec::with_capacity(variants.len());
    let mut master_refs = Vec::with_capacity(variants.len());
    let mut spread_refs = Vec::with_capacity(variants.len());

    for (i, (name, variant)) in variants.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        // Stable per-master item ids; the overridable one's id is what
        // the body page lists in its OverrideList.
        let shared_item_id = self_id(SAMPLE, "MasterShared", seq);
        let overridable_item_id = self_id(SAMPLE, "MasterOverridable", seq);
        let replacement_id = self_id(SAMPLE, "Replacement", seq);

        // Master items: a shared navy rect + an overridable black rect.
        let shared_master_rect: PageItem = Rect {
            self_id: shared_item_id.clone(),
            width_pt: SHARED_W,
            height_pt: SHARED_H,
            item_transform: translate(SHARED_X, SHARED_Y),
            fill_color: Some("Color/Black".into()),
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
            frame_effects: Vec::new(),
            text_frame_pref: None,
        }
        .into();
        let overridable_master_rect: PageItem = Rect {
            self_id: overridable_item_id.clone(),
            width_pt: OVR_W,
            height_pt: OVR_H,
            item_transform: translate(OVR_X, OVR_Y),
            fill_color: Some("Color/Black".into()),
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
            frame_effects: Vec::new(),
            text_frame_pref: None,
        }
        .into();

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: master_page_id.clone(),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![shared_master_rect, overridable_master_rect],
            }),
        ));
        master_refs.push(master_id.clone());

        // Body-page items + override list depend on the variant.
        let mut page_items: Vec<PageItem> = Vec::new();
        let mut override_list: Vec<String> = Vec::new();
        let mut show_master_items_false = false;
        match variant {
            Variant::InheritBoth => {}
            Variant::OverrideOne => {
                // The body page overrides the bottom-right master rect:
                // list its id so the master copy is suppressed, and add
                // a red replacement frame at the same position.
                override_list.push(overridable_item_id.clone());
                page_items.push(
                    Rect {
                        self_id: replacement_id.clone(),
                        width_pt: OVR_W,
                        height_pt: OVR_H,
                        item_transform: translate(OVR_X, OVR_Y),
                        // Paper (white) so the override reads visually
                        // distinct from the black master placeholder it
                        // suppresses. Both resolve in the built-in
                        // palette (Color/Black, Color/Paper).
                        fill_color: Some("Color/Paper".into()),
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
                        frame_effects: Vec::new(),
                        text_frame_pref: None,
                    }
                    .into(),
                );
            }
            Variant::HideAll => {
                show_master_items_false = true;
            }
        }

        let spread = Spread {
            self_id: spread_id.clone(),
            page_self_id: page_id,
            page_name: name.to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items,
            override_list,
            margins: None,
        };
        // `ShowMasterItems="false"` is a per-page attribute; the spread
        // builder doesn't model it, so stamp it into the page via the
        // post-hoc rewrite below (the only variant that needs it).
        let bytes = if show_master_items_false {
            write_spread_with_hidden_masters(&spread)
        } else {
            write_spread(&spread)
        };
        spreads.push((spread_id.clone(), bytes));
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

/// Emit a spread whose body `<Page>` carries `ShowMasterItems="false"`.
/// The standard `write_spread` doesn't model that per-page attribute, so
/// we splice it into the emitted XML. Cheap and local to this sample.
fn write_spread_with_hidden_masters(s: &Spread) -> Vec<u8> {
    let bytes = write_spread(s);
    let xml = String::from_utf8(bytes).expect("spread xml is utf-8");
    // Insert the attribute right after the page's `Self="…"` so it lands
    // on the `<Page>` element (the only element carrying that Self id).
    let needle = format!("<Page Self=\"{}\"", s.page_self_id);
    let patched = xml.replacen(&needle, &format!("{needle} ShowMasterItems=\"false\""), 1);
    patched.into_bytes()
}
