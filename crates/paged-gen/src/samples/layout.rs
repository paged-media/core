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

//! W4.10 mega-file: `layout.idml` — page-geometry features.
//!
//! Six A4 pages, each a single layout concern so per-page heatmaps stay
//! self-describing (the `layout · variant · detail` `Page.Name`
//! convention):
//!
//!   1. `layout · margins-columns · asymmetric-3col` — a page whose
//!      `<MarginPreference>` has asymmetric edges and a 3-column grid
//!      with a custom 18pt gutter, plus ruler `<Guide>`s pinned at the
//!      margin box's column boundaries. Proves the column grid +
//!      asymmetric margins + guides all parse off one page.
//!   2. `layout · text-columns · 2col-custom-gutter` — a body text frame
//!      whose `<TextFramePreference>` declares `TextColumnCount=2` and a
//!      custom 24pt `TextColumnGutter`. (The composer's per-column
//!      layout is a later wave; the fixture exercises the parse + frame
//!      wiring so the conformance corpus carries a multi-column frame.)
//!   3. `layout · autosize · height-only-grow` — the W1.7 Phase A+B
//!      downward-grow case (`AutoSizingType=HeightOnly`,
//!      `TopLeftPoint`): an undersized headline box the renderer grows
//!      downward to fit its lines.
//!   4. `layout · autosize · center-grow` — the W1.7 "visible box"
//!      behaviour: `HeightAndWidth` + `CenterPoint` reference, so the
//!      grown box expands symmetrically about its centre rather than
//!      down-and-right. A second, identically-authored TopLeft box sits
//!      alongside so the renderer test can prove the centre box's top
//!      rises ABOVE the TopLeft box's (the distinctive visible-box
//!      effect).
//!   5. `layout · spread-transform · rotate-15` — the body `<Spread>`
//!      carries a 15° `ItemTransform` rotation (W1.9). Its single rect's
//!      emitted fill transform must pick up the rotation.
//!   6. `layout · spread-transform · scale-1p25` — the body `<Spread>`
//!      carries a 1.25× uniform scale `ItemTransform` (W1.9).
//!
//! Body text pins `AppliedFont="Inter"` so the autosize grow is
//! deterministic against `corpus/fonts/Inter.ttf`. Fixtures are
//! gitignored — regenerate with `cargo run -p paged-gen -- emit layout
//! <out_dir>` before consuming the pipeline tests.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Rect, TextFramePref},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml, ExtraColor,
    },
    spread::{write_spread, MarginPreference, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{compose, rotate_deg, scale, translate};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "layout";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;

const BODY_FONT: &str = "Inter";

/// Asymmetric margin box for variant 1. Distinct on every edge so a
/// round-trip test can prove each margin is honoured independently, and
/// the column grid divides the *content* width (page − left − right).
const MARGIN_TOP: f32 = 48.0;
const MARGIN_BOTTOM: f32 = 64.0;
const MARGIN_LEFT: f32 = 54.0;
const MARGIN_RIGHT: f32 = 36.0;
const COLUMN_COUNT: u32 = 3;
const COLUMN_GUTTER: f32 = 18.0;

/// One body paragraph pinned to Inter at `point_size`.
fn inter_paragraph(text: &str, point_size: f32) -> Paragraph {
    Paragraph {
        extra_paragraph_attrs: Vec::new(),
        runs: vec![Run {
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
        ..Paragraph::plain("")
    }
}

/// Build the full `Sample` ready for `write_idml`.
pub fn build() -> Sample {
    let names: Vec<&'static str> = vec![
        "layout · margins-columns · asymmetric-3col",
        "layout · text-columns · 2col-custom-gutter",
        "layout · autosize · height-only-grow",
        "layout · autosize · center-grow",
        "layout · spread-transform · rotate-15",
        "layout · spread-transform · scale-1p25",
    ];

    let mut master_spreads = Vec::with_capacity(names.len());
    let mut spreads = Vec::with_capacity(names.len());
    let mut stories = Vec::with_capacity(names.len());
    let mut master_refs = Vec::with_capacity(names.len());
    let mut spread_refs = Vec::with_capacity(names.len());
    let mut story_refs = Vec::with_capacity(names.len());

    for (i, name) in names.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let label_story_id = self_id(SAMPLE, "LabelStory", seq);
        let body_story_id = self_id(SAMPLE, "BodyStory", seq);
        let label_frame_id = self_id(SAMPLE, "LabelFrame", seq);
        let body_frame_id = self_id(SAMPLE, "BodyFrame", seq);
        let alt_frame_id = self_id(SAMPLE, "AltFrame", seq);
        let alt_story_id = self_id(SAMPLE, "AltStory", seq);

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: master_page_id,
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: Vec::new(),
            }),
        ));
        master_refs.push(master_id.clone());

        // Per-page label frame, top-left.
        stories.push((
            label_story_id.clone(),
            write_story(&Story {
                self_id: label_story_id.clone(),
                paragraphs: vec![Paragraph::plain(*name)],
            }),
        ));
        story_refs.push(label_story_id.clone());
        let label = Rect {
            self_id: label_frame_id,
            width_pt: 460.0,
            height_pt: 24.0,
            item_transform: translate(36.0, 12.0),
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

        let mut page_items: Vec<PageItem> = vec![label.into()];
        let mut margins: Option<MarginPreference> = None;
        let mut spread_xform: Option<[f32; 6]> = None;

        match i {
            // 1. Asymmetric margins + 3-column grid + boundary guides.
            //    The body story / frame is a single text column spanning
            //    the margin box so the page isn't blank; the column grid
            //    + guides are the feature under test.
            0 => {
                margins = Some(MarginPreference {
                    top: MARGIN_TOP,
                    bottom: MARGIN_BOTTOM,
                    left: MARGIN_LEFT,
                    right: MARGIN_RIGHT,
                    column_count: COLUMN_COUNT,
                    column_gutter: COLUMN_GUTTER,
                });
                let content_w = PAGE_W_PT - MARGIN_LEFT - MARGIN_RIGHT;
                let content_h = PAGE_H_PT - MARGIN_TOP - MARGIN_BOTTOM;
                stories.push((
                    body_story_id.clone(),
                    write_story(&Story {
                        self_id: body_story_id.clone(),
                        paragraphs: vec![inter_paragraph(BODY_FILLER, 9.0)],
                    }),
                ));
                story_refs.push(body_story_id.clone());
                page_items.push(
                    Rect {
                        self_id: body_frame_id,
                        width_pt: content_w,
                        height_pt: content_h,
                        item_transform: translate(MARGIN_LEFT, MARGIN_TOP),
                        fill_color: None,
                        stroke_color: Some("Color/RGBCyan".to_string()),
                        stroke_weight_pt: Some(0.5),
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
                    }
                    .into(),
                );
            }
            // 2. Two-column text frame with a custom 24pt gutter.
            1 => {
                stories.push((
                    body_story_id.clone(),
                    write_story(&Story {
                        self_id: body_story_id.clone(),
                        paragraphs: vec![inter_paragraph(BODY_FILLER, 10.0)],
                    }),
                ));
                story_refs.push(body_story_id.clone());
                page_items.push(
                    Rect {
                        self_id: body_frame_id,
                        width_pt: 460.0,
                        height_pt: 600.0,
                        item_transform: translate(67.0, 80.0),
                        fill_color: None,
                        stroke_color: Some("Color/RGBCyan".to_string()),
                        stroke_weight_pt: Some(0.5),
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
                        text_frame_pref: Some(TextFramePref {
                            text_column_count: Some(2),
                            text_column_gutter: Some(24.0),
                            ..Default::default()
                        }),
                        custom_subpaths: None,
                    }
                    .into(),
                );
            }
            // 3. AutoSize HeightOnly + TopLeftPoint — grow downward.
            2 => {
                stories.push((
                    body_story_id.clone(),
                    write_story(&Story {
                        self_id: body_story_id.clone(),
                        paragraphs: (0..10)
                            .map(|n| inter_paragraph(&format!("Headline line {n}"), 12.0))
                            .collect(),
                    }),
                ));
                story_refs.push(body_story_id.clone());
                page_items.push(autosize_box(
                    body_frame_id,
                    body_story_id.clone(),
                    translate(60.0, 80.0),
                    "HeightOnly",
                    "TopLeftPoint",
                ));
            }
            // 4. AutoSize HeightAndWidth + CenterPoint — visible-box: the
            //    grown box expands symmetrically about its centre. An
            //    identically-authored TopLeft box alongside is the
            //    control: the centre box's top rises above the control's.
            3 => {
                let grow_lines = || -> Vec<Paragraph> {
                    (0..8)
                        .map(|n| inter_paragraph(&format!("Centre grow {n}"), 12.0))
                        .collect()
                };
                stories.push((
                    body_story_id.clone(),
                    write_story(&Story {
                        self_id: body_story_id.clone(),
                        paragraphs: grow_lines(),
                    }),
                ));
                story_refs.push(body_story_id.clone());
                stories.push((
                    alt_story_id.clone(),
                    write_story(&Story {
                        self_id: alt_story_id.clone(),
                        paragraphs: grow_lines(),
                    }),
                ));
                story_refs.push(alt_story_id.clone());
                // Both boxes share the SAME authored top (200) so the
                // grow-direction difference is the only variable.
                page_items.push(autosize_box(
                    body_frame_id,
                    body_story_id.clone(),
                    translate(80.0, 200.0),
                    "HeightAndWidth",
                    "CenterPoint",
                ));
                page_items.push(autosize_box(
                    alt_frame_id,
                    alt_story_id.clone(),
                    translate(330.0, 200.0),
                    "HeightAndWidth",
                    "TopLeftPoint",
                ));
            }
            // 5. Spread ItemTransform — 15° rotation. A single filled
            //    rect rides the rotation.
            4 => {
                spread_xform = Some(compose(
                    rotate_deg(15.0),
                    translate(PAGE_W_PT * 0.4, PAGE_H_PT * 0.4),
                ));
                page_items.push(filled_demo_rect(body_frame_id));
            }
            // 6. Spread ItemTransform — 1.25× uniform scale.
            5 => {
                spread_xform = Some(compose(
                    scale(1.25, 1.25),
                    translate(PAGE_W_PT * 0.3, PAGE_H_PT * 0.3),
                ));
                page_items.push(filled_demo_rect(body_frame_id));
            }
            _ => unreachable!(),
        }

        // Column-boundary guides for variant 1 only. Vertical guides at
        // each interior column edge of the margin box.
        let guides_xml = if i == 0 {
            column_boundary_guides()
        } else {
            String::new()
        };

        spreads.push((
            spread_id.clone(),
            write_spread_with_guides(
                &Spread {
                    self_id: spread_id.clone(),
                    page_self_id: page_id,
                    page_name: name.to_string(),
                    applied_master: format!("MasterSpread/{master_id}"),
                    page_width_pt: PAGE_W_PT,
                    page_height_pt: PAGE_H_PT,
                    page_items,
                    override_list: Vec::new(),
                    margins,
                    item_transform: spread_xform,
                },
                &guides_xml,
            ),
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

/// Several sentences of short words so a single-column body fills its
/// frame and a multi-column frame would have plenty to flow.
const BODY_FILLER: &str = "the quick brown fox jumps over a lazy dog and then the slow \
    red cat naps on a warm sunny windowsill all day long while the patient owl watches \
    the field below and the wind moves gently through the tall summer grass nearby";

/// An undersized, filled+stroked headline box that the renderer grows.
fn autosize_box(
    self_id: String,
    story_id: String,
    item_transform: crate::geometry::Matrix,
    auto_sizing_type: &'static str,
    reference_point: &'static str,
) -> PageItem {
    Rect {
        self_id,
        width_pt: 200.0,
        height_pt: 36.0,
        item_transform,
        fill_color: Some("Color/RGBCyan".to_string()),
        stroke_color: Some("Color/Black".to_string()),
        stroke_weight_pt: Some(1.0),
        parent_story: Some(story_id),
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: Some(TextFramePref {
            auto_sizing_type: Some(auto_sizing_type),
            auto_sizing_reference_point: Some(reference_point),
            ..Default::default()
        }),
        custom_subpaths: None,
    }
    .into()
}

/// A 120×80 magenta rect at the spread's local origin — the spread
/// transform is what moves/rotates/scales it onto the page.
fn filled_demo_rect(self_id: String) -> PageItem {
    Rect {
        self_id,
        width_pt: 120.0,
        height_pt: 80.0,
        item_transform: crate::geometry::IDENTITY,
        fill_color: Some("Color/RGBMagenta".to_string()),
        stroke_color: Some("Color/Black".to_string()),
        stroke_weight_pt: Some(0.5),
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
    .into()
}

/// Two vertical ruler guides at the interior column boundaries of the
/// asymmetric margin box (between column 0|1 and column 1|2). Page-local
/// x = margin_left + n·(column_w + gutter) for n in 1..count.
fn column_boundary_guides() -> String {
    let content_w = PAGE_W_PT - MARGIN_LEFT - MARGIN_RIGHT;
    let column_w = (content_w - (COLUMN_COUNT as f32 - 1.0) * COLUMN_GUTTER) / COLUMN_COUNT as f32;
    let mut out = String::new();
    for n in 1..COLUMN_COUNT {
        // Guide sits at the LEFT edge of column n (after the gutter).
        let x = MARGIN_LEFT + n as f32 * column_w + (n as f32 - 0.5) * COLUMN_GUTTER;
        out.push_str(&format!(
            r#"    <Guide Self="layout-guide-{n}" Orientation="Vertical" Location="{:.4}" PageIndex="0"/>
"#,
            x
        ));
    }
    out
}

/// Extra colours so the demo rects / strokes read distinctly.
fn extra_colors() -> Vec<ExtraColor> {
    vec![
        ExtraColor {
            self_id: "Color/RGBCyan".to_string(),
            name: "RGB Cyan".to_string(),
            space: "RGB",
            value: "0 200 220".to_string(),
        },
        ExtraColor {
            self_id: "Color/RGBMagenta".to_string(),
            name: "RGB Magenta".to_string(),
            space: "RGB",
            value: "220 0 160".to_string(),
        },
    ]
}

/// Emit a spread, then splice `guides_xml` (already-serialised `<Guide>`
/// elements) in just before the closing `</Spread>` so the page carries
/// ruler guides. `write_spread` doesn't model guides directly — they're
/// a non-export overlay — so we patch the serialised bytes. `guides_xml`
/// empty ⇒ the spread is returned verbatim.
fn write_spread_with_guides(spread: &Spread, guides_xml: &str) -> Vec<u8> {
    let bytes = write_spread(spread);
    if guides_xml.is_empty() {
        return bytes;
    }
    let text = String::from_utf8(bytes).expect("spread XML is valid UTF-8");
    // Insert before the LAST `</Spread>` close tag.
    let close = "</Spread>";
    let pos = text.rfind(close).expect("spread has a </Spread> close");
    let mut out = String::with_capacity(text.len() + guides_xml.len());
    out.push_str(&text[..pos]);
    out.push_str(guides_xml);
    out.push_str(&text[pos..]);
    out.into_bytes()
}
