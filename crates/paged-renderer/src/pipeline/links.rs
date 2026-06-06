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

//! W1.4 — text-variable resolution + hyperlink/cross-reference target
//! resolution at render time.
//!
//! ## Text variables
//!
//! A `<TextVariableInstance>` in a story carries InDesign's baked
//! `ResultText` (the value the document last composed) plus an
//! `AssociatedTextVariable` id. The parser splits each instance into
//! its own [`paged_parse::CharacterRun`] tagged with `text_variable`.
//! At paragraph emit, [`resolve_variable`] re-resolves the value per
//! the variable's `VariableType` when it can do better than the stale
//! baked string — mirroring how the auto-page-number marker is
//! substituted in `pipeline::mod`.
//!
//! Per-type semantics (honest about what's modelled):
//!
//! | VariableType        | Resolution                                    |
//! |---------------------|-----------------------------------------------|
//! | `CustomTextType`    | `TextBefore` + `Contents` + `TextAfter` (literal, from the IDML) |
//! | `PageCountType`     | real total body-page count                    |
//! | `FileNameType`      | document `Name` (`.indd`), else `ResultText`, else `"untitled.indd"` |
//! | `CreationDateType`  | baked `ResultText` (we don't model file timestamps; the baked date is the honest available value) |
//! | `ModificationDateType` | baked `ResultText` (same)                  |
//! | `OutputDateType`    | baked `ResultText` (same)                     |
//! | `ChapterNumberType` | baked `ResultText` (chapter input not modelled) |
//! | `RunningHeaderType` | baked `ResultText` (live header pickup not modelled) |
//! | (anything else)     | baked `ResultText`                            |
//!
//! Date variables keep `ResultText` rather than fabricating a value
//! from `date_format` (printing the format pattern would be wrong, and
//! we have no parsed timestamp to format) — the baked string is the
//! date InDesign actually computed. When `ResultText` is itself empty
//! the resolver returns `None` and the caller keeps the run's text.
//!
//! ## Hyperlinks / cross-references
//!
//! A run tagged with `hyperlink_source` came from a
//! `<HyperlinkTextSource>` / `<CrossReferenceSource>` span. The
//! designmap's `<Hyperlink Source=... Destination=...>` maps the source
//! id to a destination resource ([`resolve_link_target`]); page
//! destinations resolve to a flat 0-based body-page index.

use paged_compose::LinkTarget;
use paged_parse::{DesignMap, HyperlinkDestinationKind};

/// Resolve a tagged variable run to its render-time value, or `None`
/// to keep the run's baked `ResultText`.
///
/// `variable_id` is the run's `text_variable` (`TextVariable/<id>`).
/// `result_text` is the run's current text (the baked value).
pub(crate) fn resolve_variable(
    designmap: &DesignMap,
    variable_id: &str,
    result_text: &str,
    total_pages: usize,
) -> Option<String> {
    let var = designmap
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
        "PageCountType" => Some(decorate(total_pages.to_string())),
        "FileNameType" => {
            let name = designmap
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
        // Date types + chapter/running-header: the baked ResultText is
        // the honest available value (we model neither file timestamps
        // nor live header pickup). Keep the run's text unless it's
        // empty, in which case fall back to the format pattern / a
        // documented placeholder so the slot never renders blank.
        "CreationDateType" | "ModificationDateType" | "OutputDateType" => {
            if result_text.is_empty() {
                // No baked value: surface the declared format pattern so
                // the slot is at least self-describing, else a generic
                // placeholder. Documented in this module's table.
                let fallback = var
                    .date_format
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "0000-00-00".to_string());
                Some(decorate(fallback))
            } else {
                None
            }
        }
        "ChapterNumberType" | "RunningHeaderType" | "RunningHeaderVariableType" => {
            if result_text.is_empty() {
                // Inputs not modelled and no baked value — emit a
                // documented placeholder rather than an empty slot.
                Some(decorate("—".to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
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
    use paged_parse::{Hyperlink, HyperlinkDestination, TextVariable};

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
        }
    }

    #[test]
    fn custom_text_resolves_to_contents_with_decoration() {
        let mut v = var("TextVariable/u1", "CustomTextType");
        v.contents = Some("Spring".to_string());
        v.text_before = Some("[".to_string());
        v.text_after = Some("]".to_string());
        let dm = designmap_with(vec![v]);
        assert_eq!(
            resolve_variable(&dm, "TextVariable/u1", "stale", 7),
            Some("[Spring]".to_string())
        );
    }

    #[test]
    fn page_count_resolves_to_real_total() {
        let dm = designmap_with(vec![var("TextVariable/u2", "PageCountType")]);
        assert_eq!(
            resolve_variable(&dm, "TextVariable/u2", "1", 12),
            Some("12".to_string())
        );
    }

    #[test]
    fn file_name_prefers_document_name() {
        let dm = designmap_with(vec![var("TextVariable/u3", "FileNameType")]);
        assert_eq!(
            resolve_variable(&dm, "TextVariable/u3", "old.indd", 1),
            Some("brochure.indd".to_string())
        );
    }

    #[test]
    fn date_keeps_baked_result_text() {
        let dm = designmap_with(vec![var("TextVariable/u4", "CreationDateType")]);
        // Non-empty baked value: keep it (None ⇒ caller uses run.text).
        assert_eq!(
            resolve_variable(&dm, "TextVariable/u4", "2024-01-02", 1),
            None
        );
    }

    #[test]
    fn unknown_variable_id_keeps_run_text() {
        let dm = designmap_with(vec![]);
        assert_eq!(resolve_variable(&dm, "TextVariable/missing", "x", 1), None);
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
