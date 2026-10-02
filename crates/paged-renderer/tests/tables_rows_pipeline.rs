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

//! How a table row's height comes from its cells, over the generated
//! `tables-rows.idml`, against where InDesign 20.0.1 put every cell line
//! of the same file and how tall it made every row (2026-10-02; read
//! from its DOM — `cell.lines[i].baseline` / `horizontalOffset`,
//! `row.height` — after opening the generated IDML; the fixture is also
//! in the fidelity gate with InDesign's PDF export as reference).
//!
//! InDesign's rules, as measured (Open Sans 12 pt: ascent 12.826, auto
//! leading 14.4; 1 pt default edges):
//!
//! * A cell's FIRST line sits its ascent below the top inset; every later
//!   line, in the same paragraph or the next, one LEADING below the line
//!   before — the next line's own leading (8 pt after 18 pt is +9.6).
//! * Between paragraphs: the earlier one's space after plus the later
//!   one's space before. The first paragraph's space before and the last
//!   one's space after count for nothing.
//! * The cell is as tall as top inset + the last baseline + bottom inset:
//!   nothing below the last baseline, no descent, no half-leading.
//! * An inset is never less than half the cell's edge stroke on that
//!   side: a 0 pt top inset under a 1 pt edge sets the text 0.5 pt down,
//!   under a 4 pt edge 2 pt (insets 1 and 3 under 4 pt: 2 and 3).
//! * A row is as tall as its tallest cell, and never under its
//!   `MinimumHeight`. Under a `MaximumHeight` the cell keeps the lines that
//!   fit (top inset + baseline + bottom inset within the cap), the rest is
//!   overset, and the row takes the height of what it kept — 20.826, not
//!   the 30 pt cap.
//! * Vertical justification moves the block from the first line's ascent
//!   to the last baseline (the trailing space after left out) inside the
//!   insets.
//! * A row never splits: one that does not fit moves to the next frame,
//!   whose header rows repeat; one taller than every frame is overset with
//!   everything after it.
//! * A fixed row (`AutoGrow="false"`, page 10) is exactly its
//!   `SingleRowHeight`: its cells keep the lines that fit (top inset +
//!   baseline + bottom inset within it) and the rest is overset — a 20 pt
//!   row holds none of a 20.826 pt line, a 40 pt row two of four.
//! * `KeepWithNextRow` (page 11, rows 3-6): a run of kept rows moves to
//!   the next frame together — rows 3-7 open the continuation frame,
//!   under its repeated header, where without the keeps (page 8) the
//!   table breaks after row 5; rows 8-10 no longer fit and are overset.

use paged_gen::samples::tables_rows::{body_story_id, table_id, GATED_PAGES};
use paged_renderer::pipeline::{self, CellAddr};
use paged_renderer::PipelineOptions;

const TOLERANCE: f32 = 0.25;

/// Every line InDesign placed: (page, col, row, line, text, x, baseline).
const LINES: &[(usize, u32, u32, usize, &str, f32, f32)] = &[
    (0, 0, 0, 0, "A1 · p1", 76.500, 137.326),
    (0, 1, 0, 0, "B1 · p1", 206.500, 137.326),
    (0, 1, 0, 1, "B1 · p2", 206.500, 151.726),
    (0, 2, 0, 0, "C1 · p1", 336.500, 137.326),
    (0, 2, 0, 1, "C1 · p2", 336.500, 151.726),
    (0, 2, 0, 2, "C1 · p3", 336.500, 166.126),
    (0, 0, 1, 0, "next row 1", 76.500, 186.952),
    (0, 1, 1, 0, "next row 2", 206.500, 186.952),
    (0, 2, 1, 0, "next row 3", 336.500, 186.952),
    (0, 0, 2, 0, "A3 · p1", 76.500, 207.779),
    (0, 0, 2, 1, "A3 · p2", 76.500, 222.179),
    (0, 0, 2, 2, "A3 · p3", 76.500, 236.579),
    (0, 1, 2, 0, "B3 · p1", 206.500, 207.779),
    (0, 2, 2, 0, "C3 · p1", 336.500, 207.779),
    (0, 2, 2, 1, "C3 · p2", 336.500, 222.179),
    (0, 0, 3, 0, "last row 1", 76.500, 257.405),
    (0, 1, 3, 0, "last row 2", 206.500, 257.405),
    (0, 2, 3, 0, "last row 3", 336.500, 257.405),
    (1, 0, 0, 0, "A1 after 9 · p1", 76.500, 137.326),
    (1, 0, 0, 1, "A1 after 9 · p2", 76.500, 160.726),
    (1, 1, 0, 0, "B1 before 6 · p1", 206.500, 137.326),
    (1, 1, 0, 1, "B1 before 6 · p2", 206.500, 157.726),
    (1, 2, 0, 0, "C1 only last after 12", 336.500, 137.326),
    (1, 2, 0, 1, "· p1", 336.500, 151.726),
    (1, 2, 0, 2, "C1 only last after 12", 336.500, 166.126),
    (1, 2, 0, 3, "· p2", 336.500, 180.526),
    (1, 0, 1, 0, "next row 1", 76.500, 201.352),
    (1, 1, 1, 0, "next row 2", 206.500, 201.352),
    (1, 2, 1, 0, "next row 3", 336.500, 201.352),
    (1, 0, 2, 0, "A3 first before 10 · p1", 76.500, 222.179),
    (1, 1, 2, 0, "B3 after 9 · p1", 206.500, 222.179),
    (1, 1, 2, 1, "B3 after 9 · p2", 206.500, 245.579),
    (1, 1, 2, 2, "B3 after 9 · p3", 206.500, 268.979),
    (1, 2, 2, 0, "C3 both 6/6 · p1", 336.500, 222.179),
    (1, 2, 2, 1, "C3 both 6/6 · p2", 336.500, 248.579),
    (1, 0, 3, 0, "last row 1", 76.500, 289.805),
    (1, 1, 3, 0, "last row 2", 206.500, 289.805),
    (1, 2, 3, 0, "last row 3", 336.500, 289.805),
    (2, 0, 0, 0, "A1 lead 24 · p1", 76.500, 137.326),
    (2, 0, 0, 1, "A1 lead 24 · p2", 76.500, 161.326),
    (2, 1, 0, 0, "B1 lead 9 · p1", 206.500, 137.326),
    (2, 1, 0, 1, "B1 lead 9 · p2", 206.500, 146.326),
    (2, 1, 0, 2, "B1 lead 9 · p3", 206.500, 155.326),
    (2, 2, 0, 0, "C1 · p1", 336.500, 143.739),
    (2, 2, 0, 1, "C1 · p2", 336.500, 153.339),
    (2, 0, 1, 0, "next row 1", 76.500, 182.152),
    (2, 1, 1, 0, "next row 2", 206.500, 182.152),
    (2, 2, 1, 0, "next row 3", 336.500, 182.152),
    (2, 0, 2, 0, "A3 8pt · p1", 76.500, 198.703),
    (2, 0, 2, 1, "A3 8pt · p2", 76.500, 208.303),
    (2, 1, 2, 0, "B3 · p1", 206.500, 198.703),
    (2, 1, 2, 1, "B3 · p2", 206.500, 220.303),
    (2, 2, 2, 0, "C3 a long paragraph", 336.500, 202.979),
    (2, 2, 2, 1, "that wraps onto more", 336.500, 217.379),
    (2, 2, 2, 2, "lines · p1", 336.500, 231.779),
    (2, 0, 3, 0, "last row 1", 76.500, 252.605),
    (2, 1, 3, 0, "last row 2", 206.500, 252.605),
    (2, 2, 3, 0, "last row 3", 336.500, 252.605),
    (3, 0, 0, 0, "A1 · p1", 84.500, 143.326),
    (3, 0, 0, 1, "A1 · p2", 84.500, 157.726),
    (3, 1, 0, 0, "B1 · p1", 206.500, 133.826),
    (3, 2, 0, 0, "C1 · p1", 333.000, 133.826),
    (3, 0, 1, 0, "next row 1", 76.500, 176.552),
    (3, 1, 1, 0, "next row 2", 206.500, 176.552),
    (3, 2, 1, 0, "next row 3", 336.500, 176.552),
    (3, 0, 2, 0, "A3 · p1", 73.000, 193.879),
    (3, 1, 2, 0, "B3 · p1", 203.000, 193.879),
    (3, 2, 2, 0, "C3 · p1", 333.000, 193.879),
    (3, 0, 3, 0, "last row 1", 76.500, 211.205),
    (3, 1, 3, 0, "last row 2", 206.500, 211.205),
    (3, 2, 3, 0, "last row 3", 336.500, 211.205),
    (4, 0, 0, 0, "A1 top 0 · p1", 78.000, 136.826),
    (4, 1, 0, 0, "B1 top 1 · p1", 208.000, 136.826),
    (4, 2, 0, 0, "C1 top 3 · p1", 338.000, 137.826),
    (4, 0, 1, 0, "A2 bottom 0 · p1", 78.000, 158.652),
    (4, 1, 1, 0, "B2 · p1", 208.000, 158.652),
    (4, 2, 1, 0, "C2 · p1", 338.000, 158.652),
    (4, 0, 2, 0, "A3 bottom 1 · p1", 78.000, 177.479),
    (4, 1, 2, 0, "B3 · p1", 208.000, 177.479),
    (4, 2, 2, 0, "C3 · p1", 338.000, 177.479),
    (4, 0, 3, 0, "A4 bottom 3 · p1", 78.000, 196.305),
    (4, 1, 3, 0, "B4 · p1", 208.000, 196.305),
    (4, 2, 3, 0, "C4 · p1", 338.000, 196.305),
    (4, 0, 4, 0, "last row 1", 78.000, 216.131),
    (4, 1, 4, 0, "last row 2", 208.000, 216.131),
    (4, 2, 4, 0, "last row 3", 338.000, 216.131),
    (5, 0, 0, 0, "A1 min 50 · p1", 76.500, 137.326),
    (5, 1, 0, 0, "B1 · p1", 206.500, 137.326),
    (5, 2, 0, 0, "C1 · p1", 336.500, 137.326),
    (5, 2, 0, 1, "C1 · p2", 336.500, 151.726),
    (5, 0, 1, 0, "A2 min 20 · p1", 76.500, 187.326),
    (5, 1, 1, 0, "B2 · p1", 206.500, 187.326),
    (5, 1, 1, 1, "B2 · p2", 206.500, 201.726),
    (5, 1, 1, 2, "B2 · p3", 206.500, 216.126),
    (5, 2, 1, 0, "C2 · p1", 336.500, 187.326),
    (5, 0, 2, 0, "A3 max 30 · p1", 76.500, 236.952),
    (5, 1, 2, 0, "B3 · p1", 206.500, 236.952),
    (5, 2, 2, 0, "C3 · p1", 336.500, 236.952),
    (5, 0, 3, 0, "last row 1", 76.500, 257.779),
    (5, 1, 3, 0, "last row 2", 206.500, 257.779),
    (5, 2, 3, 0, "last row 3", 336.500, 257.779),
    (6, 0, 0, 0, "top · p1", 76.500, 137.326),
    (6, 0, 0, 1, "top · p2", 76.500, 151.726),
    (6, 1, 0, 0, "center · p1", 176.500, 164.713),
    (6, 1, 0, 1, "center · p2", 176.500, 179.113),
    (6, 2, 0, 0, "bottom · p1", 276.500, 192.100),
    (6, 2, 0, 1, "bottom · p2", 276.500, 206.500),
    (6, 3, 0, 0, "justify · p1", 376.500, 137.326),
    (6, 3, 0, 1, "justify · p2", 376.500, 206.500),
    (6, 0, 1, 0, "top sa6 · p1", 76.500, 227.326),
    (6, 0, 1, 1, "top sa6 · p2", 76.500, 247.726),
    (6, 1, 1, 0, "center sa6 · p1", 176.500, 251.713),
    (6, 1, 1, 1, "center sa6 · p2", 176.500, 272.113),
    (6, 2, 1, 0, "bottom sa6 · p1", 276.500, 276.100),
    (6, 2, 1, 1, "bottom sa6 · p2", 276.500, 296.500),
    (6, 3, 1, 0, "justify sa6 · p1", 376.500, 227.326),
    (6, 3, 1, 1, "justify sa6 · p2", 376.500, 296.500),
    (6, 0, 2, 0, "last row 1", 76.500, 317.326),
    (6, 1, 2, 0, "last row 2", 176.500, 317.326),
    (6, 2, 2, 0, "last row 3", 276.500, 317.326),
    (6, 3, 2, 0, "last row 4", 376.500, 317.326),
    (7, 0, 0, 0, "header 1", 40.500, 137.326),
    (7, 1, 0, 0, "header 2", 160.500, 137.326),
    (7, 0, 1, 0, "R1A · p1", 40.500, 158.152),
    (7, 0, 1, 1, "R1A · p2", 40.500, 172.552),
    (7, 1, 1, 0, "R1B · p1", 160.500, 158.152),
    (7, 1, 1, 1, "R1B · p2", 160.500, 172.552),
    (7, 0, 2, 0, "R2A · p1", 40.500, 193.379),
    (7, 0, 2, 1, "R2A · p2", 40.500, 207.779),
    (7, 1, 2, 0, "R2B · p1", 160.500, 193.379),
    (7, 1, 2, 1, "R2B · p2", 160.500, 207.779),
    (7, 1, 2, 2, "R2B · p3", 160.500, 222.179),
    (7, 0, 3, 0, "R3A · p1", 40.500, 243.005),
    (7, 0, 3, 1, "R3A · p2", 40.500, 257.405),
    (7, 1, 3, 0, "R3B · p1", 160.500, 243.005),
    (7, 0, 4, 0, "R4A · p1", 40.500, 278.231),
    (7, 0, 4, 1, "R4A · p2", 40.500, 292.631),
    (7, 1, 4, 0, "R4B · p1", 160.500, 278.231),
    (7, 1, 4, 1, "R4B · p2", 160.500, 292.631),
    (7, 0, 5, 0, "R5A · p1", 40.500, 313.457),
    (7, 0, 5, 1, "R5A · p2", 40.500, 327.857),
    (7, 1, 5, 0, "R5B · p1", 160.500, 313.457),
    (7, 1, 5, 1, "R5B · p2", 160.500, 327.857),
    (7, 1, 5, 2, "R5B · p3", 160.500, 342.257),
    (7, 0, 6, 0, "R6A · p1", 314.500, 158.152),
    (7, 0, 6, 1, "R6A · p2", 314.500, 172.552),
    (7, 1, 6, 0, "R6B · p1", 434.500, 158.152),
    (7, 0, 7, 0, "R7A · p1", 314.500, 193.379),
    (7, 0, 7, 1, "R7A · p2", 314.500, 207.779),
    (7, 1, 7, 0, "R7B · p1", 434.500, 193.379),
    (7, 1, 7, 1, "R7B · p2", 434.500, 207.779),
    (7, 0, 8, 0, "R8A · p1", 314.500, 228.605),
    (7, 0, 8, 1, "R8A · p2", 314.500, 243.005),
    (7, 1, 8, 0, "R8B · p1", 434.500, 228.605),
    (7, 1, 8, 1, "R8B · p2", 434.500, 243.005),
    (7, 1, 8, 2, "R8B · p3", 434.500, 257.405),
    (7, 0, 9, 0, "R9A · p1", 314.500, 278.231),
    (7, 0, 9, 1, "R9A · p2", 314.500, 292.631),
    (7, 1, 9, 0, "R9B · p1", 434.500, 278.231),
    (7, 0, 10, 0, "R10A · p1", 314.500, 313.457),
    (7, 0, 10, 1, "R10A · p2", 314.500, 327.857),
    (7, 1, 10, 0, "R10B · p1", 434.500, 313.457),
    (7, 1, 10, 1, "R10B · p2", 434.500, 327.857),
    (8, 0, 0, 0, "first 1", 40.500, 137.326),
    (8, 1, 0, 0, "first 2", 160.500, 137.326),
    (9, 0, 1, 0, "A2 fixed 40 · p1", 76.500, 157.326),
    (9, 1, 1, 0, "B2 · p1", 206.500, 157.326),
    (9, 2, 1, 0, "C2 · p1", 336.500, 157.326),
    (9, 2, 1, 1, "C2 · p2", 336.500, 171.726),
    (9, 0, 2, 0, "last row 1", 76.500, 197.326),
    (9, 1, 2, 0, "last row 2", 206.500, 197.326),
    (9, 2, 2, 0, "last row 3", 336.500, 197.326),
    (10, 0, 0, 0, "header 1", 40.500, 137.326),
    (10, 1, 0, 0, "header 2", 160.500, 137.326),
    (10, 0, 1, 0, "R1A · p1", 40.500, 158.152),
    (10, 0, 1, 1, "R1A · p2", 40.500, 172.552),
    (10, 1, 1, 0, "R1B · p1", 160.500, 158.152),
    (10, 1, 1, 1, "R1B · p2", 160.500, 172.552),
    (10, 0, 2, 0, "R2A · p1", 40.500, 193.379),
    (10, 0, 2, 1, "R2A · p2", 40.500, 207.779),
    (10, 1, 2, 0, "R2B · p1", 160.500, 193.379),
    (10, 1, 2, 1, "R2B · p2", 160.500, 207.779),
    (10, 1, 2, 2, "R2B · p3", 160.500, 222.179),
    (10, 0, 3, 0, "R3A · p1", 314.500, 158.152),
    (10, 0, 3, 1, "R3A · p2", 314.500, 172.552),
    (10, 1, 3, 0, "R3B · p1", 434.500, 158.152),
    (10, 0, 4, 0, "R4A · p1", 314.500, 193.379),
    (10, 0, 4, 1, "R4A · p2", 314.500, 207.779),
    (10, 1, 4, 0, "R4B · p1", 434.500, 193.379),
    (10, 1, 4, 1, "R4B · p2", 434.500, 207.779),
    (10, 0, 5, 0, "R5A · p1", 314.500, 228.605),
    (10, 0, 5, 1, "R5A · p2", 314.500, 243.005),
    (10, 1, 5, 0, "R5B · p1", 434.500, 228.605),
    (10, 1, 5, 1, "R5B · p2", 434.500, 243.005),
    (10, 1, 5, 2, "R5B · p3", 434.500, 257.405),
    (10, 0, 6, 0, "R6A · p1", 314.500, 278.231),
    (10, 0, 6, 1, "R6A · p2", 314.500, 292.631),
    (10, 1, 6, 0, "R6B · p1", 434.500, 278.231),
    (10, 0, 7, 0, "R7A · p1", 314.500, 313.457),
    (10, 0, 7, 1, "R7A · p2", 314.500, 327.857),
    (10, 1, 7, 0, "R7B · p1", 434.500, 313.457),
    (10, 1, 7, 1, "R7B · p2", 434.500, 327.857),
];

/// How many lines InDesign placed in each cell (the rest are overset).
const PLACED: &[(usize, u32, u32, usize)] = &[
    (0, 0, 0, 1),
    (0, 1, 0, 2),
    (0, 2, 0, 3),
    (0, 0, 1, 1),
    (0, 1, 1, 1),
    (0, 2, 1, 1),
    (0, 0, 2, 3),
    (0, 1, 2, 1),
    (0, 2, 2, 2),
    (0, 0, 3, 1),
    (0, 1, 3, 1),
    (0, 2, 3, 1),
    (1, 0, 0, 2),
    (1, 1, 0, 2),
    (1, 2, 0, 4),
    (1, 0, 1, 1),
    (1, 1, 1, 1),
    (1, 2, 1, 1),
    (1, 0, 2, 1),
    (1, 1, 2, 3),
    (1, 2, 2, 2),
    (1, 0, 3, 1),
    (1, 1, 3, 1),
    (1, 2, 3, 1),
    (2, 0, 0, 2),
    (2, 1, 0, 3),
    (2, 2, 0, 2),
    (2, 0, 1, 1),
    (2, 1, 1, 1),
    (2, 2, 1, 1),
    (2, 0, 2, 2),
    (2, 1, 2, 2),
    (2, 2, 2, 3),
    (2, 0, 3, 1),
    (2, 1, 3, 1),
    (2, 2, 3, 1),
    (3, 0, 0, 2),
    (3, 1, 0, 1),
    (3, 2, 0, 1),
    (3, 0, 1, 1),
    (3, 1, 1, 1),
    (3, 2, 1, 1),
    (3, 0, 2, 1),
    (3, 1, 2, 1),
    (3, 2, 2, 1),
    (3, 0, 3, 1),
    (3, 1, 3, 1),
    (3, 2, 3, 1),
    (4, 0, 0, 1),
    (4, 1, 0, 1),
    (4, 2, 0, 1),
    (4, 0, 1, 1),
    (4, 1, 1, 1),
    (4, 2, 1, 1),
    (4, 0, 2, 1),
    (4, 1, 2, 1),
    (4, 2, 2, 1),
    (4, 0, 3, 1),
    (4, 1, 3, 1),
    (4, 2, 3, 1),
    (4, 0, 4, 1),
    (4, 1, 4, 1),
    (4, 2, 4, 1),
    (5, 0, 0, 1),
    (5, 1, 0, 1),
    (5, 2, 0, 2),
    (5, 0, 1, 1),
    (5, 1, 1, 3),
    (5, 2, 1, 1),
    (5, 0, 2, 1),
    (5, 1, 2, 1),
    (5, 2, 2, 1),
    (5, 0, 3, 1),
    (5, 1, 3, 1),
    (5, 2, 3, 1),
    (6, 0, 0, 2),
    (6, 1, 0, 2),
    (6, 2, 0, 2),
    (6, 3, 0, 2),
    (6, 0, 1, 2),
    (6, 1, 1, 2),
    (6, 2, 1, 2),
    (6, 3, 1, 2),
    (6, 0, 2, 1),
    (6, 1, 2, 1),
    (6, 2, 2, 1),
    (6, 3, 2, 1),
    (7, 0, 0, 1),
    (7, 1, 0, 1),
    (7, 0, 1, 2),
    (7, 1, 1, 2),
    (7, 0, 2, 2),
    (7, 1, 2, 3),
    (7, 0, 3, 2),
    (7, 1, 3, 1),
    (7, 0, 4, 2),
    (7, 1, 4, 2),
    (7, 0, 5, 2),
    (7, 1, 5, 3),
    (7, 0, 6, 2),
    (7, 1, 6, 1),
    (7, 0, 7, 2),
    (7, 1, 7, 2),
    (7, 0, 8, 2),
    (7, 1, 8, 3),
    (7, 0, 9, 2),
    (7, 1, 9, 1),
    (7, 0, 10, 2),
    (7, 1, 10, 2),
    (8, 0, 0, 1),
    (8, 1, 0, 1),
    (8, 0, 1, 0),
    (8, 1, 1, 0),
    (8, 0, 2, 0),
    (8, 1, 2, 0),
    (9, 0, 0, 0),
    (9, 1, 0, 0),
    (9, 2, 0, 0),
    (9, 0, 1, 1),
    (9, 1, 1, 1),
    (9, 2, 1, 2),
    (9, 0, 2, 1),
    (9, 1, 2, 1),
    (9, 2, 2, 1),
    (10, 0, 0, 1),
    (10, 1, 0, 1),
    (10, 0, 1, 2),
    (10, 1, 1, 2),
    (10, 0, 2, 2),
    (10, 1, 2, 3),
    (10, 0, 3, 2),
    (10, 1, 3, 1),
    (10, 0, 4, 2),
    (10, 1, 4, 2),
    (10, 0, 5, 2),
    (10, 1, 5, 3),
    (10, 0, 6, 2),
    (10, 1, 6, 1),
    (10, 0, 7, 2),
    (10, 1, 7, 2),
    (10, 0, 8, 0),
    (10, 1, 8, 0),
    (10, 0, 9, 0),
    (10, 1, 9, 0),
    (10, 0, 10, 0),
    (10, 1, 10, 0),
];

/// Every row's height: (page, row, height).
const ROWS: &[(usize, u32, f32)] = &[
    (0, 0, 49.626),
    (0, 1, 20.826),
    (0, 2, 49.626),
    (0, 3, 20.826),
    (1, 0, 64.026),
    (1, 1, 20.826),
    (1, 2, 67.626),
    (1, 3, 20.826),
    (2, 0, 44.826),
    (2, 1, 20.826),
    (2, 2, 49.626),
    (2, 3, 20.826),
    (3, 0, 39.226),
    (3, 1, 20.826),
    (3, 2, 13.826),
    (3, 3, 20.826),
    (4, 0, 19.826),
    (4, 1, 18.826),
    (4, 2, 18.826),
    (4, 3, 19.826),
    (4, 4, 20.826),
    (5, 0, 50.000),
    (5, 1, 49.626),
    (5, 2, 20.826),
    (5, 3, 20.826),
    (6, 0, 90.000),
    (6, 1, 90.000),
    (6, 2, 20.826),
    (7, 0, 20.826),
    (7, 1, 35.226),
    (7, 2, 49.626),
    (7, 3, 35.226),
    (7, 4, 35.226),
    (7, 5, 49.626),
    (7, 6, 35.226),
    (7, 7, 35.226),
    (7, 8, 49.626),
    (7, 9, 35.226),
    (7, 10, 35.226),
    (8, 0, 20.826),
    (8, 1, 179.226),
    (8, 2, 3.000),
    (9, 0, 20.000),
    (9, 1, 40.000),
    (9, 2, 20.826),
    (10, 0, 20.826),
    (10, 1, 35.226),
    (10, 2, 49.626),
    (10, 3, 35.226),
    (10, 4, 35.226),
    (10, 5, 49.626),
    (10, 6, 35.226),
    (10, 7, 35.226),
    (10, 8, 49.626),
    (10, 9, 3.000),
    (10, 10, 3.000),
];

/// Rows InDesign oversets (page, row): not drawn at all. Named rather
/// than inferred from `PLACED`: page 10's fixed 20 pt row places no line
/// in any cell, yet the row is drawn and the next row starts 20 pt below
/// it.
const OVERSET_ROWS: &[(usize, u32)] = &[(8, 1), (8, 2), (10, 8), (10, 9), (10, 10)];

/// Header rows InDesign repeats at the top of the continuation frame
/// on the break page: the DOM reports a cell's lines once, the PDF
/// export draws the replay 274 pt to the right at the same baseline.
/// Page 11's replays were read off the PDF export (`pdftotext -bbox`:
/// "header" at x 314.5 / 434.5, the same box as page 8's).
const HEADER_REPLAYS: &[(usize, u32, u32, f32, f32)] = &[
    (7, 0, 0, 314.5, 137.326),
    (7, 1, 0, 434.5, 137.326),
    (10, 0, 0, 314.5, 137.326),
    (10, 1, 0, 434.5, 137.326),
];

fn open_sans() -> Vec<u8> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts/OpenSans.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read OpenSans.ttf: {e}"))
}

fn build() -> pipeline::BuiltDocument {
    let bytes = paged_gen::write_idml(&paged_gen::samples::tables_rows::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = open_sans();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    pipeline::build_document(&doc, &opts).expect("build")
}

#[test]
fn every_cell_line_lands_where_indesign_puts_it() {
    let built = build();
    let mut report = Vec::new();
    let mut failures = 0;
    for &(page, col, row, placed) in PLACED {
        if page >= GATED_PAGES {
            continue;
        }
        let addr = CellAddr {
            table_id: table_id(page as u32),
            row,
            col,
        };
        let lines = built.cell_layout(&body_story_id(page as u32), &addr);
        let mut want: Vec<(String, f32, f32)> = LINES
            .iter()
            .filter(|l| (l.0, l.1, l.2) == (page, col, row))
            .map(|l| (l.4.to_string(), l.5, l.6))
            .collect();
        want.extend(
            HEADER_REPLAYS
                .iter()
                .filter(|h| (h.0, h.1, h.2) == (page, col, row))
                .map(|h| ("(header replay)".to_string(), h.3, h.4)),
        );
        assert!(want.len() >= placed);
        let mut used = vec![false; lines.len()];
        let mut bad = lines.len() != want.len();
        let mut rows_out = Vec::new();
        for (text, wx, wy) in &want {
            let best = lines
                .iter()
                .enumerate()
                .filter(|(i, _)| !used[*i])
                .map(|(i, l)| {
                    let x = l
                        .clusters
                        .iter()
                        .map(|c| c.x_pt)
                        .fold(f32::INFINITY, f32::min);
                    (i, x, l.baseline_y_pt)
                })
                .min_by(|a, b| {
                    let da = (a.1 - wx).abs() + (a.2 - wy).abs();
                    let db = (b.1 - wx).abs() + (b.2 - wy).abs();
                    da.total_cmp(&db)
                });
            match best {
                Some((i, x, y)) => {
                    used[i] = true;
                    let ok = (x - wx).abs() <= TOLERANCE && (y - wy).abs() <= TOLERANCE;
                    bad |= !ok;
                    rows_out.push(format!(
                        "   {} {text:28} engine x {x:8.3} y {y:8.3}   indesign x {wx:8.3} y {wy:8.3}",
                        if ok { " " } else { "✗" }
                    ));
                }
                None => {
                    bad = true;
                    rows_out.push(format!("   ✗ {text:28} missing (indesign y {wy:8.3})"));
                }
            }
        }
        if bad {
            failures += 1;
            report.push(format!(
                "── page {} cell {col}:{row}: engine {} lines, InDesign {}",
                page + 1,
                lines.len(),
                want.len()
            ));
            report.extend(rows_out);
        }
    }
    assert_eq!(failures, 0, "\n{}", report.join("\n"));
}

#[test]
fn every_row_is_as_tall_as_indesign_makes_it() {
    let built = build();
    let mut report = Vec::new();
    for &(page, row, height) in ROWS {
        if page >= GATED_PAGES {
            continue;
        }
        let tid = table_id(page as u32);
        let rects: Vec<f32> = built
            .pages
            .iter()
            .flat_map(|p| p.cell_rects.iter())
            .filter(|r| r.table_id == tid && r.row == row && r.col == 0)
            .map(|r| r.rect[3])
            .collect();
        // A row InDesign oversets is not drawn: it has no rect.
        let overset = OVERSET_ROWS.contains(&(page, row));
        let ok = if overset {
            rects.is_empty()
        } else {
            !rects.is_empty() && rects.iter().all(|h| (h - height).abs() <= TOLERANCE)
        };
        if !ok {
            report.push(format!(
                "page {} row {row}: engine {rects:?}, InDesign {height:.3}{}",
                page + 1,
                if overset { " (overset)" } else { "" }
            ));
        }
    }
    assert!(report.is_empty(), "\n{}", report.join("\n"));
}

/// Page 12: a paragraph with no word in a cell is a line. Where InDesign
/// put the top of each cell paragraph's first word, as distances below
/// `A1 · p1` (`pdftotext -bbox` on its PDF export, 2026-10-02; Open Sans
/// 12 pt, so a leading is 14.4): `(col, row, paragraph, distance)`.
///
/// * a blank paragraph between two others: the later one sits two
///   leadings below the earlier (A1, and A3 where it is three spaces);
/// * a blank FIRST paragraph takes the cell's first line (B1 · p2 is one
///   leading down);
/// * two blank paragraphs in a row: three leadings (C3 · p4);
/// * the rows are as tall as their tallest cell, blank lines counted
///   (`next row` 49.626 and `last row` 64.026 below their row's first
///   line: a three-line and a four-line cell).
const BLANK_PARAGRAPH_LINES: &[(u32, u32, u32, f32)] = &[
    (0, 0, 0, 0.0),
    (0, 0, 2, 28.8),
    (1, 0, 1, 14.4),
    (2, 0, 0, 0.0),
    (0, 1, 0, 49.626),
    (0, 2, 0, 70.452),
    (0, 2, 2, 99.252),
    (2, 2, 0, 70.452),
    (2, 2, 3, 113.652),
    (0, 3, 0, 134.478),
];

#[test]
fn a_blank_paragraph_in_a_cell_is_a_line_as_in_indesign() {
    const PAGE: u32 = 11;
    let built = build();
    let baseline = |col: u32, row: u32, paragraph: u32| {
        let addr = CellAddr {
            table_id: table_id(PAGE),
            row,
            col,
        };
        built
            .cell_layout(&body_story_id(PAGE), &addr)
            .iter()
            .find(|l| l.paragraph_idx == paragraph)
            .map(|l| l.baseline_y_pt)
    };
    let origin = baseline(0, 0, 0).expect("A1 · p1");
    let mut wrong = Vec::new();
    for &(col, row, paragraph, want) in BLANK_PARAGRAPH_LINES {
        match baseline(col, row, paragraph) {
            Some(b) if (b - origin - want).abs() <= TOLERANCE => {}
            got => wrong.push(format!(
                "col {col} row {row} paragraph {paragraph}: {:?} below A1 · p1, InDesign {want}",
                got.map(|b| b - origin)
            )),
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
