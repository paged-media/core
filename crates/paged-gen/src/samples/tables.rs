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

//! Phase-1 mega-file: `tables.idml`.
//!
//! Pages exercise table rendering — the renderer's table cell
//! emission path that no prior generator sample touches. Each
//! variant lives on its own A4 page. The host TextFrame holds a
//! single paragraph whose CharacterStyleRange contains a `<Table>`.
//!
//! Variants:
//!   * 2×2 plain table — body content only
//!   * 3×3 with one header row
//!   * 3×3 with alternating row fills (per-cell, no table style)
//!   * 3×3 with per-cell stroke colour overrides
//!   * 3×3 with table-style-driven alternating ROW fills
//!   * 3×3 with table-style-driven alternating COLUMN fills
//!   * 2×2 with cell diagonals (TL→BR, TR→BL, and an in-front one)

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml,
        styles_xml_with_table_styles, ExtraColor, TableStyleSpec,
    },
    spread::{write_spread, Spread},
    story::{write_story, Cell, CellDiagonal, Paragraph, Run, Story, Table},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};

/// Table-style self-ids referenced by the style-driven variants. The
/// styles themselves are declared in [`table_styles`] and emitted into
/// the shared styles manifest.
const ALT_ROW_STYLE: &str = "TableStyle/AltRows";
const ALT_COL_STYLE: &str = "TableStyle/AltCols";
use crate::geometry::{translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "tables";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 480.0;
const FRAME_H_PT: f32 = 360.0;
const ROW_H_PT: f32 = 28.0;
const COL_W_PT: f32 = 120.0;

struct Variant {
    name: &'static str,
    /// Builder closure produces the Table given a Self id; lets each
    /// variant emit a different shape without modelling another data
    /// type per case.
    table: Box<dyn Fn(&str) -> Table>,
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "tables · 2x2 · plain",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: None,
                header_row_count: 0,
                footer_row_count: 0,
                body_row_count: 2,
                column_count: 2,
                row_heights_pt: vec![ROW_H_PT; 2],
                column_widths_pt: vec![COL_W_PT; 2],
                cells: vec![
                    Cell::plain("A1"),
                    Cell::plain("A2"),
                    Cell::plain("B1"),
                    Cell::plain("B2"),
                ],
            }),
        },
        Variant {
            name: "tables · 3x3 · header-row",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: None,
                header_row_count: 1,
                footer_row_count: 0,
                body_row_count: 2,
                column_count: 3,
                row_heights_pt: vec![ROW_H_PT; 3],
                column_widths_pt: vec![COL_W_PT; 3],
                // column-major: col 0 then col 1 then col 2
                cells: {
                    let mut v = Vec::new();
                    for c in 0..3 {
                        for r in 0..3 {
                            let label = if r == 0 {
                                format!("Hdr {}", c + 1)
                            } else {
                                format!("R{}C{}", r, c + 1)
                            };
                            let mut cell = Cell::plain(label);
                            if r == 0 {
                                cell.fill_color = Some("Color/CMYKCyan20".to_string());
                            }
                            v.push(cell);
                        }
                    }
                    v
                },
            }),
        },
        Variant {
            name: "tables · 3x3 · alternating-rows",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: None,
                header_row_count: 0,
                footer_row_count: 0,
                body_row_count: 3,
                column_count: 3,
                row_heights_pt: vec![ROW_H_PT; 3],
                column_widths_pt: vec![COL_W_PT; 3],
                cells: {
                    let mut v = Vec::new();
                    for c in 0..3 {
                        for r in 0..3 {
                            let mut cell = Cell::plain(format!("R{}C{}", r + 1, c + 1));
                            // Even rows (0, 2) get the alternating fill.
                            if r % 2 == 0 {
                                cell.fill_color = Some("Color/CMYKCyan20".to_string());
                            }
                            v.push(cell);
                        }
                    }
                    v
                },
            }),
        },
        Variant {
            name: "tables · 3x3 · cell-strokes",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: None,
                header_row_count: 0,
                footer_row_count: 0,
                body_row_count: 3,
                column_count: 3,
                row_heights_pt: vec![ROW_H_PT; 3],
                column_widths_pt: vec![COL_W_PT; 3],
                cells: {
                    let mut v = Vec::new();
                    for c in 0..3 {
                        for r in 0..3 {
                            let mut cell = Cell::plain(format!("R{}C{}", r + 1, c + 1));
                            // Highlight the centre cell with a heavy
                            // magenta border on every side. The
                            // surrounding cells inherit cascade
                            // defaults so the diff harness can attribute
                            // any over-paint to the override path.
                            if r == 1 && c == 1 {
                                cell.top_edge_stroke_color = Some("Color/CMYKMagenta");
                                cell.bottom_edge_stroke_color = Some("Color/CMYKMagenta");
                                cell.left_edge_stroke_color = Some("Color/CMYKMagenta");
                                cell.right_edge_stroke_color = Some("Color/CMYKMagenta");
                                cell.top_edge_stroke_weight = Some(3.0);
                                cell.bottom_edge_stroke_weight = Some(3.0);
                                cell.left_edge_stroke_weight = Some(3.0);
                                cell.right_edge_stroke_weight = Some(3.0);
                            }
                            v.push(cell);
                        }
                    }
                    v
                },
            }),
        },
        // Merged-cells: row 0 column 0 spans 2 columns; row 0 column 2
        // also spans 2 rows. The IDML convention is column-major, with
        // covered slots omitted from the cell list (the spanning cell
        // owns them).
        Variant {
            name: "tables · 3x3 · merged-cells",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: None,
                header_row_count: 0,
                footer_row_count: 0,
                body_row_count: 3,
                column_count: 3,
                row_heights_pt: vec![ROW_H_PT; 3],
                column_widths_pt: vec![COL_W_PT; 3],
                cells: {
                    // Col 0: row 0 spans 2 cols (occupies col 0..1
                    // row 0); rows 1, 2 plain. Cells are listed
                    // column-major; the (col=1 row=0) slot is covered
                    // by the span and omitted.
                    // Col 1: only rows 1 and 2 (row 0 covered).
                    // Col 2: row 0 spans 2 rows (occupies col 2 rows
                    // 0..1); row 2 plain. The (col=2 row=1) slot is
                    // covered.
                    vec![
                        // col 0
                        Cell::plain("Spans 2 columns").with_span(1, 2),
                        Cell::plain("R2C1"),
                        Cell::plain("R3C1"),
                        // col 1 — row 0 covered by the col 0 column-span
                        Cell::plain("R2C2"),
                        Cell::plain("R3C2"),
                        // col 2
                        Cell::plain("Spans 2 rows").with_span(2, 1),
                        // (col 2 row 1 covered by the col 2 row-span)
                        Cell::plain("R3C3"),
                    ]
                },
            }),
        },
        // Multi-paragraph cell content. Verifies the renderer flows
        // multiple paragraphs vertically inside one cell using the
        // standard ParagraphStyleRange/CharacterStyleRange machinery.
        Variant {
            name: "tables · 2x2 · multi-paragraph-cells",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: None,
                header_row_count: 0,
                footer_row_count: 0,
                body_row_count: 2,
                column_count: 2,
                // Tall rows so two paragraphs fit comfortably.
                row_heights_pt: vec![60.0, 60.0],
                column_widths_pt: vec![COL_W_PT * 1.5; 2],
                cells: vec![
                    // col 0 row 0: two paragraphs in one cell
                    Cell {
                        paragraphs: vec![Paragraph::plain("Line 1"), Paragraph::plain("Line 2")],
                        ..Cell::plain("")
                    },
                    Cell::plain("Single line"),
                    // col 1
                    Cell::plain("Single line"),
                    Cell {
                        paragraphs: vec![
                            Paragraph::plain("First"),
                            Paragraph::plain("Second"),
                            Paragraph::plain("Third"),
                        ],
                        ..Cell::plain("")
                    },
                ],
            }),
        },
        // Right-aligned cell text — one paragraph with explicit
        // RightAlign Justification on the cell's paragraph style.
        Variant {
            name: "tables · 2x2 · right-aligned",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: None,
                header_row_count: 0,
                footer_row_count: 0,
                body_row_count: 2,
                column_count: 2,
                row_heights_pt: vec![ROW_H_PT; 2],
                column_widths_pt: vec![COL_W_PT * 1.5; 2],
                cells: {
                    let right_aligned = |s: &str| Cell {
                        paragraphs: vec![Paragraph {
                            justification: Some("RightAlign"),
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
                                text: s.to_string(),
                                point_size: None,
                                fill_color: None,
                                font_style: None,
                                tracking: None,
                                baseline_shift: None,
                                underline: None,
                                applied_font: None,
                                anchored_frame: None,
                            }],
                        }],
                        ..Cell::plain("")
                    };
                    vec![
                        right_aligned("$1,234"),
                        right_aligned("$56,789"),
                        right_aligned("$10"),
                        right_aligned("$200,000"),
                    ]
                },
            }),
        },
        // 90° text orientation in header cells — `RotationAngle="90"`
        // plus the matching `TableTextRotation="Rotate90Degrees"`.
        // Body cells stay axis-aligned so the diff harness can
        // attribute any rotation regression to the header row.
        Variant {
            name: "tables · text-orientation · 90deg-vertical",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: None,
                header_row_count: 1,
                footer_row_count: 0,
                body_row_count: 2,
                column_count: 3,
                // Tall header so the rotated label fits comfortably
                // upright when displayed at 90°.
                row_heights_pt: vec![60.0, ROW_H_PT, ROW_H_PT],
                column_widths_pt: vec![COL_W_PT; 3],
                cells: {
                    let mut v = Vec::new();
                    for c in 0..3 {
                        for r in 0..3 {
                            let label = if r == 0 {
                                format!("Hdr {}", c + 1)
                            } else {
                                format!("R{}C{}", r, c + 1)
                            };
                            let mut cell = Cell::plain(label);
                            if r == 0 {
                                cell.fill_color = Some("Color/CMYKCyan20".to_string());
                                cell.rotation_angle = Some(90.0);
                            }
                            v.push(cell);
                        }
                    }
                    v
                },
            }),
        },
        // Table-style-driven alternating ROW fills. Unlike the earlier
        // "alternating-rows" variant (which fakes the effect with
        // per-cell FillColor), this references a TableStyle carrying
        // `AlternatingFills="AlternatingRows"` so the renderer's
        // style-resolution + alternating-fill emit path is exercised.
        // No per-cell fills — the fill must come entirely from the
        // resolved table style.
        Variant {
            name: "tables · 3x3 · style-alt-rows",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: Some(ALT_ROW_STYLE.to_string()),
                header_row_count: 0,
                footer_row_count: 0,
                body_row_count: 3,
                column_count: 3,
                row_heights_pt: vec![ROW_H_PT; 3],
                column_widths_pt: vec![COL_W_PT; 3],
                cells: {
                    let mut v = Vec::new();
                    for c in 0..3 {
                        for r in 0..3 {
                            v.push(Cell::plain(format!("R{}C{}", r + 1, c + 1)));
                        }
                    }
                    v
                },
            }),
        },
        // Table-style-driven alternating COLUMN fills — same idea on
        // the column axis (`AlternatingFills="AlternatingColumns"`).
        Variant {
            name: "tables · 3x3 · style-alt-cols",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: Some(ALT_COL_STYLE.to_string()),
                header_row_count: 0,
                footer_row_count: 0,
                body_row_count: 3,
                column_count: 3,
                row_heights_pt: vec![ROW_H_PT; 3],
                column_widths_pt: vec![COL_W_PT; 3],
                cells: {
                    let mut v = Vec::new();
                    for c in 0..3 {
                        for r in 0..3 {
                            v.push(Cell::plain(format!("R{}C{}", r + 1, c + 1)));
                        }
                    }
                    v
                },
            }),
        },
        // Cell diagonals. Top-left cell carries a TL→BR ("Left")
        // diagonal; top-right a TR→BL ("Right") diagonal; bottom-left
        // carries BOTH (an X); bottom-right draws a Left diagonal with
        // `DiagonalLineInFront=true` so it paints over the cell text.
        Variant {
            name: "tables · 2x2 · diagonals",
            table: Box::new(|id| Table {
                self_id: id.to_string(),
                applied_table_style: None,
                header_row_count: 0,
                footer_row_count: 0,
                body_row_count: 2,
                column_count: 2,
                row_heights_pt: vec![ROW_H_PT * 1.5; 2],
                column_widths_pt: vec![COL_W_PT; 2],
                cells: {
                    let left = CellDiagonal {
                        left_line_drawn: Some(true),
                        left_line_color: Some("Color/CMYKMagenta"),
                        left_line_weight: Some(1.5),
                        ..CellDiagonal::default()
                    };
                    let right = CellDiagonal {
                        right_line_drawn: Some(true),
                        right_line_color: Some("Color/CMYKMagenta"),
                        right_line_weight: Some(1.5),
                        ..CellDiagonal::default()
                    };
                    let both = CellDiagonal {
                        left_line_drawn: Some(true),
                        left_line_color: Some("Color/CMYKMagenta"),
                        left_line_weight: Some(1.0),
                        right_line_drawn: Some(true),
                        right_line_color: Some("Color/CMYKMagenta"),
                        right_line_weight: Some(1.0),
                        ..CellDiagonal::default()
                    };
                    let in_front = CellDiagonal {
                        left_line_drawn: Some(true),
                        left_line_color: Some("Color/CMYKMagenta"),
                        left_line_weight: Some(2.0),
                        diagonal_in_front: Some(true),
                        ..CellDiagonal::default()
                    };
                    // Column-major: col0row0, col0row1, col1row0, col1row1.
                    vec![
                        Cell::plain("TL→BR").with_diagonal(left),
                        Cell::plain("X both").with_diagonal(both),
                        Cell::plain("TR→BL").with_diagonal(right),
                        Cell::plain("in-front").with_diagonal(in_front),
                    ]
                },
            }),
        },
    ]
}

/// Table styles referenced by the style-driven alternating-fill
/// variants. Declared once and emitted into the shared styles
/// manifest; `<Table>` elements opt in via `AppliedTableStyle`.
fn table_styles() -> Vec<TableStyleSpec> {
    vec![
        TableStyleSpec {
            self_id: ALT_ROW_STYLE.to_string(),
            name: "AltRows".to_string(),
            alternating_fills: Some("AlternatingRows"),
            // Row 0 cyan, row 1 plain, row 2 cyan, … (1-row cycle each).
            start_row_fill_color: Some("Color/CMYKCyan20".to_string()),
            start_row_fill_count: Some(1),
            end_row_fill_color: Some("Swatch/None".to_string()),
            end_row_fill_count: Some(1),
            ..TableStyleSpec::default()
        },
        TableStyleSpec {
            self_id: ALT_COL_STYLE.to_string(),
            name: "AltCols".to_string(),
            alternating_fills: Some("AlternatingColumns"),
            start_column_fill_color: Some("Color/CMYKCyan20".to_string()),
            start_column_fill_count: Some(1),
            end_column_fill_color: Some("Swatch/None".to_string()),
            end_column_fill_count: Some(1),
            ..TableStyleSpec::default()
        },
    ]
}

fn extra_colors() -> Vec<ExtraColor> {
    vec![
        // 20% cyan tint for header / alternating-row fills.
        ExtraColor {
            self_id: "Color/CMYKCyan20".to_string(),
            name: "CMYK Cyan 20".to_string(),
            space: "CMYK",
            value: "20 0 0 0".to_string(),
        },
        // Pure magenta for cell-stroke highlights.
        ExtraColor {
            self_id: "Color/CMYKMagenta".to_string(),
            name: "CMYK Magenta".to_string(),
            space: "CMYK",
            value: "0 100 0 0".to_string(),
        },
    ]
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
        let label_story_id = self_id(SAMPLE, "LabelStory", seq);
        let label_frame_id = self_id(SAMPLE, "LabelFrame", seq);
        let body_frame_id = self_id(SAMPLE, "TextFrame", seq);
        let table_self_id = self_id(SAMPLE, "Table", seq);

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

        // Top-left label story.
        stories.push((
            label_story_id.clone(),
            write_story(&Story {
                self_id: label_story_id.clone(),
                paragraphs: vec![Paragraph::plain(variant.name)],
            }),
        ));
        story_refs.push(label_story_id.clone());

        // Body story containing the table host paragraph.
        let table = (variant.table)(&table_self_id);
        let host_paragraph = Paragraph {
            justification: None,
            space_before: None,
            space_after: None,
            leading: None,
            first_line_indent: None,
            left_indent: None,
            right_indent: None,
            drop_cap_characters: None,
            drop_cap_lines: None,
            bullets_list_type: None,
            bullet_character: None,
            tab_list: Vec::new(),
            table: Some(table),
            runs: Vec::new(),
            minimum_letter_spacing: None,
            desired_letter_spacing: None,
            maximum_letter_spacing: None,
        };
        stories.push((
            story_id.clone(),
            write_story(&Story {
                self_id: story_id.clone(),
                paragraphs: vec![host_paragraph],
            }),
        ));
        story_refs.push(story_id.clone());

        let label_frame = Rect {
            self_id: label_frame_id,
            width_pt: 360.0,
            height_pt: 24.0,
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
            text_frame_pref: None,
        };

        let body_frame = Rect {
            self_id: body_frame_id,
            width_pt: FRAME_W_PT,
            height_pt: FRAME_H_PT,
            item_transform: compose_translate(
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
            text_frame_pref: None,
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
                page_items: vec![label_frame.into(), body_frame.into()],
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
        styles_xml: styles_xml_with_table_styles(&table_styles()),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}

fn compose_translate(tx: f32, ty: f32) -> Matrix {
    let mut m = IDENTITY;
    m[4] = tx;
    m[5] = ty;
    m
}
