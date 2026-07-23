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

//! Content-addressed selection.
//!
//! Phase 3 correctness layer (Item 1).
//!
//! `ContentSelection { story_id, start, end, affinity }` is the
//! canonical selection state. It is *content-addressed* — the
//! triple references characters in a story, not pixels on a page —
//! so it survives re-layout, zoom changes, frame moves, and
//! pagination shifts (AC-E-9). The view layer derives caret +
//! selection rectangles from this primitive via separate worker
//! queries (Items 3 and 4).
//!
//! ### Story-offset contract
//!
//! Story-local byte offsets count the byte length of each paragraph's
//! concatenated run text, **plus one synthetic `\n` per inter-
//! paragraph boundary**. Mirrors text-editor intuition and makes
//! paragraph-boundary positions unambiguous up to *affinity*. This
//! contract is set here and consumed by `paged_mutate::text_index`
//! (Item 5) and `BuiltDocument::story_layout` (Item A).
//!
//! ### Affinity
//!
//! At an end-of-line / start-of-next-line boundary, the byte offset
//! is identical but the visual caret position differs. The
//! `affinity: bool` field disambiguates:
//!
//! - `false` (default, "upstream") — caret displays at the end of
//!   the line. Set by left-arrow at line start or by clicking at
//!   the end of a line.
//! - `true` ("downstream") — caret displays at the start of the
//!   next line. Set by right-arrow at line end.
//!
//! Mirrors Cocoa's `NSSelectionAffinity` / the Web's
//! `Selection.modify("extend", "forward", "character")` rule.

use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

/// W1.13 — cell qualifier for a content address that points INTO a
/// table cell rather than the story's main paragraph flow.
///
/// ## The two-stream addressing model
///
/// Table-cell text is stored out of band on `Table.cells[].paragraphs`
/// (see `idml_import`), disjoint from `Story.paragraphs`. So a content
/// address needs to say *which* paragraph stream its byte offsets index:
///
/// - `ContentSelection.cell == None` — offsets are story-local bytes
///   over `story.paragraphs` (the body flow). Unchanged from before.
/// - `ContentSelection.cell == Some(addr)` — offsets are CELL-LOCAL
///   bytes over `cell.paragraphs`, under the same story-offset contract
///   (run bytes + one synthetic `\n` per inter-paragraph boundary,
///   counted within the cell). The owning story is still `story_id`;
///   `addr` picks the cell within that story's table.
///
/// `table_id` / `row` / `col` are the SAME identifiers the hit-test
/// surface emits (`HitResult.table_context` / `TableHitContext`) and
/// that the renderer stamps onto cell `LineLayout`s
/// (`paged_renderer::CellAddr`), so a hit that lands in a cell hands
/// back exactly the qualifier the caret/edit address needs — no second
/// query.
///
/// ## Why a qualifier and not a re-numbered flat offset
///
/// The alternative — fold cells into one flat story-offset space via a
/// reserved high-bit/region scheme — was rejected: it makes
/// `shift_for_insert`/`shift_for_delete`, undo inverse offsets, and the
/// existing body-only consumers (BreakRecord, the A/B harness, every
/// `RequestWordBounds`/`RequestLineBounds` caller) all have to learn the
/// encoding, and a single arithmetic slip silently routes an edit into
/// the wrong cell. The qualifier keeps body addressing byte-identical
/// (the field defaults to `None` and is `#[serde(default)]`, so it
/// rides v35 additively — old senders omit it) and makes "which stream"
/// an explicit, type-checked decision. Undo is trivially correct
/// because the inverse op carries the same `cell` qualifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct TextCellAddr {
    /// `<Table Self="...">` id within `story_id`.
    pub table_id: String,
    /// Template row (0-based); span-origin row for spanned cells.
    pub row: u32,
    /// Column (0-based); span-origin column for spanned cells.
    pub col: u32,
}

/// Canonical selection / caret. `start == end` is a caret;
/// `start < end` is a range. Endpoints are normalised so `start ≤
/// end` always holds (use `Side` to recover direction information
/// elsewhere if needed).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct ContentSelection {
    pub story_id: String,
    pub start: u32,
    pub end: u32,
    /// Downstream affinity bit. See module docs.
    #[serde(default)]
    pub affinity: bool,
    /// W1.13 — cell qualifier. `None` (default) ⇒ `start`/`end` are
    /// story-local body offsets. `Some(addr)` ⇒ they are cell-local
    /// offsets into `addr`'s cell. Rides v35 additively. See
    /// [`TextCellAddr`].
    #[serde(default)]
    pub cell: Option<TextCellAddr>,
}

impl ContentSelection {
    /// A caret at `offset` in `story_id`'s body flow. Default affinity
    /// (upstream).
    pub fn caret(story_id: impl Into<String>, offset: u32) -> Self {
        Self {
            story_id: story_id.into(),
            start: offset,
            end: offset,
            affinity: false,
            cell: None,
        }
    }

    /// A range selection in `story_id`'s body flow. Auto-normalised so
    /// `start ≤ end`.
    pub fn range(story_id: impl Into<String>, a: u32, b: u32) -> Self {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        Self {
            story_id: story_id.into(),
            start,
            end,
            affinity: false,
            cell: None,
        }
    }

    /// W1.13 — a caret at cell-local `offset` inside the cell named by
    /// `addr` of `story_id`'s table.
    pub fn cell_caret(story_id: impl Into<String>, addr: TextCellAddr, offset: u32) -> Self {
        Self {
            story_id: story_id.into(),
            start: offset,
            end: offset,
            affinity: false,
            cell: Some(addr),
        }
    }

    /// W1.13 — attach a cell qualifier to a selection (builder).
    pub fn with_cell(mut self, cell: Option<TextCellAddr>) -> Self {
        self.cell = cell;
        self
    }

    /// W1.13 — true when `self` and `other_cell` address the SAME
    /// stream of the SAME story (both body, or both the same cell).
    fn same_stream(&self, other_story: &str, other_cell: &Option<TextCellAddr>) -> bool {
        self.story_id == other_story && &self.cell == other_cell
    }

    pub fn is_caret(&self) -> bool {
        self.start == self.end
    }

    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.is_caret()
    }

    /// Selection with explicit affinity. Used at line boundaries.
    pub fn with_affinity(mut self, affinity: bool) -> Self {
        self.affinity = affinity;
        self
    }

    /// Anchor offset for one side. For range selections the caller
    /// uses this to track the "fixed end" while drag-extending the
    /// other end.
    pub fn anchor(&self, side: Side) -> u32 {
        match side {
            Side::Start => self.start,
            Side::End => self.end,
        }
    }

    /// Apply an `InsertText`-style delta to the selection: characters
    /// inserted at `at` push later endpoints right by `len`. Mirrors
    /// the rule from the spec §6 Worked Example A: an insert at
    /// offset O shifts any endpoint ≥ O right by len.
    ///
    /// Selections in a different stream (different story OR different
    /// cell qualifier) are returned unchanged — an edit inside one cell
    /// never moves a caret in the body flow or in a sibling cell, since
    /// their offset spaces are disjoint (W1.13).
    pub fn shift_for_insert(
        mut self,
        at_story: &str,
        at_cell: &Option<TextCellAddr>,
        at: u32,
        len: u32,
    ) -> Self {
        if !self.same_stream(at_story, at_cell) || len == 0 {
            return self;
        }
        if self.start >= at {
            self.start += len;
        }
        if self.end >= at {
            self.end += len;
        }
        self
    }

    /// Apply a `DeleteRange`-style delta `[del_start, del_end)`.
    /// Endpoints that fall inside the deleted span collapse to
    /// `del_start`; endpoints past the span shift left by the
    /// deleted length.
    pub fn shift_for_delete(
        mut self,
        at_story: &str,
        at_cell: &Option<TextCellAddr>,
        del_start: u32,
        del_end: u32,
    ) -> Self {
        if !self.same_stream(at_story, at_cell) || del_end <= del_start {
            return self;
        }
        let len = del_end - del_start;
        self.start = shift_endpoint_for_delete(self.start, del_start, del_end, len);
        self.end = shift_endpoint_for_delete(self.end, del_start, del_end, len);
        // After shift the endpoints may have crossed; renormalise.
        if self.start > self.end {
            std::mem::swap(&mut self.start, &mut self.end);
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Start,
    End,
}

fn shift_endpoint_for_delete(p: u32, del_start: u32, del_end: u32, len: u32) -> u32 {
    if p <= del_start {
        p
    } else if p < del_end {
        del_start
    } else {
        p - len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_roundtrips_through_json() {
        let c = ContentSelection::caret("story1", 42);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"storyId\":\"story1\""), "{json}");
        assert!(json.contains("\"start\":42"), "{json}");
        assert!(json.contains("\"end\":42"), "{json}");
        let back: ContentSelection = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        assert!(back.is_caret());
    }

    #[test]
    fn range_normalises_endpoints() {
        let r = ContentSelection::range("s", 10, 4);
        assert_eq!(r.start, 4);
        assert_eq!(r.end, 10);
        assert_eq!(r.len(), 6);
        assert!(!r.is_caret());
    }

    #[test]
    fn shift_for_insert_pushes_later_endpoints() {
        let sel = ContentSelection::range("s", 5, 8);
        let after = sel.shift_for_insert("s", &None, 4, 3);
        assert_eq!(after.start, 8);
        assert_eq!(after.end, 11);
    }

    #[test]
    fn shift_for_insert_at_endpoint_pushes_endpoint() {
        // Insert at the caret offset: spec §6 says endpoint ≥ at
        // shifts right. So a caret at 5, with insert at 5 of length
        // 3, becomes a caret at 8 (the inserted text is on the
        // *left* of the caret).
        let caret = ContentSelection::caret("s", 5);
        let after = caret.shift_for_insert("s", &None, 5, 3);
        assert_eq!(after.start, 8);
        assert_eq!(after.end, 8);
    }

    #[test]
    fn shift_for_insert_before_endpoint_skipped() {
        let sel = ContentSelection::range("s", 5, 8);
        let after = sel.shift_for_insert("s", &None, 10, 4);
        assert_eq!(after.start, 5);
        assert_eq!(after.end, 8);
    }

    #[test]
    fn shift_for_insert_different_story_noop() {
        let sel = ContentSelection::range("s", 5, 8);
        let after = sel.clone().shift_for_insert("other", &None, 0, 100);
        assert_eq!(after, sel);
    }

    #[test]
    fn shift_for_delete_collapses_inside_span() {
        let sel = ContentSelection::range("s", 6, 12);
        // Delete [4, 10) — start (6) falls inside → collapse to 4;
        // end (12) past span → shifts left by 6.
        let after = sel.shift_for_delete("s", &None, 4, 10);
        assert_eq!(after.start, 4);
        assert_eq!(after.end, 6);
    }

    #[test]
    fn shift_for_delete_endpoints_outside_unchanged() {
        let sel = ContentSelection::range("s", 2, 4);
        let after = sel.shift_for_delete("s", &None, 10, 15);
        assert_eq!(after.start, 2);
        assert_eq!(after.end, 4);
    }

    #[test]
    fn shift_for_delete_endpoints_after_span_shift_left() {
        let sel = ContentSelection::range("s", 15, 20);
        let after = sel.shift_for_delete("s", &None, 5, 10);
        assert_eq!(after.start, 10);
        assert_eq!(after.end, 15);
    }

    #[test]
    fn affinity_is_preserved_through_serde() {
        let s = ContentSelection::caret("s", 10).with_affinity(true);
        let json = serde_json::to_string(&s).unwrap();
        let back: ContentSelection = serde_json::from_str(&json).unwrap();
        assert!(back.affinity);
    }
}
