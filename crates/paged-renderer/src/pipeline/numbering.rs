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

//! List-marker machinery: bullet / numbered-list prefixes and the
//! counter formats (`^#` expression substitution, Roman, alpha, hanzi).

/// Map IDML `<TabStop Alignment="...">` values to the layout
/// crate's `TabAlignment`.
/// Build the list-marker prefix for a paragraph, or `None` when no
/// list applies. Mutates `counter` based on the paragraph's
/// numbering attributes:
///  - BulletList: counter resets to 0 (bullets don't number);
///    returns `<bullet><separator>`.
///  - NumberedList: applies `NumberingStartAt` / `NumberingContinue`
///    overrides to `counter`, then increments and substitutes
///    `NumberingExpression` (default `^#.^t`). Tokens: `^#` → the
///    formatted counter (per `numbering_format`), `^.` → a literal
///    period, `^t` → a literal tab. Literal characters pass through.
///  - NoList / absent: counter resets to 0; returns `None`.
///
/// `^t` substitutions are snapped to the next tab stop by the
/// existing `apply_tab_stops` pass; the default 36 pt grid gives a
/// reasonable hanging indent without explicit `<TabList>`.
///
/// W1.22 — `cross_story_seed` carries cross-story numbering continuity
/// (engine gap 22). When `Some(prior)`, the paragraph is bound to a
/// `<NumberingList>` with `ContinueNumbersAcrossStories="true"` and
/// `prior` is that list's last counter value from the document-level
/// ledger. In that mode the implicit per-story reset is suppressed:
/// the counter is seeded from `prior` so the first numbered paragraph
/// of the list in a later story continues the sequence rather than
/// restarting at 1. `None` ⇒ the legacy per-story scope (no named
/// continue-across-stories list applies). `NumberingStartAt` still
/// wins over the seed — an explicit restart is honoured even for a
/// continued list (matches InDesign, where "Start At" overrides
/// "Continue from Previous Number").
pub(super) fn list_prefix(
    p: &paged_scene::ResolvedParagraphAttrs,
    counter: &mut u32,
    prev_was_numbered: &mut bool,
    cross_story_seed: Option<u32>,
) -> Option<String> {
    match p.bullets_list_type.as_deref() {
        Some("BulletList") => {
            // Don't touch the counter here — a later NumberedList
            // paragraph with `NumberingContinue` may want to resume
            // off the prior count across an intervening bullet.
            *prev_was_numbered = false;
            // InDesign's default bullet glyph when none is declared
            // is U+2022 (•). Real IDML usually carries an explicit
            // BulletChar, but real-world exports sometimes leave it
            // implicit on the cascade — fall back so visible bullets
            // still appear.
            let cp = p.bullet_character.unwrap_or(0x2022);
            let ch = char::from_u32(cp)?;
            // `^t` in IDML serialises a literal tab in BulletsTextAfter.
            let after = p
                .bullets_text_after
                .as_deref()
                .map(|s| s.replace("^t", "\t"))
                .unwrap_or_else(|| " ".to_string());
            Some(format!("{ch}{after}"))
        }
        Some("NumberedList") => {
            // Decide whether to reset the counter on entry:
            //   1. Explicit `NumberingStartAt` always wins — the
            //      counter jumps to (start - 1) so the increment
            //      below lands on `start`.
            //   2. Otherwise, if the previous paragraph wasn't
            //      numbered AND this paragraph isn't carrying
            //      `NumberingContinue="true"`, reset to 0 so the
            //      increment lands at 1 (a fresh sequence).
            //   3. Otherwise carry the count forward.
            if let Some(start) = p.numbering_start_at {
                // Negative IDML values clamp to 0 (renders as "0" /
                // whatever the format yields for n=0; matches
                // InDesign's UI which disallows entries < 1 but the
                // schema permits them).
                *counter = (start - 1).max(0) as u32;
            } else if let Some(prior) = cross_story_seed {
                // W1.22 — a ContinueNumbersAcrossStories list. Seed
                // from the document-level ledger and DON'T apply the
                // per-story implicit reset: the first numbered
                // paragraph of this list in a later story must
                // continue, not restart. `prev_was_numbered` is local
                // to this story's emitter, so at story start it is
                // false (no neighbour) — exactly the case the legacy
                // branch would have reset; the ledger seed overrides it.
                *counter = prior;
            } else if !*prev_was_numbered && p.numbering_continue != Some(true) {
                *counter = 0;
            }
            *counter = counter.checked_add(1).unwrap_or(1);
            *prev_was_numbered = true;
            let formatted = format_number(*counter, p.numbering_format.as_deref());
            // IDML default expression is `^#.^t` — `<n>` + period +
            // tab. The tab snaps to a tab stop via `apply_tab_stops`
            // (default 36 pt grid if no <TabList>), giving a
            // hanging indent without explicit setup.
            let expr = p.numbering_expression.as_deref().unwrap_or("^#.^t");
            Some(substitute_numbering_expression(expr, &formatted))
        }
        _ => {
            // NoList / absent. Like BulletList, don't reset the
            // counter — a later NumberedList paragraph with
            // `NumberingContinue` may want to resume.
            *prev_was_numbered = false;
            None
        }
    }
}

/// Pick the cascaded `CharacterStyle/<id>` that styles the list
/// marker, per IDML's two-field convention:
///
/// - `NumberedList` paragraphs read
///   `BulletsAndNumberingDigitsCharacterStyle` (the digits-style).
/// - `BulletList` paragraphs read `BulletsCharacterStyle` if set,
///   otherwise fall back to
///   `BulletsAndNumberingDigitsCharacterStyle` — the InDesign UI
///   exposes a single "Character Style" picker per paragraph style
///   regardless of list kind, and real-world IDML often lands the
///   reference in the digits-style slot even when the paragraph is
///   a bullet list.
///
/// Returns `None` when no override applies (the bullet/marker then
/// inherits the first run's formatting, the historical behaviour).
pub(super) fn bullet_marker_character_style(
    p: &paged_scene::ResolvedParagraphAttrs,
) -> Option<&str> {
    match p.bullets_list_type.as_deref() {
        Some("NumberedList") => p.bullets_and_numbering_digits_character_style.as_deref(),
        Some("BulletList") => p
            .bullets_character_style
            .as_deref()
            .or(p.bullets_and_numbering_digits_character_style.as_deref()),
        _ => None,
    }
}

/// Substitute `^#`, `^.`, `^t` tokens in a NumberingExpression
/// template. Anything else (including unknown `^x` sequences) passes
/// through unchanged.
///
/// IDML escapes a literal caret as `^^` (a doubled caret); decode
/// that so styles that want a literal `^` in their template don't
/// accidentally trigger token replacement.
pub(super) fn substitute_numbering_expression(expr: &str, formatted_counter: &str) -> String {
    let mut out = String::with_capacity(expr.len() + formatted_counter.len());
    let mut chars = expr.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' {
            match chars.peek().copied() {
                Some('#') => {
                    chars.next();
                    out.push_str(formatted_counter);
                }
                Some('.') => {
                    chars.next();
                    out.push('.');
                }
                Some('t') => {
                    chars.next();
                    out.push('\t');
                }
                Some('^') => {
                    chars.next();
                    out.push('^');
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Format a 1-based list counter per IDML's `NumberingFormat`
/// sample string. Reads the prefix before the first comma to
/// pick a style:
///  - "1, 2, 3..."   → Arabic ("1", "2", "3", ...)
///  - "01, 02, 03..." (or "001, ...") → zero-padded Arabic
///  - "I, II, III..." → upper Roman
///  - "i, ii, iii..." → lower Roman
///  - "A, B, C..."   → upper alpha (A..Z, AA..ZZ, ...)
///  - "a, b, c..."   → lower alpha
///
/// Anything else (or `None`) falls through to plain Arabic.
pub(super) fn format_number(n: u32, format: Option<&str>) -> String {
    let Some(spec) = format else {
        return n.to_string();
    };
    let head = spec.split(',').next().unwrap_or("").trim();
    match head {
        "I" => to_roman(n, false),
        "i" => to_roman(n, true),
        "A" => to_alpha(n, false),
        "a" => to_alpha(n, true),
        // Phase 7 — CJK numerals. IDML serialises these as the sample
        // first-glyph (the same convention as Latin formats). 一 is
        // the Han numeral 1; 壹 is the formal "financial" variant.
        // Numbers above the implemented ceiling fall back to Arabic
        // (matches InDesign's behaviour for numbers it can't represent
        // in the chosen system).
        "一" => to_hanzi(n, false),
        "壹" => to_hanzi(n, true),
        s if s.starts_with('0') && s.chars().all(|c| c.is_ascii_digit()) => {
            // Zero-padded Arabic; width = head's length.
            format!("{:0>width$}", n, width = s.len())
        }
        _ => n.to_string(),
    }
}

/// Phase 7 — Chinese / Japanese numeral conversion. `formal` selects
/// the "financial" character set (壹, 貳, 參, …) over the everyday
/// set (一, 二, 三, …). Both share the same digit shape for numbers
/// ≥ 10 (十, 百, 千, 萬) since the financial variant only applies to
/// the unit digits.
///
/// Implemented for 1..=999. Larger values fall through to Arabic to
/// avoid emitting partial/incorrect strings for very long
/// documents — matches the spirit of Adobe's fallback when a chosen
/// numbering system can't represent the actual page number.
fn to_hanzi(n: u32, formal: bool) -> String {
    if n == 0 || n > 999 {
        return n.to_string();
    }
    // Digit tables. Index 0 is empty so digit[n] is the glyph for n.
    let digits_everyday = ["", "一", "二", "三", "四", "五", "六", "七", "八", "九"];
    let digits_formal = ["", "壹", "貳", "參", "肆", "伍", "陸", "柒", "捌", "玖"];
    let digits = if formal {
        &digits_formal
    } else {
        &digits_everyday
    };

    let hundreds = (n / 100) as usize;
    let tens = ((n / 10) % 10) as usize;
    let units = (n % 10) as usize;

    let mut out = String::new();
    if hundreds > 0 {
        out.push_str(digits[hundreds]);
        out.push('百');
    }
    if tens > 0 {
        // The "一十" prefix is suppressed for n in 10..=19 in
        // everyday Chinese (which writes 10 as 十 not 一十), but the
        // formal financial form keeps the 壹 prefix. We follow
        // everyday-Chinese convention for the unformal case.
        if !(tens == 1 && !formal && hundreds == 0) {
            out.push_str(digits[tens]);
        }
        out.push('十');
    } else if hundreds > 0 && units > 0 {
        // "Zero gap" — e.g. 101 = 一百零一. Chinese inserts 零 to
        // signal the missing tens place. Both everyday and formal
        // conventions agree here.
        out.push('零');
    }
    if units > 0 {
        out.push_str(digits[units]);
    }
    out
}

/// Roman numeral conversion. `n` must be ≥ 1; `n == 0` returns
/// an empty string (lists start at 1, so this is a sanity guard).
fn to_roman(mut n: u32, lower: bool) -> String {
    if n == 0 {
        return String::new();
    }
    const MAP: &[(u32, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for &(value, symbol) in MAP {
        while n >= value {
            out.push_str(symbol);
            n -= value;
        }
    }
    if lower {
        out.make_ascii_lowercase();
    }
    out
}

/// Spreadsheet-column-style alpha encoding: 1→A, 2→B, …, 26→Z,
/// 27→AA, 28→AB, …, 702→ZZ, 703→AAA. Lowercase mode shifts to
/// 'a'..'z'.
fn to_alpha(mut n: u32, lower: bool) -> String {
    if n == 0 {
        return String::new();
    }
    let base_char = if lower { b'a' } else { b'A' };
    let mut chars = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        chars.push(base_char + rem);
        n = (n - 1) / 26;
    }
    chars.reverse();
    String::from_utf8(chars).expect("ascii letters are valid utf-8")
}
