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
    /// could be inserted. Indices are relative to the slice — so for
    /// "computer" the result is [3, 6] (com-put-er). The list never
    /// contains 0 or `word.len()` (those aren't hyphenation breaks).
    pub fn opportunities(&self, word: &str) -> Vec<usize> {
        // hypher::hyphenate yields syllable slices in order. Their
        // cumulative byte lengths give the break offsets we want.
        let mut breaks = Vec::new();
        let mut offset = 0usize;
        let mut iter = hypher::hyphenate(word, self.lang);
        // The first syllable doesn't produce a break — skip it.
        if let Some(first) = iter.next() {
            offset += first.len();
        }
        for syllable in iter {
            // Successive iterations: the break sits at the boundary
            // between the previous syllable and this one.
            if offset > 0 && offset < word.len() {
                breaks.push(offset);
            }
            offset += syllable.len();
        }
        breaks
    }
}

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
    }
}
