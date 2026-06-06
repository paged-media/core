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
            self.start_color.map(|c| (c, self.start_tint))
        } else {
            self.end_color.map(|c| (c, self.end_tint))
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
///   max(SingleRowHeight, MinimumHeight, max_cell_required) ` clamped
///   to `MaximumHeight`. For RowSpan > 1 cells the constraint is
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
    table: &paged_parse::Table,
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

    let resolved_table = table
        .applied_table_style
        .as_deref()
        .map(|id| em.document.styles.resolve_table(id))
        .unwrap_or_default();
    let header_count = table.header_row_count as usize;
    let footer_count = table.footer_row_count as usize;
    let total_rows = table.rows.len();
    let total_cols = col_widths.len();

    // Content-driven row growth. For each row, find the tallest
    // required cell height (sum of per-paragraph consumed heights
    // + top/bottom insets). For span > 1 cells, only the LAST row
    // of the span enforces the shortfall — earlier rows in the
    // span are left at their declared height. This is the simpler
    // heuristic the plan calls out; a smarter distributor would
    // share the slack across the span proportionally.
    //
    // Final height per row =
    //   max(SingleRowHeight, MinimumHeight, content_required)
    // clamped to MaximumHeight (when set; unbounded otherwise).
    let mut row_heights: Vec<f32> = table
        .rows
        .iter()
        .map(|r| {
            r.single_row_height
                .unwrap_or(0.0)
                .max(r.minimum_height.unwrap_or(0.0))
        })
        .collect();
    // Per-cell pre-measured content height, keyed by the cell's
    // starting (col, row) — independent of where the cell lands
    // geometrically. Used both for the row-growth pass and to skip
    // re-laying-out the same cell during emission.
    let mut cell_required: std::collections::HashMap<(u32, u32), f32> =
        std::collections::HashMap::with_capacity(table.cells.len());
    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else { continue };
        let (cu, ru) = (c as usize, r as usize);
        if cu >= col_widths.len() || ru >= total_rows {
            continue;
        }
        let span_cols = cell.column_span.max(1) as usize;
        let last_c = (cu + span_cols).min(col_widths.len());
        let inner_w = (col_x[last_c]
            - col_x[cu]
            - cell.text_left_inset
            - cell.text_right_inset)
            .max(0.0);
        let mut paragraph_y = 0.0f32;
        for paragraph in &cell.paragraphs {
            if paragraph.runs.is_empty() {
                if let Some(inner_t) = paragraph.table.as_ref() {
                    // Phase 5 — measure a nested table's height by
                    // summing its row heights (with the same
                    // SingleRowHeight / MinimumHeight default the
                    // emit pass uses, plus content-driven growth
                    // from inner cell paragraphs).
                    paragraph_y += measure_nested_table_height(em, inner_t, inner_w);
                }
                continue;
            }
            paragraph_y += measure_cell_paragraph(em, paragraph, inner_w);
        }
        let required = paragraph_y + cell.text_top_inset + cell.text_bottom_inset;
        cell_required.insert((c, r), required);
    }
    // Walk rows top-to-bottom; for each row grow it to fit cells
    // that *end* in this row (span_rows + start_row - 1 == r).
    // We iterate by ending-row, look at all cells with that ending,
    // and bump `row_heights[r]` to cover any shortfall remaining
    // after the prior rows of the span. This way RowSpan > 1
    // cells don't blow up multiple rows.
    for r in 0..total_rows {
        let mut required = row_heights[r];
        for cell in &table.cells {
            let Some((c, sr)) = cell.coords() else { continue };
            let span = cell.row_span.max(1) as usize;
            let (cu, sru) = (c as usize, sr as usize);
            if sru + span - 1 != r {
                continue;
            }
            if cu >= col_widths.len() {
                continue;
            }
            let Some(cell_h) = cell_required.get(&(c, sr)).copied() else {
                continue;
            };
            // Heights already grown for the prior rows of the span.
            let prior: f32 = (sru..r).map(|i| row_heights[i]).sum();
            let shortfall = cell_h - prior;
            if shortfall > required {
                required = shortfall;
            }
        }
        let max_h = table
            .rows
            .get(r)
            .and_then(|tr| tr.maximum_height)
            .unwrap_or(f32::INFINITY);
        row_heights[r] = required.min(max_h);
    }

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
    let mut chain_idx = em.frame_idx;
    let (mut tab_left, mut frame_top_in_page, mut frame_height, mut top_inset, mut target_page) =
        frame_basis_for(chain_idx, em.column_x_shift_pt);
    let mut row_top_y_in_frame = if em.y_cursor >= 0 {
        em.y_cursor as f32 / paged_text::shape::ADVANCE_PRECISION
            - em.options.default_point_size * 0.8
    } else {
        top_inset
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
    for r in body_range.clone() {
        let h = row_heights[r];
        let need_extra_for_split = footer_reserved_h;
        let would_overflow = row_top_y_in_frame + h + need_extra_for_split > frame_height;
        if would_overflow && chain_idx + 1 < em.chain.len() && placed_in_frame > 0 {
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
            tab_left = l;
            frame_top_in_page = ftop;
            frame_height = h_next;
            top_inset = ti;
            target_page = tp;
            row_top_y_in_frame = top_inset;
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

    // Original footer rows — emitted on whatever frame the body
    // left off in (= the last frame), in their natural sequence.
    for r in (total_rows - footer_count)..total_rows {
        if footer_count == 0 {
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
            let Some(paint) = color_id_to_paint(fill_id, em.palette, em.cmyk_xform) else {
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
            skip_last: resolved_table.skip_last_alternating_fill_columns.unwrap_or(0) as usize,
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
                let Some(paint) = color_id_to_paint(fill_id, em.palette, em.cmyk_xform) else {
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
    let mut cell_by_origin: std::collections::HashMap<(u32, u32), &paged_parse::TableCell> =
        std::collections::HashMap::with_capacity(table.cells.len());
    for cell in &table.cells {
        if let Some(coords) = cell.coords() {
            cell_by_origin.insert(coords, cell);
        }
    }
    for prow_i in 0..physical_rows.len() {
        let prow = physical_rows[prow_i];
        let r = prow.template_idx;
        for c in 0..col_widths.len() {
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

        let inner_left = cell_x_pt + cell.text_left_inset;
        let inner_top = cell_y_pt + cell.text_top_inset;
        let inner_w = (cell_w_pt - cell.text_left_inset - cell.text_right_inset).max(0.0);
        let inner_h = (cell_h_pt - cell.text_top_inset - cell.text_bottom_inset).max(0.0);

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
            cell_fill_id.and_then(|id| color_id_to_paint(id, em.palette, em.cmyk_xform))
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
            if let (Some(color_id), Some(weight)) = (color, weight) {
                if weight > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform)
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
            if let (Some(color_id), Some(weight)) = (color, weight) {
                if weight > 0.0 {
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform)
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
                if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform)
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

        // Lay out the cell paragraphs into a working buffer first
        // so we know their total height; then apply vertical
        // justification by shifting all of them by a uniform dy.
        let mut paragraph_y = 0.0f32;
        let mut emitted_extents: Vec<(usize, usize)> = Vec::new();
        for paragraph in &cell.paragraphs {
            if paragraph.runs.is_empty() {
                // Phase 5 — nested table inside a cell paragraph.
                // Lay it out at the current cell-paragraph cursor
                // and advance by its consumed height. Inner content
                // (further nested tables, cell paragraphs) recurses
                // through emit_nested_table_inline.
                if let Some(inner_t) = paragraph.table.as_ref() {
                    let cmd_start = pages[target_page].list.commands.len();
                    let consumed = emit_nested_table_inline(
                        em,
                        inner_t,
                        inner_left,
                        inner_top + paragraph_y,
                        inner_w,
                        target_page,
                        pages,
                        total_stats,
                    );
                    let cmd_end = pages[target_page].list.commands.len();
                    if cmd_end > cmd_start {
                        emitted_extents.push((cmd_start, cmd_end));
                    }
                    paragraph_y += consumed;
                    if paragraph_y >= inner_h {
                        break;
                    }
                }
                continue;
            }
            let cmd_start = pages[target_page].list.commands.len();
            let consumed = emit_cell_paragraph(
                em,
                paragraph,
                target_page,
                (inner_left, inner_top),
                inner_w,
                paragraph_y,
                pages,
                total_stats,
            );
            let cmd_end = pages[target_page].list.commands.len();
            if cmd_end > cmd_start {
                emitted_extents.push((cmd_start, cmd_end));
            }
            paragraph_y += consumed;
            if paragraph_y >= inner_h {
                break;
            }
        }
        // Apply CellStyle vertical justification by shifting every
        // glyph command we emitted in this cell by dy = slack/factor.
        // CenterAlign → centre vertically; BottomAlign → push to the
        // bottom inset. Top is the default (no shift).
        let used_h = paragraph_y;
        if used_h > 0.0 && used_h < inner_h {
            let dy = match resolved_cell.vertical_justification.as_deref() {
                Some("CenterAlign") => Some((inner_h - used_h) * 0.5),
                Some("BottomAlign") => Some(inner_h - used_h),
                _ => None,
            };
            if let Some(dy) = dy {
                for (s, e) in &emitted_extents {
                    for cmd in &mut pages[target_page].list.commands[*s..*e] {
                        cmd.transform_mut().0[5] += dy;
                    }
                }
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
    let row_end_type = row_decl
        .end_type
        .clone()
        .or_else(|| row_start_type.clone());
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
            let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) else {
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
        let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) else {
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
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
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
                    if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
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
                if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
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
                if let Some(paint) = color_id_to_paint(color_id, em.palette, em.cmyk_xform) {
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

fn resolve_table_line_strokes(decl: &paged_parse::TableLineStrokes) -> ResolvedLineStroke {
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
    let start_weight = decl.start_weight.unwrap_or(if has_decl { 1.0 } else { 0.0 });
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
    table: &paged_parse::Table,
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
    let mut row_heights: Vec<f32> = table
        .rows
        .iter()
        .map(|r| {
            r.single_row_height
                .unwrap_or(0.0)
                .max(r.minimum_height.unwrap_or(0.0))
        })
        .collect();
    // Pre-measure every cell so row heights can grow to fit content.
    // Spans are clamped to 1 here — proper span layout is a follow-up.
    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else { continue };
        let (cu, ru) = (c as usize, r as usize);
        if cu >= col_widths.len() || ru >= total_rows {
            continue;
        }
        let inner_w = (col_widths[cu]
            - cell.text_left_inset
            - cell.text_right_inset)
            .max(0.0);
        let mut paragraph_y = 0.0f32;
        for paragraph in &cell.paragraphs {
            // Nested table inside the nested table's cell — recurse.
            if paragraph.runs.is_empty() && paragraph.table.is_some() {
                // Approximate the nested-nested table's height as
                // sum-of-row-heights to avoid pre-emit recursion.
                if let Some(inner_t) = paragraph.table.as_ref() {
                    paragraph_y += inner_t
                        .rows
                        .iter()
                        .map(|r| {
                            r.single_row_height
                                .unwrap_or(0.0)
                                .max(r.minimum_height.unwrap_or(0.0))
                        })
                        .sum::<f32>();
                }
                continue;
            }
            if paragraph.runs.is_empty() {
                continue;
            }
            paragraph_y += measure_cell_paragraph(em, paragraph, inner_w);
        }
        let required = paragraph_y + cell.text_top_inset + cell.text_bottom_inset;
        let clamp = table
            .rows
            .get(ru)
            .and_then(|tr| tr.maximum_height)
            .unwrap_or(f32::INFINITY);
        row_heights[ru] = row_heights[ru].max(required).min(clamp);
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
    for r in 0..=total_rows {
        emit_rect(
            Rect {
                x: origin_x,
                y: origin_y + row_y[r] - GRID_W * 0.5,
                w: total_w,
                h: GRID_W,
            },
            grid_paint,
            &mut pages[target_page].list,
        );
    }
    // Vertical lines (left of each column + right of last column).
    for c in 0..=col_widths.len() {
        emit_rect(
            Rect {
                x: origin_x + col_x[c] - GRID_W * 0.5,
                y: origin_y,
                w: GRID_W,
                h: table_h,
            },
            grid_paint,
            &mut pages[target_page].list,
        );
    }

    // Emit cell content.
    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else { continue };
        let (cu, ru) = (c as usize, r as usize);
        if cu >= col_widths.len() || ru >= total_rows {
            continue;
        }
        let cell_x_pt = origin_x + col_x[cu];
        let cell_y_pt = origin_y + row_y[ru];
        let cell_w_pt = col_widths[cu];
        let cell_h_pt = row_heights[ru];
        let inner_left = cell_x_pt + cell.text_left_inset;
        let inner_top = cell_y_pt + cell.text_top_inset;
        let inner_w = (cell_w_pt - cell.text_left_inset - cell.text_right_inset).max(0.0);
        let inner_h = (cell_h_pt - cell.text_top_inset - cell.text_bottom_inset).max(0.0);
        let mut paragraph_y = 0.0f32;
        for paragraph in &cell.paragraphs {
            if paragraph.runs.is_empty() {
                // Double-nested table — recurse.
                if let Some(inner_t) = paragraph.table.as_ref() {
                    let consumed = emit_nested_table_inline(
                        em,
                        inner_t,
                        inner_left,
                        inner_top + paragraph_y,
                        inner_w,
                        target_page,
                        pages,
                        total_stats,
                    );
                    paragraph_y += consumed;
                }
                continue;
            }
            let consumed = emit_cell_paragraph(
                em,
                paragraph,
                target_page,
                (inner_left, inner_top),
                inner_w,
                paragraph_y,
                pages,
                total_stats,
            );
            paragraph_y += consumed;
            if paragraph_y >= inner_h {
                break;
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
    table: &paged_parse::Table,
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
    let mut row_heights: Vec<f32> = table
        .rows
        .iter()
        .map(|r| {
            r.single_row_height
                .unwrap_or(0.0)
                .max(r.minimum_height.unwrap_or(0.0))
        })
        .collect();
    for cell in &table.cells {
        let Some((c, r)) = cell.coords() else { continue };
        let (cu, ru) = (c as usize, r as usize);
        if cu >= col_widths.len() || ru >= total_rows {
            continue;
        }
        let inner_w = (col_widths[cu]
            - cell.text_left_inset
            - cell.text_right_inset)
            .max(0.0);
        let mut paragraph_y = 0.0f32;
        for paragraph in &cell.paragraphs {
            if paragraph.runs.is_empty() {
                if let Some(inner_t) = paragraph.table.as_ref() {
                    paragraph_y += measure_nested_table_height(em, inner_t, inner_w);
                }
                continue;
            }
            paragraph_y += measure_cell_paragraph(em, paragraph, inner_w);
        }
        let required = paragraph_y + cell.text_top_inset + cell.text_bottom_inset;
        let clamp = table
            .rows
            .get(ru)
            .and_then(|tr| tr.maximum_height)
            .unwrap_or(f32::INFINITY);
        row_heights[ru] = row_heights[ru].max(required).min(clamp);
    }
    row_heights.iter().sum()
}

/// Returns `0.0` when the paragraph is empty or the font assets
/// don't resolve — callers compare against `SingleRowHeight` /
/// `MinimumHeight` so a 0 is safely absorbed.
fn measure_cell_paragraph(
    em: &StoryEmitter,
    paragraph: &paged_parse::Paragraph,
    column_width_pt: f32,
) -> f32 {
    if column_width_pt <= 0.0 || paragraph.runs.is_empty() {
        return 0.0;
    }
    let resolved_runs: Vec<paged_scene::ResolvedRunAttrs> = paragraph
        .runs
        .iter()
        .map(|r| em.document.resolved_run_attrs(paragraph, r))
        .collect();
    // Per-run bytes with per-paragraph fallback for any run whose
    // (family, style) doesn't resolve — keeps height-measurement
    // honest even when one cell run references an absent font.
    let Some(bytes_pool) = em.font_table.resolve_paragraph_bytes(&resolved_runs) else {
        return 0.0;
    };
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
    let mut owned_shaping_faces: Vec<Option<rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut shaping_faces: Vec<Option<&rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
    let bytes_font_ids: Vec<u32> = bytes_pool
        .iter()
        .map(|b| fnv_1a_u32(b.as_ref()))
        .collect();
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
            let Some(mut rf) = rustybuzz::Face::from_slice(bytes_ref, 0) else {
                return 0.0;
            };
            let has_wght_axis = rf
                .variation_axes()
                .into_iter()
                .any(|axis| axis.tag == wght_tag);
            if has_wght_axis {
                rf.set_variations(&[rustybuzz::Variation {
                    tag: wght_tag,
                    value: wghts[i],
                }]);
            }
            owned_shaping_faces[i] = Some(rf);
        }
    }
    for i in 0..bytes_pool.len() {
        let head = unique_idx[i];
        if let Some(cached) = em.font_table.face(bytes_font_ids[head], wghts[head].to_bits()) {
            shaping_faces[i] = Some(cached);
        } else if let Some(owned) = owned_shaping_faces[head].as_ref() {
            shaping_faces[i] = Some(owned);
        }
    }
    let font_ids: Vec<u32> = bytes_pool
        .iter()
        .zip(wghts.iter())
        .map(|(b, w)| fnv_1a_u32(b.as_ref()) ^ w.to_bits())
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
            fallback_faces: &[],
            shaping_features: shaping_features_from(
                resolved_runs[i].ligatures_on,
                resolved_runs[i].kerning_method.as_deref(),
            ),
        })
        .collect();
    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let resolved_paragraph = em.document.resolved_paragraph_attrs(paragraph);
    let mut lopts = paged_text::LayoutOptions::new(column_width_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification);
    apply_paragraph_compose_options(&mut lopts, em.hyphenator, &resolved_paragraph);
    lopts.first_baseline =
        ((paragraph_size * 0.8) * paged_text::shape::ADVANCE_PRECISION).round() as i32;
    let laid_out = paged_text::cache::layout_runs_cached(&styled_runs, &lopts);
    if laid_out.lines.is_empty() {
        return 0.0;
    }
    let leading_pt = paragraph_size * 1.2;
    let max_baseline_pt = laid_out
        .lines
        .iter()
        .map(|l| l.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION)
        .fold(0.0f32, f32::max);
    max_baseline_pt + leading_pt * 0.4
}

/// Lay out and emit a single cell paragraph at `(origin_pt.0,
/// origin_pt.1 + paragraph_y)` with `column_width_pt` available.
/// Returns the vertical extent the paragraph consumed so the
/// caller can stack subsequent cell paragraphs underneath.
/// Self-contained shape → layout → emit; no inter-paragraph state.
#[allow(clippy::too_many_arguments)]
fn emit_cell_paragraph(
    em: &mut StoryEmitter,
    paragraph: &paged_parse::Paragraph,
    target_page: usize,
    origin_pt: (f32, f32),
    column_width_pt: f32,
    paragraph_y: f32,
    pages: &mut [BuiltPage],
    total_stats: &mut PipelineStats,
) -> f32 {
    if column_width_pt <= 0.0 || paragraph.runs.is_empty() {
        return 0.0;
    }
    let resolved_runs: Vec<paged_scene::ResolvedRunAttrs> = paragraph
        .runs
        .iter()
        .map(|r| em.document.resolved_run_attrs(paragraph, r))
        .collect();
    // Per-run bytes with per-paragraph fallback (matches the main
    // emit path). A single unresolvable run no longer takes the
    // whole cell paragraph down with it.
    let Some(bytes_pool) = em.font_table.resolve_paragraph_bytes(&resolved_runs) else {
        return 0.0;
    };
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
    // needs two distinct rustybuzz::Face objects so set_variations
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
    let mut owned_shaping_faces: Vec<Option<rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let mut shaping_faces: Vec<Option<&rustybuzz::Face>> =
        (0..bytes_pool.len()).map(|_| None).collect();
    let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
    let bytes_font_ids: Vec<u32> = bytes_pool
        .iter()
        .map(|b| fnv_1a_u32(b.as_ref()))
        .collect();
    for i in 0..bytes_pool.len() {
        if unique_idx[i] != i {
            continue;
        }
        let bytes_ref = bytes_pool[i].as_ref();
        let Ok(mut of) = ttf_parser::Face::parse(bytes_ref, 0) else {
            return 0.0;
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
            let Some(mut rf) = rustybuzz::Face::from_slice(bytes_ref, 0) else {
                return 0.0;
            };
            if has_wght_axis {
                rf.set_variations(&[rustybuzz::Variation {
                    tag: wght_tag,
                    value: wghts[i],
                }]);
            }
            owned_shaping_faces[i] = Some(rf);
        }
    }
    for i in 0..bytes_pool.len() {
        let head = unique_idx[i];
        if let Some(cached) = em.font_table.face(bytes_font_ids[head], wghts[head].to_bits()) {
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
        .map(|(b, w)| fnv_1a_u32(b.as_ref()) ^ w.to_bits())
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
            fallback_faces: &[],
            shaping_features: shaping_features_from(
                resolved_runs[i].ligatures_on,
                resolved_runs[i].kerning_method.as_deref(),
            ),
        })
        .collect();
    let paragraph_size = styled_runs.first().map(|r| r.point_size).unwrap_or(12.0);
    let resolved_paragraph = em.document.resolved_paragraph_attrs(paragraph);
    let mut lopts = paged_text::LayoutOptions::new(column_width_pt, paragraph_size);
    lopts.alignment = map_justification(resolved_paragraph.justification);
    apply_paragraph_compose_options(&mut lopts, em.hyphenator, &resolved_paragraph);
    lopts.first_baseline =
        ((paragraph_size * 0.8) * paged_text::shape::ADVANCE_PRECISION).round() as i32;

    let laid_out = paged_text::cache::layout_runs_cached(&styled_runs, &lopts);
    if laid_out.lines.is_empty() {
        return 0.0;
    }

    let picker = build_run_paint_picker_resolved(
        paragraph,
        &resolved_runs,
        em.palette,
        em.cmyk_xform,
        em.options.fallback_text_paint,
        None,
    );
    let stroke_picker = build_run_stroke_picker(
        paragraph,
        &resolved_runs,
        em.palette,
        em.cmyk_xform,
        0,
    );
    let any_text_stroke = stroke_picker.any_visible();
    let leading_pt = paragraph_size * 1.2;
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
        for (line_idx, line) in laid_out.lines.iter().enumerate() {
            let start = line.byte_range.start.min(paragraph_text.len());
            let end = line.byte_range.end.min(paragraph_text.len());
            let source_text = paragraph_text
                .get(start..end)
                .unwrap_or("")
                .to_string();
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

    // Phase 3 Item A (table-cell path) — capture StoryLayout for
    // table-cell paragraphs so the canvas's caret + selection can
    // address text inside tables. Cell text shares the host story
    // id; for paragraph_idx we use the emitter's current value
    // (matches BreakRecord semantics for cells above). Phase 3 v1
    // limit: nested table-cell selection isn't separately
    // addressable from cell text on the same paragraph_idx —
    // multiple cells on the same table row would collide on
    // line_idx. Acceptable until we add a cell-coordinate axis to
    // LineLayout (Phase 3 v2).
    {
        let host_page_id = pages[target_page].id.clone();
        for (line_idx, line) in laid_out.lines.iter().enumerate() {
            let baseline_pt_local =
                line.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION;
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
                let x_pt_page =
                    cell_origin.0 + g.x as f32 / paged_text::shape::ADVANCE_PRECISION;
                clusters.push(ClusterPos {
                    byte: g.cluster,
                    x_pt: x_pt_page,
                    advance_pt: adv,
                });
            }
            pages[target_page].story_layout.push(LineLayout {
                story_id: em.current_story_id.clone(),
                page_id: host_page_id.clone(),
                paragraph_idx: em.paragraph_idx,
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
    let mut max_baseline_pt = 0.0f32;
    for line in &laid_out.lines {
        let baseline_pt = line.baseline_y as f32 / paged_text::shape::ADVANCE_PRECISION;
        if baseline_pt > max_baseline_pt {
            max_baseline_pt = baseline_pt;
        }
        let mut start = 0;
        while start < line.glyphs.len() {
            let fid = line.glyphs[start].font_id;
            let mut end = start + 1;
            while end < line.glyphs.len() && line.glyphs[end].font_id == fid {
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
    let glyph_count: usize = laid_out.lines.iter().map(|l| l.glyphs.len()).sum();
    total_stats.paragraphs += 1;
    total_stats.runs += paragraph.runs.len();
    total_stats.glyphs += glyph_count;
    total_stats.lines += laid_out.lines.len();
    pages[target_page].stats.paragraphs += 1;
    pages[target_page].stats.runs += paragraph.runs.len();
    pages[target_page].stats.glyphs += glyph_count;
    pages[target_page].stats.lines += laid_out.lines.len();
    max_baseline_pt + leading_pt * 0.4
}
