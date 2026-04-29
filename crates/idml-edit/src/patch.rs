//! Invalidation deltas returned by `Project::apply`.
//!
//! A `Patch` is a structured description of what changed: which nodes,
//! and at what level the change occurred. The incremental render
//! pipeline (M1+) reads these to evict exactly the right caches —
//! resolved styles, shaped runs, composed paragraphs, frame-chain
//! layout, per-page display lists. M0 generates empty patches; the
//! shape is the contract that downstream code depends on.

use serde::{Deserialize, Serialize};

use crate::ids::NodeId;

/// What kind of invalidation a patch entry represents. Coarse enough
/// that selectors can subscribe by kind; fine enough that the
/// pipeline knows which cache layers to drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvalidationKind {
    /// Geometry changed — frame moved/resized/rotated, shape edited.
    /// Drops display-list commands referencing the node and the page
    /// R-tree the node lives on. Doesn't drop shapes or compositions.
    Geometry,
    /// Paint / stroke / opacity changed without metric impact. Drops
    /// only the affected display-list commands; nothing upstream.
    Appearance,
    /// Text content changed. Drops shaped runs for the affected runs,
    /// composed paragraphs, frame-chain layout for the parent story.
    TextContent,
    /// Run-level attribute changed (font, size, tracking, …). Drops
    /// shaped runs and downstream composition.
    RunAttrs,
    /// Paragraph-level attribute changed (alignment, indents, …).
    /// Drops composed paragraphs and downstream layout.
    ParagraphAttrs,
    /// Style sheet changed. Drops every cache keyed by the affected
    /// style id; the dependency graph (M3) makes this surgical.
    StyleSheet,
    /// Document structure changed (page added, frame added/removed,
    /// thread linked/unlinked). Page-level cache must drop.
    Structure,
}

/// A patch entry: which node, what kind. The pipeline groups entries
/// by `kind` for efficient eviction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatchEntry {
    pub node: NodeId,
    pub kind: InvalidationKind,
}

/// Aggregated patch returned by every `apply`. `epoch` is the project's
/// version after this command applied; consumers compare it to their
/// last-seen epoch to short-circuit redundant work.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Patch {
    pub epoch: u64,
    pub entries: Vec<PatchEntry>,
}

impl Patch {
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, node: NodeId, kind: InvalidationKind) {
        self.entries.push(PatchEntry { node, kind });
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn touches_styles(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.kind == InvalidationKind::StyleSheet)
    }

    pub fn touches_structure(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.kind == InvalidationKind::Structure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{NodeId, ParaId, StoryId};

    #[test]
    fn empty_patch_round_trips() {
        let p = Patch::new(0);
        let s = serde_json::to_string(&p).unwrap();
        let back: Patch = serde_json::from_str(&s).unwrap();
        assert_eq!(p.epoch, back.epoch);
        assert!(back.is_empty());
    }

    #[test]
    fn patch_classifies_entries() {
        let mut p = Patch::new(7);
        p.push(NodeId::Frame("f".into()), InvalidationKind::Geometry);
        p.push(
            NodeId::Para(StoryId("s".into()), ParaId(0)),
            InvalidationKind::ParagraphAttrs,
        );
        assert!(!p.is_empty());
        assert!(!p.touches_styles());
        assert!(!p.touches_structure());
    }
}
