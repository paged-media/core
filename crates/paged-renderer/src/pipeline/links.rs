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
//! Per-type semantics:
//!
//! | VariableType        | Resolution                                    |
//! |---------------------|-----------------------------------------------|
//! | `CustomTextType`    | `TextBefore` + `Contents` + `TextAfter` (literal, from the IDML) |
//! | `PageCountType`     | real total body-page count                    |
//! | `FileNameType`      | document `Name` (`.indd`), else `ResultText`, else `"untitled.indd"` |
//! | `CreationDateType`  | W1.18a — `date_format` tokens applied to the document clock's `creation` date |
//! | `ModificationDateType` | W1.18a — `date_format` applied to the clock's `modification` date |
//! | `OutputDateType`    | W1.18a — `date_format` applied to the clock's INJECTED `output` instant (never wall-clock) |
//! | `ChapterNumberType` | W1.18b — the section's chapter number, styled per `<Section>` numbering |
//! | `RunningHeaderType` / `RunningHeaderVariableType` | W1.18c — text of the nearest paragraph/character on the SAME page matching the named style; resolved post-layout |
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
use paged_model::{DesignMap, HyperlinkDestinationKind, NumberingStyle, Section};

use super::datefmt::{self, DateParts};
use super::DocumentClock;

/// W1.18c — per-page running-header pickup index, built after the first
/// layout pass. Maps a paragraph-style id to the text of its first and
/// last occurrence on each page, so a `RunningHeaderType` variable in a
/// header/footer frame resolves to the matching content on that page.
///
/// `style_first[(page_idx, style_id)]` = text of the first paragraph on
/// `page_idx` whose applied paragraph style is `style_id`;
/// `style_last` is the same for the last such paragraph. InDesign's
/// "Use" option (`FirstOnPage` / `LastOnPage`) picks which one. When a
/// page has no matching paragraph, InDesign falls back to the most
/// recent match from an earlier page — captured by `style_fallback`,
/// the running last-seen text walking pages in order.
#[derive(Debug, Default, Clone)]
pub(crate) struct RunningHeaderIndex {
    pub first: HashMap<(usize, String), String>,
    pub last: HashMap<(usize, String), String>,
    /// Most-recent matching text at or before each page (carry-forward
    /// fallback for pages with no own match).
    pub fallback: HashMap<(usize, String), String>,
}

impl RunningHeaderIndex {
    /// Resolve the running-header text for `style_id` on `page_idx`,
    /// honouring `use_last` (LastOnPage vs FirstOnPage) and falling back
    /// to the carry-forward text from an earlier page.
    pub fn resolve(&self, page_idx: usize, style_id: &str, use_last: bool) -> Option<String> {
        let key = (page_idx, style_id.to_string());
        let own = if use_last {
            self.last.get(&key)
        } else {
            self.first.get(&key)
        };
        own.or_else(|| self.fallback.get(&key)).cloned()
    }
}

/// W1.18 — render-time resolution context threaded into
/// [`resolve_variable`]. Carries everything a variable needs beyond the
/// designmap + its baked text: the deterministic date clock, the
/// pre-computed chapter number, the host page index, and (post-layout)
/// the running-header pickup index.
pub(crate) struct VarResolveCtx<'a> {
    pub designmap: &'a DesignMap,
    pub total_pages: usize,
    /// Deterministic date clock (creation / modification / output).
    pub clock: &'a DocumentClock,
    /// Chapter number for the section hosting this frame's page,
    /// already styled per the section's numbering. `None` when the
    /// document declares no sections (then `ResultText` / a placeholder
    /// is used).
    pub chapter_number: Option<&'a str>,
    /// Flat 0-based page index of the frame currently emitting — the
    /// page a running header resolves *for*.
    pub page_index: usize,
    /// Post-layout running-header pickup index. `None` on the first
    /// (pre-layout) pass; populated for the re-emit so
    /// `RunningHeaderType` variables resolve to live content.
    pub running_headers: Option<&'a RunningHeaderIndex>,
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
            Some(decorate(name))
        }
        // W1.18a — date variables: apply the declared format tokens to
        // the deterministic clock field for this type. The clock is
        // injected (never `now()`), so the output is reproducible.
        "CreationDateType" => Some(decorate(format_date_var(var, ctx.clock.creation))),
        "ModificationDateType" => Some(decorate(format_date_var(var, ctx.clock.modification))),
        "OutputDateType" => Some(decorate(format_date_var(var, ctx.clock.output))),
        // W1.18b — chapter number from section settings (styled per the
        // section's numbering). Falls back to the baked ResultText, then
        // a placeholder, so the slot is never blank.
        "ChapterNumberType" => {
            let n = ctx
                .chapter_number
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    Some(result_text)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "1".to_string());
            Some(decorate(n))
        }
        // W1.18c — running header. Post-layout, the index carries the
        // matching paragraph text per page; pre-layout (index None) we
        // keep the baked value so the first pass still renders something.
        "RunningHeaderType" | "RunningHeaderVariableType" => {
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
            let resolved = var
                .running_header_style
                .as_deref()
                .and_then(|style_id| index.resolve(ctx.page_index, style_id, use_last));
            match resolved {
                Some(text) if !text.is_empty() => Some(decorate(text)),
                // No match on this page (and no carry-forward): InDesign
                // shows the baked value, else nothing. Keep the run's
                // text rather than overwriting with a placeholder.
                _ => {
                    if result_text.is_empty() {
                        Some(decorate(String::new()))
                    } else {
                        None
                    }
                }
            }
        }
        _ => None,
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

/// IDML `<DateVariablePreference>` records its day-of-week first/last
/// pickup as `DateOrder`-adjacent enums; running headers use a separate
/// flag. Map the `RunningHeaderVariablePreference Use` value to whether
/// the LAST on-page match is wanted (`LastOnPage`) vs the first.
fn running_header_use_last(use_value: &str) -> bool {
    matches!(use_value, "LastOnPage" | "lastOnPage")
}

/// W1.18b — compute the chapter number for the page at flat index
/// `page_idx`, styled per the owning `<Section>`'s numbering. Returns
/// `None` when the document has no sections.
///
/// IDML models chapter numbering on `<Section>`: `Marker` is the
/// explicit chapter label (used verbatim when present), otherwise the
/// chapter NUMBER is the section's `PageNumberStart` formatted in its
/// `PageNumberStyle` — InDesign shares the same numbering machinery for
/// page and chapter numbers. `page_starts` maps a section's
/// `PageStart` `<Page Self>` id to its flat page index so we can find
/// which section owns `page_idx`.
pub(crate) fn chapter_number_for_page(
    sections: &[Section],
    page_starts: &HashMap<String, usize>,
    page_idx: usize,
) -> Option<String> {
    if sections.is_empty() {
        return None;
    }
    // Find the section whose start page is the greatest one <= page_idx
    // (the last section to begin at or before this page owns it).
    let mut best: Option<(usize, &Section)> = None;
    for sec in sections {
        let start = sec
            .page_start
            .as_deref()
            .and_then(|id| page_starts.get(id).copied())
            .unwrap_or(0);
        if start <= page_idx {
            match best {
                Some((bstart, _)) if bstart >= start => {}
                _ => best = Some((start, sec)),
            }
        }
    }
    let (_, sec) = best.or_else(|| sections.first().map(|s| (0, s)))?;
    // An explicit chapter marker wins verbatim.
    if let Some(marker) = sec.marker.as_deref().filter(|s| !s.is_empty()) {
        return Some(marker.to_string());
    }
    let style: NumberingStyle = sec.numbering_style;
    let n = sec.start_at.unwrap_or(1).max(1);
    Some(style.format(n))
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
            name: None,
            variable_type: Some(ty.to_string()),
            contents: None,
            date_format: None,
            text_before: None,
            text_after: None,
            running_header_style: None,
            running_header_use: None,
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

    /// Build a resolution context over `dm` with the given total page
    /// count. Clock = `test_clock`, no chapter / running-header context.
    fn ctx<'a>(
        dm: &'a DesignMap,
        clock: &'a DocumentClock,
        total_pages: usize,
    ) -> VarResolveCtx<'a> {
        VarResolveCtx {
            designmap: dm,
            total_pages,
            clock,
            chapter_number: None,
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

    #[test]
    fn file_name_prefers_document_name() {
        let dm = designmap_with(vec![var("TextVariable/u3", "FileNameType")]);
        let clock = DocumentClock::default();
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/u3", "old.indd"),
            Some("brochure.indd".to_string())
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

    #[test]
    fn chapter_number_uses_section_context() {
        let dm = designmap_with(vec![var("TextVariable/uch", "ChapterNumberType")]);
        let clock = DocumentClock::default();
        let mut c = ctx(&dm, &clock, 1);
        c.chapter_number = Some("IV");
        assert_eq!(
            resolve_variable(&c, "TextVariable/uch", "1"),
            Some("IV".to_string())
        );
        // No section context → fall back to baked value.
        assert_eq!(
            resolve_variable(&ctx(&dm, &clock, 1), "TextVariable/uch", "7"),
            Some("7".to_string())
        );
    }

    #[test]
    fn running_header_resolves_post_layout_per_page() {
        let mut v = var("TextVariable/urh", "RunningHeaderType");
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
    fn running_header_last_on_page_and_carry_forward() {
        let mut v = var("TextVariable/urh", "RunningHeaderType");
        v.running_header_style = Some("ParagraphStyle/Term".to_string());
        v.running_header_use = Some("LastOnPage".to_string());
        let dm = designmap_with(vec![v]);
        let clock = DocumentClock::default();
        let mut index = RunningHeaderIndex::default();
        index.first.insert(
            (0, "ParagraphStyle/Term".to_string()),
            "Aardvark".to_string(),
        );
        index
            .last
            .insert((0, "ParagraphStyle/Term".to_string()), "Azure".to_string());
        // Page 1 has no own match but carries forward the last seen.
        index
            .fallback
            .insert((1, "ParagraphStyle/Term".to_string()), "Azure".to_string());
        // LastOnPage on page 0 → "Azure" (the last term, not first).
        let mut c0 = ctx(&dm, &clock, 2);
        c0.running_headers = Some(&index);
        c0.page_index = 0;
        assert_eq!(
            resolve_variable(&c0, "TextVariable/urh", "baked"),
            Some("Azure".to_string())
        );
        // Page 1 (no own match) carries forward "Azure".
        let mut c1 = ctx(&dm, &clock, 2);
        c1.running_headers = Some(&index);
        c1.page_index = 1;
        assert_eq!(
            resolve_variable(&c1, "TextVariable/urh", "baked"),
            Some("Azure".to_string())
        );
    }

    #[test]
    fn chapter_number_for_page_picks_owning_section() {
        let sections = vec![
            Section {
                self_id: "sec1".to_string(),
                page_start: Some("Page/p1".to_string()),
                continue_numbering: false,
                start_at: Some(1),
                numbering_style: NumberingStyle::Arabic,
                section_prefix: None,
                marker: None,
                include_prefix: false,
            },
            Section {
                self_id: "sec2".to_string(),
                page_start: Some("Page/p3".to_string()),
                continue_numbering: false,
                start_at: Some(2),
                numbering_style: NumberingStyle::UpperRoman,
                section_prefix: None,
                marker: None,
                include_prefix: false,
            },
        ];
        let mut starts = HashMap::new();
        starts.insert("Page/p1".to_string(), 0usize);
        starts.insert("Page/p3".to_string(), 2usize);
        // Page 0,1 → section 1 (chapter "1"); page 2,3 → section 2 ("II").
        assert_eq!(
            chapter_number_for_page(&sections, &starts, 0).as_deref(),
            Some("1")
        );
        assert_eq!(
            chapter_number_for_page(&sections, &starts, 1).as_deref(),
            Some("1")
        );
        assert_eq!(
            chapter_number_for_page(&sections, &starts, 2).as_deref(),
            Some("II")
        );
        // Explicit marker wins verbatim.
        let mut marked = sections.clone();
        marked[1].marker = Some("Appendix".to_string());
        assert_eq!(
            chapter_number_for_page(&marked, &starts, 3).as_deref(),
            Some("Appendix")
        );
        // No sections → None.
        assert_eq!(chapter_number_for_page(&[], &starts, 0), None);
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
