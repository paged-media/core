//! Hyphenation via TeX patterns.
//!
//! Wraps `hyphenation::Standard` with a thin loader that picks a
//! language at runtime. The composer uses this to insert flagged
//! penalty break opportunities mid-word; whether to take them is
//! decided by `paragraph_breaker` against the configured tolerance.
//!
//! Calibration vs. InDesign's Proximity dictionaries is part of
//! Spike B; for now the TeX patterns ship with the `embed_all`
//! feature of the `hyphenation` crate so no runtime asset loading
//! is required.

use hyphenation::{Hyphenator as _, Load, Standard};

/// Supported hyphenation languages. Map to `hyphenation::Language`
/// without exposing that crate's whole enum publicly — keeps the API
/// stable as the dictionary list grows.
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
    fn to_hyph(self) -> hyphenation::Language {
        use hyphenation::Language as H;
        match self {
            Language::EnglishUS => H::EnglishUS,
            Language::EnglishGB => H::EnglishGB,
            Language::German1996 => H::German1996,
            Language::French => H::French,
            Language::Spanish => H::Spanish,
            Language::Italian => H::Italian,
            Language::Dutch => H::Dutch,
            Language::Portuguese => H::Portuguese,
        }
    }
}

/// Hyphenation engine for a single language. Cheap to clone (the
/// underlying dictionary is shared internally).
pub struct Hyphenator {
    inner: Standard,
}

impl std::fmt::Debug for Hyphenator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hyphenator").finish_non_exhaustive()
    }
}

impl Hyphenator {
    /// Load the embedded TeX dictionary for `lang`.
    pub fn for_language(lang: Language) -> Self {
        // `embed_all` is enabled in Cargo.toml so this is infallible
        // for the bundled languages — unwrap is safe.
        let inner = Standard::from_embedded(lang.to_hyph()).expect("embedded dictionary");
        Self { inner }
    }

    /// Return a list of byte indices inside `word` where a hyphen
    /// could be inserted. Indices are relative to the slice — so for
    /// "computer" the result is [3, 6] (com-put-er). The list never
    /// contains 0 or `word.len()` (those aren't hyphenation breaks).
    pub fn opportunities(&self, word: &str) -> Vec<usize> {
        self.inner.hyphenate(word).breaks.into_iter().collect()
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
