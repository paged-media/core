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

//! thoughts ADR 028 — backing the emitter up to a paragraph boundary
//! inside one pass, so a keep option is decided when the lines it reads are
//! placed (`keeps::paragraph_break`) instead of one whole-story pass later.
//!
//! Only for a story whose emission is a pure function of the flow state at
//! a paragraph boundary and moves forward over its pages
//! (`resume::eligible`, the ADR 027 early-stop premise): its output before
//! a paragraph lies on the pages up to the current frame's, and what came
//! after is cut off by truncating that page to its sizes then and every
//! later page to its sizes before the story.

use super::footnotes::{rollback_page, snapshot_page_reset, BodyStoryPageReset};
use super::{BuiltPage, PipelineStats, SegmentState, StoryEmitter};

/// The emitter where one paragraph began, and the story's output so far.
pub(super) struct Rewind {
    paragraph_idx: u32,
    frame_idx: usize,
    y_cursor: i32,
    prev_line_height_64: Option<i32>,
    numbered_counter: u32,
    prev_was_numbered: bool,
    last_placed_frame: Option<usize>,
    overset_reported: bool,
    force_overset: bool,
    frame_first_line: Option<(u32, u32)>,
    segment: SegmentState,
    /// The current frame (clamped to the chain) and its partial state;
    /// every later frame is still empty.
    cur: usize,
    cur_range: Option<(usize, usize)>,
    cur_max_baseline_64: i32,
    cur_paragraph_ranges: usize,
    cur_placed_height_64: Option<i64>,
    overset_height_64: i64,
    diagnostics: usize,
    breaks: usize,
    anchored: usize,
    page: usize,
    page_reset: BodyStoryPageReset,
    stats: PipelineStats,
}

impl Rewind {
    pub(super) fn capture(em: &StoryEmitter, pages: &[BuiltPage], stats: &PipelineStats) -> Self {
        let cur = em.frame_idx.min(em.chain_pages.len().saturating_sub(1));
        let page = em.chain_pages.get(cur).copied().unwrap_or(0);
        Rewind {
            paragraph_idx: em.paragraph_idx,
            frame_idx: em.frame_idx,
            y_cursor: em.y_cursor,
            prev_line_height_64: em.prev_line_height_64,
            numbered_counter: em.numbered_counter,
            prev_was_numbered: em.prev_was_numbered,
            last_placed_frame: em.last_placed_frame,
            overset_reported: em.overset_reported,
            force_overset: em.force_overset,
            frame_first_line: em.frame_first_line,
            segment: em.segment,
            cur,
            cur_range: em.frame_cmd_ranges.get(cur).copied().flatten(),
            cur_max_baseline_64: em.frame_max_baseline_64.get(cur).copied().unwrap_or(0),
            cur_paragraph_ranges: em.paragraph_cmd_ranges.get(cur).map_or(0, Vec::len),
            cur_placed_height_64: em.placed_height_64.get(&cur).copied(),
            overset_height_64: em.overset_height_64,
            diagnostics: em.diagnostics.len(),
            breaks: em.breaks.len(),
            anchored: em.anchored_image_queue.len(),
            page,
            page_reset: snapshot_page_reset(&pages[page]),
            stats: *stats,
        }
    }

    /// The paragraph this rewind starts.
    pub(super) fn paragraph(&self) -> usize {
        self.paragraph_idx as usize
    }

    /// Put `em`, `pages` and `stats` back to where the paragraph began.
    /// `pre` holds every page's sizes before the story. The keep state
    /// (`forced_breaks`) is the caller's: it is what changed.
    pub(super) fn restore(
        &self,
        em: &mut StoryEmitter,
        pages: &mut [BuiltPage],
        pre: &[BodyStoryPageReset],
        stats: &mut PipelineStats,
    ) {
        rollback_page(&mut pages[self.page], &self.page_reset);
        for (page, reset) in pages.iter_mut().zip(pre).skip(self.page + 1) {
            rollback_page(page, reset);
        }
        let cur = self.cur;
        for i in cur..em.frame_cmd_ranges.len() {
            let here = i == cur;
            em.frame_cmd_ranges[i] = if here { self.cur_range } else { None };
            em.frame_max_baseline_64[i] = if here { self.cur_max_baseline_64 } else { 0 };
        }
        for (i, ranges) in em.paragraph_cmd_ranges.iter_mut().enumerate().skip(cur) {
            ranges.truncate(if i == cur { self.cur_paragraph_ranges } else { 0 });
        }
        em.placed_height_64.retain(|&f, _| f < cur);
        if let Some(h) = self.cur_placed_height_64 {
            em.placed_height_64.insert(cur, h);
        }
        em.overset_height_64 = self.overset_height_64;
        em.diagnostics.truncate(self.diagnostics);
        em.breaks.truncate(self.breaks);
        em.anchored_image_queue.truncate(self.anchored);
        em.paragraph_idx = self.paragraph_idx;
        em.frame_idx = self.frame_idx;
        em.y_cursor = self.y_cursor;
        em.prev_line_height_64 = self.prev_line_height_64;
        em.numbered_counter = self.numbered_counter;
        em.prev_was_numbered = self.prev_was_numbered;
        em.last_placed_frame = self.last_placed_frame;
        em.overset_reported = self.overset_reported;
        em.force_overset = self.force_overset;
        em.frame_first_line = self.frame_first_line;
        em.segment = self.segment;
        let p = self.paragraph();
        em.placements.truncate(p);
        em.keep_specs.truncate(p);
        let p = self.paragraph_idx;
        em.applied_breaks.retain(|&q| q < p);
        *stats = self.stats;
    }
}

/// After paragraph `j` was laid out: settle the keep decisions its
/// placement completes (`keeps::paragraph_break`), in flow order — the
/// previous paragraph's keep with next, which reads `j`'s first lines,
/// then `j`'s own rules (when it has no keep with next, or is the last
/// paragraph). Returns the paragraph to lay out again, when a decision
/// changed its forced break. Paragraphs before `first` are not laid out
/// in this pass (a resumed prefix) and are left to the pass's
/// `keep_breaks`.
pub(super) fn settle_after(em: &mut StoryEmitter, j: usize, first: usize, last: bool) -> Option<usize> {
    use super::keeps::{paragraph_break, settle_paragraph};
    let judge = |em: &mut StoryEmitter, p: usize| -> bool {
        let fresh = paragraph_break(&em.placements, &em.keep_specs, p, &em.reserved_footnote_64);
        let applied = em.applied_breaks.contains(&(p as u32));
        settle_paragraph(&mut em.forced_breaks, p as u32, applied, fresh)
    };
    if j > first && em.keep_specs.get(j - 1).is_some_and(|s| s.with_next > 0) && judge(em, j - 1) {
        return Some(j - 1);
    }
    let own = last || em.keep_specs.get(j).is_none_or(|s| s.with_next == 0);
    (own && judge(em, j)).then_some(j)
}
