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

//! W4.3 mega-file: `conditions.idml`.
//!
//! The first generated fixture to carry populated `<Condition>`
//! definitions (the W2.14 honest gap: no corpus IDML previously declared
//! conditions, and there is no condition-CREATE op, so the renderer's
//! pre-layout conditional-text DROP path had only a logic-mirror unit
//! test — never an end-to-end fixture). One A4 page, one body frame whose
//! story holds three runs, each in its own paragraph so the glyph stream
//! is unambiguous:
//!
//!   * an **ungated** run (`AppliedConditions` absent) — always renders.
//!   * a **visible-gated** run (`AppliedConditions="Condition/Visible"`,
//!     defined `Visible="true"`) — renders.
//!   * a **hidden-gated** run (`AppliedConditions="Condition/Hidden"`,
//!     defined `Visible="false"`) — DROPPED before layout.
//!
//! `Resources/Styles.xml` carries both `<Condition>` defs via
//! [`styles_xml_with_conditions`]. The renderer test asserts the hidden
//! run's glyphs are absent while the other two render.

use crate::builders::designmap::{write_designmap, DesignMap};
use crate::builders::master::{write_master, Master};
use crate::builders::page_item::Rect;
use crate::builders::resources::{
    container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml_with_conditions,
    ConditionSpec,
};
use crate::builders::spread::{write_spread, Spread};
use crate::builders::xml_folder::{backing_story_xml, mapping_xml, tags_xml};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;
use crate::xml::XmlBuilder;

const SAMPLE: &str = "conditions";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 480.0;
const FRAME_H_PT: f32 = 360.0;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

const NO_PARA_STYLE: &str = "ParagraphStyle/$ID/[No paragraph style]";
const NO_CHAR_STYLE: &str = "CharacterStyle/$ID/[No character style]";

/// The two conditions this fixture defines. The token values double as
/// the `AppliedConditions` references on the gated runs.
pub const CONDITION_VISIBLE: &str = "Condition/Visible";
pub const CONDITION_HIDDEN: &str = "Condition/Hidden";

/// Text bodies — distinct ASCII glyphs so a substring assert over the
/// rendered glyph stream can tell which runs survived the filter.
pub const UNGATED_TEXT: &str = "ALWAYS";
pub const VISIBLE_TEXT: &str = "SHOWME";
pub const HIDDEN_TEXT: &str = "DROPME";

/// One body paragraph carrying a single run. `applied_conditions` is the
/// space-separated `AppliedConditions` value (empty ⇒ omit the attribute,
/// i.e. an ungated run).
fn write_gated_paragraph(b: &mut XmlBuilder, text: &str, applied_conditions: &str) {
    b.start(
        "ParagraphStyleRange",
        &[("AppliedParagraphStyle", NO_PARA_STYLE)],
    );
    let mut attrs: Vec<(&str, &str)> = vec![("AppliedCharacterStyle", NO_CHAR_STYLE)];
    if !applied_conditions.is_empty() {
        attrs.push(("AppliedConditions", applied_conditions));
    }
    b.start("CharacterStyleRange", &attrs);
    b.start("Content", &[]);
    b.text(text);
    b.end("Content");
    b.end("CharacterStyleRange");
    b.end("ParagraphStyleRange");
}

fn body_story_xml(story_id: &str) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", story_id)]);
    write_gated_paragraph(&mut b, UNGATED_TEXT, "");
    write_gated_paragraph(&mut b, VISIBLE_TEXT, CONDITION_VISIBLE);
    write_gated_paragraph(&mut b, HIDDEN_TEXT, CONDITION_HIDDEN);
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}

pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let story_id = self_id(SAMPLE, "Story", 0);
    let frame_id = self_id(SAMPLE, "TextFrame", 0);
    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);

    let story_bytes = body_story_xml(&story_id);

    let body_rect = Rect {
        self_id: frame_id,
        width_pt: FRAME_W_PT,
        height_pt: FRAME_H_PT,
        item_transform: translate(
            (PAGE_W_PT - FRAME_W_PT) * 0.5,
            (PAGE_H_PT - FRAME_H_PT) * 0.33,
        ),
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
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    };

    let master_bytes = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });

    let spread_bytes = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: page_id,
        page_name: "conditions".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: vec![body_rect.into()],
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id.clone()],
        spreads: vec![spread_id.clone()],
        stories: vec![story_id.clone()],
    });

    let conditions = [
        ConditionSpec {
            self_id: CONDITION_VISIBLE,
            name: "Visible",
            visible: true,
        },
        ConditionSpec {
            self_id: CONDITION_HIDDEN,
            name: "Hidden",
            visible: false,
        },
    ];

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml_with_conditions(&conditions),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads: vec![(master_id, master_bytes)],
        spreads: vec![(spread_id, spread_bytes)],
        stories: vec![(story_id, story_bytes)],
    }
}
