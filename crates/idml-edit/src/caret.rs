//! Caret + selection model.
//!
//! M2 ships the logical model in Rust + a simple bridge surface for
//! the editor. Glyph-precise pixel ↔ byte-offset mapping (which
//! requires walking the laid-out lines from `idml-text`) lands as
//! part of M3 once the renderer hands out per-line metadata. For
//! M2 the type tool uses an end-of-paragraph caret by default and
//! the bridge exposes setters/getters so the front-end can navigate
//! and edit text with `InsertText` / `DeleteRange` commands.

use serde::{Deserialize, Serialize};

use crate::ids::{ParaId, StoryId};

/// A logical caret position: which story, which paragraph, and the
/// byte offset within that paragraph's concatenated text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretPos {
    pub story: StoryId,
    pub para: ParaId,
    pub byte_offset: u32,
}

impl CaretPos {
    pub fn new(story: StoryId, para: ParaId, byte_offset: u32) -> Self {
        Self {
            story,
            para,
            byte_offset,
        }
    }
}

/// A directed selection. `anchor` stays where the gesture started;
/// `head` follows the cursor. A collapsed selection (anchor == head)
/// is just a caret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub anchor: CaretPos,
    pub head: CaretPos,
}

impl Selection {
    pub fn caret(pos: CaretPos) -> Self {
        Self {
            anchor: pos.clone(),
            head: pos,
        }
    }

    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }

    /// Within-paragraph byte range — `None` when anchor and head are
    /// in different paragraphs (cross-paragraph selection); the
    /// command layer uses multiple `DeleteRange`/`MergeParagraph`
    /// commands to handle those.
    pub fn within_paragraph(&self) -> Option<(StoryId, ParaId, u32, u32)> {
        if self.anchor.story != self.head.story || self.anchor.para != self.head.para {
            return None;
        }
        let from = self.anchor.byte_offset.min(self.head.byte_offset);
        let to = self.anchor.byte_offset.max(self.head.byte_offset);
        Some((self.anchor.story.clone(), self.anchor.para, from, to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(story: &str, para: u32, off: u32) -> CaretPos {
        CaretPos {
            story: StoryId(story.into()),
            para: ParaId(para),
            byte_offset: off,
        }
    }

    #[test]
    fn caret_is_collapsed_selection() {
        let s = Selection::caret(pos("u100", 0, 5));
        assert!(s.is_caret());
        let (_, _, from, to) = s.within_paragraph().unwrap();
        assert_eq!((from, to), (5, 5));
    }

    #[test]
    fn within_paragraph_normalises_direction() {
        let s = Selection {
            anchor: pos("u100", 0, 10),
            head: pos("u100", 0, 3),
        };
        let (_, _, from, to) = s.within_paragraph().unwrap();
        assert_eq!((from, to), (3, 10));
    }

    #[test]
    fn cross_paragraph_selection_returns_none() {
        let s = Selection {
            anchor: pos("u100", 0, 0),
            head: pos("u100", 1, 0),
        };
        assert!(s.within_paragraph().is_none());
    }
}
