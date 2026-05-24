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
//! contract is set here and consumed by `idml_mutate::text_index`
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

/// Canonical selection / caret. `start == end` is a caret;
/// `start < end` is a range. Endpoints are normalised so `start ≤
/// end` always holds (use `Side` to recover direction information
/// elsewhere if needed).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentSelection {
    pub story_id: String,
    pub start: u32,
    pub end: u32,
    /// Downstream affinity bit. See module docs.
    #[serde(default)]
    pub affinity: bool,
}

impl ContentSelection {
    /// A caret at `offset` in `story_id`. Default affinity (upstream).
    pub fn caret(story_id: impl Into<String>, offset: u32) -> Self {
        Self {
            story_id: story_id.into(),
            start: offset,
            end: offset,
            affinity: false,
        }
    }

    /// A range selection. Auto-normalised so `start ≤ end`.
    pub fn range(story_id: impl Into<String>, a: u32, b: u32) -> Self {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        Self {
            story_id: story_id.into(),
            start,
            end,
            affinity: false,
        }
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
    /// Selections in a different story are returned unchanged.
    pub fn shift_for_insert(mut self, at_story: &str, at: u32, len: u32) -> Self {
        if self.story_id != at_story || len == 0 {
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
    pub fn shift_for_delete(mut self, at_story: &str, del_start: u32, del_end: u32) -> Self {
        if self.story_id != at_story || del_end <= del_start {
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
        let after = sel.shift_for_insert("s", 4, 3);
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
        let after = caret.shift_for_insert("s", 5, 3);
        assert_eq!(after.start, 8);
        assert_eq!(after.end, 8);
    }

    #[test]
    fn shift_for_insert_before_endpoint_skipped() {
        let sel = ContentSelection::range("s", 5, 8);
        let after = sel.shift_for_insert("s", 10, 4);
        assert_eq!(after.start, 5);
        assert_eq!(after.end, 8);
    }

    #[test]
    fn shift_for_insert_different_story_noop() {
        let sel = ContentSelection::range("s", 5, 8);
        let after = sel.clone().shift_for_insert("other", 0, 100);
        assert_eq!(after, sel);
    }

    #[test]
    fn shift_for_delete_collapses_inside_span() {
        let sel = ContentSelection::range("s", 6, 12);
        // Delete [4, 10) — start (6) falls inside → collapse to 4;
        // end (12) past span → shifts left by 6.
        let after = sel.shift_for_delete("s", 4, 10);
        assert_eq!(after.start, 4);
        assert_eq!(after.end, 6);
    }

    #[test]
    fn shift_for_delete_endpoints_outside_unchanged() {
        let sel = ContentSelection::range("s", 2, 4);
        let after = sel.shift_for_delete("s", 10, 15);
        assert_eq!(after.start, 2);
        assert_eq!(after.end, 4);
    }

    #[test]
    fn shift_for_delete_endpoints_after_span_shift_left() {
        let sel = ContentSelection::range("s", 15, 20);
        let after = sel.shift_for_delete("s", 5, 10);
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
