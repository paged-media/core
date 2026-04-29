//! Style cascade dependency graph.
//!
//! M3 builds an index from `style_id → using_paragraphs / using_runs
//! / using_frames` so that editing a `ParagraphStyleDef`,
//! `CharacterStyleDef`, or `ObjectStyleDef` invalidates exactly the
//! nodes that reference it. The renderer pipeline already resolves
//! the cascade per-paragraph at emit time; this index exists for
//! Patch invalidation, not for layout — it keeps the M3 incremental
//! invariant ("editing a style invalidates every story that uses
//! it") cheap.
//!
//! The index is rebuilt whenever the document's structure or style
//! sheet changes. For typical IDML inputs (a few thousand nodes,
//! a few dozen styles) the rebuild is microseconds; M3+ adds diff-
//! based incremental refresh once we measure the cost on real
//! editorial documents.

use std::collections::{BTreeMap, BTreeSet};

use idml_scene::Document;

use crate::ids::{ParaId, StoryId};

/// Forward-index from a style id to the nodes that reference it.
#[derive(Debug, Default, Clone)]
pub struct StyleGraph {
    /// `ParagraphStyle/Foo` → set of `(story_id, paragraph_idx)`.
    pub paragraph_users: BTreeMap<String, BTreeSet<(String, u32)>>,
    /// `CharacterStyle/Foo` → set of `(story_id, paragraph_idx,
    /// run_idx)`. Run indexes are positional within the paragraph at
    /// rebuild time; they shift on text edits but are stable enough
    /// for invalidation purposes (an over-eager invalidation is fine).
    pub character_users: BTreeMap<String, BTreeSet<(String, u32, u32)>>,
    /// `ObjectStyle/Foo` → set of frame `Self` ids.
    pub object_users: BTreeMap<String, BTreeSet<String>>,
}

impl StyleGraph {
    /// Build a fresh index from the working document.
    pub fn from_document(doc: &Document) -> Self {
        let mut g = Self::default();
        for ps in &doc.stories {
            let story_id = ps.self_id.clone();
            for (pi, p) in ps.story.paragraphs.iter().enumerate() {
                if let Some(style) = p.paragraph_style.as_deref() {
                    g.paragraph_users
                        .entry(style.to_string())
                        .or_default()
                        .insert((story_id.clone(), pi as u32));
                }
                for (ri, r) in p.runs.iter().enumerate() {
                    if let Some(cs) = r.character_style.as_deref() {
                        g.character_users
                            .entry(cs.to_string())
                            .or_default()
                            .insert((story_id.clone(), pi as u32, ri as u32));
                    }
                }
            }
        }
        for spread in &doc.spreads {
            for f in &spread.spread.text_frames {
                if let (Some(self_id), Some(style)) =
                    (f.self_id.as_ref(), f.applied_object_style.as_ref())
                {
                    g.object_users
                        .entry(style.clone())
                        .or_default()
                        .insert(self_id.clone());
                }
            }
            for r in &spread.spread.rectangles {
                if let (Some(self_id), Some(style)) =
                    (r.self_id.as_ref(), r.applied_object_style.as_ref())
                {
                    g.object_users
                        .entry(style.clone())
                        .or_default()
                        .insert(self_id.clone());
                }
            }
        }
        g
    }

    pub fn paragraphs_using(&self, style_id: &str) -> impl Iterator<Item = (StoryId, ParaId)> + '_ {
        self.paragraph_users
            .get(style_id)
            .into_iter()
            .flat_map(|set| set.iter())
            .map(|(s, p)| (StoryId(s.clone()), ParaId(*p)))
    }

    pub fn frames_using(&self, style_id: &str) -> impl Iterator<Item = &str> + '_ {
        self.object_users
            .get(style_id)
            .into_iter()
            .flat_map(|set| set.iter())
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph_round_trips() {
        let g = StyleGraph::default();
        assert!(g.paragraphs_using("ParagraphStyle/Body").next().is_none());
        assert!(g.frames_using("ObjectStyle/None").next().is_none());
    }
}
