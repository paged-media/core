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

//! Greedy first-fit line breaking — the last resort when Knuth–Plass
//! finds no feasible set of breaks at any tolerance.
//!
//! That happens when a single token is wider than the measure, which is
//! exactly the case an auto-sizing frame drives the composer into: the
//! fit bisects toward the narrowest column that still holds the text,
//! and the last few trials have no total-fit solution at all.
//!
//! The previous fallback (P-17) put every `Item::Box` on its own line.
//! It kept the text visible, but it also split a word at every
//! hyphenation opportunity and never let two words share a line — so a
//! `HeightAndWidth` frame fitted 44 fragment lines where InDesign fits
//! 35 ("lets / the / box / an / swer" against InDesign's "lets the /
//! box / answer"). Greedy first-fit is what InDesign's own composer
//! falls back to, and it packs what fits: a word is split only when it
//! alone exceeds the measure.

use paragraph_breaker::{Breakpoint, Item, INFINITE_PENALTY};

/// Break `items` greedily: fill each line with the longest run that
/// fits, break at the last legal opportunity, and split a word only
/// when it does not fit a line by itself.
pub(crate) fn first_fit_breaks<T>(items: &[Item<T>], lengths: &[i32]) -> Vec<Breakpoint> {
    let mut breaks: Vec<Breakpoint> = Vec::new();
    if items.is_empty() {
        return breaks;
    }
    let measure = |line: usize| -> i32 {
        let idx = line.min(lengths.len().saturating_sub(1));
        lengths.get(idx).copied().unwrap_or(i32::MAX).max(1)
    };

    // Width since the last break, the last glue we could break at, and
    // the last hyphen we could break at — plus the width each would
    // leave on the line if taken.
    let mut width = 0i32;
    let mut last_glue: Option<(usize, i32)> = None;
    let mut last_hyphen: Option<(usize, i32)> = None;
    let mut line = 0usize;
    let mut i = 0usize;
    let take = |breaks: &mut Vec<Breakpoint>, at: usize, w: i32, line: &mut usize| {
        breaks.push(Breakpoint {
            index: at,
            ratio: 0.0,
            width: w,
        });
        *line += 1;
    };

    while i < items.len() {
        match &items[i] {
            Item::Box { width: bw, .. } => {
                // Would this box overflow? Break at the best chance we
                // have passed — but never on an empty line, or a box
                // wider than the whole measure would loop forever.
                if width + bw > measure(line) && width > 0 {
                    if let Some((at, w)) = last_glue {
                        take(&mut breaks, at, w, &mut line);
                        // Resume after the glue we broke at.
                        i = at + 1;
                        width = 0;
                        last_glue = None;
                        last_hyphen = None;
                        continue;
                    }
                    if let Some((at, w)) = last_hyphen {
                        take(&mut breaks, at, w, &mut line);
                        i = at + 1;
                        width = 0;
                        last_glue = None;
                        last_hyphen = None;
                        continue;
                    }
                }
                width += bw;
            }
            Item::Glue { width: gw, .. } => {
                // A glue is a break opportunity once the line has ink.
                if width > 0 {
                    last_glue = Some((i, width));
                }
                width += gw;
            }
            Item::Penalty {
                width: pw,
                penalty,
                flagged,
            } => {
                if *penalty <= -INFINITE_PENALTY {
                    take(&mut breaks, i, width, &mut line);
                    width = 0;
                    last_glue = None;
                    last_hyphen = None;
                    i += 1;
                    continue;
                }
                if *penalty < INFINITE_PENALTY && width > 0 {
                    // Taking this break costs the hyphen's width.
                    let with_hyphen = width + pw;
                    if with_hyphen <= measure(line) {
                        if *flagged {
                            last_hyphen = Some((i, with_hyphen));
                        } else {
                            last_glue = Some((i, width));
                        }
                    }
                }
            }
        }
        i += 1;
    }
    // Whatever is left is the last line. The item list always ends with
    // the finishing glue + forced penalty, so this only fires when that
    // penalty was already consumed above.
    if width > 0 || breaks.is_empty() {
        take(&mut breaks, items.len() - 1, width, &mut line);
    }
    breaks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxes(widths: &[i32]) -> Vec<Item<()>> {
        // word, glue, word, glue, … then the finishing pair.
        let mut items: Vec<Item<()>> = Vec::new();
        for (i, w) in widths.iter().enumerate() {
            if i > 0 {
                items.push(Item::Glue {
                    width: 10,
                    stretch: 5,
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

    #[test]
    fn words_that_fit_share_a_line() {
        // 30 + 10 + 30 = 70 fits in 80; adding 30 more would not.
        let items = boxes(&[30, 30, 30]);
        let breaks = first_fit_breaks(&items, &[80]);
        assert_eq!(breaks.len(), 2, "two lines, not one per word");
        // The first break is the glue between word 2 and word 3.
        assert_eq!(breaks[0].index, 3);
    }

    #[test]
    fn a_word_wider_than_the_measure_gets_its_own_line() {
        let items = boxes(&[20, 200, 20]);
        let breaks = first_fit_breaks(&items, &[80]);
        // 20 | 200 | 20 — the over-wide word cannot be helped, but it
        // must not drag its neighbours onto its line.
        assert_eq!(breaks.len(), 3);
    }

    #[test]
    fn a_hyphen_is_taken_only_when_no_glue_will_do() {
        // One long word split at a flagged penalty: "aaaa-bbbb".
        let items: Vec<Item<()>> = vec![
            Item::Box {
                width: 50,
                data: (),
            },
            Item::Penalty {
                width: 5,
                penalty: 50,
                flagged: true,
            },
            Item::Box {
                width: 50,
                data: (),
            },
            Item::Glue {
                width: 0,
                stretch: i32::MAX / 4,
                shrink: 0,
            },
            Item::Penalty {
                width: 0,
                penalty: -INFINITE_PENALTY,
                flagged: true,
            },
        ];
        let breaks = first_fit_breaks(&items, &[60]);
        assert_eq!(breaks.len(), 2, "the word splits at its hyphen");
        assert_eq!(breaks[0].index, 1);
    }

    #[test]
    fn an_empty_item_list_breaks_nowhere() {
        let items: Vec<Item<()>> = Vec::new();
        assert!(first_fit_breaks(&items, &[80]).is_empty());
    }
}
