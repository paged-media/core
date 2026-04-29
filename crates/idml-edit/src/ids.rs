//! Stable node identity for editor addressing.
//!
//! IDML elements that already carry a `Self` attribute (spreads,
//! pages, frames, shapes, stories, tables) reuse that string directly.
//! Paragraphs and runs have no IDML id, so the editor mints
//! `(StoryId, generation)` ids the first time they're addressed; the
//! generation counter lives on the rope's per-paragraph metadata and
//! is monotonic — ids are never reused.

use serde::{Deserialize, Serialize};

/// Story id is the parsed `Self` attribute on `<Story>`. Always present
/// because the manifest indexes stories by it; we surface it as a
/// strongly typed wrapper to keep `NodeId` cases readable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StoryId(pub String);

/// Paragraph id within a story. The first paragraph of a story is
/// `ParaId(0)` at open time; inserts/deletes mutate the rope's
/// metadata so addresses stay stable across edits within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParaId(pub u32);

/// Run id within a paragraph. Same generational semantics as `ParaId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub u32);

/// Editor-wide stable address for any node the user can act on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id")]
pub enum NodeId {
    Spread(String),
    Page(String),
    Frame(String),
    Shape(String),
    Story(StoryId),
    Para(StoryId, ParaId),
    Run(StoryId, ParaId, RunId),
    Table(String),
    Cell { table: String, row: u32, col: u32 },
}

impl NodeId {
    pub fn is_text(&self) -> bool {
        matches!(self, NodeId::Story(_) | NodeId::Para(..) | NodeId::Run(..))
    }

    /// The story this node lives in, if any. Useful for cache
    /// invalidation: editing a run invalidates everything keyed by
    /// the parent story's text.
    pub fn story(&self) -> Option<&StoryId> {
        match self {
            NodeId::Story(s) | NodeId::Para(s, _) | NodeId::Run(s, _, _) => Some(s),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_serializes_round_trip() {
        let n = NodeId::Run(StoryId("u123".into()), ParaId(2), RunId(0));
        let s = serde_json::to_string(&n).unwrap();
        let back: NodeId = serde_json::from_str(&s).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn frame_is_not_text() {
        assert!(!NodeId::Frame("f1".into()).is_text());
        assert!(NodeId::Story(StoryId("s1".into())).is_text());
    }
}
