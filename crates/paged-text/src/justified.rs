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

//! Line breaking for JUSTIFIED text the way InDesign's Paragraph Composer
//! sets it (ADR 144). Knuth–Plass demerits over every legal break, with two
//! departures InDesign 20.0.1 was measured to make
//! (`tools/indesign-export/justified-lines-probe.*`, 160 paragraphs):
//!
//! - a line may shrink its word spaces past `MinimumWordSpacing`, at a
//!   steep price instead of being infeasible ("Considerable experimentation"
//!   in 140 pt sets its one space at 41 % rather than stretch "Considerable"
//!   across the measure);
//! - a line may stretch as far as it must: no tolerance makes a loose line
//!   infeasible, its badness prices it.
//!
//! The weights below were fitted on that probe: 98 of its 160 paragraphs set
//! every line as InDesign does, against 73 for the Knuth–Plass breaker with
//! TeX's tolerance, and the rest differ by a word or a hyphenation point.

use paragraph_breaker::{Breakpoint, Item, INFINITE_PENALTY};

/// `(line penalty + badness)²` per line, as in TeX.
const LINE_PENALTY: f64 = 20.0;
/// Two hyphenated lines in a row.
const FLAGGED_DEMERITS: f64 = 30_000.0;
/// Adjacent lines more than one fitness class apart.
const FITNESS_DEMERITS: f64 = 10_000.0;
/// Shrinking past `MinimumWordSpacing`: each unit of `|r|³` beyond 1 costs
/// this many times the badness of a fully shrunk line.
const OVERSHRINK: f64 = 100.0;
/// No line shrinks its spaces further than this ratio of their shrink.
const MAX_SHRINK: f64 = 3.0;

fn fitness_class(r: f64) -> usize {
    if r < -0.5 {
        0
    } else if r <= 0.5 {
        1
    } else if r <= 1.0 {
        2
    } else {
        3
    }
}

/// Break `items` (justified text) by least total demerits.
pub(crate) fn justified_breaks<T>(items: &[Item<T>], lengths: &[i32]) -> Vec<Breakpoint> {
    if items.is_empty() || lengths.is_empty() {
        return Vec::new();
    }
    let n = items.len();
    let (mut width, mut stretch, mut shrink) =
        (vec![0i64; n + 1], vec![0i64; n + 1], vec![0i64; n + 1]);
    for (i, it) in items.iter().enumerate() {
        let (w, y, z) = match it {
            Item::Box { width, .. } => (*width as i64, 0, 0),
            Item::Glue {
                width,
                stretch,
                shrink,
            } => (*width as i64, *stretch as i64, *shrink as i64),
            Item::Penalty { .. } => (0, 0, 0),
        };
        width[i + 1] = width[i] + w;
        stretch[i + 1] = stretch[i] + y;
        shrink[i + 1] = shrink[i] + z;
    }
    let forced = |i: usize| matches!(items[i], Item::Penalty { penalty, .. } if penalty <= -INFINITE_PENALTY);
    let mut box_after = vec![false; n + 1];
    for i in (0..n).rev() {
        box_after[i] = match items[i] {
            Item::Box { .. } => true,
            _ if forced(i) => false,
            _ => box_after[i + 1],
        };
    }
    let candidates: Vec<usize> = (0..n)
        .filter(|&i| match &items[i] {
            Item::Glue { .. } => {
                i > 0 && matches!(items[i - 1], Item::Box { .. }) && box_after[i + 1]
            }
            Item::Penalty { penalty, .. } => *penalty < INFINITE_PENALTY,
            Item::Box { .. } => false,
        })
        .collect();
    let Some(last_c) = candidates.iter().rposition(|&j| forced(j)) else {
        return Vec::new();
    };
    let start_after = |b: Option<usize>| -> usize {
        let mut s = b.map_or(0, |b| b + 1);
        while s < n && !matches!(items[s], Item::Box { .. }) && !forced(s) {
            s += 1;
        }
        s
    };
    let horizon = lengths.len();
    let measure = |l: usize| lengths[l.min(lengths.len() - 1)] as i64;
    // state: (candidate, line capped at horizon, fitness class, hyphenated)
    let idx =
        |c: usize, l: usize, f: usize, h: bool| ((c * (horizon + 1) + l) * 4 + f) * 2 + h as usize;
    type Node = (f64, usize, f64, i64); // demerits, previous state (usize::MAX = start), ratio, width
    let mut dp: Vec<Option<Node>> = vec![None; candidates.len() * (horizon + 1) * 8];
    let badness = |r: f64| -> f64 {
        if r >= 0.0 {
            100.0 * r.powi(3)
        } else if r >= -1.0 {
            100.0 * (-r).powi(3)
        } else {
            100.0 + OVERSHRINK * 100.0 * ((-r).powi(3) - 1.0)
        }
    };
    let expand =
        |dp: &mut Vec<Option<Node>>, from: Option<(usize, usize, usize, bool)>, base: f64| {
            let (first_c, line, prev_fit, prev_h, prev_state) = match from {
                None => (0, 0, 1, false, usize::MAX),
                Some((c, l, f, h)) => (c + 1, l, f, h, idx(c, l, f, h)),
            };
            let s = start_after(from.map(|(c, ..)| candidates[c]));
            let m = measure(line);
            let mut first = true;
            for (c, &j) in candidates.iter().enumerate().skip(first_c) {
                if j < s {
                    continue;
                }
                let (pen_w, pen) = match &items[j] {
                    Item::Penalty { width, penalty, .. } => (*width as i64, *penalty),
                    _ => (0, 0),
                };
                let hyph = matches!(items[j], Item::Penalty { flagged: true, .. }) && !forced(j);
                let w = width[j] - width[s] + pen_w;
                let y = (stretch[j] - stretch[s]) as f64;
                let z = (shrink[j] - shrink[s]) as f64;
                let r = if forced(j) && w <= m {
                    0.0
                } else if w < m {
                    if y > 0.0 {
                        (m - w) as f64 / y
                    } else {
                        f64::INFINITY
                    }
                } else if w > m {
                    if z > 0.0 {
                        (m - w) as f64 / z
                    } else {
                        f64::NEG_INFINITY
                    }
                } else {
                    0.0
                };
                if r < -MAX_SHRINK && !first {
                    break;
                }
                first = false;
                // A line with no space to stretch, or none to shrink, is priced
                // as the worst a line can be, so a paragraph always breaks.
                let b = if r.is_finite() {
                    badness(r)
                } else {
                    1.0e12 * (1.0 + (w - m).abs() as f64 / 64.0)
                };
                let mut d = (LINE_PENALTY + b).powi(2);
                if pen > 0 && pen < INFINITE_PENALTY {
                    d += (pen as f64).powi(2);
                }
                if hyph && prev_h {
                    d += FLAGGED_DEMERITS;
                }
                let fit = if r.is_finite() {
                    fitness_class(r)
                } else if r > 0.0 {
                    3
                } else {
                    0
                };
                if !forced(j) && (fit as i32 - prev_fit as i32).abs() > 1 {
                    d += FITNESS_DEMERITS;
                }
                let next = (line + 1).min(horizon);
                let k = idx(c, next, fit, hyph);
                let node = (base + d, prev_state, r, w);
                if dp[k].is_none_or(|old| node.0 < old.0) {
                    dp[k] = Some(node);
                }
                if forced(j) || r < -MAX_SHRINK {
                    break;
                }
            }
        };
    expand(&mut dp, None, 0.0);
    for c in 0..last_c {
        for l in 0..=horizon {
            for f in 0..4 {
                for h in [false, true] {
                    if let Some((cost, ..)) = dp[idx(c, l, f, h)] {
                        expand(&mut dp, Some((c, l, f, h)), cost);
                    }
                }
            }
        }
    }
    let mut best: Option<(usize, f64)> = None;
    for l in 0..=horizon {
        for f in 0..4 {
            for h in [false, true] {
                let k = idx(last_c, l, f, h);
                if let Some((cost, ..)) = dp[k] {
                    if best.is_none_or(|(_, b)| cost < b) {
                        best = Some((k, cost));
                    }
                }
            }
        }
    }
    let Some((mut k, _)) = best else {
        return Vec::new();
    };
    let mut out = Vec::new();
    loop {
        let (_, prev, r, w) = dp[k].expect("reached node");
        let c = k / 8 / (horizon + 1);
        out.push(Breakpoint {
            index: candidates[c],
            ratio: r.clamp(-100.0, 100.0) as f32,
            width: w as i32,
        });
        if prev == usize::MAX {
            break;
        }
        k = prev;
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words of the given widths (pt) joined by 4 pt spaces that stretch
    /// 1.32 pt and shrink 0.8 pt (word spacing 80/100/133), then the
    /// paragraph's end.
    fn words(widths: &[i32]) -> Vec<Item<()>> {
        let mut items: Vec<Item<()>> = Vec::new();
        for (i, w) in widths.iter().enumerate() {
            if i > 0 {
                items.push(Item::Glue {
                    width: 4 * 64,
                    stretch: 84,
                    shrink: 51,
                });
            }
            items.push(Item::Box {
                width: w * 64,
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
    fn an_over_shrunk_line_beats_a_word_stretched_alone() {
        // 50 + 4 + 60 = 114 pt in 112.5 pt: the one space must lose 1.5 pt,
        // past its 0.8 pt of shrink (ratio −1.9). The other choice sets the
        // first word alone across the measure. InDesign squeezes.
        let items = words(&[50, 60, 30]);
        let breaks = justified_breaks(&items, &[112 * 64 + 32]);
        assert_eq!(line_words(&items, &breaks), vec![2, 1]);
    }

    #[test]
    fn no_line_shrinks_past_its_limit() {
        // 9 pt over with 0.8 pt of shrink (ratio −11): the second word goes.
        let items = words(&[50, 60, 30]);
        let breaks = justified_breaks(&items, &[105 * 64]);
        assert_eq!(line_words(&items, &breaks), vec![1, 2]);
    }

    #[test]
    fn a_line_that_fits_is_preferred_to_squeezing() {
        // Three 30 pt words in 70 pt: two fit with room to stretch.
        let items = words(&[30, 30, 30, 30]);
        let breaks = justified_breaks(&items, &[70 * 64]);
        assert_eq!(line_words(&items, &breaks), vec![2, 2]);
    }
}
