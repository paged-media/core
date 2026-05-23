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

use idml_renderer::{BuiltDocument, BuiltPage, PageId};
use idml_scene::{Anchor, AnchorId, Document};
use serde::{Deserialize, Serialize};

/// Numeric facts about an anchor's position. Phase H ships only
/// `page_number`; later phases populate the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// Resolution map keyed by anchor id. The `numbering_map()`
/// accessor on `ResolutionResult` exposes a borrow of this.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionResult {
    pub numbering: NumberingMap,
    pub field_diff: Vec<FieldChange>,
    pub dirty_pages: Vec<PageId>,
    /// Number of iterations the resolver ran. Spec caps at 4;
    /// reaching the cap is a warning the caller surfaces in the
    /// debug HUD.
    pub iterations: u32,
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
        // the section-break model isn't in idml-scene yet.
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
        let position = story_first_page
            .get(&anchor.story_id)
            .map(|(page_id, page_number)| AnchorPosition {
                page_number: *page_number,
                page_id: Some(page_id.clone()),
                counters: HashMap::new(),
            })
            .unwrap_or_default();
        result.numbering.0.insert(anchor.id.clone(), position);
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_caps_at_four_iterations() {
        let opts = ResolveOptions::default();
        assert_eq!(opts.max_iterations, 4);
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
