//! Operation-based mutation channel for the IDML scene graph.
//!
//! Stage 1 of the Verso scripting layer (`docs/verso/scripting-layer.md`):
//! a single typed, serializable, invertible [`Operation`] is the sole
//! committed mutation surface. The inspector, the future REPL, the
//! QuickJS-based scripting layer, the gesture commit path, undo/redo,
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
pub mod error;
pub mod history;
pub mod invert;
pub mod notify;
pub mod operation;

pub use apply::apply;
pub use error::OperationError;
pub use history::{History, DEFAULT_HISTORY_CAPACITY};
pub use notify::Notifier;
pub use operation::{
    AppliedOperation, InvalidationHint, NodeId, NodeSpec, Operation, PropertyPath, Value,
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
        Bounds, Container, DesignMap, Graphic, Spread, StyleSheet, TextFrame as ParsedTextFrame,
    };
    use idml_scene::ParsedSpread;

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
        ];

        for op in ops {
            let json = serde_json::to_string(&op).expect("serialize");
            let parsed: Operation = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, op, "round-trip failed for: {json}");
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
}
