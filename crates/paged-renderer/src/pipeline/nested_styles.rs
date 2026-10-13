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

//! Nested styles: where each entry of a paragraph's nested-style list
//! starts and ends, and the run split that layers its character style
//! under the paragraph's runs.
//!
//! # The rules, as InDesign 20.0.1 sets them (asked 2026-10-12)
//!
//! Every rule below was read off InDesign's own per-character formatting
//! (`tools/indesign-export/nested-styles-probe.jsx`) and is pinned by a
//! test in `pipeline/tests.rs`; the `nested-styles` fixture is the pixel
//! oracle.
//!
//! * Entries chain: each starts where the one before it ended. When an
//!   entry's delimiter never comes, it runs to the end of the paragraph
//!   and the entries after it never apply.
//! * "through N" (`Inclusive="true"`) includes the Nth delimiter, "up to
//!   N" (`false`) stops before it. For `AnyCharacter` the Nth character
//!   IS the delimiter: up to 3 characters styles two, up to 1 none.
//! * A word ends at white space — tab, em / en / thin space and a forced
//!   line break included, a no-break space not (it joins two words, as a
//!   hyphen or a dash does). Through N words also takes the whole run of
//!   white space after the Nth; white space before the first word belongs
//!   to it.
//! * A sentence ends at `.`, `!` or `?` (a run of them) plus any closing
//!   quotes or brackets, FOLLOWED by white space: `3.14` and `dot.Next`
//!   end nothing, `Dr. ` does. Through takes the white space after it, up
//!   to stops before it (the full stop and the quote are in).
//! * `Dropcap` covers the paragraph's drop-cap characters: InDesign writes
//!   the drop cap's character style as the first nested style, and the
//!   next entry starts after the cap. Without such an entry the first
//!   nested style counts from the paragraph's first character, cap
//!   included.
//! * `Repeat` (with Repetition N) loops the N entries before it until the
//!   paragraph ends.
//! * A literal delimiter of several characters ends at any ONE of them.
//! * End Nested Style Here ends the range whether through or up to.

use std::borrow::Cow;

/// One nested-style application: the half-open byte range of the
/// paragraph text that the character style is layered under.
/// `applied_character_style` mirrors
/// [`paged_model::NestedStyle::applied_character_style`].
#[derive(Debug, Clone, PartialEq)]
pub struct NestedStyleApplication {
    pub byte_range: std::ops::Range<usize>,
    pub applied_character_style: String,
}

/// Walk a paragraph's text against its nested-style list (see the module
/// doc for the rules), producing the byte range each entry styles.
/// `drop_cap_characters` is the paragraph's drop-cap count, which a
/// `Dropcap` entry covers. Empty when the list is empty or no entry
/// reaches any text.
pub fn compute_nested_style_overlay(
    paragraph_text: &str,
    nested_styles: &[paged_model::NestedStyle],
    drop_cap_characters: u32,
) -> Vec<NestedStyleApplication> {
    use paged_model::NestedDelimiter as D;
    if nested_styles.is_empty() || paragraph_text.is_empty() {
        return Vec::new();
    }
    // A `Repeat` entry closes the list: the entries before it run once,
    // then the last `repetition` of them loop.
    let (head, cycle): (&[paged_model::NestedStyle], &[paged_model::NestedStyle]) =
        match nested_styles
            .iter()
            .position(|ns| ns.delimiter == D::Repeat)
        {
            Some(r) => {
                let n = (nested_styles[r].repetition.max(0) as usize).min(r);
                (&nested_styles[..r], &nested_styles[r - n..r])
            }
            None => (nested_styles, &[]),
        };
    let mut out: Vec<NestedStyleApplication> = Vec::new();
    let mut cursor: usize = 0;
    let mut step = |ns: &paged_model::NestedStyle, cursor: &mut usize| {
        let end = match &ns.delimiter {
            D::Dropcap => advance_chars(paragraph_text, *cursor, drop_cap_characters as usize),
            D::Repeat => *cursor,
            _ if ns.repetition <= 0 => *cursor,
            delimiter => find_nested_end(
                paragraph_text,
                *cursor,
                delimiter,
                ns.repetition,
                ns.inclusive,
            ),
        };
        if end > *cursor {
            out.push(NestedStyleApplication {
                byte_range: *cursor..end,
                applied_character_style: ns.applied_character_style.clone(),
            });
            *cursor = end;
        }
    };
    for ns in head {
        if cursor >= paragraph_text.len() {
            break;
        }
        step(ns, &mut cursor);
    }
    if !cycle.is_empty() {
        while cursor < paragraph_text.len() {
            let before = cursor;
            for ns in cycle {
                if cursor >= paragraph_text.len() {
                    break;
                }
                step(ns, &mut cursor);
            }
            // A loop that styles nothing would never end.
            if cursor == before {
                break;
            }
        }
    }
    out
}

/// White space that ends a word: Unicode white space but the no-break
/// spaces, which join the words either side (InDesign 20.0.1).
pub(super) fn is_word_space(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\u{00A0}' | '\u{202F}')
}

fn is_sentence_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

fn is_sentence_closer(c: char) -> bool {
    matches!(
        c,
        '"' | '\'' | '\u{201D}' | '\u{2019}' | '\u{00BB}' | '\u{203A}' | ')' | ']'
    )
}

/// The byte offset `n` characters past `start`, or the end of `text`.
fn advance_chars(text: &str, start: usize, n: usize) -> usize {
    text[start..]
        .char_indices()
        .nth(n)
        .map_or(text.len(), |(off, _)| start + off)
}

/// The byte offset past the run of word spaces starting at `at`.
fn skip_word_spaces(text: &str, at: usize) -> usize {
    text[at..]
        .char_indices()
        .find(|&(_, c)| !is_word_space(c))
        .map_or(text.len(), |(off, _)| at + off)
}

/// Where a nested-style range starting at `start` ends: `repetition`
/// occurrences of `delimiter` on, by the rules in the module doc.
/// `text.len()` when fewer occur.
pub(super) fn find_nested_end(
    text: &str,
    start: usize,
    delimiter: &paged_model::NestedDelimiter,
    repetition: i32,
    inclusive: bool,
) -> usize {
    use paged_model::NestedDelimiter as D;
    let class = |is_match: &dyn Fn(char) -> bool| {
        find_class_end(text, start, repetition, inclusive, is_match)
    };
    match delimiter {
        D::Characters => {
            // The Nth character is the delimiter itself.
            let n = if inclusive {
                repetition
            } else {
                repetition - 1
            };
            advance_chars(text, start, n.max(0) as usize)
        }
        D::Words => {
            let mut words = 0;
            let mut in_word = false;
            for (off, c) in text[start..].char_indices() {
                if is_word_space(c) {
                    if in_word {
                        words += 1;
                        in_word = false;
                        if words >= repetition {
                            let at = start + off;
                            return if inclusive {
                                skip_word_spaces(text, at)
                            } else {
                                at
                            };
                        }
                    }
                } else {
                    in_word = true;
                }
            }
            text.len()
        }
        D::Sentences => {
            let chars: Vec<(usize, char)> = text[start..].char_indices().collect();
            let mut sentences = 0;
            let mut i = 0;
            while i < chars.len() {
                if !is_sentence_terminator(chars[i].1) {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j < chars.len() && is_sentence_terminator(chars[j].1) {
                    j += 1;
                }
                while j < chars.len() && is_sentence_closer(chars[j].1) {
                    j += 1;
                }
                if j == chars.len() || is_word_space(chars[j].1) {
                    sentences += 1;
                    if sentences >= repetition {
                        let at = chars.get(j).map_or(text.len(), |&(off, _)| start + off);
                        return if inclusive {
                            skip_word_spaces(text, at)
                        } else {
                            at
                        };
                    }
                }
                i = j;
            }
            text.len()
        }
        D::AnyDigit => class(&|c| c.is_ascii_digit()),
        D::AnyLetter => class(&|c| c.is_alphabetic()),
        D::AnyDoubleQuotes => class(&|c| matches!(c, '"' | '\u{201C}' | '\u{201D}')),
        D::AnySingleQuotes => class(&|c| matches!(c, '\'' | '\u{2018}' | '\u{2019}')),
        D::Tab => class(&|c| c == '\t'),
        // `<Br/>` reaches the run text as `\n` in older documents; a
        // forced line break is U+2028.
        D::ForcedLineBreak => class(&|c| c == '\n' || c == '\u{2028}'),
        // The marker is invisible, and InDesign styles it with the range
        // either way.
        D::EndNestedStyle => find_class_end(text, start, repetition, true, &|c| c == '\u{0003}'),
        D::Char(target) => class(&|c| c == *target),
        D::AnyOf(set) => class(&|c| set.contains(c)),
        D::EmSpace => class(&|c| c == '\u{2003}'),
        D::EnSpace => class(&|c| c == '\u{2002}'),
        D::NonbreakingSpace => class(&|c| matches!(c, '\u{00A0}' | '\u{202F}')),
        D::IndentHereTab => class(&|c| c == '\u{0007}'),
        D::AutoPageNumber => class(&|c| {
            matches!(
                c,
                paged_model::AUTO_PAGE_NUMBER_MARKER
                    | paged_model::NEXT_PAGE_NUMBER_MARKER
                    | paged_model::PREVIOUS_PAGE_NUMBER_MARKER
            )
        }),
        D::SectionMarker => class(&|c| c == paged_model::SECTION_MARKER),
        // An inline object composes as one placeholder character at its
        // anchor (`anchored::with_object_placeholders`).
        D::InlineGraphic => class(&|c| c == paged_text::layout::OBJECT_REPLACEMENT),
        D::Dropcap | D::Repeat | D::Unknown => start,
    }
    .min(text.len())
}

pub(super) fn find_class_end(
    text: &str,
    start: usize,
    repetition: i32,
    inclusive: bool,
    is_match: &dyn Fn(char) -> bool,
) -> usize {
    let mut matches = 0;
    for (off, c) in text[start..].char_indices() {
        if is_match(c) {
            matches += 1;
            if matches >= repetition {
                let abs = start + off;
                return if inclusive { abs + c.len_utf8() } else { abs };
            }
        }
    }
    text.len()
}

/// Layer a nested-style overlay under a paragraph's character runs.
/// Each run that overlaps a range is split at its boundaries, and the
/// fragments inside carry the range's style as
/// [`paged_model::CharacterRun::nested_character_style`] — UNDER the
/// run's own character style, which keeps every attribute it defines
/// (see `paged_scene::Document::resolved_run_attrs`). Runs no range
/// touches pass through unchanged, in order.
pub fn split_runs_for_nested_styles(
    runs: &[paged_model::CharacterRun],
    overlay: &[NestedStyleApplication],
) -> Vec<paged_model::CharacterRun> {
    if overlay.is_empty() {
        return runs.to_vec();
    }
    let mut out: Vec<paged_model::CharacterRun> = Vec::with_capacity(runs.len());
    let mut cursor: usize = 0; // paragraph-byte position of the next run.
    for run in runs {
        let run_start = cursor;
        let run_end = cursor + run.text.len();
        cursor = run_end;
        let mut boundaries: Vec<usize> = vec![run_start, run_end];
        for ov in overlay {
            if ov.byte_range.start > run_start && ov.byte_range.start < run_end {
                boundaries.push(ov.byte_range.start);
            }
            if ov.byte_range.end > run_start && ov.byte_range.end < run_end {
                boundaries.push(ov.byte_range.end);
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        for window in boundaries.windows(2) {
            let (frag_start, frag_end) = (window[0], window[1]);
            if frag_start >= frag_end {
                continue;
            }
            let nested = overlay
                .iter()
                .find(|ov| frag_start >= ov.byte_range.start && frag_start < ov.byte_range.end)
                .map(|ov| ov.applied_character_style.clone());
            let mut frag = run.clone();
            frag.text = run.text[frag_start - run_start..frag_end - run_start].to_string();
            if nested.is_some() {
                frag.nested_character_style = nested;
            }
            out.push(frag);
        }
    }
    out
}

/// The paragraph's drop cap as InDesign sets it: the range's own
/// `DropCapCharacters` / `DropCapLines`, else its paragraph style's
/// (InDesign's own documents define drop caps on the style). `(0, 0)`
/// when there is none.
pub fn effective_drop_cap(
    paragraph: &paged_model::Paragraph,
    resolved: &paged_scene::ResolvedParagraphAttrs,
) -> (u32, u32) {
    if paragraph.drop_cap_characters > 0 && paragraph.drop_cap_lines > 0 {
        return (paragraph.drop_cap_characters, paragraph.drop_cap_lines);
    }
    match (resolved.drop_cap_characters, resolved.drop_cap_lines) {
        (Some(c), Some(l)) if c > 0 && l > 0 => (c, l),
        _ => (0, 0),
    }
}

/// `paragraph` with its nested styles layered in (see
/// [`split_runs_for_nested_styles`]) and its style's drop cap made its
/// own, so everything downstream reads one paragraph. Borrowed unchanged
/// when neither applies.
pub fn with_nested_styles<'p>(
    document: &paged_scene::Document,
    paragraph: &'p paged_model::Paragraph,
) -> Cow<'p, paged_model::Paragraph> {
    let resolved = document.resolved_paragraph_attrs(paragraph);
    let (cap_chars, cap_lines) = effective_drop_cap(paragraph, &resolved);
    let cap_from_style = cap_chars > 0 && paragraph.drop_cap_characters == 0;
    let overlay = if resolved.nested_styles.is_empty() {
        Vec::new()
    } else {
        let text: String = paragraph.runs.iter().map(|r| r.text.as_str()).collect();
        compute_nested_style_overlay(&text, &resolved.nested_styles, cap_chars)
    };
    if overlay.is_empty() && !cap_from_style {
        return Cow::Borrowed(paragraph);
    }
    let mut owned = paragraph.clone();
    if !overlay.is_empty() {
        owned.runs = split_runs_for_nested_styles(&paragraph.runs, &overlay);
    }
    if cap_from_style {
        owned.drop_cap_characters = cap_chars;
        owned.drop_cap_lines = cap_lines;
    }
    Cow::Owned(owned)
}
