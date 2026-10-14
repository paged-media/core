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

//! Align to baseline grid (ADR 140).
//!
//! The rule, asked of InDesign 20.0.1 through the `baseline-grid` and
//! `baseline-grid-margin` fixtures: a line of a paragraph aligned to the
//! grid (`GridAlignment="AlignBaseline"`) stands on the first grid line AT
//! OR BELOW where its leading put it, and every line after it is placed
//! from there. So 10/12 text on a 14 pt grid advances 14 pt a line, a
//! 28.8 pt heading three grid lines, and leading 15 two. With
//! `GridAlignFirstLineOnly` only the paragraph's first line moves; the
//! others keep their leading from it.
//!
//! The grid lines are `origin + k × step` for `k ≥ 0` only: a frame that
//! opens above the first one sets its first line ON the first one, not on
//! a line extended upward. The origin is the page's top edge or top
//! margin plus `BaselineStart` (`<GridPreference>`), or, for a frame that
//! uses its own grid (`<BaselineFrameGridOption
//! UseCustomBaselineFrameGrid="true">`), the top of the page, margin,
//! frame or inset plus its starting offset.

use paged_model::{BaselineFrameGridRelativeOption, GridAlignment, TextFrame};
use paged_scene::{Document, ResolvedParagraphAttrs};

use super::text_frame::frame_spread_top_left;
use super::BuiltPage;

/// Within this distance (pt) above a grid line a baseline counts as on
/// it: the composer's 1/64 pt positions round, the grid does not.
const ON_GRID_PT: f64 = 0.01;

/// A frame's baseline grid in the frame's own coordinates: lines at
/// `origin_pt + k × step_pt` (pt from the frame's top edge), `k ≥ 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Grid {
    pub(super) origin_pt: f32,
    pub(super) step_pt: f32,
}

impl Grid {
    /// How far (1/64 pt) a baseline at `baseline_64` moves to stand on
    /// the first grid line at or below it.
    pub(super) fn snap_64(&self, baseline_64: i32) -> i32 {
        let precision = f64::from(paged_text::shape::ADVANCE_PRECISION);
        let y = f64::from(baseline_64) / precision;
        let origin = f64::from(self.origin_pt);
        let step = f64::from(self.step_pt);
        let k = ((y - origin - ON_GRID_PT) / step).ceil().max(0.0);
        let target_64 = ((origin + k * step) * precision).round() as i32;
        target_64 - baseline_64
    }

    /// [`Self::snap_64`] for an exact baseline (1/64 pt, unrounded): how
    /// far it moves, exactly, to the grid line at or below it.
    pub(super) fn snap_exact(&self, baseline: f64) -> f64 {
        let precision = f64::from(paged_text::shape::ADVANCE_PRECISION);
        let y = baseline / precision;
        let origin = f64::from(self.origin_pt);
        let step = f64::from(self.step_pt);
        let k = ((y - origin - ON_GRID_PT) / step).ceil().max(0.0);
        (origin + k * step) * precision - baseline
    }
}

/// Whether a line of a paragraph aligns to the grid: every line of an
/// `AlignBaseline` paragraph, or only its first with
/// `GridAlignFirstLineOnly`. The CJK em-box / ICF alignments are not
/// laid out.
pub(super) fn line_aligns(attrs: &ResolvedParagraphAttrs, paragraph_first_line: bool) -> bool {
    attrs.grid_alignment == Some(GridAlignment::AlignBaseline)
        && (paragraph_first_line || attrs.grid_align_first_line_only != Some(true))
}

/// The baseline grid `frame` sets its aligned lines on, on `page`: its
/// own when it uses one, else the document's. A grid measured from the
/// page needs the frame upright and unscaled, so the frame's own
/// coordinates are the page's shifted; any other frame keeps its lines
/// where the leading put them.
pub(super) fn frame_grid(document: &Document, frame: &TextFrame, page: &BuiltPage) -> Option<Grid> {
    let custom = frame.baseline_frame_grid.and_then(|g| g.custom());
    let (from_page_pt, step_pt) = match custom {
        Some((BaselineFrameGridRelativeOption::TopOfFrame, offset, step)) => {
            return Grid::checked(offset, step);
        }
        Some((BaselineFrameGridRelativeOption::TopOfInset, offset, step)) => {
            let inset_top = frame.inset_spacing.map_or(0.0, |i| i[0]);
            return Grid::checked(inset_top + offset, step);
        }
        Some((BaselineFrameGridRelativeOption::TopOfPage, offset, step)) => (offset, step),
        Some((BaselineFrameGridRelativeOption::TopOfMargin, offset, step)) => {
            (top_margin(document, page) + offset, step)
        }
        None => {
            let grid = document.designmap.grid_preference.baseline_grid();
            let reference = if grid.from_top_margin {
                top_margin(document, page)
            } else {
                0.0
            };
            (reference + grid.start, grid.division)
        }
    };
    if !translation_only(frame.item_transform) {
        return None;
    }
    let (_, frame_top) = frame_spread_top_left(frame.bounds, frame.item_transform);
    let frame_top_on_page = frame_top - page.spread_origin.1;
    Grid::checked(from_page_pt - frame_top_on_page, step_pt)
}

impl Grid {
    fn checked(origin_pt: f32, step_pt: f32) -> Option<Self> {
        (step_pt.is_finite() && origin_pt.is_finite() && step_pt >= 0.1)
            .then_some(Self { origin_pt, step_pt })
    }
}

fn top_margin(document: &Document, page: &BuiltPage) -> f32 {
    super::geom::resolve_page_margin_box(document, page).map_or(0.0, |m| m.top)
}

fn translation_only(m: Option<[f32; 6]>) -> bool {
    const EPS: f32 = 1e-4;
    m.is_none_or(|m| {
        (m[0] - 1.0).abs() < EPS && m[1].abs() < EPS && m[2].abs() < EPS && (m[3] - 1.0).abs() < EPS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: i32 = paged_text::shape::ADVANCE_PRECISION as i32;

    fn pt(v: f32) -> i32 {
        (v * P as f32).round() as i32
    }

    fn snapped(grid: Grid, baseline: f32) -> f32 {
        (pt(baseline) + grid.snap_64(pt(baseline))) as f32 / P as f32
    }

    /// The document grid of the `baseline-grid` fixture, seen from a
    /// frame whose top is at 100 pt on the page: lines at 36 + 14k on the
    /// page, so at -64 + 14k in the frame.
    const PAGE_GRID_AT_100: Grid = Grid {
        origin_pt: -64.0,
        step_pt: 14.0,
    };

    #[test]
    fn a_line_moves_down_to_the_next_grid_line() {
        // Case 2: the first baseline at 10.688 (110.688 on the page)
        // stands on 120.
        assert_eq!(snapped(PAGE_GRID_AT_100, 10.6875), 20.0);
        // The next line, 12 below it, on 134; and so on.
        assert_eq!(snapped(PAGE_GRID_AT_100, 32.0), 34.0);
    }

    #[test]
    fn a_line_on_a_grid_line_stays() {
        assert_eq!(snapped(PAGE_GRID_AT_100, 20.0), 20.0);
        // A hair below a grid line by the composer's rounding is on it: an
        // exact baseline 1/128 pt below moves up onto the line, not down
        // a step.
        let exact = (20.0 + 1.0 / 128.0) * f64::from(P);
        assert!(PAGE_GRID_AT_100.snap_exact(exact).abs() < 1.0);
    }

    #[test]
    fn a_heading_takes_as_many_grid_lines_as_its_leading_needs() {
        // Case 6: 288 + 28.8 = 316.8 on the page stands on 330.
        let grid = Grid {
            origin_pt: 36.0,
            step_pt: 14.0,
        };
        assert_eq!(snapped(grid, 316.8), 330.0);
        // Case 9: leading 15 from a grid line moves two grid lines.
        assert_eq!(snapped(grid, 442.0 + 15.0), 470.0);
    }

    #[test]
    fn no_grid_line_lies_above_the_first() {
        // Case 12: a frame at 6 pt on the page, its first baseline at
        // 16.688; the grid starts at 36, so the line stands on 36, not on
        // 22 (36 - 14).
        let grid = Grid {
            origin_pt: 30.0,
            step_pt: 14.0,
        };
        assert_eq!(snapped(grid, 10.6875), 30.0);
    }

    #[test]
    fn only_align_baseline_aligns_and_first_line_only_keeps_the_rest() {
        let mut attrs = ResolvedParagraphAttrs {
            grid_alignment: Some(GridAlignment::AlignBaseline),
            ..Default::default()
        };
        assert!(line_aligns(&attrs, true));
        assert!(line_aligns(&attrs, false));
        attrs.grid_align_first_line_only = Some(true);
        assert!(line_aligns(&attrs, true));
        assert!(!line_aligns(&attrs, false));
        for other in [
            None,
            Some(GridAlignment::None),
            Some(GridAlignment::AlignEmCenter),
        ] {
            attrs.grid_alignment = other;
            assert!(!line_aligns(&attrs, true), "{other:?}");
        }
    }

    #[test]
    fn a_degenerate_grid_is_none() {
        assert_eq!(Grid::checked(0.0, 0.0), None);
        assert_eq!(Grid::checked(f32::NAN, 12.0), None);
        assert!(Grid::checked(0.0, 12.0).is_some());
    }

    #[test]
    fn a_rotated_or_scaled_frame_takes_no_page_grid() {
        assert!(translation_only(None));
        assert!(translation_only(Some([1.0, 0.0, 0.0, 1.0, 20.0, 30.0])));
        assert!(!translation_only(Some([0.0, 1.0, -1.0, 0.0, 0.0, 0.0])));
        assert!(!translation_only(Some([2.0, 0.0, 0.0, 2.0, 0.0, 0.0])));
    }
}
