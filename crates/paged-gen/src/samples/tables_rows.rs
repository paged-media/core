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

//! `tables-rows.idml` — how a table row's height comes from its cells.
//!
//! Each page asks InDesign one question about row height, and the
//! reference PDF is its answer:
//!
//!   1. one, two and three paragraphs in a cell (auto leading)
//!   2. space before / space after on cell paragraphs
//!   3. fixed leading, tight leading, mixed point sizes
//!   4. cell insets, top / bottom / left / right
//!   5. `MinimumHeight` floors and a `MaximumHeight` cap
//!   6. fixed rows (`AutoGrow="false"`) holding more text than fits
//!   7. vertical justification inside a cell taller than its text
//!   8. a table breaking across two threaded frames, header repeating
//!   9. the same with `KeepWithNextRow` on the rows at the break
//!  10. a row taller than its frame
//!  11. (after the two `AutoGrow` / `KeepWithNextRow` pages) paragraphs
//!      with no word in a cell: empty, and of spaces
//!
//! Every cell's text names its own cell and paragraph ("B2 · p3"), so a
//! misplaced line reads off the diff, and every table has a plain row
//! BELOW the row under test: a row that comes out too short shows as
//! that row's text overlapping the one above it.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Cell, Paragraph, Story, Table},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "tables-rows";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const FRAME_X_PT: f32 = 72.0;
const FRAME_Y_PT: f32 = 120.0;
const FRAME_W_PT: f32 = 400.0;
/// A floor low enough that content, not the declared height, sizes the
/// row. InDesign's own smallest row is a few points.
const LOW_FLOOR_PT: f32 = 3.0;

/// How many leading pages the fidelity gate covers: all twelve. Pages
/// 10-11 (`AutoGrow`, `KeepWithNextRow`) joined once the IDML reader
/// carried both attributes (plugin-publish 96cfb6c); page 12 (blank
/// paragraphs in a cell) was added after them.
pub const GATED_PAGES: usize = 12;

/// The story holding page `page`'s table (0-based page).
pub fn body_story_id(page: u32) -> String {
    self_id(SAMPLE, "Story", page)
}

/// The `<Table Self>` on page `page` (0-based).
pub fn table_id(page: u32) -> String {
    self_id(SAMPLE, "Table", page)
}

struct Variant {
    name: &'static str,
    /// Frame boxes `(x, y, w, h)`, threaded in order.
    frames: Vec<(f32, f32, f32, f32)>,
    table: Table,
}

/// A cell of `n` paragraphs named `"{tag} · p{i}"`, each shaped by `f`.
fn cell_of(tag: &str, n: usize, f: impl Fn(usize, &mut Paragraph)) -> Cell {
    let paragraphs = (0..n)
        .map(|i| {
            let mut p = Paragraph::plain(format!("{tag} · p{}", i + 1));
            f(i, &mut p);
            p
        })
        .collect();
    Cell {
        paragraphs,
        ..Cell::plain("")
    }
}

fn plain_cells(tag: &str, cols: usize) -> Vec<Cell> {
    (0..cols)
        .map(|c| Cell::plain(format!("{tag}{}", c + 1)))
        .collect()
}

/// Build a table from rows of cells (row-major in the source, written
/// column-major as IDML orders them).
fn table(id: &str, header_rows: u32, col_w: Vec<f32>, rows: Vec<(f32, Vec<Cell>)>) -> Table {
    let cols = col_w.len();
    let row_heights_pt: Vec<f32> = rows.iter().map(|(h, _)| *h).collect();
    let mut grid: Vec<Vec<Option<Cell>>> = rows
        .into_iter()
        .map(|(_, cells)| {
            assert_eq!(cells.len(), cols, "{id}: every row carries {cols} cells");
            cells.into_iter().map(Some).collect()
        })
        .collect();
    let n_rows = grid.len();
    let mut cells = Vec::with_capacity(n_rows * cols);
    for c in 0..cols {
        for row in grid.iter_mut() {
            cells.push(row[c].take().expect("cell"));
        }
    }
    Table {
        self_id: id.to_string(),
        applied_table_style: None,
        header_row_count: header_rows,
        footer_row_count: 0,
        body_row_count: n_rows as u32 - header_rows,
        column_count: cols as u32,
        row_heights_pt,
        column_widths_pt: col_w,
        cells,
        extra_row_attrs: Vec::new(),
    }
}

fn one_frame(h: f32) -> Vec<(f32, f32, f32, f32)> {
    vec![(FRAME_X_PT, FRAME_Y_PT, FRAME_W_PT, h)]
}

/// Two frames side by side, the second continuing the first.
fn two_frames(h: f32) -> Vec<(f32, f32, f32, f32)> {
    vec![(36.0, FRAME_Y_PT, 250.0, h), (310.0, FRAME_Y_PT, 250.0, h)]
}

fn variants() -> Vec<Variant> {
    let c3 = vec![130.0; 3];
    let mut out = Vec::new();

    out.push(Variant {
        name: "rows · 1, 2 and 3 paragraphs in a cell · auto leading",
        frames: one_frame(400.0),
        table: table(
            "t1",
            0,
            c3.clone(),
            vec![
                (
                    LOW_FLOOR_PT,
                    vec![
                        cell_of("A1", 1, |_, _| {}),
                        cell_of("B1", 2, |_, _| {}),
                        cell_of("C1", 3, |_, _| {}),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("next row ", 3)),
                (
                    LOW_FLOOR_PT,
                    vec![
                        cell_of("A3", 3, |_, _| {}),
                        cell_of("B3", 1, |_, _| {}),
                        cell_of("C3", 2, |_, _| {}),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("last row ", 3)),
            ],
        ),
    });

    out.push(Variant {
        name: "rows · space before 6 / space after 9 on cell paragraphs",
        frames: one_frame(400.0),
        table: table(
            "t2",
            0,
            c3.clone(),
            vec![
                (
                    LOW_FLOOR_PT,
                    vec![
                        cell_of("A1 after 9", 2, |_, p| p.space_after = Some(9.0)),
                        cell_of("B1 before 6", 2, |_, p| p.space_before = Some(6.0)),
                        cell_of("C1 only last after 12", 2, |i, p| {
                            if i == 1 {
                                p.space_after = Some(12.0)
                            }
                        }),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("next row ", 3)),
                (
                    LOW_FLOOR_PT,
                    vec![
                        cell_of("A3 first before 10", 1, |_, p| p.space_before = Some(10.0)),
                        cell_of("B3 after 9", 3, |_, p| p.space_after = Some(9.0)),
                        cell_of("C3 both 6/6", 2, |_, p| {
                            p.space_before = Some(6.0);
                            p.space_after = Some(6.0);
                        }),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("last row ", 3)),
            ],
        ),
    });

    out.push(Variant {
        name: "rows · leading 24, leading 9, 18pt over 8pt",
        frames: one_frame(400.0),
        table: table(
            "t3",
            0,
            c3.clone(),
            vec![
                (
                    LOW_FLOOR_PT,
                    vec![
                        cell_of("A1 lead 24", 2, |_, p| p.leading = Some(24.0)),
                        cell_of("B1 lead 9", 3, |_, p| p.leading = Some(9.0)),
                        cell_of("C1", 2, |i, p| {
                            p.runs[0].point_size = Some(if i == 0 { 18.0 } else { 8.0 })
                        }),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("next row ", 3)),
                (
                    LOW_FLOOR_PT,
                    vec![
                        cell_of("A3 8pt", 2, |_, p| p.runs[0].point_size = Some(8.0)),
                        cell_of("B3", 2, |i, p| {
                            p.runs[0].point_size = Some(if i == 0 { 8.0 } else { 18.0 })
                        }),
                        cell_of(
                            "C3 a long paragraph that wraps onto more lines",
                            1,
                            |_, _| {},
                        ),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("last row ", 3)),
            ],
        ),
    });

    let inset = |mut cell: Cell, t: f32, l: f32, b: f32, r: f32| {
        for (k, v) in [
            ("TextTopInset", t),
            ("TextLeftInset", l),
            ("TextBottomInset", b),
            ("TextRightInset", r),
        ] {
            cell.extra_cell_attrs.push((k, crate::xml::format_f32(v)));
        }
        cell
    };
    out.push(Variant {
        name: "rows · cell insets T10 L12 B2 R0 · T0 B16 · all 0",
        frames: one_frame(400.0),
        table: table(
            "t4",
            0,
            c3.clone(),
            vec![
                (
                    LOW_FLOOR_PT,
                    vec![
                        inset(cell_of("A1", 2, |_, _| {}), 10.0, 12.0, 2.0, 0.0),
                        inset(cell_of("B1", 1, |_, _| {}), 0.0, 4.0, 16.0, 4.0),
                        inset(cell_of("C1", 1, |_, _| {}), 0.0, 0.0, 0.0, 0.0),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("next row ", 3)),
                (
                    LOW_FLOOR_PT,
                    vec![
                        inset(cell_of("A3", 1, |_, _| {}), 0.0, 0.0, 0.0, 0.0),
                        inset(cell_of("B3", 1, |_, _| {}), 0.0, 0.0, 0.0, 0.0),
                        inset(cell_of("C3", 1, |_, _| {}), 0.0, 0.0, 0.0, 0.0),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("last row ", 3)),
            ],
        ),
    });

    // The same insets under a 4 pt edge: a 1 pt edge moved a zero-inset
    // cell's text half a point down (page 4), and a rule seen at one
    // weight is a coincidence.
    let heavy = |mut cell: Cell, top: f32, bottom: f32| {
        cell.top_edge_stroke_color = Some("Color/Black");
        cell.bottom_edge_stroke_color = Some("Color/Black");
        cell.left_edge_stroke_color = Some("Color/Black");
        cell.right_edge_stroke_color = Some("Color/Black");
        cell.top_edge_stroke_weight = Some(4.0);
        cell.bottom_edge_stroke_weight = Some(4.0);
        cell.left_edge_stroke_weight = Some(4.0);
        cell.right_edge_stroke_weight = Some(4.0);
        inset(cell, top, 4.0, bottom, 4.0)
    };
    out.push(Variant {
        name: "rows · 4pt edges · top inset 0 / 1 / 3 · bottom inset 0 / 1 / 3",
        frames: one_frame(400.0),
        table: table(
            "t4b",
            0,
            c3.clone(),
            vec![
                (
                    LOW_FLOOR_PT,
                    vec![
                        heavy(cell_of("A1 top 0", 1, |_, _| {}), 0.0, 4.0),
                        heavy(cell_of("B1 top 1", 1, |_, _| {}), 1.0, 4.0),
                        heavy(cell_of("C1 top 3", 1, |_, _| {}), 3.0, 4.0),
                    ],
                ),
                (
                    LOW_FLOOR_PT,
                    vec![
                        heavy(cell_of("A2 bottom 0", 1, |_, _| {}), 4.0, 0.0),
                        heavy(cell_of("B2", 1, |_, _| {}), 4.0, 0.0),
                        heavy(cell_of("C2", 1, |_, _| {}), 4.0, 0.0),
                    ],
                ),
                (
                    LOW_FLOOR_PT,
                    vec![
                        heavy(cell_of("A3 bottom 1", 1, |_, _| {}), 4.0, 1.0),
                        heavy(cell_of("B3", 1, |_, _| {}), 4.0, 1.0),
                        heavy(cell_of("C3", 1, |_, _| {}), 4.0, 1.0),
                    ],
                ),
                (
                    LOW_FLOOR_PT,
                    vec![
                        heavy(cell_of("A4 bottom 3", 1, |_, _| {}), 4.0, 3.0),
                        heavy(cell_of("B4", 1, |_, _| {}), 4.0, 3.0),
                        heavy(cell_of("C4", 1, |_, _| {}), 4.0, 3.0),
                    ],
                ),
                (
                    LOW_FLOOR_PT,
                    vec![
                        heavy(Cell::plain("last row 1"), 4.0, 4.0),
                        heavy(Cell::plain("last row 2"), 4.0, 4.0),
                        heavy(Cell::plain("last row 3"), 4.0, 4.0),
                    ],
                ),
            ],
        ),
    });

    let mut t5 = table(
        "t5",
        0,
        c3.clone(),
        vec![
            (
                50.0,
                vec![
                    cell_of("A1 min 50", 1, |_, _| {}),
                    cell_of("B1", 1, |_, _| {}),
                    cell_of("C1", 2, |_, _| {}),
                ],
            ),
            (
                20.0,
                vec![
                    cell_of("A2 min 20", 1, |_, _| {}),
                    cell_of("B2", 3, |_, _| {}),
                    cell_of("C2", 1, |_, _| {}),
                ],
            ),
            (
                LOW_FLOOR_PT,
                vec![
                    cell_of("A3 max 30", 1, |_, _| {}),
                    cell_of("B3", 4, |_, _| {}),
                    cell_of("C3", 1, |_, _| {}),
                ],
            ),
            (LOW_FLOOR_PT, plain_cells("last row ", 3)),
        ],
    );
    t5.extra_row_attrs
        .push((2, "MaximumHeight", "30".to_string()));
    out.push(Variant {
        name: "rows · MinimumHeight 50 / 20 · MaximumHeight 30",
        frames: one_frame(400.0),
        table: t5,
    });

    let mut t6 = table(
        "t6",
        0,
        c3.clone(),
        vec![
            (
                20.0,
                vec![
                    cell_of("A1 fixed 20", 1, |_, _| {}),
                    cell_of("B1", 3, |_, _| {}),
                    cell_of("C1", 1, |_, _| {}),
                ],
            ),
            (
                40.0,
                vec![
                    cell_of("A2 fixed 40", 1, |_, _| {}),
                    cell_of("B2", 1, |_, _| {}),
                    cell_of("C2", 4, |_, _| {}),
                ],
            ),
            (LOW_FLOOR_PT, plain_cells("last row ", 3)),
        ],
    );
    for r in 0..2u32 {
        t6.extra_row_attrs
            .push((r, "AutoGrow", "false".to_string()));
    }
    out.push(Variant {
        name: "rows · fixed 20 and 40 (AutoGrow false) · overset cells",
        frames: one_frame(400.0),
        table: t6,
    });

    let vj = |v: &'static str, tag: &str, after: Option<f32>| {
        cell_of(tag, 2, |_, p| p.space_after = after).with_vertical_justification(v)
    };
    out.push(Variant {
        name: "rows · vertical justification in 90pt rows",
        frames: one_frame(400.0),
        table: table(
            "t7",
            0,
            vec![100.0; 4],
            vec![
                (
                    90.0,
                    vec![
                        vj("TopAlign", "top", None),
                        vj("CenterAlign", "center", None),
                        vj("BottomAlign", "bottom", None),
                        vj("JustifyAlign", "justify", None),
                    ],
                ),
                (
                    90.0,
                    vec![
                        vj("TopAlign", "top sa6", Some(6.0)),
                        vj("CenterAlign", "center sa6", Some(6.0)),
                        vj("BottomAlign", "bottom sa6", Some(6.0)),
                        vj("JustifyAlign", "justify sa6", Some(6.0)),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("last row ", 4)),
            ],
        ),
    });

    let breaking_rows = |id: &str| {
        let mut rows = vec![(LOW_FLOOR_PT, plain_cells("header ", 2))];
        for r in 1..=10 {
            rows.push((
                LOW_FLOOR_PT,
                vec![
                    cell_of(&format!("R{r}A"), 2, |_, _| {}),
                    cell_of(&format!("R{r}B"), 1 + (r % 3), |_, _| {}),
                ],
            ));
        }
        table(id, 1, vec![120.0; 2], rows)
    };
    out.push(Variant {
        name: "rows · break across two frames · header repeats",
        frames: two_frames(250.0),
        table: breaking_rows("t8"),
    });

    let mut t9 = breaking_rows("t9");
    for r in 3..=6u32 {
        t9.extra_row_attrs
            .push((r, "KeepWithNextRow", "true".to_string()));
    }
    out.push(Variant {
        name: "rows · break across two frames · KeepWithNextRow on rows 3-6",
        frames: two_frames(250.0),
        table: t9,
    });

    out.push(Variant {
        name: "rows · a row taller than its frame",
        frames: two_frames(150.0),
        table: table(
            "t10",
            0,
            vec![120.0; 2],
            vec![
                (LOW_FLOOR_PT, plain_cells("first ", 2)),
                (
                    LOW_FLOOR_PT,
                    vec![
                        cell_of("tall", 12, |_, _| {}),
                        cell_of("beside", 1, |_, _| {}),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("after ", 2)),
            ],
        ),
    });

    // The two pages whose rows depend on `AutoGrow` / `KeepWithNextRow`
    // go LAST. They were held out of the gate until the IDML reader
    // carried both attributes; they are gated now, and stay last because
    // InDesign's reference PDF has its pages in this order.
    let (mut gated, ungated): (Vec<_>, Vec<_>) = out
        .into_iter()
        .partition(|v| !(v.name.contains("AutoGrow") || v.name.contains("KeepWithNextRow")));
    gated.extend(ungated);

    // Page 12, after them so no earlier page moves: paragraphs with no
    // word in a cell. A blank paragraph between two others, first in its
    // cell, last in its cell; a paragraph of spaces; two blank ones in a
    // row; and a cell holding nothing but a blank paragraph next to one
    // line of text.
    let cell = |texts: &[&str]| Cell {
        paragraphs: texts.iter().map(|t| Paragraph::plain(*t)).collect(),
        ..Cell::plain("")
    };
    gated.push(Variant {
        name: "rows · blank paragraphs in a cell",
        frames: one_frame(400.0),
        table: table(
            "t11",
            0,
            vec![130.0; 3],
            vec![
                (
                    LOW_FLOOR_PT,
                    vec![
                        cell(&["A1 · p1", "", "A1 · p3"]),
                        cell(&["", "B1 · p2"]),
                        cell(&["C1 · p1", ""]),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("next row ", 3)),
                (
                    LOW_FLOOR_PT,
                    vec![
                        cell(&["A3 · p1", "   ", "A3 · p3"]),
                        cell(&["", ""]),
                        cell(&["C3 · p1", "", "", "C3 · p4"]),
                    ],
                ),
                (LOW_FLOOR_PT, plain_cells("last row ", 3)),
            ],
        ),
    });
    gated
}

pub fn build() -> Sample {
    let variants = variants();

    let mut master_spreads = Vec::with_capacity(variants.len());
    let mut spreads = Vec::with_capacity(variants.len());
    let mut stories = Vec::with_capacity(variants.len());
    let mut master_refs = Vec::with_capacity(variants.len());
    let mut spread_refs = Vec::with_capacity(variants.len());
    let mut story_refs = Vec::with_capacity(variants.len());

    for (i, variant) in variants.into_iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let story_id = self_id(SAMPLE, "Story", seq);
        let label_story_id = self_id(SAMPLE, "LabelStory", seq);
        let label_frame_id = self_id(SAMPLE, "LabelFrame", seq);
        let mut table = variant.table;
        table.self_id = self_id(SAMPLE, "Table", seq);

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
            label_story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: label_story_id.clone(),
                paragraphs: vec![Paragraph::plain(variant.name)],
            }),
        ));
        story_refs.push(label_story_id.clone());

        stories.push((
            story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: story_id.clone(),
                paragraphs: vec![Paragraph::plain("").with_table(table)],
            }),
        ));
        story_refs.push(story_id.clone());

        let mut page_items: Vec<Rect> = vec![plain_frame(
            label_frame_id,
            (480.0, 24.0),
            translate_m(36.0, 36.0),
            label_story_id,
        )];
        let frame_ids: Vec<String> = (0..variant.frames.len())
            .map(|f| self_id(SAMPLE, "TextFrame", seq * 10 + f as u32))
            .collect();
        for (f, &(x, y, w, h)) in variant.frames.iter().enumerate() {
            let mut frame = plain_frame(
                frame_ids[f].clone(),
                (w, h),
                translate_m(x, y),
                story_id.clone(),
            );
            frame.previous_text_frame = f.checked_sub(1).map(|p| frame_ids[p].clone());
            frame.next_text_frame = frame_ids.get(f + 1).cloned();
            page_items.push(frame);
        }

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: variant.name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: page_items.into_iter().map(Into::into).collect(),
                override_list: Vec::new(),
                margins: None,
                item_transform: None,
            }),
        ));
        spread_refs.push(spread_id);
    }

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: master_refs,
            spreads: spread_refs,
            stories: story_refs,
        }),
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

fn plain_frame(
    self_id: String,
    (width_pt, height_pt): (f32, f32),
    item_transform: Matrix,
    parent_story: String,
) -> Rect {
    Rect {
        self_id,
        width_pt,
        height_pt,
        item_transform,
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: Some(parent_story),
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

fn translate_m(tx: f32, ty: f32) -> Matrix {
    let mut m = IDENTITY;
    m[4] = tx;
    m[5] = ty;
    m
}
