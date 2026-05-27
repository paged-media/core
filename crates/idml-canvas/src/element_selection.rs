//! Element-level selection model (application state).
//!
//! Distinct from `ContentSelection` (text caret / range): this is the
//! set of *page items* — text frames, rectangles, ovals, polygons,
//! graphic lines, and groups — the user has selected. Selection lives
//! in application state, never enters the Operation log, and Cmd-Z
//! never changes it.
//!
//! `ElementId`'s variants mirror `idml-mutate::NodeId` so a future
//! `From<ElementId> for NodeId` can bridge them when Phase B lands the
//! gesture → Operation pipeline. The duplication is deliberate: Phase A
//! avoids the cross-crate dep so element selection can ship before the
//! mutate-log bridge.

use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

/// Page item identifier the user can select. The String payload is the
/// item's `Self` id from the IDML XML.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum ElementId {
    TextFrame(String),
    Rectangle(String),
    Oval(String),
    Polygon(String),
    GraphicLine(String),
    Group(String),
}

impl ElementId {
    /// The bare `Self` id, regardless of kind. Useful when matching
    /// against parser/scene structures that only expose the string.
    pub fn raw_id(&self) -> &str {
        match self {
            ElementId::TextFrame(id)
            | ElementId::Rectangle(id)
            | ElementId::Oval(id)
            | ElementId::Polygon(id)
            | ElementId::GraphicLine(id)
            | ElementId::Group(id) => id,
        }
    }
}

/// How a `SetElementSelection` request combines with the current set.
/// Mirrors the canonical macOS / industry convention:
/// - `Replace` — plain click; selection becomes the request.
/// - `Add` — Shift-click; union (already-selected ids stay).
/// - `Toggle` — Cmd/Ctrl-click; ids already in the set are removed,
///   ids not in the set are added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub enum SelectionMode {
    Replace,
    Add,
    Toggle,
}

/// The application-state set of selected elements. Order is preserved
/// in selection order so the UI can render "primary" selection chrome
/// (e.g., the last-selected item) differently if it wants.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementSelection {
    pub ids: Vec<ElementId>,
}

impl ElementSelection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn contains(&self, id: &ElementId) -> bool {
        self.ids.iter().any(|i| i == id)
    }

    pub fn clear(&mut self) {
        self.ids.clear();
    }

    /// Replace the entire selection with `ids`. Duplicates in the
    /// input are kept in first-occurrence order.
    pub fn set(&mut self, ids: Vec<ElementId>) {
        let mut out: Vec<ElementId> = Vec::with_capacity(ids.len());
        for id in ids {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        self.ids = out;
    }

    /// Add `id` if not already present.
    pub fn add(&mut self, id: ElementId) {
        if !self.contains(&id) {
            self.ids.push(id);
        }
    }

    /// Add `id` if absent, remove it if present.
    pub fn toggle(&mut self, id: ElementId) {
        if let Some(pos) = self.ids.iter().position(|i| i == &id) {
            self.ids.remove(pos);
        } else {
            self.ids.push(id);
        }
    }

    /// Remove `id` if present; no-op otherwise.
    pub fn remove(&mut self, id: &ElementId) {
        if let Some(pos) = self.ids.iter().position(|i| i == id) {
            self.ids.remove(pos);
        }
    }

    /// Apply a `SelectionMode` against the current set with the given
    /// request ids.
    pub fn apply_mode(&mut self, ids: &[ElementId], mode: SelectionMode) {
        match mode {
            SelectionMode::Replace => self.set(ids.to_vec()),
            SelectionMode::Add => {
                for id in ids {
                    self.add(id.clone());
                }
            }
            SelectionMode::Toggle => {
                for id in ids {
                    self.toggle(id.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tf(id: &str) -> ElementId {
        ElementId::TextFrame(id.to_string())
    }
    fn rect(id: &str) -> ElementId {
        ElementId::Rectangle(id.to_string())
    }

    #[test]
    fn empty_by_default() {
        let s = ElementSelection::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn add_dedupes() {
        let mut s = ElementSelection::new();
        s.add(tf("a"));
        s.add(tf("a"));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn toggle_round_trips() {
        let mut s = ElementSelection::new();
        s.toggle(tf("a"));
        assert!(s.contains(&tf("a")));
        s.toggle(tf("a"));
        assert!(!s.contains(&tf("a")));
    }

    #[test]
    fn replace_resets() {
        let mut s = ElementSelection::new();
        s.add(tf("a"));
        s.add(tf("b"));
        s.apply_mode(&[rect("c")], SelectionMode::Replace);
        assert_eq!(s.ids, vec![rect("c")]);
    }

    #[test]
    fn add_mode_unions() {
        let mut s = ElementSelection::new();
        s.add(tf("a"));
        s.apply_mode(&[tf("b"), tf("a")], SelectionMode::Add);
        assert_eq!(s.ids, vec![tf("a"), tf("b")]);
    }

    #[test]
    fn toggle_mode_xor() {
        let mut s = ElementSelection::new();
        s.add(tf("a"));
        s.apply_mode(&[tf("a"), tf("b")], SelectionMode::Toggle);
        // a was present → removed; b was absent → added
        assert_eq!(s.ids, vec![tf("b")]);
    }

    #[test]
    fn set_dedupes_input() {
        let mut s = ElementSelection::new();
        s.set(vec![tf("a"), tf("a"), tf("b")]);
        assert_eq!(s.ids, vec![tf("a"), tf("b")]);
    }

    #[test]
    fn raw_id_unwraps_variant() {
        assert_eq!(tf("uXYZ").raw_id(), "uXYZ");
        assert_eq!(rect("uABC").raw_id(), "uABC");
        assert_eq!(ElementId::Group("g1".to_string()).raw_id(), "g1");
    }

    #[test]
    fn variants_with_same_id_are_distinct() {
        let mut s = ElementSelection::new();
        s.add(tf("u1"));
        s.add(rect("u1"));
        assert_eq!(s.len(), 2);
        assert!(s.contains(&tf("u1")));
        assert!(s.contains(&rect("u1")));
    }
}
