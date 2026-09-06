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

//! Hyphenation via TeX patterns.
//!
//! Wraps `hypher` (typst's pattern-trie crate) with a thin loader that
//! picks a language at runtime. The composer uses this to insert
//! flagged penalty break opportunities mid-word; whether to take them
//! is decided by `paragraph_breaker` against the configured tolerance.
//!
//! `hypher` ships pattern data inline as compact tries (~1-2 MB total
//! across ~70 languages), so there's no runtime dictionary loading and
//! no separate asset to bundle for WASM. Same Liang-pattern algorithm
//! and break quality as the older `hyphenation` crate; the upgrade
//! reason is purely binary size.

use hypher::Lang;

/// Supported hyphenation languages. Maps to `hypher::Lang` without
/// exposing that crate's whole enum publicly — keeps the API stable
/// as the dictionary list grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    EnglishUS,
    EnglishGB,
    German1996,
    French,
    Spanish,
    Italian,
    Dutch,
    Portuguese,
}

impl Language {
    fn to_hypher(self) -> Lang {
        match self {
            // hypher doesn't split English by region — both US/GB land
            // on the same shared dictionary.
            Language::EnglishUS | Language::EnglishGB => Lang::English,
            Language::German1996 => Lang::German,
            Language::French => Lang::French,
            Language::Spanish => Lang::Spanish,
            Language::Italian => Lang::Italian,
            Language::Dutch => Lang::Dutch,
            Language::Portuguese => Lang::Portuguese,
        }
    }
}

/// Hyphenation engine for a single language. Cheap to clone (the
/// underlying language enum is `Copy`).
#[derive(Debug, Clone, Copy)]
pub struct Hyphenator {
    lang: Lang,
}

impl Hyphenator {
    /// Pick the embedded TeX dictionary for `lang`.
    pub fn for_language(lang: Language) -> Self {
        Self {
            lang: lang.to_hypher(),
        }
    }

    /// Stable byte id of the underlying language. Used by the layout
    /// cache to fold the hyphenator's contribution into a cache key
    /// without depending on Debug-format stability.
    pub fn lang_id(&self) -> u8 {
        match self.lang {
            Lang::English => 1,
            Lang::German => 2,
            Lang::French => 3,
            Lang::Spanish => 4,
            Lang::Italian => 5,
            Lang::Dutch => 6,
            Lang::Portuguese => 7,
            _ => 0,
        }
    }

    /// Return a list of byte indices inside `word` where a hyphen
    /// could be inserted, under InDesign's default hyphenation limits
    /// (see [`Self::opportunities_with`]). Indices are relative to the
    /// slice — so for "computer" the result is [3, 6] (com-put-er). The
    /// list never contains 0 or `word.len()` (those aren't hyphenation
    /// breaks).
    pub fn opportunities(&self, word: &str) -> Vec<usize> {
        self.opportunities_with(word, AFTER_FIRST, BEFORE_LAST, WORDS_LONGER_THAN)
    }

    /// Break opportunities inside `word` limited the way InDesign's
    /// paragraph hyphenation settings limit them: only the alphabetic
    /// core of the token is hyphenated (a leading "(" or a trailing ","
    /// is not a letter — measured 2026-09-06: InDesign set "(overset)"
    /// whole where the unlimited patterns broke it "(o-verset)"), the
    /// core must have more than `words_longer_than` letters, and a break
    /// must leave at least `after_first` letters before the hyphen and
    /// `before_last` letters after it. Indices are byte offsets into the
    /// full token.
    pub fn opportunities_with(
        &self,
        word: &str,
        after_first: usize,
        before_last: usize,
        words_longer_than: usize,
    ) -> Vec<usize> {
        // The alphabetic core: skip leading / trailing non-letters.
        let head = word
            .char_indices()
            .find(|(_, c)| c.is_alphabetic())
            .map(|(i, _)| i);
        let Some(head) = head else {
            return Vec::new();
        };
        let tail = word
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_alphabetic())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(word.len());
        let core = &word[head..tail];
        let letters = core.chars().count();
        if letters < words_longer_than {
            return Vec::new();
        }
        // hypher::hyphenate yields syllable slices in order. Their
        // cumulative byte lengths give the break offsets we want.
        let mut breaks = Vec::new();
        let mut offset = 0usize;
        let mut iter = hypher::hyphenate(core, self.lang);
        // The first syllable doesn't produce a break — skip it.
        if let Some(first) = iter.next() {
            offset += first.len();
        }
        for syllable in iter {
            // Successive iterations: the break sits at the boundary
            // between the previous syllable and this one.
            if offset > 0 && offset < core.len() {
                let before = core[..offset].chars().count();
                let after = letters - before;
                if before >= after_first && after >= before_last {
                    breaks.push(head + offset);
                }
            }
            offset += syllable.len();
        }
        breaks
    }
}

/// InDesign's default "Hyphenate: After First _ letters".
const AFTER_FIRST: usize = 2;
/// InDesign's default "Hyphenate: Before Last _ letters".
const BEFORE_LAST: usize = 2;
/// InDesign's default "Words with at Least _ letters" (5).
const WORDS_LONGER_THAN: usize = 5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_word_has_break_opportunities() {
        let h = Hyphenator::for_language(Language::EnglishUS);
        let breaks = h.opportunities("computer");
        assert!(!breaks.is_empty(), "expected breaks in 'computer'");
        // Every break sits strictly inside the word.
        for &b in &breaks {
            assert!(b > 0 && b < "computer".len());
        }
    }

    #[test]
    fn short_word_has_no_breaks() {
        let h = Hyphenator::for_language(Language::EnglishUS);
        assert!(h.opportunities("a").is_empty());
        assert!(h.opportunities("the").is_empty());
        // Four letters: under InDesign's "words with at least 5 letters".
        assert!(h.opportunities("open").is_empty());
    }

    #[test]
    fn punctuation_is_not_a_letter_and_the_edge_limits_hold() {
        let h = Hyphenator::for_language(Language::EnglishUS);
        // "(o-ver-set)": the "o" alone before a hyphen is under the
        // after-first-2 limit, so only "(over|set)" survives — and the
        // offset counts the "(" the patterns never saw.
        assert_eq!(h.opportunities("(overset)"), vec![5]);
        assert_eq!(h.opportunities("overset"), vec![4]);
        // A trailing comma is not a letter either: the patterns see
        // "computer" (com-puter), not "computer," (com-put-er,).
        assert_eq!(h.opportunities("computer,"), h.opportunities("computer"));
        // The limits are what gate the edges: "over-set" needs at least
        // four letters before the hyphen when the paragraph asks for it.
        assert!(h.opportunities_with("(overset)", 5, 2, 5).is_empty());
    }
}
