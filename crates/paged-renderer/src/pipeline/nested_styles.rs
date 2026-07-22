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

//! Nested-style overlay computation + run splitting — extracted from pipeline/mod.rs (1.6b).

/// Map an IDML `Justification` enum value to `paged_text::Alignment`.
/// `None` (no attribute on the cascade) falls back to `Left`, the
/// IDML default.
///
/// `ToBindingSide` / `AwayFromBindingSide` are binding-aware values
/// that ideally consult the spread's page side (left vs. right). We
/// don't plumb binding side through to the composer today, so they
/// resolve to `Left` / `Right` respectively — matches the historical
/// stringly-typed behaviour, which fell through to `Left` for any
/// unrecognised string.
/// Phase 4 typography — one nested-style application: the half-open
/// byte range of the paragraph text that the override character style
/// should apply to. `byte_range.start` is inclusive; `byte_range.end`
/// is exclusive. `applied_character_style` mirrors
/// [`paged_model::NestedStyle::applied_character_style`].
#[derive(Debug, Clone, PartialEq)]
pub struct NestedStyleApplication {
    pub byte_range: std::ops::Range<usize>,
    pub applied_character_style: String,
}

/// Phase 4 typography — walk a paragraph's text against its cascaded
/// `<NestedStyle>` list, producing the half-open byte ranges each
/// override should apply to. The first entry's range starts at byte 0;
/// each subsequent entry starts where the previous one ended. Returns
/// an empty vec when `nested_styles` is empty or when every entry has
/// an unsupported delimiter / zero repetition.
///
/// The walker handles every `NestedDelimiter` variant. Single-paragraph
/// scope: the walker stops when the cursor reaches the end of
/// `paragraph_text`, even if some entries are unconsumed (their range
/// would extend past the text). This matches InDesign's behaviour for
/// short paragraphs.
pub fn compute_nested_style_overlay(
    paragraph_text: &str,
    nested_styles: &[paged_model::NestedStyle],
) -> Vec<NestedStyleApplication> {
    if nested_styles.is_empty() || paragraph_text.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<NestedStyleApplication> = Vec::new();
    let mut cursor: usize = 0;
    for ns in nested_styles {
        if cursor >= paragraph_text.len() {
            break;
        }
        if ns.repetition <= 0 {
            continue;
        }
        let end = find_nested_end(
            paragraph_text,
            cursor,
            &ns.delimiter,
            ns.repetition,
            ns.inclusive,
        );
        if end > cursor {
            out.push(NestedStyleApplication {
                byte_range: cursor..end,
                applied_character_style: ns.applied_character_style.clone(),
            });
            cursor = end;
        }
    }
    out
}

/// Internal — locate the byte offset where a nested-style range ends,
/// scanning `text[start..]` for `repetition` occurrences of
/// `delimiter`. Returns `text.len()` when fewer than `repetition`
/// matches are found (the range stretches to the paragraph's end).
pub(super) fn find_nested_end(
    text: &str,
    start: usize,
    delimiter: &paged_model::NestedDelimiter,
    repetition: i32,
    inclusive: bool,
) -> usize {
    use paged_model::NestedDelimiter as D;
    let bytes = text.as_bytes();
    let slice = &text[start..];
    // For Words / Sentences / Characters the count is the number of
    // logical units to traverse, INCLUDING the trailing boundary.
    // For class matchers (AnyDigit / AnyLetter / quote pairs / Char /
    // Tab) it's the count of matches.
    match delimiter {
        D::Characters => {
            // Walk `repetition` Unicode scalar values (≠ bytes).
            let mut indices = slice.char_indices();
            for _ in 0..repetition {
                if indices.next().is_none() {
                    return text.len();
                }
            }
            // `indices.offset()` is unstable; reconstruct via next().
            match indices.next() {
                Some((off, _)) => start + off,
                None => text.len(),
            }
        }
        D::Words => {
            // Walk `repetition` words. A word is a maximal run of
            // non-whitespace chars; the boundary after a word is its
            // trailing whitespace.
            let mut idx = 0usize;
            let mut words_seen = 0;
            let mut in_word = false;
            // Skip leading whitespace so word 1 starts at the first
            // non-space char (matches InDesign).
            while idx < slice.len() && slice.as_bytes()[idx].is_ascii_whitespace() {
                idx += 1;
            }
            while idx < slice.len() {
                let b = slice.as_bytes()[idx];
                if b.is_ascii_whitespace() {
                    if in_word {
                        words_seen += 1;
                        in_word = false;
                        if words_seen >= repetition {
                            // Boundary candidate is the trailing space.
                            if inclusive {
                                // Consume whitespace run; range ends
                                // after the last whitespace byte.
                                while idx < slice.len()
                                    && slice.as_bytes()[idx].is_ascii_whitespace()
                                {
                                    idx += 1;
                                }
                            }
                            return start + idx;
                        }
                    }
                } else {
                    in_word = true;
                }
                idx += 1;
            }
            // Reaching here means fewer than `repetition` word boundaries
            // were found before the slice ended (the in-loop check at the
            // `repetition`th word returns early), so the boundary is the
            // end of the text — whether or not it ended mid-word.
            text.len()
        }
        D::Sentences => {
            // A sentence boundary is `.`, `!`, or `?` followed by
            // optional whitespace.
            let mut idx = 0usize;
            let mut sentences_seen = 0;
            while idx < slice.len() {
                let b = slice.as_bytes()[idx];
                if matches!(b, b'.' | b'!' | b'?') {
                    sentences_seen += 1;
                    if sentences_seen >= repetition {
                        if inclusive {
                            idx += 1;
                            // Consume the trailing whitespace run too.
                            while idx < slice.len() && slice.as_bytes()[idx].is_ascii_whitespace() {
                                idx += 1;
                            }
                        }
                        return start + idx;
                    }
                }
                idx += 1;
            }
            text.len()
        }
        D::AnyDigit => find_class_end(text, start, repetition, inclusive, |c| c.is_ascii_digit()),
        D::AnyLetter => find_class_end(text, start, repetition, inclusive, |c| c.is_alphabetic()),
        D::AnyDoubleQuotes => find_class_end(text, start, repetition, inclusive, |c| {
            matches!(c, '"' | '\u{201C}' | '\u{201D}')
        }),
        D::AnySingleQuotes => find_class_end(text, start, repetition, inclusive, |c| {
            matches!(c, '\'' | '\u{2018}' | '\u{2019}')
        }),
        D::Tab => find_class_end(text, start, repetition, inclusive, |c| c == '\t'),
        D::ForcedLineBreak => find_class_end(text, start, repetition, inclusive, |c| {
            // U+2028 LINE SEPARATOR; IDML serialises forced line
            // breaks as `<Br/>` which the parser materialises as `\n`
            // in run text.
            c == '\n' || c == '\u{2028}'
        }),
        D::EndNestedStyle => {
            // U+0003 END OF TEXT — InDesign's "End Nested Style Here"
            // marker. Inserted by the user via a special character.
            find_class_end(text, start, repetition, inclusive, |c| c == '\u{0003}')
        }
        D::Char(target) => find_class_end(text, start, repetition, inclusive, |c| c == *target),
        D::Unknown => start,
    }
    .min(bytes.len())
}

pub(super) fn find_class_end<F: Fn(char) -> bool>(
    text: &str,
    start: usize,
    repetition: i32,
    inclusive: bool,
    is_match: F,
) -> usize {
    let mut matches = 0;
    for (off, c) in text[start..].char_indices() {
        if is_match(c) {
            matches += 1;
            if matches >= repetition {
                let abs = start + off;
                let end = if inclusive { abs + c.len_utf8() } else { abs };
                return end;
            }
        }
    }
    text.len()
}

/// Phase 4 typography — apply a nested-style overlay to a paragraph's
/// character runs. Returns a new run vec where each run that overlaps
/// a `<NestedStyle>` range has been split so its `character_style`
/// field carries the override id. Runs that don't touch any overlay
/// range pass through unchanged.
///
/// Empty overlay → returns `runs.to_vec()`. The walker preserves run
/// ordering: any run produced by splitting one source run appears in
/// the same paragraph-byte-order position. All non-text fields on a
/// split run are cloned from the source run — only the override
/// `character_style` differs.
pub fn split_runs_for_nested_styles(
    runs: &[paged_model::CharacterRun],
    overlay: &[NestedStyleApplication],
) -> Vec<paged_model::CharacterRun> {
    if overlay.is_empty() {
        return runs.to_vec();
    }
    // Build a per-byte map of "what character style overrides this
    // position?" Sparse: only the bytes covered by some overlay range
    // are touched. We build it as a sorted Vec of (range, style) and
    // do binary search per-run-byte during splitting.
    let mut out: Vec<paged_model::CharacterRun> = Vec::with_capacity(runs.len());
    let mut cursor: usize = 0; // paragraph-byte position of the next run.
    for run in runs {
        let run_start = cursor;
        let run_end = cursor + run.text.len();
        cursor = run_end;
        // Compute the set of overlay-defined boundaries inside this
        // run, plus the run's own start and end. Then walk the sorted
        // boundaries and emit a fragment per (start, end) pair.
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
            let frag_start = window[0];
            let frag_end = window[1];
            if frag_start >= frag_end {
                continue;
            }
            // Find an overlay whose range covers frag_start (any
            // byte inside the fragment maps to the same override
            // because we split at every overlay boundary).
            let override_style = overlay
                .iter()
                .find(|ov| frag_start >= ov.byte_range.start && frag_start < ov.byte_range.end)
                .map(|ov| ov.applied_character_style.clone());
            let local_lo = frag_start - run_start;
            let local_hi = frag_end - run_start;
            let mut frag = run.clone();
            frag.text = run.text[local_lo..local_hi].to_string();
            if let Some(s) = override_style {
                frag.character_style = Some(s);
            }
            out.push(frag);
        }
    }
    out
}
