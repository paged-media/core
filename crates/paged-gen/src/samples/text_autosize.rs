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

//! W1.7 fixture: `text-autosize.idml`.
//!
//! One A4 page exercising **TextFrame AutoSizing Phase B** — the
//! visible effects of an `AutoSizingType="HeightOnly"` frame:
//!
//!   * **frame A — the auto-sizing headline.** Authored deliberately
//!     undersized (40 pt tall) but holds many short paragraphs, so the
//!     renderer grows its box downward to fit. The frame carries a
//!     `FillColor` + stroke (so the *painted box* growth is visible) and
//!     a `BoundingBoxTextWrap` (so the grown box becomes a wrap
//!     exclusion).
//!   * **frame B — the wrapped neighbour.** A plain body text frame
//!     positioned so its column overlaps frame A's GROWN vertical band
//!     (well below A's authored 40 pt bottom). Its lines that fall in
//!     that band wrap around frame A's grown box, not its authored rect.
//!
//! Phase A (landed earlier) keeps placing A's overflow lines downward
//! instead of dropping them; Phase B makes A's fill/stroke box and the
//! neighbour's wrap exclusion follow that growth. Both are layout-time
//! effects, so the snapshot tests build the document through
//! `paged-renderer` and assert the grown box + shifted neighbour wrap
//! rather than a structural proxy.
//!
//! Body runs pin `AppliedFont="Inter"` for deterministic shaping
//! against `corpus/fonts/Inter.ttf` (the harness-registered face).

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref, TextWrap},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "text-autosize";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

const BODY_FONT: &str = "Inter";

/// Frame A authored size: a short, wide headline box that the renderer
/// grows downward to fit its 12 short paragraphs.
const A_W_PT: f32 = 240.0;
const A_H_PT: f32 = 40.0;
/// Frame A's page position (top-left of its inner box, page coords).
const A_X_PT: f32 = 36.0;
const A_Y_PT: f32 = 60.0;

/// Frame B (the neighbour) starts well below A's authored 40 pt bottom
/// (A_Y + A_H = 100) but inside A's grown band, and overlaps A's column
/// in x so the grown box carves its left edge.
const B_X_PT: f32 = 36.0;
const B_Y_PT: f32 = 140.0;
const B_W_PT: f32 = 360.0;
const B_H_PT: f32 = 560.0;

/// One body paragraph pinned to Inter.
fn inter_paragraph(text: &str, point_size: f32) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: Vec::new(),
        justification: None,
        applied_numbering_list: None,
        space_before: None,
        space_after: None,
        leading: None,
        first_line_indent: None,
        left_indent: None,
        right_indent: None,
        drop_cap_characters: None,
        drop_cap_lines: None,
        tab_list: Vec::new(),
        bullets_list_type: None,
        bullet_character: None,
        table: None,
        minimum_letter_spacing: None,
        desired_letter_spacing: None,
        maximum_letter_spacing: None,
        runs: vec![Run {
            extra_char_attrs: Vec::new(),
            text: text.to_string(),
            point_size: Some(point_size),
            fill_color: Some("Color/Black".to_string()),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: Some(BODY_FONT),
            anchored_frame: None,
        }],
    }
}

pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let master = write_master(&Master {
        self_id: format!("MasterSpread/{master_id}"),
        page_self_id: master_page_id,
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: Vec::new(),
    });

    let spread_id = self_id(SAMPLE, "Spread", 0);
    let page_id = self_id(SAMPLE, "Page", 0);
    let head_story_id = self_id(SAMPLE, "Story", 0);
    let head_frame_id = self_id(SAMPLE, "TextFrame", 0);
    let body_story_id = self_id(SAMPLE, "Story", 1);
    let body_frame_id = self_id(SAMPLE, "TextFrame", 1);

    // Frame A's story: 12 short headline lines so the 40 pt box grows
    // several-fold.
    let head_story = Story {
        self_id: head_story_id.clone(),
        paragraphs: (0..12)
            .map(|i| inter_paragraph(&format!("Headline line {i}"), 11.0))
            .collect(),
    };
    // Frame B's story: one paragraph that wraps to several lines; the
    // lines in A's grown band get carved on the left, shifting the
    // breaks. Sized to fit the 560 pt frame even when the grown box adds
    // a few carved lines (so B itself never oversets — only its wrap
    // shifts).
    let body_story = Story {
        self_id: body_story_id.clone(),
        paragraphs: vec![inter_paragraph(
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu \
             nu xi omicron pi rho sigma tau upsilon phi chi psi omega",
            12.0,
        )],
    };

    // Frame A — the auto-sizing headline: undersized, filled + stroked,
    // with a BoundingBox text wrap so the GROWN box excludes neighbours.
    let head_frame = Rect {
        self_id: head_frame_id,
        width_pt: A_W_PT,
        height_pt: A_H_PT,
        item_transform: translate(A_X_PT, A_Y_PT),
        fill_color: Some("Color/Black".to_string()),
        stroke_color: Some("Color/Black".to_string()),
        stroke_weight_pt: Some(1.0),
        parent_story: Some(head_story_id.clone()),
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: Some(TextWrap {
            mode: "BoundingBoxTextWrap",
            offsets: [0.0, 0.0, 0.0, 0.0],
            side: None,
        }),
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: Some(TextFramePref {
            auto_sizing_type: Some("HeightOnly"),
            auto_sizing_reference_point: Some("TopLeftPoint"),
            ..Default::default()
        }),
        custom_subpaths: None,
    };
    // Frame B — the wrapped neighbour: plain body text frame.
    let body_frame = Rect {
        self_id: body_frame_id,
        width_pt: B_W_PT,
        height_pt: B_H_PT,
        item_transform: translate(B_X_PT, B_Y_PT),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(body_story_id.clone()),
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

    let stories: Vec<(String, Vec<u8>)> = vec![
        (head_story_id.clone(), write_story(&head_story)),
        (body_story_id.clone(), write_story(&body_story)),
    ];
    let story_refs = vec![head_story_id, body_story_id];

    let items: Vec<PageItem> = vec![head_frame.into(), body_frame.into()];
    let spread = write_spread(&Spread {
        self_id: spread_id.clone(),
        page_self_id: page_id,
        page_name: "autosize · height-only + wrapped neighbour".to_string(),
        applied_master: format!("MasterSpread/{master_id}"),
        page_width_pt: PAGE_W_PT,
        page_height_pt: PAGE_H_PT,
        page_items: items,
        override_list: Vec::new(),
        margins: None,
        item_transform: None,
    });

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id.clone()],
        spreads: vec![spread_id.clone()],
        stories: story_refs,
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
        master_spreads: vec![(master_id, master)],
        spreads: vec![(spread_id, spread)],
        stories,
    }
}
