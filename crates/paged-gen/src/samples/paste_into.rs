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

//! `paste-into.idml` — B-18 nested content (InDesign paste-into).
//!
//! One A4 page: a rounded-corner container `<Rectangle>` whose element
//! NESTS three child page items (the paste-into serialisation — nested
//! page items are the container element's last children, their
//! `ItemTransform`s relative to the container):
//!
//! 1. a black rectangle that protrudes past the container's LEFT edge
//!    AND covers the container's rounded top-left corner — pinning both
//!    the straight-edge clip and the corner-effect clip;
//! 2. a black oval fully inside the container (multiple children);
//! 3. a text frame whose box extends far BELOW the container, with
//!    enough story text to reach the protruding band — pinning the
//!    story-pass glyph clip (`apply_container_clip`).
//!
//! This sample carries no paired InDesign-exported PDF and is therefore
//! NOT in `fidelity-thresholds.json` — the hard fidelity gate skips it.
//! Its correctness is pinned by `paged-renderer/tests/paste_into.rs`
//! (display-list + pixel probes) and by the writer round-trip suite.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{Oval, PageItem, Rect},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "paste-into";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

/// Container geometry (spread space): x ∈ [140, 440], y ∈ [200, 500].
pub const HOST_X: f32 = 140.0;
pub const HOST_Y: f32 = 200.0;
pub const HOST_W: f32 = 300.0;
pub const HOST_H: f32 = 300.0;
pub const HOST_CORNER_RADIUS: f32 = 60.0;

fn plain_rect(self_id: String, w: f32, h: f32, tx: crate::geometry::Matrix) -> Rect {
    Rect {
        self_id,
        width_pt: w,
        height_pt: h,
        item_transform: tx,
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
        custom_subpaths: None,
    }
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let seq = 0u32;
    let master_id = self_id(SAMPLE, "MasterSpread", seq);
    let master_page_id = self_id(SAMPLE, "MasterPage", seq);
    let spread_id = self_id(SAMPLE, "Spread", seq);
    let page_id = self_id(SAMPLE, "Page", seq);
    let story_id = self_id(SAMPLE, "Story", seq);
    let host_id = self_id(SAMPLE, "Host", seq);
    let child_rect_id = self_id(SAMPLE, "ChildRect", seq);
    let child_oval_id = self_id(SAMPLE, "ChildOval", seq);
    let child_text_id = self_id(SAMPLE, "ChildText", seq);

    let master_spreads = vec![(
        master_id.clone(),
        write_master(&Master {
            self_id: format!("MasterSpread/{master_id}"),
            page_self_id: master_page_id,
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
        }),
    )];

    // Enough text to flow well past the container's bottom edge in the
    // protruding text-frame child, so glyphs exist for the story-pass
    // clip to mask.
    let words = "paste into clips nested content to the container path ";
    let story_text: String = words.repeat(24);
    let stories = vec![(
        story_id.clone(),
        write_story(&Story {
            extra_story_attrs: Vec::new(),
            self_id: story_id.clone(),
            paragraphs: vec![Paragraph::plain(story_text)],
        }),
    )];

    // The container: rounded corners so the clip pins corner effects,
    // Paper fill + black stroke so the outline is visible but the
    // interior stays white (making child pixels unambiguous).
    let mut host = plain_rect(host_id, HOST_W, HOST_H, translate(HOST_X, HOST_Y));
    host.fill_color = Some("Color/Paper".into());
    host.stroke_color = Some("Color/Black".into());
    host.stroke_weight_pt = Some(2.0);
    host.extra_attrs = vec![
        ("CornerOption".to_string(), "Rounded".to_string()),
        ("CornerRadius".to_string(), HOST_CORNER_RADIUS.to_string()),
    ];

    // Child 1 — black rect protruding past the host's left edge and
    // over the rounded top-left corner. Child transforms are
    // HOST-RELATIVE (paste-into convention): world x ∈ [100, 400],
    // y ∈ [160, 360].
    let child_rect = plain_rect(child_rect_id, 300.0, 200.0, translate(-40.0, -40.0));

    // Child 2 — black oval fully inside: world [320, 400] × [380, 460].
    let child_oval = Oval {
        self_id: child_oval_id,
        width_pt: 80.0,
        height_pt: 80.0,
        item_transform: translate(180.0, 180.0),
        fill_color: Some("Color/Black".into()),
        stroke_color: None,
        stroke_weight_pt: None,
        extra_attrs: Vec::new(),
    };

    // Child 3 — text frame extending far below the host (world
    // [160, 360] × [360, 660]; host bottom is 500): the story fills it
    // past the host edge, and only the clip keeps the protruding band
    // white.
    let mut child_text = plain_rect(child_text_id, 200.0, 300.0, translate(20.0, 160.0));
    child_text.fill_color = None;
    child_text.parent_story = Some(story_id.clone());

    let page_items: Vec<PageItem> = vec![PageItem::PasteInto {
        container: Box::new(host),
        children: vec![child_rect.into(), child_oval.into(), child_text.into()],
    }];

    let spreads = vec![(
        spread_id.clone(),
        write_spread(&Spread {
            self_id: spread_id.clone(),
            page_self_id: page_id,
            page_name: "paste-into · rounded container · three children".to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items,
            override_list: Vec::new(),
            margins: None,
            item_transform: None,
        }),
    )];

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: vec![master_id],
        spreads: vec![spread_id],
        stories: vec![story_id],
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
        stories,
    }
}
