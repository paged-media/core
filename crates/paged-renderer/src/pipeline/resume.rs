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

//! thoughts ADR 027 §3 / plan step 6 — early stop inside a story.
//!
//! An edited story used to re-emit from its first paragraph. Its previous
//! emission is now kept with a [`ParaMark`] per paragraph: the emitter's
//! flow state where the paragraph began and how much of each page's output
//! came before it. An edit that changed paragraphs `[first, new_end)`
//! (`[first, old_end)` before it):
//!
//! - **resumes** at `first`: the output of the paragraphs before it is
//!   spliced from the previous emission and the emitter restored to its
//!   state there (those paragraphs are unchanged, and so is everything
//!   their layout reads, which the plan checks);
//! - **stops** at the first paragraph boundary after the edit whose flow
//!   state equals the previous emission's at the same paragraph: same
//!   frame, same baseline cursor and leading, same list counter, the same
//!   path-buffer print on the page. Every later paragraph is unchanged
//!   content laid out from the same state, so its output is the previous
//!   emission's, spliced, with paragraph indices and command offsets
//!   shifted.
//!
//! The granularity is a paragraph boundary, not a frame boundary
//! (`emit_paragraph` lays a paragraph out whole), which is finer: a stop
//! can happen inside a frame. Only stories whose emission is a pure
//! function of that state are recorded (see [`eligible`]); everything
//! else re-emits from the top as before. The digest gate compares the
//! result with a cold build after every scripted op.

use std::collections::HashMap;

use paged_model::TextFrame;

use super::keeps::{KeepSpec, LinePlace};
use super::{pool_print, BodyStoryPageDelta, BuiltPage, Diagnostic, PipelineStats, StoryEmitter};

/// The paragraphs one text edit changed, by index: `[first, new_end)` now,
/// `[first, old_end)` before the edit. Every paragraph before `first` is
/// unchanged, and so is every paragraph from `new_end` on (which was at
/// `old_end + i`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditSpan {
    pub first: u32,
    pub old_end: u32,
    pub new_end: u32,
}

/// The emitter's flow state where one paragraph began, and how much of
/// the story's output on its page came before it.
#[derive(Debug, Clone, Default)]
pub(super) struct ParaMark {
    frame_idx: usize,
    y_cursor: i32,
    prev_line_height_64: Option<i32>,
    numbered_counter: u32,
    prev_was_numbered: bool,
    last_placed_frame: Option<usize>,
    overset_reported: bool,
    force_overset: bool,
    /// The current frame's deepest baseline so far.
    cur_max_baseline_64: i32,
    /// The current frame's command range so far, relative to the story's
    /// first command on `page`.
    cur_range: Option<(usize, usize)>,
    /// The page of the current frame, and the story's paths, commands and
    /// `story_layout` lines on it so far.
    page: usize,
    paths: usize,
    cmds: usize,
    lines: usize,
    /// `pool_print` of that page.
    print: u64,
    /// Diagnostics reported so far.
    diags: usize,
    /// What the story added to the build's stats so far.
    stats: PipelineStats,
}

impl ParaMark {
    /// The flow continues from here exactly as it did from `other`.
    fn same_flow(&self, other: &ParaMark) -> bool {
        self.frame_idx == other.frame_idx
            && self.y_cursor == other.y_cursor
            && self.prev_line_height_64 == other.prev_line_height_64
            && self.numbered_counter == other.numbered_counter
            && self.prev_was_numbered == other.prev_was_numbered
            && self.last_placed_frame == other.last_placed_frame
            && self.overset_reported == other.overset_reported
            && self.force_overset == other.force_overset
            && self.cur_max_baseline_64 == other.cur_max_baseline_64
            && self.cur_range.is_some() == other.cur_range.is_some()
            && self.page == other.page
            && self.paths == other.paths
            && self.print == other.print
    }
}

/// One story's previous emission, kept for the next edit.
#[derive(Debug, Clone)]
pub struct StoryResume {
    /// `body_story_signature` of the chain it was laid out in.
    chain_sig: u64,
    /// The build that wrote it (a record is never resumed from in the
    /// build that wrote it: the grow loop's later passes see the same
    /// edit span against content the record already holds).
    generation: u64,
    /// The story's output per page, as the body-story cache captures it.
    per_page: Vec<(usize, BodyStoryPageDelta)>,
    /// One mark per paragraph, at its start, and one for the end.
    marks: Vec<ParaMark>,
    /// The emitter's per-frame command ranges at the end, relative to the
    /// story's first command on the frame's page.
    frame_ranges: Vec<Option<(usize, usize)>>,
    frame_max_baseline_64: Vec<i32>,
    placements: Vec<Vec<LinePlace>>,
    keep_specs: Vec<KeepSpec>,
    forced: HashMap<u32, u32>,
    diagnostics: Vec<Diagnostic>,
}

/// Story id and "is the post-layout pass" → [`StoryResume`].
pub type StoryResumeStore = std::cell::RefCell<HashMap<(String, bool), StoryResume>>;

/// Per page, the sizes the story started from: paths, commands, gradients,
/// radial gradients, images, `story_layout`, footnotes.
pub(super) type PreSnapshot = (usize, usize, usize, usize, usize, usize, usize);

/// Whether a story's emission can be recorded and resumed: a pure function
/// of the flow state in [`ParaMark`], laid out forward over pages, with no
/// post-pass rewriting its commands afterwards. Checked on the content and
/// chain; the caller also checks at runtime that the post-passes added
/// nothing.
pub(super) fn eligible(
    story: &paged_model::Story,
    chain: &[&TextFrame],
    chain_pages: &[usize],
    options: &super::PipelineOptions,
) -> bool {
    !options.collect_breaks
        && !options.collect_link_regions
        && !options.collect_glyph_runs
        && chain_pages.windows(2).all(|w| w[0] <= w[1])
        && chain.iter().all(|f| {
            matches!(
                f.vertical_justification,
                None | Some(paged_model::VerticalJustification::Top)
            ) && f.column_balance != Some(true)
        })
        && story.paragraphs.iter().all(|p| {
            p.anchored_frames.is_empty()
                && p.table.is_none()
                && p.footnotes.is_empty()
                && p.span_columns == paged_model::SpanColumns::default()
        })
}

/// The mark for the paragraph about to be emitted.
pub(super) fn mark(
    em: &StoryEmitter,
    pages: &[BuiltPage],
    pre: &[PreSnapshot],
    stats: &PipelineStats,
    stats_base: &PipelineStats,
) -> ParaMark {
    let frame_idx = em.frame_idx.min(em.chain_pages.len().saturating_sub(1));
    let page = em.chain_pages.get(frame_idx).copied().unwrap_or(0);
    let (p, snap) = (&pages[page], &pre[page]);
    ParaMark {
        frame_idx: em.frame_idx,
        y_cursor: em.y_cursor,
        prev_line_height_64: em.prev_line_height_64,
        numbered_counter: em.numbered_counter,
        prev_was_numbered: em.prev_was_numbered,
        last_placed_frame: em.last_placed_frame,
        overset_reported: em.overset_reported,
        force_overset: em.force_overset,
        cur_max_baseline_64: em
            .frame_max_baseline_64
            .get(frame_idx)
            .copied()
            .unwrap_or(0),
        cur_range: em
            .frame_cmd_ranges
            .get(frame_idx)
            .copied()
            .flatten()
            .map(|(s, e)| (s - snap.1, e - snap.1)),
        page,
        paths: p.list.paths.len() - snap.0,
        cmds: p.list.commands.len() - snap.1,
        lines: p.story_layout.len() - snap.5,
        print: pool_print(&p.list),
        diags: em.diagnostics.len(),
        stats: stats.emitted_since(stats_base),
    }
}

/// A resume the previous emission allows for this edit.
pub(super) struct Plan<'r> {
    rec: &'r StoryResume,
    span: EditSpan,
}

impl Plan<'_> {
    /// The first paragraph to emit.
    pub(super) fn first(&self) -> usize {
        self.span.first as usize
    }

    /// Paragraph `j` of the edited story is past the edit, so it may stop
    /// there.
    pub(super) fn past_edit(&self, j: usize) -> bool {
        j >= self.span.new_end as usize
    }

    /// The chain frame the resume starts in.
    pub(super) fn resume_frame(&self) -> usize {
        self.rec.marks[self.first()].frame_idx
    }

    /// The marks of the paragraphs before the edit, for the new record.
    pub(super) fn prefix_marks(&self) -> &[ParaMark] {
        &self.rec.marks[..self.first()]
    }

    fn old_index(&self, j: usize) -> usize {
        j + self.span.old_end as usize - self.span.new_end as usize
    }
}

/// Decide whether `rec` lets this pass resume at the edit. `forced` is the
/// pass's forced keep breaks; `paragraphs` the story's paragraph count now.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan<'r>(
    rec: &'r StoryResume,
    span: EditSpan,
    chain_sig: u64,
    generation: u64,
    paragraphs: usize,
    forced: &HashMap<u32, u32>,
    pages: &[BuiltPage],
) -> Option<Plan<'r>> {
    let (first, old_end, new_end) = (
        span.first as usize,
        span.old_end as usize,
        span.new_end as usize,
    );
    let old_paragraphs = (paragraphs + old_end).checked_sub(new_end)?;
    if rec.chain_sig != chain_sig
        || rec.generation == generation
        || rec.marks.len() != old_paragraphs + 1
        || first > old_end
        || first > new_end
        || old_end > old_paragraphs
        || new_end > paragraphs
    {
        return None;
    }
    // The paragraphs before the edit were laid out with the same forced
    // breaks, or their output is not this pass's.
    let before = |m: &HashMap<u32, u32>| -> Vec<(u32, u32)> {
        let mut v: Vec<(u32, u32)> = m
            .iter()
            .filter(|(p, _)| (**p as usize) < first)
            .map(|(p, l)| (*p, *l))
            .collect();
        v.sort_unstable();
        v
    };
    if before(forced) != before(&rec.forced) {
        return None;
    }
    // Every page the spliced prefix lands on holds the pools it was
    // captured against.
    let resume_page = rec.marks[first].page;
    let pools_hold = rec
        .per_page
        .iter()
        .filter(|(p, _)| *p <= resume_page)
        .all(|(p, d)| *p < pages.len() && pool_print(&pages[*p].list) == d.pre_fingerprint);
    pools_hold.then_some(Plan { rec, span })
}

fn shift_lines(
    lines: &[super::LineLayout],
    by: i64,
) -> impl Iterator<Item = super::LineLayout> + '_ {
    lines.iter().map(move |l| {
        let mut l = l.clone();
        l.paragraph_idx = (l.paragraph_idx as i64 + by) as u32;
        l
    })
}

/// Append `delta`'s paths `[paths..]`, commands `[cmds..]` and lines
/// `[lines..]` to `page` (the story started at `snap` on it), shifting
/// paragraph indices by `para_shift`.
fn splice_page_tail(
    page: &mut BuiltPage,
    delta: &BodyStoryPageDelta,
    snap: &PreSnapshot,
    (paths, cmds, lines): (usize, usize, usize),
    para_shift: i64,
) {
    for (path, key) in delta.paths[paths..].iter().zip(&delta.path_keys[paths..]) {
        page.list.paths.replay(*key, path.clone());
    }
    for cmd in &delta.commands[cmds..] {
        let mut c = cmd.clone();
        super::rebase_path_ids(&mut c, snap.0 as i64);
        page.list.commands.push(c);
    }
    page.story_layout
        .extend(shift_lines(&delta.story_layout[lines..], para_shift));
}

/// Splice the previous output of the paragraphs before the edit and
/// restore the emitter to its state where the edit begins.
pub(super) fn splice_prefix(
    em: &mut StoryEmitter,
    pages: &mut [BuiltPage],
    pre: &[PreSnapshot],
    plan: &Plan,
    stats: &mut PipelineStats,
) {
    let rec = plan.rec;
    let first = plan.first();
    let m = &rec.marks[first];
    for (p, delta) in &rec.per_page {
        if *p > m.page {
            continue;
        }
        let page = &mut pages[*p];
        let new_base = pre[*p].0 as i64;
        let end = if *p == m.page {
            (m.paths, m.cmds, m.lines)
        } else {
            (
                delta.paths.len(),
                delta.commands.len(),
                delta.story_layout.len(),
            )
        };
        for (path, key) in delta.paths[..end.0].iter().zip(&delta.path_keys[..end.0]) {
            page.list.paths.replay(*key, path.clone());
        }
        for cmd in &delta.commands[..end.1] {
            let mut c = cmd.clone();
            super::rebase_path_ids(&mut c, new_base);
            page.list.commands.push(c);
        }
        page.story_layout
            .extend(delta.story_layout[..end.2].iter().cloned());
    }
    let cur = m.frame_idx.min(em.chain_pages.len().saturating_sub(1));
    for i in 0..em.frame_cmd_ranges.len() {
        let base = pre[em.chain_pages[i]].1;
        em.frame_cmd_ranges[i] = match i.cmp(&cur) {
            std::cmp::Ordering::Less => rec.frame_ranges[i].map(|(s, e)| (s + base, e + base)),
            std::cmp::Ordering::Equal => m.cur_range.map(|(s, e)| (s + base, e + base)),
            std::cmp::Ordering::Greater => None,
        };
        em.frame_max_baseline_64[i] = match i.cmp(&cur) {
            std::cmp::Ordering::Less => rec.frame_max_baseline_64[i],
            std::cmp::Ordering::Equal => m.cur_max_baseline_64,
            std::cmp::Ordering::Greater => 0,
        };
    }
    em.frame_idx = m.frame_idx;
    em.y_cursor = m.y_cursor;
    em.prev_line_height_64 = m.prev_line_height_64;
    em.numbered_counter = m.numbered_counter;
    em.prev_was_numbered = m.prev_was_numbered;
    em.last_placed_frame = m.last_placed_frame;
    em.overset_reported = m.overset_reported;
    em.force_overset = m.force_overset;
    em.paragraph_idx = first as u32;
    em.placements = rec.placements[..first].to_vec();
    em.keep_specs = rec.keep_specs[..first].to_vec();
    em.diagnostics = rec.diagnostics[..m.diags].to_vec();
    stats.add_emitted(&m.stats);
}

/// At the start of paragraph `j` (past the edit), with `now` its mark:
/// when the flow is where the previous emission's was at the same
/// paragraph, splice the rest of the previous emission, leave the emitter
/// in its end state, extend `marks` with the shifted previous marks and
/// return the number of chain frames the emission touched since the
/// resume. `None` when it must keep emitting.
#[allow(clippy::too_many_arguments)]
pub(super) fn try_stop(
    em: &mut StoryEmitter,
    pages: &mut [BuiltPage],
    pre: &[PreSnapshot],
    plan: &Plan,
    j: usize,
    now: &ParaMark,
    stats: &mut PipelineStats,
    marks: Option<&mut Vec<ParaMark>>,
) -> Option<usize> {
    let rec = plan.rec;
    let jo = plan.old_index(j);
    let old = rec.marks.get(jo)?;
    if !now.same_flow(old) {
        return None;
    }
    // The paragraphs from here on were laid out under the same forced
    // breaks (none, when a resume was planned past every keep).
    if em.forced_breaks.keys().any(|p| *p as usize >= j)
        || rec.forced.keys().any(|p| *p as usize >= jo)
    {
        return None;
    }
    // Every later page the previous output lands on holds its pools.
    if !rec
        .per_page
        .iter()
        .filter(|(p, _)| *p > now.page)
        .all(|(p, d)| *p < pages.len() && pool_print(&pages[*p].list) == d.pre_fingerprint)
    {
        return None;
    }
    let para_shift = j as i64 - jo as i64;
    let d_cmds = now.cmds as i64 - old.cmds as i64;
    let d_lines = now.lines as i64 - old.lines as i64;
    let d_diags = now.diags as i64 - old.diags as i64;
    for (p, delta) in &rec.per_page {
        if *p < now.page {
            continue;
        }
        let tail = if *p == now.page {
            (old.paths, old.cmds, old.lines)
        } else {
            (0, 0, 0)
        };
        splice_page_tail(&mut pages[*p], delta, &pre[*p], tail, para_shift);
    }
    let end = rec.marks.last().expect("a record has an end mark");
    let cur = now.frame_idx.min(em.chain_pages.len().saturating_sub(1));
    let relaid = cur + 1 - rec.marks[plan.first()].frame_idx.min(cur);
    let new_cur_start = em.frame_cmd_ranges[cur].map(|(s, _)| s);
    for i in cur..em.frame_cmd_ranges.len() {
        let page = em.chain_pages[i];
        let base = pre[page].1 as i64;
        let shift = if page == now.page { d_cmds } else { 0 };
        em.frame_cmd_ranges[i] = rec.frame_ranges[i].map(|(s, e)| {
            let start = match (i == cur, new_cur_start) {
                (true, Some(s)) => s,
                _ => (s as i64 + base + shift) as usize,
            };
            (start, (e as i64 + base + shift) as usize)
        });
        em.frame_max_baseline_64[i] = rec.frame_max_baseline_64[i];
    }
    em.frame_idx = end.frame_idx;
    em.y_cursor = end.y_cursor;
    em.prev_line_height_64 = end.prev_line_height_64;
    em.numbered_counter = end.numbered_counter;
    em.prev_was_numbered = end.prev_was_numbered;
    em.last_placed_frame = end.last_placed_frame;
    em.overset_reported = end.overset_reported;
    em.force_overset = end.force_overset;
    em.paragraph_idx = (rec.placements.len() as i64 + para_shift) as u32;
    em.placements.truncate(j);
    em.placements.extend(rec.placements[jo..].iter().cloned());
    em.keep_specs.truncate(j);
    em.keep_specs.extend(rec.keep_specs[jo..].iter().copied());
    em.diagnostics
        .extend(rec.diagnostics[old.diags..].iter().cloned());
    stats.add_emitted(&end.stats.emitted_since(&old.stats));
    if let Some(marks) = marks {
        for m in &rec.marks[jo..] {
            let mut m = m.clone();
            if m.page == now.page {
                m.cmds = (m.cmds as i64 + d_cmds) as usize;
                m.lines = (m.lines as i64 + d_lines) as usize;
                m.cur_range = m.cur_range.map(|(s, e)| {
                    let start = if m.frame_idx == now.frame_idx {
                        now.cur_range.map_or(s, |(ns, _)| ns)
                    } else {
                        (s as i64 + d_cmds) as usize
                    };
                    (start, (e as i64 + d_cmds) as usize)
                });
            }
            m.diags = (m.diags as i64 + d_diags) as usize;
            let mut stats_at = now.stats;
            stats_at.add_emitted(&m.stats.emitted_since(&old.stats));
            m.stats = stats_at;
            marks.push(m);
        }
    }
    Some(relaid)
}

/// What a pass's emitter ends with that a record keeps.
pub(super) struct EmitterEnd {
    frame_ranges: Vec<Option<(usize, usize)>>,
    frame_max_baseline_64: Vec<i32>,
    placements: Vec<Vec<LinePlace>>,
    keep_specs: Vec<KeepSpec>,
}

impl EmitterEnd {
    /// Capture `em`'s end state, ranges relative to the story's first
    /// command on each frame's page.
    pub(super) fn of(em: &StoryEmitter, pre: &[PreSnapshot]) -> Self {
        Self {
            frame_ranges: em
                .frame_cmd_ranges
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let base = pre[em.chain_pages[i]].1;
                    r.map(|(s, e)| (s - base, e - base))
                })
                .collect(),
            frame_max_baseline_64: em.frame_max_baseline_64.clone(),
            placements: em.placements.clone(),
            keep_specs: em.keep_specs.clone(),
        }
    }
}

/// Keep a story's final emission for the next edit.
pub(super) fn record(
    end: EmitterEnd,
    chain_sig: u64,
    generation: u64,
    per_page: Vec<(usize, BodyStoryPageDelta)>,
    marks: Vec<ParaMark>,
    forced: HashMap<u32, u32>,
    diagnostics: Vec<Diagnostic>,
) -> StoryResume {
    StoryResume {
        chain_sig,
        generation,
        per_page,
        marks,
        frame_ranges: end.frame_ranges,
        frame_max_baseline_64: end.frame_max_baseline_64,
        placements: end.placements,
        keep_specs: end.keep_specs,
        forced,
        diagnostics,
    }
}
