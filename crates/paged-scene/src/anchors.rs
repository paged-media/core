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

//! Anchor + field model for the IDML scene.
//!
//! Phase 2 prep (Prep-D from the canvas roadmap). The model lives
//! here so Tier 3 resolution (paged-canvas::resolve) can build the
//! numbering map and resolve fields without round-tripping back into
//! the parser.
//!
//! **Anchors** are named positions within stories that other content
//! references — heading paragraphs (TOC source), footnote markers
//! (footnote anchor on the body side), cross-reference targets,
//! bookmarks. The anchor table maps `AnchorId` → location inside a
//! specific story.
//!
//! **Fields** are placeholders in text runs that get resolved during
//! the Tier 3 pass: page references (`PageRef`), text references
//! (`TextRef`, used by the TOC to repeat heading text), autonumbers
//! (figure / equation / footnote markers), document/section variables,
//! and running headers (per-page chapter title in the page footer).
//!
//! Phase 1 lands the *types*. Resolution semantics arrive with the
//! Tier 3 resolver in `paged-canvas`. The parser does not yet emit
//! `Field` placeholders into the run schema; that wiring is the
//! next concrete piece of Phase 2.

use serde::{Deserialize, Serialize};

/// Stable identifier for an anchor. Synthesized from the source —
/// for heading-paragraph anchors, derives from `(story_id, paragraph_idx)`
/// so the id stays stable across re-builds. For IDML-declared
/// anchors (Hyperlink destinations, bookmarks), this is the IDML
/// `Self` attribute on the anchor element.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnchorId(pub String);

impl AnchorId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Synthesize an id for a heading paragraph. Combines the story
    /// id and paragraph index so it stays stable as long as the
    /// paragraph keeps its position.
    pub fn heading(story_id: &str, paragraph_idx: usize) -> Self {
        Self(format!("h:{story_id}:{paragraph_idx}"))
    }
}

impl std::fmt::Display for AnchorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What an anchor refers to. Phase 2 prep covers heading anchors and
/// the type slots for footnotes / cross-references / bookmarks; the
/// detection logic for those latter kinds lands when the parser
/// emits the corresponding markers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "info")]
pub enum AnchorKind {
    /// A paragraph styled as a heading (paragraph style name
    /// contains "heading" case-insensitive). Source of TOC entries.
    HeadingParagraph {
        /// Heading level 1..6 inferred from the trailing digit in
        /// the style name, e.g. "Heading 2" → 2. Defaults to 1 when
        /// the name has no digit.
        level: u8,
    },
    /// Footnote body anchor — the position in the body story where
    /// the footnote marker sits. Reserved for Phase 2; not yet
    /// emitted.
    FootnoteBody,
    /// Cross-reference target — the destination of a `Hyperlink
    /// TextDestination`. Reserved for Phase 2.
    CrossRefTarget,
    /// Bookmark — IDML `<Bookmark>` element. Reserved for Phase 2.
    Bookmark,
}

/// One anchor entry. `story_id` is the IDML story `Self`;
/// `paragraph_index` is the paragraph's position in that story's
/// `paragraphs` vec.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Anchor {
    pub id: AnchorId,
    pub story_id: String,
    pub paragraph_index: usize,
    pub kind: AnchorKind,
}

/// Field placeholder kinds. The Tier 3 resolver substitutes the
/// resolved text into a copy of the run that contained the placeholder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "info")]
pub enum FieldKind {
    /// Resolves to the page number of the anchor's containing line.
    PageRef { target: AnchorId },
    /// Resolves to the text content of the anchor's containing
    /// paragraph. Used by TOCs to quote heading text.
    TextRef { target: AnchorId },
    /// Resolves to an autonumber counter value (footnote markers,
    /// figure / equation / list numbers).
    AutoNumber { scope: String, format: String },
    /// Resolves to a document- or section-level variable (chapter
    /// title from running heading, document date, file name).
    Variable { name: String },
    /// For each page, the most recent paragraph with the named
    /// paragraph style. Resolved per-page, not per-field, by the
    /// Tier 3 pass.
    RunningHeader { style: String },
}

/// A field placeholder. Carries its own stable id so the resolver
/// can emit a diff (`field_id → old / new value`) cheaply.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub id: String,
    pub kind: FieldKind,
}

/// Heuristic detector: does the paragraph style name indicate a
/// heading? Matches "heading", "title", and similar — broad enough
/// to catch Adobe defaults across templates while requiring an
/// explicit style choice from the author.
pub fn paragraph_style_is_heading(style_name: &str) -> bool {
    let lower = style_name.to_lowercase();
    lower.contains("heading")
        || lower.contains("title")
        || lower.starts_with("h1")
        || lower.starts_with("h2")
        || lower.starts_with("h3")
}

/// Infer a heading level from a style name. Looks for a trailing
/// digit in 1..=6; falls back to 1 if absent. "Heading 2" → 2;
/// "Heading" → 1; "Title" → 1.
pub fn heading_level_from_style(style_name: &str) -> u8 {
    for ch in style_name.chars().rev() {
        if ch.is_ascii_digit() {
            let digit = ch.to_digit(10).unwrap_or(1) as u8;
            return digit.clamp(1, 6);
        }
        if !ch.is_ascii_whitespace() {
            // Hit a non-digit, non-space — stop scanning.
            break;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_id_heading_format_is_stable() {
        let a = AnchorId::heading("s1", 4);
        assert_eq!(a.as_str(), "h:s1:4");
    }

    #[test]
    fn heading_detector_matches_common_names() {
        assert!(paragraph_style_is_heading("Heading 1"));
        assert!(paragraph_style_is_heading("HEADING 2"));
        assert!(paragraph_style_is_heading("Heading"));
        assert!(paragraph_style_is_heading("Title"));
        assert!(paragraph_style_is_heading("H1"));
        assert!(!paragraph_style_is_heading("Body"));
        assert!(!paragraph_style_is_heading("[Basic Paragraph]"));
    }

    #[test]
    fn heading_level_parses_trailing_digit() {
        assert_eq!(heading_level_from_style("Heading 1"), 1);
        assert_eq!(heading_level_from_style("Heading 2"), 2);
        assert_eq!(heading_level_from_style("Heading 6"), 6);
        assert_eq!(heading_level_from_style("Heading"), 1);
        assert_eq!(heading_level_from_style("Title"), 1);
        assert_eq!(heading_level_from_style("H3"), 3);
    }

    #[test]
    fn field_kinds_serde_roundtrip_camel_case() {
        let f = Field {
            id: "fld-1".into(),
            kind: FieldKind::PageRef {
                target: AnchorId("h:s1:4".into()),
            },
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"kind\":\"pageRef\""), "{json}");
        assert!(json.contains("\"target\":"), "{json}");
        let back: Field = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "fld-1");
    }
}
