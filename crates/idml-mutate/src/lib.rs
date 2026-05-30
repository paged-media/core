//! Operation-based mutation channel for the IDML scene graph.
//!
//! Stage 1 of the Verso scripting layer (`docs/verso/scripting-layer.md`):
//! a single typed, serializable, invertible [`Operation`] is the sole
//! committed mutation surface. The inspector, the future REPL, the
//! Boa-based scripting layer, the gesture commit path, undo/redo,
//! and any future collaboration layer all go through this one channel.
//!
//! ```text
//!   Operation ─► Project::apply ─► AppliedOperation
//!                                    │
//!                                    ├─► undo stack
//!                                    └─► Notifier subscribers
//! ```
//!
//! Five variants, closed set: [`SetProperty`], [`InsertNode`],
//! [`RemoveNode`], [`MoveNode`], [`Batch`]. Every Op is `Serialize` +
//! `Deserialize` so the same value moves freely across the WASM/JS
//! boundary, into a persisted log, or — when collaboration arrives —
//! over a wire to peers.
//!
//! [`SetProperty`]: Operation::SetProperty
//! [`InsertNode`]: Operation::InsertNode
//! [`RemoveNode`]: Operation::RemoveNode
//! [`MoveNode`]: Operation::MoveNode
//! [`Batch`]: Operation::Batch

use std::cell::RefCell;
use std::rc::Rc;

use idml_scene::Document;

pub mod apply;
pub mod bezier_conv;
pub mod error;
pub mod history;
pub mod invert;
pub mod notify;
pub mod operation;
pub mod path_math;
pub mod pathfinder;

pub use apply::apply;
pub use error::OperationError;
pub use history::{History, DEFAULT_HISTORY_CAPACITY};
pub use notify::Notifier;
pub use operation::{
    AppliedOperation, InvalidationHint, NodeId, NodeSpec, Operation, PathPointAddress,
    PathPointRole, PathfinderKind, PropertyPath, Value,
};

/// Holds a [`Document`] plus the Operation surface, undo/redo
/// history, and change-notification fan-out around it.
///
/// `Project` is the single owner during an interactive session.
/// `idml-introspect` wraps one of these and exposes it to JS via
/// `Rc<RefCell<Project>>`.
pub struct Project {
    document: Document,
    history: History,
    notifier: Notifier,
}

impl Project {
    pub fn new(document: Document) -> Self {
        Self::with_history_capacity(document, DEFAULT_HISTORY_CAPACITY)
    }

    pub fn with_history_capacity(document: Document, capacity: usize) -> Self {
        Self {
            document,
            history: History::with_capacity(capacity),
            notifier: Notifier::new(),
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn notifier_mut(&mut self) -> &mut Notifier {
        &mut self.notifier
    }

    /// Apply an Op. On success the op is recorded on the undo stack
    /// (clearing redo), and every subscriber on the notifier sees the
    /// `AppliedOperation`. On failure the document is unchanged and
    /// no notification fires.
    pub fn apply(&mut self, op: Operation) -> Result<AppliedOperation, OperationError> {
        let applied = apply::apply(&mut self.document, &op)?;
        self.history.record(applied.clone());
        self.notifier.notify(&applied);
        Ok(applied)
    }

    /// Undo the most recent applied op. Returns the
    /// `AppliedOperation` that ran (whose `op` is the *inverse* of
    /// the original — i.e., the op that just got applied to revert
    /// the document). The original op is now on the redo stack.
    pub fn undo(&mut self) -> Result<Option<AppliedOperation>, OperationError> {
        let Some(original) = self.history.pop_for_undo() else {
            return Ok(None);
        };
        let undo_applied = apply::apply(&mut self.document, &original.inverse)?;
        // The undo's inverse is the original op — pushing the
        // *original* applied entry onto redo lets a subsequent
        // `redo()` re-run the same forward op (and its inverse is
        // already cached on `original`).
        self.history.record_redo(original);
        self.notifier.notify(&undo_applied);
        Ok(Some(undo_applied))
    }

    /// Redo the most recently undone op. Symmetric to `undo`.
    pub fn redo(&mut self) -> Result<Option<AppliedOperation>, OperationError> {
        let Some(original) = self.history.pop_for_redo() else {
            return Ok(None);
        };
        let redo_applied = apply::apply(&mut self.document, &original.op)?;
        self.history.record_after_redo(redo_applied.clone());
        self.notifier.notify(&redo_applied);
        Ok(Some(redo_applied))
    }

    /// Convenience: wrap in `Rc<RefCell<>>` for the WASM/JS shared-
    /// ownership model.
    pub fn into_shared(self) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::{BTreeMap, HashMap};

    use bytes::Bytes;
    use idml_parse::{
        Bounds, Container, DesignMap, Graphic, PathAnchor, Polygon, Spread, StyleSheet,
        TextFrame as ParsedTextFrame,
    };
    use idml_scene::{ParsedSpread, ParsedStory};
    use crate::operation::PathAnchorSpec;
    use crate::path_math::smooth_handles_from_neighbours;

    // ---- Fixtures ---------------------------------------------------------

    fn empty_text_frame(self_id: &str, bounds: Bounds) -> ParsedTextFrame {
        ParsedTextFrame {
            self_id: Some(self_id.to_string()),
            parent_story: None,
            bounds,
            item_transform: None,
            fill_color: None,
            fill_tint: None,
            stroke_color: None,
            stroke_weight: None,
            stroke_type: None,
            drop_shadow: None,
            stroke_drop_shadow: None,
            next_text_frame: None,
            vertical_justification: None,
            first_baseline_offset: None,
            minimum_first_baseline_offset: None,
            inset_spacing: None,
            auto_sizing: None,
            auto_sizing_reference_point: None,
            minimum_width_for_auto_sizing: None,
            minimum_height_for_auto_sizing: None,
            use_minimum_height_for_auto_sizing: None,
            applied_object_style: None,
            text_wrap: None,
            item_layer: None,
            is_anchored: false,
            opacity: None,
            blend_mode: None,
            anchors: Vec::new(),
            subpath_starts: Vec::new(),
            subpath_open: Vec::new(),
            effects: None,
            gradient_fill_angle: None,
            gradient_fill_length: None,
            gradient_stroke_angle: None,
            gradient_stroke_length: None,
            applied_toc_style: None,
            overprint_fill: false,
            overprint_stroke: false,
        }
    }

    fn document_with_one_textframe(self_id: &str) -> Document {
        let mut spread = Spread::default();
        spread.self_id = Some("Spread/u_main".to_string());
        spread.text_frames.push(empty_text_frame(
            self_id,
            Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 100.0,
                right: 200.0,
            },
        ));

        Document {
            container: Container {
                mimetype: "application/vnd.adobe.indesign-idml-package".to_string(),
                designmap_raw: Bytes::new(),
                designmap: DesignMap::default(),
                entries: BTreeMap::new(),
            },
            palette: Graphic::default(),
            spreads: vec![ParsedSpread {
                src: "Spreads/syn.xml".to_string(),
                spread,
            }],
            stories: Vec::new(),
            master_spreads: HashMap::new(),
            frame_for_story: HashMap::new(),
            text_frame_index: HashMap::new(),
            styles: StyleSheet::default(),
            anchors: Vec::new(),
        }
    }

    fn set_bounds_op(self_id: &str, b: [f32; 4]) -> Operation {
        Operation::SetProperty {
            node: NodeId::TextFrame(self_id.to_string()),
            path: PropertyPath::FrameBounds,
            value: Value::Bounds(b),
        }
    }

    fn set_fill_op(self_id: &str, color: Option<&str>) -> Operation {
        Operation::SetProperty {
            node: NodeId::TextFrame(self_id.to_string()),
            path: PropertyPath::FrameFillColor,
            value: Value::ColorRef(color.map(String::from)),
        }
    }

    // ---- Migrated tests from the previous Mutation surface ---------------

    #[test]
    fn set_frame_bounds_updates_the_frame_and_returns_inverse() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));

        let applied = project
            .apply(set_bounds_op("TextFrame/u1", [10.0, 20.0, 110.0, 220.0]))
            .expect("apply must succeed");

        assert_eq!(
            applied.inverse,
            set_bounds_op("TextFrame/u1", [0.0, 0.0, 100.0, 200.0])
        );
        assert_eq!(applied.invalidation.frame_geometry.len(), 1);

        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.bounds.top, 10.0);
        assert_eq!(frame.bounds.left, 20.0);
        assert_eq!(frame.bounds.bottom, 110.0);
        assert_eq!(frame.bounds.right, 220.0);
    }

    #[test]
    fn set_frame_transform_sets_matrix_and_inverse_carries_previous() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let m = [
            0.7071, 0.7071, -0.7071, 0.7071, 50.0, 100.0,
        ];
        let applied = project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameTransform,
                value: Value::Transform(Some(m)),
            })
            .expect("apply");
        // Inverse carries the previous transform — `None` since the
        // freshly-built fixture has no ItemTransform.
        assert_eq!(
            applied.inverse,
            Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameTransform,
                value: Value::Transform(None),
            }
        );
        assert_eq!(applied.invalidation.frame_geometry.len(), 1);
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.item_transform, Some(m));
    }

    #[test]
    fn frame_transform_apply_inverse_restores() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameTransform,
                value: Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 5.0, 10.0])),
            })
            .unwrap();
        project.undo().expect("undo");
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.item_transform, None);
    }

    #[test]
    fn clone_translate_duplicates_source_with_shifted_bounds_and_unique_id() {
        // Phase H — Alt-duplicate translate. Source: TextFrame/u1.
        // The freshly-built fixture has u1 at bounds [0,0,100,200].
        // Insert a clone at bounds + (10, 20).
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let applied = project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 1,
                node: NodeSpec::CloneTranslate {
                    self_id: "TextFrame/u1_dup".to_string(),
                    source: NodeId::TextFrame("TextFrame/u1".to_string()),
                    dx: 10.0,
                    dy: 20.0,
                    destination_spread_id: None,
                },
            })
            .expect("apply clone translate");
        assert!(applied.invalidation.structural);
        // Original stays put.
        let orig = project.document().spreads[0]
            .spread
            .text_frames
            .iter()
            .find(|f| f.self_id.as_deref() == Some("TextFrame/u1"))
            .expect("original still there");
        assert_eq!(orig.bounds.top, 0.0);
        // Duplicate shifted by (10, 20).
        let dup = project.document().spreads[0]
            .spread
            .text_frames
            .iter()
            .find(|f| f.self_id.as_deref() == Some("TextFrame/u1_dup"))
            .expect("duplicate exists");
        assert_eq!(dup.bounds.top, 20.0);
        assert_eq!(dup.bounds.left, 10.0);
        assert_eq!(dup.bounds.bottom, 120.0);
        assert_eq!(dup.bounds.right, 210.0);
        // Undo removes the duplicate; original stays.
        project.undo().expect("undo");
        let after_undo = &project.document().spreads[0].spread.text_frames;
        assert_eq!(after_undo.len(), 1);
        assert_eq!(after_undo[0].self_id.as_deref(), Some("TextFrame/u1"));
    }

    #[test]
    fn set_image_content_transform_routes_to_rectangle_image_transform() {
        // Phase F — Rectangles host the image and carry the inner
        // image_item_transform. The fixture starts with no rectangle,
        // so insert one first via the existing InsertNode path, then
        // exercise the new property.
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        // Insert a Rectangle into the spread.
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: NodeSpec::Rectangle {
                    self_id: "Rectangle/r1".to_string(),
                    bounds: [0.0, 0.0, 100.0, 100.0],
                    fill_color: None,
                },
            })
            .expect("insert rect");
        // Apply the new transform.
        let m = [1.5, 0.0, 0.0, 1.5, 25.0, -10.0];
        let applied = project
            .apply(Operation::SetProperty {
                node: NodeId::Rectangle("Rectangle/r1".to_string()),
                path: PropertyPath::ImageContentTransform,
                value: Value::Transform(Some(m)),
            })
            .expect("apply");
        assert_eq!(
            applied.inverse,
            Operation::SetProperty {
                node: NodeId::Rectangle("Rectangle/r1".to_string()),
                path: PropertyPath::ImageContentTransform,
                value: Value::Transform(None),
            }
        );
        assert_eq!(applied.invalidation.frame_geometry.len(), 1);
        let rect = &project.document().spreads[0].spread.rectangles[0];
        assert_eq!(rect.image_item_transform, Some(m));
        // Undo restores to None.
        project.undo().expect("undo");
        let rect = &project.document().spreads[0].spread.rectangles[0];
        assert_eq!(rect.image_item_transform, None);
    }

    #[test]
    fn frame_transform_type_mismatch_errors_cleanly() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let err = project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameTransform,
                value: Value::Bounds([0.0, 0.0, 1.0, 1.0]),
            })
            .expect_err("must reject mismatched value");
        assert!(matches!(err, OperationError::TypeMismatch { .. }));
    }

    #[test]
    fn set_frame_fill_color_round_trips_through_inverse() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let applied = project
            .apply(set_fill_op("TextFrame/u1", Some("Color/Red")))
            .unwrap();

        assert_eq!(applied.inverse, set_fill_op("TextFrame/u1", None));
        assert_eq!(applied.invalidation.frame_style.len(), 1);
        assert_eq!(
            project.document().spreads[0].spread.text_frames[0]
                .fill_color
                .as_deref(),
            Some("Color/Red")
        );
    }

    #[test]
    fn unknown_node_returns_not_found_error() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let err = project
            .apply(set_bounds_op("TextFrame/missing", [0.0, 0.0, 1.0, 1.0]))
            .unwrap_err();
        assert!(matches!(err, OperationError::NodeNotFound(_)));
    }

    #[test]
    fn notifier_fires_once_per_apply() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let counter = Rc::new(Cell::new(0));
        {
            let counter = counter.clone();
            project.notifier_mut().subscribe(move |_applied| {
                counter.set(counter.get() + 1);
            });
        }
        project
            .apply(set_bounds_op("TextFrame/u1", [1.0, 2.0, 3.0, 4.0]))
            .unwrap();
        assert_eq!(counter.get(), 1);
    }

    // ---- New invariants: invert round-trip -------------------------------

    #[test]
    fn applying_inverse_restores_the_document() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let applied = project
            .apply(set_bounds_op("TextFrame/u1", [10.0, 20.0, 110.0, 220.0]))
            .unwrap();

        // Apply the inverse directly via the free function — exercises
        // that invert is correct independent of the history stack.
        crate::apply(project.document_mut(), &applied.inverse).unwrap();

        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.bounds.top, 0.0);
        assert_eq!(frame.bounds.left, 0.0);
        assert_eq!(frame.bounds.bottom, 100.0);
        assert_eq!(frame.bounds.right, 200.0);
    }

    // ---- Undo / redo -----------------------------------------------------

    #[test]
    fn undo_restores_previous_state_and_populates_redo() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));

        project
            .apply(set_fill_op("TextFrame/u1", Some("Color/Red")))
            .unwrap();
        assert_eq!(
            project.document().spreads[0].spread.text_frames[0]
                .fill_color
                .as_deref(),
            Some("Color/Red")
        );

        project.undo().unwrap().expect("had one op to undo");
        assert_eq!(
            project.document().spreads[0].spread.text_frames[0].fill_color,
            None
        );
        assert_eq!(project.history().undo_len(), 0);
        assert_eq!(project.history().redo_len(), 1);

        project.redo().unwrap().expect("had one op to redo");
        assert_eq!(
            project.document().spreads[0].spread.text_frames[0]
                .fill_color
                .as_deref(),
            Some("Color/Red")
        );
        assert_eq!(project.history().undo_len(), 1);
        assert_eq!(project.history().redo_len(), 0);
    }

    #[test]
    fn new_apply_clears_redo_branch() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(set_fill_op("TextFrame/u1", Some("Color/Red")))
            .unwrap();
        project.undo().unwrap();
        assert_eq!(project.history().redo_len(), 1);

        project
            .apply(set_fill_op("TextFrame/u1", Some("Color/Blue")))
            .unwrap();
        assert_eq!(project.history().redo_len(), 0);
    }

    #[test]
    fn undo_on_empty_history_is_a_noop() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let out = project.undo().unwrap();
        assert!(out.is_none());
    }

    // ---- Batch -----------------------------------------------------------

    #[test]
    fn batch_applies_children_and_produces_one_undo_entry() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let batch = Operation::Batch {
            ops: vec![
                set_bounds_op("TextFrame/u1", [1.0, 2.0, 3.0, 4.0]),
                set_fill_op("TextFrame/u1", Some("Color/Red")),
            ],
        };
        let applied = project.apply(batch).unwrap();
        assert!(matches!(applied.op, Operation::Batch { .. }));
        assert_eq!(project.history().undo_len(), 1);

        // Both children landed.
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.bounds.top, 1.0);
        assert_eq!(frame.fill_color.as_deref(), Some("Color/Red"));
    }

    #[test]
    fn batch_fires_notifier_exactly_once() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let counter = Rc::new(Cell::new(0));
        {
            let c = counter.clone();
            project.notifier_mut().subscribe(move |_| c.set(c.get() + 1));
        }
        let batch = Operation::Batch {
            ops: vec![
                set_bounds_op("TextFrame/u1", [1.0, 2.0, 3.0, 4.0]),
                set_fill_op("TextFrame/u1", Some("Color/Red")),
                set_fill_op("TextFrame/u1", None),
            ],
        };
        project.apply(batch).unwrap();
        assert_eq!(counter.get(), 1);
    }

    #[test]
    fn batch_with_failing_child_rolls_back_prior_children() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let original_color = project.document().spreads[0].spread.text_frames[0]
            .fill_color
            .clone();

        let batch = Operation::Batch {
            ops: vec![
                set_fill_op("TextFrame/u1", Some("Color/Red")),
                set_bounds_op("TextFrame/u1", [10.0, 20.0, 110.0, 220.0]),
                // Third child targets a missing node — this is the failure point.
                set_fill_op("TextFrame/missing", Some("Color/Blue")),
            ],
        };

        let err = project.apply(batch).unwrap_err();
        match err {
            OperationError::BatchFailed { failed_at, .. } => assert_eq!(failed_at, 2),
            other => panic!("expected BatchFailed, got {other:?}"),
        }

        // First two children should be rolled back.
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.fill_color, original_color);
        assert_eq!(frame.bounds.top, 0.0);
        assert_eq!(frame.bounds.left, 0.0);

        // Failed apply must leave the undo stack untouched.
        assert_eq!(project.history().undo_len(), 0);
    }

    #[test]
    fn batch_undo_reverses_all_children() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let batch = Operation::Batch {
            ops: vec![
                set_bounds_op("TextFrame/u1", [1.0, 2.0, 3.0, 4.0]),
                set_fill_op("TextFrame/u1", Some("Color/Red")),
            ],
        };
        project.apply(batch).unwrap();

        project.undo().unwrap();
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.bounds.top, 0.0);
        assert_eq!(frame.fill_color, None);
    }

    // ---- Insert / Remove / Move ------------------------------------------

    #[test]
    fn insert_node_adds_a_text_frame_to_the_spread() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let op = Operation::InsertNode {
            parent: NodeId::Spread("Spread/u_main".to_string()),
            position: 1,
            node: NodeSpec::TextFrame {
                self_id: "TextFrame/u_new".to_string(),
                bounds: [10.0, 20.0, 30.0, 40.0],
                fill_color: None,
            },
        };
        project.apply(op).unwrap();
        assert_eq!(project.document().spreads[0].spread.text_frames.len(), 2);
        assert_eq!(
            project.document().spreads[0].spread.text_frames[1]
                .self_id
                .as_deref(),
            Some("TextFrame/u_new")
        );
    }

    #[test]
    fn insert_with_duplicate_self_id_fails() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let op = Operation::InsertNode {
            parent: NodeId::Spread("Spread/u_main".to_string()),
            position: 0,
            node: NodeSpec::TextFrame {
                self_id: "TextFrame/u1".to_string(),
                bounds: [0.0, 0.0, 1.0, 1.0],
                fill_color: None,
            },
        };
        let err = project.apply(op).unwrap_err();
        assert!(matches!(err, OperationError::DuplicateNodeId { .. }));
    }

    #[test]
    fn remove_then_undo_restores_the_node() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(set_fill_op("TextFrame/u1", Some("Color/Red")))
            .unwrap();

        project
            .apply(Operation::RemoveNode {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
            })
            .unwrap();
        assert_eq!(project.document().spreads[0].spread.text_frames.len(), 0);

        project.undo().unwrap();
        assert_eq!(project.document().spreads[0].spread.text_frames.len(), 1);
        let restored = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(restored.self_id.as_deref(), Some("TextFrame/u1"));
        // Stage 1: fill_color round-trips through the captured NodeSpec.
        assert_eq!(restored.fill_color.as_deref(), Some("Color/Red"));
    }

    #[test]
    fn move_node_within_spread_reorders_then_undoes() {
        // Two frames in one spread.
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 1,
                node: NodeSpec::TextFrame {
                    self_id: "TextFrame/u2".to_string(),
                    bounds: [0.0, 0.0, 1.0, 1.0],
                    fill_color: None,
                },
            })
            .unwrap();

        // Move u1 from index 0 to index 1.
        project
            .apply(Operation::MoveNode {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                new_parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 1,
            })
            .unwrap();

        let order: Vec<_> = project.document().spreads[0]
            .spread
            .text_frames
            .iter()
            .map(|f| f.self_id.clone().unwrap_or_default())
            .collect();
        assert_eq!(order, vec!["TextFrame/u2", "TextFrame/u1"]);

        // Undo the move.
        project.undo().unwrap();
        let order: Vec<_> = project.document().spreads[0]
            .spread
            .text_frames
            .iter()
            .map(|f| f.self_id.clone().unwrap_or_default())
            .collect();
        assert_eq!(order, vec!["TextFrame/u1", "TextFrame/u2"]);
    }

    // ---- Serde round-trip ------------------------------------------------

    #[test]
    fn serde_round_trip_for_every_variant() {
        let ops = vec![
            set_bounds_op("TextFrame/u1", [1.0, 2.0, 3.0, 4.0]),
            set_fill_op("TextFrame/u1", Some("Color/Red")),
            Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 1,
                node: NodeSpec::TextFrame {
                    self_id: "TextFrame/u_new".to_string(),
                    bounds: [10.0, 20.0, 30.0, 40.0],
                    fill_color: Some("Color/Blue".to_string()),
                },
            },
            Operation::RemoveNode {
                node: NodeId::TextFrame("TextFrame/u_new".to_string()),
            },
            Operation::MoveNode {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                new_parent: NodeId::Spread("Spread/u_other".to_string()),
                position: 0,
            },
            Operation::Batch {
                ops: vec![
                    set_bounds_op("TextFrame/u1", [5.0, 6.0, 7.0, 8.0]),
                    set_fill_op("TextFrame/u1", None),
                ],
            },
            // Phase D — FrameTransform + Value::Transform variants.
            Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameTransform,
                value: Value::Transform(Some([1.5, 0.0, 0.0, 1.5, 12.5, -3.0])),
            },
            Operation::SetProperty {
                node: NodeId::Rectangle("Rectangle/u1".to_string()),
                path: PropertyPath::FrameTransform,
                value: Value::Transform(None),
            },
        ];

        for op in ops {
            let json = serde_json::to_string(&op).expect("serialize");
            let parsed: Operation = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, op, "round-trip failed for: {json}");
        }
    }

    // ---- Track K — cross-spread Alt-duplicate ---------------------------

    /// Build a Document with two spreads at distinct world origins.
    /// Source spread carries TextFrame `src_id` at the given bounds.
    /// The destination spread is empty (the apply path inserts into
    /// its text_frames vec when routed cross-spread).
    fn document_with_two_spreads(
        src_id: &str,
        src_bounds: Bounds,
        src_spread_origin: (f32, f32),
        dest_spread_origin: (f32, f32),
    ) -> Document {
        let mut src_spread = Spread::default();
        src_spread.self_id = Some("Spread/u_src".to_string());
        src_spread.item_transform = Some([
            1.0,
            0.0,
            0.0,
            1.0,
            src_spread_origin.0,
            src_spread_origin.1,
        ]);
        src_spread
            .text_frames
            .push(empty_text_frame(src_id, src_bounds));

        let mut dest_spread = Spread::default();
        dest_spread.self_id = Some("Spread/u_dest".to_string());
        dest_spread.item_transform = Some([
            1.0,
            0.0,
            0.0,
            1.0,
            dest_spread_origin.0,
            dest_spread_origin.1,
        ]);

        Document {
            container: Container {
                mimetype: "application/vnd.adobe.indesign-idml-package".to_string(),
                designmap_raw: Bytes::new(),
                designmap: DesignMap::default(),
                entries: BTreeMap::new(),
            },
            palette: Graphic::default(),
            spreads: vec![
                ParsedSpread {
                    src: "Spreads/u_src.xml".to_string(),
                    spread: src_spread,
                },
                ParsedSpread {
                    src: "Spreads/u_dest.xml".to_string(),
                    spread: dest_spread,
                },
            ],
            stories: Vec::new(),
            master_spreads: HashMap::new(),
            frame_for_story: HashMap::new(),
            text_frame_index: HashMap::new(),
            styles: StyleSheet::default(),
            anchors: Vec::new(),
        }
    }

    #[test]
    fn clone_translate_without_destination_preserves_phase_h_behaviour() {
        // AC-K-1 — same-spread Alt-duplicate, destination_spread_id =
        // None. Clone must land on the source spread with bounds
        // shifted by the raw (dx, dy). Identical to the Phase H
        // covering test, but on a doc with TWO spreads so the
        // "wrong spread" failure mode is visible.
        let src_bounds = Bounds {
            top: 0.0,
            left: 0.0,
            bottom: 100.0,
            right: 200.0,
        };
        let mut project = Project::new(document_with_two_spreads(
            "TextFrame/u1",
            src_bounds,
            (0.0, 0.0),
            (1000.0, 0.0),
        ));
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_src".to_string()),
                position: 1,
                node: NodeSpec::CloneTranslate {
                    self_id: "TextFrame/u1_dup".to_string(),
                    source: NodeId::TextFrame("TextFrame/u1".to_string()),
                    dx: 10.0,
                    dy: 20.0,
                    destination_spread_id: None,
                },
            })
            .expect("apply same-spread clone");
        // Duplicate on the SOURCE spread (index 0).
        let src_spread = &project.document().spreads[0].spread;
        let dest_spread = &project.document().spreads[1].spread;
        assert_eq!(src_spread.text_frames.len(), 2);
        assert_eq!(dest_spread.text_frames.len(), 0);
        let dup = &src_spread.text_frames[1];
        // Bounds shifted exactly by (dx, dy) — no spread-origin
        // correction in the None path.
        assert_eq!(dup.bounds.left, 10.0);
        assert_eq!(dup.bounds.top, 20.0);
    }

    #[test]
    fn clone_translate_with_destination_routes_to_dest_with_corrected_delta() {
        // AC-K-2 — cross-spread Alt-duplicate. Source spread at
        // world (0,0); destination at world (1000, 0). A drag of
        // (1050, 30) world-delta should land the clone on the
        // destination spread with bounds shifted by the EFFECTIVE
        // delta (1050 + 0 - 1000, 30 + 0 - 0) = (50, 30) — i.e.
        // 50 pt right + 30 pt down of the source-frame's
        // position INSIDE the destination spread.
        let src_bounds = Bounds {
            top: 0.0,
            left: 0.0,
            bottom: 100.0,
            right: 200.0,
        };
        let mut project = Project::new(document_with_two_spreads(
            "TextFrame/u1",
            src_bounds,
            (0.0, 0.0),
            (1000.0, 0.0),
        ));
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_dest".to_string()),
                position: 0,
                node: NodeSpec::CloneTranslate {
                    self_id: "TextFrame/u1_dup".to_string(),
                    source: NodeId::TextFrame("TextFrame/u1".to_string()),
                    dx: 1050.0,
                    dy: 30.0,
                    destination_spread_id: Some("Spread/u_dest".to_string()),
                },
            })
            .expect("apply cross-spread clone");
        let src_spread = &project.document().spreads[0].spread;
        let dest_spread = &project.document().spreads[1].spread;
        // Source spread still has only its original frame.
        assert_eq!(src_spread.text_frames.len(), 1);
        assert_eq!(src_spread.text_frames[0].self_id.as_deref(), Some("TextFrame/u1"));
        // Destination spread now hosts the duplicate.
        assert_eq!(dest_spread.text_frames.len(), 1);
        let dup = &dest_spread.text_frames[0];
        assert_eq!(dup.self_id.as_deref(), Some("TextFrame/u1_dup"));
        // Bounds shifted by EFFECTIVE delta (50, 30) — i.e. (dx +
        // src_origin - dest_origin, dy + src_origin - dest_origin).
        assert_eq!(dup.bounds.left, 50.0);
        assert_eq!(dup.bounds.top, 30.0);
        assert_eq!(dup.bounds.right, 250.0);
        assert_eq!(dup.bounds.bottom, 130.0);
    }

    #[test]
    fn cross_spread_clone_undo_removes_from_dest() {
        // AC-K-4 — undo a cross-spread Alt-duplicate. The inverse
        // is RemoveNode(self_id); the apply layer must find the
        // duplicate on the DESTINATION spread (not the source's)
        // and remove it there. Both spreads land back at their
        // pre-clone counts.
        let src_bounds = Bounds {
            top: 0.0,
            left: 0.0,
            bottom: 100.0,
            right: 200.0,
        };
        let mut project = Project::new(document_with_two_spreads(
            "TextFrame/u1",
            src_bounds,
            (0.0, 0.0),
            (1000.0, 0.0),
        ));
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_dest".to_string()),
                position: 0,
                node: NodeSpec::CloneTranslate {
                    self_id: "TextFrame/u1_dup".to_string(),
                    source: NodeId::TextFrame("TextFrame/u1".to_string()),
                    dx: 1050.0,
                    dy: 30.0,
                    destination_spread_id: Some("Spread/u_dest".to_string()),
                },
            })
            .expect("apply cross-spread clone");
        project.undo().expect("undo");
        let src_spread = &project.document().spreads[0].spread;
        let dest_spread = &project.document().spreads[1].spread;
        assert_eq!(src_spread.text_frames.len(), 1);
        assert_eq!(dest_spread.text_frames.len(), 0);
    }

    // ---- Track L — group transform --------------------------------------

    /// Build a Document with a Group hosting two TextFrame leaves.
    /// Each frame carries the composed leaf transform pre-baked
    /// per `idml-parse/spread.rs:141-144`. The Group's own
    /// `item_transform` is what L.1 mutates.
    fn document_with_group(group_xform: Option<[f32; 6]>) -> Document {
        let mut spread = Spread::default();
        spread.self_id = Some("Spread/u_main".to_string());
        spread.text_frames.push(empty_text_frame(
            "TextFrame/leaf_a",
            Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 50.0,
                right: 50.0,
            },
        ));
        spread.text_frames.push(empty_text_frame(
            "TextFrame/leaf_b",
            Bounds {
                top: 0.0,
                left: 60.0,
                bottom: 50.0,
                right: 110.0,
            },
        ));
        spread.groups.push(idml_parse::Group {
            self_id: Some("Group/g1".to_string()),
            members: vec![
                idml_parse::FrameRef::TextFrame(0),
                idml_parse::FrameRef::TextFrame(1),
            ],
            transparency: idml_parse::GroupTransparency::default(),
            item_transform: group_xform,
        });
        Document {
            container: Container {
                mimetype: "application/vnd.adobe.indesign-idml-package".to_string(),
                designmap_raw: Bytes::new(),
                designmap: DesignMap::default(),
                entries: BTreeMap::new(),
            },
            palette: Graphic::default(),
            spreads: vec![ParsedSpread {
                src: "Spreads/syn.xml".to_string(),
                spread,
            }],
            stories: Vec::new(),
            master_spreads: HashMap::new(),
            frame_for_story: HashMap::new(),
            text_frame_index: HashMap::new(),
            styles: StyleSheet::default(),
            anchors: Vec::new(),
        }
    }

    fn group_xform_op(self_id: &str, xform: Option<[f32; 6]>) -> Operation {
        Operation::SetProperty {
            node: NodeId::Group(self_id.to_string()),
            path: PropertyPath::FrameTransform,
            value: Value::Transform(xform),
        }
    }

    #[test]
    fn set_group_transform_mutates_group_and_inverse_carries_previous() {
        let mut project = Project::new(document_with_group(None));
        let m = [0.7071, 0.7071, -0.7071, 0.7071, 0.0, 0.0];
        let applied = project
            .apply(group_xform_op("Group/g1", Some(m)))
            .expect("apply group transform");
        // Group's transform set.
        let group = &project.document().spreads[0].spread.groups[0];
        assert_eq!(group.item_transform, Some(m));
        // Inverse carries previous (None).
        assert_eq!(
            applied.inverse,
            group_xform_op("Group/g1", None),
        );
        // Leaves untouched at the apply layer — the gesture spine
        // (L.2) emits their rebases as separate Batch children.
        let frames = &project.document().spreads[0].spread.text_frames;
        assert_eq!(frames[0].item_transform, None);
        assert_eq!(frames[1].item_transform, None);
    }

    #[test]
    fn set_group_transform_round_trips_through_inverse() {
        let m0 = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        let m1 = [0.7071, 0.7071, -0.7071, 0.7071, 0.0, 0.0];
        let mut project = Project::new(document_with_group(Some(m0)));
        let applied = project
            .apply(group_xform_op("Group/g1", Some(m1)))
            .unwrap();
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        let group = &project.document().spreads[0].spread.groups[0];
        assert_eq!(group.item_transform, Some(m0));
    }

    #[test]
    fn group_transform_apply_to_missing_id_fails() {
        let mut project = Project::new(document_with_group(None));
        let err = project
            .apply(group_xform_op("Group/missing", Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])))
            .unwrap_err();
        assert!(matches!(err, OperationError::NodeNotFound(_)));
    }

    // ---- Track J — path topology ----------------------------------------

    /// Build a fresh Polygon fixture with the given anchors +
    /// subpath_starts (subpath_open mirrors length). Other fields
    /// default to "none" — these tests only exercise the anchor
    /// table.
    fn polygon_with_anchors(
        self_id: &str,
        anchors: Vec<PathAnchor>,
        subpath_starts: Vec<usize>,
    ) -> Polygon {
        let open_flags = vec![false; subpath_starts.len().max(1)];
        Polygon {
            self_id: Some(self_id.to_string()),
            bounds: Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 100.0,
                right: 100.0,
            },
            item_transform: None,
            fill_color: None,
            fill_tint: None,
            stroke_color: None,
            stroke_weight: None,
            stroke_type: None,
            applied_object_style: None,
            anchors,
            subpath_starts,
            subpath_open: open_flags,
            text_wrap: None,
            item_layer: None,
            effects: None,
            gradient_fill_angle: None,
            gradient_fill_length: None,
            gradient_stroke_angle: None,
            gradient_stroke_length: None,
            opacity: None,
            blend_mode: None,
            text_paths: Vec::new(),
            image_link: None,
            image_bytes: None,
            has_image_element: false,
            has_inline_pdf: false,
            image_item_transform: None,
            overprint_fill: false,
            overprint_stroke: false,
        }
    }

    fn anchor_at(x: f32, y: f32) -> PathAnchor {
        PathAnchor {
            anchor: (x, y),
            left: (x, y),
            right: (x, y),
        }
    }

    /// Project hosting a single Polygon with the given anchors +
    /// subpath_starts. Caller picks the polygon's self_id.
    fn project_with_polygon(
        self_id: &str,
        anchors: Vec<PathAnchor>,
        subpath_starts: Vec<usize>,
    ) -> Project {
        let mut spread = Spread::default();
        spread.self_id = Some("Spread/u_main".to_string());
        spread
            .polygons
            .push(polygon_with_anchors(self_id, anchors, subpath_starts));
        let doc = Document {
            container: Container {
                mimetype: "application/vnd.adobe.indesign-idml-package".to_string(),
                designmap_raw: Bytes::new(),
                designmap: DesignMap::default(),
                entries: BTreeMap::new(),
            },
            palette: Graphic::default(),
            spreads: vec![ParsedSpread {
                src: "Spreads/syn.xml".to_string(),
                spread,
            }],
            stories: Vec::new(),
            master_spreads: HashMap::new(),
            frame_for_story: HashMap::new(),
            text_frame_index: HashMap::new(),
            styles: StyleSheet::default(),
            anchors: Vec::new(),
        };
        Project::new(doc)
    }

    fn polygon_of<'a>(project: &'a Project) -> &'a Polygon {
        &project.document().spreads[0].spread.polygons[0]
    }

    fn anchor_positions(p: &Polygon) -> Vec<(f32, f32)> {
        p.anchors.iter().map(|a| a.anchor).collect()
    }

    fn insert_op(self_id: &str, index: usize, anchor: PathAnchorSpec) -> Operation {
        Operation::SetProperty {
            node: NodeId::Polygon(self_id.to_string()),
            path: PropertyPath::PathPointInsert,
            value: Value::PathPointInsert {
                index,
                anchor,
                prev_subpath_starts: None,
            },
        }
    }

    fn insert_op_with_starts(
        self_id: &str,
        index: usize,
        anchor: PathAnchorSpec,
        prev_subpath_starts: Vec<usize>,
    ) -> Operation {
        Operation::SetProperty {
            node: NodeId::Polygon(self_id.to_string()),
            path: PropertyPath::PathPointInsert,
            value: Value::PathPointInsert {
                index,
                anchor,
                prev_subpath_starts: Some(prev_subpath_starts),
            },
        }
    }

    fn remove_op(self_id: &str, index: usize) -> Operation {
        Operation::SetProperty {
            node: NodeId::Polygon(self_id.to_string()),
            path: PropertyPath::PathPointRemove,
            value: Value::PathPointRemove {
                index,
                prev_subpath_starts: None,
            },
        }
    }

    fn curve_op(self_id: &str, index: usize, smooth: bool) -> Operation {
        Operation::SetProperty {
            node: NodeId::Polygon(self_id.to_string()),
            path: PropertyPath::PathPointCurveType,
            value: Value::PathPointCurveType {
                index,
                smooth,
                prev: None,
            },
        }
    }

    #[test]
    fn insert_grows_anchors_and_returns_remove_inverse() {
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![anchor_at(0.0, 0.0), anchor_at(10.0, 0.0)],
            vec![],
        );
        let new_anchor = PathAnchorSpec {
            anchor: [5.0, 0.0],
            left: [3.0, 0.0],
            right: [7.0, 0.0],
        };
        let applied = project
            .apply(insert_op("Polygon/p1", 1, new_anchor))
            .expect("insert");
        // Anchor count grew.
        assert_eq!(polygon_of(&project).anchors.len(), 3);
        assert_eq!(anchor_positions(polygon_of(&project))[1], (5.0, 0.0));
        // Inverse is a Remove at the same index (no prev_subpath_starts
        // because the forward op's increment rule was non-collapsing).
        assert_eq!(
            applied.inverse,
            Operation::SetProperty {
                node: NodeId::Polygon("Polygon/p1".to_string()),
                path: PropertyPath::PathPointRemove,
                value: Value::PathPointRemove {
                    index: 1,
                    prev_subpath_starts: None,
                },
            }
        );
    }

    #[test]
    fn closing_edge_insert_joins_prior_subpath_via_explicit_starts() {
        // Two subpaths starts=[0, 2], anchors=[A0, A1, B0, B1]. The
        // closing edge of subpath 0 runs from A1 (index 1) back to A0
        // (index 0); a click on it should land the midpoint at flat
        // index 2 (= subEnd) and bump starts[1] from 2 → 3, so the
        // new anchor stays inside subpath 0.
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![
                anchor_at(0.0, 0.0),
                anchor_at(1.0, 0.0),
                anchor_at(2.0, 2.0),
                anchor_at(3.0, 2.0),
            ],
            vec![0, 2],
        );
        let applied = project
            .apply(insert_op_with_starts(
                "Polygon/p1",
                2,
                PathAnchorSpec {
                    anchor: [0.5, 0.5],
                    left: [0.5, 0.5],
                    right: [0.5, 0.5],
                },
                vec![0, 3],
            ))
            .expect("closing-edge insert");
        let p = polygon_of(&project);
        assert_eq!(p.anchors.len(), 5);
        assert_eq!(p.anchors[2].anchor, (0.5, 0.5));
        assert_eq!(p.subpath_starts, vec![0, 3]);
        // Inverse round-trip: decrement rule shifts starts[1] back
        // from 3 → 2 (strictly-greater rule, n=2).
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        let p = polygon_of(&project);
        assert_eq!(p.anchors.len(), 4);
        assert_eq!(p.subpath_starts, vec![0, 2]);
    }

    #[test]
    fn closing_edge_insert_on_last_subpath_needs_no_override() {
        // Two subpaths starts=[0, 2], anchors=[A0, A1, B0, B1]. The
        // closing edge of subpath 1 (B1 → B0) inserts at flat index 4
        // (= anchors.len()) — no boundary entry exists at that index
        // so the standard increment rule (strictly-greater) leaves
        // starts unchanged. No override required.
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![
                anchor_at(0.0, 0.0),
                anchor_at(1.0, 0.0),
                anchor_at(2.0, 2.0),
                anchor_at(3.0, 2.0),
            ],
            vec![0, 2],
        );
        let applied = project
            .apply(insert_op(
                "Polygon/p1",
                4,
                PathAnchorSpec {
                    anchor: [2.5, 2.5],
                    left: [2.5, 2.5],
                    right: [2.5, 2.5],
                },
            ))
            .expect("last-subpath closing insert");
        let p = polygon_of(&project);
        assert_eq!(p.anchors.len(), 5);
        assert_eq!(p.anchors[4].anchor, (2.5, 2.5));
        assert_eq!(p.subpath_starts, vec![0, 2]);
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        let p = polygon_of(&project);
        assert_eq!(p.anchors.len(), 4);
        assert_eq!(p.subpath_starts, vec![0, 2]);
    }

    #[test]
    fn insert_shifts_subpath_starts_above_index() {
        // Two subpaths, starts at [0, 2]. Insert at index 1 (inside
        // subpath 0) → starts becomes [0, 3].
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![
                anchor_at(0.0, 0.0),
                anchor_at(1.0, 0.0),
                anchor_at(2.0, 2.0),
                anchor_at(3.0, 2.0),
            ],
            vec![0, 2],
        );
        project
            .apply(insert_op(
                "Polygon/p1",
                1,
                PathAnchorSpec {
                    anchor: [0.5, 0.0],
                    left: [0.5, 0.0],
                    right: [0.5, 0.0],
                },
            ))
            .unwrap();
        assert_eq!(polygon_of(&project).subpath_starts, vec![0, 3]);
    }

    #[test]
    fn remove_shrinks_anchors_and_round_trips_through_inverse() {
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![
                anchor_at(0.0, 0.0),
                anchor_at(5.0, 1.0),
                anchor_at(10.0, 0.0),
            ],
            vec![],
        );
        let before = polygon_of(&project).anchors.clone();
        let applied = project.apply(remove_op("Polygon/p1", 1)).expect("remove");
        // Anchor count shrunk; middle anchor gone.
        assert_eq!(polygon_of(&project).anchors.len(), 2);
        assert_eq!(anchor_positions(polygon_of(&project)), vec![(0.0, 0.0), (10.0, 0.0)]);
        // Inverse re-inserts the captured anchor at the same index
        // and restores subpath_starts verbatim.
        match &applied.inverse {
            Operation::SetProperty {
                path: PropertyPath::PathPointInsert,
                value: Value::PathPointInsert { index, anchor, prev_subpath_starts },
                ..
            } => {
                assert_eq!(*index, 1);
                assert_eq!(anchor.anchor, [5.0, 1.0]);
                assert!(prev_subpath_starts.is_some());
            }
            other => panic!("unexpected inverse shape: {:?}", other),
        }
        // Apply the inverse and confirm bytewise restore.
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        assert_eq!(polygon_of(&project).anchors.len(), 3);
        for (a, b) in polygon_of(&project).anchors.iter().zip(before.iter()) {
            assert_eq!(a.anchor, b.anchor);
            assert_eq!(a.left, b.left);
            assert_eq!(a.right, b.right);
        }
    }

    #[test]
    fn remove_that_collapses_degenerate_subpath_round_trips() {
        // anchors=[A, B, C], starts=[0, 2] — subpath 1 has the lone
        // anchor C. Remove index 2: anchors=[A, B], subpath 1 should
        // disappear. Undo must restore both anchors AND starts=[0, 2].
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![
                anchor_at(0.0, 0.0),
                anchor_at(5.0, 0.0),
                anchor_at(10.0, 10.0),
            ],
            vec![0, 2],
        );
        let applied = project.apply(remove_op("Polygon/p1", 2)).expect("remove");
        assert_eq!(polygon_of(&project).anchors.len(), 2);
        assert_eq!(polygon_of(&project).subpath_starts, vec![0]);
        // Inverse restores anchors AND starts.
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        assert_eq!(polygon_of(&project).anchors.len(), 3);
        assert_eq!(polygon_of(&project).subpath_starts, vec![0, 2]);
    }

    #[test]
    fn curve_type_smooth_derives_handles_from_neighbours() {
        // Three collinear anchors with corner handles. Toggle index
        // 1 to smooth → handles should land on the 1/3 / 1/3 tangent.
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![
                anchor_at(0.0, 0.0),
                anchor_at(5.0, 0.0),
                anchor_at(15.0, 0.0),
            ],
            vec![],
        );
        project
            .apply(curve_op("Polygon/p1", 1, true))
            .expect("smooth");
        let (l_expected, r_expected) =
            smooth_handles_from_neighbours([0.0, 0.0], [5.0, 0.0], [15.0, 0.0]);
        let a = &polygon_of(&project).anchors[1];
        assert!((a.left.0 - l_expected[0]).abs() < 1e-4);
        assert!((a.right.0 - r_expected[0]).abs() < 1e-4);
    }

    #[test]
    fn curve_type_corner_collapses_handles_to_anchor() {
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![PathAnchor {
                anchor: (5.0, 5.0),
                left: (3.0, 5.0),
                right: (7.0, 5.0),
            }],
            vec![],
        );
        project
            .apply(curve_op("Polygon/p1", 0, false))
            .expect("corner");
        let a = &polygon_of(&project).anchors[0];
        assert_eq!(a.left, (5.0, 5.0));
        assert_eq!(a.right, (5.0, 5.0));
    }

    #[test]
    fn curve_type_round_trip_restores_exact_handles() {
        // Set non-trivial handles, then smooth-toggle, then undo —
        // handles must come back exactly. The plan-2 default of
        // "inverse: previous flag" would silently re-derive on undo
        // and lose the original handles; the `prev: Some(...)` capture
        // exists to honour AC-J-5.
        let original = PathAnchor {
            anchor: (5.0, 0.0),
            left: (2.7, -1.1),
            right: (7.3, 1.1),
        };
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![anchor_at(0.0, 0.0), original, anchor_at(10.0, 0.0)],
            vec![],
        );
        let applied = project
            .apply(curve_op("Polygon/p1", 1, true))
            .expect("smooth");
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        let a = &polygon_of(&project).anchors[1];
        assert_eq!(a.anchor, original.anchor);
        assert_eq!(a.left, original.left);
        assert_eq!(a.right, original.right);
    }

    #[test]
    fn arbitrary_path_topology_sequence_round_trips_bytewise() {
        // Mix insert + remove + curve-type, then apply each inverse in
        // reverse order. The polygon must equal its initial state
        // anchor-by-anchor (including handles) and subpath_starts.
        let initial_anchors = vec![
            anchor_at(0.0, 0.0),
            anchor_at(5.0, 0.0),
            anchor_at(10.0, 0.0),
            anchor_at(20.0, 5.0),
            anchor_at(25.0, 5.0),
        ];
        let initial_starts = vec![0, 3];
        let mut project = project_with_polygon(
            "Polygon/p1",
            initial_anchors.clone(),
            initial_starts.clone(),
        );

        let mid_anchor = PathAnchorSpec {
            anchor: [22.5, 5.0],
            left: [21.0, 5.0],
            right: [24.0, 5.0],
        };
        let ops = vec![
            insert_op("Polygon/p1", 4, mid_anchor),    // inside subpath 1
            curve_op("Polygon/p1", 1, true),           // smooth-derive interior of subpath 0
            remove_op("Polygon/p1", 2),                // collapses nothing (subpath 0 still has 2 anchors)
        ];
        let mut applied_stack = Vec::new();
        for op in ops {
            applied_stack.push(project.apply(op).unwrap());
        }
        for entry in applied_stack.iter().rev() {
            crate::apply(project.document_mut(), &entry.inverse).unwrap();
        }
        let p = polygon_of(&project);
        assert_eq!(p.subpath_starts, initial_starts);
        assert_eq!(p.anchors.len(), initial_anchors.len());
        for (a, b) in p.anchors.iter().zip(initial_anchors.iter()) {
            assert_eq!(a.anchor, b.anchor);
            assert_eq!(a.left, b.left);
            assert_eq!(a.right, b.right);
        }
    }

    // ---- Lossless invariant: apply then apply-inverse restores doc -------

    #[test]
    fn arbitrary_sequence_then_reverse_inverses_restores_state() {
        // Apply N ops, accumulate their inverses, then apply the
        // inverses in reverse order. Document should equal the original.
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let original_bounds = project.document().spreads[0].spread.text_frames[0].bounds;
        let original_fill = project.document().spreads[0].spread.text_frames[0]
            .fill_color
            .clone();

        let ops = vec![
            set_bounds_op("TextFrame/u1", [10.0, 20.0, 30.0, 40.0]),
            set_fill_op("TextFrame/u1", Some("Color/Red")),
            set_bounds_op("TextFrame/u1", [50.0, 60.0, 70.0, 80.0]),
            set_fill_op("TextFrame/u1", Some("Color/Blue")),
        ];

        let mut applied: Vec<AppliedOperation> = Vec::new();
        for op in ops {
            applied.push(project.apply(op).unwrap());
        }

        for entry in applied.iter().rev() {
            crate::apply(project.document_mut(), &entry.inverse).unwrap();
        }

        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.bounds, original_bounds);
        assert_eq!(frame.fill_color, original_fill);
    }

    // ---- Track M — layer toggle ops ------------------------------------

    fn document_with_one_layer(self_id: &str) -> Document {
        let mut spread = Spread::default();
        spread.self_id = Some("Spread/u_main".to_string());
        let mut designmap = DesignMap::default();
        designmap.layers.push(idml_parse::Layer {
            self_id: self_id.to_string(),
            name: Some("Body".to_string()),
            visible: true,
            locked: false,
            printable: true,
        });
        Document {
            container: Container {
                mimetype: "application/vnd.adobe.indesign-idml-package".to_string(),
                designmap_raw: Bytes::new(),
                designmap,
                entries: BTreeMap::new(),
            },
            palette: Graphic::default(),
            spreads: vec![ParsedSpread {
                src: "Spreads/syn.xml".to_string(),
                spread,
            }],
            stories: Vec::new(),
            master_spreads: HashMap::new(),
            frame_for_story: HashMap::new(),
            text_frame_index: HashMap::new(),
            styles: StyleSheet::default(),
            anchors: Vec::new(),
        }
    }

    fn layer_op(self_id: &str, path: PropertyPath, value: bool) -> Operation {
        Operation::SetProperty {
            node: NodeId::Layer(self_id.to_string()),
            path,
            value: Value::Bool(value),
        }
    }

    fn layer_of<'a>(project: &'a Project) -> &'a idml_parse::Layer {
        &project.document().container.designmap.layers[0]
    }

    #[test]
    fn layer_visible_toggles_and_round_trips() {
        let mut project = Project::new(document_with_one_layer("ua"));
        assert!(layer_of(&project).visible);
        let applied = project
            .apply(layer_op("ua", PropertyPath::LayerVisible, false))
            .expect("toggle off");
        assert!(!layer_of(&project).visible);
        // Inverse restores visibility.
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        assert!(layer_of(&project).visible);
    }

    #[test]
    fn layer_locked_and_printable_toggle_and_round_trip() {
        let mut project = Project::new(document_with_one_layer("ua"));
        let lock = project
            .apply(layer_op("ua", PropertyPath::LayerLocked, true))
            .expect("lock");
        assert!(layer_of(&project).locked);
        let unprintable = project
            .apply(layer_op("ua", PropertyPath::LayerPrintable, false))
            .expect("non-printable");
        assert!(!layer_of(&project).printable);
        crate::apply(project.document_mut(), &unprintable.inverse).unwrap();
        crate::apply(project.document_mut(), &lock.inverse).unwrap();
        assert!(!layer_of(&project).locked);
        assert!(layer_of(&project).printable);
    }

    #[test]
    fn layer_toggle_with_missing_id_returns_not_found() {
        let mut project = Project::new(document_with_one_layer("ua"));
        let err = project
            .apply(layer_op("u_missing", PropertyPath::LayerVisible, false))
            .unwrap_err();
        assert!(matches!(err, OperationError::NodeNotFound(_)));
    }

    #[test]
    fn layer_name_round_trips() {
        let mut project = Project::new(document_with_one_layer("ua"));
        let applied = project
            .apply(Operation::SetProperty {
                node: NodeId::Layer("ua".to_string()),
                path: PropertyPath::LayerName,
                value: Value::Text("Sketch".to_string()),
            })
            .expect("rename");
        assert_eq!(layer_of(&project).name.as_deref(), Some("Sketch"));
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        assert_eq!(layer_of(&project).name.as_deref(), Some("Body"));
    }

    #[test]
    fn move_layer_reorders_and_inverts() {
        let mut project = {
            let doc = document_with_one_layer("ua");
            let mut p = Project::new(doc);
            p.document_mut()
                .container
                .designmap
                .layers
                .push(idml_parse::Layer {
                    self_id: "ub".to_string(),
                    name: Some("Guides".to_string()),
                    visible: true,
                    locked: false,
                    printable: true,
                });
            p
        };
        // Move "ub" to index 0 (becomes the topmost layer).
        let applied = project
            .apply(Operation::MoveLayer {
                layer_id: "ub".to_string(),
                new_index: 0,
            })
            .expect("move");
        let ids: Vec<_> = project
            .document()
            .container
            .designmap
            .layers
            .iter()
            .map(|l| l.self_id.clone())
            .collect();
        assert_eq!(ids, vec!["ub".to_string(), "ua".to_string()]);
        // Undo restores original order.
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        let ids: Vec<_> = project
            .document()
            .container
            .designmap
            .layers
            .iter()
            .map(|l| l.self_id.clone())
            .collect();
        assert_eq!(ids, vec!["ua".to_string(), "ub".to_string()]);
    }

    #[test]
    fn insert_layer_appends_and_inverts() {
        let mut project = Project::new(document_with_one_layer("ua"));
        let applied = project
            .apply(Operation::InsertLayer {
                position: 1,
                name: "New".to_string(),
                self_id: None,
            })
            .expect("insert");
        let layers = &project.document().container.designmap.layers;
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[1].name.as_deref(), Some("New"));
        // Inverse removes it.
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        assert_eq!(project.document().container.designmap.layers.len(), 1);
    }

    #[test]
    fn remove_layer_inverts_via_batch_restoring_flags() {
        let mut project = Project::new(document_with_one_layer("ua"));
        // Toggle every flag before removal so the inverse exercises
        // the flag-restore branch.
        project.document_mut().container.designmap.layers[0].locked = true;
        project.document_mut().container.designmap.layers[0].printable = false;
        let applied = project
            .apply(Operation::RemoveLayer {
                layer_id: "ua".to_string(),
            })
            .expect("remove");
        assert!(project.document().container.designmap.layers.is_empty());
        // Inverse restores the layer with its flags.
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        let layer = &project.document().container.designmap.layers[0];
        assert_eq!(layer.self_id, "ua");
        assert!(layer.locked);
        assert!(!layer.printable);
        assert_eq!(layer.name.as_deref(), Some("Body"));
    }

    // ---- Track J fan-out — path topology on non-Polygon kinds ----------

    /// Seed a TextFrame's anchors. `document_with_one_textframe`
    /// builds the frame with empty anchors; mutate the lone frame to
    /// install a few before running the path-topology op.
    fn project_with_textframe_anchors(
        self_id: &str,
        anchors: Vec<PathAnchor>,
        subpath_starts: Vec<usize>,
    ) -> Project {
        let doc = document_with_one_textframe(self_id);
        let mut project = Project::new(doc);
        let frame = &mut project.document_mut().spreads[0].spread.text_frames[0];
        frame.anchors = anchors;
        frame.subpath_open = vec![false; subpath_starts.len().max(1)];
        frame.subpath_starts = subpath_starts;
        project
    }

    fn textframe_of<'a>(project: &'a Project) -> &'a ParsedTextFrame {
        &project.document().spreads[0].spread.text_frames[0]
    }

    #[test]
    fn path_point_insert_on_textframe_grows_anchors_and_round_trips() {
        // Track J fan-out — the apply layer treats a TextFrame's
        // anchors + subpath_starts identically to a Polygon's. The
        // same Insert / Remove inverses round-trip bytewise.
        let mut project = project_with_textframe_anchors(
            "u_tf",
            vec![anchor_at(0.0, 0.0), anchor_at(10.0, 0.0)],
            vec![],
        );
        let op = Operation::SetProperty {
            node: NodeId::TextFrame("u_tf".to_string()),
            path: PropertyPath::PathPointInsert,
            value: Value::PathPointInsert {
                index: 1,
                anchor: PathAnchorSpec {
                    anchor: [5.0, 0.0],
                    left: [3.0, 0.0],
                    right: [7.0, 0.0],
                },
                prev_subpath_starts: None,
            },
        };
        let applied = project.apply(op).expect("textframe insert");
        assert_eq!(textframe_of(&project).anchors.len(), 3);
        assert_eq!(textframe_of(&project).anchors[1].anchor, (5.0, 0.0));
        // Inverse Remove restores bytewise.
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        assert_eq!(textframe_of(&project).anchors.len(), 2);
        assert_eq!(textframe_of(&project).anchors[0].anchor, (0.0, 0.0));
        assert_eq!(textframe_of(&project).anchors[1].anchor, (10.0, 0.0));
    }

    #[test]
    fn path_point_curve_type_on_textframe_smooths_handles() {
        // Three collinear corner anchors on a TextFrame. Smooth the
        // middle one → handles derive from neighbour tangents; undo
        // restores. Same code path as the Polygon test, exercised
        // through the TextFrame fan-out arm.
        let mut project = project_with_textframe_anchors(
            "u_tf",
            vec![
                anchor_at(0.0, 0.0),
                anchor_at(5.0, 0.0),
                anchor_at(15.0, 0.0),
            ],
            vec![],
        );
        let op = Operation::SetProperty {
            node: NodeId::TextFrame("u_tf".to_string()),
            path: PropertyPath::PathPointCurveType,
            value: Value::PathPointCurveType {
                index: 1,
                smooth: true,
                prev: None,
            },
        };
        let applied = project.apply(op).expect("smooth");
        let a = &textframe_of(&project).anchors[1];
        assert!((a.left.0 - a.anchor.0).abs() > 0.5);
        // Undo collapses handles back to the anchor (the original
        // anchor was a corner — left == right == anchor).
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        let a = &textframe_of(&project).anchors[1];
        assert_eq!(a.left, a.anchor);
        assert_eq!(a.right, a.anchor);
    }

    // ---- SDK Phase 3 — addressing-model surface --------------------------

    /// `NodeId::StoryRange` serializes with the IDML-conventional
    /// `kind` tag and round-trips through serde without losing
    /// `story_id` / `start` / `end`. The variant is only the wire
    /// surface today; the apply layer's character-path arms land in
    /// Phase 3 proper.
    #[test]
    fn story_range_node_id_serde_round_trips() {
        let node = NodeId::StoryRange {
            story_id: "Story/u123".to_string(),
            start: 12,
            end: 47,
        };
        let json = serde_json::to_string(&node).expect("serialize");
        // serde with #[serde(tag="kind", content="id")] turns the
        // NodeId into {"kind":"StoryRange","id":{...}} — the same
        // shape every other NodeId variant uses. The story_id +
        // start + end land inside the id payload.
        assert!(
            json.contains("\"StoryRange\""),
            "json should contain the tag: {json}"
        );
        let parsed: NodeId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, node);
    }

    /// Builds a Document with one parsed story containing three
    /// `CharacterRun`s split across two paragraphs:
    ///   paragraph 0: runs ["Hello ", "world"]  (offsets 0..6, 6..11)
    ///   paragraph 1: runs ["!"]                (offset 11..12)
    /// Used to exercise the character-property apply arm + the
    /// whole-run-only constraint.
    fn document_with_one_story(story_id: &str) -> Document {
        use idml_parse::{CharacterRun, Paragraph, Story};

        let mk_run = |text: &str| CharacterRun {
            text: text.to_string(),
            point_size: Some(10.0),
            leading: Some(12.0),
            tracking: Some(0.0),
            fill_color: Some("Color/Black".to_string()),
            ..CharacterRun::default()
        };

        let mut para1 = Paragraph::default();
        para1.runs.push(mk_run("Hello "));
        para1.runs.push(mk_run("world"));
        let mut para2 = Paragraph::default();
        para2.runs.push(mk_run("!"));

        let story = Story {
            paragraphs: vec![para1, para2],
            optical_margin_alignment: false,
            optical_margin_size: 0.0,
            story_direction: None,
        };

        Document {
            container: Container {
                mimetype: "application/vnd.adobe.indesign-idml-package".to_string(),
                designmap_raw: Bytes::new(),
                designmap: DesignMap::default(),
                entries: BTreeMap::new(),
            },
            palette: Graphic::default(),
            spreads: Vec::new(),
            stories: vec![ParsedStory {
                src: format!("Stories/Story_{story_id}.xml"),
                self_id: story_id.to_string(),
                story,
            }],
            master_spreads: HashMap::new(),
            frame_for_story: HashMap::new(),
            text_frame_index: HashMap::new(),
            styles: StyleSheet::default(),
            anchors: Vec::new(),
        }
    }

    /// Happy path: a SetProperty against a `StoryRange` covering the
    /// first run [0, 6) sets the new font size and returns an inverse
    /// that restores the prior value.
    #[test]
    fn character_font_size_applies_to_whole_run_range_and_undo_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::CharacterFontSize,
            value: Value::Length(Some(24.0)),
        };
        let applied = project.apply(op).expect("apply must succeed");

        // First run's point_size should now be 24.0; the second
        // ("world") and third ("!") runs stay at 10.0.
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[0].point_size, Some(24.0));
        assert_eq!(story.paragraphs[0].runs[1].point_size, Some(10.0));
        assert_eq!(story.paragraphs[1].runs[0].point_size, Some(10.0));

        // Inverse restores 10.0 (the original).
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[0].point_size, Some(10.0));
    }

    /// Multi-run range: a SetProperty against [0, 11) covers both
    /// runs of paragraph 0; the inverse is a Batch of two
    /// per-run restorations.
    #[test]
    fn character_font_size_applies_across_multiple_runs() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 11,
            },
            path: PropertyPath::CharacterFontSize,
            value: Value::Length(Some(14.0)),
        };
        let applied = project.apply(op).expect("apply must succeed");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[0].point_size, Some(14.0));
        assert_eq!(story.paragraphs[0].runs[1].point_size, Some(14.0));
        // Run beyond the range is unchanged.
        assert_eq!(story.paragraphs[1].runs[0].point_size, Some(10.0));

        // Inverse should be a Batch (two ops).
        assert!(matches!(&applied.inverse, Operation::Batch { ops } if ops.len() == 2));
    }

    /// SDK Phase 3.x — partial-range splitting. A range that cuts
    /// inside a `CharacterRun` now splits the run; the inverse
    /// Batch restores the property per (now-split-)run without
    /// re-merging. Verifies the first run "Hello " (0..6) splits
    /// at offset 2 and offset 4 into three pieces: "He" (0..2,
    /// unchanged), "ll" (2..4, mutated), "o " (4..6, unchanged).
    #[test]
    fn character_set_property_splits_runs_on_partial_range() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 2,
                end: 4,
            },
            path: PropertyPath::CharacterFontSize,
            value: Value::Length(Some(14.0)),
        };
        let applied = project.apply(op).expect("apply must succeed");

        // Paragraph 0 now has 4 runs: "He" + "ll" + "o " + "world".
        // The original "Hello " (0..6) split into three; "world"
        // (6..11) is untouched.
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs.len(), 4);
        assert_eq!(story.paragraphs[0].runs[0].text, "He");
        assert_eq!(story.paragraphs[0].runs[0].point_size, Some(10.0));
        assert_eq!(story.paragraphs[0].runs[1].text, "ll");
        assert_eq!(story.paragraphs[0].runs[1].point_size, Some(14.0));
        assert_eq!(story.paragraphs[0].runs[2].text, "o ");
        assert_eq!(story.paragraphs[0].runs[2].point_size, Some(10.0));
        assert_eq!(story.paragraphs[0].runs[3].text, "world");
        assert_eq!(story.paragraphs[0].runs[3].point_size, Some(10.0));

        // Inverse restores the mutated piece's point_size to 10.0.
        // It addresses range [2, 4) — the same range the forward op
        // mutated. Undo doesn't re-merge the splits; the document
        // keeps the boundary structure.
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        // Run count stays at 4 (no re-merge).
        assert_eq!(story.paragraphs[0].runs.len(), 4);
        // Every run's point_size is back to 10.0.
        for run in &story.paragraphs[0].runs {
            assert_eq!(run.point_size, Some(10.0));
        }
    }

    /// Left-only split: the range starts inside a run but extends
    /// past it. The run splits into [pre-start] [in-range], the
    /// second piece gets mutated.
    #[test]
    fn character_set_property_splits_left_only_when_range_ends_at_run_boundary() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        // Range [3, 6): cuts inside "Hello " at offset 3, ends at
        // its boundary. Should split into "Hel" + "lo ".
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 3,
                end: 6,
            },
            path: PropertyPath::CharacterFontSize,
            value: Value::Length(Some(20.0)),
        };
        project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        // Paragraph 0: "Hel" "lo " "world".
        assert_eq!(story.paragraphs[0].runs.len(), 3);
        assert_eq!(story.paragraphs[0].runs[0].text, "Hel");
        assert_eq!(story.paragraphs[0].runs[0].point_size, Some(10.0));
        assert_eq!(story.paragraphs[0].runs[1].text, "lo ");
        assert_eq!(story.paragraphs[0].runs[1].point_size, Some(20.0));
    }

    /// Right-only split: range starts at a run boundary, ends inside.
    #[test]
    fn character_set_property_splits_right_only_when_range_starts_at_run_boundary() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        // Range [6, 9): starts at "world"'s boundary, ends inside it.
        // Should split "world" into "wor" + "ld".
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 6,
                end: 9,
            },
            path: PropertyPath::CharacterFontSize,
            value: Value::Length(Some(30.0)),
        };
        project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs.len(), 3);
        assert_eq!(story.paragraphs[0].runs[2].text, "ld");
        assert_eq!(story.paragraphs[0].runs[1].text, "wor");
        assert_eq!(story.paragraphs[0].runs[1].point_size, Some(30.0));
        assert_eq!(story.paragraphs[0].runs[2].point_size, Some(10.0));
    }

    /// SDK Phase 3 — paragraph-space-before applies to every
    /// paragraph that intersects [start, end). Paragraphs are
    /// atomic; the apply layer rounds the range to whole
    /// paragraphs by treating intersection as the trigger.
    #[test]
    fn paragraph_space_before_applies_to_intersecting_paragraphs() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        // Range [3, 11): cuts inside paragraph 0 ("Hello world",
        // chars 0..11) but doesn't reach paragraph 1 ("!" at 11..12).
        // Should apply to paragraph 0 only.
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 3,
                end: 11,
            },
            path: PropertyPath::ParagraphSpaceBefore,
            value: Value::Length(Some(18.0)),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].space_before, Some(18.0));
        // Paragraph 1 unchanged (default `None`).
        assert_eq!(story.paragraphs[1].space_before, None);

        // Inverse restores the prior value (the fixture didn't set
        // space_before so it was None).
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].space_before, None);
    }

    /// Cross-paragraph paragraph-property write: a range that
    /// touches both paragraphs writes to both.
    #[test]
    fn paragraph_space_after_applies_across_paragraphs() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        // Range [5, 12): cuts inside paragraph 0 + covers paragraph 1.
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 5,
                end: 12,
            },
            path: PropertyPath::ParagraphSpaceAfter,
            value: Value::Length(Some(6.0)),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].space_after, Some(6.0));
        assert_eq!(story.paragraphs[1].space_after, Some(6.0));
        // Inverse is a Batch of two restorations.
        assert!(matches!(&applied.inverse, Operation::Batch { ops } if ops.len() == 2));
    }

    /// Apply-an-entity per D3: setting `appliedParagraphStyle` to
    /// a style id stores the ref on every intersecting paragraph;
    /// undo restores the prior value (None in the fixture, so the
    /// inverse stores an empty-string Text payload that clears).
    #[test]
    fn applied_paragraph_style_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::AppliedParagraphStyle,
            value: Value::Text("ParagraphStyle/$ID/Heading 1".to_string()),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(
            story.paragraphs[0].paragraph_style.as_deref(),
            Some("ParagraphStyle/$ID/Heading 1")
        );
        // Inverse restores to None (empty Text clears).
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].paragraph_style, None);
    }

    /// Apply-an-entity for character ranges. Uses the same run-
    /// splitting machinery as the scalar character paths.
    #[test]
    fn applied_character_style_splits_runs_and_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 2,
                end: 4,
            },
            path: PropertyPath::AppliedCharacterStyle,
            value: Value::Text("CharacterStyle/$ID/Strong".to_string()),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        // The run "Hello " was split into "He" + "ll" + "o ".
        // "ll" got the style; the rest stayed at None.
        assert_eq!(story.paragraphs[0].runs[1].text, "ll");
        assert_eq!(
            story.paragraphs[0].runs[1].character_style.as_deref(),
            Some("CharacterStyle/$ID/Strong")
        );
        assert_eq!(story.paragraphs[0].runs[0].character_style, None);
        assert_eq!(story.paragraphs[0].runs[2].character_style, None);
        // Inverse restores.
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        for run in &story.paragraphs[0].runs {
            assert_eq!(run.character_style, None);
        }
    }

    /// SDK Phase 5 (D3 completion) — applied object style on a
    /// TextFrame. Wire shape mirrors AppliedParagraphStyle (string-
    /// id payload, empty clears).
    #[test]
    fn applied_object_style_text_frame_round_trips() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let op = Operation::SetProperty {
            node: NodeId::TextFrame("TextFrame/u1".to_string()),
            path: PropertyPath::AppliedObjectStyle,
            value: Value::Text("ObjectStyle/$ID/Logo".to_string()),
        };
        let applied = project.apply(op).expect("apply");
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(
            frame.applied_object_style.as_deref(),
            Some("ObjectStyle/$ID/Logo")
        );
        // Inverse restores to None (the fresh fixture has no override).
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.applied_object_style, None);
    }

    /// SDK Phase 5 (D3 completion) — placeholder paths for
    /// AppliedCellStyle / AppliedTableStyle return UnsupportedProperty
    /// until the Table NodeId surface (Tier 2d) lands. Wire shape
    /// exists so panels can declare their write surface today.
    #[test]
    fn applied_cell_style_is_unsupported_until_tier_2d() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let op = Operation::SetProperty {
            node: NodeId::TextFrame("TextFrame/u1".to_string()),
            path: PropertyPath::AppliedCellStyle,
            value: Value::Text("CellStyle/$ID/Header".to_string()),
        };
        let err = project.apply(op).expect_err("expected UnsupportedProperty");
        assert!(matches!(
            err,
            crate::OperationError::UnsupportedProperty {
                path: PropertyPath::AppliedCellStyle,
                ..
            }
        ));
    }

    #[test]
    fn applied_table_style_is_unsupported_until_tier_2d() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let op = Operation::SetProperty {
            node: NodeId::TextFrame("TextFrame/u1".to_string()),
            path: PropertyPath::AppliedTableStyle,
            value: Value::Text("TableStyle/$ID/Grid".to_string()),
        };
        let err = project.apply(op).expect_err("expected UnsupportedProperty");
        assert!(matches!(
            err,
            crate::OperationError::UnsupportedProperty {
                path: PropertyPath::AppliedTableStyle,
                ..
            }
        ));
    }

    /// SDK Phase 5 (D3 completion) — AppliedConditions on a
    /// StoryRange. Wire encoding: a single Value::Text with a
    /// space-separated list of condition self_ids. Empty clears.
    /// The range [0, 6) covers exactly the first run ("Hello ");
    /// the second run ("world") stays empty.
    #[test]
    fn applied_conditions_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::AppliedConditions,
            value: Value::Text("Condition/Draft Condition/Internal".to_string()),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(
            story.paragraphs[0].runs[0].applied_conditions,
            vec![
                "Condition/Draft".to_string(),
                "Condition/Internal".to_string(),
            ]
        );
        // The "world" run was outside the range — stays empty.
        assert!(story.paragraphs[0].runs[1].applied_conditions.is_empty());
        // Inverse restores the first run to empty.
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        for run in &story.paragraphs[0].runs {
            assert!(run.applied_conditions.is_empty());
        }
    }

    /// SDK Phase 5 (v1 sweep) — end-to-end PathfinderBoolean.
    /// Insert two rectangles into a spread, run Subtract via the
    /// new Operation, and verify the kept frame's anchors now
    /// match the L-shape result (6 corner vertices) while the
    /// other frame is gone. One Cmd-Z restores both.
    #[test]
    fn pathfinder_subtract_round_trips_via_operation() {
        use idml_parse::Spread;
        let mut project = Project::new(Document {
            container: Container {
                mimetype: "application/vnd.adobe.indesign-idml-package".to_string(),
                designmap_raw: Bytes::new(),
                designmap: DesignMap::default(),
                entries: BTreeMap::new(),
            },
            palette: Graphic::default(),
            spreads: vec![ParsedSpread {
                src: "Spreads/syn.xml".to_string(),
                spread: {
                    let mut s = Spread::default();
                    s.self_id = Some("Spread/u_main".to_string());
                    s
                },
            }],
            stories: Vec::new(),
            master_spreads: HashMap::new(),
            frame_for_story: HashMap::new(),
            text_frame_index: HashMap::new(),
            styles: StyleSheet::default(),
            anchors: Vec::new(),
        });
        // Two rectangles: A = [0..20, 0..20], B = [10..30, 10..30].
        // A \ B is an L-shape of 6 corner vertices.
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: NodeSpec::Rectangle {
                    self_id: "Rectangle/a".to_string(),
                    bounds: [0.0, 0.0, 20.0, 20.0],
                    fill_color: None,
                },
            })
            .expect("insert a");
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 1,
                node: NodeSpec::Rectangle {
                    self_id: "Rectangle/b".to_string(),
                    bounds: [10.0, 10.0, 30.0, 30.0],
                    fill_color: None,
                },
            })
            .expect("insert b");

        let applied = project
            .apply(Operation::PathfinderBoolean {
                kept: NodeId::Rectangle("Rectangle/a".to_string()),
                others: vec![NodeId::Rectangle("Rectangle/b".to_string())],
                op_kind: crate::operation::PathfinderKind::Subtract,
            })
            .expect("pathfinder subtract");

        let rect = &project.document().spreads[0].spread.rectangles;
        // B is gone; A is left.
        assert_eq!(rect.len(), 1);
        assert_eq!(rect[0].self_id.as_deref(), Some("Rectangle/a"));
        // A's anchors are now the L-shape — 6 corners.
        assert_eq!(rect[0].anchors.len(), 6);

        // One Cmd-Z restores both frames + the original path.
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let rect = &project.document().spreads[0].spread.rectangles;
        assert_eq!(rect.len(), 2);
    }

    /// SDK Phase 5 (v1 sweep) — whole-path replacement. Pathfinder
    /// (Subtract / Exclude) uses this to drop in a freshly-computed
    /// polygon set in one shot. Inverse captures the prior anchors
    /// + subpath_starts so undo round-trips bytewise.
    #[test]
    fn frame_path_replacement_round_trips() {
        use idml_parse::PathAnchor;
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        // Seed three anchors so the prior state is observable.
        {
            let frame = &mut project.document_mut().spreads[0].spread.text_frames[0];
            frame.anchors = vec![
                PathAnchor {
                    anchor: (0.0, 0.0),
                    left: (0.0, 0.0),
                    right: (0.0, 0.0),
                },
                PathAnchor {
                    anchor: (10.0, 0.0),
                    left: (10.0, 0.0),
                    right: (10.0, 0.0),
                },
                PathAnchor {
                    anchor: (5.0, 8.0),
                    left: (5.0, 8.0),
                    right: (5.0, 8.0),
                },
            ];
            frame.subpath_starts = vec![0];
        }

        // Replace with a single corner.
        let new_anchor = crate::operation::PathAnchorSpec {
            anchor: [100.0, 100.0],
            left: [100.0, 100.0],
            right: [100.0, 100.0],
        };
        let applied = project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FramePath,
                value: Value::FramePath {
                    anchors: vec![new_anchor],
                    subpath_starts: vec![0],
                },
            })
            .expect("apply");
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.anchors.len(), 1);
        assert_eq!(frame.anchors[0].anchor, (100.0, 100.0));

        // Undo restores the original three anchors.
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.anchors.len(), 3);
        assert_eq!(frame.anchors[1].anchor, (10.0, 0.0));
    }

    /// SDK Phase 5 (v1 sweep) — drop-shadow toggle. true → default
    /// DropShadowSetting; false → None. Inverse stores the prior
    /// `is_some()` boolean.
    #[test]
    fn frame_drop_shadow_toggle_round_trips() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        // Initially None.
        assert!(project.document().spreads[0].spread.text_frames[0]
            .drop_shadow
            .is_none());

        let applied = project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameDropShadow,
                value: Value::Bool(true),
            })
            .expect("apply on");
        let shadow = project.document().spreads[0].spread.text_frames[0]
            .drop_shadow
            .as_ref()
            .expect("drop_shadow set");
        // The default carries the InDesign-preset values from
        // `default_drop_shadow`.
        assert_eq!(shadow.mode, "Drop");
        assert_eq!(shadow.x_offset, 3.0);

        // Undo → back to None.
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        assert!(project.document().spreads[0].spread.text_frames[0]
            .drop_shadow
            .is_none());
    }

    /// SDK Phase 5 (v1 sweep) — text-wrap mode + offsets apply +
    /// undo. The two paths share the same `Option<TextWrap>` field
    /// but write distinct halves; both preserve the other half on
    /// commit.
    #[test]
    fn frame_text_wrap_round_trips() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        // Set mode on a fresh fixture (text_wrap = None) → creates
        // a TextWrap with the picked mode and default offsets.
        let applied_mode = project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameTextWrapMode,
                value: Value::Text("BoundingBoxTextWrap".to_string()),
            })
            .expect("apply mode");
        let tw = project.document().spreads[0].spread.text_frames[0]
            .text_wrap
            .expect("text_wrap should now be Some");
        assert_eq!(tw.mode, idml_parse::TextWrapMode::BoundingBoxTextWrap);
        assert_eq!(tw.offsets, [0.0; 4]);

        // Set offsets — must preserve mode.
        project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameTextWrapOffsets,
                value: Value::Bounds([6.0, 6.0, 6.0, 6.0]),
            })
            .expect("apply offsets");
        let tw = project.document().spreads[0].spread.text_frames[0]
            .text_wrap
            .expect("text_wrap should still be Some");
        assert_eq!(tw.mode, idml_parse::TextWrapMode::BoundingBoxTextWrap);
        assert_eq!(tw.offsets, [6.0, 6.0, 6.0, 6.0]);

        // Undo the mode-set: the inverse carries the prior mode
        // string, which was empty (the fixture started with
        // text_wrap = None → empty prev_mode string). Empty string
        // clears the override, so text_wrap returns to `None` —
        // the offsets set in between are dropped. Bytewise
        // round-trip across both paths would need a compound
        // inverse; v1 collapses to single-field semantics.
        crate::apply(project.document_mut(), &applied_mode.inverse).expect("undo mode");
        assert!(
            project.document().spreads[0].spread.text_frames[0]
                .text_wrap
                .is_none(),
            "text_wrap should clear back to None after undo of mode-set"
        );
    }

    /// SDK Phase 5 (v1 sweep) — paragraph justification apply +
    /// undo. Wire shape: Value::Text carrying the IDML enum string.
    #[test]
    fn paragraph_justification_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphJustification,
            value: Value::Text("CenterAlign".to_string()),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(
            story.paragraphs[0].justification,
            Some(idml_parse::Justification::CenterAlign)
        );
        // Inverse restores None.
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].justification, None);
    }

    /// SDK Phase 5 (v1 sweep) — invalid Justification string raises
    /// InvalidValue rather than silently dropping the write.
    #[test]
    fn paragraph_justification_rejects_unknown_string() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphJustification,
            value: Value::Text("NotARealAlignment".to_string()),
        };
        let err = project.apply(op).expect_err("expected InvalidValue");
        assert!(matches!(
            err,
            crate::OperationError::InvalidValue {
                path: PropertyPath::ParagraphJustification,
                ..
            }
        ));
    }

    /// SDK Phase 5 (v1 sweep) — stroke end-cap on Rectangle. Uses
    /// the InsertNode path through the apply layer to build the
    /// rectangle (mirrors how existing fixture helpers exercise
    /// `apply_insert_node`), then commits the end-cap change.
    #[test]
    fn frame_stroke_end_cap_round_trips_on_rectangle() {
        let mut project = Project::new(Document {
            container: Container {
                mimetype: "application/vnd.adobe.indesign-idml-package".to_string(),
                designmap_raw: Bytes::new(),
                designmap: DesignMap::default(),
                entries: BTreeMap::new(),
            },
            palette: Graphic::default(),
            spreads: vec![ParsedSpread {
                src: "Spreads/syn.xml".to_string(),
                spread: {
                    let mut s = idml_parse::Spread::default();
                    s.self_id = Some("Spread/u_main".to_string());
                    s
                },
            }],
            stories: Vec::new(),
            master_spreads: HashMap::new(),
            frame_for_story: HashMap::new(),
            text_frame_index: HashMap::new(),
            styles: StyleSheet::default(),
            anchors: Vec::new(),
        });
        // Insert a fresh rectangle (the InsertNode arm fills in the
        // full struct including default `end_cap: None`).
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: NodeSpec::Rectangle {
                    self_id: "Rectangle/u1".to_string(),
                    bounds: [0.0, 0.0, 100.0, 100.0],
                    fill_color: None,
                },
            })
            .expect("insert");

        let op = Operation::SetProperty {
            node: NodeId::Rectangle("Rectangle/u1".to_string()),
            path: PropertyPath::FrameStrokeEndCap,
            value: Value::Text("RoundEndCap".to_string()),
        };
        let applied = project.apply(op).expect("apply");
        assert_eq!(
            project.document().spreads[0]
                .spread
                .rectangles[0]
                .end_cap
                .as_deref(),
            Some("RoundEndCap")
        );
        // Undo: fresh fixture had end_cap = None → empty Text inverse.
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        assert_eq!(
            project.document().spreads[0].spread.rectangles[0].end_cap,
            None
        );
    }

    /// SDK Phase 5 (v1 sweep) — TextFrame inset spacing apply +
    /// undo. Wire shape: Value::Bounds([top, left, bottom, right])
    /// in pt. The renderer's text-frame composer reads the field
    /// on the next rebuild.
    #[test]
    fn frame_inset_spacing_round_trips() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let op = Operation::SetProperty {
            node: NodeId::TextFrame("TextFrame/u1".to_string()),
            path: PropertyPath::FrameInsetSpacing,
            value: Value::Bounds([12.0, 4.0, 12.0, 4.0]),
        };
        let applied = project.apply(op).expect("apply");
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.inset_spacing, Some([12.0, 4.0, 12.0, 4.0]));
        // Undo: the prior `None` round-trips as `Some([0,0,0,0])`
        // (v1 collapses "default" + "explicit zero" — a typed null-
        // bounds wire variant would distinguish them).
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.inset_spacing, Some([0.0, 0.0, 0.0, 0.0]));
    }

    /// First-line-indent — exercises the third paragraph path.
    #[test]
    fn paragraph_first_line_indent_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphFirstLineIndent,
            value: Value::Length(Some(12.0)),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].first_line_indent, Some(12.0));
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].first_line_indent, None);
    }

    /// Cross-paragraph range: covers part of paragraph 0's last run
    /// PLUS all of paragraph 1's content. Verifies the per-paragraph
    /// walk correctly handles ranges that span paragraph boundaries.
    #[test]
    fn character_set_property_spans_paragraphs() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        // Range [9, 12): cuts inside "world" at offset 3 (from para
        // start 6), then covers "!" (para 1, offset 11..12).
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 9,
                end: 12,
            },
            path: PropertyPath::CharacterFontSize,
            value: Value::Length(Some(40.0)),
        };
        project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        // Paragraph 0: "Hello " + "wor" + "ld".
        assert_eq!(story.paragraphs[0].runs.len(), 3);
        assert_eq!(story.paragraphs[0].runs[2].text, "ld");
        assert_eq!(story.paragraphs[0].runs[2].point_size, Some(40.0));
        // Paragraph 1: "!" (whole-run mutation).
        assert_eq!(story.paragraphs[1].runs.len(), 1);
        assert_eq!(story.paragraphs[1].runs[0].text, "!");
        assert_eq!(story.paragraphs[1].runs[0].point_size, Some(40.0));
    }

    /// Missing story: a SetProperty against a `StoryRange` whose
    /// story_id doesn't exist returns `NodeNotFound`, the same shape
    /// every other addressed mutation uses for "addressee gone".
    #[test]
    fn character_set_property_against_missing_story_errs_node_not_found() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/missing".to_string(),
                start: 0,
                end: 10,
            },
            path: PropertyPath::CharacterFontSize,
            value: Value::Length(Some(12.0)),
        };
        let err = project.apply(op).expect_err("story missing");
        assert!(
            matches!(err, OperationError::NodeNotFound(_)),
            "expected NodeNotFound, got: {err:?}"
        );
    }

    /// CharacterFillColor end-to-end: switches the colour ref + undo
    /// restores. Covers the `Value::ColorRef` path arm.
    #[test]
    fn character_fill_color_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 6,
                end: 11,
            },
            path: PropertyPath::CharacterFillColor,
            value: Value::ColorRef(Some("Color/Red".to_string())),
        };
        let applied = project.apply(op).expect("apply must succeed");
        let story = &project.document().stories[0].story;
        assert_eq!(
            story.paragraphs[0].runs[1].fill_color.as_deref(),
            Some("Color/Red")
        );
        // Other runs untouched.
        assert_eq!(
            story.paragraphs[0].runs[0].fill_color.as_deref(),
            Some("Color/Black")
        );
        // Undo restores Color/Black.
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(
            story.paragraphs[0].runs[1].fill_color.as_deref(),
            Some("Color/Black")
        );
    }
}
