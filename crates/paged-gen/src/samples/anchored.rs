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

//! Phase-2 mega-file: `anchored.idml`.
//!
//! Anchored objects — `<TextFrame>` (or `<Rectangle>`) elements
//! nested inside a `<CharacterStyleRange>` of a flowing story, each
//! carrying an `<AnchoredObjectSetting>` payload.
//!
//! IDML's anchored-object structure:
//!
//! ```xml
//! <Story>
//!   <ParagraphStyleRange ...>
//!     <CharacterStyleRange ...>
//!       <Content>Lorem ipsum </Content>
//!     </CharacterStyleRange>
//!     <CharacterStyleRange ...>
//!       <TextFrame Self="..." ParentStory="...">
//!         <Properties>...</Properties>
//!         <AnchoredObjectSetting AnchoredPosition="InlinePosition" .../>
//!       </TextFrame>
//!       <Content> </Content>      <!-- glyph slot for the inline anchor -->
//!     </CharacterStyleRange>
//!     <CharacterStyleRange ...>
//!       <Content> dolor sit amet.</Content>
//!     </CharacterStyleRange>
//!   </ParagraphStyleRange>
//! </Story>
//! ```
//!
//! Variants:
//!   * `anchored · inline · in-line-with-text` — small graphic
//!     anchored inline (text flows around it as if it were a glyph).
//!   * `anchored · above-line · custom` — graphic positioned above
//!     the host line.
//!   * `anchored · custom-x-y · 24-12pt-offset` — explicit AnchorXoffset
//!     and AnchorYoffset values.
//!   * `anchored · custom · textframe-top-right` — Custom positioning
//!     against the host text frame, top-right corner snapped to the
//!     frame's top-right edge (deterministic placement for the renderer
//!     emission test).
//!   * `anchored · prevent-manual-positioning` — `LockPosition="true"`.
//!   * `anchored · spine-relative` — `SpineRelative="true"`.
//!   * `anchored · with-text-wrap` — anchored frame carrying its own
//!     `<TextWrapPreference>` so the host story flows around it.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{AnchoredObjectSetting, Rect, TextWrap},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "anchored";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const BODY_W_PT: f32 = 460.0;
const BODY_H_PT: f32 = 720.0;
const ANCHOR_W_PT: f32 = 60.0;
const ANCHOR_H_PT: f32 = 36.0;
const LABEL_W_PT: f32 = 460.0;
const LABEL_H_PT: f32 = 24.0;

/// One page-spec — describes the AnchoredObjectSetting attribute
/// payload to attach to the inline frame.
struct Variant {
    name: &'static str,
    setting_factory: fn() -> AnchoredObjectSetting,
    /// When true, the anchored frame also carries a
    /// `<TextWrapPreference>` so the host story flows around it.
    with_wrap: bool,
}

fn inline() -> AnchoredObjectSetting {
    AnchoredObjectSetting::inline()
}

fn above_line() -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchored_position: "AboveLine",
        spine_relative: false,
        lock_position: false,
        pin_position: true,
        anchor_point: Some("TopCenterAnchor"),
        horizontal_reference_point: Some("TextFrame"),
        vertical_reference_point: Some("LineBaseline"),
        horizontal_alignment: Some("CenterAlign"),
        vertical_alignment: Some("BottomAlign"),
        anchor_x_offset: None,
        anchor_y_offset: None,
    }
}

fn custom_offset() -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchored_position: "Anchored",
        spine_relative: false,
        lock_position: false,
        pin_position: true,
        anchor_point: Some("TopLeftAnchor"),
        horizontal_reference_point: Some("AnchorLocation"),
        vertical_reference_point: Some("AnchorLocation"),
        horizontal_alignment: Some("LeftAlign"),
        vertical_alignment: Some("TopAlign"),
        anchor_x_offset: Some(24.0),
        anchor_y_offset: Some(12.0),
    }
}

/// Custom positioning against the host **text frame** with the frame's
/// top-right corner snapped to the text frame's top-right edge. Chosen
/// because the resulting placement is fully determined by the body
/// frame's geometry (no dependence on the still-approximated per-anchor
/// advance), so the renderer emission test can assert an exact page-
/// local position:
///   ref_x = body frame right edge, RightAlign ⇒ frame right on it;
///   ref_y = body frame top edge,   TopAlign   ⇒ frame top on it;
///   TopRightAnchor ⇒ frame_left = ref_right - frame_w, frame_top = ref_top.
fn custom_textframe_topright() -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchored_position: "Anchored",
        spine_relative: false,
        lock_position: false,
        pin_position: true,
        anchor_point: Some("TopRightAnchor"),
        horizontal_reference_point: Some("TextFrame"),
        vertical_reference_point: Some("TextFrame"),
        horizontal_alignment: Some("RightAlign"),
        vertical_alignment: Some("TopAlign"),
        anchor_x_offset: None,
        anchor_y_offset: None,
    }
}

fn lock_position() -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        lock_position: true,
        ..AnchoredObjectSetting::inline()
    }
}

fn spine_relative() -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        spine_relative: true,
        ..AnchoredObjectSetting::inline()
    }
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "anchored · inline · in-line-with-text",
            setting_factory: inline,
            with_wrap: false,
        },
        Variant {
            name: "anchored · above-line · custom",
            setting_factory: above_line,
            with_wrap: false,
        },
        Variant {
            name: "anchored · custom-x-y · 24-12pt-offset",
            setting_factory: custom_offset,
            with_wrap: false,
        },
        Variant {
            name: "anchored · custom · textframe-top-right",
            setting_factory: custom_textframe_topright,
            with_wrap: false,
        },
        Variant {
            name: "anchored · prevent-manual-positioning",
            setting_factory: lock_position,
            with_wrap: false,
        },
        Variant {
            name: "anchored · spine-relative",
            setting_factory: spine_relative,
            with_wrap: false,
        },
        Variant {
            name: "anchored · with-text-wrap",
            setting_factory: inline,
            with_wrap: true,
        },
    ]
}

/// Build the host story for one page: two leading paragraphs of
/// Lorem-ish text, a paragraph in which the anchor sits, and a
/// trailing paragraph so the anchor isn't at the very end of the
/// story (a common parser-edge bug).
fn host_paragraphs(anchor_frame: Rect) -> Vec<Paragraph> {
    let p1 = Paragraph::plain(
        "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod \
         tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, \
         quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo \
         consequat.",
    );
    // The host paragraph carries three runs: leading text, the
    // anchor (one-character placeholder), and trailing text. Splitting
    // across runs keeps the anchored frame inside its own
    // CharacterStyleRange — the IDML shape parsers expect.
    let host = Paragraph {
        justification: None,
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
        runs: vec![
            Run {
                text: "Duis aute irure dolor in reprehenderit ".to_string(),
                point_size: None,
                fill_color: None,
                font_style: None,
                tracking: None,
                baseline_shift: None,
                underline: None,
                applied_font: None,
                anchored_frame: None,
            },
            Run {
                // U+FEFF (BOM-ZWNBSP) is what real InDesign emits as
                // the anchor placeholder. Plain space works for the
                // parser; we use a non-breaking space so the placeholder
                // doesn't accidentally collapse.
                text: "\u{00A0}".to_string(),
                point_size: None,
                fill_color: None,
                font_style: None,
                tracking: None,
                baseline_shift: None,
                underline: None,
                applied_font: None,
                anchored_frame: Some(anchor_frame),
            },
            Run {
                text: " in voluptate velit esse cillum dolore eu fugiat nulla pariatur.".to_string(),
                point_size: None,
                fill_color: None,
                font_style: None,
                tracking: None,
                baseline_shift: None,
                underline: None,
                applied_font: None,
                anchored_frame: None,
            },
        ],
        table: None,
        minimum_letter_spacing: None,
        desired_letter_spacing: None,
        maximum_letter_spacing: None,
    };
    let p3 = Paragraph::plain(
        "Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia \
         deserunt mollit anim id est laborum.",
    );
    vec![p1, host, p3]
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
        let label_story_id = self_id(SAMPLE, "LabelStory", seq);
        let body_story_id = self_id(SAMPLE, "BodyStory", seq);
        let anchor_story_id = self_id(SAMPLE, "AnchorStory", seq);
        let label_frame_id = self_id(SAMPLE, "LabelFrame", seq);
        let body_frame_id = self_id(SAMPLE, "BodyFrame", seq);
        let anchor_frame_id = self_id(SAMPLE, "AnchorFrame", seq);

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

        // Label story.
        stories.push((
            label_story_id.clone(),
            write_story(&Story {
                self_id: label_story_id.clone(),
                paragraphs: vec![Paragraph::plain(variant.name)],
            }),
        ));
        story_refs.push(label_story_id.clone());

        // Anchor story — content of the inline frame.
        stories.push((
            anchor_story_id.clone(),
            write_story(&Story {
                self_id: anchor_story_id.clone(),
                paragraphs: vec![Paragraph::plain("ANCHOR")],
            }),
        ));
        story_refs.push(anchor_story_id.clone());

        // Anchor frame — the inline TextFrame that the anchor's
        // CharacterStyleRange will host. Its ItemTransform is
        // identity because IDML treats inline anchors as glyphs the
        // line-breaker positions; the page-coordinates come from
        // wherever the anchor lands.
        let anchor = Rect {
            self_id: anchor_frame_id,
            width_pt: ANCHOR_W_PT,
            height_pt: ANCHOR_H_PT,
            item_transform: IDENTITY,
            fill_color: Some("Color/Paper".to_string()),
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(0.5),
            parent_story: Some(anchor_story_id),
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: if variant.with_wrap {
                Some(TextWrap {
                    mode: "BoundingBoxTextWrap",
                    offsets: [3.0, 3.0, 3.0, 3.0],
                    side: Some("BothSides"),
                })
            } else {
                None
            },
            anchored_setting: Some((variant.setting_factory)()),
        };

        // Host body story — flows around the anchor inline.
        stories.push((
            body_story_id.clone(),
            write_story(&Story {
                self_id: body_story_id.clone(),
                paragraphs: host_paragraphs(anchor),
            }),
        ));
        story_refs.push(body_story_id.clone());

        let label = Rect {
            self_id: label_frame_id,
            width_pt: LABEL_W_PT,
            height_pt: LABEL_H_PT,
            item_transform: translate(36.0, 36.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(label_story_id),
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
        };

        let body = Rect {
            self_id: body_frame_id,
            width_pt: BODY_W_PT,
            height_pt: BODY_H_PT,
            item_transform: translate((PAGE_W_PT - BODY_W_PT) * 0.5, 80.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(body_story_id),
            extra_attrs: Vec::new(),
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
                page_name: variant.name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![label.into(), body.into()],
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

    // IDENTITY is referenced inline; suppress an unused warning if a
    // future variant drops the reference.
    let _: Matrix = IDENTITY;

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
