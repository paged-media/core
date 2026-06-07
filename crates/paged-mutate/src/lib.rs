/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! Operation-based mutation channel for the IDML scene graph.
//!
//! Stage 1 of the Paged scripting layer (`docs/paged/scripting-layer.md`):
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

use paged_scene::Document;

pub mod apply;
pub mod bezier_conv;
pub mod error;
pub mod history;
pub mod invert;
pub mod kurbo_kernel;
pub mod notify;
pub mod operation;
pub mod path_math;
pub mod pathfinder;

pub use apply::apply;
pub use error::OperationError;
pub use history::{History, DEFAULT_HISTORY_CAPACITY};
pub use notify::Notifier;
pub use operation::{
    AppliedOperation, ColorGroupSpec, FieldKind, GradientSpec, GradientStopSpec, GroupSpec,
    GuideOrientationSpec, InvalidationHint, NodeId, NodeSpec, NumberingListSpec, Operation,
    PathPointAddress, PathPointRole, PathfinderKind, PropertyPath, StyleCollection, StyleScope,
    SwatchSpec, Value,
};
pub use path_math::fit_polyline_to_anchors;

/// Holds a [`Document`] plus the Operation surface, undo/redo
/// history, and change-notification fan-out around it.
///
/// `Project` is the single owner during an interactive session.
/// `paged-introspect` wraps one of these and exposes it to JS via
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
// 0.7071 literals are cos/sin 45° rotation fixtures; the explicit matrix
// reads clearer than FRAC_1_SQRT_2 here.
#[allow(clippy::approx_constant)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::{BTreeMap, HashMap};

    use crate::operation::PathAnchorSpec;
    use crate::path_math::smooth_handles_from_neighbours;
    use bytes::Bytes;
    use paged_parse::{
        Bounds, Container, DesignMap, FrameRef, Graphic, PathAnchor, Polygon, Spread, StyleSheet,
        TextFrame as ParsedTextFrame,
    };
    use paged_scene::{ParsedSpread, ParsedStory};

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
            stroke_gap_color: None,
            stroke_gap_tint: None,
            stroke_dash: Vec::new(),
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
            column_count: None,
            column_gutter: None,
            column_balance: None,
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
            nonprinting: false,
        }
    }

    fn document_with_one_textframe(self_id: &str) -> Document {
        let mut spread = Spread {
            self_id: Some("Spread/u_main".to_string()),
            ..Default::default()
        };
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
        let m = [0.7071, 0.7071, -0.7071, 0.7071, 50.0, 100.0];
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
                z_slot: None,
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
                z_slot: None,
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: NodeSpec::Rectangle {
                    item_transform: None,
                    stroke_color: None,
                    stroke_weight: None,
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

    // ---- Swatch collection CRUD ------------------------------------------

    fn cmyk_swatch(
        self_id: Option<&str>,
        name: &str,
        cmyk: [f32; 4],
    ) -> crate::operation::SwatchSpec {
        crate::operation::SwatchSpec {
            self_id: self_id.map(String::from),
            name: Some(name.to_string()),
            space: "CMYK".to_string(),
            value: cmyk.to_vec(),
            model: None,
            alternate_space: None,
            alternate_value: Vec::new(),
            tint: None,
            alpha: None,
        }
    }

    #[test]
    fn create_swatch_inserts_and_inverse_deletes() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let applied = project
            .apply(Operation::CreateSwatch {
                spec: cmyk_swatch(Some("Color/Sky"), "Sky", [80.0, 20.0, 0.0, 0.0]),
            })
            .expect("create");
        assert!(project.document().palette.colors.contains_key("Color/Sky"));
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        assert!(!project.document().palette.colors.contains_key("Color/Sky"));
    }

    #[test]
    fn create_swatch_assigns_id_when_none() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let applied = project
            .apply(Operation::CreateSwatch {
                spec: cmyk_swatch(None, "Auto", [0.0, 0.0, 0.0, 100.0]),
            })
            .expect("create");
        assert_eq!(project.document().palette.colors.len(), 1);
        let resolved = match &applied.op {
            Operation::CreateSwatch { spec } => spec.self_id.clone().expect("id assigned"),
            _ => panic!("expected CreateSwatch"),
        };
        assert!(resolved.starts_with("Color/u"));
        assert!(project.document().palette.colors.contains_key(&resolved));
    }

    #[test]
    fn edit_swatch_replaces_and_undo_restores() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(Operation::CreateSwatch {
                spec: cmyk_swatch(Some("Color/A"), "A", [10.0, 20.0, 30.0, 40.0]),
            })
            .unwrap();
        project
            .apply(Operation::EditSwatch {
                swatch_id: "Color/A".to_string(),
                spec: cmyk_swatch(Some("Color/A"), "A-renamed", [1.0, 2.0, 3.0, 4.0]),
            })
            .unwrap();
        {
            let e = &project.document().palette.colors["Color/A"];
            assert_eq!(e.name.as_deref(), Some("A-renamed"));
            assert_eq!(e.value, vec![1.0, 2.0, 3.0, 4.0]);
        }
        project.undo().unwrap().expect("undo edit");
        let e = &project.document().palette.colors["Color/A"];
        assert_eq!(e.name.as_deref(), Some("A"));
        assert_eq!(e.value, vec![10.0, 20.0, 30.0, 40.0]);
    }

    #[test]
    fn delete_swatch_undo_recreates_lossless_including_spot_fields() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        // Spot swatch with a CMYK alternate + tint — exercises lossless capture.
        let spot = crate::operation::SwatchSpec {
            self_id: Some("Color/Pantone".to_string()),
            name: Some("PANTONE 286".to_string()),
            space: "LAB".to_string(),
            value: vec![20.0, 10.0, -60.0],
            model: Some("Spot".to_string()),
            alternate_space: Some("CMYK".to_string()),
            alternate_value: vec![100.0, 75.0, 0.0, 0.0],
            tint: Some(80.0),
            alpha: None,
        };
        project
            .apply(Operation::CreateSwatch { spec: spot })
            .unwrap();
        project
            .apply(Operation::DeleteSwatch {
                swatch_id: "Color/Pantone".to_string(),
            })
            .unwrap();
        assert!(!project
            .document()
            .palette
            .colors
            .contains_key("Color/Pantone"));
        project.undo().unwrap().expect("undo delete");
        let e = &project.document().palette.colors["Color/Pantone"];
        assert_eq!(e.name.as_deref(), Some("PANTONE 286"));
        assert_eq!(e.space, paged_parse::ColorSpace::Lab);
        assert_eq!(e.model, paged_parse::ColorModel::Spot);
        assert_eq!(e.alternate_space, Some(paged_parse::ColorSpace::Cmyk));
        assert_eq!(e.alternate_value, vec![100.0, 75.0, 0.0, 0.0]);
        assert_eq!(e.tint, Some(80.0));
    }

    #[test]
    fn edit_and_delete_missing_swatch_error() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        assert!(project
            .apply(Operation::EditSwatch {
                swatch_id: "Color/Nope".to_string(),
                spec: cmyk_swatch(None, "x", [0.0, 0.0, 0.0, 0.0]),
            })
            .is_err());
        assert!(project
            .apply(Operation::DeleteSwatch {
                swatch_id: "Color/Nope".to_string(),
            })
            .is_err());
    }

    // ---- Style collection CRUD -------------------------------------------

    #[test]
    fn create_paragraph_style_inserts_and_inverse_deletes() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let applied = project
            .apply(Operation::CreateParagraphStyle {
                self_id: Some("ParagraphStyle/Heading".to_string()),
                name: Some("Heading".to_string()),
                based_on: Some("ParagraphStyle/$ID/[No paragraph style]".to_string()),
                restore_json: None,
            })
            .expect("create");
        {
            let s = &project.document().styles.paragraph_styles["ParagraphStyle/Heading"];
            assert_eq!(s.name.as_deref(), Some("Heading"));
            assert_eq!(
                s.based_on.as_deref(),
                Some("ParagraphStyle/$ID/[No paragraph style]")
            );
        }
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        assert!(!project
            .document()
            .styles
            .paragraph_styles
            .contains_key("ParagraphStyle/Heading"));
    }

    #[test]
    fn create_paragraph_style_assigns_id_when_none() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let applied = project
            .apply(Operation::CreateParagraphStyle {
                self_id: None,
                name: Some("Auto".to_string()),
                based_on: None,
                restore_json: None,
            })
            .expect("create");
        let id = match &applied.op {
            Operation::CreateParagraphStyle { self_id, .. } => {
                self_id.clone().expect("id assigned")
            }
            _ => panic!("expected CreateParagraphStyle"),
        };
        assert!(id.starts_with("ParagraphStyle/u"));
        assert!(project.document().styles.paragraph_styles.contains_key(&id));
    }

    #[test]
    fn rename_paragraph_style_and_undo_restores_prior_name() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(Operation::CreateParagraphStyle {
                self_id: Some("ParagraphStyle/A".to_string()),
                name: Some("Body".to_string()),
                based_on: None,
                restore_json: None,
            })
            .unwrap();
        project
            .apply(Operation::RenameParagraphStyle {
                style_id: "ParagraphStyle/A".to_string(),
                name: "Body Copy".to_string(),
            })
            .unwrap();
        assert_eq!(
            project.document().styles.paragraph_styles["ParagraphStyle/A"]
                .name
                .as_deref(),
            Some("Body Copy")
        );
        project.undo().unwrap().expect("undo rename");
        assert_eq!(
            project.document().styles.paragraph_styles["ParagraphStyle/A"]
                .name
                .as_deref(),
            Some("Body")
        );
    }

    #[test]
    fn delete_paragraph_style_undo_is_lossless_for_rich_def() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(Operation::CreateParagraphStyle {
                self_id: Some("ParagraphStyle/Rich".to_string()),
                name: Some("Rich".to_string()),
                based_on: None,
                restore_json: None,
            })
            .unwrap();
        // Simulate a richly-populated style (as a parsed document would
        // carry) by setting non-default fields directly, then prove a
        // delete→undo restores them — not just name/based_on.
        {
            let s = project
                .document_mut()
                .styles
                .paragraph_styles
                .get_mut("ParagraphStyle/Rich")
                .unwrap();
            s.point_size = Some(42.0);
            s.space_before = Some(12.0);
            s.justification = Some(paged_parse::story::Justification::CenterAlign);
        }
        project
            .apply(Operation::DeleteParagraphStyle {
                style_id: "ParagraphStyle/Rich".to_string(),
            })
            .unwrap();
        assert!(!project
            .document()
            .styles
            .paragraph_styles
            .contains_key("ParagraphStyle/Rich"));
        project.undo().unwrap().expect("undo delete");
        let s = &project.document().styles.paragraph_styles["ParagraphStyle/Rich"];
        assert_eq!(s.point_size, Some(42.0));
        assert_eq!(s.space_before, Some(12.0));
        assert_eq!(
            s.justification,
            Some(paged_parse::story::Justification::CenterAlign)
        );
    }

    #[test]
    fn character_style_create_and_delete_round_trip() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(Operation::CreateCharacterStyle {
                self_id: Some("CharacterStyle/Emph".to_string()),
                name: Some("Emphasis".to_string()),
                based_on: None,
                restore_json: None,
            })
            .unwrap();
        assert!(project
            .document()
            .styles
            .character_styles
            .contains_key("CharacterStyle/Emph"));
        project
            .apply(Operation::DeleteCharacterStyle {
                style_id: "CharacterStyle/Emph".to_string(),
            })
            .unwrap();
        assert!(!project
            .document()
            .styles
            .character_styles
            .contains_key("CharacterStyle/Emph"));
        project.undo().unwrap().expect("undo delete");
        assert!(project
            .document()
            .styles
            .character_styles
            .contains_key("CharacterStyle/Emph"));
    }

    #[test]
    fn rename_and_delete_missing_style_error() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        assert!(project
            .apply(Operation::RenameParagraphStyle {
                style_id: "ParagraphStyle/Nope".to_string(),
                name: "x".to_string(),
            })
            .is_err());
        assert!(project
            .apply(Operation::DeleteCharacterStyle {
                style_id: "CharacterStyle/Nope".to_string(),
            })
            .is_err());
    }

    #[test]
    fn object_cell_table_style_crud_round_trips() {
        // The object/cell/table styles share the style_crud! macro with
        // paragraph/character (already exhaustively tested); these
        // confirm the dispatch arm + target map are wired per kind.
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));

        project
            .apply(Operation::CreateObjectStyle {
                self_id: Some("ObjectStyle/Card".to_string()),
                name: Some("Card".to_string()),
                based_on: None,
                restore_json: None,
            })
            .unwrap();
        project
            .apply(Operation::CreateCellStyle {
                self_id: Some("CellStyle/Head".to_string()),
                name: Some("Head".to_string()),
                based_on: None,
                restore_json: None,
            })
            .unwrap();
        project
            .apply(Operation::CreateTableStyle {
                self_id: Some("TableStyle/Grid".to_string()),
                name: Some("Grid".to_string()),
                based_on: None,
                restore_json: None,
            })
            .unwrap();
        assert!(project
            .document()
            .styles
            .object_styles
            .contains_key("ObjectStyle/Card"));
        assert!(project
            .document()
            .styles
            .cell_styles
            .contains_key("CellStyle/Head"));
        assert!(project
            .document()
            .styles
            .table_styles
            .contains_key("TableStyle/Grid"));

        // Delete the table style then undo — lands back in the right map.
        project
            .apply(Operation::DeleteTableStyle {
                style_id: "TableStyle/Grid".to_string(),
            })
            .unwrap();
        assert!(!project
            .document()
            .styles
            .table_styles
            .contains_key("TableStyle/Grid"));
        project.undo().unwrap().expect("undo");
        assert!(project
            .document()
            .styles
            .table_styles
            .contains_key("TableStyle/Grid"));
    }

    #[test]
    fn gradient_crud_round_trips() {
        use crate::operation::{GradientSpec, GradientStopSpec};
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let spec = GradientSpec {
            self_id: Some("Gradient/Sunset".to_string()),
            name: Some("Sunset".to_string()),
            kind: "Linear".to_string(),
            stops: vec![
                GradientStopSpec {
                    stop_color: "Color/Red".into(),
                    location_pct: 0.0,
                    midpoint_pct: Some(40.0),
                },
                GradientStopSpec {
                    stop_color: "Color/Yellow".into(),
                    location_pct: 100.0,
                    midpoint_pct: None,
                },
            ],
        };
        project.apply(Operation::CreateGradient { spec }).unwrap();
        assert_eq!(
            project.document().palette.gradients["Gradient/Sunset"]
                .stops
                .len(),
            2
        );
        // Edit (reverse to radial, drop a stop) then undo restores.
        project
            .apply(Operation::EditGradient {
                gradient_id: "Gradient/Sunset".to_string(),
                spec: GradientSpec {
                    self_id: None,
                    name: Some("Sunset".to_string()),
                    kind: "Radial".to_string(),
                    stops: vec![GradientStopSpec {
                        stop_color: "Color/Blue".into(),
                        location_pct: 0.0,
                        midpoint_pct: None,
                    }],
                },
            })
            .unwrap();
        assert_eq!(
            project.document().palette.gradients["Gradient/Sunset"].kind,
            paged_parse::GradientKind::Radial
        );
        project.undo().unwrap().expect("undo edit");
        let g = &project.document().palette.gradients["Gradient/Sunset"];
        assert_eq!(g.kind, paged_parse::GradientKind::Linear);
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].midpoint_pct, Some(40.0));
        // Delete then undo recreates.
        project
            .apply(Operation::DeleteGradient {
                gradient_id: "Gradient/Sunset".to_string(),
            })
            .unwrap();
        assert!(!project
            .document()
            .palette
            .gradients
            .contains_key("Gradient/Sunset"));
        project.undo().unwrap().expect("undo delete");
        assert!(project
            .document()
            .palette
            .gradients
            .contains_key("Gradient/Sunset"));
    }

    #[test]
    fn color_group_crud_round_trips() {
        use crate::operation::ColorGroupSpec;
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(Operation::CreateColorGroup {
                spec: ColorGroupSpec {
                    self_id: Some("ColorGroup/Brand".to_string()),
                    name: Some("Brand".to_string()),
                    members: vec!["Color/Red".into(), "Color/Blue".into()],
                },
            })
            .unwrap();
        assert_eq!(
            project.document().palette.color_groups["ColorGroup/Brand"]
                .members
                .len(),
            2
        );
        project
            .apply(Operation::DeleteColorGroup {
                group_id: "ColorGroup/Brand".to_string(),
            })
            .unwrap();
        assert!(!project
            .document()
            .palette
            .color_groups
            .contains_key("ColorGroup/Brand"));
        project.undo().unwrap().expect("undo");
        assert_eq!(
            project.document().palette.color_groups["ColorGroup/Brand"].members,
            vec!["Color/Red".to_string(), "Color/Blue".to_string()]
        );
    }

    // ── W1.22 (engine gap 22) — numbering-list CRUD + next-style ──────

    #[test]
    fn numbering_list_crud_round_trips() {
        use crate::operation::NumberingListSpec;
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        // Create with an explicit id + continue-across-stories on.
        project
            .apply(Operation::CreateNumberingList {
                spec: NumberingListSpec {
                    self_id: Some("NumberingList/Steps".to_string()),
                    name: Some("Steps".to_string()),
                    continue_across_stories: Some(true),
                    continue_across_documents: None,
                },
            })
            .unwrap();
        assert_eq!(
            project.document().styles.numbering_lists["NumberingList/Steps"]
                .continue_across_stories,
            Some(true)
        );
        // Edit: flip the flag off + rename.
        project
            .apply(Operation::EditNumberingList {
                list_id: "NumberingList/Steps".to_string(),
                spec: NumberingListSpec {
                    self_id: Some("NumberingList/Steps".to_string()),
                    name: Some("Steps (local)".to_string()),
                    continue_across_stories: Some(false),
                    continue_across_documents: None,
                },
            })
            .unwrap();
        assert_eq!(
            project.document().styles.numbering_lists["NumberingList/Steps"]
                .continue_across_stories,
            Some(false)
        );
        // Undo the edit → flag back on, name back.
        project.undo().unwrap().expect("undo edit");
        let after = &project.document().styles.numbering_lists["NumberingList/Steps"];
        assert_eq!(after.continue_across_stories, Some(true));
        assert_eq!(after.name.as_deref(), Some("Steps"));
        // Delete, then undo restores the full def.
        project
            .apply(Operation::DeleteNumberingList {
                list_id: "NumberingList/Steps".to_string(),
            })
            .unwrap();
        assert!(!project
            .document()
            .styles
            .numbering_lists
            .contains_key("NumberingList/Steps"));
        project.undo().unwrap().expect("undo delete");
        assert_eq!(
            project.document().styles.numbering_lists["NumberingList/Steps"]
                .continue_across_stories,
            Some(true)
        );
    }

    #[test]
    fn set_style_property_next_style_round_trips() {
        use crate::operation::StyleCollection;
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(Operation::CreateParagraphStyle {
                self_id: Some("ParagraphStyle/Head".to_string()),
                name: Some("Head".to_string()),
                based_on: None,
                restore_json: None,
            })
            .unwrap();
        // Set NextStyle.
        project
            .apply(Operation::SetStyleProperty {
                collection: StyleCollection::Paragraph,
                style_id: "ParagraphStyle/Head".to_string(),
                path: PropertyPath::ParagraphStyleNextStyle,
                value: Value::Text("ParagraphStyle/Body".to_string()),
            })
            .unwrap();
        assert_eq!(
            project.document().styles.paragraph_styles["ParagraphStyle/Head"]
                .next_style
                .as_deref(),
            Some("ParagraphStyle/Body")
        );
        // Empty string clears it.
        project
            .apply(Operation::SetStyleProperty {
                collection: StyleCollection::Paragraph,
                style_id: "ParagraphStyle/Head".to_string(),
                path: PropertyPath::ParagraphStyleNextStyle,
                value: Value::Text(String::new()),
            })
            .unwrap();
        assert_eq!(
            project.document().styles.paragraph_styles["ParagraphStyle/Head"].next_style,
            None
        );
        // Undo the clear → back to Body.
        project.undo().unwrap().expect("undo clear");
        assert_eq!(
            project.document().styles.paragraph_styles["ParagraphStyle/Head"]
                .next_style
                .as_deref(),
            Some("ParagraphStyle/Body")
        );
    }

    #[test]
    fn paragraph_applied_numbering_list_round_trips_on_story_range() {
        // ParagraphAppliedNumberingList writes the per-paragraph
        // override; the range rounds to whole paragraphs. Undo restores
        // the prior (absent) value.
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let applied = project
            .apply(Operation::SetProperty {
                node: NodeId::StoryRange {
                    story_id: "Story/u1".to_string(),
                    start: 0,
                    end: 6,
                },
                path: PropertyPath::ParagraphAppliedNumberingList,
                value: Value::Text("NumberingList/Steps".to_string()),
            })
            .expect("apply must succeed");
        assert_eq!(
            project.document().stories[0].story.paragraphs[0]
                .applied_numbering_list
                .as_deref(),
            Some("NumberingList/Steps")
        );
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        assert_eq!(
            project.document().stories[0].story.paragraphs[0].applied_numbering_list,
            None
        );
    }

    #[test]
    fn set_style_property_edits_paragraph_def_and_inverts() {
        use crate::operation::StyleCollection;
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(Operation::CreateParagraphStyle {
                self_id: Some("ParagraphStyle/H1".to_string()),
                name: Some("H1".to_string()),
                based_on: None,
                restore_json: None,
            })
            .unwrap();
        // Set point size on the style.
        project
            .apply(Operation::SetStyleProperty {
                collection: StyleCollection::Paragraph,
                style_id: "ParagraphStyle/H1".to_string(),
                path: PropertyPath::CharacterFontSize,
                value: Value::Length(Some(36.0)),
            })
            .unwrap();
        assert_eq!(
            project.document().styles.paragraph_styles["ParagraphStyle/H1"].point_size,
            Some(36.0)
        );
        // Set justification (Value::Text round-trips through Justification).
        project
            .apply(Operation::SetStyleProperty {
                collection: StyleCollection::Paragraph,
                style_id: "ParagraphStyle/H1".to_string(),
                path: PropertyPath::ParagraphJustification,
                value: Value::Text("CenterAlign".to_string()),
            })
            .unwrap();
        assert_eq!(
            project.document().styles.paragraph_styles["ParagraphStyle/H1"].justification,
            Some(paged_parse::story::Justification::CenterAlign)
        );
        // Undo justification, then point size — both revert.
        project.undo().unwrap().expect("undo justification");
        assert_eq!(
            project.document().styles.paragraph_styles["ParagraphStyle/H1"].justification,
            None
        );
        project.undo().unwrap().expect("undo size");
        assert_eq!(
            project.document().styles.paragraph_styles["ParagraphStyle/H1"].point_size,
            None
        );
    }

    #[test]
    fn set_style_property_paragraph_only_path_on_character_style_errors() {
        use crate::operation::StyleCollection;
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        project
            .apply(Operation::CreateCharacterStyle {
                self_id: Some("CharacterStyle/C".to_string()),
                name: Some("C".to_string()),
                based_on: None,
                restore_json: None,
            })
            .unwrap();
        // Character defs support CharacterFontSize…
        assert!(project
            .apply(Operation::SetStyleProperty {
                collection: StyleCollection::Character,
                style_id: "CharacterStyle/C".to_string(),
                path: PropertyPath::CharacterFontSize,
                value: Value::Length(Some(11.0)),
            })
            .is_ok());
        // …but not a paragraph-only path.
        assert!(project
            .apply(Operation::SetStyleProperty {
                collection: StyleCollection::Character,
                style_id: "CharacterStyle/C".to_string(),
                path: PropertyPath::ParagraphSpaceBefore,
                value: Value::Length(Some(6.0)),
            })
            .is_err());
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
            project
                .notifier_mut()
                .subscribe(move |_| c.set(c.get() + 1));
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
            z_slot: None,
            parent: NodeId::Spread("Spread/u_main".to_string()),
            position: 1,
            node: NodeSpec::TextFrame {
                item_transform: None,
                stroke_color: None,
                stroke_weight: None,
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
            z_slot: None,
            parent: NodeId::Spread("Spread/u_main".to_string()),
            position: 0,
            node: NodeSpec::TextFrame {
                item_transform: None,
                stroke_color: None,
                stroke_weight: None,
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
                z_slot: None,
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 1,
                node: NodeSpec::TextFrame {
                    item_transform: None,
                    stroke_color: None,
                    stroke_weight: None,
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
                z_slot: None,
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 1,
                node: NodeSpec::TextFrame {
                    item_transform: None,
                    stroke_color: None,
                    stroke_weight: None,
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
            // W0.5 — Oval NodeSpec + every new operation variant.
            Operation::InsertNode {
                z_slot: None,
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: NodeSpec::Oval {
                    self_id: "Oval/u_new".to_string(),
                    bounds: [1.0, 2.0, 3.0, 4.0],
                    fill_color: Some("Color/Green".to_string()),
                    stroke_color: None,
                    stroke_weight: Some(2.0),
                    item_transform: None,
                },
            },
            Operation::LinkFrames {
                from: "TextFrame/u1".to_string(),
                to: "TextFrame/u2".to_string(),
            },
            Operation::UnlinkFrames {
                frame: "TextFrame/u1".to_string(),
                prev_next: Some("TextFrame/u9".to_string()),
            },
            Operation::ApplyStyle {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 5,
                style: "ParagraphStyle/Body".to_string(),
                scope: crate::operation::StyleScope::Paragraph,
            },
            Operation::InsertField {
                story_id: "Story/u1".to_string(),
                offset: 3,
                field: crate::operation::FieldKind::PageNumber,
            },
            Operation::DeleteField {
                story_id: "Story/u1".to_string(),
                offset: 3,
                field: crate::operation::FieldKind::PageNumber,
            },
            Operation::InsertGuide {
                spread_id: "Spread/u_main".to_string(),
                orientation: crate::operation::GuideOrientationSpec::Vertical,
                position: 100.0,
                page_index: 0,
                guide_id: None,
            },
            Operation::MoveGuide {
                guide_id: "Guide/Spread/u_main/0".to_string(),
                position: 120.0,
            },
            Operation::DeleteGuide {
                guide_id: "Guide/Spread/u_main/0".to_string(),
            },
            Operation::SetConditionVisible {
                condition: "Condition/A".to_string(),
                visible: false,
            },
            Operation::ActivateConditionSet {
                set: "ConditionSet/Print".to_string(),
            },
            Operation::RestoreConditionVisibility {
                states: vec![("Condition/A".to_string(), true)],
            },
            Operation::ApplyMasterToPage {
                page: "Page/u1".to_string(),
                master: Some("MasterSpread/uA".to_string()),
            },
            Operation::DuplicatePage {
                page: "Page/u1".to_string(),
                clone_spread_json: None,
            },
            Operation::InsertSection {
                at_page: "Page/u1".to_string(),
                prefix: Some("A-".to_string()),
                numbering_style: Some("UpperRoman".to_string()),
                start_at: Some(1),
                self_id: None,
            },
            Operation::EditSection {
                section_id: "Section/u0".to_string(),
                prefix: Some(None),
                numbering_style: Some("Arabic".to_string()),
                start_at: Some(Some(5)),
            },
            Operation::DeleteSection {
                section_id: "Section/u0".to_string(),
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
        let mut src_spread = Spread {
            self_id: Some("Spread/u_src".to_string()),
            ..Default::default()
        };
        src_spread.item_transform =
            Some([1.0, 0.0, 0.0, 1.0, src_spread_origin.0, src_spread_origin.1]);
        src_spread
            .text_frames
            .push(empty_text_frame(src_id, src_bounds));

        let mut dest_spread = Spread {
            self_id: Some("Spread/u_dest".to_string()),
            ..Default::default()
        };
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
                z_slot: None,
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
                z_slot: None,
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
        assert_eq!(
            src_spread.text_frames[0].self_id.as_deref(),
            Some("TextFrame/u1")
        );
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
                z_slot: None,
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
    /// per `paged-parse/spread.rs:141-144`. The Group's own
    /// `item_transform` is what L.1 mutates.
    fn document_with_group(group_xform: Option<[f32; 6]>) -> Document {
        let mut spread = Spread {
            self_id: Some("Spread/u_main".to_string()),
            ..Default::default()
        };
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
        spread.groups.push(paged_parse::Group {
            self_id: Some("Group/g1".to_string()),
            members: vec![
                paged_parse::FrameRef::TextFrame(0),
                paged_parse::FrameRef::TextFrame(1),
            ],
            transparency: paged_parse::GroupTransparency::default(),
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
        assert_eq!(applied.inverse, group_xform_op("Group/g1", None),);
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
        let applied = project.apply(group_xform_op("Group/g1", Some(m1))).unwrap();
        crate::apply(project.document_mut(), &applied.inverse).unwrap();
        let group = &project.document().spreads[0].spread.groups[0];
        assert_eq!(group.item_transform, Some(m0));
    }

    #[test]
    fn group_transform_apply_to_missing_id_fails() {
        let mut project = Project::new(document_with_group(None));
        let err = project
            .apply(group_xform_op(
                "Group/missing",
                Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
            ))
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
            stroke_alignment: None,
            stroke_gap_color: None,
            stroke_gap_tint: None,
            stroke_dash: Vec::new(),
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
            nonprinting: false,
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
        let mut spread = Spread {
            self_id: Some("Spread/u_main".to_string()),
            ..Default::default()
        };
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

    fn polygon_of(project: &Project) -> &Polygon {
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
        assert_eq!(
            anchor_positions(polygon_of(&project)),
            vec![(0.0, 0.0), (10.0, 0.0)]
        );
        // Inverse re-inserts the captured anchor at the same index
        // and restores subpath_starts verbatim.
        match &applied.inverse {
            Operation::SetProperty {
                path: PropertyPath::PathPointInsert,
                value:
                    Value::PathPointInsert {
                        index,
                        anchor,
                        prev_subpath_starts,
                    },
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
            insert_op("Polygon/p1", 4, mid_anchor), // inside subpath 1
            curve_op("Polygon/p1", 1, true),        // smooth-derive interior of subpath 0
            remove_op("Polygon/p1", 2), // collapses nothing (subpath 0 still has 2 anchors)
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
        let spread = Spread {
            self_id: Some("Spread/u_main".to_string()),
            ..Default::default()
        };
        let mut designmap = DesignMap::default();
        designmap.layers.push(paged_parse::Layer {
            self_id: self_id.to_string(),
            name: Some("Body".to_string()),
            visible: true,
            locked: false,
            printable: true,
            parent_id: None,
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

    fn layer_of(project: &Project) -> &paged_parse::Layer {
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
                .push(paged_parse::Layer {
                    self_id: "ub".to_string(),
                    name: Some("Guides".to_string()),
                    visible: true,
                    locked: false,
                    printable: true,
                    parent_id: None,
                });
            p
        };
        // Move "ub" to index 0 (becomes the backmost layer — cycle-8
        // convention: designmap[0] paints first / sits furthest back).
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

    fn textframe_of(project: &Project) -> &ParsedTextFrame {
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
        use paged_parse::{CharacterRun, Paragraph, Story};

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
        use paged_parse::Spread;
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
                    Spread {
                        self_id: Some("Spread/u_main".to_string()),
                        ..Default::default()
                    }
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
                z_slot: None,
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: NodeSpec::Rectangle {
                    item_transform: None,
                    stroke_color: None,
                    stroke_weight: None,
                    self_id: "Rectangle/a".to_string(),
                    bounds: [0.0, 0.0, 20.0, 20.0],
                    fill_color: None,
                },
            })
            .expect("insert a");
        project
            .apply(Operation::InsertNode {
                z_slot: None,
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 1,
                node: NodeSpec::Rectangle {
                    item_transform: None,
                    stroke_color: None,
                    stroke_weight: None,
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
        use paged_parse::PathAnchor;
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

    /// SDK Phase 5 (v1 sweep) — drop-shadow per-field editors.
    /// Each field write materialises a default DropShadowSetting
    /// on the prior-None state, then sets the named field; undo
    /// restores the prior value.
    #[test]
    fn drop_shadow_per_field_round_trips() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        // Set X offset — materialises a default shadow.
        let applied = project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameDropShadowXOffset,
                value: Value::Length(Some(7.5)),
            })
            .expect("apply x_offset");
        let f = &project.document().spreads[0].spread.text_frames[0];
        let ds = f.drop_shadow.as_ref().expect("default shadow");
        assert_eq!(ds.x_offset, 7.5);
        assert_eq!(ds.mode, "Drop"); // default

        // Set opacity.
        project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameDropShadowOpacity,
                value: Value::Length(Some(60.0)),
            })
            .expect("apply opacity");
        let ds = project.document().spreads[0].spread.text_frames[0]
            .drop_shadow
            .as_ref()
            .expect("shadow");
        assert_eq!(ds.opacity_pct, 60.0);
        assert_eq!(ds.x_offset, 7.5); // preserved

        // Undo the x_offset — restores prior value (the default
        // 3.0 from when the shadow was materialised on the first
        // write).
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let ds = project.document().spreads[0].spread.text_frames[0]
            .drop_shadow
            .as_ref()
            .expect("shadow stays after partial undo");
        // The inverse stored prev=3.0 (the default at apply
        // time). Restoring brings x_offset back to 3.0.
        assert_eq!(ds.x_offset, 3.0);
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

    /// Editor-ops — gradient feather whole-struct authoring. Set on
    /// a frame with no effects (materialises `FrameEffects`), undo
    /// clears back to None; clear-then-undo restores the prior spec
    /// bytewise.
    #[test]
    fn gradient_feather_set_clear_round_trips() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        assert!(project.document().spreads[0].spread.text_frames[0]
            .effects
            .is_none());

        let spec = crate::operation::GradientFeatherSpec {
            gradient_type: Some("Linear".to_string()),
            start_point: Some([0.0, 0.0]),
            end_point: Some([100.0, 0.0]),
            angle_deg: Some(45.0),
            stops: vec![
                crate::operation::GradientFeatherStopSpec {
                    stop_color: None,
                    location_pct: 0.0,
                    alpha_pct: 100.0,
                    midpoint_pct: 50.0,
                },
                crate::operation::GradientFeatherStopSpec {
                    stop_color: None,
                    location_pct: 100.0,
                    alpha_pct: 0.0,
                    midpoint_pct: 50.0,
                },
            ],
        };
        let applied = project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameGradientFeather,
                value: Value::GradientFeather(Some(spec.clone())),
            })
            .expect("set feather");
        // Effects materialised from None; feather landed.
        let gf = project.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .expect("effects materialised")
            .gradient_feather
            .as_ref()
            .expect("feather set");
        assert_eq!(gf.angle_deg, Some(45.0));
        assert_eq!(gf.stops.len(), 2);
        assert_eq!(gf.stops[1].alpha_pct, 0.0);

        // Inverse captured prior None → undo clears the feather.
        assert!(matches!(
            &applied.inverse,
            Operation::SetProperty {
                value: Value::GradientFeather(None),
                ..
            }
        ));
        crate::apply(project.document_mut(), &applied.inverse).expect("undo set");
        assert!(project.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .is_none_or(|e| e.gradient_feather.is_none()));

        // Re-set, then clear via `GradientFeather(None)`; the clear's
        // inverse restores the full spec bytewise.
        project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameGradientFeather,
                value: Value::GradientFeather(Some(spec.clone())),
            })
            .expect("re-set feather");
        let cleared = project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameGradientFeather,
                value: Value::GradientFeather(None),
            })
            .expect("clear feather");
        assert!(project.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .expect("effects struct survives the clear")
            .gradient_feather
            .is_none());
        crate::apply(project.document_mut(), &cleared.inverse).expect("undo clear");
        let restored = project.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .expect("effects")
            .gradient_feather
            .as_ref()
            .expect("feather restored");
        assert_eq!(
            crate::operation::GradientFeatherSpec::from_parse(restored),
            spec,
            "clear-undo restores the prior spec bytewise"
        );
    }

    /// Editor-ops — gradient feather is fill-based; GraphicLine has
    /// no fill, so the property is rejected rather than silently
    /// stored.
    #[test]
    fn gradient_feather_rejected_on_graphic_line() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        // Insert a line to target.
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                z_slot: None,
                node: crate::operation::NodeSpec::GraphicLine {
                    item_transform: None,
                    self_id: "GraphicLine/u9".to_string(),
                    bounds: [0.0, 0.0, 100.0, 100.0],
                    anchors: vec![
                        crate::operation::PathAnchorSpec {
                            anchor: [0.0, 0.0],
                            left: [0.0, 0.0],
                            right: [0.0, 0.0],
                        },
                        crate::operation::PathAnchorSpec {
                            anchor: [100.0, 100.0],
                            left: [100.0, 100.0],
                            right: [100.0, 100.0],
                        },
                    ],
                    subpath_starts: vec![],
                    subpath_open: vec![],
                    stroke_color: Some("Color/Black".to_string()),
                    stroke_weight: Some(1.0),
                },
            })
            .expect("insert line");
        let err = project
            .apply(Operation::SetProperty {
                node: NodeId::GraphicLine("GraphicLine/u9".to_string()),
                path: PropertyPath::FrameGradientFeather,
                value: Value::GradientFeather(None),
            })
            .expect_err("feather on a line");
        assert!(matches!(err, OperationError::UnsupportedProperty { .. }));
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
        assert_eq!(tw.mode, paged_parse::TextWrapMode::BoundingBoxTextWrap);
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
        assert_eq!(tw.mode, paged_parse::TextWrapMode::BoundingBoxTextWrap);
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
            Some(paged_parse::Justification::CenterAlign)
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
                    paged_parse::Spread {
                        self_id: Some("Spread/u_main".to_string()),
                        ..Default::default()
                    }
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
                z_slot: None,
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: NodeSpec::Rectangle {
                    item_transform: None,
                    stroke_color: None,
                    stroke_weight: None,
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
            project.document().spreads[0].spread.rectangles[0]
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

    // ---- W0.1: character formatting paths ---------------------------------
    //
    // Each test proves the recipe acceptance: apply changes the run,
    // undo restores byte-equal, and (where exercised) run-splitting at
    // range boundaries works. The bool-field tests seed an explicit
    // prior so the `Value::Bool` inverse round-trips bytewise (a
    // prior-`None` would undo to `Some(false)` by design — see the
    // path doc-comments).

    /// `characterFontFamily` over the first run [0, 6): sets
    /// `AppliedFont`, undo restores the prior `None`.
    #[test]
    fn character_font_family_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::CharacterFontFamily,
            value: Value::Text("Minion Pro".to_string()),
        };
        let applied = project.apply(op).expect("apply must succeed");
        let story = &project.document().stories[0].story;
        assert_eq!(
            story.paragraphs[0].runs[0].font.as_deref(),
            Some("Minion Pro")
        );
        // Neighbour run untouched.
        assert_eq!(story.paragraphs[0].runs[1].font, None);

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[0].font, None);
    }

    /// Register a placeholder host text frame for `story_id` so the
    /// character-property apply emits a non-default InvalidationHint
    /// (the hint is keyed off `frame_for_story`). Returns nothing —
    /// mutates the doc in place.
    fn register_host_frame(project: &mut Project, story_id: &str, frame_self_id: &str) {
        let frame = crate::apply::new_text_frame(
            frame_self_id.to_string(),
            paged_parse::Bounds::ZERO,
            None,
        );
        project
            .document_mut()
            .frame_for_story
            .insert(story_id.to_string(), frame);
    }

    /// `characterCase` over the second run [6, 11): sets
    /// `Capitalization`, undo restores the prior `None`. Reflow path.
    #[test]
    fn character_case_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        register_host_frame(&mut project, "Story/u1", "TextFrame/f1");
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 6,
                end: 11,
            },
            path: PropertyPath::CharacterCase,
            value: Value::Text("AllCaps".to_string()),
        };
        let applied = project.apply(op).expect("apply must succeed");
        let story = &project.document().stories[0].story;
        assert_eq!(
            story.paragraphs[0].runs[1].capitalization.as_deref(),
            Some("AllCaps")
        );
        // Reflow-affecting → text_reflow hint, not frame_style.
        assert_eq!(applied.invalidation.text_reflow.len(), 1);
        assert!(applied.invalidation.frame_style.is_empty());

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[1].capitalization, None);
    }

    /// `characterPosition` over the first run: sets `Position`, undo
    /// restores the prior `None`.
    #[test]
    fn character_position_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::CharacterPosition,
            value: Value::Text("Superscript".to_string()),
        };
        let applied = project.apply(op).expect("apply must succeed");
        let story = &project.document().stories[0].story;
        assert_eq!(
            story.paragraphs[0].runs[0].position.as_deref(),
            Some("Superscript")
        );

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[0].position, None);
    }

    /// `characterBaselineShift` over the second run: sets the f32,
    /// undo restores the prior `None`. Covers the `Value::Length` arm.
    #[test]
    fn character_baseline_shift_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 6,
                end: 11,
            },
            path: PropertyPath::CharacterBaselineShift,
            value: Value::Length(Some(3.5)),
        };
        let applied = project.apply(op).expect("apply must succeed");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[1].baseline_shift, Some(3.5));
        assert_eq!(story.paragraphs[0].runs[0].baseline_shift, None);

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[1].baseline_shift, None);
    }

    /// `characterUnderline` over the first run, seeded with an
    /// explicit `Some(false)` prior so the `Value::Bool` inverse
    /// round-trips bytewise. Underline is paint-only → the
    /// invalidation hint targets `frame_style`, not `text_reflow`.
    #[test]
    fn character_underline_round_trips_and_is_paint_only() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        register_host_frame(&mut project, "Story/u1", "TextFrame/f1");
        // Seed an explicit prior so undo round-trips bytewise.
        project.document_mut().stories[0].story.paragraphs[0].runs[0].underline = Some(false);

        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::CharacterUnderline,
            value: Value::Bool(true),
        };
        let applied = project.apply(op).expect("apply must succeed");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[0].underline, Some(true));
        // Paint-only → frame_style, never text_reflow.
        assert_eq!(applied.invalidation.frame_style.len(), 1);
        assert!(applied.invalidation.text_reflow.is_empty());

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[0].underline, Some(false));
    }

    /// `characterLigatures` over the second run, seeded with an
    /// explicit `Some(true)` prior (the cascade default) so the
    /// inverse round-trips bytewise. Ligatures are reflow-affecting.
    #[test]
    fn character_ligatures_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        register_host_frame(&mut project, "Story/u1", "TextFrame/f1");
        project.document_mut().stories[0].story.paragraphs[0].runs[1].ligatures_on = Some(true);

        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 6,
                end: 11,
            },
            path: PropertyPath::CharacterLigatures,
            value: Value::Bool(false),
        };
        let applied = project.apply(op).expect("apply must succeed");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[1].ligatures_on, Some(false));
        // Reflow-affecting → text_reflow hint.
        assert_eq!(applied.invalidation.text_reflow.len(), 1);
        assert!(applied.invalidation.frame_style.is_empty());

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs[1].ligatures_on, Some(true));
    }

    /// Mid-run range: `characterFontStyle` over [2, 4) splits the
    /// first run "Hello " (0..6) into "He" / "ll" / "o ", mutating
    /// only the middle piece. Undo restores the prior value per
    /// (now-split-)run without re-merging.
    #[test]
    fn character_font_style_splits_runs_on_partial_range() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 2,
                end: 4,
            },
            path: PropertyPath::CharacterFontStyle,
            value: Value::Text("Bold".to_string()),
        };
        let applied = project.apply(op).expect("apply must succeed");

        // "Hello " split into three; "world" untouched → 4 runs.
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs.len(), 4);
        assert_eq!(story.paragraphs[0].runs[0].text, "He");
        assert_eq!(story.paragraphs[0].runs[0].font_style, None);
        assert_eq!(story.paragraphs[0].runs[1].text, "ll");
        assert_eq!(
            story.paragraphs[0].runs[1].font_style.as_deref(),
            Some("Bold")
        );
        assert_eq!(story.paragraphs[0].runs[2].text, "o ");
        assert_eq!(story.paragraphs[0].runs[2].font_style, None);
        assert_eq!(story.paragraphs[0].runs[3].text, "world");

        // Undo restores the middle piece's prior None (splits stay).
        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].runs.len(), 4);
        assert_eq!(story.paragraphs[0].runs[1].font_style, None);
    }

    // ---- W0.2: paragraph formatting paths ---------------------------------
    //
    // Paragraph-scope analogues of the W0.1 tests. Each proves apply
    // writes the field on every intersecting paragraph, undo restores
    // byte-equal, and (where exercised) paragraph-boundary rounding
    // and whole-struct / whole-list replacement work.

    /// `paragraphLeftIndent` + `paragraphRightIndent` over the first
    /// paragraph: sets both indents, undo restores the prior `None`.
    #[test]
    fn paragraph_indents_round_trip() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let left = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphLeftIndent,
            value: Value::Length(Some(18.0)),
        };
        let applied = project.apply(left).expect("apply left");
        let right = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphRightIndent,
            value: Value::Length(Some(9.0)),
        };
        let applied_r = project.apply(right).expect("apply right");

        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].left_indent, Some(18.0));
        assert_eq!(story.paragraphs[0].right_indent, Some(9.0));
        // Paragraph 1 untouched.
        assert_eq!(story.paragraphs[1].left_indent, None);

        crate::apply(project.document_mut(), &applied_r.inverse).expect("undo right");
        crate::apply(project.document_mut(), &applied.inverse).expect("undo left");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].left_indent, None);
        assert_eq!(story.paragraphs[0].right_indent, None);
    }

    /// `paragraphDropCapCharacters` + `paragraphDropCapLines` carry
    /// integers as `Value::Length`. The fields are non-`Option` `u32`
    /// (0 ⇒ no drop cap); undo restores the prior 0.
    #[test]
    fn paragraph_drop_cap_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let chars = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphDropCapCharacters,
            value: Value::Length(Some(1.0)),
        };
        let applied_c = project.apply(chars).expect("apply chars");
        let lines = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphDropCapLines,
            value: Value::Length(Some(3.0)),
        };
        let applied_l = project.apply(lines).expect("apply lines");

        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].drop_cap_characters, 1);
        assert_eq!(story.paragraphs[0].drop_cap_lines, 3);

        crate::apply(project.document_mut(), &applied_l.inverse).expect("undo lines");
        crate::apply(project.document_mut(), &applied_c.inverse).expect("undo chars");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].drop_cap_characters, 0);
        assert_eq!(story.paragraphs[0].drop_cap_lines, 0);
    }

    /// `paragraphHyphenation` toggle, seeded with an explicit
    /// `Some(true)` prior so the `Value::Bool` inverse round-trips
    /// bytewise. Reflow-affecting → the host-frame reflow hint fires.
    #[test]
    fn paragraph_hyphenation_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        register_host_frame(&mut project, "Story/u1", "TextFrame/f1");
        project.document_mut().stories[0].story.paragraphs[0].hyphenation = Some(true);

        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphHyphenation,
            value: Value::Bool(false),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].hyphenation, Some(false));
        // Paragraph properties are reflow-affecting.
        assert_eq!(applied.invalidation.text_reflow.len(), 1);

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].hyphenation, Some(true));
    }

    /// `paragraphKeepWithNext` carries an `Option<u32>` line count.
    /// Undo restores the prior `None`.
    #[test]
    fn paragraph_keep_with_next_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphKeepWithNext,
            value: Value::Length(Some(2.0)),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].keep_with_next, Some(2));

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].keep_with_next, None);
    }

    /// `paragraphRuleAbove` whole-struct: sets the rule, undo restores
    /// the prior all-`None` default. Proves the new `Value::ParagraphRule`
    /// variant round-trips the rule bytewise.
    #[test]
    fn paragraph_rule_above_whole_struct_round_trips() {
        use crate::operation::ParagraphRuleSpec;
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let spec = ParagraphRuleSpec {
            on: Some(true),
            color: Some("Color/Black".to_string()),
            tint: None,
            weight: Some(1.5),
            offset: Some(3.0),
            left_indent: None,
            right_indent: None,
            width: Some("ColumnWidth".to_string()),
        };
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphRuleAbove,
            value: Value::ParagraphRule(Some(spec.clone())),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].rule_above.on, Some(true));
        assert_eq!(story.paragraphs[0].rule_above.weight, Some(1.5));
        assert_eq!(
            story.paragraphs[0].rule_above.color.as_deref(),
            Some("Color/Black")
        );
        // The inverse carries the prior rule (all-None default).
        assert!(matches!(
            &applied.inverse,
            Operation::SetProperty {
                value: Value::ParagraphRule(Some(_)),
                ..
            }
        ));

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].rule_above.on, None);
        assert_eq!(story.paragraphs[0].rule_above.weight, None);
        assert_eq!(story.paragraphs[0].rule_above.color, None);
    }

    /// `paragraphTabStops` whole-list replacement: replaces the
    /// paragraph's `<TabList>`; undo restores the prior (empty) list.
    #[test]
    fn paragraph_tab_stops_list_replace_round_trips() {
        use crate::operation::TabStopSpec;
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let stops = vec![
            TabStopSpec {
                position: 36.0,
                alignment: Some("LeftAlign".to_string()),
                alignment_character: None,
                leader: None,
            },
            TabStopSpec {
                position: 144.0,
                alignment: Some("RightAlign".to_string()),
                alignment_character: None,
                leader: Some(".".to_string()),
            },
        ];
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphTabStops,
            value: Value::TabStops(stops.clone()),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].tab_list.len(), 2);
        assert_eq!(story.paragraphs[0].tab_list[0].position, 36.0);
        assert_eq!(
            story.paragraphs[0].tab_list[1].alignment.as_deref(),
            Some("RightAlign")
        );
        assert_eq!(story.paragraphs[0].tab_list[1].leader.as_deref(), Some("."));

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert!(story.paragraphs[0].tab_list.is_empty());
    }

    /// `paragraphListType` + `paragraphBulletCharacter` +
    /// `paragraphNumberingFormat`: list-authoring text paths. The
    /// bullet character round-trips through the glyph<->codepoint
    /// encoding; undo clears each override.
    #[test]
    fn paragraph_list_authoring_round_trips() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let list = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphListType,
            value: Value::Text("BulletList".to_string()),
        };
        let applied_list = project.apply(list).expect("apply list");
        let bullet = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphBulletCharacter,
            value: Value::Text("\u{2022}".to_string()), // •
        };
        let applied_bullet = project.apply(bullet).expect("apply bullet");
        let numfmt = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 0,
                end: 6,
            },
            path: PropertyPath::ParagraphNumberingFormat,
            value: Value::Text("^#.^t".to_string()),
        };
        let applied_num = project.apply(numfmt).expect("apply numfmt");

        let story = &project.document().stories[0].story;
        assert_eq!(
            story.paragraphs[0].bullets_list_type.as_deref(),
            Some("BulletList")
        );
        assert_eq!(story.paragraphs[0].bullet_character, Some(0x2022));
        assert_eq!(
            story.paragraphs[0].numbering_format.as_deref(),
            Some("^#.^t")
        );

        crate::apply(project.document_mut(), &applied_num.inverse).expect("undo num");
        crate::apply(project.document_mut(), &applied_bullet.inverse).expect("undo bullet");
        crate::apply(project.document_mut(), &applied_list.inverse).expect("undo list");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].bullets_list_type, None);
        assert_eq!(story.paragraphs[0].bullet_character, None);
        assert_eq!(story.paragraphs[0].numbering_format, None);
    }

    /// Paragraph-boundary rounding: a range [5, 12) cuts inside
    /// paragraph 0 ("Hello world" 0..11) and covers paragraph 1
    /// ("!" 11..12). `paragraphLeftIndent` rounds to whole paragraphs
    /// and writes BOTH; the inverse is a two-op Batch.
    #[test]
    fn paragraph_left_indent_rounds_to_whole_paragraphs() {
        let mut project = Project::new(document_with_one_story("Story/u1"));
        let op = Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: "Story/u1".to_string(),
                start: 5,
                end: 12,
            },
            path: PropertyPath::ParagraphLeftIndent,
            value: Value::Length(Some(24.0)),
        };
        let applied = project.apply(op).expect("apply");
        let story = &project.document().stories[0].story;
        // Both paragraphs got the indent despite the range starting
        // mid-paragraph-0.
        assert_eq!(story.paragraphs[0].left_indent, Some(24.0));
        assert_eq!(story.paragraphs[1].left_indent, Some(24.0));
        // Inverse is a Batch of two per-paragraph restorations.
        assert!(matches!(&applied.inverse, Operation::Batch { ops } if ops.len() == 2));

        crate::apply(project.document_mut(), &applied.inverse).expect("undo");
        let story = &project.document().stories[0].story;
        assert_eq!(story.paragraphs[0].left_indent, None);
        assert_eq!(story.paragraphs[1].left_indent, None);
    }

    // ---- Editor-ops: Scissors (PathOpenAt) --------------------------------

    fn open_at_op(self_id: &str, index: usize) -> Operation {
        Operation::SetProperty {
            node: NodeId::Polygon(self_id.to_string()),
            path: PropertyPath::PathOpenAt,
            value: Value::PathOpenAt {
                index,
                prev_anchors: None,
                prev_subpath_starts: None,
                prev_subpath_open: None,
            },
        }
    }

    /// Closed triangle cut at anchor 1: the contour opens there, the
    /// cut anchor splits into coincident head/tail endpoints, and
    /// every original edge survives.
    #[test]
    fn scissors_opens_closed_contour_at_anchor() {
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![
                anchor_at(0.0, 0.0),
                anchor_at(60.0, 0.0),
                anchor_at(30.0, 50.0),
            ],
            vec![0],
        );
        project.apply(open_at_op("Polygon/p1", 1)).expect("cut");
        let poly = polygon_of(&project);
        assert_eq!(
            anchor_positions(poly),
            vec![(60.0, 0.0), (30.0, 50.0), (0.0, 0.0), (60.0, 0.0)],
            "rotated so the cut anchor leads, with its twin appended"
        );
        assert_eq!(poly.subpath_starts, vec![0]);
        assert_eq!(poly.subpath_open, vec![true]);
        // Head lost its incoming handle; tail lost its outgoing one.
        assert_eq!(poly.anchors[0].left, poly.anchors[0].anchor);
        assert_eq!(poly.anchors[3].right, poly.anchors[3].anchor);
    }

    /// Open polyline cut at an interior anchor: two open subpaths
    /// sharing duplicated endpoints.
    #[test]
    fn scissors_splits_open_contour_into_two() {
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![
                anchor_at(0.0, 0.0),
                anchor_at(40.0, 10.0),
                anchor_at(80.0, 0.0),
                anchor_at(120.0, 10.0),
            ],
            vec![0],
        );
        // The fixture builds closed contours; flip this one open.
        project.document_mut().spreads[0].spread.polygons[0].subpath_open = vec![true];
        project.apply(open_at_op("Polygon/p1", 1)).expect("cut");
        let poly = polygon_of(&project);
        assert_eq!(
            anchor_positions(poly),
            vec![
                (0.0, 0.0),
                (40.0, 10.0),
                (40.0, 10.0),
                (80.0, 0.0),
                (120.0, 10.0),
            ],
        );
        assert_eq!(poly.subpath_starts, vec![0, 2]);
        assert_eq!(poly.subpath_open, vec![true, true]);
    }

    /// THE regression guard: the inverse must restore `subpath_open`
    /// (which `FramePath` cannot express) byte-identically; redo
    /// re-cuts identically.
    #[test]
    fn scissors_undo_restores_subpath_open_byte_identically() {
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![
                anchor_at(0.0, 0.0),
                anchor_at(60.0, 0.0),
                anchor_at(30.0, 50.0),
            ],
            vec![0],
        );
        let before = format!("{:?}", project.document().spreads);
        project.apply(open_at_op("Polygon/p1", 1)).expect("cut");
        let after_cut = format!("{:?}", project.document().spreads);
        project.undo().expect("undo");
        assert_eq!(format!("{:?}", project.document().spreads), before);
        project.redo().expect("redo");
        assert_eq!(format!("{:?}", project.document().spreads), after_cut);
    }

    #[test]
    fn scissors_rejects_endpoint_and_degenerate_cuts() {
        // Endpoint cut on an OPEN contour is a no-op → InvalidValue.
        let mut project = project_with_polygon(
            "Polygon/p1",
            vec![anchor_at(0.0, 0.0), anchor_at(40.0, 10.0)],
            vec![0],
        );
        project.document_mut().spreads[0].spread.polygons[0].subpath_open = vec![true];
        let err = project
            .apply(open_at_op("Polygon/p1", 0))
            .expect_err("endpoint cut");
        assert!(matches!(err, OperationError::InvalidValue { .. }));
        // Out-of-range index.
        let err = project
            .apply(open_at_op("Polygon/p1", 9))
            .expect_err("oob index");
        assert!(matches!(err, OperationError::InvalidValue { .. }));
    }

    // ---- Editor-ops: frames_in_order maintenance + new NodeSpec kinds ----

    /// Like real parsed documents, the spread carries a populated
    /// `frames_in_order` (the empty-table fixtures above exercise the
    /// legacy vec-walk fallback instead).
    fn document_with_ordered_textframes() -> Document {
        let mut doc = document_with_one_textframe("TextFrame/a");
        doc.spreads[0].spread.text_frames.push(empty_text_frame(
            "TextFrame/b",
            Bounds {
                top: 200.0,
                left: 0.0,
                bottom: 300.0,
                right: 200.0,
            },
        ));
        doc.spreads[0].spread.frames_in_order =
            vec![FrameRef::TextFrame(0), FrameRef::TextFrame(1)];
        doc
    }

    /// Inserting registers the new frame in `frames_in_order` (on top
    /// when `z_slot: None`) and remaps the kind-vec indices of every
    /// later same-kind ref — the renderer/hit-tester walk ONLY this
    /// table when it is non-empty, so a missing or stale entry means
    /// an invisible or wrongly-stacked frame.
    #[test]
    fn insert_node_registers_in_frames_in_order_and_remaps_indices() {
        let mut project = Project::new(document_with_ordered_textframes());
        let rect = |id: &str| NodeSpec::Rectangle {
            item_transform: None,
            self_id: id.to_string(),
            bounds: [0.0, 0.0, 50.0, 50.0],
            fill_color: None,
            stroke_color: None,
            stroke_weight: None,
        };
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: rect("Rectangle/r1"),
                z_slot: None,
            })
            .expect("insert r1");
        assert_eq!(
            project.document().spreads[0].spread.frames_in_order,
            vec![
                FrameRef::TextFrame(0),
                FrameRef::TextFrame(1),
                FrameRef::Rectangle(0),
            ],
        );
        // Insert a second rectangle at the FRONT of the kind vec: the
        // existing Rectangle(0) ref must remap to Rectangle(1).
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: rect("Rectangle/r2"),
                z_slot: None,
            })
            .expect("insert r2");
        let spread = &project.document().spreads[0].spread;
        assert_eq!(
            spread.rectangles[0].self_id.as_deref(),
            Some("Rectangle/r2")
        );
        assert_eq!(
            spread.frames_in_order,
            vec![
                FrameRef::TextFrame(0),
                FrameRef::TextFrame(1),
                FrameRef::Rectangle(1), // r1, shifted by the front insert
                FrameRef::Rectangle(0), // r2, on top
            ],
        );
    }

    /// Remove → undo must restore the document byte-identically,
    /// including the removed frame's exact `frames_in_order` slot —
    /// the regression guard for the z-order bookkeeping.
    #[test]
    fn remove_node_undo_round_trips_frames_in_order_byte_identically() {
        let mut project = Project::new(document_with_ordered_textframes());
        let before = format!("{:?}", project.document().spreads);
        let removed = project
            .apply(Operation::RemoveNode {
                node: NodeId::TextFrame("TextFrame/a".to_string()),
            })
            .expect("remove a");
        assert!(removed.invalidation.structural);
        // b's kind-vec index shifted 1 → 0 and a's slot is gone.
        assert_eq!(
            project.document().spreads[0].spread.frames_in_order,
            vec![FrameRef::TextFrame(0)],
        );
        project.undo().expect("undo");
        let after = format!("{:?}", project.document().spreads);
        assert_eq!(before, after, "undo of remove must be byte-identical");
        // Redo removes it again, with the same table shape.
        project.redo().expect("redo");
        assert_eq!(
            project.document().spreads[0].spread.frames_in_order,
            vec![FrameRef::TextFrame(0)],
        );
    }

    /// Editor-suite AC-E2E-PROVE-3 — undoing deleteFrame must restore
    /// the frame's `ItemTransform`, not snap it back to the page
    /// origin. `NodeSpec` carries `item_transform` since protocol v27;
    /// before that the RemoveNode capture dropped it.
    #[test]
    fn remove_node_undo_restores_item_transform() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        // a 3-4-5 rotation — clippy-clean, unlike FRAC_1_SQRT_2 approximations
        let m = [0.6, 0.8, -0.8, 0.6, 50.0, 100.0];
        project
            .apply(Operation::SetProperty {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                path: PropertyPath::FrameTransform,
                value: Value::Transform(Some(m)),
            })
            .expect("set transform");
        let before = format!("{:?}", project.document().spreads);

        project
            .apply(Operation::RemoveNode {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
            })
            .expect("remove");
        project.undo().expect("undo remove");

        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(
            frame.item_transform,
            Some(m),
            "undo of deleteFrame must restore the item transform"
        );
        let after = format!("{:?}", project.document().spreads);
        assert_eq!(before, after, "undo of remove must be byte-identical");
    }

    #[test]
    fn insert_graphic_line_round_trips() {
        let mut project = Project::new(document_with_ordered_textframes());
        let anchors = vec![
            PathAnchorSpec {
                anchor: [10.0, 20.0],
                left: [10.0, 20.0],
                right: [10.0, 20.0],
            },
            PathAnchorSpec {
                anchor: [110.0, 80.0],
                left: [110.0, 80.0],
                right: [110.0, 80.0],
            },
        ];
        let before = format!("{:?}", project.document().spreads);
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: NodeSpec::GraphicLine {
                    item_transform: None,
                    self_id: "GraphicLine/l1".to_string(),
                    bounds: [20.0, 10.0, 80.0, 110.0],
                    anchors: anchors.clone(),
                    subpath_starts: vec![0],
                    subpath_open: vec![true],
                    stroke_color: Some("Color/Black".to_string()),
                    stroke_weight: Some(1.0),
                },
                z_slot: None,
            })
            .expect("insert line");
        let spread = &project.document().spreads[0].spread;
        let line = &spread.graphic_lines[0];
        assert_eq!(line.self_id.as_deref(), Some("GraphicLine/l1"));
        assert_eq!(line.anchors.len(), 2);
        assert_eq!(line.anchors[1].anchor, (110.0, 80.0));
        assert_eq!(line.subpath_open, vec![true]);
        assert_eq!(line.stroke_color.as_deref(), Some("Color/Black"));
        assert_eq!(
            spread.frames_in_order.last(),
            Some(&FrameRef::GraphicLine(0)),
        );
        // Undo removes it byte-identically; redo brings it back.
        project.undo().expect("undo");
        let after = format!("{:?}", project.document().spreads);
        assert_eq!(before, after);
        project.redo().expect("redo");
        assert_eq!(project.document().spreads[0].spread.graphic_lines.len(), 1);
    }

    #[test]
    fn insert_polygon_round_trips_with_path_tables() {
        let mut project = Project::new(document_with_ordered_textframes());
        let anchors = vec![
            PathAnchorSpec {
                anchor: [0.0, 0.0],
                left: [0.0, 0.0],
                right: [0.0, 0.0],
            },
            PathAnchorSpec {
                anchor: [60.0, 0.0],
                left: [60.0, 0.0],
                right: [60.0, 0.0],
            },
            PathAnchorSpec {
                anchor: [30.0, 50.0],
                left: [30.0, 50.0],
                right: [30.0, 50.0],
            },
        ];
        let before = format!("{:?}", project.document().spreads);
        project
            .apply(Operation::InsertNode {
                parent: NodeId::Spread("Spread/u_main".to_string()),
                position: 0,
                node: NodeSpec::Polygon {
                    item_transform: None,
                    self_id: "Polygon/p1".to_string(),
                    bounds: [0.0, 0.0, 50.0, 60.0],
                    anchors,
                    subpath_starts: vec![0],
                    subpath_open: vec![false],
                    fill_color: Some("Color/Red".to_string()),
                    stroke_color: None,
                    stroke_weight: None,
                },
                z_slot: None,
            })
            .expect("insert polygon");
        let spread = &project.document().spreads[0].spread;
        let poly = &spread.polygons[0];
        assert_eq!(poly.anchors.len(), 3);
        assert_eq!(poly.subpath_starts, vec![0]);
        assert_eq!(poly.subpath_open, vec![false]);
        assert_eq!(poly.fill_color.as_deref(), Some("Color/Red"));
        assert_eq!(spread.frames_in_order.last(), Some(&FrameRef::Polygon(0)));
        project.undo().expect("undo");
        let after = format!("{:?}", project.document().spreads);
        assert_eq!(before, after);
        project.redo().expect("redo");
        assert_eq!(project.document().spreads[0].spread.polygons.len(), 1);
    }

    // ====================================================================
    // W0.3 — frame-scope property paths. Each family: apply → assert →
    // invert restores. Apply-invert-reapply identity, the W0.1/W0.2
    // recipe.
    // ====================================================================

    fn document_with_one_rectangle(self_id: &str) -> Document {
        let mut doc = document_with_one_textframe("TextFrame/unused");
        doc.spreads[0].spread.text_frames.clear();
        doc.spreads[0]
            .spread
            .rectangles
            .push(crate::apply::new_rectangle(
                self_id.to_string(),
                Bounds {
                    top: 0.0,
                    left: 0.0,
                    bottom: 100.0,
                    right: 200.0,
                },
                None,
            ));
        doc
    }

    fn set_op(node: NodeId, path: PropertyPath, value: Value) -> Operation {
        Operation::SetProperty { node, path, value }
    }

    /// Apply, assert the value landed via `check`, then apply the
    /// inverse and assert the document matches its pre-apply snapshot.
    fn assert_round_trips(project: &mut Project, op: Operation, check: impl Fn(&Document)) {
        let before = format!("{:?}", project.document().spreads);
        let applied = project.apply(op).expect("apply");
        check(project.document());
        crate::apply(project.document_mut(), &applied.inverse).expect("invert");
        let after = format!("{:?}", project.document().spreads);
        assert_eq!(before, after, "inverse did not restore the document");
    }

    #[test]
    fn w03_text_frame_column_prefs_round_trip() {
        let mut p = Project::new(document_with_one_textframe("TextFrame/u1"));
        let node = NodeId::TextFrame("TextFrame/u1".to_string());
        assert_round_trips(
            &mut p,
            set_op(
                node.clone(),
                PropertyPath::TextFrameColumnCount,
                Value::Length(Some(3.0)),
            ),
            |d| assert_eq!(d.spreads[0].spread.text_frames[0].column_count, Some(3)),
        );
        assert_round_trips(
            &mut p,
            set_op(
                node.clone(),
                PropertyPath::TextFrameColumnGutter,
                Value::Length(Some(14.0)),
            ),
            |d| assert_eq!(d.spreads[0].spread.text_frames[0].column_gutter, Some(14.0)),
        );
        // column_balance is `Option<bool>`; `Value::Bool` carries no
        // `None`, so undo of a write whose prior was `None` restores
        // `Some(false)` (the default), not `None` — the documented
        // `CharacterUnderline` lossy-default precedent. Assert value +
        // semantic restore rather than bytewise identity.
        let applied = p
            .apply(set_op(
                node,
                PropertyPath::TextFrameColumnBalance,
                Value::Bool(true),
            ))
            .unwrap();
        assert_eq!(
            p.document().spreads[0].spread.text_frames[0].column_balance,
            Some(true)
        );
        assert_eq!(
            applied.inverse,
            set_op(
                NodeId::TextFrame("TextFrame/u1".to_string()),
                PropertyPath::TextFrameColumnBalance,
                Value::Bool(false),
            )
        );
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        assert_eq!(
            p.document().spreads[0].spread.text_frames[0].column_balance,
            Some(false)
        );
    }

    #[test]
    fn w03_text_frame_enum_prefs_round_trip() {
        let mut p = Project::new(document_with_one_textframe("TextFrame/u1"));
        let node = NodeId::TextFrame("TextFrame/u1".to_string());
        let applied = p
            .apply(set_op(
                node.clone(),
                PropertyPath::TextFrameVerticalJustification,
                Value::Text("CenterAlign".to_string()),
            ))
            .unwrap();
        assert_eq!(
            p.document().spreads[0].spread.text_frames[0].vertical_justification,
            Some(paged_parse::VerticalJustification::Center)
        );
        // Reflow classification — vertical justify shifts every line.
        assert_eq!(applied.invalidation.text_reflow.len(), 1);
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        assert_eq!(
            p.document().spreads[0].spread.text_frames[0].vertical_justification,
            None
        );
        assert_round_trips(
            &mut p,
            set_op(
                node.clone(),
                PropertyPath::TextFrameAutoSizing,
                Value::Text("HeightOnly".into()),
            ),
            |d| {
                assert_eq!(
                    d.spreads[0].spread.text_frames[0].auto_sizing,
                    Some(paged_parse::AutoSizingType::HeightOnly)
                )
            },
        );
        assert_round_trips(
            &mut p,
            set_op(
                node,
                PropertyPath::TextFrameFirstBaseline,
                Value::Text("CapHeight".into()),
            ),
            |d| {
                assert_eq!(
                    d.spreads[0].spread.text_frames[0].first_baseline_offset,
                    Some(paged_parse::FirstBaselineOffset::CapHeight)
                )
            },
        );
    }

    #[test]
    fn w03_text_wrap_invert_round_trips_and_is_structural() {
        let mut p = Project::new(document_with_one_rectangle("Rectangle/r1"));
        let node = NodeId::Rectangle("Rectangle/r1".to_string());
        let applied = p
            .apply(set_op(
                node.clone(),
                PropertyPath::TextWrapInvert,
                Value::Bool(true),
            ))
            .unwrap();
        assert_eq!(
            p.document().spreads[0].spread.rectangles[0]
                .text_wrap
                .and_then(|t| t.invert),
            Some(true)
        );
        // The wrap exclusion changes → structural rebuild.
        assert!(applied.invalidation.structural);
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        // Materialised TextWrap stays (mode None, invert restored to
        // false); the inverse carries Bool(false).
        assert_eq!(
            p.document().spreads[0].spread.rectangles[0]
                .text_wrap
                .and_then(|t| t.invert),
            Some(false)
        );
    }

    #[test]
    fn w03_frame_fitting_alignment_and_auto_fit_round_trip() {
        // Writing either knob materialises a `FrameFittingOption` if the
        // prior was `None`; undo restores the *field* (reference_point /
        // auto_fit) but leaves the now-present (all-None) struct — the
        // same materialise-on-None lossiness as `FrameDropShadow`. The
        // field value, not the struct presence, is what round-trips.
        let mut p = Project::new(document_with_one_rectangle("Rectangle/r1"));
        let node = NodeId::Rectangle("Rectangle/r1".to_string());
        let applied = p
            .apply(set_op(
                node.clone(),
                PropertyPath::FrameFittingReferencePoint,
                Value::Text("CenterPoint".into()),
            ))
            .unwrap();
        assert_eq!(
            p.document().spreads[0].spread.rectangles[0]
                .frame_fitting
                .as_ref()
                .and_then(|ff| ff.reference_point.clone())
                .as_deref(),
            Some("CenterPoint")
        );
        assert_eq!(applied.invalidation.frame_style.len(), 1);
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        assert_eq!(
            p.document().spreads[0].spread.rectangles[0]
                .frame_fitting
                .as_ref()
                .and_then(|ff| ff.reference_point.clone()),
            None
        );
        let applied = p
            .apply(set_op(node, PropertyPath::FrameAutoFit, Value::Bool(true)))
            .unwrap();
        assert_eq!(
            p.document().spreads[0].spread.rectangles[0]
                .frame_fitting
                .as_ref()
                .and_then(|ff| ff.auto_fit),
            Some(true)
        );
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        assert_eq!(
            p.document().spreads[0].spread.rectangles[0]
                .frame_fitting
                .as_ref()
                .and_then(|ff| ff.auto_fit),
            Some(false)
        );
    }

    #[test]
    fn w03_stroke_family_round_trips_paint_only() {
        let mut p = Project::new(document_with_one_rectangle("Rectangle/r1"));
        let node = NodeId::Rectangle("Rectangle/r1".to_string());
        // Stroke type on a Rectangle → frame_style (paint), not reflow.
        let applied = p
            .apply(set_op(
                node.clone(),
                PropertyPath::FrameStrokeType,
                Value::Text("StrokeStyle/$ID/Dashed".into()),
            ))
            .unwrap();
        assert_eq!(applied.invalidation.frame_style.len(), 1);
        assert!(applied.invalidation.text_reflow.is_empty());
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        assert_eq!(
            p.document().spreads[0].spread.rectangles[0].stroke_type,
            None
        );
        for (path, value, check) in [
            (
                PropertyPath::FrameStrokeJoin,
                Value::Text("RoundEndJoin".into()),
                "join",
            ),
            (
                PropertyPath::FrameStrokeAlignment,
                Value::Text("InsideAlignment".into()),
                "align",
            ),
        ] {
            let _ = check;
            assert_round_trips(&mut p, set_op(node.clone(), path, value.clone()), |_| {});
        }
        assert_round_trips(
            &mut p,
            set_op(
                node.clone(),
                PropertyPath::FrameStrokeMiterLimit,
                Value::Length(Some(8.0)),
            ),
            |d| assert_eq!(d.spreads[0].spread.rectangles[0].miter_limit, Some(8.0)),
        );
        assert_round_trips(
            &mut p,
            set_op(
                node.clone(),
                PropertyPath::FrameStrokeGapColor,
                Value::ColorRef(Some("Color/Cyan".into())),
            ),
            |d| {
                assert_eq!(
                    d.spreads[0].spread.rectangles[0]
                        .stroke_gap_color
                        .as_deref(),
                    Some("Color/Cyan")
                )
            },
        );
        assert_round_trips(
            &mut p,
            set_op(
                node,
                PropertyPath::FrameStrokeGapTint,
                Value::Length(Some(60.0)),
            ),
            |d| {
                assert_eq!(
                    d.spreads[0].spread.rectangles[0].stroke_gap_tint,
                    Some(60.0)
                )
            },
        );
    }

    /// W1.1 — `frameStrokeDashArray` whole-list replacement. A
    /// non-empty dash list sets the per-frame override and round-trips;
    /// an empty list CLEARS a prior override (the
    /// `frameStrokeDashArray: []` clear path), and that clear-then-
    /// restore also round-trips bytewise via the captured prior list.
    #[test]
    fn w11_stroke_dash_array_round_trips_including_empty_clear() {
        let mut p = Project::new(document_with_one_rectangle("Rectangle/r1"));
        let node = NodeId::Rectangle("Rectangle/r1".to_string());

        // Set a dash pattern; paint-only invalidation, no reflow.
        let applied = p
            .apply(set_op(
                node.clone(),
                PropertyPath::FrameStrokeDashArray,
                Value::Lengths(vec![6.0, 3.0, 2.0, 3.0]),
            ))
            .unwrap();
        assert_eq!(applied.invalidation.frame_style.len(), 1);
        assert!(applied.invalidation.text_reflow.is_empty());
        assert_eq!(
            p.document().spreads[0].spread.rectangles[0].stroke_dash,
            vec![6.0, 3.0, 2.0, 3.0]
        );
        // Forward inverse carries the prior (empty) list → undo clears.
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        assert!(p.document().spreads[0].spread.rectangles[0]
            .stroke_dash
            .is_empty());

        // Whole round-trip helper over a fresh override.
        assert_round_trips(
            &mut p,
            set_op(
                node.clone(),
                PropertyPath::FrameStrokeDashArray,
                Value::Lengths(vec![10.0, 4.0]),
            ),
            |d| {
                assert_eq!(
                    d.spreads[0].spread.rectangles[0].stroke_dash,
                    vec![10.0, 4.0]
                )
            },
        );

        // Empty-clears: seed a dash, then clear it with the empty vec,
        // and prove apply→invert restores the seeded list bytewise.
        p.apply(set_op(
            node.clone(),
            PropertyPath::FrameStrokeDashArray,
            Value::Lengths(vec![5.0, 5.0]),
        ))
        .unwrap();
        let cleared = p
            .apply(set_op(
                node.clone(),
                PropertyPath::FrameStrokeDashArray,
                Value::Lengths(Vec::new()),
            ))
            .unwrap();
        assert!(p.document().spreads[0].spread.rectangles[0]
            .stroke_dash
            .is_empty());
        // The clear's inverse must carry the prior `[5,5]` so undo
        // restores it.
        crate::apply(p.document_mut(), &cleared.inverse).unwrap();
        assert_eq!(
            p.document().spreads[0].spread.rectangles[0].stroke_dash,
            vec![5.0, 5.0]
        );
    }

    #[test]
    fn w03_per_corner_option_and_radius_round_trip() {
        let mut p = Project::new(document_with_one_rectangle("Rectangle/r1"));
        let node = NodeId::Rectangle("Rectangle/r1".to_string());
        assert_round_trips(
            &mut p,
            set_op(
                node.clone(),
                PropertyPath::FrameCornerOptionBottomLeft,
                Value::Text("RoundedCorner".into()),
            ),
            |d| {
                // bottom_left is index 3 in IDML corners[4] order.
                assert_eq!(
                    d.spreads[0].spread.rectangles[0].corners[3].option,
                    Some(paged_parse::CornerOption::Rounded)
                )
            },
        );
        assert_round_trips(
            &mut p,
            set_op(
                node,
                PropertyPath::FrameCornerRadiusTopRight,
                Value::Length(Some(12.0)),
            ),
            |d| {
                // top_right is index 1.
                assert_eq!(
                    d.spreads[0].spread.rectangles[0].corners[1].radius,
                    Some(12.0)
                )
            },
        );
    }

    #[test]
    fn w03_transform_decompose_paths_round_trip() {
        let mut p = Project::new(document_with_one_textframe("TextFrame/u1"));
        let node = NodeId::TextFrame("TextFrame/u1".to_string());
        // Rotate 30°.
        let applied = p
            .apply(set_op(
                node.clone(),
                PropertyPath::FrameRotationAngle,
                Value::Length(Some(30.0)),
            ))
            .unwrap();
        // Geometry invalidation — rotating a text frame re-lays content.
        assert_eq!(applied.invalidation.frame_geometry.len(), 1);
        let m = p.document().spreads[0].spread.text_frames[0]
            .item_transform
            .expect("transform materialised");
        let d = crate::operation::decompose_transform(Some(m));
        assert!((d.angle_deg - 30.0).abs() < 1e-2, "angle {}", d.angle_deg);
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        // Inverse restores the pre-rotation angle (0 on identity input).
        let d0 = crate::operation::decompose_transform(
            p.document().spreads[0].spread.text_frames[0].item_transform,
        );
        assert!(d0.angle_deg.abs() < 1e-3);

        // Scale X to 2.0, then flip H, then back — each round-trips.
        assert_round_trips(
            &mut p,
            set_op(
                node.clone(),
                PropertyPath::FrameScaleX,
                Value::Length(Some(2.0)),
            ),
            |doc| {
                let dd = crate::operation::decompose_transform(
                    doc.spreads[0].spread.text_frames[0].item_transform,
                );
                assert!((dd.scale_x - 2.0).abs() < 1e-3, "sx {}", dd.scale_x);
            },
        );
        assert_round_trips(
            &mut p,
            set_op(node, PropertyPath::FrameFlipH, Value::Bool(true)),
            |doc| {
                let dd = crate::operation::decompose_transform(
                    doc.spreads[0].spread.text_frames[0].item_transform,
                );
                assert!(dd.flip_h, "flip_h must be set");
            },
        );
    }

    #[test]
    fn w03_overprint_round_trips() {
        let mut p = Project::new(document_with_one_rectangle("Rectangle/r1"));
        let node = NodeId::Rectangle("Rectangle/r1".to_string());
        assert_round_trips(
            &mut p,
            set_op(
                node.clone(),
                PropertyPath::FrameOverprintFill,
                Value::Bool(true),
            ),
            |d| assert!(d.spreads[0].spread.rectangles[0].overprint_fill),
        );
        assert_round_trips(
            &mut p,
            set_op(node, PropertyPath::FrameOverprintStroke, Value::Bool(true)),
            |d| assert!(d.spreads[0].spread.rectangles[0].overprint_stroke),
        );
    }

    #[test]
    fn w03_unsupported_kind_for_corner_path_errors() {
        // Corners are Rectangle-only; a TextFrame must reject them.
        let mut p = Project::new(document_with_one_textframe("TextFrame/u1"));
        let err = p
            .apply(set_op(
                NodeId::TextFrame("TextFrame/u1".to_string()),
                PropertyPath::FrameCornerRadiusTopLeft,
                Value::Length(Some(5.0)),
            ))
            .unwrap_err();
        assert!(matches!(err, OperationError::UnsupportedProperty { .. }));
    }

    // ====================================================================
    // W0.4 — transparency effects (gap 18). Each effect family: an
    // `*Enabled` toggle (materialise-on-true / clear-on-false, the
    // `FrameDropShadow` recipe) plus a per-field editor that
    // materialises the effect block with InDesign defaults when absent.
    // The toggle on a fresh frame round-trips the document bytewise; the
    // per-field write does NOT (the materialised block survives the
    // undo — the same documented lossiness as `FrameFitting` /
    // `FrameDropShadow`), so those assert the field value, not struct
    // presence.
    // ====================================================================

    /// `*Enabled` toggle on a fresh frame: true materialises a default
    /// block (carrying the InDesign preset), false clears it, and the
    /// inverse of the on-write restores the prior `None` bytewise.
    #[test]
    fn w04_inner_shadow_toggle_materialises_default_and_round_trips() {
        let mut p = Project::new(document_with_one_textframe("TextFrame/u1"));
        let node = NodeId::TextFrame("TextFrame/u1".to_string());
        assert!(p.document().spreads[0].spread.text_frames[0]
            .effects
            .is_none());
        let applied = p
            .apply(set_op(
                node.clone(),
                PropertyPath::FrameInnerShadowEnabled,
                Value::Bool(true),
            ))
            .unwrap();
        let is = p.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .and_then(|e| e.inner_shadow.as_ref())
            .expect("inner shadow materialised");
        // InDesign preset: Multiply / 75% / 120°.
        assert_eq!(is.blend_mode.as_deref(), Some("Multiply"));
        assert_eq!(is.opacity_pct, Some(75.0));
        assert_eq!(is.angle_deg, Some(120.0));
        // Inverse restores prior is_some()=false → clears the block.
        assert_eq!(
            applied.inverse,
            set_op(
                node,
                PropertyPath::FrameInnerShadowEnabled,
                Value::Bool(false)
            )
        );
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        assert!(p.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .is_none_or(|e| e.inner_shadow.is_none()));
        // Paint-only classification — no reflow.
        assert_eq!(applied.invalidation.frame_style.len(), 1);
        assert!(applied.invalidation.text_reflow.is_empty());
    }

    /// Per-field write on an absent effect materialises the block, sets
    /// the named field, and leaves the other fields at their preset
    /// defaults; a second write touches only its own field. Undo
    /// restores the *field's* prior (preset) value, not struct absence.
    #[test]
    fn w04_inner_shadow_per_field_materialises_and_restores_field() {
        let mut p = Project::new(document_with_one_textframe("TextFrame/u1"));
        let node = NodeId::TextFrame("TextFrame/u1".to_string());
        let applied = p
            .apply(set_op(
                node.clone(),
                PropertyPath::FrameInnerShadowSize,
                Value::Length(Some(12.0)),
            ))
            .unwrap();
        let is = p.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .unwrap()
            .inner_shadow
            .as_ref()
            .expect("materialised");
        assert_eq!(is.size, Some(12.0));
        assert_eq!(is.opacity_pct, Some(75.0)); // untouched preset
                                                // Second field write preserves size.
        p.apply(set_op(
            node,
            PropertyPath::FrameInnerShadowOpacity,
            Value::Length(Some(40.0)),
        ))
        .unwrap();
        let is = p.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .unwrap()
            .inner_shadow
            .as_ref()
            .unwrap();
        assert_eq!(is.opacity_pct, Some(40.0));
        assert_eq!(is.size, Some(12.0)); // preserved
                                         // Undo the size write → restores the preset default (5.0) the
                                         // block carried at materialisation, not absence.
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        let is = p.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .unwrap()
            .inner_shadow
            .as_ref()
            .expect("block survives partial undo");
        assert_eq!(is.size, Some(5.0));
    }

    #[test]
    fn w04_outer_glow_round_trips() {
        let mut p = Project::new(document_with_one_rectangle("Rectangle/r1"));
        let node = NodeId::Rectangle("Rectangle/r1".to_string());
        let applied = p
            .apply(set_op(
                node.clone(),
                PropertyPath::FrameOuterGlowEnabled,
                Value::Bool(true),
            ))
            .unwrap();
        let og = p.document().spreads[0].spread.rectangles[0]
            .effects
            .as_ref()
            .and_then(|e| e.outer_glow.as_ref())
            .expect("outer glow");
        assert_eq!(og.blend_mode.as_deref(), Some("Screen"));
        // Field writes: spread + colour.
        p.apply(set_op(
            node.clone(),
            PropertyPath::FrameOuterGlowSpread,
            Value::Length(Some(25.0)),
        ))
        .unwrap();
        p.apply(set_op(
            node,
            PropertyPath::FrameOuterGlowColor,
            Value::ColorRef(Some("Color/Cyan".into())),
        ))
        .unwrap();
        let og = p.document().spreads[0].spread.rectangles[0]
            .effects
            .as_ref()
            .unwrap()
            .outer_glow
            .as_ref()
            .unwrap();
        assert_eq!(og.spread_pct, Some(25.0));
        assert_eq!(og.effect_color.as_deref(), Some("Color/Cyan"));
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        assert!(p.document().spreads[0].spread.rectangles[0]
            .effects
            .as_ref()
            .is_none_or(|e| e.outer_glow.is_none()));
    }

    #[test]
    fn w04_inner_glow_source_and_fields_round_trip() {
        let mut p = Project::new(document_with_one_textframe("TextFrame/u1"));
        let node = NodeId::TextFrame("TextFrame/u1".to_string());
        p.apply(set_op(
            node.clone(),
            PropertyPath::FrameInnerGlowSource,
            Value::Text("CenterGlow".into()),
        ))
        .unwrap();
        let ig = p.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .unwrap()
            .inner_glow
            .as_ref()
            .expect("materialised");
        assert_eq!(ig.source.as_deref(), Some("CenterGlow"));
        assert_eq!(ig.blend_mode.as_deref(), Some("Screen")); // preset
                                                              // Empty string clears the source override.
        let cleared = p
            .apply(set_op(
                node,
                PropertyPath::FrameInnerGlowSource,
                Value::Text(String::new()),
            ))
            .unwrap();
        assert!(p.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .unwrap()
            .inner_glow
            .as_ref()
            .unwrap()
            .source
            .is_none());
        // Undo restores "CenterGlow".
        crate::apply(p.document_mut(), &cleared.inverse).unwrap();
        assert_eq!(
            p.document().spreads[0].spread.text_frames[0]
                .effects
                .as_ref()
                .unwrap()
                .inner_glow
                .as_ref()
                .unwrap()
                .source
                .as_deref(),
            Some("CenterGlow")
        );
    }

    #[test]
    fn w04_bevel_enum_color_and_length_fields_round_trip() {
        let mut p = Project::new(document_with_one_rectangle("Rectangle/r1"));
        let node = NodeId::Rectangle("Rectangle/r1".to_string());
        p.apply(set_op(
            node.clone(),
            PropertyPath::FrameBevelStyle,
            Value::Text("Emboss".into()),
        ))
        .unwrap();
        let applied_color = p
            .apply(set_op(
                node.clone(),
                PropertyPath::FrameBevelHighlightColor,
                Value::ColorRef(Some("Color/White".into())),
            ))
            .unwrap();
        p.apply(set_op(
            node,
            PropertyPath::FrameBevelDepth,
            Value::Length(Some(150.0)),
        ))
        .unwrap();
        let b = p.document().spreads[0].spread.rectangles[0]
            .effects
            .as_ref()
            .unwrap()
            .bevel
            .as_ref()
            .unwrap();
        assert_eq!(b.style.as_deref(), Some("Emboss"));
        assert_eq!(b.highlight_color.as_deref(), Some("Color/White"));
        assert_eq!(b.depth_pct, Some(150.0));
        assert_eq!(b.technique.as_deref(), Some("Smooth")); // preset
        assert_eq!(b.angle_deg, Some(120.0)); // preset
                                              // Undo the colour write restores the preset (None highlight).
        crate::apply(p.document_mut(), &applied_color.inverse).unwrap();
        assert!(p.document().spreads[0].spread.rectangles[0]
            .effects
            .as_ref()
            .unwrap()
            .bevel
            .as_ref()
            .unwrap()
            .highlight_color
            .is_none());
    }

    #[test]
    fn w04_satin_invert_bool_and_toggle_round_trip() {
        let mut p = Project::new(document_with_one_textframe("TextFrame/u1"));
        let node = NodeId::TextFrame("TextFrame/u1".to_string());
        // Toggling on materialises the preset (invert=true).
        p.apply(set_op(
            node.clone(),
            PropertyPath::FrameSatinEnabled,
            Value::Bool(true),
        ))
        .unwrap();
        let s = p.document().spreads[0].spread.text_frames[0]
            .effects
            .as_ref()
            .unwrap()
            .satin
            .as_ref()
            .unwrap();
        assert_eq!(s.invert, Some(true));
        assert_eq!(s.opacity_pct, Some(50.0));
        // Flip invert off; undo restores true.
        let applied = p
            .apply(set_op(
                node,
                PropertyPath::FrameSatinInvert,
                Value::Bool(false),
            ))
            .unwrap();
        assert_eq!(
            p.document().spreads[0].spread.text_frames[0]
                .effects
                .as_ref()
                .unwrap()
                .satin
                .as_ref()
                .unwrap()
                .invert,
            Some(false)
        );
        crate::apply(p.document_mut(), &applied.inverse).unwrap();
        assert_eq!(
            p.document().spreads[0].spread.text_frames[0]
                .effects
                .as_ref()
                .unwrap()
                .satin
                .as_ref()
                .unwrap()
                .invert,
            Some(true)
        );
    }

    #[test]
    fn w04_feather_and_directional_feather_fields_round_trip() {
        let mut p = Project::new(document_with_one_rectangle("Rectangle/r1"));
        let node = NodeId::Rectangle("Rectangle/r1".to_string());
        p.apply(set_op(
            node.clone(),
            PropertyPath::FrameFeatherWidth,
            Value::Length(Some(8.0)),
        ))
        .unwrap();
        p.apply(set_op(
            node.clone(),
            PropertyPath::FrameFeatherCornerType,
            Value::Text("Rounded".into()),
        ))
        .unwrap();
        let f = p.document().spreads[0].spread.rectangles[0]
            .effects
            .as_ref()
            .unwrap()
            .feather
            .as_ref()
            .unwrap();
        assert_eq!(f.width, Some(8.0));
        assert_eq!(f.corner_type.as_deref(), Some("Rounded"));
        // Directional feather per-edge widths.
        p.apply(set_op(
            node.clone(),
            PropertyPath::FrameDirectionalFeatherLeftWidth,
            Value::Length(Some(2.0)),
        ))
        .unwrap();
        p.apply(set_op(
            node,
            PropertyPath::FrameDirectionalFeatherBottomWidth,
            Value::Length(Some(9.0)),
        ))
        .unwrap();
        let df = p.document().spreads[0].spread.rectangles[0]
            .effects
            .as_ref()
            .unwrap()
            .directional_feather
            .as_ref()
            .unwrap();
        assert_eq!(df.left_width, Some(2.0));
        assert_eq!(df.bottom_width, Some(9.0));
        assert_eq!(df.right_width, Some(5.0)); // preset
    }

    /// Object-level blend mode on the page item itself round-trips
    /// bytewise (it's a plain `Option<String>` slot, no materialised
    /// block). `FrameOpacity` — the `<BlendingSetting>` Opacity half —
    /// already existed; this completes the pair.
    #[test]
    fn w04_frame_blend_mode_round_trips() {
        let mut p = Project::new(document_with_one_rectangle("Rectangle/r1"));
        let node = NodeId::Rectangle("Rectangle/r1".to_string());
        assert_round_trips(
            &mut p,
            set_op(
                node.clone(),
                PropertyPath::FrameBlendMode,
                Value::Text("Multiply".into()),
            ),
            |d| {
                assert_eq!(
                    d.spreads[0].spread.rectangles[0].blend_mode.as_deref(),
                    Some("Multiply")
                )
            },
        );
        // Empty string clears the override → back to None.
        let applied = p
            .apply(set_op(
                node,
                PropertyPath::FrameBlendMode,
                Value::Text(String::new()),
            ))
            .unwrap();
        assert!(p.document().spreads[0].spread.rectangles[0]
            .blend_mode
            .is_none());
        // Paint-only classification.
        assert_eq!(applied.invalidation.frame_style.len(), 1);
    }

    #[test]
    fn w04_effect_unsupported_on_graphic_line() {
        // Effects are fill-based; GraphicLine carries no effects bag, so
        // the per-field + toggle paths reject it.
        let mut p = Project::new(document_with_one_textframe("TextFrame/u1"));
        p.apply(Operation::InsertNode {
            parent: NodeId::Spread("Spread/u_main".to_string()),
            position: 0,
            z_slot: None,
            node: crate::operation::NodeSpec::GraphicLine {
                item_transform: None,
                self_id: "GraphicLine/u9".to_string(),
                bounds: [0.0, 0.0, 100.0, 100.0],
                anchors: vec![
                    crate::operation::PathAnchorSpec {
                        anchor: [0.0, 0.0],
                        left: [0.0, 0.0],
                        right: [0.0, 0.0],
                    },
                    crate::operation::PathAnchorSpec {
                        anchor: [100.0, 100.0],
                        left: [100.0, 100.0],
                        right: [100.0, 100.0],
                    },
                ],
                subpath_starts: vec![],
                subpath_open: vec![],
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(1.0),
            },
        })
        .unwrap();
        let err = p
            .apply(set_op(
                NodeId::GraphicLine("GraphicLine/u9".to_string()),
                PropertyPath::FrameOuterGlowEnabled,
                Value::Bool(true),
            ))
            .unwrap_err();
        assert!(matches!(err, OperationError::UnsupportedProperty { .. }));
    }

    // =======================================================================
    // W0.5 — wire-expansion operations
    // =======================================================================
    mod w05 {
        use super::*;
        use crate::operation::{FieldKind, GuideOrientationSpec, StyleScope};
        use paged_parse::styles::ConditionSetDef;
        use paged_parse::{Bounds, CharacterRun, ConditionDef, Paragraph, Spread, Story};
        use paged_scene::ParsedStory;
        use std::collections::BTreeMap;

        fn base_doc() -> Document {
            Document {
                container: Container {
                    mimetype: "application/vnd.adobe.indesign-idml-package".to_string(),
                    designmap_raw: Bytes::new(),
                    designmap: DesignMap::default(),
                    entries: BTreeMap::new(),
                },
                palette: Graphic::default(),
                spreads: Vec::new(),
                stories: Vec::new(),
                master_spreads: HashMap::new(),
                frame_for_story: HashMap::new(),
                text_frame_index: HashMap::new(),
                styles: StyleSheet::default(),
                anchors: Vec::new(),
            }
        }

        fn text_frame(self_id: &str, story: Option<&str>) -> ParsedTextFrame {
            let mut f = empty_text_frame(
                self_id,
                Bounds {
                    top: 0.0,
                    left: 0.0,
                    bottom: 100.0,
                    right: 200.0,
                },
            );
            f.parent_story = story.map(str::to_string);
            f
        }

        fn page(self_id: &str) -> paged_parse::Page {
            paged_parse::Page {
                self_id: Some(self_id.to_string()),
                bounds: Bounds {
                    top: 0.0,
                    left: 0.0,
                    bottom: 792.0,
                    right: 612.0,
                },
                applied_master: None,
                item_transform: None,
                master_page_transform: None,
                override_list: Vec::new(),
                name: None,
                show_master_items: None,
            }
        }

        /// Doc with two text frames (one carrying a story, one empty)
        /// on a single spread.
        fn doc_two_frames() -> Document {
            let mut spread = Spread {
                self_id: Some("Spread/u_main".to_string()),
                ..Default::default()
            };
            spread
                .text_frames
                .push(text_frame("TextFrame/from", Some("Story/u1")));
            spread.text_frames.push(text_frame("TextFrame/to", None));
            let story = Story {
                paragraphs: vec![{
                    let mut p = Paragraph::default();
                    p.runs.push(CharacterRun {
                        text: "Hello world".to_string(),
                        point_size: Some(10.0),
                        ..CharacterRun::default()
                    });
                    p
                }],
                ..Story::default()
            };
            let mut doc = base_doc();
            doc.spreads.push(paged_scene::ParsedSpread {
                src: "Spreads/Spread_u_main.xml".to_string(),
                spread,
            });
            doc.stories.push(ParsedStory {
                src: "Stories/Story_u1.xml".to_string(),
                self_id: "Story/u1".to_string(),
                story,
            });
            doc
        }

        // ---- LinkFrames / UnlinkFrames ----------------------------------

        #[test]
        fn link_then_unlink_round_trips() {
            let mut p = Project::new(doc_two_frames());
            let applied = p
                .apply(Operation::LinkFrames {
                    from: "TextFrame/from".to_string(),
                    to: "TextFrame/to".to_string(),
                })
                .expect("link must succeed");
            assert_eq!(
                p.document().spreads[0].spread.text_frames[0]
                    .next_text_frame
                    .as_deref(),
                Some("TextFrame/to")
            );
            // Inverse clears the link.
            assert_eq!(
                applied.inverse,
                Operation::UnlinkFrames {
                    frame: "TextFrame/from".to_string(),
                    prev_next: None,
                }
            );
            p.undo().expect("undo");
            assert!(p.document().spreads[0].spread.text_frames[0]
                .next_text_frame
                .is_none());
            p.redo().expect("redo");
            assert_eq!(
                p.document().spreads[0].spread.text_frames[0]
                    .next_text_frame
                    .as_deref(),
                Some("TextFrame/to")
            );
        }

        #[test]
        fn link_to_nonempty_frame_is_rejected() {
            let mut doc = doc_two_frames();
            // Give the `to` frame its own non-empty story.
            doc.spreads[0].spread.text_frames[1].parent_story = Some("Story/u2".to_string());
            doc.stories.push(ParsedStory {
                src: "Stories/Story_u2.xml".to_string(),
                self_id: "Story/u2".to_string(),
                story: Story {
                    paragraphs: vec![{
                        let mut p = Paragraph::default();
                        p.runs.push(CharacterRun {
                            text: "occupied".to_string(),
                            ..CharacterRun::default()
                        });
                        p
                    }],
                    ..Story::default()
                },
            });
            let mut p = Project::new(doc);
            let err = p
                .apply(Operation::LinkFrames {
                    from: "TextFrame/from".to_string(),
                    to: "TextFrame/to".to_string(),
                })
                .unwrap_err();
            assert!(matches!(err, OperationError::InvalidValue { .. }));
        }

        #[test]
        fn link_creating_a_cycle_is_rejected() {
            let mut doc = doc_two_frames();
            // Pre-thread: from → to already. Linking to → from closes a
            // cycle.
            doc.spreads[0].spread.text_frames[0].next_text_frame = Some("TextFrame/to".to_string());
            let mut p = Project::new(doc);
            let err = p
                .apply(Operation::LinkFrames {
                    from: "TextFrame/to".to_string(),
                    to: "TextFrame/from".to_string(),
                })
                .unwrap_err();
            assert!(matches!(err, OperationError::InvalidValue { .. }));
        }

        // ---- ApplyStyle --------------------------------------------------

        #[test]
        fn apply_paragraph_style_round_trips() {
            let mut p = Project::new(document_with_one_story("Story/u1"));
            let applied = p
                .apply(Operation::ApplyStyle {
                    story_id: "Story/u1".to_string(),
                    start: 0,
                    end: 6,
                    style: "ParagraphStyle/Heading".to_string(),
                    scope: StyleScope::Paragraph,
                })
                .expect("apply style");
            assert_eq!(
                p.document().stories[0].story.paragraphs[0]
                    .paragraph_style
                    .as_deref(),
                Some("ParagraphStyle/Heading")
            );
            crate::apply(p.document_mut(), &applied.inverse).expect("undo");
            assert!(p.document().stories[0].story.paragraphs[0]
                .paragraph_style
                .is_none());
        }

        #[test]
        fn apply_character_style_splits_runs_and_round_trips() {
            let mut p = Project::new(document_with_one_story("Story/u1"));
            // [0,6) covers "Hello " exactly (run boundary).
            let applied = p
                .apply(Operation::ApplyStyle {
                    story_id: "Story/u1".to_string(),
                    start: 0,
                    end: 6,
                    style: "CharacterStyle/Emph".to_string(),
                    scope: StyleScope::Character,
                })
                .expect("apply char style");
            assert_eq!(
                p.document().stories[0].story.paragraphs[0].runs[0]
                    .character_style
                    .as_deref(),
                Some("CharacterStyle/Emph")
            );
            crate::apply(p.document_mut(), &applied.inverse).expect("undo");
            assert!(p.document().stories[0].story.paragraphs[0].runs[0]
                .character_style
                .is_none());
        }

        // ---- InsertField -------------------------------------------------

        #[test]
        fn insert_page_number_field_round_trips() {
            let mut p = Project::new(document_with_one_story("Story/u1"));
            let applied = p
                .apply(Operation::InsertField {
                    story_id: "Story/u1".to_string(),
                    offset: 0,
                    field: FieldKind::PageNumber,
                })
                .expect("insert field");
            // The U+E018 marker now leads the first run.
            assert!(p.document().stories[0].story.paragraphs[0].runs[0]
                .text
                .starts_with('\u{E018}'));
            assert_eq!(
                applied.inverse,
                Operation::DeleteField {
                    story_id: "Story/u1".to_string(),
                    offset: 0,
                    field: FieldKind::PageNumber,
                }
            );
            crate::apply(p.document_mut(), &applied.inverse).expect("undo");
            assert!(!p.document().stories[0].story.paragraphs[0].runs[0]
                .text
                .contains('\u{E018}'));
        }

        // ---- Guide CRUD --------------------------------------------------

        fn doc_with_spread() -> Document {
            let mut spread = Spread {
                self_id: Some("Spread/u_main".to_string()),
                ..Default::default()
            };
            spread.pages.push(page("Page/u1"));
            let mut doc = base_doc();
            doc.spreads.push(paged_scene::ParsedSpread {
                src: "Spreads/Spread_u_main.xml".to_string(),
                spread,
            });
            doc
        }

        #[test]
        fn guide_insert_move_delete_round_trip() {
            let mut p = Project::new(doc_with_spread());
            let applied = p
                .apply(Operation::InsertGuide {
                    spread_id: "Spread/u_main".to_string(),
                    orientation: GuideOrientationSpec::Vertical,
                    position: 100.0,
                    page_index: 0,
                    guide_id: None,
                })
                .expect("insert guide");
            assert_eq!(p.document().spreads[0].spread.guides.len(), 1);
            let gid = match &applied.op {
                Operation::InsertGuide { guide_id, .. } => guide_id.clone().unwrap(),
                _ => unreachable!(),
            };
            // Move it.
            let moved = p
                .apply(Operation::MoveGuide {
                    guide_id: gid.clone(),
                    position: 150.0,
                })
                .expect("move guide");
            assert_eq!(p.document().spreads[0].spread.guides[0].location, 150.0);
            assert_eq!(
                moved.inverse,
                Operation::MoveGuide {
                    guide_id: gid.clone(),
                    position: 100.0,
                }
            );
            // Delete it; undo restores geometry.
            p.apply(Operation::DeleteGuide {
                guide_id: gid.clone(),
            })
            .expect("delete guide");
            assert_eq!(p.document().spreads[0].spread.guides.len(), 0);
            p.undo().expect("undo delete");
            assert_eq!(p.document().spreads[0].spread.guides.len(), 1);
            assert_eq!(p.document().spreads[0].spread.guides[0].location, 150.0);
        }

        #[test]
        fn delete_missing_guide_is_rejected() {
            let mut p = Project::new(doc_with_spread());
            let err = p
                .apply(Operation::DeleteGuide {
                    guide_id: "Guide/Spread/u_main/0".to_string(),
                })
                .unwrap_err();
            assert!(matches!(
                err,
                OperationError::CollectionEntryNotFound { .. }
            ));
        }

        // ---- Conditions --------------------------------------------------

        fn doc_with_conditions() -> Document {
            let mut doc = base_doc();
            let mut conditions: BTreeMap<String, ConditionDef> = BTreeMap::new();
            conditions.insert(
                "Condition/A".to_string(),
                ConditionDef {
                    self_id: "Condition/A".to_string(),
                    name: Some("A".to_string()),
                    visible: Some(true),
                    indicator_method: None,
                },
            );
            conditions.insert(
                "Condition/B".to_string(),
                ConditionDef {
                    self_id: "Condition/B".to_string(),
                    name: Some("B".to_string()),
                    visible: Some(true),
                    indicator_method: None,
                },
            );
            let mut sets: BTreeMap<String, ConditionSetDef> = BTreeMap::new();
            sets.insert(
                "ConditionSet/Print".to_string(),
                ConditionSetDef {
                    self_id: "ConditionSet/Print".to_string(),
                    name: Some("Print".to_string()),
                    conditions: vec!["Condition/A".to_string()],
                },
            );
            doc.styles.conditions = conditions;
            doc.styles.condition_sets = sets;
            doc
        }

        #[test]
        fn set_condition_visible_round_trips() {
            let mut p = Project::new(doc_with_conditions());
            let applied = p
                .apply(Operation::SetConditionVisible {
                    condition: "Condition/A".to_string(),
                    visible: false,
                })
                .expect("set visible");
            assert_eq!(
                p.document().styles.conditions["Condition/A"].visible,
                Some(false)
            );
            assert_eq!(
                applied.inverse,
                Operation::SetConditionVisible {
                    condition: "Condition/A".to_string(),
                    visible: true,
                }
            );
            p.undo().expect("undo");
            assert_eq!(
                p.document().styles.conditions["Condition/A"].visible,
                Some(true)
            );
        }

        #[test]
        fn activate_condition_set_round_trips() {
            let mut p = Project::new(doc_with_conditions());
            p.apply(Operation::ActivateConditionSet {
                set: "ConditionSet/Print".to_string(),
            })
            .expect("activate set");
            // Member A visible, non-member B hidden.
            assert_eq!(
                p.document().styles.conditions["Condition/A"].visible,
                Some(true)
            );
            assert_eq!(
                p.document().styles.conditions["Condition/B"].visible,
                Some(false)
            );
            p.undo().expect("undo");
            // Restored to the prior (both visible).
            assert_eq!(
                p.document().styles.conditions["Condition/A"].visible,
                Some(true)
            );
            assert_eq!(
                p.document().styles.conditions["Condition/B"].visible,
                Some(true)
            );
        }

        // ---- ApplyMasterToPage ------------------------------------------

        #[test]
        fn apply_master_to_page_round_trips() {
            let mut p = Project::new(doc_with_spread());
            let applied = p
                .apply(Operation::ApplyMasterToPage {
                    page: "Page/u1".to_string(),
                    master: Some("MasterSpread/uA".to_string()),
                })
                .expect("apply master");
            assert_eq!(
                p.document().spreads[0].spread.pages[0]
                    .applied_master
                    .as_deref(),
                Some("MasterSpread/uA")
            );
            assert_eq!(
                applied.inverse,
                Operation::ApplyMasterToPage {
                    page: "Page/u1".to_string(),
                    master: None,
                }
            );
            p.undo().expect("undo");
            assert!(p.document().spreads[0].spread.pages[0]
                .applied_master
                .is_none());
        }

        // ---- DuplicatePage ----------------------------------------------

        #[test]
        fn duplicate_page_clones_with_fresh_ids_and_round_trips() {
            let mut doc = doc_with_spread();
            // Put a rectangle on the source spread so the clone copies it.
            doc.spreads[0]
                .spread
                .rectangles
                .push(crate::apply::new_rectangle(
                    "Rectangle/r1".to_string(),
                    Bounds {
                        top: 1.0,
                        left: 1.0,
                        bottom: 50.0,
                        right: 50.0,
                    },
                    Some("Color/Red".to_string()),
                ));
            let mut p = Project::new(doc);
            let applied = p
                .apply(Operation::DuplicatePage {
                    page: "Page/u1".to_string(),
                    clone_spread_json: None,
                })
                .expect("duplicate page");
            assert_eq!(p.document().spreads.len(), 2);
            // The clone's page id differs from the source.
            let clone_page = p.document().spreads[1].spread.pages[0].self_id.clone();
            assert_ne!(clone_page.as_deref(), Some("Page/u1"));
            // The clone carries a copied rectangle with a fresh id.
            assert_eq!(p.document().spreads[1].spread.rectangles.len(), 1);
            assert_ne!(
                p.document().spreads[1].spread.rectangles[0]
                    .self_id
                    .as_deref(),
                Some("Rectangle/r1")
            );
            // Inverse removes the cloned page.
            assert!(matches!(applied.inverse, Operation::RemovePage { .. }));
            p.undo().expect("undo");
            assert_eq!(p.document().spreads.len(), 1);
            p.redo().expect("redo");
            assert_eq!(p.document().spreads.len(), 2);
        }

        // ---- Sections ----------------------------------------------------

        #[test]
        fn insert_edit_delete_section_round_trip() {
            let mut p = Project::new(doc_with_spread());
            let applied = p
                .apply(Operation::InsertSection {
                    at_page: "Page/u1".to_string(),
                    prefix: Some("A-".to_string()),
                    numbering_style: Some("UpperRoman".to_string()),
                    start_at: Some(1),
                    self_id: None,
                })
                .expect("insert section");
            assert_eq!(p.document().container.designmap.sections.len(), 1);
            let sid = match &applied.op {
                Operation::InsertSection { self_id, .. } => self_id.clone().unwrap(),
                _ => unreachable!(),
            };
            assert_eq!(
                p.document().container.designmap.sections[0].numbering_style,
                paged_parse::NumberingStyle::UpperRoman
            );
            // Edit it.
            p.apply(Operation::EditSection {
                section_id: sid.clone(),
                prefix: Some(None),
                numbering_style: Some("Arabic".to_string()),
                start_at: Some(Some(5)),
            })
            .expect("edit section");
            assert_eq!(
                p.document().container.designmap.sections[0].numbering_style,
                paged_parse::NumberingStyle::Arabic
            );
            assert_eq!(
                p.document().container.designmap.sections[0].start_at,
                Some(5)
            );
            // Undo the edit restores UpperRoman + prefix.
            p.undo().expect("undo edit");
            assert_eq!(
                p.document().container.designmap.sections[0].numbering_style,
                paged_parse::NumberingStyle::UpperRoman
            );
            assert_eq!(
                p.document().container.designmap.sections[0]
                    .section_prefix
                    .as_deref(),
                Some("A-")
            );
            // Delete + undo restores it.
            p.apply(Operation::DeleteSection {
                section_id: sid.clone(),
            })
            .expect("delete section");
            assert_eq!(p.document().container.designmap.sections.len(), 0);
            p.undo().expect("undo delete");
            assert_eq!(p.document().container.designmap.sections.len(), 1);
        }

        // ---- Oval NodeSpec ----------------------------------------------

        #[test]
        fn insert_oval_round_trips() {
            let mut p = Project::new(doc_with_spread());
            let before = format!("{:?}", p.document().spreads[0].spread.ovals);
            let applied = p
                .apply(Operation::InsertNode {
                    parent: NodeId::Spread("Spread/u_main".to_string()),
                    position: 0,
                    node: NodeSpec::Oval {
                        self_id: "Oval/o1".to_string(),
                        bounds: [10.0, 20.0, 60.0, 120.0],
                        fill_color: Some("Color/Blue".to_string()),
                        stroke_color: Some("Color/Black".to_string()),
                        stroke_weight: Some(2.0),
                        item_transform: None,
                    },
                    z_slot: None,
                })
                .expect("insert oval");
            assert_eq!(p.document().spreads[0].spread.ovals.len(), 1);
            assert_eq!(
                p.document().spreads[0].spread.ovals[0].self_id.as_deref(),
                Some("Oval/o1")
            );
            assert!(matches!(applied.inverse, Operation::RemoveNode { .. }));
            // Undo removes it byte-identically; redo brings it back.
            p.undo().expect("undo");
            assert_eq!(
                format!("{:?}", p.document().spreads[0].spread.ovals),
                before
            );
            p.redo().expect("redo");
            assert_eq!(p.document().spreads[0].spread.ovals.len(), 1);
        }
    }

    // ---- W3.A1 — table NodeId surface -----------------------------

    mod tables {
        use super::*;
        use paged_parse::{Story, Table, TableCell, TableColumn, TableRow};

        /// A document whose story `Story/t1` holds a single 2×2 table
        /// `Table/tbl1` (one host paragraph carrying the table). Cells
        /// are named `"col:row"`; each carries a distinguishing fill so
        /// round-trips are observable. A `TextFrame/u1` hosts the story
        /// so `frame_for_story` resolves for reflow hints.
        fn document_with_table() -> Document {
            let mut spread = Spread {
                self_id: Some("Spread/u_main".to_string()),
                ..Default::default()
            };
            let mut frame = empty_text_frame(
                "TextFrame/u1",
                Bounds {
                    top: 0.0,
                    left: 0.0,
                    bottom: 200.0,
                    right: 200.0,
                },
            );
            frame.parent_story = Some("Story/t1".to_string());
            spread.text_frames.push(frame.clone());

            let cell = |col: u32, row: u32, fill: &str| TableCell {
                name: Some(format!("{col}:{row}")),
                row_span: 1,
                column_span: 1,
                fill_color: Some(fill.to_string()),
                ..Default::default()
            };
            let table = Table {
                self_id: Some("Table/tbl1".to_string()),
                body_row_count: 2,
                column_count: 2,
                rows: vec![
                    TableRow {
                        name: Some("0".into()),
                        single_row_height: Some(20.0),
                        ..Default::default()
                    },
                    TableRow {
                        name: Some("1".into()),
                        single_row_height: Some(30.0),
                        ..Default::default()
                    },
                ],
                columns: vec![
                    TableColumn {
                        name: Some("0".into()),
                        single_column_width: Some(50.0),
                        ..Default::default()
                    },
                    TableColumn {
                        name: Some("1".into()),
                        single_column_width: Some(60.0),
                        ..Default::default()
                    },
                ],
                cells: vec![
                    cell(0, 0, "Color/A"),
                    cell(1, 0, "Color/B"),
                    cell(0, 1, "Color/C"),
                    cell(1, 1, "Color/D"),
                ],
                ..Default::default()
            };
            let host = paged_parse::Paragraph {
                table: Some(table),
                ..Default::default()
            };
            let story = Story {
                paragraphs: vec![host],
                ..Default::default()
            };

            let mut frame_for_story = HashMap::new();
            frame_for_story.insert("Story/t1".to_string(), frame);

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
                stories: vec![ParsedStory {
                    src: "Stories/Story_t1.xml".to_string(),
                    self_id: "Story/t1".to_string(),
                    story,
                }],
                master_spreads: HashMap::new(),
                frame_for_story,
                text_frame_index: HashMap::new(),
                styles: StyleSheet::default(),
                anchors: Vec::new(),
            }
        }

        /// Borrow the live table from the document under test.
        fn table_of(doc: &Document) -> &Table {
            doc.stories[0].story.paragraphs[0]
                .table
                .as_ref()
                .expect("table present")
        }

        fn cell_named(t: &Table, col: u32, row: u32) -> &TableCell {
            t.cells
                .iter()
                .find(|c| c.coords() == Some((col, row)))
                .expect("cell present")
        }

        #[test]
        fn cell_fill_color_writes_and_round_trips() {
            let mut p = Project::new(document_with_table());
            let node = NodeId::TableCell {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                row: 1,
                col: 0,
            };
            let applied = p
                .apply(Operation::SetProperty {
                    node: node.clone(),
                    path: PropertyPath::CellFillColor,
                    value: Value::ColorRef(Some("Color/Z".into())),
                })
                .expect("set cell fill");
            assert_eq!(
                cell_named(table_of(p.document()), 0, 1)
                    .fill_color
                    .as_deref(),
                Some("Color/Z")
            );
            // Reflow targets the host frame.
            assert_eq!(
                applied.invalidation.text_reflow,
                vec![NodeId::TextFrame("TextFrame/u1".into())]
            );
            // Inverse restores the prior fill ("Color/C").
            p.undo().expect("undo");
            assert_eq!(
                cell_named(table_of(p.document()), 0, 1)
                    .fill_color
                    .as_deref(),
                Some("Color/C")
            );
            p.redo().expect("redo");
            assert_eq!(
                cell_named(table_of(p.document()), 0, 1)
                    .fill_color
                    .as_deref(),
                Some("Color/Z")
            );
        }

        #[test]
        fn cell_insets_and_vertical_justify_round_trip() {
            let mut p = Project::new(document_with_table());
            let node = NodeId::TableCell {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                row: 0,
                col: 1,
            };
            p.apply(Operation::SetProperty {
                node: node.clone(),
                path: PropertyPath::CellInsetTop,
                value: Value::Length(Some(7.5)),
            })
            .expect("set inset");
            p.apply(Operation::SetProperty {
                node: node.clone(),
                path: PropertyPath::CellVerticalJustification,
                value: Value::Text("CenterAlign".into()),
            })
            .expect("set vjust");
            let c = cell_named(table_of(p.document()), 1, 0);
            assert_eq!(c.text_top_inset, 7.5);
            assert_eq!(c.vertical_justification.as_deref(), Some("CenterAlign"));
            // Undo vjust, then inset.
            p.undo().expect("undo vjust");
            p.undo().expect("undo inset");
            let c = cell_named(table_of(p.document()), 1, 0);
            assert_eq!(c.text_top_inset, 0.0);
            assert_eq!(c.vertical_justification, None);
        }

        #[test]
        fn applied_table_style_round_trips() {
            let mut p = Project::new(document_with_table());
            let node = NodeId::Table {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
            };
            p.apply(Operation::SetProperty {
                node: node.clone(),
                path: PropertyPath::AppliedTableStyle,
                value: Value::Text("TableStyle/Fancy".into()),
            })
            .expect("set table style");
            assert_eq!(
                table_of(p.document()).applied_table_style.as_deref(),
                Some("TableStyle/Fancy")
            );
            p.undo().expect("undo");
            assert_eq!(table_of(p.document()).applied_table_style, None);
        }

        #[test]
        fn set_row_height_round_trips() {
            let mut p = Project::new(document_with_table());
            p.apply(Operation::SetRowHeight {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                row: 1,
                height: Some(99.0),
            })
            .expect("set row height");
            assert_eq!(table_of(p.document()).rows[1].single_row_height, Some(99.0));
            p.undo().expect("undo");
            // Prior height was 30.0.
            assert_eq!(table_of(p.document()).rows[1].single_row_height, Some(30.0));
        }

        #[test]
        fn insert_row_shifts_cells_and_round_trips() {
            let mut p = Project::new(document_with_table());
            let before = format!("{:?}", table_of(p.document()).cells);
            p.apply(Operation::InsertTableRow {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                at: 1,
                restore: None,
            })
            .expect("insert row");
            let t = table_of(p.document());
            assert_eq!(t.rows.len(), 3);
            // The old row-1 cells (Color/C, Color/D) shifted to row 2.
            assert_eq!(cell_named(t, 0, 2).fill_color.as_deref(), Some("Color/C"));
            // Fresh empty cells minted at row 1.
            assert_eq!(cell_named(t, 0, 1).fill_color, None);
            // Undo removes the inserted row and restores cell layout.
            p.undo().expect("undo");
            assert_eq!(table_of(p.document()).rows.len(), 2);
            assert_eq!(format!("{:?}", table_of(p.document()).cells), before);
        }

        #[test]
        fn delete_row_restores_content_on_undo() {
            let mut p = Project::new(document_with_table());
            // Delete row 0 (cells Color/A, Color/B).
            p.apply(Operation::DeleteTableRow {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                at: 0,
            })
            .expect("delete row");
            let t = table_of(p.document());
            assert_eq!(t.rows.len(), 1);
            // Surviving row (was row 1) shifted up to row 0.
            assert_eq!(cell_named(t, 0, 0).fill_color.as_deref(), Some("Color/C"));
            assert_eq!(cell_named(t, 1, 0).fill_color.as_deref(), Some("Color/D"));
            // Undo restores the deleted row's cells (Color/A, Color/B)
            // with the surviving row pushed back to row 1.
            p.undo().expect("undo");
            let t = table_of(p.document());
            assert_eq!(t.rows.len(), 2);
            assert_eq!(cell_named(t, 0, 0).fill_color.as_deref(), Some("Color/A"));
            assert_eq!(cell_named(t, 1, 0).fill_color.as_deref(), Some("Color/B"));
            assert_eq!(cell_named(t, 0, 1).fill_color.as_deref(), Some("Color/C"));
        }

        #[test]
        fn insert_and_delete_column_round_trip() {
            let mut p = Project::new(document_with_table());
            p.apply(Operation::InsertTableColumn {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                at: 1,
                restore: None,
            })
            .expect("insert col");
            let t = table_of(p.document());
            assert_eq!(t.columns.len(), 3);
            // Old col-1 cells (Color/B at row 0) shifted to col 2.
            assert_eq!(cell_named(t, 2, 0).fill_color.as_deref(), Some("Color/B"));
            assert_eq!(cell_named(t, 1, 0).fill_color, None);
            p.undo().expect("undo insert col");
            assert_eq!(table_of(p.document()).columns.len(), 2);

            // Now delete column 0 and undo to confirm content restore.
            p.apply(Operation::DeleteTableColumn {
                story_id: "Story/t1".into(),
                table_id: "Table/tbl1".into(),
                at: 0,
            })
            .expect("delete col");
            assert_eq!(table_of(p.document()).columns.len(), 1);
            p.undo().expect("undo delete col");
            let t = table_of(p.document());
            assert_eq!(t.columns.len(), 2);
            assert_eq!(cell_named(t, 0, 0).fill_color.as_deref(), Some("Color/A"));
            assert_eq!(cell_named(t, 0, 1).fill_color.as_deref(), Some("Color/C"));
        }

        #[test]
        fn delete_last_row_is_rejected() {
            let mut doc = document_with_table();
            // Collapse to a single-row table.
            {
                let t = doc.stories[0].story.paragraphs[0].table.as_mut().unwrap();
                t.rows.truncate(1);
                t.cells.retain(|c| matches!(c.coords(), Some((_, 0))));
            }
            let mut p = Project::new(doc);
            let err = p
                .apply(Operation::DeleteTableRow {
                    story_id: "Story/t1".into(),
                    table_id: "Table/tbl1".into(),
                    at: 0,
                })
                .unwrap_err();
            assert!(matches!(err, OperationError::InvalidValue { .. }));
        }

        #[test]
        fn unknown_table_is_node_not_found() {
            let mut p = Project::new(document_with_table());
            let err = p
                .apply(Operation::SetProperty {
                    node: NodeId::Table {
                        story_id: "Story/t1".into(),
                        table_id: "Table/missing".into(),
                    },
                    path: PropertyPath::AppliedTableStyle,
                    value: Value::Text("TableStyle/X".into()),
                })
                .unwrap_err();
            assert!(matches!(err, OperationError::NodeNotFound(_)));
        }
    }
}
