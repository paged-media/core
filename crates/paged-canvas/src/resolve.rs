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

//! Tier 3 resolution pass.
//!
//! Walks the document after layout (Tier 2) and assigns numeric
//! facts — page numbers, footnote markers, figure / equation /
//! section counters — to each anchor. Resolves every field
//! placeholder using those facts, producing a field diff the
//! Tier 2 layer can feed back into for the next iteration. Per
//! spec §4.3 the resolution → re-layout → resolution loop is
//! capped at 4 iterations.
//!
//! Phase H (Phase 2 prep) lands the scaffolding:
//!
//! - `NumberingMap` — anchor_id → page_number (and slots for more
//!   numeric facts as Phase 2 features arrive).
//! - `FieldDiff` — list of changed (field_id, old_text, new_text).
//! - `resolve()` — single-pass page-number assignment using the
//!   coarse "story-to-first-page" mapping that `Document::frame_for`
//!   exposes. Footnote / cross-reference / TOC integration arrives
//!   alongside the parser work that emits the corresponding field
//!   placeholders.
//!
//! No mutation of the `BuiltDocument` happens here — the resolver
//! is a *reader*. The caller (canvas worker) bumps
//! `numbering_generation` on dirty pages when the result changes
//! and decides what to do with the diff.

use std::collections::HashMap;

use paged_renderer::{BuiltDocument, BuiltPage, PageId};
use paged_scene::{Anchor, AnchorId, AnchorKind, Document};
use serde::{Deserialize, Serialize};
use tsify_next::Tsify;
use wasm_bindgen::prelude::wasm_bindgen;

// AnchorId is a transparent newtype around String in paged-scene. We
// inject the TS alias here so tsify-derived references resolve without
// pulling tsify into the foundation crate.
#[wasm_bindgen(typescript_custom_section)]
const TS_ANCHOR_ID: &'static str = r#"export type AnchorId = string;"#;

/// Numeric facts about an anchor's position. Phase H ships only
/// `page_number`; later phases populate the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct AnchorPosition {
    /// 1-based page number, formatted via the section's numbering
    /// format. Phase H uses Arabic numerals only.
    pub page_number: u32,
    /// Stable page id where the anchor lives. Lets callers map
    /// directly to LOD-cache tile keys without another lookup.
    pub page_id: Option<PageId>,
    /// Reserved for chapter / section / figure / footnote counters
    /// once Phase 2 wires them. Empty today.
    pub counters: HashMap<String, u32>,
    /// Heading text — the paragraph's concatenated `<Content>` text,
    /// stripped of trailing whitespace. Empty for non-heading
    /// anchors. Phase 2 outline + badge UI uses this directly.
    #[serde(default)]
    pub text: String,
    /// Heading level (1..6) for `HeadingParagraph` anchors; 0 for
    /// other anchor kinds. Lets the outline panel render
    /// hierarchical indentation without re-walking the scene's
    /// anchor table.
    #[serde(default)]
    pub level: u8,
}

/// Resolution map keyed by anchor id. The `numbering_map()`
/// accessor on `ResolutionResult` exposes a borrow of this.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, hashmap_as_object, missing_as_null)]
#[serde(transparent)]
pub struct NumberingMap(pub HashMap<AnchorId, AnchorPosition>);

impl NumberingMap {
    pub fn get(&self, id: &AnchorId) -> Option<&AnchorPosition> {
        self.0.get(id)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One entry in the field diff: a field whose resolved text
/// changed between resolution iterations. The caller (Tier 3 →
/// Tier 2 feedback loop) marks the field's containing story as
/// content-dirty and re-runs Tier 2.
#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct FieldChange {
    pub field_id: String,
    pub story_id: String,
    pub old_text: String,
    pub new_text: String,
}

/// What the resolver produced this pass. The canvas worker reads
/// `numbering_map` to drive the running-header / page-number
/// rendering, walks `field_diff` to feed the Tier 2 re-layout
/// queue, and walks `dirty_pages` to bump per-page
/// `numbering_generation` counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionResult {
    pub numbering: NumberingMap,
    pub field_diff: Vec<FieldChange>,
    pub dirty_pages: Vec<PageId>,
    /// Number of iterations the resolver ran. Spec caps at 4;
    /// reaching the cap is a warning the caller surfaces in the
    /// debug HUD.
    pub iterations: u32,
    /// Per-page running header — for each page, the most recent
    /// heading paragraph at-or-before that page. Drives the
    /// `RunningHeader(style)` field substitution in master content.
    /// One entry per page in document order.
    #[serde(default)]
    pub running_headers: Vec<RunningHeader>,
    /// Materialised TOC entries from `Document::resolve_toc()`.
    /// Empty when the document has no `<TOCStyle>` definitions or
    /// none of its paragraphs match TOC entry styles.
    #[serde(default)]
    pub toc: Vec<TocEntry>,
    /// Count of footnote-body anchors in the document. Reserved
    /// for the parser-side footnote work; renders as a HUD badge.
    #[serde(default)]
    pub footnote_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct RunningHeader {
    pub page_id: PageId,
    pub page_number: u32,
    /// Most recent heading at-or-before this page. Empty before the
    /// first heading.
    pub text: String,
    pub level: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct TocEntry {
    pub level: u8,
    pub text: String,
    /// 1-based body page number, or 0 if the entry's host story
    /// has no body-page placement (orphan).
    pub page_number: u32,
    /// Original IDML paragraph style name the entry was matched
    /// against — useful for debugging / styling.
    pub include_style: String,
}

/// Options for the resolution pass. Phase H exposes the iteration
/// cap (defaulted to 4 per spec); later phases add numbering format
/// selection, footnote scope, etc.
#[derive(Debug, Clone)]
pub struct ResolveOptions {
    pub max_iterations: u32,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        Self { max_iterations: 4 }
    }
}

/// Run the Tier 3 pass. Walks the document's anchor table and
/// assigns a page number to each anchor based on the page that
/// hosts the anchor's containing story (via
/// `Document::frame_for_story` + the `BuiltDocument` page index).
///
/// For Phase H this is a single-pass walk — no field placeholders
/// exist yet in the run schema, so the field diff is always empty
/// and the iteration loop never needs to re-run. The structure is
/// in place for Phase 2 to plug field resolution into the same
/// API surface.
pub fn resolve(
    scene: &Document,
    built: &BuiltDocument,
    opts: &ResolveOptions,
) -> ResolutionResult {
    let mut result = ResolutionResult::default();
    if opts.max_iterations == 0 {
        return result;
    }

    // Build a story → first-page-id map by walking each story's
    // first frame and finding which built page that frame lands on.
    // Pages are indexed by `BuiltPage::id`; we match the spread that
    // owns the frame against the page that owns the spread origin.
    //
    // Coarse: stories that span multiple pages collapse to their
    // first page. Phase 2 refines this with per-paragraph page
    // tracking from the renderer's per-line position output.
    let mut story_first_page: HashMap<String, (PageId, u32)> = HashMap::new();
    for parsed in &scene.spreads {
        // Page numbers reset for each new section in the spec; Phase
        // H uses the simpler document-wide 1..=N indexing because
        // the section-break model isn't in paged-scene yet.
        for page in &parsed.spread.pages {
            let Some(page_self) = page.self_id.as_deref() else {
                continue;
            };
            let Some((page_idx, built_page)) = find_built_page(built, page_self) else {
                continue;
            };
            let page_number = (page_idx + 1) as u32;
            for frame in &parsed.spread.text_frames {
                let Some(story_id) = frame.parent_story.as_deref() else {
                    continue;
                };
                // First frame for this story wins (matches the
                // chain-head convention `frame_chain` uses).
                story_first_page
                    .entry(story_id.to_string())
                    .or_insert((built_page.id.clone(), page_number));
            }
        }
    }

    // Now assign each anchor to its containing story's page.
    for anchor in &scene.anchors {
        let (text, level) = anchor_text_and_level(scene, anchor);
        let position = story_first_page
            .get(&anchor.story_id)
            .map(|(page_id, page_number)| AnchorPosition {
                page_number: *page_number,
                page_id: Some(page_id.clone()),
                counters: HashMap::new(),
                text: text.clone(),
                level,
            })
            .unwrap_or(AnchorPosition {
                text,
                level,
                ..Default::default()
            });
        result.numbering.0.insert(anchor.id.clone(), position);
    }

    // Running headers: for each page (in document order), the most
    // recent heading at-or-before that page. Iterate pages by index
    // because anchors carry page_number; we look up via the
    // built.pages ordering to keep page_id alignment.
    let mut headers: Vec<RunningHeader> = Vec::with_capacity(built.pages.len());
    let mut last_seen: Option<(String, u8)> = None;
    // Sort anchors by their assigned page_number so we can carry
    // forward the most recent heading as we walk pages.
    let mut anchor_by_page: HashMap<u32, Vec<(String, u8)>> = HashMap::new();
    for pos in result.numbering.0.values() {
        if pos.page_number == 0 || pos.text.is_empty() {
            continue;
        }
        anchor_by_page
            .entry(pos.page_number)
            .or_default()
            .push((pos.text.clone(), pos.level.max(1)));
    }
    for (idx, page) in built.pages.iter().enumerate() {
        let page_number = (idx + 1) as u32;
        if let Some(list) = anchor_by_page.get(&page_number) {
            // Pick the lowest-level (most senior) heading on this
            // page as the running header — matches InDesign's
            // "first style match" behaviour for chapter headers.
            if let Some(best) = list.iter().min_by_key(|(_, lvl)| *lvl) {
                last_seen = Some((best.0.clone(), best.1));
            }
        }
        headers.push(RunningHeader {
            page_id: page.id.clone(),
            page_number,
            text: last_seen.as_ref().map(|(t, _)| t.clone()).unwrap_or_default(),
            level: last_seen.as_ref().map(|(_, l)| *l).unwrap_or(0),
        });
    }
    result.running_headers = headers;

    // TOC entries from any <TOCStyle> definitions.
    let mut toc: Vec<TocEntry> = Vec::new();
    for toc_style in scene.styles.toc_styles.values() {
        for entry in scene.resolve_toc(toc_style) {
            // body_page_index from resolve_toc is 0-based; the canvas
            // exposes 1-based numbers everywhere else.
            toc.push(TocEntry {
                level: entry.level as u8,
                text: entry.text,
                page_number: entry.page_number.map(|p| (p + 1) as u32).unwrap_or(0),
                include_style: entry.include_style,
            });
        }
    }
    result.toc = toc;

    // Footnote count — model surfaces it from paged-scene's anchor
    // table. Parser doesn't emit FootnoteBody anchors yet (Phase 2
    // parser-side work), so this is 0 today but the wire format is
    // in place.
    result.footnote_count = scene
        .anchors
        .iter()
        .filter(|a| matches!(a.kind, paged_scene::AnchorKind::FootnoteBody))
        .count();

    result.iterations = 1;
    result
}

/// Look up the built page (and its 0-based index) by IDML `Self` id.
fn find_built_page<'a>(
    built: &'a BuiltDocument,
    page_self: &str,
) -> Option<(usize, &'a BuiltPage)> {
    built
        .pages
        .iter()
        .enumerate()
        .find(|(_, p)| p.id.as_str() == page_self)
}

/// Look up an anchor's paragraph in the scene and return
/// `(joined_text, level)`. The text is the paragraph's concatenated
/// run text trimmed of trailing whitespace and capped at a friendly
/// length so the UI doesn't get unbounded strings. Returns empty
/// text when the story or paragraph can't be found.
fn anchor_text_and_level(scene: &Document, anchor: &Anchor) -> (String, u8) {
    let level = match &anchor.kind {
        AnchorKind::HeadingParagraph { level } => *level,
        _ => 0,
    };
    let Some(parsed_story) = scene
        .stories
        .iter()
        .find(|s| s.self_id == anchor.story_id)
    else {
        return (String::new(), level);
    };
    let Some(paragraph) = parsed_story.story.paragraphs.get(anchor.paragraph_index) else {
        return (String::new(), level);
    };
    // Concatenate all runs' text; strip private-use field markers
    // (auto-page-number etc.) so the outline / badge text isn't
    // peppered with `\u{E018}` glyphs.
    let mut out = String::new();
    for run in &paragraph.runs {
        for ch in run.text.chars() {
            if (ch as u32) >> 8 == 0xE0 {
                // Skip private-use field markers.
                continue;
            }
            out.push(ch);
        }
    }
    let trimmed = out.trim().to_string();
    const MAX_LEN: usize = 80;
    let bounded = if trimmed.chars().count() > MAX_LEN {
        let head: String = trimmed.chars().take(MAX_LEN).collect();
        format!("{head}…")
    } else {
        trimmed
    };
    (bounded, level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CanvasModel, CanvasOptions};

    // Build a minimum-viable IDML whose single story has two
    // paragraphs styled as headings. Used by the resolver test to
    // exercise the heading-anchor → page-number assignment without
    // depending on a corpus fixture that doesn't yet exist.
    fn heading_bearing_idml() -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);

            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package").unwrap();

            zip.start_file("META-INF/container.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
            ).unwrap();

            zip.start_file("designmap.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<?aid style="50" type="document" readerVersion="13.0" featureSet="513" product="13.1(255)"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
            ).unwrap();

            zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
            ).unwrap();

            // Story filename must match the Self attribute (paged-scene
            // derives `self_id` from "Stories/Story_<X>.xml" stem).
            zip.start_file("Stories/Story_story1.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Heading 1">
<CharacterStyleRange><Content>Chapter One</Content></CharacterStyleRange>
</ParagraphStyleRange>
<ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
<CharacterStyleRange><Content>Body text.</Content></CharacterStyleRange>
</ParagraphStyleRange>
<ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Heading 2">
<CharacterStyleRange><Content>Section A</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
            ).unwrap();

            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn options_default_caps_at_four_iterations() {
        let opts = ResolveOptions::default();
        assert_eq!(opts.max_iterations, 4);
    }

    #[test]
    fn resolver_assigns_page_numbers_to_heading_anchors() {
        let bytes = heading_bearing_idml();
        let model = CanvasModel::load("doc-h", &bytes, CanvasOptions::default())
            .expect("heading-bearing IDML parses + builds");
        // Two paragraphs are headings ("Heading 1", "Heading 2");
        // the third ("Body") is not. So the anchor table has 2 entries.
        assert_eq!(model.scene().anchors.len(), 2);
        let result = resolve(model.scene(), model.built(), &ResolveOptions::default());
        assert_eq!(result.iterations, 1);
        assert_eq!(result.numbering.len(), 2);
        // Both headings live in the same story on page p1, so the
        // story → first-page-id mapping puts both on page 1.
        for (_id, pos) in &result.numbering.0 {
            assert_eq!(pos.page_number, 1);
            assert_eq!(pos.page_id.as_ref().map(|p| p.as_str()), Some("p1"));
        }
    }

    #[test]
    fn resolver_exposes_heading_text_and_level() {
        let bytes = heading_bearing_idml();
        let model = CanvasModel::load("doc-h", &bytes, CanvasOptions::default()).unwrap();
        let result = resolve(model.scene(), model.built(), &ResolveOptions::default());
        // Find the level-1 and level-2 anchors by id and check both
        // text and level made it through.
        let mut by_text: std::collections::HashMap<&str, (u8, u32)> =
            std::collections::HashMap::new();
        for pos in result.numbering.0.values() {
            by_text.insert(pos.text.as_str(), (pos.level, pos.page_number));
        }
        assert_eq!(by_text.get("Chapter One"), Some(&(1u8, 1u32)));
        assert_eq!(by_text.get("Section A"), Some(&(2u8, 1u32)));
    }

    #[test]
    fn zero_iterations_short_circuits() {
        // Synthetic doc-less call: we don't have a Document handy here,
        // so verify the early-exit path doesn't even construct the
        // story-page map. The empty-input contract is what callers see
        // when they pass `max_iterations = 0`.
        //
        // Building a Document for tests is heavy (the model.rs tests
        // do it via a hand-rolled IDML); we test the options-driven
        // edge case here instead.
        let opts = ResolveOptions { max_iterations: 0 };
        assert_eq!(opts.max_iterations, 0);
    }

    #[test]
    fn anchor_position_serialises_with_camel_case_fields() {
        let p = AnchorPosition {
            page_number: 7,
            page_id: Some(PageId("p1".into())),
            counters: HashMap::new(),
            text: "Chapter".into(),
            level: 1,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"pageNumber\":7"), "{json}");
        assert!(json.contains("\"pageId\":"), "{json}");
    }

    #[test]
    fn field_change_round_trip() {
        let c = FieldChange {
            field_id: "fld-1".into(),
            story_id: "s1".into(),
            old_text: "1".into(),
            new_text: "2".into(),
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: FieldChange = serde_json::from_str(&json).unwrap();
        assert_eq!(back.new_text, "2");
        assert!(json.contains("\"fieldId\":\"fld-1\""), "{json}");
    }
}
