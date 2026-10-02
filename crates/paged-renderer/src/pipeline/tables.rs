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

//! Table emission: chain-aware table layout (header/footer row replay
//! across NextTextFrame breaks, row-growth pre-measure), per-edge
//! stroke resolution, nested tables inside cells, and the shared
//! cell-paragraph measure / emit pair.

use super::*;

/// Resolved Start/End fill pattern for one axis (rows or columns).
/// Picks the (colour, tint) for the `line_idx`-th body line, honouring
/// the Skip-First / Skip-Last counts and the Start-count / End-count
/// alternation cycle. Returns `None` when the line is skipped or no
/// pattern is configured.
struct AlternatingFillAxis<'a> {
    n_lines: usize,
    skip_first: usize,
    skip_last: usize,
    start_color: Option<&'a str>,
    start_count: usize,
    start_tint: Option<f32>,
    end_color: Option<&'a str>,
    end_count: usize,
    end_tint: Option<f32>,
}

/// InDesign's default tint for the START of an alternating-fill cycle.
///
/// The attribute is omitted when it equals the default, and a file
/// with no `StartRowFillTint` resolves to 20 in InDesign's own DOM
/// (measured 2026-09-06 by stripping the attribute from an
/// InDesign-authored IDML and reading `table.startRowFillTint` back).
/// Painting the colour at full strength instead made the annual's
/// preflight table a solid warm block where InDesign shows a wash:
/// (250, 247, 240) against InDesign's (254, 254, 252).
const ALTERNATING_START_TINT: f32 = 20.0;
/// …and 100 for the END of the cycle, measured the same way.
const ALTERNATING_END_TINT: f32 = 100.0;

impl<'a> AlternatingFillAxis<'a> {
    fn fill_for(&self, line_idx: usize) -> Option<(&'a str, Option<f32>)> {
        if line_idx < self.skip_first || line_idx + self.skip_last >= self.n_lines {
            return None;
        }
        let cycle = self.start_count + self.end_count;
        if cycle == 0 {
            return None;
        }
        let pos = (line_idx - self.skip_first) % cycle;
        if pos < self.start_count {
            self.start_color
                .map(|c| (c, Some(self.start_tint.unwrap_or(ALTERNATING_START_TINT))))
        } else {
            self.end_color
                .map(|c| (c, Some(self.end_tint.unwrap_or(ALTERNATING_END_TINT))))
        }
    }
}

/// Lay out and emit a `<Table>` at the StoryEmitter's current
/// cursor in the head frame. Treats every cell as a mini-frame:
/// computes its rect from cumulative row heights + column widths,
/// then routes each cell paragraph through `emit_cell_paragraph`
/// which does a self-contained shape → layout → emit at a fixed
/// origin and column width.
///
/// Scope:
/// * Honours per-row `SingleRowHeight`, `MinimumHeight`,
///   `MaximumHeight` (Task T3.2) and per-column `SingleColumnWidth`.
///   Cells with `RowSpan > 1` or `ColumnSpan > 1` widen / lengthen
///   their rect; multi-cell text merging across spans isn't
///   separately modelled.
/// * Cells with content overflow grow their row up to
///   `MaximumHeight` — a top-down pre-measure pass computes per-row
///   required heights, then `row_heights[r] =
///   max(SingleRowHeight, MinimumHeight, max_cell_required)`, where a
///   cell under a cap requires only the lines that fit it (see
///   `plan_cell_block`). For RowSpan > 1 cells the constraint is
///   applied to the LAST spanned row only (simpler heuristic; the
///   common case has spans inside header rows that don't grow).
/// * Header rows duplicate at the top of every continuation frame
///   when the table breaks across a NextTextFrame chain; footer
///   rows duplicate at the bottom of every frame except the last
///   (Task T3.1). `RepeatingHeader="false"` / `RepeatingFooter="false"`
///   opt out.
// Range-loops over `row_heights` carry the row index (`r`) as data
// — it doubles as a template index into `table.rows` / `table.cells`
// — so the `needless_range_loop` lint is a false positive here.
#[allow(clippy::needless_range_loop)]
pub(super) fn emit_table_into_chain(
    em: &mut StoryEmitter,
    table: &paged_model::Table,
    pages: &mut [BuiltPage],
    total_stats: &mut PipelineStats,
) {
    if table.cells.is_empty() {
        return;
    }
    let col_widths: Vec<f32> = table
        .columns
        .iter()
        .map(|c| c.single_column_width.unwrap_or(0.0))
        .collect();
    let mut col_x: Vec<f32> = Vec::with_capacity(col_widths.len() + 1);
    let mut acc = 0.0f32;
    col_x.push(0.0);
    for w in &col_widths {
        acc += *w;
        col_x.push(acc);
    }
    let total_w = col_x.last().copied().unwrap_or(0.0);

    let covered = covered_grid_positions(table);

    let resolved_table = table
        .applied_table_style
        .as_deref()
        .map(|id| em.document.styles.resolve_table(id))
        .unwrap_or_default();
    let header_count = table.header_row_count as usize;
    let footer_count = table.footer_row_count as usize;
    let total_rows = table.rows.len();
    let total_cols = col_widths.len();

    // Content-driven row growth, by InDesign's rules (`plan_cell_block`):
    // a cell needs its top inset + its last baseline + its bottom inset,
    // and a row is as tall as its tallest cell, never under its floor
    // `max(SingleRowHeight, MinimumHeight)`. Under a `MaximumHeight` a
    // cell keeps the lines that fit and the row is as tall as THEY need:
    // InDesign made a capped row 20.826 pt, not its 30 pt cap, when the
    // second line would have needed 35.2 (2026-10-02, `tables-rows`
    // page 6). A fixed row (`AutoGrow="false"`) is capped at its floor,
    // so it stays exactly that tall and its cells keep the lines that
    // fit (`row_cap_pt`). For RowSpan > 1 cells only the LAST spanned
    // row grows to cover the shortfall, which keeps a span from blowing
    // up every row it crosses.
    let mut row_heights: Vec<f32> = table.rows.iter().map(row_floor_pt).collect();
    let region_cell_style_for = |c: usize, r: usize| -> Option<&str> {
        if r < header_count {
            return resolved_table.header_region_cell_style.as_deref();
        }
        if footer_count > 0 && r + footer_count >= total_rows {
            return resolved_table.footer_region_cell_style.as_deref();
        }
        if c == 0 {
            if let Some(s) = resolved_table.left_column_region_cell_style.as_deref() {
                return Some(s);
            }
        }
        if c + 1 == total_cols {
            if let Some(s) = resolved_table.right_column_region_cell_style.as_deref() {
                return Some(s);
            }
        }
        resolved_table.body_region_cell_style.as_deref()
    };

    // Per-cell measured content and effective (top, bottom) insets,
    // keyed by the cell's starting (col, row). The emit pass draws from
    // the same block, so a row is sized from exactly what it draws.
    let mut cell_blocks: std::collections::HashMap<(u32, u32), (CellBlock, CellInsets)> =
        std::collections::HashMap::with_capacity(table.cells.len());
    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else {
            continue;
        };
        if covered.contains(&(c, r)) {
            continue;
        }
        let (cu, ru) = (c as usize, r as usize);
        if cu >= col_widths.len() || ru >= total_rows {
            continue;
        }
        let span_cols = cell.column_span.max(1) as usize;
        let last_c = (cu + span_cols).min(col_widths.len());
        let resolved_cell = cell
            .applied_cell_style
            .as_deref()
            .filter(|id| !is_none_style_id(id))
            .or_else(|| region_cell_style_for(cu, ru))
            .map(|id| em.document.styles.resolve_cell(id))
            .unwrap_or_default();
        let insets = effective_cell_insets(cell, &resolved_cell);
        let inner_w = (col_x[last_c] - col_x[cu] - insets.left - insets.right).max(0.0);
        let block = plan_cell_block(em, &cell.paragraphs, inner_w);
        cell_blocks.insert((c, r), (block, insets));
    }
    for r in 0..total_rows {
        let max_h = table.rows.get(r).map_or(f32::INFINITY, row_cap_pt);
        let mut required = row_heights[r];
        for cell in &table.cells {
            let Some((c, sr)) = cell.coords() else {
                continue;
            };
            let span = cell.row_span.max(1) as usize;
            let sru = sr as usize;
            if sru + span - 1 != r {
                continue;
            }
            let Some((block, insets)) = cell_blocks.get(&(c, sr)) else {
                continue;
            };
            let (top, bottom) = (insets.top, insets.bottom);
            // Heights already grown for the prior rows of the span.
            let prior: f32 = (sru..r).map(|i| row_heights[i]).sum();
            let (fit_bottom, _) = block.fitted(prior + max_h - top - bottom);
            required = required.max(top + fit_bottom + bottom - prior);
        }
        row_heights[r] = required.min(max_h);
    }

    // Repeating-header / repeating-footer flags. IDML defaults
    // both to true (the attribute is absent in the common case
    // and the rows *do* repeat); explicit `RepeatingHeader="false"`
    // / `RepeatingFooter="false"` opt out.
    let repeating_header = table.repeating_header.unwrap_or(true) && header_count > 0;
    let repeating_footer = table.repeating_footer.unwrap_or(true) && footer_count > 0;

    // Per-row layout basis: which chain frame the row lives in,
    // page-local row-top y, AND which template row in `table.rows`
    // sources the cells / heights for this row. Body rows have
    // `template_idx == phys_idx_in_source`; header/footer replays
    // reuse a template row's index while sitting at a different
    // geometric position.
    #[derive(Clone, Copy, Debug)]
    #[allow(dead_code)]
    enum RowKind {
        /// Body / header / footer row from the original sequence,
        /// emitted once.
        Original,
        /// Replayed header row at the top of a continuation frame.
        HeaderReplay,
        /// Replayed footer row at the bottom of a non-last frame.
        FooterReplay,
    }
    #[derive(Clone, Copy)]
    struct PhysicalRow {
        /// Index into `table.rows` whose cells / height this
        /// physical row mirrors. Cells look up `table.cells` by
        /// `(col, template_idx)`.
        template_idx: usize,
        height: f32,
        chain_idx: usize,
        target_page: usize,
        table_left_pt: f32,
        /// Page-local y for the top of THIS row.
        row_top_in_page: f32,
        /// Kept for debugging / future per-kind hooks (e.g. when
        /// header replays want a different divider style than the
        /// original header dividers). Not read by current emission.
        #[allow(dead_code)]
        kind: RowKind,
    }
    let frame_basis_for = |chain_idx: usize, x_shift: f32| -> (f32, f32, f32, f32, usize) {
        let frame = em.chain[chain_idx];
        let target_page = em.chain_pages[chain_idx];
        let (sx, sy) = frame_spread_top_left(frame.bounds, frame.item_transform);
        let (ox, oy) = pages[target_page].spread_origin;
        let insets = frame.inset_spacing.unwrap_or([0.0; 4]);
        let table_left_pt = sx - ox + insets[1] + x_shift;
        let frame_top_in_page = sy - oy;
        let frame_height = frame.bounds.height();
        (
            table_left_pt,
            frame_top_in_page,
            frame_height,
            insets[0],
            target_page,
        )
    };
    let (border_off_top, border_off_left) = outer_border_offsets(table);
    let mut chain_idx = em.frame_idx;
    let (mut tab_left, mut frame_top_in_page, mut frame_height, mut top_inset, mut target_page) =
        frame_basis_for(chain_idx, em.column_x_shift_pt);
    tab_left += border_off_left;
    let mut row_top_y_in_frame = if em.y_cursor >= 0 {
        em.y_cursor as f32 / paged_text::shape::ADVANCE_PRECISION
            - em.options.default_point_size * 0.8
    } else {
        top_inset + border_off_top
    };
    // Total replayed-footer height we should leave reserved below
    // body rows in any non-last frame. Equals the sum of footer
    // template heights when `repeating_footer` is set.
    let footer_reserved_h: f32 = if repeating_footer {
        (total_rows - footer_count..total_rows)
            .map(|r| row_heights[r])
            .sum()
    } else {
        0.0
    };
    // Same for headers — height of header rows we replay at the
    // top of every continuation frame.
    let header_reserved_h: f32 = if repeating_header {
        (0..header_count).map(|r| row_heights[r]).sum()
    } else {
        0.0
    };

    let mut physical_rows: Vec<PhysicalRow> = Vec::with_capacity(total_rows);
    // Per-frame extent for table-border emission below.
    // Each entry: (chain_idx, target_page, table_left_pt, row_top
    // of the first row in this frame, row_bottom of the last row
    // in this frame).
    let mut frame_extents: Vec<(usize, usize, f32, f32, f32)> = Vec::new();
    let mut current_frame_first_top = frame_top_in_page + row_top_y_in_frame;
    let mut current_frame_last_bottom = current_frame_first_top;

    // Track which "body" rows (rows whose template index falls in
    // `header_count..total_rows - footer_count`) we still need to
    // emit. Header rows are always emitted at the top of frame 1
    // (their position in the original sequence) plus replayed at
    // the top of every continuation frame. Footer rows are emitted
    // at the bottom of the *last* frame in the original sequence
    // position, plus replayed at the bottom of every non-last frame.
    let body_range = header_count..total_rows.saturating_sub(footer_count);

    // Helper closures need to keep the borrow of `em` short, so we
    // pull the frame-advance logic into an inline block. The body
    // of the loop below is mechanical: append the next body row
    // (or first run of original headers / final footers) and check
    // whether we still fit.
    let mut placed_in_frame = 0usize;

    // Emit the original header rows at the start of the head frame
    // (they sit in the natural sequence — no replay).
    for r in 0..header_count {
        let h = row_heights[r];
        // We don't attempt to fit headers across a frame split on
        // their own — if a head frame is too small to hold even
        // the headers we'd loop forever. Leave them in this frame
        // and let the body rows trigger the chain advance instead.
        physical_rows.push(PhysicalRow {
            template_idx: r,
            height: h,
            chain_idx,
            target_page,
            table_left_pt: tab_left,
            row_top_in_page: frame_top_in_page + row_top_y_in_frame,
            kind: RowKind::Original,
        });
        row_top_y_in_frame += h;
        current_frame_last_bottom = frame_top_in_page + row_top_y_in_frame;
        placed_in_frame += 1;
    }

    // Emit body rows. Before placing each row, check whether it
    // (plus the footer-reserve, if any) would overflow the current
    // frame. If so, close out this frame with replayed footers,
    // advance, then prepend replayed headers in the new frame.
    // Rows the last frame cannot hold are overset, as InDesign oversets
    // them (measured 2026-09-06: two chart tables and a preflight table
    // taller than their frames, which the canvas drew past the frame's
    // bottom edge with no word of it). A frame that grows to its
    // content keeps placing, as the text lane does.
    let last_frame_grows_height = em
        .chain
        .last()
        .and_then(|f| f.auto_sizing)
        .map(|a| a.grows_height())
        .unwrap_or(false);
    let mut overset_at: Option<usize> = None;
    // How many BODY rows this frame took. A frame that holds none draws
    // nothing at all, headers included — see `body_placed_in_frame`
    // below.
    let mut body_placed_in_frame = 0usize;
    for r in body_range.clone() {
        let h = row_heights[r];
        // `KeepWithNextRow`: a row that starts a run of kept rows (it
        // keeps with the next, and the row before it does not keep with
        // it) needs room for the whole run, or the run moves to the next
        // frame together. InDesign moved rows 3-7 of a table whose rows
        // 3-6 keep with the next one to the continuation frame, where
        // without the keeps it breaks after row 5 (2026-10-02,
        // `tables-rows` page 11 vs page 8). The run asks this only of a
        // frame that already holds body rows: one taller than a whole
        // frame is placed row by row from the top of the next, instead of
        // walking every frame of the chain. Not measured: how InDesign
        // places a run that does not fit the LAST frame — here its rows
        // are placed one by one and the first that does not fit oversets.
        let run_h: f32 = if r > body_range.start && keeps_with_next(table.rows.get(r - 1)) {
            h
        } else {
            let mut end = r;
            while end + 1 < body_range.end && keeps_with_next(table.rows.get(end)) {
                end += 1;
            }
            row_heights[r..=end].iter().sum()
        };
        // A row never splits: one that does not fit moves to the next
        // frame, and one that fits no frame is overset with everything
        // after it (InDesign, 2026-10-02, `tables-rows` page 9: a 179 pt
        // row between two 150 pt frames leaves the second frame empty).
        // The fit is re-tested in every frame the row moves to — it
        // used to be placed in the next frame unconditionally.
        loop {
            let need_extra_for_split = footer_reserved_h;
            let would_overflow = row_top_y_in_frame + h + need_extra_for_split > frame_height;
            let run_overflows = body_placed_in_frame > 0
                && row_top_y_in_frame + run_h + need_extra_for_split > frame_height;
            // In the LAST frame there is nowhere to advance to, so a row
            // that does not fit is overset — even the first one. The
            // `placed_in_frame > 0` guard that used to sit here belongs to
            // the frame-ADVANCE branch below, where it stops an empty frame
            // from looping forever; carrying it here made a frame too short
            // for even its first row draw that row anyway. InDesign draws
            // nothing (measured 2026-09-07 on `tables-overset`: 4 rows of
            // 28 pt in a 20 pt frame is zero ink in InDesign's own export,
            // and was one row here).
            if would_overflow && chain_idx + 1 >= em.chain.len() && !last_frame_grows_height {
                overset_at = Some(r);
                break;
            }
            if !((would_overflow || run_overflows)
                && chain_idx + 1 < em.chain.len()
                && placed_in_frame > 0)
            {
                break;
            }
            body_placed_in_frame = 0;
            // Append replayed footers at the bottom of this frame.
            if repeating_footer {
                for fr in (total_rows - footer_count)..total_rows {
                    let fh = row_heights[fr];
                    physical_rows.push(PhysicalRow {
                        template_idx: fr,
                        height: fh,
                        chain_idx,
                        target_page,
                        table_left_pt: tab_left,
                        row_top_in_page: frame_top_in_page + row_top_y_in_frame,
                        kind: RowKind::FooterReplay,
                    });
                    row_top_y_in_frame += fh;
                    current_frame_last_bottom = frame_top_in_page + row_top_y_in_frame;
                }
            }
            // Close out current frame's extent.
            frame_extents.push((
                chain_idx,
                target_page,
                tab_left,
                current_frame_first_top,
                current_frame_last_bottom,
            ));
            chain_idx += 1;
            let (l, ftop, h_next, ti, tp) = frame_basis_for(chain_idx, 0.0);
            tab_left = l + border_off_left;
            frame_top_in_page = ftop;
            frame_height = h_next;
            top_inset = ti;
            target_page = tp;
            row_top_y_in_frame = top_inset + border_off_top;
            current_frame_first_top = frame_top_in_page + row_top_y_in_frame;
            placed_in_frame = 0;
            // Prepend replayed headers at the top of the new frame.
            // (current_frame_last_bottom is updated by the body push
            // immediately following — no need to maintain it here.)
            if repeating_header {
                for hr in 0..header_count {
                    let hh = row_heights[hr];
                    physical_rows.push(PhysicalRow {
                        template_idx: hr,
                        height: hh,
                        chain_idx,
                        target_page,
                        table_left_pt: tab_left,
                        row_top_in_page: frame_top_in_page + row_top_y_in_frame,
                        kind: RowKind::HeaderReplay,
                    });
                    row_top_y_in_frame += hh;
                    placed_in_frame += 1;
                }
                let _ = header_reserved_h;
            }
            // current_frame_last_bottom updates when the next body
            // row pushes below; no need to maintain it here.
        }
        if overset_at.is_some() {
            break;
        }
        physical_rows.push(PhysicalRow {
            template_idx: r,
            height: h,
            chain_idx,
            target_page,
            table_left_pt: tab_left,
            row_top_in_page: frame_top_in_page + row_top_y_in_frame,
            kind: RowKind::Original,
        });
        row_top_y_in_frame += h;
        current_frame_last_bottom = frame_top_in_page + row_top_y_in_frame;
        placed_in_frame += 1;
        body_placed_in_frame += 1;
    }

    // A frame that holds header rows but not one body row draws
    // NOTHING — InDesign suppresses the headers too (measured
    // 2026-09-07: a header plus three 28 pt rows in a 30 pt frame, so
    // the header alone fits, is zero ink in InDesign's export and was
    // one row here). A header is a label for rows; with no rows to
    // label InDesign does not place it, and the same holds for a
    // replayed header at the top of a continuation frame.
    if body_placed_in_frame == 0 && header_count > 0 {
        physical_rows.retain(|row| row.chain_idx != chain_idx);
    }

    // Original footer rows — emitted on whatever frame the body
    // left off in (= the last frame), in their natural sequence.
    if let Some(r) = overset_at {
        // Report once per story, like the text lane: the count of
        // dropped rows is not the actionable bit, the overset is.
        if !em.overset_reported {
            em.overset_reported = true;
            let mut d = crate::diagnostics::Diagnostic::new(
                crate::diagnostics::DiagnosticCode::OversetTextDropped,
                "table rows overflow the last frame in the chain; trailing rows clipped (overset)",
            )
            .with_page(target_page)
            .with_overset(em.paragraph_idx, r as u32);
            if !em.current_story_id.is_empty() {
                d = d.with_story(em.current_story_id.clone());
            }
            em.diagnostics.push(d);
        }
    }
    for r in (total_rows - footer_count)..total_rows {
        if footer_count == 0 || overset_at.is_some() {
            break;
        }
        let h = row_heights[r];
        physical_rows.push(PhysicalRow {
            template_idx: r,
            height: h,
            chain_idx,
            target_page,
            table_left_pt: tab_left,
            row_top_in_page: frame_top_in_page + row_top_y_in_frame,
            kind: RowKind::Original,
        });
        row_top_y_in_frame += h;
        current_frame_last_bottom = frame_top_in_page + row_top_y_in_frame;
    }
    // Close out the trailing frame extent.
    frame_extents.push((
        chain_idx,
        target_page,
        tab_left,
        current_frame_first_top,
        current_frame_last_bottom,
    ));
    // Track the final frame index + y for the y_cursor advance.
    let final_chain_idx = chain_idx;
    let final_y_in_frame = row_top_y_in_frame;

    // ── Alternating row / column fills ─────────────────────────────
    //
    // Cell-background precedence (lowest → highest), painted in this
    // order so later layers cover earlier ones:
    //   1. region default (CellStyle fill, resolved later per cell),
    //   2. the table-style alternating pattern (THIS block),
    //   3. a cell-local inline `FillColor` on the `<Cell>` (per-cell
    //      loop below).
    // i.e. effective precedence is  cell-local > alternating > region
    // default. The alternating fill is emitted HERE — before the
    // per-cell loop — so cell-local / region fills paint over it, and
    // glyphs (emitted last) sit on top of everything.
    //
    // IDML's `AlternatingFills` discriminator picks the axis:
    //   * "AlternatingRows"    → cycle the Start/End *row* fills,
    //   * "AlternatingColumns" → cycle the Start/End *column* fills.
    // An absent discriminator paired with a Start *row* fill colour is
    // treated as AlternatingRows (older InDesign exports omit the
    // discriminator). Each axis honours its Start/End count cycle plus
    // the Skip-First / Skip-Last body-line counts.
    let alt_axis = resolved_table.alternating_fills.as_deref();
    let want_row_fill = matches!(alt_axis, Some("AlternatingRows"))
        || (alt_axis.is_none() && resolved_table.start_row_fill_color.is_some());
    let want_col_fill = matches!(alt_axis, Some("AlternatingColumns"));

    let body_rows = total_rows.saturating_sub(header_count + footer_count);
    if want_row_fill {
        let axis = AlternatingFillAxis {
            n_lines: body_rows,
            skip_first: resolved_table.skip_first_alternating_fill_rows.unwrap_or(0) as usize,
            skip_last: resolved_table.skip_last_alternating_fill_rows.unwrap_or(0) as usize,
            start_color: resolved_table.start_row_fill_color.as_deref(),
            start_count: resolved_table.start_row_fill_count.unwrap_or(0) as usize,
            start_tint: resolved_table.start_row_fill_tint,
            end_color: resolved_table.end_row_fill_color.as_deref(),
            end_count: resolved_table.end_row_fill_count.unwrap_or(0) as usize,
            end_tint: resolved_table.end_row_fill_tint,
        };
        // Alternating row fills iterate the physical-row sequence:
        // replayed headers / footers count from their *original*
        // template index so the visual cycle stays coherent across
        // frame splits.
        for prow in &physical_rows {
            let r = prow.template_idx;
            if r < header_count {
                continue;
            }
            if footer_count > 0 && r + footer_count >= total_rows {
                continue;
            }
            let body_idx = r - header_count;
            let Some((fill_id, tint)) = axis.fill_for(body_idx) else {
                continue;
            };
            let Some(paint) = color_id_to_paint(fill_id, em.palette, em.color_ctx) else {
                continue;
            };
            let paint = apply_fill_tint(paint, tint);
            let rect = Rect {
                x: prow.table_left_pt,
                y: prow.row_top_in_page,
                w: total_w,
                h: prow.height,
            };
            emit_rect(rect, paint, &mut pages[prow.target_page].list);
        }
    }
    if want_col_fill {
        let axis = AlternatingFillAxis {
            n_lines: col_widths.len(),
            skip_first: resolved_table
                .skip_first_alternating_fill_columns
                .unwrap_or(0) as usize,
            skip_last: resolved_table
                .skip_last_alternating_fill_columns
                .unwrap_or(0) as usize,
            start_color: resolved_table.start_column_fill_color.as_deref(),
            start_count: resolved_table.start_column_fill_count.unwrap_or(0) as usize,
            start_tint: resolved_table.start_column_fill_tint,
            end_color: resolved_table.end_column_fill_color.as_deref(),
            end_count: resolved_table.end_column_fill_count.unwrap_or(0) as usize,
            end_tint: resolved_table.end_column_fill_tint,
        };
        // Alternating column fills span the full height of each
        // physical row, painted column-by-column. Columns have no
        // header/footer concept, so every column 0..N participates
        // (subject to skip-first / skip-last).
        for prow in &physical_rows {
            for c in 0..col_widths.len() {
                let Some((fill_id, tint)) = axis.fill_for(c) else {
                    continue;
                };
                let Some(paint) = color_id_to_paint(fill_id, em.palette, em.color_ctx) else {
                    continue;
                };
                let paint = apply_fill_tint(paint, tint);
                let rect = Rect {
                    x: prow.table_left_pt + col_x[c],
                    y: prow.row_top_in_page,
                    w: col_x[c + 1] - col_x[c],
                    h: prow.height,
                };
                emit_rect(rect, paint, &mut pages[prow.target_page].list);
            }
        }
    }

    // Iterate physical rows × cells. For each physical row, find
    // the `<Cell>` entries whose template row matches and emit
    // them at the row's actual page-local coordinates. This naturally
    // handles header/footer replays — the same `<Cell>` definition
    // re-renders at the duplicated row's basis.
    //
    // Build a (col, template_row) → cell index map so the inner
    // loop is O(1) per cell rather than O(cells × physical_rows).
    let mut cell_by_origin: std::collections::HashMap<(u32, u32), &paged_model::TableCell> =
        std::collections::HashMap::with_capacity(table.cells.len());
    for cell in &table.cells {
        if let Some(coords) = cell.coords() {
            cell_by_origin.insert(coords, cell);
        }
    }
    // Whether any cell held more text than its row has room for.
    let mut cell_overset = false;
    for prow_i in 0..physical_rows.len() {
        let prow = physical_rows[prow_i];
        let r = prow.template_idx;
        for c in 0..col_widths.len() {
            // A merged cell owns every grid position its span reaches;
            // the positions it swallows are not drawn even when a
            // `<Cell>` still exists for them.
            if covered.contains(&(c as u32, r as u32)) {
                continue;
            }
            let Some(cell) = cell_by_origin.get(&(c as u32, r as u32)).copied() else {
                continue;
            };
            let target_page = prow.target_page;
            let cell_x_pt = prow.table_left_pt + col_x[c];
            let cell_y_pt = prow.row_top_in_page;
            let last_c = (c + cell.column_span.max(1) as usize).min(col_widths.len());
            // For row spans, accumulate heights of the contiguous
            // *physical* rows that sit in the same frame as this cell's
            // starting row. Spans that would straddle a frame boundary
            // clip to the originating frame's bottom (same conservative
            // policy as before). Walk physical rows starting at the
            // current physical row, advancing while their template_idx
            // is within `[r, r + span)` and `chain_idx` matches.
            let span_rows = cell.row_span.max(1) as usize;
            let mut cell_h_pt = 0.0f32;
            let mut step = 0usize;
            while step < span_rows && prow_i + step < physical_rows.len() {
                let next = &physical_rows[prow_i + step];
                if next.chain_idx != prow.chain_idx {
                    break;
                }
                // Only accumulate template rows in [r, r + span). A
                // continuation frame whose first row is a HeaderReplay
                // would otherwise add the header's height to the body
                // cell's span. The replay rows live in a *different*
                // physical row index, so we'd never reach them mid-span
                // anyway — but the explicit range guard makes this
                // robust if the physical-row sequence ever interleaves
                // replays differently.
                let t = next.template_idx;
                if t < r || t >= r + span_rows {
                    break;
                }
                cell_h_pt += next.height;
                step += 1;
            }
            if cell_h_pt <= 0.0 {
                cell_h_pt = prow.height;
            }
            let cell_w_pt = col_x[last_c] - col_x[c];

            // W3.A1 — retain this cell's page-local rect for hit-testing.
            // `r` is the template row (header / footer replays report their
            // source row, so a click on a replayed header resolves to the
            // original cell). Only addressable tables (those with a `Self`)
            // are recorded; others fall back to the frame-level hit.
            if let Some(table_id) = table.self_id.as_deref() {
                pages[target_page].cell_rects.push(CellRect {
                    story_id: em.current_story_id.clone(),
                    table_id: table_id.to_string(),
                    row: r as u32,
                    col: c as u32,
                    rect: [cell_x_pt, cell_y_pt, cell_w_pt, cell_h_pt],
                });
            }

            let Some((block, insets)) = cell_blocks.get(&(c as u32, r as u32)) else {
                continue;
            };
            let inner_left = cell_x_pt + insets.left;
            let inner_top = cell_y_pt + insets.top;
            let inner_w = (cell_w_pt - insets.left - insets.right).max(0.0);
            let inner_h = (cell_h_pt - insets.top - insets.bottom).max(0.0);

            // Resolve the cell's CellStyle. Per-cell AppliedCellStyle
            // wins; fall through to the table-style region default
            // (Header / Body / Footer / left or right column).
            let cell_style_id = cell
                .applied_cell_style
                .as_deref()
                .filter(|id| !is_none_style_id(id))
                .or_else(|| region_cell_style_for(c, r));
            let resolved_cell = cell_style_id
                .map(|id| em.document.styles.resolve_cell(id))
                .unwrap_or_default();

            // Cell fill — drawn before text so glyphs paint on top.
            // Inline FillColor on the <Cell> wins over the cascaded
            // cell-style fill — same precedence as the per-edge stroke
            // overrides above.
            let cell_fill_id = cell
                .fill_color
                .as_deref()
                .filter(|c| !is_none_swatch_id(c))
                .or(resolved_cell.fill_color.as_deref());
            if let Some(fill) =
                cell_fill_id.and_then(|id| color_id_to_paint(id, em.palette, em.color_ctx))
            {
                emit_rect(
                    Rect {
                        x: cell_x_pt,
                        y: cell_y_pt,
                        w: cell_w_pt,
                        h: cell_h_pt,
                    },
                    fill,
                    &mut pages[target_page].list,
                );
            }
            // Per-edge cell strokes. Each edge gets its own thin rect
            // (filled, since rect-stroke aligns to centerlines and we
            // want the edge to sit precisely on the cell boundary).
            // Per-cell overrides (declared inline on the <Cell> element)
            // win over the cascaded CellStyle — IDML serialises real row
            // dividers there even when AppliedCellStyle is `[None]`.
            let cell_top_color = cell
                .top_edge_stroke_color
                .as_deref()
                .filter(|c| !is_none_swatch_id(c))
                .or(resolved_cell.top_edge_stroke_color.as_deref());
            let cell_top_weight = cell
                .top_edge_stroke_weight
                .or(resolved_cell.top_edge_stroke_weight);
            let cell_bot_color = cell
                .bottom_edge_stroke_color
                .as_deref()
                .filter(|c| !is_none_swatch_id(c))
                .or(resolved_cell.bottom_edge_stroke_color.as_deref());
            let cell_bot_weight = cell
                .bottom_edge_stroke_weight
                .or(resolved_cell.bottom_edge_stroke_weight);
            let edges = [
                (
                    cell_top_color,
                    cell_top_weight,
                    cell.top_edge_stroke_tint,
                    cell_x_pt,
                    cell_y_pt,
                    cell_w_pt,
                ),
                (
                    cell_bot_color,
                    cell_bot_weight,
                    cell.bottom_edge_stroke_tint,
                    cell_x_pt,
                    cell_y_pt + cell_h_pt,
                    cell_w_pt,
                ),
            ];
            for (color, weight, tint, x, y, w) in edges {
                let (color_id, weight) = cell_edge_stroke(color, weight);
                if weight > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.color_ctx)
                        .map(|p| apply_fill_tint(p, tint))
                    {
                        emit_rect(
                            Rect {
                                x,
                                y: y - weight * 0.5,
                                w,
                                h: weight,
                            },
                            paint,
                            &mut pages[target_page].list,
                        );
                    }
                }
            }
            let cell_left_color = cell
                .left_edge_stroke_color
                .as_deref()
                .filter(|c| !is_none_swatch_id(c))
                .or(resolved_cell.left_edge_stroke_color.as_deref());
            let cell_left_weight = cell
                .left_edge_stroke_weight
                .or(resolved_cell.left_edge_stroke_weight);
            let cell_right_color = cell
                .right_edge_stroke_color
                .as_deref()
                .filter(|c| !is_none_swatch_id(c))
                .or(resolved_cell.right_edge_stroke_color.as_deref());
            let cell_right_weight = cell
                .right_edge_stroke_weight
                .or(resolved_cell.right_edge_stroke_weight);
            let v_edges = [
                (
                    cell_left_color,
                    cell_left_weight,
                    cell.left_edge_stroke_tint,
                    cell_x_pt,
                    cell_y_pt,
                    cell_h_pt,
                ),
                (
                    cell_right_color,
                    cell_right_weight,
                    cell.right_edge_stroke_tint,
                    cell_x_pt + cell_w_pt,
                    cell_y_pt,
                    cell_h_pt,
                ),
            ];
            for (color, weight, tint, x, y, h) in v_edges {
                let (color_id, weight) = cell_edge_stroke(color, weight);
                if weight > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.color_ctx)
                        .map(|p| apply_fill_tint(p, tint))
                    {
                        emit_rect(
                            Rect {
                                x: x - weight * 0.5,
                                y,
                                w: weight,
                                h,
                            },
                            paint,
                            &mut pages[target_page].list,
                        );
                    }
                }
            }

            // Diagonal cell strokes. IDML's "Left" diagonal goes
            // top-left → bottom-right; "Right" goes top-right →
            // bottom-left, each with its own colour / weight / tint. The
            // `DiagonalLineInFront` flag decides the paint order relative
            // to the cell content: when true the diagonal lands AFTER the
            // glyphs (drawn over them); otherwise it sits behind, on top
            // of the fill but under the text. We capture the closure here
            // and invoke it at the chosen point below.
            let diag = &cell.diagonal;
            let emit_diagonals = |em: &StoryEmitter, pages: &mut [BuiltPage]| {
                let one = |drawn: Option<bool>,
                           color: Option<&str>,
                           weight: Option<f32>,
                           tint: Option<f32>,
                           (x1, y1): (f32, f32),
                           (x2, y2): (f32, f32),
                           pages: &mut [BuiltPage]| {
                    if drawn != Some(true) {
                        return;
                    }
                    let Some(weight) = weight.filter(|w| *w > 0.0) else {
                        return;
                    };
                    let Some(color_id) = color else {
                        return;
                    };
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.color_ctx)
                        .map(|p| apply_fill_tint(p, tint))
                    {
                        paged_compose::emit_line(
                            x1,
                            y1,
                            x2,
                            y2,
                            Stroke::new(weight),
                            paint,
                            &mut pages[target_page].list,
                        );
                    }
                };
                one(
                    diag.left_line_drawn,
                    diag.left_line_color.as_deref(),
                    diag.left_line_weight,
                    diag.left_line_tint,
                    (cell_x_pt, cell_y_pt),
                    (cell_x_pt + cell_w_pt, cell_y_pt + cell_h_pt),
                    pages,
                );
                one(
                    diag.right_line_drawn,
                    diag.right_line_color.as_deref(),
                    diag.right_line_weight,
                    diag.right_line_tint,
                    (cell_x_pt + cell_w_pt, cell_y_pt),
                    (cell_x_pt, cell_y_pt + cell_h_pt),
                    pages,
                );
            };
            let diagonal_in_front = diag.diagonal_in_front == Some(true);
            if !diagonal_in_front {
                emit_diagonals(em, pages);
            }

            // W1.13 — only ADDRESSABLE tables (those with a `<Table
            // Self>`) get a cell qualifier for caret/text editing; this
            // matches the `cell_rects` hit-test surface exactly (built
            // above under the same `table.self_id` guard). `r` is the
            // template row, so a caret in a replayed header row maps to
            // the source cell's text — same convention as `CellRect`.
            // The cell-local paragraph index is the enumeration index
            // over `cell.paragraphs` (including any nested-table /
            // empty paragraphs) so it lines up byte-for-byte with the
            // wire-side `locate` walk over `cell.paragraphs`.
            let cell_addr_base: Option<CellAddr> = table.self_id.as_deref().map(|tid| CellAddr {
                table_id: tid.to_string(),
                row: r as u32,
                col: c as u32,
            });
            // W1.11a — cell vertical justification, decided BEFORE drawing
            // from the measured block, so glyphs and caret lines land
            // together. Precedence mirrors every other per-cell knob: an
            // INLINE `<Cell VerticalJustification="…">` wins over the
            // cascaded CellStyle value. The block InDesign moves runs from
            // the first line's ascent to the last baseline — the last
            // paragraph's space after left out (2026-10-02, `tables-rows`
            // page 7):
            //   * CenterAlign → centre the block in the slack,
            //   * BottomAlign → push it to the bottom inset,
            //   * JustifyAlign → distribute the slack BETWEEN items (not
            //     inside a paragraph — that would distort leading); with
            //     fewer than two there is nothing to distribute: Top.
            //   * TopAlign / absent → no shift.
            let (used_h, overset) = block.fitted(inner_h);
            if overset {
                cell_overset = true;
            }
            let fitting_items = block.fitting_items(inner_h);
            let cell_vjust = cell
                .vertical_justification
                .as_deref()
                .or(resolved_cell.vertical_justification.as_deref());
            let slack = if used_h > 0.0 {
                (inner_h - used_h).max(0.0)
            } else {
                0.0
            };
            let (dy, gap) = match cell_vjust {
                Some("CenterAlign") => (slack * 0.5, 0.0),
                Some("BottomAlign") => (slack, 0.0),
                Some("JustifyAlign") if fitting_items >= 2 => {
                    (0.0, slack / (fitting_items - 1) as f32)
                }
                _ => (0.0, 0.0),
            };
            let mut emitted_extents: Vec<(usize, usize)> = Vec::new();
            for (k, item) in block.items.iter().take(fitting_items).enumerate() {
                let shift = dy + gap * k as f32;
                let cmd_start = pages[target_page].list.commands.len();
                match item {
                    CellItem::Text {
                        paragraph,
                        index,
                        origin,
                        by_leading,
                        ..
                    } => emit_cell_paragraph(
                        em,
                        paragraph,
                        target_page,
                        (inner_left, inner_top),
                        inner_w,
                        origin + shift,
                        pages,
                        total_stats,
                        cell_addr_base.as_ref().map(|addr| (addr, *index as u32)),
                        *by_leading,
                        inner_h - origin,
                    ),
                    // Phase 5 — nested table inside a cell paragraph, laid
                    // out at its measured top; further nesting recurses
                    // through emit_nested_table_inline.
                    CellItem::Table { table, top, .. } => {
                        emit_nested_table_inline(
                            em,
                            table,
                            inner_left,
                            inner_top + top + shift,
                            inner_w,
                            target_page,
                            pages,
                            total_stats,
                        );
                    }
                }
                let cmd_end = pages[target_page].list.commands.len();
                if cmd_end > cmd_start {
                    emitted_extents.push((cmd_start, cmd_end));
                }
            }
            // Cell RotationAngle: rotate the (already vertically-justified)
            // content about the cell's centre. Borders / fills are emitted
            // outside `emitted_extents`, so they stay unrotated — matching
            // InDesign, which rotates only the cell's content. Cardinal
            // angles (90/180/270) are the real-world case; arbitrary angles
            // rotate too but may need content re-fit (a follow-up).
            let cell_rotation = cell.rotation_angle.or(resolved_cell.rotation_angle);
            if let Some(deg) = cell_rotation.filter(|a| a.abs() > f32::EPSILON) {
                let cx = cell_x_pt + cell_w_pt * 0.5;
                let cy = cell_y_pt + cell_h_pt * 0.5;
                let rot = Transform::translate(cx, cy)
                    .compose(&Transform::rotate_deg(deg))
                    .compose(&Transform::translate(-cx, -cy));
                for (s, e) in &emitted_extents {
                    for cmd in &mut pages[target_page].list.commands[*s..*e] {
                        let t = cmd.transform_mut();
                        *t = rot.compose(t);
                    }
                }
            }
            // `DiagonalLineInFront` → paint the diagonal(s) over the
            // (now vertically-justified / rotated) cell content.
            if diagonal_in_front {
                emit_diagonals(em, pages);
            }
        } // close inner `for c in 0..col_widths.len()`
    } // close outer `for prow_i in 0..physical_rows.len()`
      // A cell whose text does not fit its row — a `MaximumHeight` cap —
      // draws the lines that fit and none of the rest, as InDesign does;
      // say so rather than drop them silently.
    if cell_overset {
        let mut d = crate::diagnostics::Diagnostic::new(
            crate::diagnostics::DiagnosticCode::OversetTextDropped,
            "table cell text overflows its row; the lines that do not fit are not drawn (overset cell)",
        )
        .with_page(target_page);
        if !em.current_story_id.is_empty() {
            d = d.with_story(em.current_story_id.clone());
        }
        em.diagnostics.push(d);
    }

    // Resolve effective outer-border attributes. Direct `<Table>`
    // attributes (e.g. `LeftBorderStrokeColor` on the `<Table>`
    // element itself) win over the AppliedTableStyle's cascaded
    // values; weight defaults to 1pt when both are absent and a
    // colour is present.
    let direct = &table.border;
    let effective_color = |direct: Option<&str>, style: Option<&str>| -> Option<String> {
        match direct {
            Some(s) if !is_none_swatch_id(s) => Some(s.to_string()),
            _ => style.map(|s| s.to_string()),
        }
    };
    let effective_weight = |direct_w: Option<f32>, style_w: Option<f32>, has_color: bool| -> f32 {
        if let Some(w) = direct_w {
            return w;
        }
        if let Some(w) = style_w {
            return w;
        }
        if has_color {
            1.0
        } else {
            0.0
        }
    };
    let top_color = effective_color(
        direct.top_color.as_deref(),
        resolved_table.top_border_stroke_color.as_deref(),
    );
    let top_weight = effective_weight(
        direct.top_weight,
        resolved_table.top_border_stroke_weight,
        top_color.is_some(),
    );
    let top_type = direct.top_type.clone();
    let bot_color = effective_color(
        direct.bottom_color.as_deref(),
        resolved_table.bottom_border_stroke_color.as_deref(),
    );
    let bot_weight = effective_weight(
        direct.bottom_weight,
        resolved_table.bottom_border_stroke_weight,
        bot_color.is_some(),
    );
    let bot_type = direct.bottom_type.clone();
    let left_color = effective_color(
        direct.left_color.as_deref(),
        resolved_table.left_border_stroke_color.as_deref(),
    );
    let left_weight = effective_weight(
        direct.left_weight,
        resolved_table.left_border_stroke_weight,
        left_color.is_some(),
    );
    let left_type = direct.left_type.clone();
    let right_color = effective_color(
        direct.right_color.as_deref(),
        resolved_table.right_border_stroke_color.as_deref(),
    );
    let right_weight = effective_weight(
        direct.right_weight,
        resolved_table.right_border_stroke_weight,
        right_color.is_some(),
    );
    let right_type = direct.right_type.clone();

    // Row separators between rows. IDML serialises divider styles
    // via `StartRowStrokeType` / `EndRowStrokeType` on the `<Table>`.
    // The first `start_count` row separators use the start-stroke
    // style; subsequent dividers fall through to the end-stroke
    // style (alternating). When `start_color` is absent but a type
    // is declared we fall back to black — IDML's documented default.
    let row_decl = &table.row_strokes;
    let row_start_type = row_decl.start_type.clone();
    let row_start_color_raw = row_decl.start_color.clone();
    let has_row_decl = row_start_type.is_some()
        || row_start_color_raw.is_some()
        || row_decl.end_type.is_some()
        || row_decl.end_color.is_some();
    let row_start_color = if has_row_decl && row_start_color_raw.is_none() {
        Some("Color/Black".to_string())
    } else {
        row_start_color_raw
    };
    let row_start_weight = row_decl
        .start_weight
        .unwrap_or(if has_row_decl { 1.0 } else { 0.0 });
    let row_end_type = row_decl.end_type.clone().or_else(|| row_start_type.clone());
    let row_end_color = row_decl
        .end_color
        .clone()
        .or_else(|| row_start_color.clone());
    let row_end_weight = row_decl.end_weight.unwrap_or(row_start_weight);
    let row_start_count = row_decl.start_count.unwrap_or(0) as usize;
    let row_end_count = row_decl.end_count.unwrap_or(0) as usize;
    let row_cycle = row_start_count + row_end_count;
    let pick_row_stroke = |i: usize| -> (Option<&str>, Option<&str>, f32) {
        if row_cycle == 0 {
            return (
                row_start_type.as_deref(),
                row_start_color.as_deref(),
                row_start_weight,
            );
        }
        let pos = i % row_cycle;
        if pos < row_start_count {
            (
                row_start_type.as_deref(),
                row_start_color.as_deref(),
                row_start_weight,
            )
        } else {
            (
                row_end_type.as_deref(),
                row_end_color.as_deref(),
                row_end_weight,
            )
        }
    };

    // Interior column dividers. IDML serialises these via
    // `Start/EndColumnStroke*` on the `<Table>` — previously only the
    // per-cell left/right edges were drawn, so a table-style column
    // divider rendered nothing. A divider sits at the left edge of
    // columns 1..N (the interior boundaries), spanning each frame the
    // table touches. Emitted BEFORE the row dividers so the horizontal
    // row strokes paint over them at crossings — InDesign's default
    // cell-stroke precedence. (True `StrokeOrder` honouring is a
    // queued follow-up.)
    let col_decl = resolve_table_line_strokes(&table.column_strokes);
    for (_chain_idx, fp_target_page, frame_table_left, top_y, bottom_y) in frame_extents.iter() {
        let segment_h = bottom_y - top_y;
        if segment_h <= 0.0 {
            continue;
        }
        for c in 1..col_widths.len() {
            let (stype, scolor, sweight) = col_decl.pick(c - 1);
            let Some(color_id) = scolor else { continue };
            if sweight <= 0.0 {
                continue;
            }
            let Some(paint) = color_id_to_paint(color_id, em.palette, em.color_ctx) else {
                continue;
            };
            emit_table_vertical_edge(
                *frame_table_left + col_x[c],
                *top_y,
                segment_h,
                stype,
                sweight,
                paint,
                &mut pages[*fp_target_page].list,
            );
        }
    }

    // Emit row dividers. A divider sits at the bottom edge of a
    // physical row when the next physical row sits in the same
    // frame. The dividing-stroke pick still cycles via the *template*
    // index so replayed header / footer rows match the original
    // dividers (visually consistent across continuation frames).
    for i in 0..physical_rows.len().saturating_sub(1) {
        let curr = physical_rows[i];
        let next = physical_rows[i + 1];
        if curr.chain_idx != next.chain_idx {
            continue;
        }
        let (stype, scolor, sweight) = pick_row_stroke(curr.template_idx);
        let Some(color_id) = scolor else { continue };
        if sweight <= 0.0 {
            continue;
        }
        let Some(paint) = color_id_to_paint(color_id, em.palette, em.color_ctx) else {
            continue;
        };
        let y = curr.row_top_in_page + curr.height;
        emit_table_horizontal_edge(
            curr.table_left_pt,
            y,
            total_w,
            stype,
            sweight,
            paint,
            &mut pages[curr.target_page].list,
        );
    }

    // Table-level borders, drawn per-frame so a threaded table
    // gets a top border at the start of the first frame, a bottom
    // border at the end of the last frame, and full left/right
    // borders inside every frame the table touches.
    for (i, (_chain_idx, fp_target_page, frame_table_left, top_y, bottom_y)) in
        frame_extents.iter().enumerate()
    {
        let is_first = i == 0;
        let is_last = i == frame_extents.len() - 1;
        let target = *fp_target_page;
        if is_first {
            if let Some(color_id) = top_color.as_deref() {
                if top_weight > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.color_ctx) {
                        emit_table_horizontal_edge(
                            *frame_table_left,
                            *top_y,
                            total_w,
                            top_type.as_deref(),
                            top_weight,
                            paint,
                            &mut pages[target].list,
                        );
                    }
                }
            }
        }
        if is_last {
            if let Some(color_id) = bot_color.as_deref() {
                if bot_weight > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.color_ctx) {
                        emit_table_horizontal_edge(
                            *frame_table_left,
                            *bottom_y,
                            total_w,
                            bot_type.as_deref(),
                            bot_weight,
                            paint,
                            &mut pages[target].list,
                        );
                    }
                }
            }
        }
        // Left/right borders span this frame's portion of the table.
        let segment_h = bottom_y - top_y;
        if let Some(color_id) = left_color.as_deref() {
            if left_weight > 0.0 {
                if let Some(paint) = color_id_to_paint(color_id, em.palette, em.color_ctx) {
                    emit_table_vertical_edge(
                        *frame_table_left,
                        *top_y,
                        segment_h,
                        left_type.as_deref(),
                        left_weight,
                        paint,
                        &mut pages[target].list,
                    );
                }
            }
        }
        if let Some(color_id) = right_color.as_deref() {
            if right_weight > 0.0 {
                if let Some(paint) = color_id_to_paint(color_id, em.palette, em.color_ctx) {
                    emit_table_vertical_edge(
                        *frame_table_left + total_w,
                        *top_y,
                        segment_h,
                        right_type.as_deref(),
                        right_weight,
                        paint,
                        &mut pages[target].list,
                    );
                }
            }
        }
    }

    // Advance the active frame_idx + y_cursor to the row after the
    // last one we placed. The host emitter loop reads em.frame_idx
    // and em.y_cursor when continuing the surrounding paragraph
    // flow.
    em.frame_idx = final_chain_idx;
    em.y_cursor = ((final_y_in_frame + em.options.default_point_size * 0.8)
        * paged_text::shape::ADVANCE_PRECISION)
        .round() as i32;
    total_stats.paragraphs += 1;
    let stat_page = em.chain_pages[em.frame_idx];
    pages[stat_page].stats.paragraphs += 1;
}

/// Every grid position a merged cell swallows.
///
/// IDML's own rule is that a covered position carries no `<Cell>` at
/// all — "the covered grid positions have no `<Cell>` element of their
/// own, they are absorbed by the spanning cell". A table that arrives
/// through the mutation wire breaks that: `setCellSpan` widens the
/// spanning cell and leaves the cell it now covers in place, so without
/// this mask the covered cell keeps painting its own fill, edges and
/// text at its own origin and a merge renders as no merge at all.
/// Deriving the mask from the spans rather than trusting the cell list
/// makes both shapes render the same, whoever produced them.
/// InDesign's built-in cell-edge stroke: **1 pt black**, applied when
/// neither the cell nor its table style says otherwise.
///
/// A `<Cell>` with no `*EdgeStroke*` attributes is not a cell without
/// borders — it is a cell that inherits the default, and InDesign draws
/// it. Requiring both a colour AND a weight to be present before
/// drawing anything meant every table whose IDML omitted them rendered
/// borderless: measured 2026-09-07, InDesign's own export of the
/// `tables` fixture carries 4647 ink on page 1 against our 1800, and
/// the fixture passed only because it holds the corpus's loosest
/// thresholds. Adobe's own files spell the attributes 71-85% of the
/// time, which is why this stayed hidden — the corpus packs mostly say
/// it out loud, and our two writers did not.
///
/// An explicit `0` weight still draws nothing: the caller's `> 0.0`
/// test is what turns a stroke off, and that is InDesign's spelling for
/// "no border" too.
const DEFAULT_CELL_EDGE_WEIGHT: f32 = 1.0;

fn cell_edge_stroke(color: Option<&str>, weight: Option<f32>) -> (&str, f32) {
    (
        color.unwrap_or("Color/Black"),
        weight.unwrap_or(DEFAULT_CELL_EDGE_WEIGHT),
    )
}

/// How far InDesign insets a table's content from its frame's top-left:
/// half the outer border weight on each side.
///
/// InDesign keeps the outer border INSIDE the frame — the stroke is
/// centred on the table's boundary, and the boundary is moved in by
/// half a stroke so the outer half lands exactly on the frame edge.
/// Everything inherits the shift, rules and text alike.
///
/// Measured at two weights so the rule is not a coincidence
/// (2026-09-07, `tables-overset` pages 1 and 6, InDesign 20.0.1 at
/// 600 dpi): with a 1 pt border InDesign puts the first row's text at
/// 128.640 pt where we put it at 128.160; with a 4 pt border, at
/// 130.080 against the same 128.160. Offsets of 0.48 and 1.92 pt —
/// half the weight both times.
///
/// An edge that draws nothing contributes nothing: an explicit `0`
/// weight, or a `Swatch/None` colour, leaves the table flush.
fn outer_border_offsets(table: &paged_model::Table) -> (f32, f32) {
    let drawn = |color: Option<&str>, weight: Option<f32>| -> f32 {
        if color.is_some_and(is_none_swatch_id) {
            return 0.0;
        }
        weight.unwrap_or(DEFAULT_CELL_EDGE_WEIGHT).max(0.0)
    };
    let (mut top, mut left) = (0.0f32, 0.0f32);
    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else {
            continue;
        };
        if r == 0 {
            top = top.max(drawn(
                cell.top_edge_stroke_color.as_deref(),
                cell.top_edge_stroke_weight,
            ));
        }
        if c == 0 {
            left = left.max(drawn(
                cell.left_edge_stroke_color.as_deref(),
                cell.left_edge_stroke_weight,
            ));
        }
    }
    (top * 0.5, left * 0.5)
}

fn covered_grid_positions(table: &paged_model::Table) -> std::collections::HashSet<(u32, u32)> {
    // Spans are clamped to the grid before they are walked: nothing in
    // the format stops a `<Cell ColumnSpan="4000000000">`, and a span
    // taken at its word would be a hang rather than a wrong picture.
    let cols = table.columns.len().max(table.column_count as usize) as u32;
    let rows = table.rows.len() as u32;
    let mut covered = std::collections::HashSet::new();
    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else {
            continue;
        };
        let last_c = c.saturating_add(cell.column_span.max(1)).min(cols);
        let last_r = r.saturating_add(cell.row_span.max(1)).min(rows);
        for dc in c..last_c {
            for dr in r..last_r {
                if (dc, dr) == (c, r) {
                    continue; // the span's own origin still draws
                }
                covered.insert((dc, dr));
            }
        }
    }
    covered
}

/// Resolved row / column divider stroke decl: the start/end style
/// alternation IDML serialises via `Start*StrokeType` /
/// `End*StrokeType` + counts. Shared by the row-divider and the
/// column-divider emit so both honour the same "black default when a
/// type is declared without a colour" + alternation rules.
struct ResolvedLineStroke {
    start_type: Option<String>,
    start_color: Option<String>,
    start_weight: f32,
    end_type: Option<String>,
    end_color: Option<String>,
    end_weight: f32,
    start_count: usize,
    end_count: usize,
}

fn resolve_table_line_strokes(decl: &paged_model::TableLineStrokes) -> ResolvedLineStroke {
    let start_type = decl.start_type.clone();
    let start_color_raw = decl.start_color.clone();
    let has_decl = start_type.is_some()
        || start_color_raw.is_some()
        || decl.end_type.is_some()
        || decl.end_color.is_some();
    // A declared type with no colour means black (IDML's documented
    // default), matching the row-divider behaviour.
    let start_color = if has_decl && start_color_raw.is_none() {
        Some("Color/Black".to_string())
    } else {
        start_color_raw
    };
    let start_weight = decl
        .start_weight
        .unwrap_or(if has_decl { 1.0 } else { 0.0 });
    ResolvedLineStroke {
        end_type: decl.end_type.clone().or_else(|| start_type.clone()),
        end_color: decl.end_color.clone().or_else(|| start_color.clone()),
        end_weight: decl.end_weight.unwrap_or(start_weight),
        start_type,
        start_color,
        start_weight,
        start_count: decl.start_count.unwrap_or(0) as usize,
        end_count: decl.end_count.unwrap_or(0) as usize,
    }
}

impl ResolvedLineStroke {
    /// Pick the (type, color, weight) for the i-th divider, cycling
    /// `start_count` start-styled then `end_count` end-styled.
    fn pick(&self, i: usize) -> (Option<&str>, Option<&str>, f32) {
        let cycle = self.start_count + self.end_count;
        if cycle == 0 || (i % cycle) < self.start_count {
            (
                self.start_type.as_deref(),
                self.start_color.as_deref(),
                self.start_weight,
            )
        } else {
            (
                self.end_type.as_deref(),
                self.end_color.as_deref(),
                self.end_weight,
            )
        }
    }
}

/// Strip `StrokeStyle/$ID/` and an optional leading `Canned ` so the
/// remaining suffix matches the canonical stroke-style name table.
/// Mirrors `stroke_for`'s normalisation for the table-edge emitter.
fn normalise_stroke_type(name: Option<&str>) -> &str {
    let Some(name) = name else { return "Solid" };
    let suffix = name.strip_prefix("StrokeStyle/$ID/").unwrap_or(name);
    suffix.strip_prefix("Canned ").unwrap_or(suffix)
}

/// Emit a horizontal table-edge segment of length `length` starting
/// at `(x, y)` (the centre of the edge, snapped to the cell boundary).
/// Honours a small set of stroke types:
///
/// * `Solid` / unknown → single filled rect of height `weight`.
/// * `ThickThick` → two parallel rects each of height `weight/3`,
///   separated by a `weight/3` gap; the trio spans `weight` total
///   (matches InDesign's preset).
/// * `Dotted` / `Dotted2..8` / `Japanese Dots` → a series of small
///   filled circles of diameter `weight` stamped along the edge.
fn emit_table_horizontal_edge(
    x: f32,
    y: f32,
    length: f32,
    stroke_type: Option<&str>,
    weight: f32,
    paint: Paint,
    list: &mut DisplayList,
) {
    if weight <= 0.0 || length <= 0.0 {
        return;
    }
    let kind = normalise_stroke_type(stroke_type);
    match kind {
        "ThickThick" => {
            let line_w = weight / 3.0;
            let upper_centre = y - weight / 3.0;
            let lower_centre = y + weight / 3.0;
            emit_rect(
                Rect {
                    x,
                    y: upper_centre - line_w * 0.5,
                    w: length,
                    h: line_w,
                },
                paint,
                list,
            );
            emit_rect(
                Rect {
                    x,
                    y: lower_centre - line_w * 0.5,
                    w: length,
                    h: line_w,
                },
                paint,
                list,
            );
        }
        "Dotted" | "Dotted2" | "Dotted4" | "Dotted8" | "Japanese Dots" => {
            let step = match kind {
                "Dotted2" | "Dotted" => 2.0,
                "Dotted4" => 4.0,
                "Dotted8" => 8.0,
                _ => 1.5,
            } * weight.max(0.1);
            let diameter = weight;
            let mut cx = x;
            while cx <= x + length + 0.001 {
                emit_ellipse(
                    Rect {
                        x: cx - diameter * 0.5,
                        y: y - diameter * 0.5,
                        w: diameter,
                        h: diameter,
                    },
                    paint,
                    list,
                );
                cx += step;
            }
        }
        _ => {
            emit_rect(
                Rect {
                    x,
                    y: y - weight * 0.5,
                    w: length,
                    h: weight,
                },
                paint,
                list,
            );
        }
    }
}

/// Vertical analogue of [`emit_table_horizontal_edge`]. `x` is the
/// horizontal centre of the edge; the segment spans `(y, y + length)`.
fn emit_table_vertical_edge(
    x: f32,
    y: f32,
    length: f32,
    stroke_type: Option<&str>,
    weight: f32,
    paint: Paint,
    list: &mut DisplayList,
) {
    if weight <= 0.0 || length <= 0.0 {
        return;
    }
    let kind = normalise_stroke_type(stroke_type);
    match kind {
        "ThickThick" => {
            let line_w = weight / 3.0;
            let left_centre = x - weight / 3.0;
            let right_centre = x + weight / 3.0;
            emit_rect(
                Rect {
                    x: left_centre - line_w * 0.5,
                    y,
                    w: line_w,
                    h: length,
                },
                paint,
                list,
            );
            emit_rect(
                Rect {
                    x: right_centre - line_w * 0.5,
                    y,
                    w: line_w,
                    h: length,
                },
                paint,
                list,
            );
        }
        "Dotted" | "Dotted2" | "Dotted4" | "Dotted8" | "Japanese Dots" => {
            let step = match kind {
                "Dotted2" | "Dotted" => 2.0,
                "Dotted4" => 4.0,
                "Dotted8" => 8.0,
                _ => 1.5,
            } * weight.max(0.1);
            let diameter = weight;
            let mut cy = y;
            while cy <= y + length + 0.001 {
                emit_ellipse(
                    Rect {
                        x: x - diameter * 0.5,
                        y: cy - diameter * 0.5,
                        w: diameter,
                        h: diameter,
                    },
                    paint,
                    list,
                );
                cy += step;
            }
        }
        _ => {
            emit_rect(
                Rect {
                    x: x - weight * 0.5,
                    y,
                    w: weight,
                    h: length,
                },
                paint,
                list,
            );
        }
    }
}

/// Phase 5 — emit a nested table inside a cell's content area.
///
/// Unlike [`emit_table_into_chain`] this version doesn't thread the
/// table across frames or replay header/footer rows — a nested table
/// lives entirely inside ONE outer cell, so all the chain-aware
/// machinery is unnecessary. The simpler shape:
///
/// 1. Compute column widths from `table.columns`. Scale to fit
///    `max_width_pt` if the declared widths exceed it.
/// 2. Pre-measure every cell to derive content-driven row heights.
/// 3. Walk cells; for each, compute its rect within the table, then
///    route paragraphs through `emit_cell_paragraph` at the cell's
///    inner origin (text inset applied). A 0.5pt black border
///    outlines each cell so the nested table reads visibly even
///    without a fully resolved cell style.
///
/// Returns the total height consumed in pt so callers can advance
/// the cell-paragraph cursor by it (mirrors `emit_cell_paragraph`'s
/// return convention).
///
/// Honoured today:
/// - per-column `SingleColumnWidth` (with proportional scaling when
///   declared widths overflow `max_width_pt`),
/// - per-row `SingleRowHeight` / `MinimumHeight` / `MaximumHeight`
///   (max-row growth from cell content),
/// - per-cell `text_top_inset` / `text_left_inset` / ... (text
///   insets honored at emit),
/// - all cell paragraphs (including their nested character styles,
///   tab leaders, conditional text, etc. — by routing through the
///   existing `emit_cell_paragraph`).
///
/// Deferred:
/// - cell fill / cell border styling from `AppliedCellStyle` (uses
///   a simple 0.5pt grid for visibility),
/// - row/column spans (each cell occupies one row × one column;
///   spans get clamped to 1),
/// - diagonals, alternating-row fills, custom strokes,
/// - RowSpan / ColumnSpan layout (treats every cell as 1×1).
#[allow(clippy::too_many_arguments)]
fn emit_nested_table_inline(
    em: &mut StoryEmitter,
    table: &paged_model::Table,
    origin_x: f32,
    origin_y: f32,
    max_width_pt: f32,
    target_page: usize,
    pages: &mut [BuiltPage],
    total_stats: &mut PipelineStats,
) -> f32 {
    if table.cells.is_empty() || table.columns.is_empty() {
        return 0.0;
    }
    let declared_widths: Vec<f32> = table
        .columns
        .iter()
        .map(|c| c.single_column_width.unwrap_or(0.0).max(0.0))
        .collect();
    let declared_total: f32 = declared_widths.iter().sum();
    // Scale columns to fit `max_width_pt` when the declared widths
    // exceed it. Equal-width fallback when all declared widths are
    // zero (a degenerate IDML that didn't carry SingleColumnWidth).
    let col_widths: Vec<f32> = if declared_total <= 0.0 {
        let n = declared_widths.len() as f32;
        vec![max_width_pt / n; declared_widths.len()]
    } else if declared_total > max_width_pt && max_width_pt > 0.0 {
        let scale = max_width_pt / declared_total;
        declared_widths.iter().map(|w| w * scale).collect()
    } else {
        declared_widths
    };
    let mut col_x: Vec<f32> = Vec::with_capacity(col_widths.len() + 1);
    let mut acc = 0.0f32;
    col_x.push(0.0);
    for w in &col_widths {
        acc += *w;
        col_x.push(acc);
    }
    let total_rows = table.rows.len();
    if total_rows == 0 {
        return 0.0;
    }
    // Initial row heights from the IDML's row attributes (the same
    // max-of-SingleRowHeight-MinimumHeight default as the chain
    // emitter uses).
    let mut row_heights: Vec<f32> = table.rows.iter().map(row_floor_pt).collect();
    // Pre-measure every cell so row heights can grow to fit content.
    // Spans are clamped to 1 here — proper span layout is a follow-up.
    let mut blocks: Vec<Option<CellBlock>> = Vec::with_capacity(table.cells.len());
    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else {
            blocks.push(None);
            continue;
        };
        let (cu, ru) = (c as usize, r as usize);
        if cu >= col_widths.len() || ru >= total_rows {
            blocks.push(None);
            continue;
        }
        let inner_w = (col_widths[cu] - cell.text_left_inset - cell.text_right_inset).max(0.0);
        let block = plan_cell_block(em, &cell.paragraphs, inner_w);
        let insets = cell.text_top_inset + cell.text_bottom_inset;
        let clamp = table.rows.get(ru).map_or(f32::INFINITY, row_cap_pt);
        let required = block.fitted(clamp - insets).0 + insets;
        row_heights[ru] = row_heights[ru].max(required).min(clamp);
        blocks.push(Some(block));
    }
    let mut row_y: Vec<f32> = Vec::with_capacity(total_rows + 1);
    let mut yacc = 0.0f32;
    row_y.push(0.0);
    for h in &row_heights {
        yacc += *h;
        row_y.push(yacc);
    }
    let table_h = *row_y.last().unwrap_or(&0.0);
    let total_w = *col_x.last().unwrap_or(&0.0);

    // Emit a thin border grid as a placeholder so the nested table
    // is visible even without a resolved cell style. Replaces the
    // styled-stroke pass the chain emitter does — that's the
    // follow-up.
    const GRID_W: f32 = 0.5;
    let grid_paint = paged_compose::Paint::Solid(paged_compose::Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    // Horizontal lines (top of each row + bottom of last row).
    for &y in &row_y {
        emit_rect(
            Rect {
                x: origin_x,
                y: origin_y + y - GRID_W * 0.5,
                w: total_w,
                h: GRID_W,
            },
            grid_paint,
            &mut pages[target_page].list,
        );
    }
    // Vertical lines (left of each column + right of last column).
    for &x in &col_x {
        emit_rect(
            Rect {
                x: origin_x + x - GRID_W * 0.5,
                y: origin_y,
                w: GRID_W,
                h: table_h,
            },
            grid_paint,
            &mut pages[target_page].list,
        );
    }

    // Emit cell content.
    for (cell, block) in table.cells.iter().zip(&blocks) {
        let (Some((c, r)), Some(block)) = (cell.coords(), block.as_ref()) else {
            continue;
        };
        let (cu, ru) = (c as usize, r as usize);
        let cell_x_pt = origin_x + col_x[cu];
        let cell_y_pt = origin_y + row_y[ru];
        let cell_w_pt = col_widths[cu];
        let cell_h_pt = row_heights[ru];
        let inner_left = cell_x_pt + cell.text_left_inset;
        let inner_top = cell_y_pt + cell.text_top_inset;
        let inner_w = (cell_w_pt - cell.text_left_inset - cell.text_right_inset).max(0.0);
        let inner_h = (cell_h_pt - cell.text_top_inset - cell.text_bottom_inset).max(0.0);
        for item in block.items.iter().take(block.fitting_items(inner_h)) {
            match item {
                // Double-nested table — recurse.
                CellItem::Table { table, top, .. } => {
                    emit_nested_table_inline(
                        em,
                        table,
                        inner_left,
                        inner_top + top,
                        inner_w,
                        target_page,
                        pages,
                        total_stats,
                    );
                }
                // W1.13 defer — nested-table cells pass `None`: text inside
                // a table that is itself nested in a cell is laid out and
                // rendered, but not separately caret-addressable. The wire
                // address only reaches top-level tables (`cell_rects` are
                // likewise only built for the outer table). Dated defer:
                // 2026-06-07 — nested-cell text editing.
                CellItem::Text {
                    paragraph,
                    origin,
                    by_leading,
                    ..
                } => emit_cell_paragraph(
                    em,
                    paragraph,
                    target_page,
                    (inner_left, inner_top),
                    inner_w,
                    *origin,
                    pages,
                    total_stats,
                    None,
                    *by_leading,
                    inner_h - origin,
                ),
            }
        }
    }
    table_h
}

/// Phase 5 — measurement counterpart to [`emit_nested_table_inline`].
/// Returns the total height a nested table would consume given a
/// containing column width. Caller uses this in the outer table's
/// row-height pre-measure pass so a nested table can grow its host
/// row appropriately.
///
/// Heuristic — sums each row's height after applying the
/// `SingleRowHeight` / `MinimumHeight` default, then growing rows
/// whose cells host content (or nested-nested tables) measured at
/// the same per-column inner width the emit pass would see.
fn measure_nested_table_height(
    em: &StoryEmitter,
    table: &paged_model::Table,
    max_width_pt: f32,
) -> f32 {
    if table.cells.is_empty() || table.columns.is_empty() || table.rows.is_empty() {
        return 0.0;
    }
    let declared_widths: Vec<f32> = table
        .columns
        .iter()
        .map(|c| c.single_column_width.unwrap_or(0.0).max(0.0))
        .collect();
    let declared_total: f32 = declared_widths.iter().sum();
    let col_widths: Vec<f32> = if declared_total <= 0.0 {
        let n = declared_widths.len() as f32;
        vec![max_width_pt / n; declared_widths.len()]
    } else if declared_total > max_width_pt && max_width_pt > 0.0 {
        let scale = max_width_pt / declared_total;
        declared_widths.iter().map(|w| w * scale).collect()
    } else {
        declared_widths
    };
    let total_rows = table.rows.len();
    let mut row_heights: Vec<f32> = table.rows.iter().map(row_floor_pt).collect();
    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else {
            continue;
        };
        let (cu, ru) = (c as usize, r as usize);
        if cu >= col_widths.len() || ru >= total_rows {
            continue;
        }
        let inner_w = (col_widths[cu] - cell.text_left_inset - cell.text_right_inset).max(0.0);
        let block = plan_cell_block(em, &cell.paragraphs, inner_w);
        let insets = cell.text_top_inset + cell.text_bottom_inset;
        let clamp = table.rows.get(ru).map_or(f32::INFINITY, row_cap_pt);
        let required = block.fitted(clamp - insets).0 + insets;
        row_heights[ru] = row_heights[ru].max(required).min(clamp);
    }
    row_heights.iter().sum()
}

/// The height a row never goes under: `max(SingleRowHeight,
/// MinimumHeight)`.
fn row_floor_pt(row: &paged_model::TableRow) -> f32 {
    row.single_row_height
        .unwrap_or(0.0)
        .max(row.minimum_height.unwrap_or(0.0))
}

/// The height a row never goes over. A fixed row (`AutoGrow="false"`)
/// is held at its floor whatever its cells hold: InDesign kept a 20 pt
/// and a 40 pt fixed row at exactly 20 and 40, drew the lines that fit
/// in them and overset the rest — none of a 20.826 pt line in the 20 pt
/// row (2026-10-02, `tables-rows` page 10). A growing row stops at its
/// `MaximumHeight`, unbounded when it has none.
fn row_cap_pt(row: &paged_model::TableRow) -> f32 {
    if row.auto_grow == Some(false) {
        row_floor_pt(row)
    } else {
        row.maximum_height.unwrap_or(f32::INFINITY)
    }
}

/// Whether `row` keeps with the row after it (`KeepWithNextRow`).
fn keeps_with_next(row: Option<&paged_model::TableRow>) -> bool {
    row.and_then(|r| r.keep_with_next_row) == Some(true)
}

/// Slack allowed when deciding whether a cell line fits: a line flush
/// with the bottom inset is in, and the layout's 1/64 pt rounding must
/// not push it out.
const CELL_FIT_EPSILON_PT: f32 = 0.01;

/// One item of a cell's content, placed by InDesign's rules (measured
/// 2026-10-02 on `tables-rows`; the rules are written out in
/// `tests/tables_rows_pipeline.rs`). Positions are points below the
/// cell's inner top — its top inset.
enum CellItem<'t> {
    Text {
        paragraph: &'t paged_model::Paragraph,
        /// Index into the cell's paragraphs: the caret address.
        index: usize,
        /// Where the paragraph is laid out from.
        origin: f32,
        /// Its first line one leading below `origin` (every paragraph
        /// after the first) rather than one ascent (the first).
        by_leading: bool,
        /// Every line's baseline.
        baselines: Vec<f32>,
    },
    Table {
        table: &'t paged_model::Table,
        top: f32,
        height: f32,
    },
}

/// A cell's content, measured before it is drawn so the row can be
/// sized from it and the same positions drawn.
struct CellBlock<'t> {
    items: Vec<CellItem<'t>>,
}

impl CellBlock<'_> {
    /// How deep the content that fits in `avail` reaches, and whether
    /// any was left over (overset). Lines fit in order; the first that
    /// does not ends the cell.
    ///
    /// The content reaches its last baseline. Nothing below it counts —
    /// no descent, no half-leading: InDesign makes a cell of one 12 pt
    /// Open Sans line `4 + 12.826 + 4` = 20.826 pt tall, and two lines at
    /// 24 pt leading `4 + 12.826 + 24 + 4`.
    fn fitted(&self, avail: f32) -> (f32, bool) {
        let mut bottom = 0.0f32;
        for item in &self.items {
            match item {
                CellItem::Text { baselines, .. } => {
                    for &b in baselines {
                        if b > avail + CELL_FIT_EPSILON_PT {
                            return (bottom, true);
                        }
                        bottom = b;
                    }
                }
                CellItem::Table { top, height, .. } => {
                    if top + height > avail + CELL_FIT_EPSILON_PT {
                        return (bottom, true);
                    }
                    bottom = top + height;
                }
            }
        }
        (bottom, false)
    }

    /// The items whose first line (or whole table) fits in `avail`.
    fn fitting_items(&self, avail: f32) -> usize {
        self.items
            .iter()
            .take_while(|item| match item {
                CellItem::Text { baselines, .. } => baselines
                    .first()
                    .is_some_and(|b| *b <= avail + CELL_FIT_EPSILON_PT),
                CellItem::Table { top, height, .. } => top + height <= avail + CELL_FIT_EPSILON_PT,
            })
            .count()
    }
}

/// Measure a cell's paragraphs into a [`CellBlock`].
///
/// The first line sits its ascent below the inset; every later line one
/// leading below the one before, across paragraphs too, with the earlier
/// paragraph's space after and the later one's space before between
/// them. The first paragraph's space before and the last one's space
/// after count for nothing: InDesign set "space before 10" on a cell's
/// only paragraph at the same baseline as its neighbours, and a last
/// paragraph's "space after 9 / 12" left its row as tall as without.
fn plan_cell_block<'t>(
    em: &StoryEmitter,
    paragraphs: &'t [paged_model::Paragraph],
    inner_w: f32,
) -> CellBlock<'t> {
    let mut items = Vec::new();
    // The previous paragraph's last baseline and space after, while the
    // previous item is text.
    let mut prev_text: Option<(f32, f32)> = None;
    let mut cursor = 0.0f32;
    for (index, paragraph) in paragraphs.iter().enumerate() {
        if paragraph.runs.is_empty() {
            if let Some(table) = paragraph.table.as_ref() {
                let top = prev_text.map(|(b, after)| b + after).unwrap_or(cursor);
                let height = measure_nested_table_height(em, table, inner_w);
                items.push(CellItem::Table { table, top, height });
                cursor = top + height;
                prev_text = None;
            }
            continue;
        }
        let attrs = em.document.resolved_paragraph_attrs(paragraph);
        let (origin, by_leading) = match prev_text {
            Some((last, after)) => (
                last + after + attrs.space_before.unwrap_or(0.0).max(0.0),
                true,
            ),
            None => (cursor, false),
        };
        let lines = measure_cell_paragraph(em, paragraph, inner_w, by_leading);
        let Some(&last) = lines.last() else {
            continue;
        };
        items.push(CellItem::Text {
            paragraph,
            index,
            origin,
            by_leading,
            baselines: lines.iter().map(|b| origin + b).collect(),
        });
        prev_text = Some((origin + last, attrs.space_after.unwrap_or(0.0).max(0.0)));
    }
    CellBlock { items }
}

/// A cell's text insets as InDesign applies them — `CellInsets` in
/// top, left, bottom, right order: never less than half the cell's own
/// edge stroke on that side.
///
/// Measured 2026-10-02 (`tables-rows` pages 4-5): a 0 pt top inset
/// under the default 1 pt edge sets the first baseline 0.5 pt below the
/// row top, under a 4 pt edge 2 pt; insets of 1 and 3 under 4 pt give 2
/// and 3; the bottom inset grows the row the same way, and a 0 pt left
/// inset starts the line 0.5 pt in from a 1 pt edge. The text stays
/// clear of the half of the stroke that lies inside the cell.
fn effective_cell_insets(
    cell: &paged_model::TableCell,
    resolved_cell: &paged_model::ResolvedCell,
) -> CellInsets {
    let drawn = |inline_color: Option<&String>,
                 inline_weight: Option<f32>,
                 style_color: Option<&String>,
                 style_weight: Option<f32>|
     -> f32 {
        let color = inline_color
            .map(String::as_str)
            .filter(|c| !is_none_swatch_id(c))
            .or(style_color.map(String::as_str));
        let (color_id, weight) = cell_edge_stroke(color, inline_weight.or(style_weight));
        if is_none_swatch_id(color_id) {
            0.0
        } else {
            weight.max(0.0) * 0.5
        }
    };
    let rc = resolved_cell;
    CellInsets {
        top: cell.text_top_inset.max(drawn(
            cell.top_edge_stroke_color.as_ref(),
            cell.top_edge_stroke_weight,
            rc.top_edge_stroke_color.as_ref(),
            rc.top_edge_stroke_weight,
        )),
        left: cell.text_left_inset.max(drawn(
            cell.left_edge_stroke_color.as_ref(),
            cell.left_edge_stroke_weight,
            rc.left_edge_stroke_color.as_ref(),
            rc.left_edge_stroke_weight,
        )),
        bottom: cell.text_bottom_inset.max(drawn(
            cell.bottom_edge_stroke_color.as_ref(),
            cell.bottom_edge_stroke_weight,
            rc.bottom_edge_stroke_color.as_ref(),
            rc.bottom_edge_stroke_weight,
        )),
        right: cell.text_right_inset.max(drawn(
            cell.right_edge_stroke_color.as_ref(),
            cell.right_edge_stroke_weight,
            rc.right_edge_stroke_color.as_ref(),
            rc.right_edge_stroke_weight,
        )),
    }
}

#[derive(Clone, Copy)]
struct CellInsets {
    top: f32,
    left: f32,
    bottom: f32,
    right: f32,
}

/// Where a cell paragraph's first line sits below its origin, and the
/// leading of its lines.
///
/// The cell's FIRST line follows the same rule as a frame's: the real
/// face's ascender under the default "Ascent" policy (measured
/// 2026-09-06 — a top-aligned cell's first line sits at `row_top +
/// top_inset + ascender`). A substituted face keeps the `0.8 × pt`
/// heuristic, as frames do: a stand-in's ascender says nothing about
/// where InDesign, which had the real one, put the baseline.
///
/// Every LATER paragraph's first line sits one leading — its own —
/// below the previous paragraph's last baseline plus the spacing
/// between them (`by_leading`, the origin then being that baseline plus
/// the spacing): InDesign set "18 pt" then "8 pt" 9.6 apart and "8 pt"
/// then "18 pt" 21.6 apart (2026-10-02, `tables-rows` page 3). Laying
/// every paragraph out from the ascender, as cells did, stacked them
/// `ascent + 0.4 × leading` apart — 18.59 pt for 12 pt Open Sans where
/// InDesign puts 14.4, so multi-paragraph cells overflowed their rows.
fn set_cell_first_baseline(
    lopts: &mut paged_text::LayoutOptions,
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    paragraph_size: f32,
    head_metrics: Option<&FontMetrics>,
    by_leading: bool,
) {
    let leading_pt = cell_paragraph_leading_pt(resolved_runs, paragraph_size);
    // Cascaded `Leading` governs a cell's line spacing exactly as it
    // governs a frame's; the cells used 1.2 × pt regardless.
    if let Some(leading_pt) = resolved_runs.first().and_then(|r| r.leading) {
        if leading_pt > 0.0 {
            lopts.leading_override =
                Some((leading_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32);
        }
    }
    lopts.first_baseline = if by_leading {
        (leading_pt * paged_text::shape::ADVANCE_PRECISION).round() as i32
    } else {
        super::text_frame::first_baseline_offset_64(
            Some(paged_model::FirstBaselineOffset::AscentOffset),
            None,
            paragraph_size,
            ((paragraph_size * 0.8) * paged_text::shape::ADVANCE_PRECISION).round() as i32,
            head_metrics,
            None,
        )
    };
}

/// A cell paragraph's leading: its first run's cascaded `Leading`, or
/// auto (120 % of the size).
fn cell_paragraph_leading_pt(
    resolved_runs: &[paged_scene::ResolvedRunAttrs],
    paragraph_size: f32,
) -> f32 {
    resolved_runs
        .first()
        .and_then(|r| r.leading)
        .filter(|l| *l > 0.0)
        .unwrap_or(paragraph_size * 1.2)
}

/// Every line's baseline, in points below the paragraph's origin, as
/// [`emit_cell_paragraph`] will set them with the same `by_leading`.
/// Empty when the paragraph is empty or its fonts do not resolve.
fn measure_cell_paragraph(
    em: &StoryEmitter,
    paragraph: &paged_model::Paragraph,
    column_width_pt: f32,
    by_leading: bool,
) -> Vec<f32> {
    if column_width_pt <= 0.0 || paragraph.runs.is_empty() {
        return Vec::new();
    }
    let resolved_runs: Vec<paged_scene::ResolvedRunAttrs> = paragraph
        .runs
        .iter()
        .map(|r| em.document.resolved_run_attrs(paragraph, r))
        .collect();
    // Per-run bytes with per-paragraph fallback for any run whose
    // (family, style) doesn't resolve — keeps height-measurement
    // honest even when one cell run references an absent font.
    let Some(resolved_fonts) = em.font_table.resolve_paragraph_bytes(&resolved_runs) else {
        return Vec::new();
    };
    let (bytes_pool, substituted_flags): (Vec<Bytes>, Vec<bool>) =
        resolved_fonts.into_iter().unzip();
    let wghts: Vec<f32> = resolved_runs
        .iter()
        .map(|r| wght_for_font_style(r.font_style.as_deref()))
        .collect();
    let mut unique_idx: Vec<usize> = Vec::with_capacity(bytes_pool.len());
    for (i, b) in bytes_pool.iter().enumerate() {
        let head = bytes_pool[..i]
            .iter()
            .zip(wghts[..i].iter())
            .position(|(prior, w)| prior.as_ptr() == b.as_ptr() && (*w - wghts[i]).abs() < 0.5)
            .unwrap_or(i);
        unique_idx.push(head);
    }
    // Shaping faces: prefer the per-render FontTable cache (built
    // from a full harvest of every run, table cells included); fall
    // back to building on demand for runs the cache didn't see.
    let mut owned_shaping_faces: Vec<Option<paged_text::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut shaping_faces: Vec<Option<&paged_text::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
    let bytes_font_ids: Vec<u32> = bytes_pool.iter().map(font_id).collect();
    for i in 0..bytes_pool.len() {
        if unique_idx[i] != i {
            continue;
        }
        if em
            .font_table
            .face(bytes_font_ids[i], wghts[i].to_bits())
            .is_none()
        {
            let bytes_ref = bytes_pool[i].as_ref();
            let Some(mut rf) = paged_text::Face::from_slice(bytes_ref, 0) else {
                return Vec::new();
            };
            let has_wght_axis = rf
                .variation_axes()
                .into_iter()
                .any(|axis| axis.tag == wght_tag);
            if has_wght_axis {
                rf.set_variations(&[paged_text::Variation {
                    tag: wght_tag,
                    value: wghts[i],
                }]);
            }
            owned_shaping_faces[i] = Some(rf);
        }
    }
    for i in 0..bytes_pool.len() {
        let head = unique_idx[i];
        if let Some(cached) = em
            .font_table
            .face(bytes_font_ids[head], wghts[head].to_bits())
        {
            shaping_faces[i] = Some(cached);
        } else if let Some(owned) = owned_shaping_faces[head].as_ref() {
            shaping_faces[i] = Some(owned);
        }
    }
    let font_ids: Vec<u32> = bytes_pool
        .iter()
        .zip(wghts.iter())
        .map(|(b, w)| font_id(b) ^ w.to_bits())
        .collect();
    let styled_runs: Vec<paged_text::StyledRun> = paragraph
        .runs
        .iter()
        .enumerate()
        .map(|(i, run)| paged_text::StyledRun {
            text: &run.text,
            face: shaping_faces[unique_idx[i]].unwrap(),
            point_size: {
                // `Position` (super/subscript) shrinks the run to a
                // fraction of its base size — see `position_metrics`.
                let base = resolved_runs[i]
                    .point_size
                    .unwrap_or(em.options.default_point_size);
                base * position_metrics(resolved_runs[i].position.as_deref()).0
            },
            tracking: resolved_runs[i].tracking,
            font_id: font_ids[i],
            underline: resolved_runs[i].underline.unwrap_or(false),
            strikethru: resolved_runs[i].strikethru.unwrap_or(false),
            substituted: substituted_flags[i],
            baseline_shift_pt: {
                // Add the `Position` (super/subscript) baseline offset
                // on top of any explicit `BaselineShift`.
                let base = resolved_runs[i]
                    .point_size
                    .unwrap_or(em.options.default_point_size);
                resolved_runs[i].baseline_shift.unwrap_or(0.0)
                    + base * position_metrics(resolved_runs[i].position.as_deref()).1
            },
            horizontal_scale_pct: resolved_runs[i].horizontal_scale.unwrap_or(100.0),
            vertical_scale_pct: resolved_runs[i].vertical_scale.unwrap_or(100.0),
            skew_deg: resolved_runs[i].skew.unwrap_or(0.0),
            fallback_faces: &[],
            shaping_features: shaping_features_from(
                resolved_runs[i].ligatures_on,
                resolved_runs[i].kerning_method.as_deref(),
                &resolved_runs[i].otf,
                resolved_runs[i].capitalization.as_deref(),
            ),
        })
        .collect();
    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let resolved_paragraph = em.document.resolved_paragraph_attrs(paragraph);
    let mut lopts = paged_text::LayoutOptions::new(column_width_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification);
    apply_paragraph_compose_options(
        &mut lopts,
        em.hyphenator_for(&resolved_paragraph, &resolved_runs),
        &resolved_paragraph,
    );
    let head_metrics = bytes_font_ids
        .first()
        .and_then(|id| em.font_table.metrics_for(*id));
    set_cell_first_baseline(
        &mut lopts,
        &resolved_runs,
        paragraph_size,
        head_metrics,
        by_leading,
    );
    let laid_out = paged_text::cache::layout_runs_cached(&styled_runs, &lopts);
    laid_out
        .lines
        .iter()
        .map(|l| l.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION)
        .collect()
}

/// Lay out and emit a single cell paragraph at `(origin_pt.0,
/// origin_pt.1 + paragraph_y)` with `column_width_pt` available.
/// Returns the vertical extent the paragraph consumed so the
/// caller can stack subsequent cell paragraphs underneath.
/// Self-contained shape → layout → emit; no inter-paragraph state.
// `pub(super)` (not private) so the `#[cfg(doc)]` doc-link import in
// `pipeline/mod.rs` resolves — rustdoc's `--test` pass sets `cfg(doc)`,
// and a private item there is an E0603 that fails the doctest build.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_cell_paragraph(
    em: &mut StoryEmitter,
    paragraph: &paged_model::Paragraph,
    target_page: usize,
    origin_pt: (f32, f32),
    column_width_pt: f32,
    paragraph_y: f32,
    pages: &mut [BuiltPage],
    total_stats: &mut PipelineStats,
    // W1.13 — disjoint StoryLayout address for this cell paragraph.
    // `Some((addr, para_idx))` stamps emitted lines with a cell
    // qualifier + cell-local paragraph index so the canvas can address
    // (and edit) cell text without colliding with body paragraphs.
    // `None` for paths that don't participate in caret addressing
    // (nested-table cells — see defer note at the call site).
    cell_addr: Option<(&CellAddr, u32)>,
    // Lay the first line out one leading below the origin rather than
    // one ascent (see `set_cell_first_baseline`).
    by_leading: bool,
    // The lowest baseline (below the origin) the cell has room for: a
    // line below it is overset and not drawn, as InDesign draws none of
    // a cell's overset text.
    max_baseline_pt: f32,
) {
    if column_width_pt <= 0.0 || paragraph.runs.is_empty() {
        return;
    }
    let resolved_runs: Vec<paged_scene::ResolvedRunAttrs> = paragraph
        .runs
        .iter()
        .map(|r| em.document.resolved_run_attrs(paragraph, r))
        .collect();
    // Per-run bytes with per-paragraph fallback (matches the main
    // emit path). A single unresolvable run no longer takes the
    // whole cell paragraph down with it.
    let Some(resolved_fonts) = em.font_table.resolve_paragraph_bytes(&resolved_runs) else {
        return;
    };
    let (bytes_pool, substituted_flags): (Vec<Bytes>, Vec<bool>) =
        resolved_fonts.into_iter().unzip();
    // Per-run wght axis values, derived from the resolved FontStyle.
    // Identical wiring to the main `emit_paragraph_into_chain` path —
    // table-cell text needs Bold / Light pinning too. Without this,
    // table column labels styled with a Bold paragraph style render
    // at the variable font's default weight (visible regression on
    // any catalog with bold table headers).
    let wghts: Vec<f32> = resolved_runs
        .iter()
        .map(|r| wght_for_font_style(r.font_style.as_deref()))
        .collect();
    // Reuse a shaped face only when both bytes AND weight match; a
    // bold + regular pair sharing the same Inter.ttf bytes still
    // needs two distinct paged_text::Face objects so set_variations
    // doesn't fight itself.
    let mut unique_idx: Vec<usize> = Vec::with_capacity(bytes_pool.len());
    for (i, b) in bytes_pool.iter().enumerate() {
        let head = bytes_pool[..i]
            .iter()
            .zip(wghts[..i].iter())
            .position(|(prior, w)| prior.as_ptr() == b.as_ptr() && (*w - wghts[i]).abs() < 0.5)
            .unwrap_or(i);
        unique_idx.push(head);
    }
    // Outline faces stay per-paragraph; shaping faces pull from
    // the per-render FontTable cache (built from a full
    // table-cell-aware harvest at startup) with an on-demand
    // fallback for any (font_id, wght_bits) the cache didn't see.
    let mut outline_faces: Vec<Option<ttf_parser::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut owned_shaping_faces: Vec<Option<paged_text::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut shaping_faces: Vec<Option<&paged_text::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
    let bytes_font_ids: Vec<u32> = bytes_pool.iter().map(font_id).collect();
    for i in 0..bytes_pool.len() {
        if unique_idx[i] != i {
            continue;
        }
        let bytes_ref = bytes_pool[i].as_ref();
        let Ok(mut of) = ttf_parser::Face::parse(bytes_ref, 0) else {
            return;
        };
        let has_wght_axis = of
            .variation_axes()
            .into_iter()
            .any(|axis| axis.tag == wght_tag);
        if has_wght_axis {
            let _ = of.set_variation(wght_tag, wghts[i]);
        } else if (wghts[i] - 400.0).abs() > 50.0 {
            // Q-25: the IDML asked for a non-Regular weight but the
            // matched font has no `wght` variation axis (single-
            // weight TTF). Surface this as a trace so users know
            // catalog-brochure-template / brand-guidelines display
            // headlines render at the substitute's intrinsic weight
            // (e.g. "Catalog" hero ~30% thicker than ref). Curable
            // by routing the affected family through a variable font
            // in the per-pack fonts overrides.
            tracing::warn!(
                font_id = bytes_font_ids[i],
                requested_wght = wghts[i],
                "matched font has no wght axis; requested weight ignored — substitute will render at the file's intrinsic weight"
            );
        }
        outline_faces[i] = Some(of);

        if em
            .font_table
            .face(bytes_font_ids[i], wghts[i].to_bits())
            .is_none()
        {
            let Some(mut rf) = paged_text::Face::from_slice(bytes_ref, 0) else {
                return;
            };
            if has_wght_axis {
                rf.set_variations(&[paged_text::Variation {
                    tag: wght_tag,
                    value: wghts[i],
                }]);
            }
            owned_shaping_faces[i] = Some(rf);
        }
    }
    for i in 0..bytes_pool.len() {
        let head = unique_idx[i];
        if let Some(cached) = em
            .font_table
            .face(bytes_font_ids[head], wghts[head].to_bits())
        {
            shaping_faces[i] = Some(cached);
        } else if let Some(owned) = owned_shaping_faces[head].as_ref() {
            shaping_faces[i] = Some(owned);
        }
    }
    // font_id mixes in the wght variation so the glyph-outline cache
    // (keyed on (font_id, glyph_id)) doesn't conflate outlines from a
    // variable font fed at two different wght axis values.
    let font_ids: Vec<u32> = bytes_pool
        .iter()
        .zip(wghts.iter())
        .map(|(b, w)| font_id(b) ^ w.to_bits())
        .collect();

    let styled_runs: Vec<paged_text::StyledRun> = paragraph
        .runs
        .iter()
        .enumerate()
        .map(|(i, run)| paged_text::StyledRun {
            text: &run.text,
            face: shaping_faces[unique_idx[i]].unwrap(),
            point_size: {
                // `Position` (super/subscript) shrinks the run to a
                // fraction of its base size — see `position_metrics`.
                let base = resolved_runs[i]
                    .point_size
                    .unwrap_or(em.options.default_point_size);
                base * position_metrics(resolved_runs[i].position.as_deref()).0
            },
            tracking: resolved_runs[i].tracking,
            font_id: font_ids[i],
            underline: resolved_runs[i].underline.unwrap_or(false),
            strikethru: resolved_runs[i].strikethru.unwrap_or(false),
            substituted: substituted_flags[i],
            baseline_shift_pt: {
                // Add the `Position` (super/subscript) baseline offset
                // on top of any explicit `BaselineShift`.
                let base = resolved_runs[i]
                    .point_size
                    .unwrap_or(em.options.default_point_size);
                resolved_runs[i].baseline_shift.unwrap_or(0.0)
                    + base * position_metrics(resolved_runs[i].position.as_deref()).1
            },
            horizontal_scale_pct: resolved_runs[i].horizontal_scale.unwrap_or(100.0),
            vertical_scale_pct: resolved_runs[i].vertical_scale.unwrap_or(100.0),
            skew_deg: resolved_runs[i].skew.unwrap_or(0.0),
            fallback_faces: &[],
            shaping_features: shaping_features_from(
                resolved_runs[i].ligatures_on,
                resolved_runs[i].kerning_method.as_deref(),
                &resolved_runs[i].otf,
                resolved_runs[i].capitalization.as_deref(),
            ),
        })
        .collect();
    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let resolved_paragraph = em.document.resolved_paragraph_attrs(paragraph);
    let mut lopts = paged_text::LayoutOptions::new(column_width_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification);
    apply_paragraph_compose_options(
        &mut lopts,
        em.hyphenator_for(&resolved_paragraph, &resolved_runs),
        &resolved_paragraph,
    );
    let head_metrics = bytes_font_ids
        .first()
        .and_then(|id| em.font_table.metrics_for(*id));
    set_cell_first_baseline(
        &mut lopts,
        &resolved_runs,
        paragraph_size,
        head_metrics,
        by_leading,
    );

    let laid_out = paged_text::cache::layout_runs_cached(&styled_runs, &lopts);
    // Only the lines the cell has room for: the rest are overset.
    let fitting = laid_out
        .lines
        .iter()
        .take_while(|l| {
            l.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION
                <= max_baseline_pt + CELL_FIT_EPSILON_PT
        })
        .count();
    let lines = &laid_out.lines[..fitting];
    if lines.is_empty() {
        return;
    }

    let picker = build_run_paint_picker_resolved(
        paragraph,
        &resolved_runs,
        em.palette,
        em.color_ctx,
        em.options.fallback_text_paint,
        None,
    );
    let stroke_picker =
        build_run_stroke_picker(paragraph, &resolved_runs, em.palette, em.color_ctx, 0);
    let any_text_stroke = stroke_picker.any_visible();
    let leading_pt = cell_paragraph_leading_pt(&resolved_runs, paragraph_size);
    let cell_origin = (origin_pt.0, origin_pt.1 + paragraph_y);

    // Cycle-5 Track 4: emit BreakRecords for table-cell paragraphs so
    // the A/B harness covers `<TableCell>` content the same way it
    // covers regular body paragraphs. The cell paragraph has no
    // `paragraph_idx` of its own — the emitter's counter advances
    // once per body paragraph, not once per cell — so we read the
    // current value (the host paragraph that holds the table) and
    // accept the collision. Downstream tooling treats break records
    // as per-line stream, not per-paragraph indexed. Cycle-6 Track 1:
    // also gated on optional story / page-range filters.
    if em.break_filter_passes(target_page as u32) {
        let mut paragraph_text = String::new();
        for r in &styled_runs {
            paragraph_text.push_str(r.text);
        }
        for (line_idx, line) in lines.iter().enumerate() {
            let start = line.byte_range.start.min(paragraph_text.len());
            let end = line.byte_range.end.min(paragraph_text.len());
            let source_text = paragraph_text.get(start..end).unwrap_or("").to_string();
            em.breaks.push(BreakRecord {
                story_id: em.current_story_id.clone(),
                paragraph_idx: em.paragraph_idx,
                line_idx: line_idx as u32,
                page_idx: target_page as u32,
                frame_idx: em.frame_idx as u32,
                first_byte: line.byte_range.start as u32,
                last_byte: line.byte_range.end as u32,
                baseline_y_pt: line.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION,
                width_pt: line.width as f32 / paged_text::shape::ADVANCE_PRECISION,
                source_text,
            });
        }
    }

    // W1.13 (was Phase 3 Item A) — capture StoryLayout for table-cell
    // paragraphs so the canvas's caret + selection can address text
    // inside tables. Cell text shares the host story id; the disjoint
    // address axis is `LineLayout.cell` (the `(table_id, row, col)`
    // qualifier passed in via `cell_addr`) PLUS a cell-local
    // `paragraph_idx`. That pairing is what makes body paragraph N and
    // a cell's paragraph N distinct addresses — the long-standing
    // collision the W3.A1 deferral noted. `cell_addr == None` (nested-
    // table cells) keeps the legacy behaviour: lines are emitted with
    // no cell qualifier and the host paragraph_idx, so they remain
    // visible/selectable as before but aren't separately editable
    // (documented defer — nested-cell editing).
    {
        let host_page_id = pages[target_page].id.clone();
        let (line_cell, line_para_idx) = match cell_addr {
            Some((addr, para_idx)) => (Some(addr.clone()), para_idx),
            None => (None, em.paragraph_idx),
        };
        for (line_idx, line) in lines.iter().enumerate() {
            let baseline_pt_local = line.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION;
            let line_h_pt = leading_pt; // cell paragraphs use 1.2 × point size
            let mut clusters: Vec<ClusterPos> = Vec::with_capacity(line.glyphs.len());
            let mut last_cluster: Option<u32> = None;
            for g in &line.glyphs {
                let adv = g.x_advance as f32 / paged_text::shape::ADVANCE_PRECISION;
                if last_cluster == Some(g.cluster) {
                    if let Some(c) = clusters.last_mut() {
                        c.advance_pt += adv;
                    }
                    continue;
                }
                last_cluster = Some(g.cluster);
                let x_pt_page = cell_origin.0 + g.x as f32 / paged_text::shape::ADVANCE_PRECISION;
                clusters.push(ClusterPos {
                    byte: g.cluster,
                    x_pt: x_pt_page,
                    advance_pt: adv,
                });
            }
            pages[target_page].story_layout.push(LineLayout {
                story_id: em.current_story_id.clone(),
                page_id: host_page_id.clone(),
                cell: line_cell.clone(),
                paragraph_idx: line_para_idx,
                line_idx: line_idx as u32,
                frame_id: em.chain.get(em.frame_idx).and_then(|f| f.self_id.clone()),
                baseline_y_pt: cell_origin.1 + baseline_pt_local,
                ascent_pt: 0.8 * line_h_pt,
                descent_pt: 0.2 * line_h_pt,
                byte_range: line.byte_range.start as u32..line.byte_range.end as u32,
                clusters,
            });
        }
    }

    let list = &mut pages[target_page].list;
    for line in lines {
        let mut start = 0;
        while start < line.glyphs.len() {
            let fid = line.glyphs[start].font_id;
            // A slice is drawn at ONE size, so it ends where the size does:
            // two runs in the same face at different sizes (a list marker in
            // its 20 pt character style before 10 pt text) share a font_id.
            let size = line.glyphs[start].point_size;
            let mut end = start + 1;
            while end < line.glyphs.len()
                && line.glyphs[end].font_id == fid
                && (line.glyphs[end].point_size - size).abs() < 0.01
            {
                end += 1;
            }
            let face_idx = match font_ids.iter().position(|f| *f == fid) {
                Some(i) => unique_idx[i],
                None => {
                    start = end;
                    continue;
                }
            };
            let Some(outline) = outline_faces[face_idx].as_ref() else {
                start = end;
                continue;
            };
            let outliner = TtfOutliner::new(outline);
            emit_glyph_slice(
                &line.glyphs[start..end],
                fid,
                line.glyphs[start].point_size,
                |cluster| picker.pick(cluster),
                cell_origin,
                &outliner,
                list,
            );
            if any_text_stroke {
                emit_glyph_slice_stroke(
                    &line.glyphs[start..end],
                    fid,
                    line.glyphs[start].point_size,
                    |cluster| stroke_picker.pick(cluster),
                    cell_origin,
                    &outliner,
                    list,
                );
            }
            start = end;
        }
    }
    let glyph_count: usize = lines.iter().map(|l| l.glyphs.len()).sum();
    total_stats.paragraphs += 1;
    total_stats.runs += paragraph.runs.len();
    total_stats.glyphs += glyph_count;
    total_stats.lines += lines.len();
    pages[target_page].stats.paragraphs += 1;
    pages[target_page].stats.runs += paragraph.runs.len();
    pages[target_page].stats.glyphs += glyph_count;
    pages[target_page].stats.lines += lines.len();
}
