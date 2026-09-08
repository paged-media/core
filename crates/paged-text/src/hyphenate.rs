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

//! Hyphenation via Liang patterns.
//!
//! Two pattern sources, because they are not equivalent and the
//! difference is measurable. Where the licence lets us, the language's
//! `hyph_*.dic` from LibreOffice is vendored (`../patterns/`) and read
//! by [`crate::libhyphen`] — that is the same FORMAT Adobe hyphenates
//! with, and running it over Adobe's own `hyph_en_US.dic` reproduces
//! InDesign's measured breaks exactly. Everywhere else `hypher`
//! (typst's pattern-trie crate) stands in.
//!
//! The two differ by more than packaging: on 25 English words swept in
//! InDesign, the vendored `hyph_en_US.dic` agrees on 24 and hypher's
//! trie on 16, because the LibreOffice file folds in the TUGboat
//! hyphenation-exception log and the trie carries only the base
//! patterns. `hypher` also collapses English into one dictionary; the
//! vendored files keep US and GB apart, which they are.
//!
//! The composer uses this to insert flagged penalty break
//! opportunities mid-word; whether to take them is decided by
//! `paragraph_breaker` against the configured tolerance.

use std::sync::OnceLock;

use hypher::Lang;

use crate::libhyphen::{self, Patterns};

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
            // on the same shared dictionary. The vendored files do
            // split them, so this only matters as the fallback.
            Language::EnglishUS | Language::EnglishGB => Lang::English,
            Language::German1996 => Lang::German,
            Language::French => Lang::French,
            Language::Spanish => Lang::Spanish,
            Language::Italian => Lang::Italian,
            Language::Dutch => Lang::Dutch,
            Language::Portuguese => Lang::Portuguese,
        }
    }

    /// The vendored `hyph_*.dic` for this language, or `None` when its
    /// upstream licence keeps it out of this repo (German, French,
    /// Italian and Portuguese are LGPL or GPL — see
    /// `../patterns/README.md`).
    fn vendored(self) -> Option<&'static Patterns> {
        macro_rules! vendored {
            ($cell:ident, $file:literal) => {{
                static $cell: OnceLock<Patterns> = OnceLock::new();
                Some($cell.get_or_init(|| {
                    Patterns::parse(&libhyphen::decode(include_bytes!(concat!(
                        "../patterns/",
                        $file
                    ))))
                }))
            }};
        }
        match self {
            Language::EnglishUS => vendored!(EN_US, "hyph_en_US.dic"),
            Language::EnglishGB => vendored!(EN_GB, "hyph_en_GB.dic"),
            Language::Spanish => vendored!(ES, "hyph_es.dic"),
            Language::Dutch => vendored!(NL, "hyph_nl_NL.dic"),
            Language::German1996 | Language::French | Language::Italian | Language::Portuguese => {
                None
            }
        }
    }

    /// Stable byte id, distinct per language. The layout cache keys on
    /// it, so US and GB English must not share one now that they read
    /// different patterns.
    fn id(self) -> u8 {
        match self {
            Language::EnglishUS => 1,
            Language::German1996 => 2,
            Language::French => 3,
            Language::Spanish => 4,
            Language::Italian => 5,
            Language::Dutch => 6,
            Language::Portuguese => 7,
            Language::EnglishGB => 8,
        }
    }
}

/// Hyphenation engine for a single language. Cheap to clone (both
/// variants are `Copy`).
#[derive(Debug, Clone, Copy)]
pub struct Hyphenator {
    language: Language,
    /// The vendored `hyph_*.dic`, when this language has one; `None`
    /// falls through to `hypher`'s trie.
    patterns: Option<&'static Patterns>,
    lang: Lang,
}

impl Hyphenator {
    /// Pick the pattern source for `lang` — the vendored `hyph_*.dic`
    /// where there is one, else hypher's embedded trie.
    pub fn for_language(lang: Language) -> Self {
        Self {
            language: lang,
            patterns: lang.vendored(),
            lang: lang.to_hypher(),
        }
    }

    /// Stable byte id of the language. Used by the layout cache to fold
    /// the hyphenator's contribution into a cache key without
    /// depending on Debug-format stability.
    pub fn lang_id(&self) -> u8 {
        self.language.id()
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

    /// Break opportunities under a paragraph's own hyphenation
    /// settings. `is_last_word` says whether this token ends the
    /// paragraph, which `HyphenateLastWord="false"` protects.
    ///
    /// A word carrying a SOFT HYPHEN breaks only there — the author
    /// placed a discretionary hyphen, and InDesign takes that as the
    /// whole answer for the word.
    pub fn opportunities_for(
        &self,
        word: &str,
        limits: &HyphenationLimits,
        is_last_word: bool,
    ) -> Vec<usize> {
        if word.contains(SOFT_HYPHEN) {
            return word
                .char_indices()
                .filter(|(_, c)| *c == SOFT_HYPHEN)
                .map(|(i, c)| i + c.len_utf8())
                .filter(|&i| i > 0 && i < word.len())
                .collect();
        }
        if is_last_word && !limits.last_word {
            return Vec::new();
        }
        if !limits.capitalized_words
            && word
                .chars()
                .find(|c| c.is_alphabetic())
                .is_some_and(char::is_uppercase)
        {
            return Vec::new();
        }
        self.opportunities_with(
            word,
            limits.after_first,
            limits.before_last,
            limits.words_longer_than,
        )
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
        // The vendored `hyph_*.dic` is matched with the PARAGRAPH's
        // limits, because that is what InDesign does: its factory
        // `HyphenateBeforeLast` is 2 and it really takes those breaks —
        // `com-put-er`, `de-sign-er`, `pub-lish-er`, `print-er`,
        // `print-ed`, `start-ed`, all measured on InDesign 20.0.1 by
        // sweeping each word's frame width. The file's own
        // `RIGHTHYPHENMIN 3` is read and deliberately not applied;
        // running Liang over Adobe's own `hyph_en_US.dic` with the
        // paragraph's 2 reproduces all 25 measured words exactly, and
        // with the file's 3 it reproduces 19.
        if let Some(patterns) = self.patterns {
            return patterns
                .breaks(core, after_first, before_last)
                .into_iter()
                .map(|o| head + o)
                .collect();
        }
        // hypher's trie carries no such declaration to consult, so its
        // language bounds stand and the paragraph's limits below can
        // only tighten them. Loosening THEM was measured and rejected:
        // the trie lacks the exception half of the dictionary, so a
        // looser bound bought six English words and cost the corpus
        // (`text-wrap` 0.639 → 0.823 mean ΔE, `text-in-shape`'s donut
        // 2.000 → 2.239) by unlocking breaks Adobe refuses. This path
        // is now only the four languages whose patterns are LGPL or GPL
        // and so are not vendored here.
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

/// U+00AD SOFT HYPHEN — InDesign's "discretionary hyphen". A word that
/// carries one hyphenates ONLY there: the author has said where the
/// break belongs, and the dictionary does not get a second opinion.
pub const SOFT_HYPHEN: char = '\u{00ad}';

/// A paragraph's hyphenation settings, the seven IDML attributes that
/// gate where a word may break.
///
/// The composer used to hard-code InDesign's factory 2 / 2 / 5 and
/// ignore the rest, so a paragraph that turned capitalised-word
/// hyphenation off, or asked for four letters before the hyphen, got
/// the defaults anyway.
///
/// Absent attributes keep today's behaviour rather than InDesign's
/// factory value where the two differ: `ladder_limit` defaults to
/// "unlimited" because our composer has never enforced one and
/// adopting InDesign's 3 unmeasured would move every justified
/// paragraph in every document at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HyphenationLimits {
    /// `HyphenateAfterFirst` — letters that must precede the hyphen.
    pub after_first: usize,
    /// `HyphenateBeforeLast` — letters that must follow it.
    pub before_last: usize,
    /// `HyphenateWordsLongerThan` — shorter words are never broken.
    pub words_longer_than: usize,
    /// `HyphenateCapitalizedWords`. `false` leaves a word whose first
    /// letter is a capital whole (hypher lowercases internally, so
    /// this test has to be ours).
    pub capitalized_words: bool,
    /// `HyphenateLastWord`. `false` leaves the paragraph's final word
    /// whole, so the last line never ends in a hyphen.
    pub last_word: bool,
    /// `HyphenateLadderLimit` — the most consecutive lines that may
    /// end in a hyphen. `0` means unlimited.
    pub ladder_limit: usize,
}

impl Default for HyphenationLimits {
    fn default() -> Self {
        Self {
            after_first: AFTER_FIRST,
            before_last: BEFORE_LAST,
            words_longer_than: WORDS_LONGER_THAN,
            capitalized_words: true,
            last_word: true,
            ladder_limit: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    /// The paragraph's `HyphenateBeforeLast` wins over the pattern
    /// file's own `RIGHTHYPHENMIN 3`, which is what InDesign does:
    /// these six all leave exactly two letters after the hyphen, and
    /// InDesign takes every one (20.0.1, English: USA, each word swept
    /// by frame width to enumerate its breaks).
    #[test]
    fn the_paragraphs_limit_wins_over_the_files_declared_minimum() {
        let h = Hyphenator::for_language(Language::EnglishUS);
        for (word, want) in [
            ("computer", vec![3usize, 6]),
            ("designer", vec![2, 6]),
            ("publisher", vec![3, 7]),
            ("printer", vec![5]),
            ("printed", vec![5]),
            ("started", vec![5]),
        ] {
            assert_eq!(h.opportunities(word), want, "{word}");
        }
    }

    /// A paragraph may ask for MORE than the patterns' minimum, and
    /// that is honoured — the limits tighten, they never loosen.
    #[test]
    fn a_paragraph_can_tighten_the_patterns_minimum() {
        let h = Hyphenator::for_language(Language::EnglishUS);
        assert_eq!(h.opportunities("typography"), vec![2, 5, 7]);
        // "Before last 4" drops typogra-phy: only three letters follow.
        assert_eq!(h.opportunities_with("typography", 2, 4, 5), vec![2, 5]);
        // "After first 3" drops ty-pography: only two letters precede.
        assert_eq!(h.opportunities_with("typography", 3, 3, 5), vec![5, 7]);
    }

    /// The same sweep's words where we already agreed — pinned so the
    /// looser bound does not start inventing breaks InDesign refuses.
    #[test]
    fn the_measured_breaks_indesign_takes_are_the_ones_we_offer() {
        let h = Hyphenator::for_language(Language::EnglishUS);
        for (word, want) in [
            ("walked", vec![]),
            ("sentence", vec![3usize]),
            ("yesterday", vec![3, 6]),
            ("paragraph", vec![4]),
            ("hyphenation", vec![2, 6]),
            ("typography", vec![2, 5, 7]),
            ("background", vec![4]),
            ("something", vec![4]),
            ("understand", vec![2, 5]),
            ("information", vec![2, 5, 7]),
            ("development", vec![2, 5, 7]),
            ("rendered", vec![3]),
            ("modern", vec![3]),
            ("capital", vec![3, 4]),
            ("layouts", vec![3]),
            ("broken", vec![3]),
        ] {
            assert_eq!(h.opportunities(word), want, "{word}");
        }
    }

    /// The one word of the measured 25 still left to Adobe's own,
    /// proprietary pattern file. `spell-ing` and `ev-ery-thing` used to
    /// be here too and are now right — they came with the TUGboat
    /// exception log the vendored file carries.
    #[test]
    fn the_one_word_adobes_own_dictionary_still_wins() {
        let h = Hyphenator::for_language(Language::EnglishUS);
        // InDesign: win-dow-sill. The open patterns offer win-dowsill.
        assert_eq!(h.opportunities("windowsill"), vec![3]);
        assert_eq!(h.opportunities("spelling"), vec![5]);
        assert_eq!(h.opportunities("everything"), vec![2, 5]);
    }

    /// Every vendored file loads and parses to a plausible size — the
    /// guard against a data file that went missing, got truncated, or
    /// was read in the wrong encoding and produced nothing.
    #[test]
    fn every_vendored_pattern_file_loads() {
        for (lang, least) in [
            (Language::EnglishUS, 10_000),
            (Language::EnglishGB, 12_000),
            (Language::Spanish, 5_000),
            (Language::Dutch, 14_000),
        ] {
            let p = lang
                .vendored()
                .unwrap_or_else(|| panic!("{lang:?} is vendored"));
            assert!(
                p.len() >= least,
                "{lang:?} parsed only {} patterns",
                p.len()
            );
        }
        // The four whose upstream licence keeps them out of this repo
        // fall back to hypher rather than to nothing.
        for lang in [
            Language::German1996,
            Language::French,
            Language::Italian,
            Language::Portuguese,
        ] {
            assert!(lang.vendored().is_none(), "{lang:?} is not vendored");
            assert!(
                !Hyphenator::for_language(lang)
                    .opportunities("Silbentrennung")
                    .is_empty()
                    || lang != Language::German1996,
                "the fallback still hyphenates"
            );
        }
    }

    /// US and GB English are different dictionaries now, so they must
    /// not share a layout-cache key.
    #[test]
    fn the_two_englishes_are_told_apart() {
        let us = Hyphenator::for_language(Language::EnglishUS);
        let gb = Hyphenator::for_language(Language::EnglishGB);
        assert_ne!(us.lang_id(), gb.lang_id());
        // `capital`: cap-i-tal in American English, cap-it-al in
        // British — the two files really do disagree.
        assert_eq!(us.opportunities("capital"), vec![3, 4]);
        assert_eq!(gb.opportunities("capital"), vec![3, 5]);
    }

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
    fn a_paragraph_can_protect_capitalised_words_and_its_last_one() {
        let h = Hyphenator::for_language(Language::EnglishUS);
        let defaults = HyphenationLimits::default();
        assert!(!h.opportunities_for("computer", &defaults, false).is_empty());
        assert!(
            !h.opportunities_for("Computer", &defaults, false).is_empty(),
            "capitalised words hyphenate by default"
        );

        let no_caps = HyphenationLimits {
            capitalized_words: false,
            ..defaults
        };
        assert!(h.opportunities_for("Computer", &no_caps, false).is_empty());
        assert!(
            !h.opportunities_for("computer", &no_caps, false).is_empty(),
            "the rule is about the capital, not the word"
        );

        let no_last = HyphenationLimits {
            last_word: false,
            ..defaults
        };
        assert!(h.opportunities_for("computer", &no_last, true).is_empty());
        assert!(!h.opportunities_for("computer", &no_last, false).is_empty());
    }

    #[test]
    fn a_soft_hyphen_is_the_only_break_the_word_gets() {
        // A discretionary hyphen is the author saying where the word
        // breaks; the dictionary does not get a second opinion.
        let h = Hyphenator::for_language(Language::EnglishUS);
        let limits = HyphenationLimits::default();
        let word = "compu\u{00ad}ter";
        let breaks = h.opportunities_for(word, &limits, false);
        assert_eq!(breaks.len(), 1, "one soft hyphen, one opportunity");
        assert_eq!(&word[..breaks[0]], "compu\u{00ad}");
        // And it wins even where the automatic patterns would say more.
        assert!(!h.opportunities("computer").is_empty());
    }

    #[test]
    fn the_letter_limits_come_from_the_paragraph() {
        let h = Hyphenator::for_language(Language::EnglishUS);
        let strict = HyphenationLimits {
            after_first: 5,
            before_last: 5,
            ..HyphenationLimits::default()
        };
        assert!(h.opportunities_for("computer", &strict, false).is_empty());
        let long_only = HyphenationLimits {
            words_longer_than: 20,
            ..HyphenationLimits::default()
        };
        assert!(h
            .opportunities_for("computer", &long_only, false)
            .is_empty());
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
