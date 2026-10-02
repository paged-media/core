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

//! InDesign's Adobe Single-line Composer (`Composer="HL Single"`): one
//! line at a time, never looking back — the way Word breaks a paragraph.
//!
//! Every rule here was measured on InDesign 20.0.1 (Inter 10 pt, English:
//! USA), by sweeping a frame's width in quarter points across a break and
//! reading the first line back from InDesign's DOM; the `composer`
//! fixture is the regression record.
//!
//! **Ragged text** fills a line with whole words at their desired spacing
//! — never compressing a space, whatever `MinimumWordSpacing` allows. When
//! the next word does not fit, it is hyphenated only if the line would
//! otherwise end at least the hyphenation zone short of the margin
//! (`HyphenationZone`, 36 pt by default), at the LAST hyphenation point
//! that still fits.
//!
//! **Justified text** may compress word spaces down to
//! `MinimumWordSpacing` (and letters down to `MinimumLetterSpacing`). Of
//! the longest line that fits at desired spacing (A) and the next one,
//! which fits only compressed (B), it takes whichever deviates less in
//! points — A on a tie. Maximum word spacing never rejects a line: an
//! unhyphenatable paragraph sets loose lines rather than overflow.
//!
//! When the next word does not fit even compressed, A stands unless one
//! of these makes InDesign hyphenate the word instead:
//! - A ends at least one em plus 7.3 pt short of the margin — measured as
//!   17.25..17.5 pt at 10 pt, 19..20 at 12 pt, 21..21.5 at 14 pt, with
//!   the same value for 4, 9, 14 and 19 word spaces on the line and every
//!   `MaximumWordSpacing` from 110 to 300;
//! - A's spaces stretch beyond desired by more than the larger of 60 % of
//!   a space and `MaximumWordSpacing − DesiredWordSpacing` (a 9-space line
//!   hyphenates at 15.25 pt of slack for any maximum up to 160 %, at
//!   17.5 pt — the em rule — from 170 %);
//! - a hyphenation point fits without compressing anything.
//!
//! It then takes the fitting hyphenation point that deviates least
//! (`documen-` stretched 0.155 of a space beat `documenta-` compressed
//! 0.162; 0.263 lost to 0.056). A line with nothing before the word
//! hyphenates whenever a point fits.
//!
//! In both, a paragraph never ends more than `HyphenateLadderLimit`
//! consecutive lines in a hyphen (3 when unset: the 80 pt long-word case
//! sets `aries arrive` loose rather than a fourth hyphen).

use paragraph_breaker::{Breakpoint, Item, INFINITE_PENALTY};

use crate::layout::FINISHING_STRETCH;

/// A paragraph's Single-line Composer settings, as the caller knows them
/// from the paragraph's attributes. [`ComposeOptions::single_line`]
/// carries them; `None` there is the Paragraph Composer.
///
/// [`ComposeOptions::single_line`]: crate::ComposeOptions::single_line
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SingleLineOptions {
    /// `HyphenationZone`, 1/64 pt — ragged text only.
    pub zone: i32,
    /// `HyphenateLadderLimit`; `0` is unlimited.
    pub ladder_limit: usize,
    /// How far each character may compress, as a fraction of a word
    /// space (`(DesiredLetterSpacing − MinimumLetterSpacing) / 100`) —
    /// justified text only.
    pub letter_shrink_ratio: f32,
    /// `(MaximumWordSpacing − DesiredWordSpacing) / 100`: with 0.6, the
    /// floor below it, the stretch per space past which a justified line
    /// is too loose to keep when its next word could be hyphenated.
    pub max_stretch_ratio: f32,
}

impl Default for SingleLineOptions {
    /// InDesign's defaults: a 36 pt zone, three hyphens in a row,
    /// letter spacing fixed, 133 % maximum word spacing.
    fn default() -> Self {
        Self {
            zone: (36.0 * crate::shape::ADVANCE_PRECISION) as i32,
            ladder_limit: 3,
            letter_shrink_ratio: 0.0,
            max_stretch_ratio: 0.33,
        }
    }
}

/// [`SingleLineOptions`] resolved against the paragraph being set, in
/// 1/64 pt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SingleLineParams {
    /// Justified (word spaces compress and stretch) or ragged.
    pub justified: bool,
    pub zone: i32,
    pub ladder_limit: usize,
    /// How far each character may compress.
    pub letter_shrink: i32,
    /// The slack from which a justified line hyphenates the next word.
    pub hyphen_slack: i32,
    /// Stretch per space past which a justified line is too loose.
    pub loose_per_space: i32,
}

impl SingleLineParams {
    /// Resolve `o` for a paragraph whose first run is `point_size` and
    /// whose natural word space is `natural_space` (1/64 pt).
    pub(crate) fn new(
        o: &SingleLineOptions,
        justified: bool,
        point_size: f32,
        natural_space: i32,
    ) -> Self {
        let space = natural_space.max(0) as f32;
        Self {
            justified,
            zone: o.zone.max(0),
            ladder_limit: o.ladder_limit,
            letter_shrink: (space * o.letter_shrink_ratio.max(0.0)).round() as i32,
            // One em plus 7.3 pt.
            hyphen_slack: ((point_size + 7.3) * crate::shape::ADVANCE_PRECISION).round() as i32,
            loose_per_space: (space * o.max_stretch_ratio.max(0.6)).round() as i32,
        }
    }
}

/// One place the current line could end.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    /// Item the line breaks at.
    index: usize,
    /// The line's natural width at desired spacing, a hyphen included.
    width: i32,
    /// How far it may compress.
    shrink: i32,
    /// Word spaces on the line.
    spaces: i32,
    /// Ends in a hyphen.
    hyphen: bool,
    /// A forced break (paragraph end, forced line break).
    forced: bool,
}

/// Break `items` one line at a time. `chars` holds, per item, how many
/// characters the paragraph has up to and including it (for letter
/// spacing); it may be empty when `letter_shrink` is zero.
pub(crate) fn single_line_breaks<T>(
    items: &[Item<T>],
    lengths: &[i32],
    chars: &[u32],
    opts: &SingleLineParams,
) -> Vec<Breakpoint> {
    let mut breaks = Vec::new();
    if items.is_empty() {
        return breaks;
    }
    let measure = |line: usize| -> i32 {
        lengths
            .get(line.min(lengths.len().saturating_sub(1)))
            .copied()
            .unwrap_or(i32::MAX)
            .max(1)
    };
    let letters = |from: usize, to: usize| -> i32 {
        if opts.letter_shrink == 0 || chars.is_empty() {
            return 0;
        }
        let a = from.checked_sub(1).and_then(|i| chars.get(i)).copied();
        let b = chars.get(to).copied().unwrap_or(0);
        // Letter spacing sits between characters: one gap fewer than
        // the line has characters.
        let mut n = b.saturating_sub(a.unwrap_or(0)) as i32;
        // After a break at a word space, that space is counted but not
        // set.
        if from > 0 && matches!(items.get(from - 1), Some(Item::Glue { .. })) {
            n -= 1;
        }
        (n - 1).max(0) * opts.letter_shrink
    };

    let mut start = 0usize;
    let mut line = 0usize;
    let mut hyphens_in_a_row = 0usize;
    while start < items.len() {
        // A line starts at its first box: the glue and penalties a break
        // leaves behind are discarded.
        while start < items.len() && !matches!(items[start], Item::Box { .. }) {
            if let Item::Penalty { penalty, .. } = items[start] {
                if penalty <= -INFINITE_PENALTY {
                    break;
                }
            }
            start += 1;
        }
        if start >= items.len() {
            break;
        }
        let m = measure(line);
        let fits_natural = |c: &Candidate| c.width <= m;
        let fits = |c: &Candidate| c.width - c.shrink <= m;

        let mut width = 0i32;
        let mut shrink = 0i32;
        let mut spaces = 0i32;
        let mut first_gap: Option<Candidate> = None;
        // A: longest line at desired spacing; B: the next, compressed.
        let mut a: Option<Candidate> = None;
        let mut b: Option<Candidate> = None;
        // Hyphenation points inside the word after A.
        let mut points: Vec<Candidate> = Vec::new();
        let mut i = start;
        while i < items.len() {
            match &items[i] {
                Item::Box { width: w, .. } => width += w,
                Item::Glue {
                    width: w,
                    stretch,
                    shrink: sh,
                } => {
                    if *stretch == FINISHING_STRETCH {
                        i += 1;
                        continue;
                    }
                    let c = Candidate {
                        index: i,
                        width,
                        shrink: if opts.justified {
                            shrink + letters(start, i.saturating_sub(1))
                        } else {
                            0
                        },
                        spaces,
                        hyphen: false,
                        forced: false,
                    };
                    first_gap.get_or_insert(c);
                    if fits_natural(&c) {
                        a = Some(c);
                        points.clear();
                    } else {
                        if opts.justified && fits(&c) {
                            b = Some(c);
                        }
                        break;
                    }
                    width += w;
                    shrink += sh;
                    spaces += 1;
                }
                Item::Penalty {
                    width: pw,
                    penalty,
                    flagged,
                } => {
                    if *penalty <= -INFINITE_PENALTY || (*penalty < INFINITE_PENALTY && !*flagged) {
                        let forced = *penalty <= -INFINITE_PENALTY;
                        let c = Candidate {
                            index: i,
                            width,
                            shrink: if opts.justified {
                                shrink + letters(start, i)
                            } else {
                                0
                            },
                            spaces,
                            hyphen: false,
                            forced,
                        };
                        first_gap.get_or_insert(c);
                        if fits_natural(&c) {
                            a = Some(c);
                            points.clear();
                            if forced {
                                break;
                            }
                        } else {
                            if opts.justified && fits(&c) {
                                b = Some(c);
                            }
                            break;
                        }
                    } else if *penalty < INFINITE_PENALTY && width > 0 {
                        points.push(Candidate {
                            index: i,
                            width: width + pw,
                            shrink: if opts.justified {
                                shrink + letters(start, i)
                            } else {
                                0
                            },
                            spaces,
                            hyphen: true,
                            forced: false,
                        });
                    }
                }
            }
            i += 1;
        }

        let chosen = choose(a, b, &points, m, opts, hyphens_in_a_row)
            .or(first_gap)
            .unwrap_or(Candidate {
                index: items.len() - 1,
                width,
                shrink: 0,
                spaces,
                hyphen: false,
                forced: true,
            });
        let ratio = if chosen.width <= m || chosen.shrink == 0 {
            0.0
        } else {
            (m - chosen.width) as f32 / chosen.shrink as f32
        };
        breaks.push(Breakpoint {
            index: chosen.index,
            ratio,
            width: chosen.width,
        });
        hyphens_in_a_row = if chosen.hyphen {
            hyphens_in_a_row + 1
        } else {
            0
        };
        line += 1;
        start = chosen.index + 1;
        if chosen.forced && chosen.index + 1 >= items.len() {
            break;
        }
    }
    breaks
}

/// Pick the line's break from A (longest at desired spacing), B (the next
/// break, compressed) and the hyphenation points of the word after A.
fn choose(
    a: Option<Candidate>,
    b: Option<Candidate>,
    points: &[Candidate],
    m: i32,
    opts: &SingleLineParams,
    hyphens_in_a_row: usize,
) -> Option<Candidate> {
    if let Some(b) = b {
        return Some(match a {
            Some(a) if m - a.width <= b.width - m => a,
            _ => b,
        });
    }
    if let Some(a) = a.filter(|a| a.forced) {
        return Some(a);
    }
    // The ladder limit yields where keeping it would leave nothing that
    // fits: InDesign sets a compound longer than three lines as
    // `Donaudampfschiff-` / `fahrtsgesell-` / `schaftskapitaens-` under a
    // limit of 1 rather than run the word out of the frame (measured,
    // `soft-hyphens` fixture, 2026-10-02).
    let ladder_ok = a.is_none() || opts.ladder_limit == 0 || hyphens_in_a_row < opts.ladder_limit;
    let fitting: Vec<&Candidate> = points.iter().filter(|p| p.width - p.shrink <= m).collect();
    if ladder_ok && !fitting.is_empty() {
        let trigger = match a {
            None => true,
            Some(a) => {
                let slack = m - a.width;
                if opts.justified {
                    slack >= opts.hyphen_slack
                        || a.spaces == 0
                        || slack > opts.loose_per_space * a.spaces
                        || fitting.iter().any(|p| p.width <= m)
                } else {
                    slack >= opts.zone
                }
            }
        };
        if trigger {
            let pick = if opts.justified {
                fitting
                    .iter()
                    .min_by_key(|p| (m - p.width).abs())
                    .copied()
                    .copied()
            } else {
                fitting.last().copied().copied()
            };
            if pick.is_some() {
                return pick;
            }
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    const SP: i32 = 10;

    /// Words of the given widths, `SP`-wide glue between them (shrink
    /// 2, stretch 3), each word optionally split by hyphenation points
    /// (`|` in the width list's companion) — plus the finishing pair.
    fn para(words: &[&[i32]], shrink: i32) -> Vec<Item<()>> {
        let mut items = Vec::new();
        for (i, parts) in words.iter().enumerate() {
            if i > 0 {
                items.push(Item::Glue {
                    width: SP,
                    stretch: 3,
                    shrink,
                });
            }
            for (k, w) in parts.iter().enumerate() {
                if k > 0 {
                    items.push(Item::Penalty {
                        width: 4,
                        penalty: 50,
                        flagged: true,
                    });
                }
                items.push(Item::Box {
                    width: *w,
                    data: (),
                });
            }
        }
        items.push(Item::Glue {
            width: 0,
            stretch: FINISHING_STRETCH,
            shrink: 0,
        });
        items.push(Item::Penalty {
            width: 0,
            penalty: -INFINITE_PENALTY,
            flagged: true,
        });
        items
    }

    fn opts(justified: bool) -> SingleLineParams {
        SingleLineParams {
            justified,
            zone: 100,
            ladder_limit: 3,
            letter_shrink: 0,
            hyphen_slack: 60,
            loose_per_space: 1000,
        }
    }

    #[test]
    fn ragged_text_fills_greedily_and_never_compresses() {
        // 40 + 10 + 40 = 90 fits 95; a third word would need 140 —
        // even compressed (shrink 50) a ragged line takes it whole only
        // at its natural width.
        let items = para(&[&[40], &[40], &[40]], 50);
        let b = single_line_breaks(&items, &[95], &[], &opts(false));
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].index, 3, "breaks at the glue before word 3");
    }

    #[test]
    fn justified_text_takes_the_compressed_line_when_it_deviates_less() {
        // A = 40 + 10 + 40 = 90 (8 short of 98); B = 140 − 4·… : with a
        // shrink of 50 per gap B = 140 fits 98 compressed by 42 > 8 → A.
        let items = para(&[&[40], &[40], &[40]], 50);
        let b = single_line_breaks(&items, &[98], &[], &opts(true));
        assert_eq!(b[0].index, 3);
        // At 136, A is 46 short and B 4 over: B.
        let b = single_line_breaks(&items, &[136], &[], &opts(true));
        assert_eq!(b.len(), 1, "all three words on one compressed line");
        assert!(b[0].ratio < 0.0);
    }

    #[test]
    fn a_short_line_is_not_hyphenated_inside_the_threshold() {
        // A = 20 + 10 + 20 = 50. Word 3 = 60|30: "… 60-4" = 124 fits 118
        // only compressed (shrink 3 a gap). A's slack is 68: under a 70
        // threshold A stands, over a 60 one the word is hyphenated.
        let items = para(&[&[20], &[20], &[60, 30]], 3);
        let mut o = opts(true);
        o.hyphen_slack = 70;
        let b = single_line_breaks(&items, &[118], &[], &o);
        assert!(!items_is_hyphen(&items, b[0].index));
        o.hyphen_slack = 60;
        let b = single_line_breaks(&items, &[118], &[], &o);
        assert!(items_is_hyphen(&items, b[0].index));
        // A single word cannot be justified at all: hyphenate the next.
        let items = para(&[&[50], &[30, 30]], 5);
        let b = single_line_breaks(&items, &[92], &[], &opts(true));
        assert!(items_is_hyphen(&items, b[0].index));
    }

    #[test]
    fn a_hyphenation_point_that_fits_uncompressed_is_taken() {
        // Slack 44 at 94, under the threshold, but the point fits at
        // desired spacing.
        let items = para(&[&[20], &[20], &[30, 30]], 5);
        let b = single_line_breaks(&items, &[94], &[], &opts(true));
        assert!(items_is_hyphen(&items, b[0].index));
    }

    #[test]
    fn ragged_text_hyphenates_only_beyond_the_zone_at_the_last_fitting_point() {
        let items = para(&[&[50], &[20, 20, 20]], 0);
        let mut o = opts(false);
        o.zone = 40;
        // Slack of A at 100 is 50 ≥ 40: hyphenate at the last point that
        // fits: 50 + 10 + 20 + 20 + 4 = 104 > 100, so after the first.
        let b = single_line_breaks(&items, &[100], &[], &o);
        assert!(items_is_hyphen(&items, b[0].index));
        assert_eq!(b[0].width, 84);
        o.zone = 60;
        let b = single_line_breaks(&items, &[100], &[], &o);
        assert!(!items_is_hyphen(&items, b[0].index));
    }

    #[test]
    fn the_ladder_limit_stops_a_run_of_hyphens() {
        // Line 1 ends "10 20-"; line 2 could end "20 20-" too (54 fits
        // 54), but a ladder limit of 1 sends it out whole.
        let items = para(&[&[10], &[20, 20], &[20, 20], &[20, 20]], 0);
        let mut o = opts(false);
        o.zone = 0;
        let b = single_line_breaks(&items, &[54], &[], &o);
        assert!(items_is_hyphen(&items, b[0].index));
        assert!(
            items_is_hyphen(&items, b[1].index),
            "no limit: two in a row"
        );
        o.ladder_limit = 1;
        let b = single_line_breaks(&items, &[54], &[], &o);
        assert!(items_is_hyphen(&items, b[0].index));
        assert!(!items_is_hyphen(&items, b[1].index));
    }

    #[test]
    fn the_ladder_limit_yields_when_nothing_else_fits() {
        // One word four segments long in a measure that holds one: every
        // line can only end in a hyphen, and InDesign takes them all
        // rather than run the word out of the frame (`soft-hyphens`).
        let items = para(&[&[30, 30, 30, 30]], 0);
        let mut o = opts(false);
        o.zone = 0;
        o.ladder_limit = 1;
        let b = single_line_breaks(&items, &[40], &[], &o);
        assert_eq!(b.len(), 4);
        assert!(b[..3].iter().all(|p| items_is_hyphen(&items, p.index)));
    }

    fn items_is_hyphen(items: &[Item<()>], i: usize) -> bool {
        matches!(items[i], Item::Penalty { flagged: true, penalty, .. } if penalty > -INFINITE_PENALTY)
    }
}
