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

//! Paragraph keep options (thoughts ADR 028), as InDesign 2025 applies them.
//!
//! Every rule reduces to ONE kind of decision: "this line of this paragraph
//! must open the next frame/column". The story is emitted, the placement of
//! every line is recorded, and [`keep_breaks`] turns violations into forced
//! breaks for the next emit pass. The re-emit loop repeats until the set
//! stops changing. Deciding from real placement (not a lookahead estimate)
//! keeps the rules exact, and the same for split sub-paragraphs.
//!
//! Measured with the `keeps` paged-gen fixture against InDesign's export
//! (2026-10-01). What the measurements settled:
//!
//! - **KeepWithNext** moves only the paragraph's LAST line to join the next
//!   paragraph (a 3-line paragraph ends 2 | 1), not the whole paragraph.
//! - **KeepAllLinesTogether** moves the whole paragraph.
//! - **KeepFirstLines = F**: fewer than F lines before the break moves the
//!   whole paragraph.
//! - **KeepLastLines = L**: fewer than L lines after the break pulls lines
//!   over until L are there (3 | 1 becomes 2 | 2).
//!
//! A break is never forced onto a line that already opens its frame (that
//! cannot help), and only where a next frame exists (the emitter checks).
//!
//! **A break holds only where it was decided.** Each break records the
//! frame its line sat in and the line that opened that frame
//! ([`ForcedBreak`]). A frame's content is fixed by where it starts, so
//! while both match, the measured violation is still there and the break
//! applies. When an earlier break has moved the text (a heading pulled to
//! the next page drags its paragraph along; growth or an edit shifted
//! everything after it), the break is STALE: the emitter does not force it
//! and the next pass drops it, so the paragraph is laid where the flow now
//! puts it and judged again. Carrying stale breaks over was the
//! `keeps-reflow` anomaly (a page holding a lone heading, or two lines, and
//! then empty space), because a break decided for a paragraph straddling
//! page 4 stayed forced after the paragraph had moved to the top of page 5.

use std::collections::{HashMap, HashSet};

/// One forced break: line `line` of a paragraph opens the frame after
/// `frame`, decided while `frame` began with line `frame_start`
/// (`(paragraph, line)`) and held back `reserve_64` (1/64 pt) for its
/// footnotes. It applies only while all three still hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ForcedBreak {
    pub line: u32,
    pub frame: usize,
    pub frame_start: Option<(u32, u32)>,
    pub reserve_64: i32,
}

/// Top-level paragraph index → its forced break.
pub(crate) type ForcedBreaks = HashMap<u32, ForcedBreak>;

/// A paragraph's resolved keep options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct KeepSpec {
    /// `KeepLinesTogether`: the within-paragraph rules apply at all.
    pub together: bool,
    /// `KeepAllLinesTogether`.
    pub all: bool,
    /// `KeepFirstLines` (InDesign default 2).
    pub first: u32,
    /// `KeepLastLines` (InDesign default 2).
    pub last: u32,
    /// `KeepWithNext`: lines of the next paragraph that must share a frame
    /// with this paragraph's last line. 0 = off.
    pub with_next: u32,
}

impl KeepSpec {
    pub(super) fn from_attrs(a: &paged_scene::ResolvedParagraphAttrs) -> Self {
        KeepSpec {
            together: a.keep_lines_together.unwrap_or(false),
            all: a.keep_all_lines_together.unwrap_or(false),
            first: a.keep_first_lines.unwrap_or(2),
            last: a.keep_last_lines.unwrap_or(2),
            with_next: a.keep_with_next.unwrap_or(0),
        }
    }

    /// True when no rule can ever force a break.
    pub(super) fn is_inert(&self) -> bool {
        !self.together && self.with_next == 0
    }
}

/// Where one laid-out line landed in a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LinePlace {
    /// Chain frame (column) index; `None` when the line was dropped
    /// (overset).
    pub frame: Option<usize>,
    /// The line is the first one placed in its frame.
    pub opens_frame: bool,
}

/// The line that must open the next frame for `spec` to hold, given that
/// `k` of the paragraph's `n` lines fit before the break.
fn within_paragraph_break(spec: &KeepSpec, n: usize, k: usize) -> Option<usize> {
    if !spec.together || n < 2 || k == 0 || k >= n {
        return None;
    }
    if spec.all || k < spec.first as usize {
        return Some(0);
    }
    let after = n - k;
    let last = spec.last.max(1) as usize;
    if after < last {
        let b = n.saturating_sub(last);
        // Pulling lines over must not leave too few before the break.
        return Some(if b < spec.first as usize { 0 } else { b });
    }
    None
}

/// The line of a keep-with-next paragraph that must move so its end joins
/// the next paragraph: the last line, or as many as its own keeps demand.
fn with_next_break(spec: &KeepSpec, n: usize) -> usize {
    if !spec.together {
        return n - 1;
    }
    if spec.all {
        return 0;
    }
    let b = n.saturating_sub(spec.last.max(1) as usize);
    if b < spec.first as usize {
        0
    } else {
        b
    }
}

/// Forced breaks for the next pass: top-level paragraph index → the line
/// (within that paragraph) that must open the next frame, with where that
/// was decided.
///
/// `prev` is the set the measured pass was emitted with and `applied` the
/// paragraphs whose break the pass actually forced. An applied break
/// carries over: it resolved a violation that returns without it. One the
/// pass did not apply (stale, see the module doc) is dropped. Returns
/// `prev` unchanged when the pass applied every break and satisfied every
/// rule (fixpoint). `reserved[f]` is the footnote space frame `f` held back
/// in the pass.
pub(super) fn keep_breaks(
    placements: &[Vec<LinePlace>],
    specs: &[KeepSpec],
    prev: &ForcedBreaks,
    applied: &HashSet<u32>,
    reserved: &[i32],
) -> ForcedBreaks {
    let mut next: ForcedBreaks = prev
        .iter()
        .filter(|(p, _)| applied.contains(p))
        .map(|(&p, &b)| (p, b))
        .collect();
    // The line that opened each frame in this pass.
    let mut starts: HashMap<usize, (u32, u32)> = HashMap::new();
    for (p, lines) in placements.iter().enumerate() {
        for (i, l) in lines.iter().enumerate() {
            if let (true, Some(f)) = (l.opens_frame, l.frame) {
                starts.entry(f).or_insert((p as u32, i as u32));
            }
        }
    }
    for (p, lines) in placements.iter().enumerate() {
        let Some(spec) = specs.get(p) else { continue };
        if spec.is_inert() || lines.is_empty() {
            continue;
        }
        let n = lines.len();
        let mut want: Option<usize> = None;

        // Within-paragraph keeps: look at the paragraph's first break.
        if let Some(k) = (1..n)
            .find(|&i| matches!((lines[i - 1].frame, lines[i].frame), (Some(a), Some(b)) if a != b))
        {
            want = within_paragraph_break(spec, n, k);
        }

        // Keep with next: the next paragraph's first lines must share the
        // frame of this paragraph's last line.
        if spec.with_next > 0 {
            if let (Some(last_frame), Some(next_lines)) =
                (lines[n - 1].frame, placements.get(p + 1))
            {
                if !next_lines.is_empty() {
                    let need = (spec.with_next as usize).min(next_lines.len());
                    let together = next_lines
                        .iter()
                        .take_while(|l| l.frame == Some(last_frame))
                        .count();
                    if together < need {
                        let b = with_next_break(spec, n);
                        want = Some(want.map_or(b, |w| w.min(b)));
                    }
                }
            }
        }

        let Some(b) = want else { continue };
        // A line that already opens its frame cannot be helped by a break.
        let (false, Some(frame)) = (lines[b].opens_frame, lines[b].frame) else {
            continue;
        };
        let fresh = ForcedBreak {
            line: b as u32,
            frame,
            frame_start: starts.get(&frame).copied(),
            reserve_64: reserved.get(frame).copied().unwrap_or(0),
        };
        next.entry(p as u32)
            .and_modify(|e| {
                if fresh.line < e.line {
                    *e = fresh;
                }
            })
            .or_insert(fresh);
    }
    next
}

/// The line (`(paragraph, line)`) that opened the latest frame in
/// `placements`: what the emitter's `frame_first_line` is after them.
pub(super) fn last_frame_opener(placements: &[Vec<LinePlace>]) -> Option<(u32, u32)> {
    placements.iter().enumerate().rev().find_map(|(p, lines)| {
        lines
            .iter()
            .rposition(|l| l.opens_frame)
            .map(|i| (p as u32, i as u32))
    })
}

/// What a paragraph's `StartParagraph` rule does to its first line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StartMove {
    /// The line stays where the flow puts it.
    Stay,
    /// The line opens chain entry (column) `.0`.
    To(usize),
    /// No entry satisfies the rule: the rest of the story is overset.
    Overset,
}

/// Where the first line of a paragraph with `rule` goes, measured against
/// InDesign 2025 with the `start-paragraph` paged-gen fixture
/// (2026-10-01):
///
/// - a paragraph that already opens the column, frame or page the rule
///   asks for stays (`NextFrame` on a paragraph that opens a frame, the
///   story's first paragraph with `NextPage`, ...);
/// - `NextOddPage` / `NextEvenPage` also need the page's parity, so they
///   move a paragraph that opens a page of the wrong parity, the story's
///   first paragraph included.
///
/// `cur` is the chain entry the flow is in, `last_placed` the entry that
/// holds the story's last placed line, `frames[i]` the id of the frame
/// entry `i` belongs to (columns of one frame share it) and `pages[i]`
/// its page index (page number = index + 1).
pub(super) fn start_target(
    rule: paged_model::StartParagraph,
    cur: usize,
    last_placed: Option<usize>,
    frames: &[Option<&str>],
    pages: &[usize],
) -> StartMove {
    use paged_model::StartParagraph as R;
    let n = pages.len();
    if cur >= n {
        return StartMove::Stay;
    }
    let same_frame = |a: usize, b: usize| match (frames[a], frames[b]) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    };
    let first = |pred: &dyn Fn(usize) -> bool| (cur + 1..n).find(|&j| pred(j));
    let moved = |j: Option<usize>| j.map_or(StartMove::Overset, StartMove::To);
    let page_used = last_placed.is_some_and(|l| pages[l] == pages[cur]);
    match rule {
        R::Anywhere => StartMove::Stay,
        R::NextColumn if last_placed == Some(cur) => moved(first(&|_| true)),
        R::NextFrame if last_placed.is_some_and(|l| same_frame(l, cur)) => {
            moved(first(&|j| !same_frame(j, cur)))
        }
        R::NextPage if page_used => moved(first(&|j| pages[j] != pages[cur])),
        R::NextOddPage | R::NextEvenPage => {
            let odd = rule == R::NextOddPage;
            let start = if page_used {
                first(&|j| pages[j] != pages[cur])
            } else {
                Some(cur)
            };
            let Some(start) = start else {
                return StartMove::Overset;
            };
            let fits = |j: usize| ((pages[j] + 1) % 2 == 1) == odd;
            match (start..n).find(|&j| fits(j) && (j == start || pages[j] != pages[j - 1])) {
                Some(j) if j == cur => StartMove::Stay,
                j => moved(j),
            }
        }
        _ => StartMove::Stay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` lines, the first `k` in frame 0 (the first opening it), the rest
    /// in frame 1 (the first of those opening it).
    fn split(n: usize, k: usize) -> Vec<LinePlace> {
        (0..n)
            .map(|i| LinePlace {
                frame: Some(if i < k { 0 } else { 1 }),
                opens_frame: i == k,
            })
            .collect()
    }

    /// `n` lines all in `frame`, none opening it (mid-frame).
    fn within(n: usize, frame: usize, opens_first: bool) -> Vec<LinePlace> {
        (0..n)
            .map(|i| LinePlace {
                frame: Some(frame),
                opens_frame: opens_first && i == 0,
            })
            .collect()
    }

    /// The breaks a pass from no forced breaks asks for, as paragraph →
    /// line.
    fn brk(placements: &[Vec<LinePlace>], specs: &[KeepSpec]) -> HashMap<u32, u32> {
        keep_breaks(
            placements,
            specs,
            &ForcedBreaks::new(),
            &HashSet::new(),
            &[],
        )
        .into_iter()
        .map(|(p, b)| (p, b.line))
        .collect()
    }

    fn keeps(all: bool, first: u32, last: u32) -> KeepSpec {
        KeepSpec {
            together: true,
            all,
            first,
            last,
            with_next: 0,
        }
    }

    // The five measured InDesign outcomes (keeps fixture, 2026-10-01).

    #[test]
    fn keep_with_next_moves_a_one_line_paragraph() {
        // P10 (one line, KeepWithNext=2) ends column one; P11 opens column two.
        let placements = vec![within(1, 0, false), within(6, 1, true)];
        let specs = vec![
            KeepSpec {
                with_next: 2,
                ..Default::default()
            },
            KeepSpec::default(),
        ];
        let b = brk(&placements, &specs);
        assert_eq!(b.get(&0), Some(&0));
    }

    #[test]
    fn keep_with_next_moves_only_the_last_line() {
        // A three-line paragraph ends column one: InDesign answers 2 | 1.
        let placements = vec![within(3, 0, false), within(6, 1, true)];
        let specs = vec![
            KeepSpec {
                with_next: 1,
                ..Default::default()
            },
            KeepSpec::default(),
        ];
        let b = brk(&placements, &specs);
        assert_eq!(b.get(&0), Some(&2));
    }

    #[test]
    fn keep_all_lines_together_moves_the_whole_paragraph() {
        let b = brk(&[split(4, 2)], &[keeps(true, 2, 2)]);
        assert_eq!(b.get(&0), Some(&0));
    }

    #[test]
    fn too_few_first_lines_move_the_whole_paragraph() {
        // 1 | 3 with KeepFirstLines=2.
        let b = brk(&[split(4, 1)], &[keeps(false, 2, 2)]);
        assert_eq!(b.get(&0), Some(&0));
    }

    #[test]
    fn too_few_last_lines_pull_lines_over() {
        // 3 | 1 with KeepLastLines=2 becomes 2 | 2.
        let b = brk(&[split(4, 3)], &[keeps(false, 2, 2)]);
        assert_eq!(b.get(&0), Some(&2));
    }

    // Guards.

    #[test]
    fn a_satisfied_split_forces_nothing() {
        // 2 | 2 satisfies first=2 / last=2.
        let b = brk(&[split(4, 2)], &[keeps(false, 2, 2)]);
        assert!(b.is_empty());
    }

    #[test]
    fn no_keeps_no_breaks() {
        let b = brk(&[split(4, 1)], &[KeepSpec::default()]);
        assert!(b.is_empty());
    }

    #[test]
    fn a_line_that_already_opens_its_frame_is_left_alone() {
        // The paragraph already starts a frame: moving it cannot help.
        let placements = vec![within(1, 0, true), within(6, 1, true)];
        let specs = vec![
            KeepSpec {
                with_next: 2,
                ..Default::default()
            },
            KeepSpec::default(),
        ];
        assert!(brk(&placements, &specs).is_empty());
    }

    fn held_heading() -> (Vec<Vec<LinePlace>>, Vec<KeepSpec>, ForcedBreaks) {
        // After the forced break the pass satisfies the rule.
        let mut prev = ForcedBreaks::new();
        prev.insert(
            0,
            ForcedBreak {
                line: 0,
                frame: 0,
                frame_start: Some((0, 0)),
                reserve_64: 0,
            },
        );
        let placements = vec![within(1, 1, true), within(6, 1, false)];
        let specs = vec![
            KeepSpec {
                with_next: 2,
                ..Default::default()
            },
            KeepSpec::default(),
        ];
        (placements, specs, prev)
    }

    #[test]
    fn a_resolved_break_carries_over_to_the_fixpoint() {
        // The pass forced the break and the rule holds: the break stays.
        let (placements, specs, prev) = held_heading();
        let applied: HashSet<u32> = [0].into_iter().collect();
        assert_eq!(keep_breaks(&placements, &specs, &prev, &applied, &[]), prev);
    }

    #[test]
    fn a_break_the_pass_did_not_apply_is_dropped() {
        // The text moved: the frame the break was decided in no longer
        // starts where it did, the emitter left the break alone, and the
        // pass satisfies every rule without it.
        let (placements, specs, prev) = held_heading();
        assert!(keep_breaks(&placements, &specs, &prev, &HashSet::new(), &[]).is_empty());
    }

    #[test]
    fn a_break_records_where_it_was_decided() {
        // Frame 1 opens with paragraph 0's line 2; paragraph 1's 3 | 1
        // split in frame 1 (lines 0-2) pulls line 2 over.
        let placements = vec![
            (0..4)
                .map(|i| LinePlace {
                    frame: Some(if i < 2 { 0 } else { 1 }),
                    opens_frame: i == 0 || i == 2,
                })
                .collect(),
            (0..4)
                .map(|i| LinePlace {
                    frame: Some(if i < 3 { 1 } else { 2 }),
                    opens_frame: i == 3,
                })
                .collect(),
        ];
        let specs = vec![keeps(false, 2, 2), keeps(false, 2, 2)];
        let b = keep_breaks(
            &placements,
            &specs,
            &ForcedBreaks::new(),
            &HashSet::new(),
            &[],
        );
        assert_eq!(
            b.get(&1),
            Some(&ForcedBreak {
                line: 2,
                frame: 1,
                frame_start: Some((0, 2)),
                reserve_64: 0,
            })
        );
    }

    #[test]
    fn keep_with_next_respects_the_paragraphs_own_last_lines() {
        // With last=2 on, moving one line would break it: move two.
        let placements = vec![within(4, 0, false), within(6, 1, true)];
        let mut spec = keeps(false, 2, 2);
        spec.with_next = 1;
        let b = brk(&placements, &[spec, KeepSpec::default()]);
        assert_eq!(b.get(&0), Some(&2));
    }

    // ---- StartParagraph (the `start-paragraph` fixture's chain) ----
    // Entries: A column 1, A column 2, B (page 1), C (page 2), D (page 3).
    const FRAMES: [Option<&str>; 5] = [Some("A"), Some("A"), Some("B"), Some("C"), Some("D")];
    const PAGES: [usize; 5] = [0, 0, 0, 1, 2];

    fn start(rule: paged_model::StartParagraph, cur: usize, last: Option<usize>) -> StartMove {
        start_target(rule, cur, last, &FRAMES, &PAGES)
    }

    #[test]
    fn start_paragraph_moves_like_indesign() {
        use paged_model::StartParagraph as R;
        // P04 after three lines in A column 1.
        assert_eq!(start(R::Anywhere, 0, Some(0)), StartMove::Stay);
        assert_eq!(start(R::NextColumn, 0, Some(0)), StartMove::To(1));
        assert_eq!(start(R::NextFrame, 0, Some(0)), StartMove::To(2));
        assert_eq!(start(R::NextPage, 0, Some(0)), StartMove::To(3));
        assert_eq!(start(R::NextOddPage, 0, Some(0)), StartMove::To(4));
        assert_eq!(start(R::NextEvenPage, 0, Some(0)), StartMove::To(3));
    }

    #[test]
    fn start_paragraph_at_the_top_of_its_unit() {
        use paged_model::StartParagraph as R;
        // The story's first paragraph: NextPage stays, parity still moves.
        assert_eq!(start(R::NextPage, 0, None), StartMove::Stay);
        assert_eq!(start(R::NextEvenPage, 0, None), StartMove::To(3));
        assert_eq!(start(R::NextOddPage, 0, None), StartMove::Stay);
        // Already opening frame B / column two: no skip.
        assert_eq!(start(R::NextFrame, 2, Some(1)), StartMove::Stay);
        assert_eq!(start(R::NextColumn, 1, Some(0)), StartMove::Stay);
    }

    #[test]
    fn start_paragraph_without_a_target_oversets() {
        use paged_model::StartParagraph as R;
        assert_eq!(start(R::NextPage, 4, Some(4)), StartMove::Overset);
        assert_eq!(start(R::NextEvenPage, 4, Some(4)), StartMove::Overset);
    }
}
