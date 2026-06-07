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

//! Phase-1 mega-file: `text-advanced.idml`.
//!
//! Pages exercise paragraph-level features the basic `text.idml`
//! mega-file doesn't touch — drop caps, indents (positive and
//! hanging), left + right column-narrowing, tabbed columnar layout
//! with a dotted leader, and an explicitly justified paragraph.
//!
//! Each variant lives on its own A4 page with the variant descriptor
//! both as the `Page.Name` and as the visible body text — the diff
//! harness can attribute failure per page, and the rendered PDF
//! stays human-readable.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml, ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story, TabStop},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, Matrix};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "text-advanced";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 480.0;
const FRAME_H_PT: f32 = 500.0;

struct Variant {
    name: &'static str,
    /// One or more paragraphs to lay into the body text frame.
    paragraphs: Vec<Paragraph>,
}

fn variants() -> Vec<Variant> {
    let lorem_long = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, \
        sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. \
        Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris \
        nisi ut aliquip ex ea commodo consequat.";
    let drop_cap_body = "In a hole in the ground there lived a hobbit. \
        Not a nasty, dirty, wet hole, filled with the ends of worms and an \
        oozy smell, nor yet a dry, bare, sandy hole with nothing in it to \
        sit down on or to eat: it was a hobbit-hole, and that means comfort. \
        It had a perfectly round door like a porthole.";
    vec![
        // 1. Drop cap — first 2 characters drop across 3 lines.
        Variant {
            name: "text-adv · drop-cap · 2-chars-3-lines",
            paragraphs: vec![Paragraph {
                justification: None,
                space_before: None,
                space_after: None,
                leading: None,
                first_line_indent: None,
                left_indent: None,
                right_indent: None,
                drop_cap_characters: Some(2),
                drop_cap_lines: Some(3),
                tab_list: Vec::new(),
                bullets_list_type: None,
                bullet_character: None,
                runs: vec![Run {
                    text: drop_cap_body.to_string(),
                    point_size: Some(12.0),
                    fill_color: None,
                    font_style: None,
                    tracking: None,
                    baseline_shift: None,
                    underline: None,
                    applied_font: None,
                    anchored_frame: None,
                }],
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
            }],
        },
        // 2. First-line indent (positive) — first line shifts right.
        Variant {
            name: "text-adv · first-line-indent · 36pt",
            paragraphs: vec![Paragraph {
                justification: None,
                space_before: None,
                space_after: None,
                leading: None,
                first_line_indent: Some(36.0),
                left_indent: None,
                right_indent: None,
                drop_cap_characters: None,
                drop_cap_lines: None,
                tab_list: Vec::new(),
                bullets_list_type: None,
                bullet_character: None,
                runs: vec![Run {
                    text: lorem_long.to_string(),
                    point_size: Some(12.0),
                    fill_color: None,
                    font_style: None,
                    tracking: None,
                    baseline_shift: None,
                    underline: None,
                    applied_font: None,
                    anchored_frame: None,
                }],
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
            }],
        },
        // 3. Hanging indent — negative first-line indent paired with
        // a positive left indent (the classic dictionary / glossary
        // pattern).
        Variant {
            name: "text-adv · hanging-indent · -18+18",
            paragraphs: vec![Paragraph {
                justification: None,
                space_before: None,
                space_after: None,
                leading: None,
                first_line_indent: Some(-18.0),
                left_indent: Some(18.0),
                right_indent: None,
                drop_cap_characters: None,
                drop_cap_lines: None,
                tab_list: Vec::new(),
                bullets_list_type: None,
                bullet_character: None,
                runs: vec![Run {
                    text: lorem_long.to_string(),
                    point_size: Some(12.0),
                    fill_color: None,
                    font_style: None,
                    tracking: None,
                    baseline_shift: None,
                    underline: None,
                    applied_font: None,
                    anchored_frame: None,
                }],
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
            }],
        },
        // 4. Symmetric column narrowing — LeftIndent + RightIndent.
        Variant {
            name: "text-adv · indents · L36-R36",
            paragraphs: vec![Paragraph {
                justification: None,
                space_before: None,
                space_after: None,
                leading: None,
                first_line_indent: None,
                left_indent: Some(36.0),
                right_indent: Some(36.0),
                drop_cap_characters: None,
                drop_cap_lines: None,
                tab_list: Vec::new(),
                bullets_list_type: None,
                bullet_character: None,
                runs: vec![Run {
                    text: lorem_long.to_string(),
                    point_size: Some(12.0),
                    fill_color: None,
                    font_style: None,
                    tracking: None,
                    baseline_shift: None,
                    underline: None,
                    applied_font: None,
                    anchored_frame: None,
                }],
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
            }],
        },
        // 5. Tab stops with a dotted leader. Left-aligned label,
        // two centre-ish numeric columns, right-aligned price with
        // a dotted leader filling the gap. Tab characters in the
        // run text become <Tab/> at emit time.
        Variant {
            name: "text-adv · tabs · leaders-dotted",
            paragraphs: vec![
                Paragraph {
                    justification: None,
                    space_before: None,
                    space_after: Some(6.0),
                    leading: None,
                    first_line_indent: None,
                    left_indent: None,
                    right_indent: None,
                    drop_cap_characters: None,
                    drop_cap_lines: None,
                    tab_list: vec![
                        TabStop {
                            position_pt: 100.0,
                            alignment: "LeftAlign",
                            leader: None,
                        },
                        TabStop {
                            position_pt: 200.0,
                            alignment: "LeftAlign",
                            leader: None,
                        },
                        TabStop {
                            position_pt: 300.0,
                            alignment: "RightAlign",
                            leader: Some(".".to_string()),
                        },
                    ],
                    bullets_list_type: None,
                    bullet_character: None,
                    runs: vec![Run {
                        text: "Apples\t1.20\t10\t12.00".to_string(),
                        point_size: Some(12.0),
                        fill_color: None,
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    }],
                    table: None,
                    minimum_letter_spacing: None,
                    desired_letter_spacing: None,
                    maximum_letter_spacing: None,
                },
                Paragraph {
                    justification: None,
                    space_before: None,
                    space_after: Some(6.0),
                    leading: None,
                    first_line_indent: None,
                    left_indent: None,
                    right_indent: None,
                    drop_cap_characters: None,
                    drop_cap_lines: None,
                    tab_list: vec![
                        TabStop {
                            position_pt: 100.0,
                            alignment: "LeftAlign",
                            leader: None,
                        },
                        TabStop {
                            position_pt: 200.0,
                            alignment: "LeftAlign",
                            leader: None,
                        },
                        TabStop {
                            position_pt: 300.0,
                            alignment: "RightAlign",
                            leader: Some(".".to_string()),
                        },
                    ],
                    bullets_list_type: None,
                    bullet_character: None,
                    runs: vec![Run {
                        text: "Bread\t3.50\t2\t7.00".to_string(),
                        point_size: Some(12.0),
                        fill_color: None,
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    }],
                    table: None,
                    minimum_letter_spacing: None,
                    desired_letter_spacing: None,
                    maximum_letter_spacing: None,
                },
                Paragraph {
                    justification: None,
                    space_before: None,
                    space_after: None,
                    leading: None,
                    first_line_indent: None,
                    left_indent: None,
                    right_indent: None,
                    drop_cap_characters: None,
                    drop_cap_lines: None,
                    tab_list: vec![
                        TabStop {
                            position_pt: 100.0,
                            alignment: "LeftAlign",
                            leader: None,
                        },
                        TabStop {
                            position_pt: 200.0,
                            alignment: "LeftAlign",
                            leader: None,
                        },
                        TabStop {
                            position_pt: 300.0,
                            alignment: "RightAlign",
                            leader: Some(".".to_string()),
                        },
                    ],
                    bullets_list_type: None,
                    bullet_character: None,
                    runs: vec![Run {
                        text: "Cheese\t8.99\t1\t8.99".to_string(),
                        point_size: Some(12.0),
                        fill_color: None,
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    }],
                    table: None,
                    minimum_letter_spacing: None,
                    desired_letter_spacing: None,
                    maximum_letter_spacing: None,
                },
            ],
        },
        // 6a. BulletList — three paragraphs share the same list
        // type. Renderer prepends the renderer's default `•` glyph
        // (U+2022) per item since no inline override is set.
        Variant {
            name: "text-adv · bullets · default-bullet",
            paragraphs: (0..3)
                .map(|i| Paragraph {
                    justification: None,
                    space_before: None,
                    space_after: None,
                    leading: None,
                    first_line_indent: None,
                    left_indent: Some(18.0),
                    right_indent: None,
                    drop_cap_characters: None,
                    drop_cap_lines: None,
                    tab_list: Vec::new(),
                    bullets_list_type: Some("BulletList"),
                    bullet_character: None,
                    runs: vec![Run {
                        text: format!("Bulleted item {}", i + 1),
                        point_size: Some(12.0),
                        fill_color: None,
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    }],
                    table: None,
                    minimum_letter_spacing: None,
                    desired_letter_spacing: None,
                    maximum_letter_spacing: None,
                })
                .collect(),
        },
        // 6b. BulletList with inline `»` (U+00BB) override.
        Variant {
            name: "text-adv · bullets · arrow-override",
            paragraphs: (0..2)
                .map(|i| Paragraph {
                    justification: None,
                    space_before: None,
                    space_after: None,
                    leading: None,
                    first_line_indent: None,
                    left_indent: Some(18.0),
                    right_indent: None,
                    drop_cap_characters: None,
                    drop_cap_lines: None,
                    tab_list: Vec::new(),
                    bullets_list_type: Some("BulletList"),
                    bullet_character: Some(0x00BB),
                    runs: vec![Run {
                        text: format!("Arrow-bullet item {}", i + 1),
                        point_size: Some(12.0),
                        fill_color: None,
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    }],
                    table: None,
                    minimum_letter_spacing: None,
                    desired_letter_spacing: None,
                    maximum_letter_spacing: None,
                })
                .collect(),
        },
        // 6c. NumberedList — three items, default Arabic numerals.
        Variant {
            name: "text-adv · numbered · arabic",
            paragraphs: (0..3)
                .map(|i| Paragraph {
                    justification: None,
                    space_before: None,
                    space_after: None,
                    leading: None,
                    first_line_indent: None,
                    left_indent: Some(18.0),
                    right_indent: None,
                    drop_cap_characters: None,
                    drop_cap_lines: None,
                    tab_list: Vec::new(),
                    bullets_list_type: Some("NumberedList"),
                    bullet_character: None,
                    runs: vec![Run {
                        text: format!("Numbered item {}", i + 1),
                        point_size: Some(12.0),
                        fill_color: None,
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    }],
                    table: None,
                    minimum_letter_spacing: None,
                    desired_letter_spacing: None,
                    maximum_letter_spacing: None,
                })
                .collect(),
        },
        // 7. Justified paragraph — LeftJustified means each line fills
        // the column except the last, which left-aligns. The renderer
        // should stretch word-spacing to reach the right edge.
        Variant {
            name: "text-adv · justified · left-justified",
            paragraphs: vec![Paragraph {
                justification: Some("LeftJustified"),
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
                runs: vec![Run {
                    text: lorem_long.to_string(),
                    point_size: Some(12.0),
                    fill_color: None,
                    font_style: None,
                    tracking: None,
                    baseline_shift: None,
                    underline: None,
                    applied_font: None,
                    anchored_frame: None,
                }],
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
            }],
        },
    ]
}

fn extra_colors() -> Vec<ExtraColor> {
    // Reuse the same shared swatch the basic text sample registers
    // so any future per-run colour exercises work without rewiring
    // Graphic.xml.
    vec![ExtraColor {
        self_id: "Color/RGBCyan".to_string(),
        name: "RGB Cyan".to_string(),
        space: "RGB",
        value: "0 200 220".to_string(),
    }]
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
        let frame_id = self_id(SAMPLE, "TextFrame", seq);

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

        // Clone the variant's paragraphs into a fresh story. The
        // builder consumes the field-by-field shape so renaming /
        // adding paragraph attributes only needs one mirror site.
        let paragraphs = variant
            .paragraphs
            .iter()
            .map(|p| Paragraph {
                justification: p.justification,
                space_before: p.space_before,
                space_after: p.space_after,
                leading: p.leading,
                first_line_indent: p.first_line_indent,
                left_indent: p.left_indent,
                right_indent: p.right_indent,
                drop_cap_characters: p.drop_cap_characters,
                drop_cap_lines: p.drop_cap_lines,
                tab_list: p
                    .tab_list
                    .iter()
                    .map(|s| TabStop {
                        position_pt: s.position_pt,
                        alignment: s.alignment,
                        leader: s.leader.clone(),
                    })
                    .collect(),
                bullets_list_type: p.bullets_list_type,
                bullet_character: p.bullet_character,
                runs: p
                    .runs
                    .iter()
                    .map(|r| Run {
                        text: r.text.clone(),
                        point_size: r.point_size,
                        fill_color: r.fill_color.clone(),
                        font_style: r.font_style,
                        tracking: r.tracking,
                        baseline_shift: r.baseline_shift,
                        underline: r.underline,
                        applied_font: r.applied_font,
                        anchored_frame: None,
                    })
                    .collect(),
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
            })
            .collect();

        stories.push((
            story_id.clone(),
            write_story(&Story {
                self_id: story_id.clone(),
                paragraphs,
            }),
        ));
        story_refs.push(story_id.clone());

        // Body frame — same geometry as the basic text sample, a bit
        // taller so the multi-line wrapping variants (drop cap,
        // hanging indent, justified) have room to breathe.
        let frame_transform: Matrix = translate(
            (PAGE_W_PT - FRAME_W_PT) * 0.5,
            (PAGE_H_PT - FRAME_H_PT) * 0.25,
        );
        let body_frame = Rect {
            self_id: frame_id,
            width_pt: FRAME_W_PT,
            height_pt: FRAME_H_PT,
            item_transform: frame_transform,
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
                page_items: vec![body_frame.into()],
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
