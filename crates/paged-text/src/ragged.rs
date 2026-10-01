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

//! Minimum-raggedness line breaking — how InDesign's composer breaks
//! ragged (left / right / centred) text whose lines have DIFFERENT
//! measures, as a shaped frame gives them.
//!
//! Knuth–Plass prices a ragged line by how far its word spaces would
//! have to stretch, so a line with few spaces is dear whatever its slack:
//! in a triangle it fills the narrow top lines greedily and then sets
//! short lines further down. InDesign does the opposite. Measured on
//! InDesign 20.0.1 (`shaped-bands`: circles, triangles, a donut, a
//! narrow ellipse; Inter 10/12, hyphenation off), it chose
//! `delta | echo foxtrot | golf hotel india | juliet kilo lima mike` in a
//! triangle where greedy filling and the engine's breaker both set
//! `delta echo | foxtrot golf | hotel india juliet | kilo lima mike` — a
//! nearly empty line traded for four full ones. Minimising the sum of
//! the squared space every line leaves at the end of its band reproduces
//! every break of the fixture's seventeen ragged frames (and of
//! `text-in-shape`'s oval and donut); minimising it relative to the
//! band's width, or the cube of the slack per stretchable space as the
//! breaker does, does not.

use paragraph_breaker::{Breakpoint, Item, INFINITE_PENALTY};

/// A line that cannot be set inside its measure (a word wider than the
/// band) is taken only when nothing fits, at this cost per point over.
const OVERFULL_COST: f64 = 1.0e9;

/// What a paragraph's lines are, beyond their measures.
pub(crate) struct RaggedLines<'a> {
    /// Lines from this index on cost nothing (overset, past the frame).
    pub free_after: usize,
    /// `joined[i]`: line `i` is the far part of the row line `i − 1`
    /// starts — a hole splits the row.
    pub joined: &'a [bool],
    /// The paragraph's hyphenation penalty.
    pub hyphen_penalty: i32,
    /// How far a word space may shrink, as a fraction of its width
    /// (`1 − MinimumWordSpacing`).
    pub space_shrink: f32,
}

/// What squeezing a line costs per pt² it is over its measure, against
/// one pt² per point of slack. InDesign squeezes a ragged line's word
/// spaces (down to `MinimumWordSpacing`) to keep a word, but only where
/// it must: `text-in-shape`'s donut sets `day long the quick` — 93.18 pt
/// at natural spacing — in a 93 pt band with its spaces 2.3 % narrow,
/// and `shaped-bands` never squeezes at all. Both come out as InDesign's
/// from 10 000 (5 000 squeezes `shaped-bands`' stroked donut, 30 000
/// refuses `text-in-shape`'s).
const SQUEEZE_WEIGHT: f64 = 10_000.0;

/// What a hyphenated break costs per point of hyphenation penalty, in
/// pt² of slack: the default penalty of 50 weighs like twenty-two points
/// of slack on one line. Fitted on `text-in-shape` (InDesign hyphenates
/// its oval's `win-dow` twice and its donut never): anything from 4 to 24
/// sets both as InDesign does, 2 hyphenates the donut, 30 loses the
/// oval's hyphens.
const HYPHEN_WEIGHT: f64 = 10.0;

/// Break `items` so the sum over lines of `slack²` (in pt) is smallest.
///
/// Every composer line is priced on its own, the parts of a row a hole
/// splits included: `shaped-bands`' donuts measured that against pricing
/// the row's total slack, and only the per-part sum reproduces both. The
/// last line of the paragraph, a line ended by a forced break, and every
/// line from `free_after` on cost nothing. Where that starts was measured
/// too: charging every overset line moves `shaped-bands`' stroked donut
/// off InDesign's breaks, charging none moves `text-in-shape`'s oval off
/// its last three — charging exactly the first line past the frame (the
/// one InDesign sets to find it does not fit) holds both. A row is ONE
/// line to InDesign,
/// so no word is hyphenated across the hole that splits it.
pub(crate) fn min_ragged_breaks<T>(
    items: &[Item<T>],
    lengths: &[i32],
    lines: &RaggedLines<'_>,
) -> Vec<Breakpoint> {
    let free_after = lines.free_after;
    let joined = |l: usize| lines.joined.get(l).copied().unwrap_or(false);
    if items.is_empty() || lengths.is_empty() {
        return Vec::new();
    }
    let n = items.len();
    // prefix[i] = width of items[..i] (boxes and glue; penalties only
    // count when a line breaks at them).
    let mut prefix = vec![0i64; n + 1];
    // glue[i] = width of the glue in items[..i] — what may shrink.
    let mut glue = vec![0i64; n + 1];
    for (i, it) in items.iter().enumerate() {
        prefix[i + 1] = prefix[i]
            + match it {
                Item::Box { width, .. } | Item::Glue { width, .. } => *width as i64,
                Item::Penalty { .. } => 0,
            };
        glue[i + 1] = glue[i]
            + match it {
                Item::Glue { width, .. } => *width as i64,
                _ => 0,
            };
    }
    let shrink = lines.space_shrink.clamp(0.0, 1.0) as f64;
    let forced = |i: usize| matches!(items[i], Item::Penalty { penalty, .. } if penalty <= -INFINITE_PENALTY);
    // Legal breaks, in item order: a glue after a box, any penalty
    // short of +infinity.
    let candidates: Vec<usize> = (0..n)
        .filter(|&i| match &items[i] {
            Item::Glue { .. } => i > 0 && matches!(items[i - 1], Item::Box { .. }),
            Item::Penalty { penalty, .. } => *penalty < INFINITE_PENALTY,
            Item::Box { .. } => false,
        })
        .collect();
    let Some(last_c) = candidates.iter().rposition(|&j| forced(j)) else {
        return Vec::new();
    };
    // Where a line after a break at item `b` starts: the first box past
    // it (glue and penalties at a line's start are discarded).
    let start_after = |b: Option<usize>| -> usize {
        let mut s = b.map_or(0, |b| b + 1);
        while s < n && !matches!(items[s], Item::Box { .. }) && !forced(s) {
            s += 1;
        }
        s
    };
    // A line index only matters while it can change the measure or the
    // price: past both ends every line is alike.
    let horizon = if free_after == usize::MAX {
        lengths.len()
    } else {
        lengths.len().max(free_after)
    };
    let measure = |l: usize| lengths[l.min(lengths.len() - 1)] as i64;

    // dp[c][l]: the cheapest way to end a line at candidate `c` with
    // line `l` (capped at `horizon`) next: (cost, previous node, width).
    type Node = (f64, Option<(usize, usize)>, i64);
    let mut dp: Vec<Vec<Option<Node>>> = vec![vec![None; horizon + 1]; candidates.len()];
    let expand =
        |dp: &mut Vec<Vec<Option<Node>>>, from: Option<(usize, usize)>, base: f64, line: usize| {
            let s = start_after(from.map(|(c, _)| candidates[c]));
            let m = measure(line);
            let first_c = from.map_or(0, |(c, _)| c + 1);
            let mut first = true;
            for (c, &j) in candidates.iter().enumerate().skip(first_c) {
                if j < s {
                    continue;
                }
                let pen_w = match &items[j] {
                    Item::Penalty { width, .. } => *width as i64,
                    _ => 0,
                };
                let hyphen = matches!(items[j], Item::Penalty { flagged: true, .. }) && !forced(j);
                if hyphen && joined(line + 1) {
                    continue;
                }
                let w = prefix[j] - prefix[s] + pen_w;
                // The narrowest the line can be set: its spaces squeezed.
                let tightest = w - ((glue[j] - glue[s]) as f64 * shrink).floor() as i64;
                if tightest > m && !first {
                    break;
                }
                first = false;
                let cost = if tightest > m {
                    OVERFULL_COST * (1.0 + (tightest - m) as f64 / 64.0)
                } else if forced(j) && w <= m || line >= free_after {
                    0.0
                } else if w > m {
                    let over = (w - m) as f64 / 64.0;
                    SQUEEZE_WEIGHT * over * over
                } else {
                    let slack = (m - w) as f64 / 64.0;
                    let hyphen_cost = if hyphen {
                        lines.hyphen_penalty.max(0) as f64 * HYPHEN_WEIGHT
                    } else {
                        0.0
                    };
                    slack * slack + hyphen_cost
                };
                let next = (line + 1).min(horizon);
                let node = (base + cost, from, w);
                let slot = &mut dp[c][next];
                if slot.map_or(true, |old| node.0 < old.0) {
                    *slot = Some(node);
                }
                if forced(j) || tightest > m {
                    break;
                }
            }
        };
    expand(&mut dp, None, 0.0, 0);
    for c in 0..last_c {
        for l in 0..=horizon {
            if let Some((cost, _, _)) = dp[c][l] {
                expand(&mut dp, Some((c, l)), cost, l);
            }
        }
    }
    let Some(best_l) = (0..=horizon)
        .filter_map(|l| dp[last_c][l].map(|n| (l, n.0)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(l, _)| l)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut at = Some((last_c, best_l));
    while let Some((c, l)) = at {
        let (_, prev, w) = dp[c][l].expect("reached node");
        out.push(Breakpoint {
            index: candidates[c],
            ratio: 0.0,
            width: w as i32,
        });
        at = prev;
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(widths: &[i32]) -> Vec<Item<()>> {
        let mut items: Vec<Item<()>> = Vec::new();
        for (i, w) in widths.iter().enumerate() {
            if i > 0 {
                items.push(Item::Glue {
                    width: 64,
                    stretch: 0,
                    shrink: 0,
                });
            }
            items.push(Item::Box {
                width: *w,
                data: (),
            });
        }
        items.push(Item::Glue {
            width: 0,
            stretch: i32::MAX / 4,
            shrink: 0,
        });
        items.push(Item::Penalty {
            width: 0,
            penalty: -INFINITE_PENALTY,
            flagged: true,
        });
        items
    }

    fn lines(free_after: usize) -> RaggedLines<'static> {
        RaggedLines {
            free_after,
            joined: &[],
            hyphen_penalty: 50,
            space_shrink: 0.0,
        }
    }

    fn line_words(items: &[Item<()>], breaks: &[Breakpoint]) -> Vec<usize> {
        let mut out = Vec::new();
        let mut from = 0;
        for b in breaks {
            out.push(
                items[from..b.index]
                    .iter()
                    .filter(|i| matches!(i, Item::Box { .. }))
                    .count(),
            );
            from = b.index;
        }
        out
    }

    #[test]
    fn a_short_line_buys_fuller_ones() {
        // Measure 6 pt; words 3, 2, 2, 5 pt with 1 pt spaces. Greedy:
        // [3 2] (slack 0) | [2] (4) | [5] -> 16. Better: [3] (3) |
        // [2 2] (1) | [5] -> 10.
        let items = words(&[3 * 64, 2 * 64, 2 * 64, 5 * 64]);
        let breaks = min_ragged_breaks(&items, &[6 * 64], &lines(usize::MAX));
        assert_eq!(line_words(&items, &breaks), vec![1, 2, 1]);
    }

    #[test]
    fn a_word_wider_than_its_band_still_gets_a_line() {
        let items = words(&[20 * 64, 3 * 64]);
        let breaks = min_ragged_breaks(&items, &[640], &lines(usize::MAX));
        assert_eq!(line_words(&items, &breaks), vec![1, 1]);
    }

    #[test]
    fn the_last_line_costs_nothing() {
        // One measure of 10: [4 4] then [4] — the short last line is free.
        let items = words(&[4 * 64, 4 * 64, 4 * 64]);
        let breaks = min_ragged_breaks(&items, &[640], &lines(usize::MAX));
        assert_eq!(line_words(&items, &breaks), vec![2, 1]);
    }

    #[test]
    fn overset_lines_cost_nothing() {
        // Measure 6 pt, but only the first line is in the frame: fill it.
        let items = words(&[3 * 64, 2 * 64, 2 * 64, 5 * 64]);
        let breaks = min_ragged_breaks(&items, &[6 * 64], &lines(1));
        assert_eq!(line_words(&items, &breaks), vec![2, 1, 1]);
    }
}
