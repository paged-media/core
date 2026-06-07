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
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml,
        styles_xml_with_paragraph_styles, ExtraColor, ParagraphStyleSpec,
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

/// W2.1 — a named, visually-distinct paragraph style emitted inside the
/// canonical `<RootParagraphStyleGroup>`. The editor's `applyStyle`
/// render-contrast fixme (AC-E2E-TEXT-3) attributes this style to a body
/// range and proves the swap repaints: it contrasts the document default
/// ([No paragraph style]: 12pt black) on TWO axes — 28pt (vs 12) and RGB
/// cyan fill (vs black) — so any reasonable diff threshold trips. It is
/// the LAST paragraph style in the collection (what
/// `lastCollectionId("paragraphStyles")` resolves to). Only round-
/// trippable axes are used (no `Justification`, which `paged-write`
/// drops on its reader→writer rewrite — see `ParagraphStyleSpec`).
pub const EMPHASIS_STYLE_ID: &str = "ParagraphStyle/EmphasisDisplay";
const EMPHASIS_STYLE_NAME: &str = "Emphasis Display";

fn emphasis_style() -> ParagraphStyleSpec {
    ParagraphStyleSpec {
        self_id: EMPHASIS_STYLE_ID,
        name: EMPHASIS_STYLE_NAME,
        applied_font: "Open Sans",
        point_size: 28.0,
        fill_color: "Color/RGBCyan",
    }
}

struct Variant {
    name: &'static str,
    /// One or more paragraphs to lay into the body text frame.
    paragraphs: Vec<Paragraph>,
}

/// Body-frame width (pt) for a variant. Most use the default 480pt
/// column; the hyphenation variant pins a narrow 130pt column so a
/// long word can't fit on one line and the composer must hyphenate.
/// Keyed off the page name so existing literals stay untouched.
fn frame_width_for(name: &str) -> f32 {
    if name.contains("hyphenation") {
        130.0
    } else {
        FRAME_W_PT
    }
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
        // ── W2.1 typography hosts (editor op-suite content) ──────────
        // These pages append AFTER the original 14 so the existing
        // page-0-targeting specs (character/paragraph ops on the first
        // story) stay byte-stable. Each carries content engineered so a
        // single character/paragraph EDIT produces a render delta the
        // minimal pangram couldn't.
        //
        // Kern pairs: AV / To / Wa / Ye carry large negative kern values
        // in any kerned face (Inter included), so flipping
        // `characterKerningMethod` to None shifts the glyphs → delta.
        Variant {
            name: "text · kern · pairs",
            paragraphs: vec![one_run("AVATAR To Wave Yes Tom", 32.0, None, None)],
        },
        // Superscript digit: a footnote-style "E = mc2" where the editor
        // raises the trailing digit. `characterPosition=Superscript`
        // lifts + shrinks the selected glyphs → delta on any content,
        // but a digit reads as a real superscript use.
        Variant {
            name: "text · superscript · digit",
            paragraphs: vec![one_run("E = mc2 and 1st 2nd 3rd", 28.0, None, None)],
        },
        // Hyphenation: a single very long word in a NARROW column
        // (130pt via `frame_width_for`) that cannot fit on one line, so
        // enabling `paragraphHyphenation` forces a hyphen break →
        // reflow delta. The word is a real dictionary-hyphenable term.
        Variant {
            name: "text · wrap · hyphenation",
            paragraphs: vec![one_run(
                "antidisestablishmentarianism incomprehensibilities",
                18.0,
                None,
                None,
            )],
        },
        // Multi-paragraph with a trailing-space last line: two stacked
        // paragraphs so a `paragraphRuleBelow` on the first draws a rule
        // in the gap above the second (the trailing-space case the
        // single-paragraph fixmes lacked). SpaceAfter opens the gap.
        Variant {
            name: "text · para · multi-trailing",
            paragraphs: vec![
                Paragraph {
                    space_after: Some(18.0),
                    ..one_run("First paragraph with a rule below it. ", 16.0, None, None)
                },
                one_run("Second paragraph follows the rule.", 16.0, None, None),
            ],
        },
        // Standard-ligature content: an "fi / ffi / fl" cluster. Inter
        // ships no `liga` table, so the EDITOR spec loads this fixture
        // with a liga-bearing fallback (Cormorant) to make the toggle
        // visible; the content is the fixture's half.
        Variant {
            name: "text · liga · fi-ffi",
            paragraphs: vec![one_run(
                "office affix fluffier final firefly",
                40.0,
                None,
                None,
            )],
        },
        // A second font family mid-story: a run pinned to "Open Sans"
        // beside the default family, so a family-aware edit (or the
        // FONTS panel) sees two families. The harness registers Open
        // Sans for this fixture.
        Variant {
            name: "text · family · second",
            paragraphs: vec![Paragraph {
                runs: vec![
                    Run {
                        text: "Default ".to_string(),
                        point_size: Some(24.0),
                        ..plain_run()
                    },
                    Run {
                        text: "OpenSans".to_string(),
                        point_size: Some(24.0),
                        applied_font: Some("Open Sans"),
                        ..plain_run()
                    },
                ],
                ..one_run("", 24.0, None, None)
            }],
        },
    ]
}

/// A `Run` with every optional field cleared — the spread base for the
/// W2.1 multi-run variants.
fn plain_run() -> Run {
    Run {
        text: String::new(),
        point_size: None,
        fill_color: None,
        font_style: None,
        tracking: None,
        baseline_shift: None,
        underline: None,
        applied_font: None,
        anchored_frame: None,
    }
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
        // descriptor + body line up legibly. The hyphenation variant
        // pins a narrow column (see `frame_width_for`).
        let frame_w = frame_width_for(variant.name);
        let frame_transform: Matrix =
            translate((PAGE_W_PT - frame_w) * 0.5, (PAGE_H_PT - FRAME_H_PT) * 0.33);
        // `translate` returns a row-major identity with tx/ty filled,
        // matching the geometry sample's helper but explicit here so
        // future variant transforms have a hook.
        let _: Matrix = IDENTITY;
        let body_frame = Rect {
            self_id: frame_id,
            width_pt: frame_w,
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
                item_transform: None,
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
        // The contrasting "Emphasis Display" paragraph style (W2.1) so
        // the editor's applyStyle render-contrast fixme has a visually-
        // distinct named style to attribute. Emitted in the canonical
        // group so the fixture round-trips byte-identically.
        styles_xml: styles_xml_with_paragraph_styles(&[emphasis_style()]),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}
