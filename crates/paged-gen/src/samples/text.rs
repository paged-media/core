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

//! Phase-1 mega-file: `text.idml`.
//!
//! Pages exercise text rendering paths the strokes-fills sample
//! intentionally left untouched:
//!   * point size (12pt baseline, 24pt large)
//!   * paragraph alignment (center, right)
//!   * run fill colour (built-in CMYK Black + custom RGB swatch)
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
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "text";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 480.0;
const FRAME_H_PT: f32 = 400.0;

struct Variant {
    name: &'static str,
    /// One or more paragraphs to lay into the body text frame.
    paragraphs: Vec<Paragraph>,
}

fn variants() -> Vec<Variant> {
    let lorem = "The quick brown fox jumps over the lazy dog. \
        Pack my box with five dozen liquor jugs. \
        Sphinx of black quartz, judge my vow. \
        How vexingly quick daft zebras jump!";
    vec![
        Variant {
            name: "text · size · 12pt",
            paragraphs: vec![one_run(lorem, 12.0, None, None)],
        },
        Variant {
            name: "text · size · 24pt",
            paragraphs: vec![one_run("Twenty-four point body copy", 24.0, None, None)],
        },
        Variant {
            name: "text · align · center",
            paragraphs: vec![one_run(
                "Centred body copy across one paragraph",
                14.0,
                None,
                Some("CenterAlign"),
            )],
        },
        Variant {
            name: "text · align · right",
            paragraphs: vec![one_run(
                "Right-aligned body copy across one paragraph",
                14.0,
                None,
                Some("RightAlign"),
            )],
        },
        Variant {
            name: "text · color · cyan",
            paragraphs: vec![one_run(
                "Cyan run on white frame",
                18.0,
                Some("Color/RGBCyan"),
                None,
            )],
        },
        // Wrapping: a single long paragraph that must break across
        // multiple lines inside the 480pt-wide frame. Knuth-Plass
        // line-breaking will choose the break points.
        Variant {
            name: "text · wrap · 4-lines",
            paragraphs: vec![one_run(lorem, 14.0, None, None)],
        },
        // Leading: explicit 24pt leading on a 12pt run. The renderer
        // should space the lines twice the body height instead of the
        // default 1.2× auto leading.
        Variant {
            name: "text · leading · 24pt-on-12pt",
            paragraphs: vec![Paragraph {
                justification: None,
                space_before: None,
                space_after: None,
                leading: Some(24.0),
                first_line_indent: None,
                left_indent: None,
                right_indent: None,
                drop_cap_characters: None,
                drop_cap_lines: None,
                tab_list: Vec::new(),
                bullets_list_type: None,
                applied_numbering_list: None,
                bullet_character: None,
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
                runs: vec![Run {
                    text: lorem.to_string(),
                    point_size: Some(12.0),
                    fill_color: None,
                    font_style: None,
                    tracking: None,
                    baseline_shift: None,
                    underline: None,
                    applied_font: None,
                    anchored_frame: None,
                }],
            }],
        },
        // Multi-paragraph + SpaceAfter: three short paragraphs with
        // 18pt of separation. Verifies the renderer honours
        // per-paragraph vertical spacing.
        Variant {
            name: "text · paragraphs · space-after-18",
            paragraphs: (0..3)
                .map(|i| Paragraph {
                    justification: None,
                    space_before: None,
                    space_after: Some(18.0),
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
                    table: None,
                    minimum_letter_spacing: None,
                    desired_letter_spacing: None,
                    maximum_letter_spacing: None,
                    runs: vec![Run {
                        text: format!("Paragraph {} of three", i + 1),
                        point_size: Some(14.0),
                        fill_color: None,
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    }],
                })
                .collect(),
        },
        // Mixed runs: one paragraph with three runs of distinct
        // colours. Verifies per-run paint application across a single
        // line of shaped text.
        Variant {
            name: "text · runs · mixed-color",
            paragraphs: vec![Paragraph {
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
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
                runs: vec![
                    Run {
                        text: "Black ".to_string(),
                        point_size: Some(18.0),
                        fill_color: Some("Color/Black".to_string()),
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    },
                    Run {
                        text: "cyan ".to_string(),
                        point_size: Some(18.0),
                        fill_color: Some("Color/RGBCyan".to_string()),
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    },
                    Run {
                        text: "again black".to_string(),
                        point_size: Some(18.0),
                        fill_color: Some("Color/Black".to_string()),
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    },
                ],
            }],
        },
        // Tracking: 200/1000 em (heavily letter-spaced).
        Variant {
            name: "text · tracking · 200",
            paragraphs: vec![Paragraph {
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
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
                runs: vec![Run {
                    text: "wide-tracked headline".to_string(),
                    point_size: Some(18.0),
                    fill_color: None,
                    font_style: None,
                    tracking: Some(200.0),
                    baseline_shift: None,
                    underline: None,
                    applied_font: None,
                    anchored_frame: None,
                }],
            }],
        },
        // Underline.
        Variant {
            name: "text · underline · single",
            paragraphs: vec![Paragraph {
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
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
                runs: vec![Run {
                    text: "underlined run".to_string(),
                    point_size: Some(18.0),
                    fill_color: None,
                    font_style: None,
                    tracking: None,
                    baseline_shift: None,
                    underline: Some(true),
                    applied_font: None,
                    anchored_frame: None,
                }],
            }],
        },
        // Baseline shift: a small "²" lifted 6pt above the line.
        Variant {
            name: "text · baseline-shift · superscript",
            paragraphs: vec![Paragraph {
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
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
                runs: vec![
                    Run {
                        text: "x".to_string(),
                        point_size: Some(20.0),
                        fill_color: None,
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    },
                    Run {
                        text: "2".to_string(),
                        point_size: Some(12.0),
                        fill_color: None,
                        font_style: None,
                        tracking: None,
                        baseline_shift: Some(6.0),
                        underline: None,
                        applied_font: None,
                        anchored_frame: None,
                    },
                ],
            }],
        },
        // Italic font face — references Open Sans Italic via the
        // `Open Sans/Italic` family path. The diff harness's font
        // substitution rules already map this to the bundled italic
        // TTF.
        Variant {
            name: "text · italic · open-sans",
            paragraphs: vec![Paragraph {
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
                table: None,
                minimum_letter_spacing: None,
                desired_letter_spacing: None,
                maximum_letter_spacing: None,
                runs: vec![Run {
                    text: "italic Open Sans run".to_string(),
                    point_size: Some(18.0),
                    fill_color: None,
                    font_style: Some("Italic"),
                    tracking: None,
                    baseline_shift: None,
                    underline: None,
                    applied_font: Some("Open Sans"),
                    anchored_frame: None,
                }],
            }],
        },
    ]
}

/// One-paragraph, one-run convenience.
fn one_run(
    text: &str,
    point_size: f32,
    fill_color: Option<&str>,
    justification: Option<&'static str>,
) -> Paragraph {
    Paragraph {
        justification,
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
        table: None,
        minimum_letter_spacing: None,
        desired_letter_spacing: None,
        maximum_letter_spacing: None,
        runs: vec![Run {
            text: text.to_string(),
            point_size: Some(point_size),
            fill_color: fill_color.map(str::to_string),
            font_style: None,
            tracking: None,
            baseline_shift: None,
            underline: None,
            applied_font: None,
            anchored_frame: None,
        }],
    }
}

fn extra_colors() -> Vec<ExtraColor> {
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

        stories.push((
            story_id.clone(),
            write_story(&Story {
                self_id: story_id.clone(),
                paragraphs: variant
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
                            .map(|s| crate::builders::story::TabStop {
                                position_pt: s.position_pt,
                                alignment: s.alignment,
                                leader: s.leader.clone(),
                            })
                            .collect(),
                        bullets_list_type: p.bullets_list_type,
                        applied_numbering_list: p.applied_numbering_list,
                        bullet_character: p.bullet_character,
                        table: None,
                        minimum_letter_spacing: None,
                        desired_letter_spacing: None,
                        maximum_letter_spacing: None,
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
                    })
                    .collect(),
            }),
        ));
        story_refs.push(story_id.clone());

        // One body text frame, large enough to fit the run. Centred
        // horizontally on the page; positioned ~1/3 down so the
        // descriptor + body line up legibly.
        let frame_transform: Matrix = translate(
            (PAGE_W_PT - FRAME_W_PT) * 0.5,
            (PAGE_H_PT - FRAME_H_PT) * 0.33,
        );
        // `translate` returns a row-major identity with tx/ty filled,
        // matching the geometry sample's helper but explicit here so
        // future variant transforms have a hook.
        let _: Matrix = IDENTITY;
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
                page_items: vec![body_frame.into()],
                override_list: Vec::new(),
                margins: None,
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
