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
    spread::{write_spread, MarginPreference, Spread},
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
    /// When true, the host `<Page>` emits a `<MarginPreference>` so the
    /// `PageMargins` reference point resolves to the margin box instead
    /// of degenerating to the page edge. Only the page-margins variant
    /// sets this — the rest leave the page margin-less.
    with_margins: bool,
}

/// Margin box (pt) for the page-margins variant. Asymmetric on every
/// edge so the emission test can prove each margin is honoured
/// independently (and that the right/bottom inset, not the page edge,
/// drives the anchored placement).
const MARGIN_TOP_PT: f32 = 36.0;
const MARGIN_BOTTOM_PT: f32 = 48.0;
const MARGIN_LEFT_PT: f32 = 54.0;
const MARGIN_RIGHT_PT: f32 = 60.0;

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

/// Vertical reference against the anchor LINE's baseline, with the same
/// deterministic horizontal placement as [`custom_line_cap_height`]
/// (TextFrame + RightAlign + TopRightAnchor). The reference Y for the
/// cap-height / top-of-leading variants is measured relative to THIS
/// one, so they must share an identical horizontal anchor and the same
/// anchor line — only `vertical_reference_point` differs.
fn custom_line_baseline() -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchored_position: "Anchored",
        spine_relative: false,
        lock_position: false,
        pin_position: true,
        anchor_point: Some("TopRightAnchor"),
        horizontal_reference_point: Some("TextFrame"),
        vertical_reference_point: Some("LineBaseline"),
        horizontal_alignment: Some("RightAlign"),
        vertical_alignment: Some("TopAlign"),
        anchor_x_offset: None,
        anchor_y_offset: None,
    }
}

/// Like [`custom_line_baseline`] but anchors the frame's vertical
/// position against the anchor LINE's cap-height instead of the
/// baseline. Horizontal placement is unchanged (TextFrame + RightAlign +
/// TopRightAnchor ⇒ deterministic x), so the renderer emission test can
/// isolate the vertical reference: the frame's top lands
/// `cap_height · point_size` above the anchor line's baseline.
fn custom_line_cap_height() -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchored_position: "Anchored",
        spine_relative: false,
        lock_position: false,
        pin_position: true,
        anchor_point: Some("TopRightAnchor"),
        horizontal_reference_point: Some("TextFrame"),
        vertical_reference_point: Some("LineCapHeight"),
        horizontal_alignment: Some("RightAlign"),
        vertical_alignment: Some("TopAlign"),
        anchor_x_offset: None,
        anchor_y_offset: None,
    }
}

/// Vertical reference against the anchor line's leading-top
/// (`TopOfLeading`); same deterministic horizontal placement as
/// [`custom_line_cap_height`]. The frame's top lands at the top of the
/// line's leading slug — `leading · ascent/(ascent+descent)` above the
/// baseline.
fn custom_line_top_of_leading() -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchored_position: "Anchored",
        spine_relative: false,
        lock_position: false,
        pin_position: true,
        anchor_point: Some("TopRightAnchor"),
        horizontal_reference_point: Some("TextFrame"),
        vertical_reference_point: Some("TopOfLeading"),
        horizontal_alignment: Some("RightAlign"),
        vertical_alignment: Some("TopAlign"),
        anchor_x_offset: None,
        anchor_y_offset: None,
    }
}

/// Custom positioning against the page MARGIN box, bottom-right corner.
/// Exercises the W1.16 margin wire-up: with `<MarginPreference>` parsed,
/// `PageMargins` resolves to the margin rectangle, NOT the page edge. The
/// frame's bottom-right corner snaps to the margin box's bottom-right:
///   ref_x = page_width - margin_right (RightAlign);
///   ref_y = page_height - margin_bottom (BottomAlign);
///   BottomRightAnchor ⇒ frame_left = ref_x - frame_w, frame_top = ref_y - frame_h.
fn custom_page_margins_bottom_right() -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchored_position: "Anchored",
        spine_relative: false,
        lock_position: false,
        pin_position: true,
        anchor_point: Some("BottomRightAnchor"),
        horizontal_reference_point: Some("PageMargins"),
        vertical_reference_point: Some("PageMargins"),
        horizontal_alignment: Some("RightAlign"),
        vertical_alignment: Some("BottomAlign"),
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
            with_margins: false,
        },
        Variant {
            name: "anchored · above-line · custom",
            setting_factory: above_line,
            with_wrap: false,
            with_margins: false,
        },
        Variant {
            name: "anchored · custom-x-y · 24-12pt-offset",
            setting_factory: custom_offset,
            with_wrap: false,
            with_margins: false,
        },
        Variant {
            name: "anchored · custom · textframe-top-right",
            setting_factory: custom_textframe_topright,
            with_wrap: false,
            with_margins: false,
        },
        Variant {
            name: "anchored · prevent-manual-positioning",
            setting_factory: lock_position,
            with_wrap: false,
            with_margins: false,
        },
        Variant {
            name: "anchored · spine-relative",
            setting_factory: spine_relative,
            with_wrap: false,
            with_margins: false,
        },
        Variant {
            name: "anchored · with-text-wrap",
            setting_factory: inline,
            with_wrap: true,
            with_margins: false,
        },
        // W1.16 — deterministic vertical-reference-point variants. The
        // first three share identical horizontal placement (TextFrame +
        // RightAlign + TopRightAnchor) and the SAME anchor line, so the
        // emission test isolates the vertical reference: the Y delta
        // between them equals exactly the font-metric distance
        // (baseline → cap-height, baseline → top-of-leading).
        Variant {
            name: "anchored · custom · line-baseline",
            setting_factory: custom_line_baseline,
            with_wrap: false,
            with_margins: false,
        },
        Variant {
            name: "anchored · custom · line-cap-height",
            setting_factory: custom_line_cap_height,
            with_wrap: false,
            with_margins: false,
        },
        Variant {
            name: "anchored · custom · line-top-of-leading",
            setting_factory: custom_line_top_of_leading,
            with_wrap: false,
            with_margins: false,
        },
        // W1.16 — PageMargins reference. This page DOES declare margins,
        // so the anchored frame snaps to the margin box's bottom-right,
        // proving the placement diverges from the page edge.
        Variant {
            name: "anchored · custom · page-margins-bottom-right",
            setting_factory: custom_page_margins_bottom_right,
            with_wrap: false,
            with_margins: true,
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
        applied_numbering_list: None,
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
                text: " in voluptate velit esse cillum dolore eu fugiat nulla pariatur."
                    .to_string(),
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
                page_items: Vec::new(),
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
            next_text_frame: None,
            previous_text_frame: None,
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
            frame_effects: Vec::new(),
            text_frame_pref: None,
            custom_subpaths: None,
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

        let body = Rect {
            self_id: body_frame_id,
            width_pt: BODY_W_PT,
            height_pt: BODY_H_PT,
            item_transform: translate((PAGE_W_PT - BODY_W_PT) * 0.5, 80.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(body_story_id),
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
                override_list: Vec::new(),
                margins: if variant.with_margins {
                    Some(MarginPreference {
                        top: MARGIN_TOP_PT,
                        bottom: MARGIN_BOTTOM_PT,
                        left: MARGIN_LEFT_PT,
                        right: MARGIN_RIGHT_PT,
                    })
                } else {
                    None
                },
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
