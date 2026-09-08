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

//! libhyphen (`hyph_*.dic`) pattern files — the data Adobe hyphenates
//! with.
//!
//! InDesign 2025 ships an `AdobeHunspellPlugin` whose `hyph_en_US.dic`
//! is a file in exactly this format, and running Liang's algorithm over
//! it with the paragraph's own limits reproduces InDesign's measured
//! line breaks word for word. Adobe's copy is theirs; the open
//! LibreOffice one is vendored beside this module and gets within one
//! word of it on the same test.
//!
//! The format: line 1 names the encoding; then `LEFTHYPHENMIN` /
//! `RIGHTHYPHENMIN` / `COMPOUND*` declarations, `%` comments, and one
//! Liang pattern per line — letters interleaved with digits, `.`
//! anchoring a word boundary. Odd values at a position permit a break
//! there, even values forbid one, and the highest value at each
//! position wins.
//!
//! Not implemented: `NEXTLEVEL` (a second compounding pass, used by the
//! German file we do not vendor) and the non-standard `=` / `/` pattern
//! forms that spell a break which changes the spelling. Lines using
//! either are skipped rather than half-read — eight of them across
//! every file vendored here, all Spanish.

use std::collections::HashMap;

/// One language's Liang patterns, keyed by their letters.
pub struct Patterns {
    /// Pattern letters (lowercase, `.` for a word boundary) → the
    /// point value at each of the `letters.len() + 1` positions.
    map: HashMap<Box<str>, Box<[u8]>>,
    /// The file's own `LEFTHYPHENMIN` / `RIGHTHYPHENMIN`. Kept for
    /// reference: InDesign overrides them with the paragraph's limits
    /// (measured), so they are not applied here.
    pub declared_left_min: usize,
    pub declared_right_min: usize,
}

impl std::fmt::Debug for Patterns {
    /// Names the size, never the contents — a `{:?}` of eleven thousand
    /// patterns helps nobody.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Patterns")
            .field("patterns", &self.map.len())
            .field("declared_left_min", &self.declared_left_min)
            .field("declared_right_min", &self.declared_right_min)
            .finish()
    }
}

impl Patterns {
    /// Parse a `hyph_*.dic`. `src` is the file's text, already decoded.
    pub fn parse(src: &str) -> Self {
        let mut map: HashMap<Box<str>, Box<[u8]>> = HashMap::new();
        let mut declared_left_min = 2;
        let mut declared_right_min = 2;
        for (i, raw) in src.lines().enumerate() {
            let line = raw.trim();
            // Line 0 is the encoding name, never a pattern.
            if i == 0 || line.is_empty() || line.starts_with('%') || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("LEFTHYPHENMIN ") {
                declared_left_min = rest.trim().parse().unwrap_or(declared_left_min);
                continue;
            }
            if let Some(rest) = line.strip_prefix("RIGHTHYPHENMIN ") {
                declared_right_min = rest.trim().parse().unwrap_or(declared_right_min);
                continue;
            }
            if line.starts_with("COMPOUND") || line.starts_with("NEXTLEVEL") {
                continue;
            }
            // Non-standard patterns change the spelling around the
            // break; skipping one loses an opportunity, mis-reading one
            // would place a hyphen inside a word.
            if line.contains('=') || line.contains('/') {
                continue;
            }
            let mut letters = String::with_capacity(line.len());
            let mut values: Vec<u8> = vec![0];
            for ch in line.chars() {
                match ch.to_digit(10) {
                    Some(d) => {
                        // The digit applies at the position before the
                        // letter that follows it.
                        *values.last_mut().expect("seeded") = d as u8;
                    }
                    None => {
                        letters.push(ch);
                        values.push(0);
                    }
                }
            }
            if letters.is_empty() {
                continue;
            }
            map.insert(letters.into_boxed_str(), values.into_boxed_slice());
        }
        Self {
            map,
            declared_left_min,
            declared_right_min,
        }
    }

    /// How many patterns the file contributed — the vacuity guard for a
    /// data file that failed to load or parse.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Byte offsets inside `word` where a hyphen may go, leaving at
    /// least `left_min` characters before it and `right_min` after.
    ///
    /// `word` is matched case-insensitively; offsets index the original.
    pub fn breaks(&self, word: &str, left_min: usize, right_min: usize) -> Vec<usize> {
        let chars: Vec<char> = word.chars().collect();
        let n = chars.len();
        if n < 2 {
            return Vec::new();
        }
        // `.word.` lowercased — the boundary dots are what anchors a
        // pattern to the start or end of a word.
        let mut keyed = String::with_capacity(word.len() + 2);
        keyed.push('.');
        for c in &chars {
            keyed.extend(c.to_lowercase());
        }
        keyed.push('.');
        // Position `k` of `points` sits before keyed char `k`.
        let keyed_chars: Vec<char> = keyed.chars().collect();
        let mut points = vec![0u8; keyed_chars.len() + 1];
        // Byte offset of each char in `keyed`, plus the end.
        let mut starts: Vec<usize> = keyed.char_indices().map(|(i, _)| i).collect();
        starts.push(keyed.len());
        for i in 0..keyed_chars.len() {
            for j in (i + 1)..=keyed_chars.len() {
                if let Some(values) = self.map.get(&keyed[starts[i]..starts[j]]) {
                    for (k, v) in values.iter().enumerate() {
                        let at = i + k;
                        if at < points.len() && *v > points[at] {
                            points[at] = *v;
                        }
                    }
                }
            }
        }
        // `points[k]` guards the gap before keyed char `k`; keyed char
        // 1 is the word's first character, so the gap before word char
        // `c` is `points[c + 1]`.
        //
        // A lowercase mapping may not be 1:1 in chars (ẞ → ss), which
        // would slide every later position; fall back to no breaks
        // rather than hyphenate at a wrong offset.
        if keyed_chars.len() != n + 2 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut offset = 0usize;
        for (c, ch) in chars.iter().enumerate() {
            if c > 0 && c >= left_min && n - c >= right_min && points[c + 1] % 2 == 1 {
                out.push(offset);
            }
            offset += ch.len_utf8();
        }
        out
    }
}

/// Decode a vendored file according to the encoding it declares on its
/// first line. Only the two encodings the vendored files use are
/// handled; anything else is read as UTF-8, which is right for every
/// pattern file published since 2010.
pub fn decode(bytes: &'static [u8]) -> String {
    let head_len = bytes
        .iter()
        .position(|b| *b == b'\n')
        .unwrap_or(bytes.len());
    let head = String::from_utf8_lossy(&bytes[..head_len]);
    if head.trim().eq_ignore_ascii_case("ISO8859-1") {
        // Latin-1 is the identity on code points 0..=255.
        return bytes.iter().map(|b| *b as char).collect();
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_pattern_into_positions() {
        let p = Patterns::parse("UTF-8\nLEFTHYPHENMIN 2\nRIGHTHYPHENMIN 3\n%comment\nhy3ph\n");
        assert_eq!(p.len(), 1);
        assert_eq!(p.declared_left_min, 2);
        assert_eq!(p.declared_right_min, 3);
        // "hyph" with a 3 between y and p.
        assert_eq!(&**p.map.get("hyph").expect("pattern"), &[0, 0, 3, 0, 0]);
    }

    #[test]
    fn a_leading_digit_binds_before_the_first_letter() {
        let p = Patterns::parse("UTF-8\n4a1ma\n");
        assert_eq!(&**p.map.get("ama").expect("pattern"), &[4, 1, 0, 0]);
    }

    #[test]
    fn non_standard_and_compound_lines_are_skipped_not_half_read() {
        let p = Patterns::parse("UTF-8\nNEXTLEVEL\nCOMPOUNDLEFTHYPHENMIN 2\na1b=c\nd1e/f\ng1h\n");
        assert_eq!(p.len(), 1, "only the plain pattern survives");
    }

    #[test]
    fn latin_1_is_decoded_by_the_declared_encoding() {
        // 0xE9 is é in Latin-1 and invalid UTF-8.
        let bytes: &'static [u8] = b"ISO8859-1\n.a\xe91\n";
        assert!(decode(bytes).contains('\u{e9}'));
        let utf8: &'static [u8] = b"UTF-8\n.a\xc3\xa91\n";
        assert!(decode(utf8).contains('\u{e9}'));
    }

    #[test]
    fn the_limits_are_applied_in_characters_and_offsets_are_bytes() {
        // One pattern that permits a break after every letter.
        let p = Patterns::parse("UTF-8\na1b1c1d1e\n");
        assert_eq!(p.breaks("abcde", 1, 1), vec![1, 2, 3, 4]);
        assert_eq!(p.breaks("abcde", 2, 2), vec![2, 3]);
        assert_eq!(p.breaks("abcde", 9, 1), Vec::<usize>::new());
    }
}
