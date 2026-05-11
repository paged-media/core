//! Command bus surface.
//!
//! Every mutation goes through `Project::apply(Command) -> Patch`. The
//! enum is the single source of truth for the editor's mutation
//! vocabulary; `ts-rs` will eventually mirror it into TypeScript so
//! the bridge stays typed end-to-end.

use idml_parse::Justification;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ids::{NodeId, ParaId, StoryId};

/// Run-attribute key + value pairs for `SetRunAttr`. Each variant
/// covers one IDML attribute on `CharacterRun`. Coarse on purpose —
/// the editor's vocabulary stays small even as IDML grows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "key", content = "value")]
pub enum RunAttrPatch {
    Font(Option<String>),
    FontStyle(Option<String>),
    PointSize(Option<f32>),
    FillColor(Option<String>),
    FillTint(Option<f32>),
    Tracking(Option<f32>),
    BaselineShift(Option<f32>),
    Capitalization(Option<String>),
    Underline(Option<bool>),
    Strikethru(Option<bool>),
    /// `AppliedCharacterStyle` reference, e.g. `CharacterStyle/Caption`.
    CharacterStyle(Option<String>),
}

/// Paragraph-attribute key + value for `SetParagraphAttr`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "key", content = "value")]
pub enum ParagraphAttrPatch {
    /// `Justification` as the typed IDML enum. The serde format is
    /// the raw IDML attribute string (`"LeftAlign"`, etc.), so the
    /// JSON wire payload from the bridge is byte-identical to the
    /// pre-enum world.
    Justification(Option<Justification>),
    FirstLineIndent(Option<f32>),
    SpaceBefore(Option<f32>),
    SpaceAfter(Option<f32>),
    ParagraphStyle(Option<String>),
}

/// Commands the editor can apply. Every variant is fully serialisable
/// so the bridge can ship them as JSON without bespoke marshalling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    /// Pure no-op. Used by the bridge smoke test and to flush undo
    /// coalescing windows. Always succeeds; produces an empty patch.
    Noop,

    /// Translate a frame's origin by `(dx, dy)` in pt. The translation
    /// composes onto the frame's `ItemTransform` rather than into its
    /// `Bounds` so the geometry stays in the frame's local coords (as
    /// IDML stores it). Transient commands (drag in flight) bypass the
    /// undo stack; the pointer-up commit lands as a single non-
    /// transient command summarising the gesture.
    MoveFrame {
        frame: NodeId,
        dx_pt: f32,
        dy_pt: f32,
        transient: bool,
    },

    /// Set a frame's outer-spread bounds in pt. Used by the inspector
    /// numeric fields and resize handles. The new bounds are in
    /// *spread* coordinates after `ItemTransform` has been applied;
    /// `Project::apply` decomposes them back into the frame's local
    /// `Bounds` + a translation on `ItemTransform`.
    SetFrameBounds {
        frame: NodeId,
        x_pt: f32,
        y_pt: f32,
        w_pt: f32,
        h_pt: f32,
        transient: bool,
    },

    /// Z-order: bring the frame to the front of its spread (last in
    /// draw order). Mirror command `SendFrameToBack`.
    BringFrameToFront {
        frame: NodeId,
    },
    SendFrameToBack {
        frame: NodeId,
    },

    /// Delete a frame from its spread.
    DeleteFrame {
        frame: NodeId,
    },

    /// Insert text at `(story, para, byte_offset)`. Used by the type
    /// tool on each keystroke. Sequential `InsertText` commands by
    /// the same caller within a 500 ms window coalesce into a single
    /// undo entry — `Project` does that based on a `coalesce` token.
    InsertText {
        story: StoryId,
        para: ParaId,
        byte_offset: u32,
        text: String,
        /// Coalesce key — same value across consecutive InsertTexts
        /// within a typing run. `None` forces a fresh undo entry.
        coalesce: Option<u32>,
    },

    /// Delete a byte range within a paragraph. `byte_to` is exclusive.
    /// Cross-paragraph deletes go through this command for the head
    /// paragraph plus a `MergeParagraph` and a tail-paragraph delete
    /// — the type tool composes them on Backspace at paragraph start.
    DeleteRange {
        story: StoryId,
        para: ParaId,
        byte_from: u32,
        byte_to: u32,
        coalesce: Option<u32>,
    },

    /// Replace a byte range with `text` in one shot. Equivalent to
    /// `DeleteRange` + `InsertText` but emits a single undo entry.
    ReplaceRange {
        story: StoryId,
        para: ParaId,
        byte_from: u32,
        byte_to: u32,
        text: String,
        coalesce: Option<u32>,
    },

    /// Set a run attribute over the byte range `[byte_from, byte_to)`.
    /// The rope splits runs as needed at the boundaries before
    /// applying the attribute change.
    SetRunAttr {
        story: StoryId,
        para: ParaId,
        byte_from: u32,
        byte_to: u32,
        attr: RunAttrPatch,
    },

    /// Set a paragraph attribute on a single paragraph. Multi-
    /// paragraph application goes through repeated commands.
    SetParagraphAttr {
        story: StoryId,
        para: ParaId,
        attr: ParagraphAttrPatch,
    },

    /// Split a paragraph at `byte_offset` — the head keeps bytes
    /// `[0, byte_offset)`, the new tail paragraph carries the rest
    /// and inherits the head's paragraph attributes.
    SplitParagraph {
        story: StoryId,
        para: ParaId,
        byte_offset: u32,
    },

    /// Merge `para` with the following paragraph. The combined
    /// paragraph keeps the leading paragraph's attributes; the
    /// trailing paragraph's runs concatenate after.
    MergeParagraph {
        story: StoryId,
        para: ParaId,
    },

    /// Link `from`'s `NextTextFrame` to `to`. If `from` already linked
    /// elsewhere the previous link is captured in the inverse so undo
    /// restores it.
    LinkFrames {
        from: NodeId,
        to: NodeId,
    },

    /// Clear `from`'s `NextTextFrame` link.
    UnlinkFrames {
        from: NodeId,
    },

    /// Apply an `ObjectStyle` to a frame. Pass `style` = `None` to
    /// clear the applied style.
    ApplyObjectStyle {
        frame: NodeId,
        style: Option<String>,
    },

    /// Set or clear the `applied_master` reference on a page.
    /// `master` of `None` removes the master overlay.
    ApplyMasterToPage {
        page: NodeId,
        master: Option<String>,
    },

    /// Toggle a layer's visibility flag. The renderer skips items
    /// whose `item_layer` references a hidden layer.
    SetLayerVisible {
        layer_id: String,
        visible: bool,
    },
    /// Toggle a layer's locked flag.
    SetLayerLocked {
        layer_id: String,
        locked: bool,
    },

    /// Set or clear the `LinkResourceURI` (`image_link`) on a
    /// rectangle frame so the renderer's asset resolver picks up the
    /// new image. M3 ships embed via `data:` URIs; OPFS-backed link
    /// mode (`idml:opfs/<sha256>`) is a post-M3 follow-up.
    PlaceImageInFrame {
        frame: NodeId,
        link_uri: Option<String>,
    },

    /// Set or clear `FillColor` and `StrokeColor` references plus
    /// `StrokeWeight` on a frame. Color references are swatch ids
    /// (e.g. `Color/Black`); the renderer resolves them through the
    /// document palette.
    SetFrameFill {
        frame: NodeId,
        color: Option<String>,
    },
    SetFrameStroke {
        frame: NodeId,
        color: Option<String>,
        weight_pt: Option<f32>,
    },

    /// Create a new rectangle frame at the end of `spread_idx`'s
    /// rectangle list. `self_id` of `None` makes the project mint a
    /// fresh id (`idml-edit-<counter>`); pass `Some(id)` if the
    /// caller has already chosen one (replays this from a saved
    /// project, for example). Returns the new frame's id via the
    /// patch's first entry.
    CreateRectangle {
        spread_idx: u32,
        self_id: Option<String>,
        bounds: RectanglePayloadBounds,
        item_transform: Option<[f32; 6]>,
        fill_color: Option<String>,
        stroke_color: Option<String>,
        stroke_weight: Option<f32>,
        applied_object_style: Option<String>,
        image_link: Option<String>,
    },
}

/// Carries `idml_parse::Bounds` in command payloads. We don't derive
/// serde directly on `idml_parse::Bounds` to avoid a foreign-impl
/// dependency; this small mirror is all the editor needs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RectanglePayloadBounds {
    pub top: f32,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
}

impl Command {
    pub fn is_transient(&self) -> bool {
        match self {
            Command::Noop => false,
            Command::MoveFrame { transient, .. } | Command::SetFrameBounds { transient, .. } => {
                *transient
            }
            Command::BringFrameToFront { .. }
            | Command::SendFrameToBack { .. }
            | Command::DeleteFrame { .. }
            | Command::InsertText { .. }
            | Command::DeleteRange { .. }
            | Command::ReplaceRange { .. }
            | Command::SetRunAttr { .. }
            | Command::SetParagraphAttr { .. }
            | Command::SplitParagraph { .. }
            | Command::MergeParagraph { .. }
            | Command::LinkFrames { .. }
            | Command::UnlinkFrames { .. }
            | Command::ApplyObjectStyle { .. }
            | Command::ApplyMasterToPage { .. }
            | Command::SetLayerVisible { .. }
            | Command::SetLayerLocked { .. }
            | Command::PlaceImageInFrame { .. }
            | Command::SetFrameFill { .. }
            | Command::SetFrameStroke { .. }
            | Command::CreateRectangle { .. } => false,
        }
    }

    /// Whether the command goes on the undo stack.
    pub fn is_undoable(&self) -> bool {
        match self {
            Command::Noop => false,
            _ if self.is_transient() => false,
            _ => true,
        }
    }

    /// Coalesce key, if this command participates in undo coalescing.
    /// Two consecutive commands with the same `Some(key)` collapse
    /// into one undo entry; `None` forces a fresh entry.
    pub fn coalesce_key(&self) -> Option<u32> {
        match self {
            Command::InsertText { coalesce, .. }
            | Command::DeleteRange { coalesce, .. }
            | Command::ReplaceRange { coalesce, .. } => *coalesce,
            _ => None,
        }
    }
}

/// Errors a command can fail with.
#[derive(Debug, Error)]
pub enum EditError {
    #[error("node {0:?} not found")]
    NodeNotFound(NodeId),

    #[error("command targets a non-frame node: {0:?}")]
    WrongNodeKind(NodeId),

    #[error("command not yet implemented in this milestone")]
    NotImplemented,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::NodeId;

    #[test]
    fn noop_is_not_undoable() {
        assert!(!Command::Noop.is_undoable());
    }

    #[test]
    fn transient_move_is_not_undoable() {
        let c = Command::MoveFrame {
            frame: NodeId::Frame("f1".into()),
            dx_pt: 10.0,
            dy_pt: 0.0,
            transient: true,
        };
        assert!(c.is_transient());
        assert!(!c.is_undoable());
    }

    #[test]
    fn committed_move_is_undoable() {
        let c = Command::MoveFrame {
            frame: NodeId::Frame("f1".into()),
            dx_pt: 10.0,
            dy_pt: 0.0,
            transient: false,
        };
        assert!(!c.is_transient());
        assert!(c.is_undoable());
    }

    #[test]
    fn delete_is_undoable_and_not_transient() {
        let c = Command::DeleteFrame {
            frame: NodeId::Frame("f1".into()),
        };
        assert!(!c.is_transient());
        assert!(c.is_undoable());
    }

    #[test]
    fn command_round_trips_json() {
        let c = Command::MoveFrame {
            frame: NodeId::Frame("f1".into()),
            dx_pt: 1.5,
            dy_pt: -2.0,
            transient: false,
        };
        let s = serde_json::to_string(&c).unwrap();
        let back: Command = serde_json::from_str(&s).unwrap();
        assert_eq!(c, back);
    }
}
