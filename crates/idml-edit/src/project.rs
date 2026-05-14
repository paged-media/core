//! `Project` — the editable document.
//!
//! Wraps the parsed `idml_scene::Document` with a working copy that
//! commands mutate, and an undo/redo stack of inverse-command entries
//! that capture whatever previous state is needed to invert each
//! command. The base `Arc<Document>` stays around so a future export
//! pass can compare against the original.

use std::collections::HashMap;
use std::sync::Arc;

use idml_parse::{GraphicLine, Oval, Polygon, Rectangle, TextFrame};
use idml_scene::Document;

use crate::command::{
    Command, EditError, ParagraphAttrPatch, RectanglePayloadBounds, RunAttrPatch,
};
use crate::ids::{NodeId, ParaId, StoryId};
use crate::patch::{InvalidationKind, Patch};
use crate::rope::{ParagraphAttrs, ParagraphRope, RunAttrs, RunSeg, StoryRope};
use crate::style_graph::StyleGraph;

/// Snapshot of a rectangle frame, suitable for clipboard / duplicate
/// payloads. The frontend turns this into a `CreateRectangle`
/// command (typically with an offset on `item_transform`).
#[derive(Debug, Clone)]
pub struct RectanglePayloadSnapshot {
    pub spread_idx: usize,
    pub bounds: RectanglePayloadBounds,
    pub item_transform: Option<[f32; 6]>,
    pub fill_color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
    pub applied_object_style: Option<String>,
    pub image_link: Option<String>,
}

/// Lightweight stats the bridge surfaces to the UI for the loading /
/// document-info displays. Cheaper than `parse_summary` because the
/// `Document` is already parsed.
#[derive(Debug, Default, Clone)]
pub struct ProjectStats {
    pub spreads: usize,
    pub stories: usize,
    pub master_spreads: usize,
    pub text_frames: usize,
}

/// Internal undo entry. Captures whatever previous state is needed
/// to invert a forward command. Kept private to `Project` so the
/// public `Command` vocab stays minimal — UI never authors these.
#[derive(Debug, Clone)]
enum UndoEntry {
    /// Inverse of `MoveFrame { dx, dy }`.
    Translate {
        frame: String,
        dx_pt: f32,
        dy_pt: f32,
    },
    /// Inverse of `SetFrameBounds`. Captures the previous full
    /// transform + bounds for the frame.
    SetFrameTransform {
        frame: String,
        prev_item_transform: Option<[f32; 6]>,
        prev_bounds: idml_parse::Bounds,
    },
    /// Inverse of any z-order command. Restores a frame entry to a
    /// specific position in its spread's typed list.
    SetZ {
        spread_idx: usize,
        kind: FrameKind,
        from_idx: usize,
        to_idx: usize,
    },
    /// Inverse of `DeleteFrame` — restore the frame at its original
    /// position with its original payload.
    Restore {
        spread_idx: usize,
        kind: FrameKind,
        position: usize,
        payload: FramePayload,
    },
    /// Inverse of `InsertText` — delete the bytes the insert added.
    /// Carries the inserted text length so the inverse delete knows
    /// the exact range.
    DeleteInserted {
        story: StoryId,
        para: ParaId,
        byte_offset: u32,
        byte_len: u32,
        coalesce: Option<u32>,
    },
    /// Inverse of `DeleteRange` — re-insert the deleted bytes at the
    /// original position. The split into typed RunSegs is approximated
    /// (M2 dumps the whole undo into a single new run with default
    /// attrs at the seam — a future pass captures per-byte attribute
    /// runs for precise restoration).
    InsertDeleted {
        story: StoryId,
        para: ParaId,
        byte_offset: u32,
        text: String,
        coalesce: Option<u32>,
    },
    /// Inverse of `ReplaceRange` — restore the original text.
    InsertDeletedReplace {
        story: StoryId,
        para: ParaId,
        byte_offset: u32,
        prev_text: String,
        new_len: u32,
        coalesce: Option<u32>,
    },
    /// Inverse of `SetRunAttr` — restore each affected run's previous
    /// attribute. We capture a list of `(byte_from, byte_to,
    /// previous RunAttrs snapshot)` because the apply path may have
    /// merged runs.
    RestoreRunAttrs {
        story: StoryId,
        para: ParaId,
        previous: Vec<(u32, u32, RunAttrs)>,
    },
    /// Inverse of `SetParagraphAttr`.
    RestoreParagraphAttrs {
        story: StoryId,
        para: ParaId,
        prev: ParagraphAttrs,
    },
    /// Inverse of `SplitParagraph` — merge the two halves back.
    MergeAfterSplit {
        story: StoryId,
        para: ParaId,
    },
    /// Inverse of `MergeParagraph` — split at the recorded offset.
    SplitAfterMerge {
        story: StoryId,
        para: ParaId,
        byte_offset: u32,
        tail_attrs: ParagraphAttrs,
    },
    /// Inverse of `LinkFrames` / `UnlinkFrames` — restores the
    /// previous `NextTextFrame` value (None if there was no link).
    SetNextTextFrame {
        from: String,
        prev: Option<String>,
    },
    /// Inverse of `ApplyObjectStyle` — restores the previous
    /// `applied_object_style` reference.
    SetAppliedObjectStyle {
        frame: String,
        prev: Option<String>,
    },
    /// Inverse of `ApplyMasterToPage` — restores the previous
    /// `applied_master` value (None if there was no master).
    SetAppliedMaster {
        page: String,
        prev: Option<String>,
    },
    /// Inverse of layer-flag commands.
    SetLayerVisible {
        layer_id: String,
        prev: bool,
    },
    SetLayerLocked {
        layer_id: String,
        prev: bool,
    },
    /// Inverse of `PlaceImageInFrame`.
    SetImageLink {
        frame: String,
        prev: Option<String>,
    },
    /// Inverse of `SetFrameFill`.
    SetFrameFill {
        frame: String,
        prev: Option<String>,
    },
    /// Inverse of `SetFrameStroke`. Captures both `stroke_color` and
    /// `stroke_weight` so the inverse restores both atomically.
    SetFrameStroke {
        frame: String,
        prev_color: Option<String>,
        prev_weight: Option<f32>,
    },
    /// Inverse of `CreateRectangle` — delete the rectangle that was
    /// just inserted at `position` in `spread_idx`.
    DeleteCreated {
        spread_idx: usize,
        kind: FrameKind,
        position: usize,
    },
}

impl UndoEntry {
    /// Coalesce key for typing-coalescing. Only InsertText /
    /// DeleteRange / ReplaceRange participate.
    fn coalesce_key(&self) -> Option<u32> {
        match self {
            UndoEntry::DeleteInserted { coalesce, .. }
            | UndoEntry::InsertDeleted { coalesce, .. }
            | UndoEntry::InsertDeletedReplace { coalesce, .. } => *coalesce,
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Text,
    Rectangle,
    Oval,
    GraphicLine,
    Polygon,
}

#[derive(Debug, Clone)]
enum FramePayload {
    Text(Box<TextFrame>),
    Rectangle(Box<Rectangle>),
    Oval(Box<Oval>),
    GraphicLine(Box<GraphicLine>),
    Polygon(Box<Polygon>),
}

/// The editable document. `apply` mutates `doc` and pushes an
/// `UndoEntry` onto `undo`.
pub struct Project {
    base: Arc<Document>,
    doc: Document,
    /// Per-story rope, keyed by `Story.self_id` (the `StoryId`).
    /// Hydrated lazily on first text command targeting the story.
    /// Once a rope exists, it is the source of truth and the
    /// matching `Document::stories[i].story` is rebuilt from the
    /// rope after every text command.
    ropes: HashMap<String, StoryRope>,
    /// Map from story self_id → index into `Document::stories`. Built
    /// once at open and refreshed when stories list changes (M2 keeps
    /// it stable; M3 may add/remove).
    story_index: HashMap<String, usize>,
    /// Style cascade dependency graph. Rebuilt lazily when the patch
    /// stream signals structure or text-content changes.
    style_graph: StyleGraph,
    /// Set when the style_graph is known to be stale and must be
    /// rebuilt before the next read.
    style_graph_dirty: bool,
    /// Forward log of every committed (non-transient, non-noop)
    /// command applied to this project. Persistence saves this list
    /// alongside the original IDML bytes so reopening replays the
    /// exact same edit state.
    forward_log: Vec<Command>,
    /// Original IDML container bytes captured at `open` time. Used
    /// by the native-format serializer; stays empty for
    /// `from_document`-constructed projects unless filled in via
    /// `set_original_idml_bytes`.
    original_idml: Vec<u8>,
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    epoch: u64,
    /// Monotonic counter the project mints fresh `Self` ids from for
    /// CreateRectangle / paste-from-clipboard / etc. Persistence
    /// captures it via the forward command log; ids generated for
    /// session N stay stable across replay.
    id_counter: u64,
}

impl Project {
    /// Soft cap on the undo stack.
    pub const UNDO_CAP: usize = 1000;

    pub fn open(idml_bytes: &[u8]) -> Result<Self, idml_scene::OpenError> {
        let document = Document::open(idml_bytes)?;
        let mut project = Self::from_document(document);
        project.original_idml = idml_bytes.to_vec();
        Ok(project)
    }

    pub fn from_document(document: Document) -> Self {
        let base = Arc::new(document.clone());
        let story_index: HashMap<String, usize> = document
            .stories
            .iter()
            .enumerate()
            .map(|(i, s)| (s.self_id.clone(), i))
            .collect();
        let style_graph = StyleGraph::from_document(&document);
        Self {
            base,
            doc: document,
            ropes: HashMap::new(),
            story_index,
            style_graph,
            style_graph_dirty: false,
            forward_log: Vec::new(),
            original_idml: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            epoch: 0,
            id_counter: 0,
        }
    }

    fn mint_id(&mut self) -> String {
        self.id_counter += 1;
        format!("idml-edit-{}", self.id_counter)
    }

    /// Captured IDML bytes used by the native serializer.
    pub fn original_idml_bytes(&self) -> &[u8] {
        &self.original_idml
    }

    pub fn set_original_idml_bytes(&mut self, bytes: Vec<u8>) {
        self.original_idml = bytes;
    }

    /// Forward command log — every committed command applied since
    /// `open`. Used by `serialize_native`; not exposed to JS.
    pub fn forward_log(&self) -> &[Command] {
        &self.forward_log
    }

    /// Style dependency graph. Lazily rebuilt when the document
    /// structure has changed since the last access.
    pub fn style_graph(&mut self) -> &StyleGraph {
        if self.style_graph_dirty {
            self.style_graph = StyleGraph::from_document(&self.doc);
            self.style_graph_dirty = false;
        }
        &self.style_graph
    }

    /// Working document the pipeline reads from. The base stays
    /// available via `original` for comparison / export-of-original
    /// scenarios.
    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn original(&self) -> &Document {
        &self.base
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn stats(&self) -> ProjectStats {
        ProjectStats {
            spreads: self.doc.spreads.len(),
            stories: self.doc.stories.len(),
            master_spreads: self.doc.master_spreads.len(),
            text_frames: self.doc.text_frame_index.len(),
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Concatenated paragraph text as it lives in the working
    /// document. Returns `None` for unknown story/para indexes.
    pub fn paragraph_text(&self, story: &StoryId, para: ParaId) -> Option<String> {
        let idx = *self.story_index.get(&story.0)?;
        let p = self.doc.stories[idx]
            .story
            .paragraphs
            .get(para.0 as usize)?;
        Some(p.runs.iter().map(|r| r.text.as_str()).collect())
    }

    pub fn paragraph_count(&self, story: &StoryId) -> Option<usize> {
        let idx = *self.story_index.get(&story.0)?;
        Some(self.doc.stories[idx].story.paragraphs.len())
    }

    /// Snapshot of a paragraph's attributes (justification, indents,
    /// etc.) as it lives in the working document. Useful to populate
    /// the Paragraph inspector panel.
    pub fn paragraph_attrs(&self, story: &StoryId, para: ParaId) -> Option<ParagraphAttrs> {
        let idx = *self.story_index.get(&story.0)?;
        let p = self.doc.stories[idx]
            .story
            .paragraphs
            .get(para.0 as usize)?;
        Some(ParagraphAttrs {
            paragraph_style: p.paragraph_style.clone(),
            justification: p.justification,
            first_line_indent: p.first_line_indent,
            space_before: p.space_before,
            space_after: p.space_after,
            tab_list: p.tab_list.clone(),
        })
    }

    /// Run attrs of the first run of a paragraph — proxy for the
    /// caret's current run attributes when no selection is active.
    /// Returns `None` if the paragraph has no runs.
    pub fn first_run_attrs(&self, story: &StoryId, para: ParaId) -> Option<RunAttrs> {
        let idx = *self.story_index.get(&story.0)?;
        let p = self.doc.stories[idx]
            .story
            .paragraphs
            .get(para.0 as usize)?;
        let r = p.runs.first()?;
        Some(RunAttrs {
            character_style: r.character_style.clone(),
            font: r.font.clone(),
            font_style: r.font_style.clone(),
            point_size: r.point_size,
            fill_color: r.fill_color.clone(),
            fill_tint: r.fill_tint,
            capitalization: r.capitalization.clone(),
            baseline_shift: r.baseline_shift,
            horizontal_scale: r.horizontal_scale,
            vertical_scale: r.vertical_scale,
            skew: r.skew,
            position: r.position.clone(),
            tracking: r.tracking,
            underline: r.underline,
            strikethru: r.strikethru,
            leading: r.leading,
        })
    }

    /// Story id for the frame's parent_story (text frames only).
    pub fn parent_story_of_frame(&self, frame_self_id: &str) -> Option<StoryId> {
        let frame = self.doc.text_frame(frame_self_id)?;
        frame.parent_story.clone().map(StoryId)
    }

    /// Spread index containing `frame_self_id`, or None.
    pub fn spread_index_of_frame(&self, frame_self_id: &str) -> Option<usize> {
        self.locate_frame(frame_self_id).ok().map(|(s, _, _)| s)
    }

    /// Snapshot a rectangle frame's payload for clipboard / duplicate.
    /// Returns None if the id isn't a rectangle. Other frame kinds
    /// arrive once their CreateXxx commands ship.
    pub fn rectangle_payload(&self, frame_self_id: &str) -> Option<RectanglePayloadSnapshot> {
        let (spread_idx, kind, idx) = self.locate_frame(frame_self_id).ok()?;
        if kind != FrameKind::Rectangle {
            return None;
        }
        let r = &self.doc.spreads[spread_idx].spread.rectangles[idx];
        Some(RectanglePayloadSnapshot {
            spread_idx,
            bounds: RectanglePayloadBounds {
                top: r.bounds.top,
                left: r.bounds.left,
                bottom: r.bounds.bottom,
                right: r.bounds.right,
            },
            item_transform: r.item_transform,
            fill_color: r.fill_color.clone(),
            stroke_color: r.stroke_color.clone(),
            stroke_weight: r.stroke_weight,
            applied_object_style: r.applied_object_style.clone(),
            image_link: r.image_link.clone(),
        })
    }

    /// List of `(self_id, name)` for the document's paragraph styles.
    pub fn paragraph_style_list(&self) -> Vec<(String, String)> {
        self.doc
            .styles
            .paragraph_styles
            .values()
            .map(|s| {
                (
                    s.self_id.clone(),
                    s.name.clone().unwrap_or_else(|| s.self_id.clone()),
                )
            })
            .collect()
    }

    /// List of `(self_id, name)` for the document's character styles.
    pub fn character_style_list(&self) -> Vec<(String, String)> {
        self.doc
            .styles
            .character_styles
            .values()
            .map(|s| {
                (
                    s.self_id.clone(),
                    s.name.clone().unwrap_or_else(|| s.self_id.clone()),
                )
            })
            .collect()
    }

    /// List of `(self_id, name)` for the document's object styles.
    pub fn object_style_list(&self) -> Vec<(String, String)> {
        self.doc
            .styles
            .object_styles
            .values()
            .map(|s| {
                (
                    s.self_id.clone(),
                    s.name.clone().unwrap_or_else(|| s.self_id.clone()),
                )
            })
            .collect()
    }

    /// List of master spreads as `(self_id, label)`. The label uses
    /// the master's `name_prefix` + `base_name` when present, else
    /// the self id.
    pub fn master_spread_list(&self) -> Vec<(String, String)> {
        self.doc
            .master_spreads
            .iter()
            .map(|(id, ms)| {
                let label = ms.spread.self_id.clone().unwrap_or_else(|| id.clone());
                (id.clone(), label)
            })
            .collect()
    }

    /// Palette colours as `(self_id, name)`. Used by the Swatches
    /// panel to populate the click-to-apply list.
    pub fn swatch_list(&self) -> Vec<(String, String)> {
        self.doc
            .palette
            .colors
            .iter()
            .map(|(id, c)| (id.clone(), c.name.clone().unwrap_or_else(|| id.clone())))
            .collect()
    }

    /// List of layers as `(self_id, name, visible, locked)`.
    pub fn layer_list(&self) -> Vec<(String, String, bool, bool)> {
        self.doc
            .container
            .designmap
            .layers
            .iter()
            .map(|l| {
                (
                    l.self_id.clone(),
                    l.name.clone().unwrap_or_else(|| l.self_id.clone()),
                    l.visible,
                    l.locked,
                )
            })
            .collect()
    }

    /// List of body pages as `(page_self_id, applied_master)`. Useful
    /// for the Pages panel.
    pub fn page_list(&self) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        for ps in &self.doc.spreads {
            for p in &ps.spread.pages {
                if let Some(self_id) = p.self_id.clone() {
                    out.push((self_id, p.applied_master.clone()));
                }
            }
        }
        out
    }

    pub fn apply(&mut self, cmd: Command) -> Result<Patch, EditError> {
        let undo_entry = match &cmd {
            Command::Noop => None,
            Command::MoveFrame {
                frame,
                dx_pt,
                dy_pt,
                ..
            } => Some(self.do_move(frame, *dx_pt, *dy_pt)?),
            Command::SetFrameBounds {
                frame,
                x_pt,
                y_pt,
                w_pt,
                h_pt,
                ..
            } => Some(self.do_set_bounds(frame, *x_pt, *y_pt, *w_pt, *h_pt)?),
            Command::BringFrameToFront { frame } => {
                Some(self.do_z_order(frame, /* to_front= */ true)?)
            }
            Command::SendFrameToBack { frame } => {
                Some(self.do_z_order(frame, /* to_front= */ false)?)
            }
            Command::DeleteFrame { frame } => Some(self.do_delete(frame)?),
            Command::InsertText {
                story,
                para,
                byte_offset,
                text,
                coalesce,
            } => Some(self.do_insert_text(story, *para, *byte_offset, text, *coalesce)?),
            Command::DeleteRange {
                story,
                para,
                byte_from,
                byte_to,
                coalesce,
            } => Some(self.do_delete_range(story, *para, *byte_from, *byte_to, *coalesce)?),
            Command::ReplaceRange {
                story,
                para,
                byte_from,
                byte_to,
                text,
                coalesce,
            } => Some(self.do_replace_range(story, *para, *byte_from, *byte_to, text, *coalesce)?),
            Command::SetRunAttr {
                story,
                para,
                byte_from,
                byte_to,
                attr,
            } => Some(self.do_set_run_attr(story, *para, *byte_from, *byte_to, attr)?),
            Command::SetParagraphAttr { story, para, attr } => {
                Some(self.do_set_paragraph_attr(story, *para, attr)?)
            }
            Command::SplitParagraph {
                story,
                para,
                byte_offset,
            } => Some(self.do_split_paragraph(story, *para, *byte_offset)?),
            Command::MergeParagraph { story, para } => Some(self.do_merge_paragraph(story, *para)?),
            Command::LinkFrames { from, to } => Some(self.do_link_frames(from, Some(to))?),
            Command::UnlinkFrames { from } => Some(self.do_link_frames(from, None)?),
            Command::ApplyObjectStyle { frame, style } => {
                Some(self.do_apply_object_style(frame, style.clone())?)
            }
            Command::ApplyMasterToPage { page, master } => {
                Some(self.do_apply_master_to_page(page, master.clone())?)
            }
            Command::SetLayerVisible { layer_id, visible } => {
                Some(self.do_set_layer_visible(layer_id, *visible)?)
            }
            Command::SetLayerLocked { layer_id, locked } => {
                Some(self.do_set_layer_locked(layer_id, *locked)?)
            }
            Command::PlaceImageInFrame { frame, link_uri } => {
                Some(self.do_place_image(frame, link_uri.clone())?)
            }
            Command::SetFrameFill { frame, color } => {
                Some(self.do_set_frame_fill(frame, color.clone())?)
            }
            Command::SetFrameStroke {
                frame,
                color,
                weight_pt,
            } => Some(self.do_set_frame_stroke(frame, color.clone(), *weight_pt)?),
            Command::CreateRectangle {
                spread_idx,
                self_id,
                bounds,
                item_transform,
                fill_color,
                stroke_color,
                stroke_weight,
                applied_object_style,
                image_link,
            } => Some(self.do_create_rectangle(
                *spread_idx as usize,
                self_id.clone(),
                *bounds,
                *item_transform,
                fill_color.clone(),
                stroke_color.clone(),
                *stroke_weight,
                applied_object_style.clone(),
                image_link.clone(),
            )?),
        };

        self.epoch = self.epoch.wrapping_add(1);
        if cmd.is_undoable() {
            if let Some(entry) = undo_entry {
                self.push_undo(entry);
            }
            self.forward_log.push(cmd.clone());
        }

        let patch = self.patch_for(&cmd);
        if patch.touches_structure()
            || patch
                .entries
                .iter()
                .any(|e| matches!(e.kind, InvalidationKind::TextContent))
        {
            self.style_graph_dirty = true;
        }
        Ok(patch)
    }

    /// Push an undo entry, coalescing into the previous entry when
    /// the same coalesce key matches. M2 only coalesces InsertText
    /// runs where the new insert sits immediately after the previous
    /// one (the typing-forward case). Other patterns push fresh.
    fn push_undo(&mut self, entry: UndoEntry) {
        if let Some(k) = entry.coalesce_key() {
            if let Some(last) = self.undo.last_mut() {
                if last.coalesce_key() == Some(k) {
                    if let (
                        UndoEntry::DeleteInserted {
                            story: ls,
                            para: lp,
                            byte_offset: lo,
                            byte_len: ll,
                            ..
                        },
                        UndoEntry::DeleteInserted {
                            story: ns,
                            para: np,
                            byte_offset: no,
                            byte_len: nl,
                            ..
                        },
                    ) = (last, &entry)
                    {
                        if ls == ns && lp == np && *lo + *ll == *no {
                            *ll += *nl;
                            self.redo.clear();
                            return;
                        }
                    }
                }
            }
        }
        self.redo.clear();
        self.undo.push(entry);
        if self.undo.len() > Self::UNDO_CAP {
            self.undo.remove(0);
        }
    }

    pub fn undo(&mut self) -> Patch {
        if let Some(entry) = self.undo.pop() {
            if let Some(re) = self.invert(&entry) {
                self.redo.push(re);
            }
            self.epoch = self.epoch.wrapping_add(1);
        }
        Patch::new(self.epoch)
    }

    pub fn redo(&mut self) -> Patch {
        if let Some(entry) = self.redo.pop() {
            if let Some(un) = self.invert(&entry) {
                self.undo.push(un);
            }
            self.epoch = self.epoch.wrapping_add(1);
        }
        Patch::new(self.epoch)
    }

    // -----------------------------------------------------------------
    // Mutation primitives. Each returns the UndoEntry that, when
    // applied via `invert`, reverses the mutation.

    fn frame_id<'a>(&self, node: &'a NodeId) -> Result<&'a str, EditError> {
        match node {
            NodeId::Frame(s) => Ok(s.as_str()),
            other => Err(EditError::WrongNodeKind(other.clone())),
        }
    }

    fn do_move(&mut self, node: &NodeId, dx_pt: f32, dy_pt: f32) -> Result<UndoEntry, EditError> {
        let id = self.frame_id(node)?.to_string();
        let is_text;
        {
            let mut frame = self.find_frame_mut(&id)?;
            is_text = matches!(frame, FrameMutRef::Text { .. });
            translate_in_place(&mut frame, dx_pt, dy_pt);
        }
        if is_text {
            // Keep `frame_for_story` cache aligned with the mutation
            // so chained-text reflow downstream sees the new origin.
            self.refresh_frame_for_story_cache(&id);
        }
        Ok(UndoEntry::Translate {
            frame: id,
            dx_pt: -dx_pt,
            dy_pt: -dy_pt,
        })
    }

    fn do_set_bounds(
        &mut self,
        node: &NodeId,
        x_pt: f32,
        y_pt: f32,
        w_pt: f32,
        h_pt: f32,
    ) -> Result<UndoEntry, EditError> {
        let id = self.frame_id(node)?.to_string();
        let prev_xform;
        let prev_bounds;
        let is_text;
        {
            let mut frame = self.find_frame_mut(&id)?;
            prev_xform = frame.item_transform_clone();
            prev_bounds = frame.bounds_clone();
            is_text = matches!(frame, FrameMutRef::Text { .. });
            // Express the new spread-rect as: ItemTransform = identity-
            // translation to (x_pt, y_pt); bounds = (0,0)–(w,h). This
            // discards any rotation/skew the frame had — M1 numeric
            // setters always axis-align.
            frame.set_item_transform(Some([1.0, 0.0, 0.0, 1.0, x_pt, y_pt]));
            frame.set_bounds(idml_parse::Bounds {
                top: 0.0,
                left: 0.0,
                bottom: h_pt,
                right: w_pt,
            });
        }
        if is_text {
            self.refresh_frame_for_story_cache(&id);
        }
        Ok(UndoEntry::SetFrameTransform {
            frame: id,
            prev_item_transform: prev_xform,
            prev_bounds,
        })
    }

    fn do_z_order(&mut self, node: &NodeId, to_front: bool) -> Result<UndoEntry, EditError> {
        let id = self.frame_id(node)?;
        let (spread_idx, kind, from_idx) = self.locate_frame(id)?;
        let len = self.kind_len(spread_idx, kind);
        let to_idx = if to_front { len - 1 } else { 0 };
        if to_idx != from_idx {
            self.move_frame_in_kind(spread_idx, kind, from_idx, to_idx);
        }
        Ok(UndoEntry::SetZ {
            spread_idx,
            kind,
            from_idx: to_idx,
            to_idx: from_idx,
        })
    }

    fn do_delete(&mut self, node: &NodeId) -> Result<UndoEntry, EditError> {
        let id = self.frame_id(node)?;
        let (spread_idx, kind, position) = self.locate_frame(id)?;
        let payload = self.remove_frame(spread_idx, kind, position);
        // text_frame_index references shift on remove; rebuild the
        // index for the touched spread.
        self.rebuild_frame_indexes();
        Ok(UndoEntry::Restore {
            spread_idx,
            kind,
            position,
            payload,
        })
    }

    fn invert(&mut self, entry: &UndoEntry) -> Option<UndoEntry> {
        match entry {
            UndoEntry::Translate {
                frame,
                dx_pt,
                dy_pt,
            } => {
                {
                    let mut f = self.find_frame_mut(frame).ok()?;
                    translate_in_place(&mut f, *dx_pt, *dy_pt);
                }
                self.refresh_frame_for_story_cache(frame);
                Some(UndoEntry::Translate {
                    frame: frame.clone(),
                    dx_pt: -dx_pt,
                    dy_pt: -dy_pt,
                })
            }
            UndoEntry::SetFrameTransform {
                frame,
                prev_item_transform,
                prev_bounds,
            } => {
                let cur_xform;
                let cur_bounds;
                {
                    let mut f = self.find_frame_mut(frame).ok()?;
                    cur_xform = f.item_transform_clone();
                    cur_bounds = f.bounds_clone();
                    f.set_item_transform(*prev_item_transform);
                    f.set_bounds(*prev_bounds);
                }
                self.refresh_frame_for_story_cache(frame);
                Some(UndoEntry::SetFrameTransform {
                    frame: frame.clone(),
                    prev_item_transform: cur_xform,
                    prev_bounds: cur_bounds,
                })
            }
            UndoEntry::SetZ {
                spread_idx,
                kind,
                from_idx,
                to_idx,
            } => {
                self.move_frame_in_kind(*spread_idx, *kind, *from_idx, *to_idx);
                Some(UndoEntry::SetZ {
                    spread_idx: *spread_idx,
                    kind: *kind,
                    from_idx: *to_idx,
                    to_idx: *from_idx,
                })
            }
            UndoEntry::DeleteInserted {
                story,
                para,
                byte_offset,
                byte_len,
                coalesce,
            } => {
                let removed;
                {
                    let rope = self.ensure_rope(story).ok()?;
                    let p = Self::paragraph_mut(rope, story, *para).ok()?;
                    removed =
                        p.delete_range(*byte_offset as usize, (*byte_offset + *byte_len) as usize);
                }
                self.sync_story_to_doc(story);
                Some(UndoEntry::InsertDeleted {
                    story: story.clone(),
                    para: *para,
                    byte_offset: *byte_offset,
                    text: removed,
                    coalesce: *coalesce,
                })
            }
            UndoEntry::InsertDeleted {
                story,
                para,
                byte_offset,
                text,
                coalesce,
            } => {
                let inserted_len;
                {
                    let rope = self.ensure_rope(story).ok()?;
                    let p = Self::paragraph_mut(rope, story, *para).ok()?;
                    inserted_len = p.insert_str(*byte_offset as usize, text);
                }
                self.sync_story_to_doc(story);
                Some(UndoEntry::DeleteInserted {
                    story: story.clone(),
                    para: *para,
                    byte_offset: *byte_offset,
                    byte_len: inserted_len as u32,
                    coalesce: *coalesce,
                })
            }
            UndoEntry::InsertDeletedReplace {
                story,
                para,
                byte_offset,
                prev_text,
                new_len,
                coalesce,
            } => {
                let removed;
                let restored_len;
                {
                    let rope = self.ensure_rope(story).ok()?;
                    let p = Self::paragraph_mut(rope, story, *para).ok()?;
                    removed =
                        p.delete_range(*byte_offset as usize, (*byte_offset + *new_len) as usize);
                    restored_len = p.insert_str(*byte_offset as usize, prev_text) as u32;
                }
                self.sync_story_to_doc(story);
                Some(UndoEntry::InsertDeletedReplace {
                    story: story.clone(),
                    para: *para,
                    byte_offset: *byte_offset,
                    prev_text: removed,
                    new_len: restored_len,
                    coalesce: *coalesce,
                })
            }
            UndoEntry::RestoreRunAttrs {
                story,
                para,
                previous,
            } => {
                // Capture current attrs over the same byte ranges,
                // then restore previous; the inverse is a fresh
                // RestoreRunAttrs with the captured "current" snapshot.
                let mut cur_snapshot: Vec<(u32, u32, RunAttrs)> = Vec::new();
                {
                    let rope = self.ensure_rope(story).ok()?;
                    let p = Self::paragraph_mut(rope, story, *para).ok()?;
                    cur_snapshot = capture_run_attrs(p, previous, cur_snapshot);
                    apply_run_attrs_snapshot(p, previous);
                }
                self.sync_story_to_doc(story);
                Some(UndoEntry::RestoreRunAttrs {
                    story: story.clone(),
                    para: *para,
                    previous: cur_snapshot,
                })
            }
            UndoEntry::RestoreParagraphAttrs { story, para, prev } => {
                let cur;
                {
                    let rope = self.ensure_rope(story).ok()?;
                    let p = Self::paragraph_mut(rope, story, *para).ok()?;
                    cur = p.attrs.clone();
                    p.attrs = prev.clone();
                }
                self.sync_story_to_doc(story);
                Some(UndoEntry::RestoreParagraphAttrs {
                    story: story.clone(),
                    para: *para,
                    prev: cur,
                })
            }
            UndoEntry::MergeAfterSplit { story, para } => {
                // Merge the split that the forward command made.
                // Capture the merge offset for the redo path.
                let split_offset;
                let tail_attrs;
                {
                    let rope = self.ensure_rope(story).ok()?;
                    let idx = para.0 as usize;
                    if idx + 1 >= rope.paragraphs.len() {
                        return None;
                    }
                    split_offset = rope.paragraphs[idx].len_bytes() as u32;
                    let next = rope.paragraphs.remove(idx + 1);
                    tail_attrs = next.attrs;
                    for r in next.runs {
                        if !r.text.is_empty() {
                            rope.paragraphs[idx].runs.push(r);
                        }
                    }
                    if rope.paragraphs[idx].runs.is_empty() {
                        rope.paragraphs[idx]
                            .runs
                            .push(RunSeg::new(RunAttrs::default(), String::new()));
                    }
                }
                self.sync_story_to_doc(story);
                Some(UndoEntry::SplitAfterMerge {
                    story: story.clone(),
                    para: *para,
                    byte_offset: split_offset,
                    tail_attrs,
                })
            }
            UndoEntry::SplitAfterMerge {
                story,
                para,
                byte_offset,
                tail_attrs,
            } => {
                {
                    let rope = self.ensure_rope(story).ok()?;
                    let idx = para.0 as usize;
                    if idx >= rope.paragraphs.len() {
                        return None;
                    }
                    let off = *byte_offset as usize;
                    let (head_runs, tail_runs) = split_runs_at(&rope.paragraphs[idx].runs, off);
                    rope.paragraphs[idx].runs = head_runs;
                    rope.paragraphs.insert(
                        idx + 1,
                        ParagraphRope {
                            attrs: tail_attrs.clone(),
                            runs: ensure_at_least_one(tail_runs),
                            table: None,
                        },
                    );
                }
                self.sync_story_to_doc(story);
                Some(UndoEntry::MergeAfterSplit {
                    story: story.clone(),
                    para: *para,
                })
            }
            UndoEntry::SetFrameFill { frame, prev } => {
                let cur;
                {
                    let mut f = self.find_frame_mut(frame).ok()?;
                    cur = f.fill_color_clone();
                    f.set_fill_color(prev.clone());
                }
                Some(UndoEntry::SetFrameFill {
                    frame: frame.clone(),
                    prev: cur,
                })
            }
            UndoEntry::DeleteCreated {
                spread_idx,
                kind,
                position,
            } => {
                let payload = self.remove_frame(*spread_idx, *kind, *position);
                self.rebuild_frame_indexes();
                Some(UndoEntry::Restore {
                    spread_idx: *spread_idx,
                    kind: *kind,
                    position: *position,
                    payload,
                })
            }
            UndoEntry::SetFrameStroke {
                frame,
                prev_color,
                prev_weight,
            } => {
                let cur_c;
                let cur_w;
                {
                    let mut f = self.find_frame_mut(frame).ok()?;
                    cur_c = f.stroke_color_clone();
                    cur_w = f.stroke_weight_clone();
                    f.set_stroke_color(prev_color.clone());
                    f.set_stroke_weight(*prev_weight);
                }
                Some(UndoEntry::SetFrameStroke {
                    frame: frame.clone(),
                    prev_color: cur_c,
                    prev_weight: cur_w,
                })
            }
            UndoEntry::SetImageLink { frame, prev } => {
                let cur;
                {
                    let mut found = None;
                    for ps in self.doc.spreads.iter_mut() {
                        if let Some(r) = ps
                            .spread
                            .rectangles
                            .iter_mut()
                            .find(|r| r.self_id.as_deref() == Some(frame.as_str()))
                        {
                            found = Some(r);
                            break;
                        }
                    }
                    let r = found?;
                    cur = r.image_link.clone();
                    r.image_link = prev.clone();
                }
                Some(UndoEntry::SetImageLink {
                    frame: frame.clone(),
                    prev: cur,
                })
            }
            UndoEntry::SetLayerVisible { layer_id, prev } => {
                let cur;
                {
                    let layer = self
                        .doc
                        .container
                        .designmap
                        .layers
                        .iter_mut()
                        .find(|l| &l.self_id == layer_id)?;
                    cur = layer.visible;
                    layer.visible = *prev;
                }
                Some(UndoEntry::SetLayerVisible {
                    layer_id: layer_id.clone(),
                    prev: cur,
                })
            }
            UndoEntry::SetLayerLocked { layer_id, prev } => {
                let cur;
                {
                    let layer = self
                        .doc
                        .container
                        .designmap
                        .layers
                        .iter_mut()
                        .find(|l| &l.self_id == layer_id)?;
                    cur = layer.locked;
                    layer.locked = *prev;
                }
                Some(UndoEntry::SetLayerLocked {
                    layer_id: layer_id.clone(),
                    prev: cur,
                })
            }
            UndoEntry::SetAppliedMaster { page, prev } => {
                let mut cur = None;
                for ps in self.doc.spreads.iter_mut() {
                    if let Some(p) = ps
                        .spread
                        .pages
                        .iter_mut()
                        .find(|p| p.self_id.as_deref() == Some(page.as_str()))
                    {
                        cur = p.applied_master.clone();
                        p.applied_master = prev.clone();
                        break;
                    }
                }
                Some(UndoEntry::SetAppliedMaster {
                    page: page.clone(),
                    prev: cur,
                })
            }
            UndoEntry::SetAppliedObjectStyle { frame, prev } => {
                let cur;
                {
                    let mut f = self.find_frame_mut(frame).ok()?;
                    cur = f.applied_object_style_clone();
                    f.set_applied_object_style(prev.clone());
                }
                Some(UndoEntry::SetAppliedObjectStyle {
                    frame: frame.clone(),
                    prev: cur,
                })
            }
            UndoEntry::SetNextTextFrame { from, prev } => {
                let cur;
                {
                    let mut frame = self.find_frame_mut(from).ok()?;
                    if let FrameMutRef::Text { f } = &mut frame {
                        cur = f.next_text_frame.clone();
                        f.next_text_frame = prev.clone();
                    } else {
                        return None;
                    }
                }
                Some(UndoEntry::SetNextTextFrame {
                    from: from.clone(),
                    prev: cur,
                })
            }
            UndoEntry::Restore {
                spread_idx,
                kind,
                position,
                payload,
            } => {
                self.insert_frame(*spread_idx, *kind, *position, payload.clone());
                self.rebuild_frame_indexes();
                // Inverse of restore is the matching delete.
                let id = payload_self_id(payload).unwrap_or_default();
                let pos = *position;
                let kind = *kind;
                let spread_idx = *spread_idx;
                // Capture the payload at the *current* state for redo;
                // re-run `remove_frame` (which rebuilds indexes via
                // the caller's path) to get the snapshot.
                let payload = self.remove_frame(spread_idx, kind, pos);
                self.rebuild_frame_indexes();
                // Re-insert immediately so the redo direction lands a
                // proper round-trip on the next invocation.
                self.insert_frame(spread_idx, kind, pos, payload.clone());
                self.rebuild_frame_indexes();
                let _ = id;
                Some(UndoEntry::Restore {
                    spread_idx,
                    kind,
                    position: pos,
                    payload,
                })
            }
        }
    }

    fn patch_for(&self, cmd: &Command) -> Patch {
        let mut p = Patch::new(self.epoch);
        match cmd {
            Command::Noop => {}
            Command::MoveFrame { frame, .. } | Command::SetFrameBounds { frame, .. } => {
                p.push(frame.clone(), InvalidationKind::Geometry);
            }
            Command::BringFrameToFront { frame }
            | Command::SendFrameToBack { frame }
            | Command::DeleteFrame { frame } => {
                p.push(frame.clone(), InvalidationKind::Structure);
            }
            Command::InsertText { story, para, .. }
            | Command::DeleteRange { story, para, .. }
            | Command::ReplaceRange { story, para, .. } => {
                p.push(
                    NodeId::Para(story.clone(), *para),
                    InvalidationKind::TextContent,
                );
            }
            Command::SetRunAttr { story, para, .. } => {
                p.push(
                    NodeId::Para(story.clone(), *para),
                    InvalidationKind::RunAttrs,
                );
            }
            Command::SetParagraphAttr { story, para, .. } => {
                p.push(
                    NodeId::Para(story.clone(), *para),
                    InvalidationKind::ParagraphAttrs,
                );
            }
            Command::SplitParagraph { story, para, .. }
            | Command::MergeParagraph { story, para } => {
                p.push(NodeId::Story(story.clone()), InvalidationKind::Structure);
                p.push(
                    NodeId::Para(story.clone(), *para),
                    InvalidationKind::TextContent,
                );
            }
            Command::LinkFrames { from, .. } | Command::UnlinkFrames { from } => {
                p.push(from.clone(), InvalidationKind::Structure);
            }
            Command::ApplyObjectStyle { frame, .. } => {
                p.push(frame.clone(), InvalidationKind::StyleSheet);
            }
            Command::ApplyMasterToPage { page, .. } => {
                p.push(page.clone(), InvalidationKind::Structure);
            }
            Command::SetLayerVisible { .. } | Command::SetLayerLocked { .. } => {
                // Invalidate every spread — items reference layers
                // across the whole document. The renderer's existing
                // visibility check already short-circuits per item.
                for ps in &self.doc.spreads {
                    if let Some(self_id) = &ps.spread.self_id {
                        p.push(
                            NodeId::Spread(self_id.clone()),
                            InvalidationKind::Appearance,
                        );
                    }
                }
            }
            Command::PlaceImageInFrame { frame, .. }
            | Command::SetFrameFill { frame, .. }
            | Command::SetFrameStroke { frame, .. } => {
                p.push(frame.clone(), InvalidationKind::Appearance);
            }
            Command::CreateRectangle { spread_idx, .. } => {
                p.push(
                    NodeId::Spread(format!("spread#{spread_idx}")),
                    InvalidationKind::Structure,
                );
            }
        }
        p
    }

    // -----------------------------------------------------------------
    // Frame lookup + mutation utilities.

    fn find_frame_mut<'a>(&'a mut self, frame_id: &str) -> Result<FrameMutRef<'a>, EditError> {
        let (spread_idx, kind, idx) = self.locate_frame(frame_id)?;
        let spread = &mut self.doc.spreads[spread_idx].spread;
        Ok(match kind {
            FrameKind::Text => FrameMutRef::Text {
                f: &mut spread.text_frames[idx],
            },
            FrameKind::Rectangle => FrameMutRef::Rect {
                f: &mut spread.rectangles[idx],
            },
            FrameKind::Oval => FrameMutRef::Oval {
                f: &mut spread.ovals[idx],
            },
            FrameKind::GraphicLine => FrameMutRef::Line {
                f: &mut spread.graphic_lines[idx],
            },
            FrameKind::Polygon => FrameMutRef::Poly {
                f: &mut spread.polygons[idx],
            },
        })
    }

    /// Locate `frame_id` across every spread's typed lists.
    fn locate_frame(&self, frame_id: &str) -> Result<(usize, FrameKind, usize), EditError> {
        for (spread_idx, ps) in self.doc.spreads.iter().enumerate() {
            if let Some(i) = position_of(&ps.spread.text_frames, frame_id, |f| f.self_id.as_deref())
            {
                return Ok((spread_idx, FrameKind::Text, i));
            }
            if let Some(i) = position_of(&ps.spread.rectangles, frame_id, |f| f.self_id.as_deref())
            {
                return Ok((spread_idx, FrameKind::Rectangle, i));
            }
            if let Some(i) = position_of(&ps.spread.ovals, frame_id, |f| f.self_id.as_deref()) {
                return Ok((spread_idx, FrameKind::Oval, i));
            }
            if let Some(i) =
                position_of(&ps.spread.graphic_lines, frame_id, |f| f.self_id.as_deref())
            {
                return Ok((spread_idx, FrameKind::GraphicLine, i));
            }
            if let Some(i) = position_of(&ps.spread.polygons, frame_id, |f| f.self_id.as_deref()) {
                return Ok((spread_idx, FrameKind::Polygon, i));
            }
        }
        Err(EditError::NodeNotFound(NodeId::Frame(frame_id.to_string())))
    }

    fn kind_len(&self, spread_idx: usize, kind: FrameKind) -> usize {
        let s = &self.doc.spreads[spread_idx].spread;
        match kind {
            FrameKind::Text => s.text_frames.len(),
            FrameKind::Rectangle => s.rectangles.len(),
            FrameKind::Oval => s.ovals.len(),
            FrameKind::GraphicLine => s.graphic_lines.len(),
            FrameKind::Polygon => s.polygons.len(),
        }
    }

    fn move_frame_in_kind(&mut self, spread_idx: usize, kind: FrameKind, from: usize, to: usize) {
        let s = &mut self.doc.spreads[spread_idx].spread;
        match kind {
            FrameKind::Text => move_in_vec(&mut s.text_frames, from, to),
            FrameKind::Rectangle => move_in_vec(&mut s.rectangles, from, to),
            FrameKind::Oval => move_in_vec(&mut s.ovals, from, to),
            FrameKind::GraphicLine => move_in_vec(&mut s.graphic_lines, from, to),
            FrameKind::Polygon => move_in_vec(&mut s.polygons, from, to),
        }
        self.rebuild_frame_indexes();
    }

    fn remove_frame(
        &mut self,
        spread_idx: usize,
        kind: FrameKind,
        position: usize,
    ) -> FramePayload {
        let s = &mut self.doc.spreads[spread_idx].spread;
        match kind {
            FrameKind::Text => FramePayload::Text(Box::new(s.text_frames.remove(position))),
            FrameKind::Rectangle => {
                FramePayload::Rectangle(Box::new(s.rectangles.remove(position)))
            }
            FrameKind::Oval => FramePayload::Oval(Box::new(s.ovals.remove(position))),
            FrameKind::GraphicLine => {
                FramePayload::GraphicLine(Box::new(s.graphic_lines.remove(position)))
            }
            FrameKind::Polygon => FramePayload::Polygon(Box::new(s.polygons.remove(position))),
        }
    }

    fn insert_frame(
        &mut self,
        spread_idx: usize,
        kind: FrameKind,
        position: usize,
        payload: FramePayload,
    ) {
        let s = &mut self.doc.spreads[spread_idx].spread;
        match (kind, payload) {
            (FrameKind::Text, FramePayload::Text(b)) => s.text_frames.insert(position, *b),
            (FrameKind::Rectangle, FramePayload::Rectangle(b)) => s.rectangles.insert(position, *b),
            (FrameKind::Oval, FramePayload::Oval(b)) => s.ovals.insert(position, *b),
            (FrameKind::GraphicLine, FramePayload::GraphicLine(b)) => {
                s.graphic_lines.insert(position, *b)
            }
            (FrameKind::Polygon, FramePayload::Polygon(b)) => s.polygons.insert(position, *b),
            _ => panic!("FrameKind / FramePayload mismatch"),
        }
    }

    fn rebuild_frame_indexes(&mut self) {
        // Rebuild text_frame_index after structural changes. We don't
        // touch frame_for_story here — translates already update the
        // cached frame in place via refresh_frame_for_story_cache,
        // and remove/insert flushes the parent_story → frame mapping
        // by re-walking spreads.
        self.doc.text_frame_index.clear();
        for (spread_idx, ps) in self.doc.spreads.iter().enumerate() {
            for (frame_idx, f) in ps.spread.text_frames.iter().enumerate() {
                if let Some(self_id) = f.self_id.clone() {
                    self.doc
                        .text_frame_index
                        .insert(self_id, (spread_idx, frame_idx));
                }
            }
        }
        // frame_for_story: keep entries pointing at the latest frame
        // by walking all spreads. Cheap for small docs; M3+ will care.
        self.doc.frame_for_story.clear();
        for ps in &self.doc.spreads {
            for f in &ps.spread.text_frames {
                if let Some(parent) = f.parent_story.clone() {
                    self.doc.frame_for_story.insert(parent, f.clone());
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Rope-backed text mutation. The rope is hydrated lazily on the
    // first command targeting a story; once a rope exists, the
    // underlying `Document::stories[i].story` is rebuilt from the
    // rope after every text command so the existing render pipeline
    // sees the new text.

    fn ensure_rope(&mut self, story: &StoryId) -> Result<&mut StoryRope, EditError> {
        let key = &story.0;
        if !self.ropes.contains_key(key) {
            let idx = *self
                .story_index
                .get(key)
                .ok_or_else(|| EditError::NodeNotFound(NodeId::Story(story.clone())))?;
            let parsed = &self.doc.stories[idx].story;
            self.ropes
                .insert(key.clone(), StoryRope::from_story(parsed));
        }
        Ok(self.ropes.get_mut(key).expect("just inserted"))
    }

    fn sync_story_to_doc(&mut self, story: &StoryId) {
        let key = &story.0;
        let Some(rope) = self.ropes.get(key) else {
            return;
        };
        let Some(&idx) = self.story_index.get(key) else {
            return;
        };
        self.doc.stories[idx].story = rope.to_story();
    }

    fn paragraph_mut<'a>(
        rope: &'a mut StoryRope,
        story: &StoryId,
        para: ParaId,
    ) -> Result<&'a mut ParagraphRope, EditError> {
        rope.paragraph_mut(para.0 as usize)
            .ok_or_else(|| EditError::NodeNotFound(NodeId::Para(story.clone(), para)))
    }

    fn do_insert_text(
        &mut self,
        story: &StoryId,
        para: ParaId,
        byte_offset: u32,
        text: &str,
        coalesce: Option<u32>,
    ) -> Result<UndoEntry, EditError> {
        let inserted_len;
        {
            let rope = self.ensure_rope(story)?;
            let p = Self::paragraph_mut(rope, story, para)?;
            let off = byte_offset as usize;
            inserted_len = p.insert_str(off, text);
        }
        self.sync_story_to_doc(story);
        Ok(UndoEntry::DeleteInserted {
            story: story.clone(),
            para,
            byte_offset,
            byte_len: inserted_len as u32,
            coalesce,
        })
    }

    fn do_delete_range(
        &mut self,
        story: &StoryId,
        para: ParaId,
        byte_from: u32,
        byte_to: u32,
        coalesce: Option<u32>,
    ) -> Result<UndoEntry, EditError> {
        let removed;
        {
            let rope = self.ensure_rope(story)?;
            let p = Self::paragraph_mut(rope, story, para)?;
            removed = p.delete_range(byte_from as usize, byte_to as usize);
        }
        self.sync_story_to_doc(story);
        Ok(UndoEntry::InsertDeleted {
            story: story.clone(),
            para,
            byte_offset: byte_from,
            text: removed,
            coalesce,
        })
    }

    fn do_replace_range(
        &mut self,
        story: &StoryId,
        para: ParaId,
        byte_from: u32,
        byte_to: u32,
        text: &str,
        coalesce: Option<u32>,
    ) -> Result<UndoEntry, EditError> {
        let prev_text;
        let new_len;
        {
            let rope = self.ensure_rope(story)?;
            let p = Self::paragraph_mut(rope, story, para)?;
            prev_text = p.delete_range(byte_from as usize, byte_to as usize);
            new_len = p.insert_str(byte_from as usize, text) as u32;
        }
        self.sync_story_to_doc(story);
        Ok(UndoEntry::InsertDeletedReplace {
            story: story.clone(),
            para,
            byte_offset: byte_from,
            prev_text,
            new_len,
            coalesce,
        })
    }

    fn do_set_run_attr(
        &mut self,
        story: &StoryId,
        para: ParaId,
        byte_from: u32,
        byte_to: u32,
        attr: &RunAttrPatch,
    ) -> Result<UndoEntry, EditError> {
        let mut previous: Vec<(u32, u32, RunAttrs)> = Vec::new();
        {
            let rope = self.ensure_rope(story)?;
            let p = Self::paragraph_mut(rope, story, para)?;
            previous =
                split_and_apply_run_attr(p, byte_from as usize, byte_to as usize, attr, previous);
        }
        self.sync_story_to_doc(story);
        Ok(UndoEntry::RestoreRunAttrs {
            story: story.clone(),
            para,
            previous,
        })
    }

    fn do_set_paragraph_attr(
        &mut self,
        story: &StoryId,
        para: ParaId,
        attr: &ParagraphAttrPatch,
    ) -> Result<UndoEntry, EditError> {
        let prev;
        {
            let rope = self.ensure_rope(story)?;
            let p = Self::paragraph_mut(rope, story, para)?;
            prev = p.attrs.clone();
            apply_paragraph_attr(&mut p.attrs, attr);
        }
        self.sync_story_to_doc(story);
        Ok(UndoEntry::RestoreParagraphAttrs {
            story: story.clone(),
            para,
            prev,
        })
    }

    fn do_split_paragraph(
        &mut self,
        story: &StoryId,
        para: ParaId,
        byte_offset: u32,
    ) -> Result<UndoEntry, EditError> {
        {
            let rope = self.ensure_rope(story)?;
            let idx = para.0 as usize;
            if idx >= rope.paragraphs.len() {
                return Err(EditError::NodeNotFound(NodeId::Para(story.clone(), para)));
            }
            let off = byte_offset as usize;
            let (head_runs, tail_runs) = split_runs_at(&rope.paragraphs[idx].runs, off);
            let head_attrs = rope.paragraphs[idx].attrs.clone();
            rope.paragraphs[idx].runs = head_runs;
            rope.paragraphs.insert(
                idx + 1,
                ParagraphRope {
                    attrs: head_attrs,
                    runs: ensure_at_least_one(tail_runs),
                    table: None,
                },
            );
        }
        self.sync_story_to_doc(story);
        Ok(UndoEntry::MergeAfterSplit {
            story: story.clone(),
            para,
        })
    }

    fn do_merge_paragraph(
        &mut self,
        story: &StoryId,
        para: ParaId,
    ) -> Result<UndoEntry, EditError> {
        let split_offset;
        let tail_attrs;
        {
            let rope = self.ensure_rope(story)?;
            let idx = para.0 as usize;
            if idx + 1 >= rope.paragraphs.len() {
                return Err(EditError::NodeNotFound(NodeId::Para(
                    story.clone(),
                    ParaId(para.0 + 1),
                )));
            }
            split_offset = rope.paragraphs[idx].len_bytes() as u32;
            let next = rope.paragraphs.remove(idx + 1);
            tail_attrs = next.attrs;
            // Append tail runs onto the head paragraph.
            for r in next.runs {
                if !r.text.is_empty() {
                    rope.paragraphs[idx].runs.push(r);
                }
            }
            // If we ended up with no runs (both halves were empty),
            // restore one default-attrs run so the rope invariant
            // holds.
            if rope.paragraphs[idx].runs.is_empty() {
                rope.paragraphs[idx]
                    .runs
                    .push(RunSeg::new(RunAttrs::default(), String::new()));
            }
        }
        self.sync_story_to_doc(story);
        Ok(UndoEntry::SplitAfterMerge {
            story: story.clone(),
            para,
            byte_offset: split_offset,
            tail_attrs,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn do_create_rectangle(
        &mut self,
        spread_idx: usize,
        self_id: Option<String>,
        bounds: RectanglePayloadBounds,
        item_transform: Option<[f32; 6]>,
        fill_color: Option<String>,
        stroke_color: Option<String>,
        stroke_weight: Option<f32>,
        applied_object_style: Option<String>,
        image_link: Option<String>,
    ) -> Result<UndoEntry, EditError> {
        if spread_idx >= self.doc.spreads.len() {
            return Err(EditError::NodeNotFound(NodeId::Spread(format!(
                "spread#{spread_idx}"
            ))));
        }
        let id = self_id.unwrap_or_else(|| self.mint_id());
        let rect = idml_parse::Rectangle {
            self_id: Some(id.clone()),
            bounds: idml_parse::Bounds {
                top: bounds.top,
                left: bounds.left,
                bottom: bounds.bottom,
                right: bounds.right,
            },
            item_transform,
            fill_color,
            fill_tint: None,
            stroke_color,
            stroke_weight,
            drop_shadow: None,
            stroke_drop_shadow: None,
            image_link: image_link.clone(),
            has_image_element: image_link.is_some(),
            image_item_transform: None,
            applied_object_style,
            text_wrap: None,
            frame_fitting: None,
            stroke_type: None,
            stroke_alignment: None,
            end_cap: None,
            end_join: None,
            miter_limit: None,
            item_layer: None,
            corner_radius: None,
            corner_option: None,
            is_anchored: false,
            opacity: None,
            blend_mode: None,
            effects: None,
            gradient_fill_angle: None,
            gradient_fill_length: None,
            gradient_stroke_angle: None,
            gradient_stroke_length: None,
            text_paths: Vec::new(),
            overprint_fill: false,
            overprint_stroke: false,
        };
        let position = self.doc.spreads[spread_idx].spread.rectangles.len();
        self.doc.spreads[spread_idx].spread.rectangles.push(rect);
        Ok(UndoEntry::DeleteCreated {
            spread_idx,
            kind: FrameKind::Rectangle,
            position,
        })
    }

    fn do_set_frame_fill(
        &mut self,
        frame: &NodeId,
        color: Option<String>,
    ) -> Result<UndoEntry, EditError> {
        let id = self.frame_id(frame)?.to_string();
        let prev;
        {
            let mut f = self.find_frame_mut(&id)?;
            prev = f.fill_color_clone();
            f.set_fill_color(color);
        }
        Ok(UndoEntry::SetFrameFill { frame: id, prev })
    }

    fn do_set_frame_stroke(
        &mut self,
        frame: &NodeId,
        color: Option<String>,
        weight: Option<f32>,
    ) -> Result<UndoEntry, EditError> {
        let id = self.frame_id(frame)?.to_string();
        let prev_color;
        let prev_weight;
        {
            let mut f = self.find_frame_mut(&id)?;
            prev_color = f.stroke_color_clone();
            prev_weight = f.stroke_weight_clone();
            f.set_stroke_color(color);
            f.set_stroke_weight(weight);
        }
        Ok(UndoEntry::SetFrameStroke {
            frame: id,
            prev_color,
            prev_weight,
        })
    }

    fn do_place_image(
        &mut self,
        frame: &NodeId,
        link_uri: Option<String>,
    ) -> Result<UndoEntry, EditError> {
        let id = self.frame_id(frame)?.to_string();
        // Image placement is a Rectangle field today (the renderer
        // routes <Image> to image_link on its hosting Rectangle).
        for ps in self.doc.spreads.iter_mut() {
            if let Some(r) = ps
                .spread
                .rectangles
                .iter_mut()
                .find(|r| r.self_id.as_deref() == Some(id.as_str()))
            {
                let prev = r.image_link.clone();
                r.image_link = link_uri;
                return Ok(UndoEntry::SetImageLink { frame: id, prev });
            }
        }
        Err(EditError::NodeNotFound(NodeId::Frame(id)))
    }

    fn do_set_layer_visible(
        &mut self,
        layer_id: &str,
        visible: bool,
    ) -> Result<UndoEntry, EditError> {
        let layer = self
            .doc
            .container
            .designmap
            .layers
            .iter_mut()
            .find(|l| l.self_id == layer_id)
            .ok_or_else(|| EditError::NodeNotFound(NodeId::Frame(layer_id.to_string())))?;
        let prev = layer.visible;
        layer.visible = visible;
        Ok(UndoEntry::SetLayerVisible {
            layer_id: layer_id.to_string(),
            prev,
        })
    }

    fn do_set_layer_locked(
        &mut self,
        layer_id: &str,
        locked: bool,
    ) -> Result<UndoEntry, EditError> {
        let layer = self
            .doc
            .container
            .designmap
            .layers
            .iter_mut()
            .find(|l| l.self_id == layer_id)
            .ok_or_else(|| EditError::NodeNotFound(NodeId::Frame(layer_id.to_string())))?;
        let prev = layer.locked;
        layer.locked = locked;
        Ok(UndoEntry::SetLayerLocked {
            layer_id: layer_id.to_string(),
            prev,
        })
    }

    fn do_apply_master_to_page(
        &mut self,
        page: &NodeId,
        master: Option<String>,
    ) -> Result<UndoEntry, EditError> {
        let id = match page {
            NodeId::Page(s) => s.clone(),
            other => return Err(EditError::WrongNodeKind(other.clone())),
        };
        for ps in self.doc.spreads.iter_mut() {
            if let Some(p) = ps
                .spread
                .pages
                .iter_mut()
                .find(|p| p.self_id.as_deref() == Some(id.as_str()))
            {
                let prev = p.applied_master.clone();
                p.applied_master = master;
                return Ok(UndoEntry::SetAppliedMaster { page: id, prev });
            }
        }
        Err(EditError::NodeNotFound(NodeId::Page(id)))
    }

    fn do_apply_object_style(
        &mut self,
        frame: &NodeId,
        style: Option<String>,
    ) -> Result<UndoEntry, EditError> {
        let id = self.frame_id(frame)?.to_string();
        let prev;
        {
            let mut f = self.find_frame_mut(&id)?;
            prev = f.applied_object_style_clone();
            f.set_applied_object_style(style);
        }
        Ok(UndoEntry::SetAppliedObjectStyle { frame: id, prev })
    }

    fn do_link_frames(
        &mut self,
        from: &NodeId,
        to: Option<&NodeId>,
    ) -> Result<UndoEntry, EditError> {
        let from_id = self.frame_id(from)?.to_string();
        let to_id = match to {
            Some(node) => Some(self.frame_id(node)?.to_string()),
            None => None,
        };
        let prev;
        {
            let mut frame = self.find_frame_mut(&from_id)?;
            match &mut frame {
                FrameMutRef::Text { f } => {
                    prev = f.next_text_frame.clone();
                    f.next_text_frame = to_id;
                }
                _ => return Err(EditError::WrongNodeKind(from.clone())),
            }
        }
        Ok(UndoEntry::SetNextTextFrame {
            from: from_id,
            prev,
        })
    }

    fn refresh_frame_for_story_cache(&mut self, frame_id: &str) {
        // After a translate, copy the mutated TextFrame into
        // frame_for_story so the renderer sees the new origin.
        let Ok((spread_idx, FrameKind::Text, idx)) = self.locate_frame(frame_id) else {
            return;
        };
        let frame = self.doc.spreads[spread_idx].spread.text_frames[idx].clone();
        if let Some(parent) = frame.parent_story.clone() {
            self.doc.frame_for_story.insert(parent, frame);
        }
    }
}

// ---------------------------------------------------------------------
// Frame mut-ref helpers — uniform interface across the five frame
// types so the apply functions don't grow N×M match arms.

enum FrameMutRef<'a> {
    Text { f: &'a mut TextFrame },
    Rect { f: &'a mut Rectangle },
    Oval { f: &'a mut Oval },
    Line { f: &'a mut GraphicLine },
    Poly { f: &'a mut Polygon },
}

impl FrameMutRef<'_> {
    fn fill_color_clone(&self) -> Option<String> {
        match self {
            FrameMutRef::Text { f } => f.fill_color.clone(),
            FrameMutRef::Rect { f } => f.fill_color.clone(),
            FrameMutRef::Oval { f } => f.fill_color.clone(),
            // GraphicLine has no fill — stroke-only.
            FrameMutRef::Line { .. } => None,
            FrameMutRef::Poly { f } => f.fill_color.clone(),
        }
    }
    fn set_fill_color(&mut self, v: Option<String>) {
        match self {
            FrameMutRef::Text { f } => f.fill_color = v,
            FrameMutRef::Rect { f } => f.fill_color = v,
            FrameMutRef::Oval { f } => f.fill_color = v,
            FrameMutRef::Line { .. } => {} // no-op, lines carry no fill
            FrameMutRef::Poly { f } => f.fill_color = v,
        }
    }
    fn stroke_color_clone(&self) -> Option<String> {
        match self {
            FrameMutRef::Text { f } => f.stroke_color.clone(),
            FrameMutRef::Rect { f } => f.stroke_color.clone(),
            FrameMutRef::Oval { f } => f.stroke_color.clone(),
            FrameMutRef::Line { f } => f.stroke_color.clone(),
            FrameMutRef::Poly { f } => f.stroke_color.clone(),
        }
    }
    fn set_stroke_color(&mut self, v: Option<String>) {
        match self {
            FrameMutRef::Text { f } => f.stroke_color = v,
            FrameMutRef::Rect { f } => f.stroke_color = v,
            FrameMutRef::Oval { f } => f.stroke_color = v,
            FrameMutRef::Line { f } => f.stroke_color = v,
            FrameMutRef::Poly { f } => f.stroke_color = v,
        }
    }
    fn stroke_weight_clone(&self) -> Option<f32> {
        match self {
            FrameMutRef::Text { f } => f.stroke_weight,
            FrameMutRef::Rect { f } => f.stroke_weight,
            FrameMutRef::Oval { f } => f.stroke_weight,
            FrameMutRef::Line { f } => f.stroke_weight,
            FrameMutRef::Poly { f } => f.stroke_weight,
        }
    }
    fn set_stroke_weight(&mut self, v: Option<f32>) {
        match self {
            FrameMutRef::Text { f } => f.stroke_weight = v,
            FrameMutRef::Rect { f } => f.stroke_weight = v,
            FrameMutRef::Oval { f } => f.stroke_weight = v,
            FrameMutRef::Line { f } => f.stroke_weight = v,
            FrameMutRef::Poly { f } => f.stroke_weight = v,
        }
    }
    fn applied_object_style_clone(&self) -> Option<String> {
        match self {
            FrameMutRef::Text { f } => f.applied_object_style.clone(),
            FrameMutRef::Rect { f } => f.applied_object_style.clone(),
            FrameMutRef::Oval { f } => f.applied_object_style.clone(),
            FrameMutRef::Line { f } => f.applied_object_style.clone(),
            FrameMutRef::Poly { f } => f.applied_object_style.clone(),
        }
    }
    fn set_applied_object_style(&mut self, v: Option<String>) {
        match self {
            FrameMutRef::Text { f } => f.applied_object_style = v,
            FrameMutRef::Rect { f } => f.applied_object_style = v,
            FrameMutRef::Oval { f } => f.applied_object_style = v,
            FrameMutRef::Line { f } => f.applied_object_style = v,
            FrameMutRef::Poly { f } => f.applied_object_style = v,
        }
    }
    fn item_transform_clone(&self) -> Option<[f32; 6]> {
        match self {
            FrameMutRef::Text { f } => f.item_transform,
            FrameMutRef::Rect { f } => f.item_transform,
            FrameMutRef::Oval { f } => f.item_transform,
            FrameMutRef::Line { f } => f.item_transform,
            FrameMutRef::Poly { f } => f.item_transform,
        }
    }
    fn bounds_clone(&self) -> idml_parse::Bounds {
        match self {
            FrameMutRef::Text { f } => f.bounds,
            FrameMutRef::Rect { f } => f.bounds,
            FrameMutRef::Oval { f } => f.bounds,
            FrameMutRef::Line { f } => f.bounds,
            FrameMutRef::Poly { f } => f.bounds,
        }
    }
    fn set_item_transform(&mut self, t: Option<[f32; 6]>) {
        match self {
            FrameMutRef::Text { f } => f.item_transform = t,
            FrameMutRef::Rect { f } => f.item_transform = t,
            FrameMutRef::Oval { f } => f.item_transform = t,
            FrameMutRef::Line { f } => f.item_transform = t,
            FrameMutRef::Poly { f } => f.item_transform = t,
        }
    }
    fn set_bounds(&mut self, b: idml_parse::Bounds) {
        match self {
            FrameMutRef::Text { f } => f.bounds = b,
            FrameMutRef::Rect { f } => f.bounds = b,
            FrameMutRef::Oval { f } => f.bounds = b,
            FrameMutRef::Line { f } => f.bounds = b,
            FrameMutRef::Poly { f } => f.bounds = b,
        }
    }
}

/// Translate the frame's outer-spread origin by (dx, dy) pt. We
/// add the delta to the `tx`/`ty` of the frame's `ItemTransform`,
/// preserving any rotation/skew the frame already carries. If the
/// frame had no `ItemTransform`, we materialize an identity-with-
/// translation.
fn translate_in_place(frame: &mut FrameMutRef<'_>, dx_pt: f32, dy_pt: f32) {
    let cur = frame.item_transform_clone();
    let next = match cur {
        Some([a, b, c, d, tx, ty]) => Some([a, b, c, d, tx + dx_pt, ty + dy_pt]),
        None => Some([1.0, 0.0, 0.0, 1.0, dx_pt, dy_pt]),
    };
    frame.set_item_transform(next);
}

fn position_of<T, F>(v: &[T], needle: &str, get_id: F) -> Option<usize>
where
    F: Fn(&T) -> Option<&str>,
{
    v.iter().position(|x| get_id(x) == Some(needle))
}

fn move_in_vec<T>(v: &mut Vec<T>, from: usize, to: usize) {
    if from == to {
        return;
    }
    let item = v.remove(from);
    v.insert(to.min(v.len()), item);
}

/// Apply a single `RunAttrPatch` to the given run-level attrs.
fn apply_run_attr(attrs: &mut RunAttrs, patch: &RunAttrPatch) {
    match patch {
        RunAttrPatch::Font(v) => attrs.font = v.clone(),
        RunAttrPatch::FontStyle(v) => attrs.font_style = v.clone(),
        RunAttrPatch::PointSize(v) => attrs.point_size = *v,
        RunAttrPatch::FillColor(v) => attrs.fill_color = v.clone(),
        RunAttrPatch::FillTint(v) => attrs.fill_tint = *v,
        RunAttrPatch::Tracking(v) => attrs.tracking = *v,
        RunAttrPatch::BaselineShift(v) => attrs.baseline_shift = *v,
        RunAttrPatch::Capitalization(v) => attrs.capitalization = v.clone(),
        RunAttrPatch::Underline(v) => attrs.underline = *v,
        RunAttrPatch::Strikethru(v) => attrs.strikethru = *v,
        RunAttrPatch::CharacterStyle(v) => attrs.character_style = v.clone(),
    }
}

fn apply_paragraph_attr(attrs: &mut ParagraphAttrs, patch: &ParagraphAttrPatch) {
    match patch {
        ParagraphAttrPatch::Justification(v) => attrs.justification = *v,
        ParagraphAttrPatch::FirstLineIndent(v) => attrs.first_line_indent = *v,
        ParagraphAttrPatch::SpaceBefore(v) => attrs.space_before = *v,
        ParagraphAttrPatch::SpaceAfter(v) => attrs.space_after = *v,
        ParagraphAttrPatch::ParagraphStyle(v) => attrs.paragraph_style = v.clone(),
    }
}

/// Split runs at byte boundaries `byte_from` and `byte_to`, then
/// apply `attr` to every run in the range. Returns a vector of
/// `(byte_from, byte_to, prev_attrs)` snapshots so the inverse can
/// restore the prior state.
fn split_and_apply_run_attr(
    p: &mut ParagraphRope,
    byte_from: usize,
    byte_to: usize,
    attr: &RunAttrPatch,
    mut acc: Vec<(u32, u32, RunAttrs)>,
) -> Vec<(u32, u32, RunAttrs)> {
    if byte_to <= byte_from {
        return acc;
    }
    split_run_at(&mut p.runs, byte_from);
    split_run_at(&mut p.runs, byte_to);
    let mut pos = 0usize;
    for r in p.runs.iter_mut() {
        let len = r.text.len();
        let start = pos;
        let end = pos + len;
        if end <= byte_from {
            pos = end;
            continue;
        }
        if start >= byte_to {
            break;
        }
        acc.push((start as u32, end as u32, r.attrs.clone()));
        apply_run_attr(&mut r.attrs, attr);
        pos = end;
    }
    acc
}

/// Capture the current attrs over each `previous` range so that the
/// inverse undo can restore them. (Re-uses the same byte ranges; if
/// the apply path merged runs we still recover values per range.)
fn capture_run_attrs(
    p: &mut ParagraphRope,
    previous: &[(u32, u32, RunAttrs)],
    mut acc: Vec<(u32, u32, RunAttrs)>,
) -> Vec<(u32, u32, RunAttrs)> {
    for (from, to, _) in previous {
        split_run_at(&mut p.runs, *from as usize);
        split_run_at(&mut p.runs, *to as usize);
    }
    for (from, to, _) in previous {
        let mut pos = 0usize;
        for r in p.runs.iter() {
            let len = r.text.len();
            let start = pos;
            let end = pos + len;
            if end <= *from as usize {
                pos = end;
                continue;
            }
            if start >= *to as usize {
                break;
            }
            acc.push((*from, *to, r.attrs.clone()));
            break;
        }
    }
    acc
}

/// Restore the captured attrs onto the rope. Re-splits runs at the
/// recorded byte boundaries first, then writes the snapshot back.
fn apply_run_attrs_snapshot(p: &mut ParagraphRope, snapshot: &[(u32, u32, RunAttrs)]) {
    for (from, to, _) in snapshot {
        split_run_at(&mut p.runs, *from as usize);
        split_run_at(&mut p.runs, *to as usize);
    }
    for (from, to, attrs) in snapshot {
        let mut pos = 0usize;
        for r in p.runs.iter_mut() {
            let len = r.text.len();
            let start = pos;
            let end = pos + len;
            if end <= *from as usize {
                pos = end;
                continue;
            }
            if start >= *to as usize {
                break;
            }
            r.attrs = attrs.clone();
            pos = end;
        }
    }
}

/// Split the run that contains byte position `at` so that there's a
/// run boundary exactly at `at`. No-op if a boundary already exists.
fn split_run_at(runs: &mut Vec<RunSeg>, at: usize) {
    let mut pos = 0usize;
    for i in 0..runs.len() {
        let len = runs[i].text.len();
        let start = pos;
        let end = pos + len;
        if at == start || at == end {
            return;
        }
        if at > start && at < end {
            let local = at - start;
            // Snap to char boundary, just in case.
            let local = if runs[i].text.is_char_boundary(local) {
                local
            } else {
                let mut k = local;
                while k > 0 && !runs[i].text.is_char_boundary(k) {
                    k -= 1;
                }
                k
            };
            let tail = runs[i].text[local..].to_string();
            runs[i].text.truncate(local);
            let attrs = runs[i].attrs.clone();
            runs.insert(i + 1, RunSeg::new(attrs, tail));
            return;
        }
        pos = end;
    }
}

/// Split the rope's run list at byte `at`: returns `(head_runs,
/// tail_runs)`. Used by SplitParagraph.
fn split_runs_at(runs: &[RunSeg], at: usize) -> (Vec<RunSeg>, Vec<RunSeg>) {
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let mut pos = 0usize;
    let mut split_done = false;
    for r in runs {
        let len = r.text.len();
        let start = pos;
        let end = pos + len;
        if split_done {
            tail.push(r.clone());
            pos = end;
            continue;
        }
        if at >= end {
            head.push(r.clone());
            if at == end {
                split_done = true;
            }
            pos = end;
            continue;
        }
        if at <= start {
            tail.push(r.clone());
            split_done = true;
            pos = end;
            continue;
        }
        // Split this run.
        let local = at - start;
        let local = if r.text.is_char_boundary(local) {
            local
        } else {
            let mut k = local;
            while k > 0 && !r.text.is_char_boundary(k) {
                k -= 1;
            }
            k
        };
        head.push(RunSeg::new(r.attrs.clone(), r.text[..local].to_string()));
        let tail_text = r.text[local..].to_string();
        if !tail_text.is_empty() {
            tail.push(RunSeg::new(r.attrs.clone(), tail_text));
        }
        split_done = true;
        pos = end;
    }
    (head, tail)
}

fn ensure_at_least_one(runs: Vec<RunSeg>) -> Vec<RunSeg> {
    if runs.is_empty() {
        vec![RunSeg::new(RunAttrs::default(), String::new())]
    } else {
        runs
    }
}

fn payload_self_id(payload: &FramePayload) -> Option<String> {
    match payload {
        FramePayload::Text(f) => f.self_id.clone(),
        FramePayload::Rectangle(f) => f.self_id.clone(),
        FramePayload::Oval(f) => f.self_id.clone(),
        FramePayload::GraphicLine(f) => f.self_id.clone(),
        FramePayload::Polygon(f) => f.self_id.clone(),
    }
}
