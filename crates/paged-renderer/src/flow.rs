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

//! The publishing engine's **flow content** — the IDML/print side of the
//! region-chain protocol (`paged_flow`), migration slice **S2**.
//!
//! [`TextFlow`] implements [`paged_flow::FlowContent`] for laid-out text: it
//! band-packs a story's **already-shaped lines** down each region in a chain,
//! advancing to the next region when a line's baseline would fall past the
//! region's content height — exactly the rule the renderer's `StoryEmitter`
//! applies imperatively today (`build_engine.rs`: advance when `baseline_y >
//! text_bottom`). Unlike `StoryEmitter`, the leftover past the last region is a
//! **first-class continuation** (`paged_flow::Overset::Remains`), not dropped.
//!
//! This is the *real* (non-synthetic) content engine behind the protocol — it
//! fragments actual `paged-text` output — proving the protocol drives genuine
//! shaped text, not just the `paged-flow` unit-test stub. It does **not** yet
//! replace `StoryEmitter` (that is slice S3, where the two become the same code
//! path). Present limitations, resolved in later slices:
//! - **uniform leading** — one line height per flow (the common single-size
//!   paragraph); mixed-size leading is S3.
//! - **no per-region re-line-breaking** — lines are shaped once at a reference
//!   width, then packed by height; variable-width regions that must re-wrap
//!   (the paged.web rung-2 behaviour) are S3/S6.

use paged_flow::{region_overflows, FlowContent, Placement, Region};

/// One shaped line's vertical footprint within a flow: the slot it advances
/// (`height_pt`, the leading) and where its baseline sits inside that slot
/// (`baseline_offset_pt`, ~the ascent). Together these place the line's
/// baseline as `slot_top + baseline_offset_pt`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowLine {
    /// Vertical advance of this line's slot, in pt (the leading).
    pub height_pt: f32,
    /// Baseline position within the slot, measured from the slot top, in pt.
    pub baseline_offset_pt: f32,
}

/// A line placed into a region: which flow line, and its baseline in
/// **region-content coordinates** (0 = the region's content-box top).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedLine {
    /// Index into the flow's line list.
    pub line: usize,
    /// Baseline y within this region's content box, in pt.
    pub baseline_y_pt: f32,
}

/// A publishing/IDML **text flow**: an ordered list of shaped lines that
/// [`paged_flow::run_flow`] fragments across a region-chain.
#[derive(Debug, Clone, Default)]
pub struct TextFlow {
    lines: Vec<FlowLine>,
}

impl TextFlow {
    /// A flow from explicit per-line footprints.
    pub fn new(lines: Vec<FlowLine>) -> Self {
        TextFlow { lines }
    }

    /// A flow of `count` lines with uniform leading — the common
    /// single-point-size paragraph. `line_height_pt` is the baseline advance;
    /// `first_baseline_pt` is the first line's baseline below the content top
    /// (matching `paged_text::LayoutOptions::new`, which advances the baseline
    /// by `line_height` per line starting at `first_baseline`).
    pub fn uniform(count: usize, line_height_pt: f32, first_baseline_pt: f32) -> Self {
        TextFlow {
            lines: vec![
                FlowLine {
                    height_pt: line_height_pt,
                    baseline_offset_pt: first_baseline_pt,
                };
                count
            ],
        }
    }

    /// Number of lines in the flow.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

impl FlowContent for TextFlow {
    /// The lines that landed in one region, with region-relative baselines.
    type Fragment = Vec<PlacedLine>;
    /// The index of the next line to place — a real continuation position.
    type Cursor = usize;

    fn start(&self) -> usize {
        0
    }

    fn place(&self, region: &Region, cursor: usize) -> Placement<Vec<PlacedLine>, usize> {
        let height = region.geometry.height_pt;
        let mut placed = Vec::new();
        // `slot_top` is the top of the current line's slot within this region;
        // regions restart at 0 (the flow re-bases per region — LESSONS #1: the
        // region-chain windows the content).
        let mut slot_top = 0.0f32;
        let mut i = cursor;
        while i < self.lines.len() {
            let line = self.lines[i];
            let baseline = slot_top + line.baseline_offset_pt;
            // A line belongs to this region while its baseline is within the
            // content box — decided by the shared `region_overflows` rule, the
            // same boundary StoryEmitter uses to advance frames (advance when
            // `baseline_y > text_bottom`). The epsilon (added to the bottom)
            // absorbs float slack so an exact fit lands.
            if !region_overflows(baseline, height + 0.01) {
                placed.push(PlacedLine {
                    line: i,
                    baseline_y_pt: baseline,
                });
                slot_top += line.height_pt;
                i += 1;
            } else {
                break;
            }
        }
        let next = if i < self.lines.len() { Some(i) } else { None };
        Placement {
            fragment: placed,
            next,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_flow::{run_flow, FlowId, Overset, RegionChain, RegionGeometry};
    use paged_text::{layout_paragraph, LayoutOptions, MonospaceMeasurer};

    /// A chain of single-column regions of the given content heights (pt), all
    /// 400pt wide.
    fn chain(heights: &[f32]) -> RegionChain {
        let regions = heights
            .iter()
            .enumerate()
            .map(|(i, &h)| paged_flow::Region::new(format!("r{i}"), RegionGeometry::new(400.0, h)))
            .collect();
        RegionChain::new(FlowId::new("story"), regions)
    }

    /// Lay real text out through the font-free monospace shaper, then wrap it
    /// as a `TextFlow`. The line *count* comes from genuine line-breaking; the
    /// leading is uniform (single point size).
    fn shaped_flow(text: &str, width_pt: f32, point_size: f32) -> (TextFlow, f32) {
        let shaper = MonospaceMeasurer::new(
            (point_size * 0.6 * 64.0) as i32, // char advance, 1/64 pt
            (point_size * 0.3 * 64.0) as i32, // space advance
        );
        let opts = LayoutOptions::new(width_pt, point_size);
        let para = layout_paragraph(text, &shaper, &opts);
        let line_height_pt = point_size * 1.2; // LayoutOptions::new default
        let first_baseline_pt = point_size * 0.8;
        let flow = TextFlow::uniform(para.lines.len(), line_height_pt, first_baseline_pt);
        (flow, line_height_pt)
    }

    #[test]
    fn real_shaped_text_fragments_across_regions() {
        // Narrow column forces the sentence to wrap into several lines.
        let (flow, lh) = shaped_flow(
            "The quick brown fox jumps over the lazy dog again and again and again.",
            120.0,
            12.0,
        );
        assert!(
            flow.line_count() >= 3,
            "expected the text to wrap into several lines"
        );

        // A chain tall enough for everything → all lines placed, no overset.
        let tall = flow.line_count() as f32 * lh + 20.0;
        let run = run_flow(&flow, &chain(&[tall]));
        let placed: usize = run.placements.iter().map(|(_, f)| f.len()).sum();
        assert_eq!(placed, flow.line_count());
        assert_eq!(run.overset, Overset::Fits);
    }

    #[test]
    fn short_first_region_oversets_into_a_continuation() {
        let (flow, lh) = shaped_flow(
            "The quick brown fox jumps over the lazy dog again and again and again.",
            120.0,
            12.0,
        );
        let n = flow.line_count();
        // A single region that holds ~2 lines → the rest is a real continuation.
        let two_lines = lh * 2.0 + 2.0;
        let run = run_flow(&flow, &chain(&[two_lines]));
        let placed_first = run.placements[0].1.len();
        assert!(
            placed_first >= 1 && placed_first < n,
            "some but not all lines fit"
        );
        match run.overset {
            Overset::Remains(cursor) => assert_eq!(cursor, placed_first),
            Overset::Fits => panic!("expected overset for a too-short chain"),
        }
    }

    #[test]
    fn nothing_is_lost_across_the_chain() {
        // The continuation preserves every line — contrast StoryEmitter, which
        // drops overflow past the last frame.
        let (flow, lh) = shaped_flow(
            "Alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu.",
            100.0,
            10.0,
        );
        let n = flow.line_count();
        let run = run_flow(&flow, &chain(&[lh * 1.5, lh * 1.5]));
        let placed: usize = run.placements.iter().map(|(_, f)| f.len()).sum();
        let remaining = match run.overset {
            Overset::Fits => 0,
            Overset::Remains(cursor) => n - cursor,
        };
        assert_eq!(
            placed + remaining,
            n,
            "every line is either placed or a continuation"
        );
    }

    #[test]
    fn baselines_are_region_relative_and_increasing() {
        let flow = TextFlow::uniform(6, 14.0, 11.0);
        // Two regions, each holding several lines; the second must re-base its
        // baselines from the top (0), not continue the first's coordinates.
        let run = run_flow(&flow, &chain(&[40.0, 100.0]));
        for (_, frag) in &run.placements {
            // Each region's first placed baseline equals the first-baseline
            // offset (re-based to the region top).
            if let Some(first) = frag.first() {
                assert!((first.baseline_y_pt - 11.0).abs() < 0.01);
            }
            // Baselines strictly increase within a region.
            for w in frag.windows(2) {
                assert!(w[1].baseline_y_pt > w[0].baseline_y_pt);
            }
        }
    }

    #[test]
    fn taller_region_places_at_least_as_many_lines() {
        let flow = TextFlow::uniform(20, 12.0, 9.0);
        let mut prev = 0usize;
        for h in [20.0f32, 40.0, 80.0, 160.0] {
            let run = run_flow(&flow, &chain(&[h]));
            let placed = run.placements[0].1.len();
            assert!(placed >= prev, "line count is monotonic in region height");
            prev = placed;
        }
    }

    #[test]
    fn region_too_short_for_one_line_moves_it_whole() {
        // First region can't fit even one line (baseline 9 > height 5) → empty
        // fragment, the line moves whole to the taller second region.
        let flow = TextFlow::uniform(2, 12.0, 9.0);
        let run = run_flow(&flow, &chain(&[5.0, 100.0]));
        assert!(run.placements[0].1.is_empty());
        assert_eq!(run.placements[1].1.len(), 2);
        assert_eq!(run.overset, Overset::Fits);
    }
}
