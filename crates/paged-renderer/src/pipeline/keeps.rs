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

use std::collections::HashMap;

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

/// Forced breaks for the next pass: top-level paragraph index → index of the
/// line (within that paragraph) that must open the next frame.
///
/// `prev` is the set the measured pass was emitted with. It carries over:
/// a break that resolved a violation must stay, or the violation returns.
/// Returns `prev` unchanged when the pass satisfied every rule (fixpoint).
pub(super) fn keep_breaks(
    placements: &[Vec<LinePlace>],
    specs: &[KeepSpec],
    prev: &HashMap<u32, u32>,
) -> HashMap<u32, u32> {
    let mut next = prev.clone();
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
        if lines[b].opens_frame || lines[b].frame.is_none() {
            continue;
        }
        let entry = next.entry(p as u32).or_insert(b as u32);
        *entry = (*entry).min(b as u32);
    }
    next
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
        let b = keep_breaks(&placements, &specs, &HashMap::new());
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
        let b = keep_breaks(&placements, &specs, &HashMap::new());
        assert_eq!(b.get(&0), Some(&2));
    }

    #[test]
    fn keep_all_lines_together_moves_the_whole_paragraph() {
        let b = keep_breaks(&[split(4, 2)], &[keeps(true, 2, 2)], &HashMap::new());
        assert_eq!(b.get(&0), Some(&0));
    }

    #[test]
    fn too_few_first_lines_move_the_whole_paragraph() {
        // 1 | 3 with KeepFirstLines=2.
        let b = keep_breaks(&[split(4, 1)], &[keeps(false, 2, 2)], &HashMap::new());
        assert_eq!(b.get(&0), Some(&0));
    }

    #[test]
    fn too_few_last_lines_pull_lines_over() {
        // 3 | 1 with KeepLastLines=2 becomes 2 | 2.
        let b = keep_breaks(&[split(4, 3)], &[keeps(false, 2, 2)], &HashMap::new());
        assert_eq!(b.get(&0), Some(&2));
    }

    // Guards.

    #[test]
    fn a_satisfied_split_forces_nothing() {
        // 2 | 2 satisfies first=2 / last=2.
        let b = keep_breaks(&[split(4, 2)], &[keeps(false, 2, 2)], &HashMap::new());
        assert!(b.is_empty());
    }

    #[test]
    fn no_keeps_no_breaks() {
        let b = keep_breaks(&[split(4, 1)], &[KeepSpec::default()], &HashMap::new());
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
        assert!(keep_breaks(&placements, &specs, &HashMap::new()).is_empty());
    }

    #[test]
    fn a_resolved_break_carries_over_to_the_fixpoint() {
        // After the forced break the pass satisfies the rule; the break stays.
        let mut prev = HashMap::new();
        prev.insert(0u32, 0u32);
        let placements = vec![within(1, 1, true), within(6, 1, false)];
        let specs = vec![
            KeepSpec {
                with_next: 2,
                ..Default::default()
            },
            KeepSpec::default(),
        ];
        assert_eq!(keep_breaks(&placements, &specs, &prev), prev);
    }

    #[test]
    fn keep_with_next_respects_the_paragraphs_own_last_lines() {
        // With last=2 on, moving one line would break it: move two.
        let placements = vec![within(4, 0, false), within(6, 1, true)];
        let mut spec = keeps(false, 2, 2);
        spec.with_next = 1;
        let b = keep_breaks(&placements, &[spec, KeepSpec::default()], &HashMap::new());
        assert_eq!(b.get(&0), Some(&2));
    }
}
