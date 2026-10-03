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

//! W1.4 / W1.18 — text-variable resolution + hyperlink/cross-reference
//! target resolution at render time.
//!
//! ## Text variables
//!
//! A `<TextVariableInstance>` in a story carries InDesign's baked
//! `ResultText` (the value the document last composed) plus an
//! `AssociatedTextVariable` id. The parser splits each instance into
//! its own [`paged_model::CharacterRun`] tagged with `text_variable`.
//! At paragraph emit, [`resolve_variable`] re-resolves the value per
//! the variable's `VariableType` when it can do better than the stale
//! baked string — mirroring how the auto-page-number marker is
//! substituted in `pipeline::mod`.
//!
//! Per-type semantics, as InDesign 20.0.1 resolves them (asked by
//! building the constructs through its DOM and reading its PDF export;
//! ADR 033, and the `variables` paged-gen fixture):
//!
//! | VariableType        | Resolution                                    |
//! |---------------------|-----------------------------------------------|
//! | `CustomTextType`    | `TextBefore` + `Contents` + `TextAfter` (literal, from the IDML) |
//! | `MatchParagraphStyleType` (aliases `RunningHeaderType`, `RunningHeaderVariableType`) | the text of the first / last paragraph in the style on the page; a page without one carries forward (see [`RunningHeaderIndex`]); then `DeleteEndPunctuation`, `ChangeCase` |
//! | `MatchCharacterStyleType` | the same over contiguous ranges in a character style |
//! | `LastPageNumberType` | the LABEL of the last page of the document (`DocumentScope`) or of the page's section (`SectionScope`): "3" on a five-page document whose numbering restarts — not a count; `Format` other than `Current` re-styles that page's number |
//! | `PageCountType`     | real total page count (the engine's own older name; InDesign does not write it) |
//! | `ChapterNumberType` | the DOCUMENT's chapter number (`<ChapterNumberPreference>`, default 1) — not a section property: InDesign printed "1" on pages whose sections had markers and restarts |
//! | `FileNameType`      | document `Name`, without its extension unless `IncludeExtension` |
//! | `CreationDateType` / `ModificationDateType` / `OutputDateType` | `Format` tokens applied to the document clock |
//! | (anything else)     | baked `ResultText`                            |
//!
//! Date variables are computed from the deterministic
//! [`crate::pipeline::DocumentClock`] — the `output` instant is an
//! explicit render-options field, never the wall clock, so two renders
//! of the same model are byte-identical.
//!
//! Running headers can only be resolved once the body text is seated on
//! pages (the matching paragraph may live on the same page as the
//! header). The build runs a first layout pass, indexes the per-page
//! style→text occurrences ([`RunningHeaderIndex`]), then re-emits the
//! frames that carry running-header (or page-number xref) variables
//! with that index in hand. See `pipeline::mod`'s post-layout pass.
//!
//! ## Hyperlinks / cross-references
//!
//! A run tagged with `hyperlink_source` came from a
//! `<HyperlinkTextSource>` / `<CrossReferenceSource>` span. The
//! designmap's `<Hyperlink Source=... Destination=...>` maps the source
//! id to a destination resource ([`resolve_link_target`]); page
//! destinations resolve to a flat 0-based body-page index. A
//! cross-reference whose destination is a story / text anchor resolves
//! to the page that story landed on AFTER layout (same post-layout
//! phase as the running header — both read a page index that only
//! exists once text is seated).

use std::collections::HashMap;

use paged_compose::LinkTarget;
use paged_model::{DesignMap, HyperlinkDestinationKind, NumberingStyle};

use super::datefmt::{self, DateParts};
use super::DocumentClock;

/// W1.18c — per-page running-header pickup index, built after the first
/// layout pass. Keys are `(flat page index, style id)`; a style id is a
/// `ParagraphStyle/…` or a `CharacterStyle/…`, so both kinds share the
/// maps without colliding.
///
/// `first` / `last` hold the value a `FirstOnPage` / `LastOnPage`
/// variable shows on that page — already carried forward for a page with
/// no match of its own. InDesign's carry-forward, measured (InDesign
/// 20.0.1, 2026-10-01): a page without a match takes the last match that
/// PRECEDES the page's text in its own story; when its story has none
/// (each page its own story), it shows what the previous page showed, per
/// strategy. So two headings on page 1 and none on page 2 read
/// first/last = H-B/H-B on page 2 when one story threads both pages, and
/// H-A/H-B when the pages hold separate stories.
#[derive(Debug, Default, Clone)]
pub(crate) struct RunningHeaderIndex {
    pub first: HashMap<(usize, String), String>,
    pub last: HashMap<(usize, String), String>,
}

impl RunningHeaderIndex {
    /// The running-header text for `style_id` on `page_idx`, honouring
    /// `use_last` (LastOnPage vs FirstOnPage).
    pub fn resolve(&self, page_idx: usize, style_id: &str, use_last: bool) -> Option<String> {
        let key = (page_idx, style_id.to_string());
        if use_last {
            self.last.get(&key)
        } else {
            self.first.get(&key)
        }
        .cloned()
    }
}

/// Per-page numbering facts the page-dependent variables and markers
/// read, computed once per build from the section walk.
#[derive(Debug, Default, Clone)]
pub(crate) struct PageNumbering {
    /// The number the section rules give each page (before its numbering
    /// style and prefix). Parallel to the page labels.
    pub numbers: Vec<u32>,
    /// Index into `designmap.sections` of each page's section.
    pub section_of: Vec<Option<usize>>,
}

impl PageNumbering {
    /// A shared empty table, for emitters built without one.
    pub(crate) fn empty() -> &'static PageNumbering {
        static EMPTY: std::sync::OnceLock<PageNumbering> = std::sync::OnceLock::new();
        EMPTY.get_or_init(PageNumbering::default)
    }

    /// Flat index of the last page of `page_idx`'s section (pages before
    /// any section, or a document without sections, form one implicit
    /// section).
    fn section_last_page(&self, page_idx: usize) -> usize {
        let section = self.section_of.get(page_idx).copied().flatten();
        let mut last = page_idx;
        while self.section_of.get(last + 1).is_some_and(|s| *s == section) {
            last += 1;
        }
        last
    }
}

/// W1.18 — render-time resolution context threaded into
/// [`resolve_variable`]. Carries everything a variable needs beyond the
/// designmap + its baked text: the deterministic date clock, the page
/// labels and numbering, the host page index, and (post-layout) the
/// running-header pickup index.
pub(crate) struct VarResolveCtx<'a> {
    pub designmap: &'a DesignMap,
    pub total_pages: usize,
    /// Deterministic date clock (creation / modification / output).
    pub clock: &'a DocumentClock,
    /// Every page's label, by flat index.
    pub page_labels: &'a [String],
    /// Every page's number and section, by flat index.
    pub numbering: &'a PageNumbering,
    /// Flat 0-based page index of the frame currently emitting — the
    /// page a running header resolves *for*.
    pub page_index: usize,
    /// Post-layout running-header pickup index. `None` on the first
    /// (pre-layout) pass; populated for the re-emit so running headers
    /// resolve to live content.
    pub running_headers: Option<&'a RunningHeaderIndex>,
}

/// True for the variable types whose value is another paragraph's or
/// range's text on the page (they need the post-layout pass).
pub(crate) fn is_running_header_type(variable_type: Option<&str>) -> bool {
    matches!(
        variable_type,
        Some(
            "MatchParagraphStyleType"
                | "MatchCharacterStyleType"
                | "RunningHeaderType"
                | "RunningHeaderVariableType"
        )
    )
}

/// The style a running-header variable matches: the character style of
/// a `MatchCharacterStyleType`, the paragraph style otherwise.
pub(crate) fn running_header_style(var: &paged_model::TextVariable) -> Option<&str> {
    if var.variable_type.as_deref() == Some("MatchCharacterStyleType") {
        var.running_header_character_style
            .as_deref()
            .or(var.running_header_style.as_deref())
    } else {
        var.running_header_style.as_deref()
    }
}

/// Resolve a tagged variable run to its render-time value, or `None`
/// to keep the run's baked `ResultText`.
///
/// `variable_id` is the run's `text_variable` (`TextVariable/<id>`).
/// `result_text` is the run's current text (the baked value).
pub(crate) fn resolve_variable(
    ctx: &VarResolveCtx,
    variable_id: &str,
    result_text: &str,
) -> Option<String> {
    let var = ctx
        .designmap
        .text_variables
        .iter()
        .find(|v| v.self_id == variable_id)?;
    let kind = var.variable_type.as_deref().unwrap_or("");
    let decorate = |core: String| -> String {
        let before = var.text_before.as_deref().unwrap_or("");
        let after = var.text_after.as_deref().unwrap_or("");
        format!("{before}{core}{after}")
    };
    match kind {
        "CustomTextType" => {
            // The literal custom string lives in the IDML — fully
            // honest. Empty contents still decorate (matches InDesign,
            // which lets a custom variable be pure before/after text).
            let contents = var.contents.clone().unwrap_or_default();
            Some(decorate(contents))
        }
        "PageCountType" => Some(decorate(ctx.total_pages.to_string())),
        "LastPageNumberType" => {
            let last = if var.page_number_scope.as_deref() == Some("SectionScope") {
                ctx.numbering.section_last_page(ctx.page_index)
            } else {
                ctx.page_labels.len().saturating_sub(1)
            };
            let label = match explicit_number_style(var.number_format.as_deref()) {
                Some(style) => ctx.numbering.numbers.get(last).map(|n| style.format(*n)),
                None => ctx.page_labels.get(last).cloned(),
            };
            Some(decorate(
                label.unwrap_or_else(|| ctx.total_pages.to_string()),
            ))
        }
        "FileNameType" => {
            let name = ctx
                .designmap
                .document_name
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    Some(result_text)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "untitled.indd".to_string());
            let name = if var.include_extension {
                name
            } else {
                match name.rsplit_once('.') {
                    Some((stem, _)) if !stem.is_empty() => stem.to_string(),
                    _ => name,
                }
            };
            Some(decorate(name))
        }
        // W1.18a — date variables: apply the declared format tokens to
        // the deterministic clock field for this type. The clock is
        // injected (never `now()`), so the output is reproducible.
        "CreationDateType" => Some(decorate(format_date_var(var, ctx.clock.creation))),
        "ModificationDateType" => Some(decorate(format_date_var(var, ctx.clock.modification))),
        "OutputDateType" => Some(decorate(format_date_var(var, ctx.clock.output))),
        "ChapterNumberType" => {
            let pref = ctx.designmap.chapter_number.as_ref();
            let number = pref.and_then(|p| p.number).unwrap_or(1);
            let style = explicit_number_style(var.number_format.as_deref())
                .or_else(|| {
                    pref.and_then(|p| p.format.as_deref())
                        .map(chapter_format_style)
                })
                .unwrap_or(NumberingStyle::Arabic);
            Some(decorate(style.format(number)))
        }
        // W1.18c — running header. Post-layout, the index carries the
        // matching text per page; pre-layout (index None) we keep the
        // baked value so the first pass still renders something.
        _ if is_running_header_type(Some(kind)) => {
            let Some(index) = ctx.running_headers else {
                // First pass: keep baked ResultText (or a placeholder if
                // even that is empty) so layout is stable.
                return if result_text.is_empty() {
                    Some(decorate("—".to_string()))
                } else {
                    None
                };
            };
            let use_last = var
                .running_header_use
                .as_deref()
                .map(running_header_use_last)
                .unwrap_or(false);
            let resolved = running_header_style(var)
                .and_then(|style_id| index.resolve(ctx.page_index, style_id, use_last));
            // No match on this page and nothing to carry forward: InDesign
            // prints nothing.
            let text = resolved.unwrap_or_default();
            let text = if var.delete_end_punctuation {
                delete_end_punctuation(&text)
            } else {
                text
            };
            Some(decorate(change_case(&text, var.change_case.as_deref())))
        }
        _ => None,
    }
}

/// A `Format` naming an explicit numbering style; `None` for `Current`
/// (and absent), which keeps the page's / chapter's own numbering.
fn explicit_number_style(format: Option<&str>) -> Option<NumberingStyle> {
    match format {
        None | Some("Current") | Some("") => None,
        Some(f) => Some(NumberingStyle::from_idml(f)),
    }
}

/// `<ChapterNumberFormat>` spells its style by example (`1, 2, 3, 4...`,
/// `I, II, III, IV...`, `a, b, c, d...`).
fn chapter_format_style(format: &str) -> NumberingStyle {
    match format.trim_start().chars().next() {
        Some('I') => NumberingStyle::UpperRoman,
        Some('i') => NumberingStyle::LowerRoman,
        Some('A') => NumberingStyle::UpperAlpha,
        Some('a') => NumberingStyle::LowerAlpha,
        _ => NumberingStyle::Arabic,
    }
}

/// `DeleteEndPunctuation`: drop the punctuation that ends the text.
/// Measured: a trailing `.` and `!` go, a closing `)` stays.
fn delete_end_punctuation(text: &str) -> String {
    text.trim_end_matches(['.', ',', ';', ':', '!', '?', '…'])
        .to_string()
}

/// `ChangeCase` as InDesign applies it to a running header (measured,
/// InDesign 20.0.1): `Titlecase` capitalises the first character of every
/// space-separated word and lowercases the rest ("PART TWO begins (a third
/// heading)" → "Part Two Begins (a Third Heading)" — a word opening with
/// `(` keeps its letter); `Sentencecase` lowercases everything and
/// capitalises the first letter of each sentence ("Introduction: the First
/// Heading." → "Introduction: the first heading.").
fn change_case(text: &str, case: Option<&str>) -> String {
    match case {
        Some("Uppercase") => text.to_uppercase(),
        Some("Lowercase") => text.to_lowercase(),
        Some("Titlecase") => {
            let mut out = String::with_capacity(text.len());
            let mut word_start = true;
            for ch in text.chars() {
                if ch.is_whitespace() {
                    word_start = true;
                    out.push(ch);
                } else if word_start {
                    out.extend(ch.to_uppercase());
                    word_start = false;
                } else {
                    out.extend(ch.to_lowercase());
                }
            }
            out
        }
        Some("Sentencecase") => {
            let mut out = String::with_capacity(text.len());
            let mut sentence_start = true;
            for ch in text.chars() {
                if sentence_start && ch.is_alphabetic() {
                    out.extend(ch.to_uppercase());
                    sentence_start = false;
                } else {
                    out.extend(ch.to_lowercase());
                    if matches!(ch, '.' | '!' | '?') {
                        sentence_start = true;
                    }
                }
            }
            out
        }
        _ => text.to_string(),
    }
}

/// W1.18a — format a date variable: apply its `date_format` token
/// pattern to `date`. An absent / empty pattern uses a documented
/// ISO-ish default so the slot is self-describing rather than blank.
fn format_date_var(var: &paged_model::TextVariable, date: DateParts) -> String {
    let pattern = var
        .date_format
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("MM/dd/yyyy");
    datefmt::format_date(pattern, date)
}

/// `SearchStrategy` (the older fixtures' `Use`): whether the LAST on-page
/// match is wanted (`LastOnPage`) rather than the first.
fn running_header_use_last(use_value: &str) -> bool {
    matches!(use_value, "LastOnPage" | "lastOnPage")
}

/// Resolve a hyperlink/cross-reference *source* span id to a concrete
/// [`LinkTarget`]. `page_index_of` maps a target `<Page Self=...>` id
/// (or a story/text-anchor id) to a flat 0-based body-page index.
///
/// Returns `LinkTarget::Unresolved` (carrying the dangling id) when the
/// source has no matching `<Hyperlink>`, no destination, or the
/// destination's page can't be located — so tooling can still see that
/// a link existed.
pub(crate) fn resolve_link_target(
    designmap: &DesignMap,
    source_id: &str,
    mut page_index_of: impl FnMut(&str) -> Option<u32>,
) -> LinkTarget {
    let Some(hyperlink) = designmap
        .hyperlinks
        .iter()
        .find(|h| h.source.as_deref() == Some(source_id))
    else {
        return LinkTarget::Unresolved(source_id.to_string());
    };
    let Some(dest_id) = hyperlink.destination.as_deref() else {
        return LinkTarget::Unresolved(source_id.to_string());
    };
    let Some(dest) = designmap
        .hyperlink_destinations
        .iter()
        .find(|d| d.self_id == dest_id)
    else {
        return LinkTarget::Unresolved(dest_id.to_string());
    };
    match &dest.kind {
        HyperlinkDestinationKind::Url(url) if !url.is_empty() => LinkTarget::Url(url.clone()),
        HyperlinkDestinationKind::Url(_) => LinkTarget::Unresolved(dest_id.to_string()),
        HyperlinkDestinationKind::Page(page_id) => page_index_of(page_id)
            .map(LinkTarget::PageIndex)
            .unwrap_or_else(|| LinkTarget::Unresolved(page_id.clone())),
        HyperlinkDestinationKind::TextAnchor(text_id) => page_index_of(text_id)
            .map(LinkTarget::PageIndex)
            .unwrap_or_else(|| LinkTarget::Unresolved(text_id.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_model::{Hyperlink, HyperlinkDestination, TextVariable};

    fn designmap_with(vars: Vec<TextVariable>) -> DesignMap {
        DesignMap {
            text_variables: vars,
            document_name: Some("brochure.indd".to_string()),
            ..DesignMap::default()
        }
    }

    fn var(id: &str, ty: &str) -> TextVariable {
        TextVariable {
            self_id: id.to_string(),
            variable_type: Some(ty.to_string()),
            ..TextVariable::default()
        }
    }

    // A deterministic clock for the date-variable tests:
    // creation 2020-01-02, modification 2024-03-09, output 2026-12-31.
    fn test_clock() -> DocumentClock {
        DocumentClock {
            creation: DateParts {
                year: 2020,
                month: 1,
                day: 2,
                hour: 0,
                minute: 0,
                second: 0,
            },
            modification: DateParts {
                year: 2024,
                month: 3,
                day: 9,
                hour: 13,
                minute: 30,
                second: 0,
            },
            output: DateParts {
                year: 2026,
                month: 12,
                day: 31,
                hour: 9,
                minute: 5,
                second: 0,
            },
        }
    }

    /// The five-page document of ADR 033: a section restarting
    /// at 1 on page 3, so the labels read 1 2 1 2 3.
    fn restart_numbering() -> (Vec<String>, PageNumbering) {
        let labels = ["1", "2", "1", "2", "3"].map(String::from).to_vec();
        let numbering = PageNumbering {
            numbers: vec![1, 2, 1, 2, 3],
            section_of: vec![Some(0), Some(0), Some(1), Some(1), Some(1)],
        };
        (labels, numbering)
    }

    /// Build a resolution context over `dm` with the given total page
    /// count. No labels, no running-header context.
    fn ctx<'a>(
        dm: &'a DesignMap,
        clock: &'a DocumentClock,
        total_pages: usize,
    ) -> VarResolveCtx<'a> {
        static NO_NUMBERING: std::sync::OnceLock<PageNumbering> = std::sync::OnceLock::new();
        VarResolveCtx {
            designmap: dm,
            total_pages,
            clock,
            page_labels: &[],
            numbering: NO_NUMBERING.get_or_init(PageNumbering::default),
            page_index: 0,
            running_headers: None,
        }
    }

    #[test]
    fn custom_text_resolves_to_contents_with_decoration() {
        let mut v = var("TextVariable/u1", "CustomTextType");
        v.contents = Some("Spring".to_string());
        v.text_before = Some("[".to_string());
        v.text_after = Some("]".to_string());
        let dm = designmap_with(vec![v]);
        let clock = DocumentClock::default();
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 7), "TextVariable/u1", "stale"),
            Some("[Spring]".to_string())
        );
        // RFI C-39: an instance with an empty stored result resolves too.
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 7), "TextVariable/u1", ""),
            Some("[Spring]".to_string())
        );
    }

    #[test]
    fn page_count_resolves_to_real_total() {
        let dm = designmap_with(vec![var("TextVariable/u2", "PageCountType")]);
        let clock = DocumentClock::default();
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 12), "TextVariable/u2", "1"),
            Some("12".to_string())
        );
    }

    /// Measured (ADR 033): the last page number is the last page's
    /// LABEL — 3 on a five-page document whose numbering restarts — and in
    /// section scope the last label of the page's own section.
    #[test]
    fn last_page_number_is_a_label_in_document_or_section_scope() {
        let doc = var("TextVariable/doc", "LastPageNumberType");
        let mut sec = var("TextVariable/sec", "LastPageNumberType");
        sec.page_number_scope = Some("SectionScope".to_string());
        let mut roman = var("TextVariable/roman", "LastPageNumberType");
        roman.number_format = Some("UpperRoman".to_string());
        let dm = designmap_with(vec![doc, sec, roman]);
        let clock = DocumentClock::default();
        let (labels, numbering) = restart_numbering();
        let at = |page: usize, id: &str| {
            let c = VarResolveCtx {
                page_labels: &labels,
                numbering: &numbering,
                page_index: page,
                ..ctx(&dm, &clock, 5)
            };
            resolve_variable(&c, id, "").unwrap()
        };
        let per_page = |id: &str| (0..5).map(|p| at(p, id)).collect::<Vec<_>>();
        assert_eq!(per_page("TextVariable/doc"), ["3", "3", "3", "3", "3"]);
        assert_eq!(per_page("TextVariable/sec"), ["2", "2", "3", "3", "3"]);
        assert_eq!(per_page("TextVariable/roman"), ["III"; 5]);
    }

    #[test]
    fn file_name_drops_the_extension_unless_asked() {
        let mut with_ext = var("TextVariable/u3", "FileNameType");
        with_ext.include_extension = true;
        let without = var("TextVariable/u4", "FileNameType");
        let dm = designmap_with(vec![with_ext, without]);
        let clock = DocumentClock::default();
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/u3", "old.indd"),
            Some("brochure.indd".to_string())
        );
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/u4", "old.indd"),
            Some("brochure".to_string())
        );
    }

    #[test]
    fn dates_format_from_clock_not_baked_value() {
        // W1.18a — each date type reads its own clock field and applies
        // the declared format tokens, ignoring the stale baked string.
        let mut cre = var("TextVariable/uc", "CreationDateType");
        cre.date_format = Some("yyyy-MM-dd".to_string());
        let mut modi = var("TextVariable/um", "ModificationDateType");
        modi.date_format = Some("MMM d, yyyy".to_string());
        let mut out = var("TextVariable/uo", "OutputDateType");
        out.date_format = Some("MM/dd/yy".to_string());
        let dm = designmap_with(vec![cre, modi, out]);
        let clock = test_clock();
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/uc", "STALE"),
            Some("2020-01-02".to_string())
        );
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/um", "STALE"),
            Some("Mar 9, 2024".to_string())
        );
        // OutputDate uses the INJECTED output instant (2026-12-31).
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/uo", "STALE"),
            Some("12/31/26".to_string())
        );
    }

    #[test]
    fn date_without_format_uses_documented_default() {
        let dm = designmap_with(vec![var("TextVariable/uc", "CreationDateType")]);
        let clock = test_clock();
        // No Format → MM/dd/yyyy default. Creation = 2020-01-02.
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/uc", ""),
            Some("01/02/2020".to_string())
        );
    }

    /// Measured: the chapter number is the DOCUMENT's
    /// (`<ChapterNumberPreference>`, default 1), not a section property.
    #[test]
    fn chapter_number_is_the_documents() {
        let mut v = var("TextVariable/uch", "ChapterNumberType");
        v.number_format = Some("Current".to_string());
        let mut dm = designmap_with(vec![v]);
        dm.sections = vec![paged_model::Section {
            self_id: "s".to_string(),
            page_start: None,
            continue_numbering: false,
            start_at: Some(2),
            numbering_style: NumberingStyle::UpperRoman,
            section_prefix: None,
            marker: Some("Appendix".to_string()),
            include_prefix: false,
        }];
        let clock = DocumentClock::default();
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/uch", "7"),
            Some("1".to_string()),
            "no preference: chapter 1, whatever the sections say"
        );
        dm.chapter_number = Some(paged_model::ChapterNumberPreference {
            number: Some(4),
            format: Some("I, II, III, IV...".to_string()),
        });
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/uch", "7"),
            Some("IV".to_string())
        );
    }

    #[test]
    fn running_header_resolves_post_layout_per_page() {
        let mut v = var("TextVariable/urh", "MatchParagraphStyleType");
        v.running_header_style = Some("ParagraphStyle/Heading".to_string());
        let dm = designmap_with(vec![v]);
        let clock = DocumentClock::default();
        let mut index = RunningHeaderIndex::default();
        index.first.insert(
            (0, "ParagraphStyle/Heading".to_string()),
            "Chapter One".to_string(),
        );
        index.first.insert(
            (1, "ParagraphStyle/Heading".to_string()),
            "Chapter Two".to_string(),
        );
        // Page 0 picks up "Chapter One".
        let mut c0 = ctx(&dm, &clock, 2);
        c0.page_index = 0;
        c0.running_headers = Some(&index);
        assert_eq!(
            resolve_variable(&c0, "TextVariable/urh", "baked"),
            Some("Chapter One".to_string())
        );
        // Page 1 picks up "Chapter Two" — proving per-page resolution.
        let mut c1 = ctx(&dm, &clock, 2);
        c1.page_index = 1;
        c1.running_headers = Some(&index);
        assert_eq!(
            resolve_variable(&c1, "TextVariable/urh", "baked"),
            Some("Chapter Two".to_string())
        );
        // Pre-layout (index None) keeps the baked value.
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 2), "TextVariable/urh", "baked"),
            None
        );
    }

    #[test]
    fn character_style_running_header_reads_its_own_slot() {
        let mut v = var("TextVariable/uk", "MatchCharacterStyleType");
        v.running_header_character_style = Some("CharacterStyle/Keyword".to_string());
        v.running_header_use = Some("LastOnPage".to_string());
        let dm = designmap_with(vec![v]);
        let clock = DocumentClock::default();
        let mut index = RunningHeaderIndex::default();
        index.last.insert(
            (0, "CharacterStyle/Keyword".to_string()),
            "beta".to_string(),
        );
        let mut c = ctx(&dm, &clock, 1);
        c.running_headers = Some(&index);
        assert_eq!(
            resolve_variable(&c, "TextVariable/uk", "<Keyword>"),
            Some("beta".to_string())
        );
    }

    /// InDesign's answers for the `variables` fixture's headings.
    #[test]
    fn change_case_and_end_punctuation_follow_indesign() {
        let cases = [
            (
                "Titlecase",
                "Introduction: the First Heading.",
                "Introduction: The First Heading.",
            ),
            (
                "Titlecase",
                "PART TWO begins (a third heading)",
                "Part Two Begins (a Third Heading)",
            ),
            ("Titlecase", "the final heading!", "The Final Heading!"),
            (
                "Sentencecase",
                "Introduction: the First Heading.",
                "Introduction: the first heading.",
            ),
            (
                "Sentencecase",
                "PART TWO begins (a third heading)",
                "Part two begins (a third heading)",
            ),
            (
                "Sentencecase",
                "heading a. the first",
                "Heading a. The first",
            ),
            (
                "Lowercase",
                "PART TWO begins (a third heading)",
                "part two begins (a third heading)",
            ),
            ("Uppercase", "the final heading!", "THE FINAL HEADING!"),
        ];
        for (case, input, want) in cases {
            assert_eq!(change_case(input, Some(case)), want, "{case} of {input:?}");
        }
        assert_eq!(
            delete_end_punctuation("Introduction: the First Heading."),
            "Introduction: the First Heading"
        );
        assert_eq!(
            delete_end_punctuation("the final heading!"),
            "the final heading"
        );
        assert_eq!(
            delete_end_punctuation("PART TWO begins (a third heading)"),
            "PART TWO begins (a third heading)"
        );
    }

    #[test]
    fn unknown_variable_id_keeps_run_text() {
        let dm = designmap_with(vec![]);
        let clock = DocumentClock::default();
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/missing", "x"),
            None
        );
    }

    #[test]
    fn url_hyperlink_resolves() {
        let dm = DesignMap {
            hyperlinks: vec![Hyperlink {
                self_id: "Hyperlink/h1".to_string(),
                name: None,
                source: Some("HyperlinkTextSource/s1".to_string()),
                destination: Some("HyperlinkURLDestination/d1".to_string()),
            }],
            hyperlink_destinations: vec![HyperlinkDestination {
                self_id: "HyperlinkURLDestination/d1".to_string(),
                kind: HyperlinkDestinationKind::Url("https://paged.media".to_string()),
            }],
            ..DesignMap::default()
        };
        assert_eq!(
            resolve_link_target(&dm, "HyperlinkTextSource/s1", |_| None),
            LinkTarget::Url("https://paged.media".to_string())
        );
    }

    #[test]
    fn page_hyperlink_resolves_to_index() {
        let dm = DesignMap {
            hyperlinks: vec![Hyperlink {
                self_id: "Hyperlink/h2".to_string(),
                name: None,
                source: Some("HyperlinkTextSource/s2".to_string()),
                destination: Some("HyperlinkPageDestination/d2".to_string()),
            }],
            hyperlink_destinations: vec![HyperlinkDestination {
                self_id: "HyperlinkPageDestination/d2".to_string(),
                kind: HyperlinkDestinationKind::Page("Page/p3".to_string()),
            }],
            ..DesignMap::default()
        };
        let target = resolve_link_target(&dm, "HyperlinkTextSource/s2", |id| {
            (id == "Page/p3").then_some(2)
        });
        assert_eq!(target, LinkTarget::PageIndex(2));
    }

    #[test]
    fn dangling_source_is_unresolved() {
        let dm = DesignMap::default();
        assert_eq!(
            resolve_link_target(&dm, "HyperlinkTextSource/nope", |_| None),
            LinkTarget::Unresolved("HyperlinkTextSource/nope".to_string())
        );
    }
}
