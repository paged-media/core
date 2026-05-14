//! Typed mutation surface for the IDML scene graph.
//!
//! The inspector (`idml-introspect`) and any other interactive
//! consumer goes through this crate to change scene-graph state.
//! A [`Mutation`] is a `(node, property, new_value)` triple; calling
//! [`Project::apply`] mutates the underlying `Document`, captures the
//! previous value for undo/journal, returns an [`InvalidationKind`]
//! so caches can evict the right slice, and fires the [`Notifier`]
//! so subscribed views can refresh.
//!
//! Coverage is deliberately narrow today: enough property kinds for
//! the inspector to demonstrate the round-trip (frame bounds + frame
//! fill colour), with room for the descriptor surface in
//! `idml-introspect` to drive what gets added next.
//!
//! Reuses lessons from the deleted `idml-edit` crate (see
//! `docs/RETROSPECTIVE.md`):
//!   - operation + patch shape ✅
//!   - transient-vs-commit modelled as `session_id: Option<SessionId>`
//!     rather than a `transient` flag (the flag's edge cases bit us)
//!   - one uniform `Mutation` type rather than a wide `Command` enum,
//!     keyed by `PropertyKey` instead of operation name

use std::cell::RefCell;
use std::rc::Rc;

use idml_parse::{Bounds, TextFrame};
use idml_scene::Document;

/// Stable identifier for a node in the scene graph. Today covers only
/// the frame kinds the inspector wires up; new variants land as
/// `idml-introspect` exposes more node types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeId {
    TextFrame(String),
    Rectangle(String),
    Oval(String),
    Polygon(String),
    GraphicLine(String),
    Group(String),
}

impl NodeId {
    pub fn self_id(&self) -> &str {
        match self {
            NodeId::TextFrame(s)
            | NodeId::Rectangle(s)
            | NodeId::Oval(s)
            | NodeId::Polygon(s)
            | NodeId::GraphicLine(s)
            | NodeId::Group(s) => s,
        }
    }
}

/// Typed property keys. Each one nominates both the *kind* of value
/// that can be written (`PropertyValue` discriminant) and the
/// invalidation cost of changing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropertyKey {
    /// Frame geometric bounds in spread coords: `[top, left, bottom, right]`.
    FrameBounds,
    /// Frame fill-color reference, e.g. `Some("Color/Red")`, `None` for unset.
    FrameFillColor,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Bounds([f32; 4]),
    ColorRef(Option<String>),
}

#[derive(Debug, Clone)]
pub struct Mutation {
    pub node: NodeId,
    pub property: PropertyKey,
    pub value: PropertyValue,
}

#[derive(Debug, Clone)]
pub struct MutationResult {
    pub node: NodeId,
    pub property: PropertyKey,
    pub previous_value: PropertyValue,
    pub new_value: PropertyValue,
    pub invalidation: InvalidationKind,
}

/// How much of the cached pipeline must be evicted after a mutation.
/// Ordered roughly cheapest → most expensive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidationKind {
    /// Style-only change on a single frame; only the frame's paint
    /// commands need to refresh.
    FrameStyle,
    /// Frame geometry changed; the frame's position/size in the
    /// display list rebuilds, and any threaded-story line distribution
    /// must re-flow.
    FrameGeometry,
    /// Text content or paragraph attributes changed; the host story
    /// re-shapes + re-composes.
    Text,
}

#[derive(Debug, thiserror::Error)]
pub enum MutationError {
    #[error("node not found: {0:?}")]
    NodeNotFound(NodeId),
    #[error("property {1:?} is not supported on {0:?}")]
    UnsupportedProperty(NodeId, PropertyKey),
    #[error("value type for property {0:?} doesn't match (expected {1})")]
    TypeMismatch(PropertyKey, &'static str),
}

/// Tiny pub-sub for `MutationResult`s. Subscribers register a closure;
/// every successful `Project::apply` fans out the result. Single-
/// threaded (the inspector runs on the wasm main thread); native-side
/// callers needing thread-safety can wrap in their own channel.
#[derive(Default)]
pub struct Notifier {
    listeners: Vec<Box<dyn FnMut(&MutationResult)>>,
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe<F: FnMut(&MutationResult) + 'static>(&mut self, f: F) {
        self.listeners.push(Box::new(f));
    }

    pub fn notify(&mut self, result: &MutationResult) {
        for listener in &mut self.listeners {
            listener(result);
        }
    }
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifier")
            .field("listener_count", &self.listeners.len())
            .finish()
    }
}

/// Holds a `Document` plus the mutation surface around it.
///
/// `Project` is the single owner during an interactive session.
/// `idml-introspect` wraps one of these and exposes it to JS via
/// `Rc<RefCell<Project>>` — mirroring the deleted `idml-edit`'s
/// `ProjectHandle` pattern but stripped down to what the inspector
/// actually needs.
pub struct Project {
    document: Document,
    notifier: Notifier,
}

impl Project {
    pub fn new(document: Document) -> Self {
        Self {
            document,
            notifier: Notifier::new(),
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn notifier_mut(&mut self) -> &mut Notifier {
        &mut self.notifier
    }

    pub fn apply(&mut self, mutation: Mutation) -> Result<MutationResult, MutationError> {
        let result = apply(&mut self.document, &mutation)?;
        self.notifier.notify(&result);
        Ok(result)
    }

    /// Convenience: wrap in `Rc<RefCell<>>` for the WASM/JS shared-
    /// ownership model. Equivalent to `Rc::new(RefCell::new(self))`.
    pub fn into_shared(self) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(self))
    }
}

/// Apply a mutation to a `Document` without going through `Project`
/// (handy in tests + when the caller already has unique `&mut`).
/// Returns the captured `MutationResult`.
pub fn apply(
    document: &mut Document,
    mutation: &Mutation,
) -> Result<MutationResult, MutationError> {
    match (&mutation.node, mutation.property) {
        (NodeId::TextFrame(id), PropertyKey::FrameBounds) => {
            let new_bounds = match &mutation.value {
                PropertyValue::Bounds(b) => *b,
                _ => return Err(MutationError::TypeMismatch(mutation.property, "Bounds")),
            };
            let frame = find_text_frame_mut(document, id)
                .ok_or_else(|| MutationError::NodeNotFound(mutation.node.clone()))?;
            let prev = frame.bounds;
            frame.bounds = Bounds {
                top: new_bounds[0],
                left: new_bounds[1],
                bottom: new_bounds[2],
                right: new_bounds[3],
            };
            Ok(MutationResult {
                node: mutation.node.clone(),
                property: PropertyKey::FrameBounds,
                previous_value: PropertyValue::Bounds([
                    prev.top,
                    prev.left,
                    prev.bottom,
                    prev.right,
                ]),
                new_value: PropertyValue::Bounds(new_bounds),
                invalidation: InvalidationKind::FrameGeometry,
            })
        }
        (NodeId::TextFrame(id), PropertyKey::FrameFillColor) => {
            let new_color = match &mutation.value {
                PropertyValue::ColorRef(c) => c.clone(),
                _ => return Err(MutationError::TypeMismatch(mutation.property, "ColorRef")),
            };
            let frame = find_text_frame_mut(document, id)
                .ok_or_else(|| MutationError::NodeNotFound(mutation.node.clone()))?;
            let prev = frame.fill_color.clone();
            frame.fill_color = new_color.clone();
            Ok(MutationResult {
                node: mutation.node.clone(),
                property: PropertyKey::FrameFillColor,
                previous_value: PropertyValue::ColorRef(prev),
                new_value: PropertyValue::ColorRef(new_color),
                invalidation: InvalidationKind::FrameStyle,
            })
        }
        _ => Err(MutationError::UnsupportedProperty(
            mutation.node.clone(),
            mutation.property,
        )),
    }
}

/// Walk every parsed spread looking for the TextFrame whose `self_id`
/// matches. O(N) — fine for the inspector's interactive cadence; a
/// pre-built mutable index can land if profiling proves it.
fn find_text_frame_mut<'a>(document: &'a mut Document, self_id: &str) -> Option<&'a mut TextFrame> {
    for parsed in &mut document.spreads {
        for frame in &mut parsed.spread.text_frames {
            if frame.self_id.as_deref() == Some(self_id) {
                return Some(frame);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    use bytes::Bytes;
    use idml_parse::{Container, DesignMap, Graphic, Spread, StyleSheet, TextFrame};
    use idml_scene::ParsedSpread;

    fn empty_text_frame(self_id: &str, bounds: Bounds) -> TextFrame {
        TextFrame {
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

    #[test]
    fn frame_bounds_mutation_updates_the_frame_and_returns_previous() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));

        let result = project
            .apply(Mutation {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                property: PropertyKey::FrameBounds,
                value: PropertyValue::Bounds([10.0, 20.0, 110.0, 220.0]),
            })
            .expect("apply must succeed");

        assert_eq!(
            result.previous_value,
            PropertyValue::Bounds([0.0, 0.0, 100.0, 200.0])
        );
        assert_eq!(
            result.new_value,
            PropertyValue::Bounds([10.0, 20.0, 110.0, 220.0])
        );
        assert_eq!(result.invalidation, InvalidationKind::FrameGeometry);

        let frame = &project.document().spreads[0].spread.text_frames[0];
        assert_eq!(frame.bounds.top, 10.0);
        assert_eq!(frame.bounds.left, 20.0);
        assert_eq!(frame.bounds.bottom, 110.0);
        assert_eq!(frame.bounds.right, 220.0);
    }

    #[test]
    fn frame_fill_color_mutation_updates_the_color_ref() {
        let mut project = Project::new(document_with_one_textframe("TextFrame/u2"));

        let result = project
            .apply(Mutation {
                node: NodeId::TextFrame("TextFrame/u2".to_string()),
                property: PropertyKey::FrameFillColor,
                value: PropertyValue::ColorRef(Some("Color/Red".to_string())),
            })
            .expect("apply must succeed");

        assert_eq!(result.previous_value, PropertyValue::ColorRef(None));
        assert_eq!(
            result.new_value,
            PropertyValue::ColorRef(Some("Color/Red".to_string()))
        );
        assert_eq!(result.invalidation, InvalidationKind::FrameStyle);

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
            .apply(Mutation {
                node: NodeId::TextFrame("TextFrame/missing".to_string()),
                property: PropertyKey::FrameBounds,
                value: PropertyValue::Bounds([0.0, 0.0, 1.0, 1.0]),
            })
            .unwrap_err();
        matches!(err, MutationError::NodeNotFound(_));
    }

    #[test]
    fn notifier_fires_on_successful_mutation() {
        use std::cell::Cell;
        use std::rc::Rc;

        let mut project = Project::new(document_with_one_textframe("TextFrame/u1"));
        let counter = Rc::new(Cell::new(0));
        {
            let counter = counter.clone();
            project.notifier_mut().subscribe(move |_result| {
                counter.set(counter.get() + 1);
            });
        }

        project
            .apply(Mutation {
                node: NodeId::TextFrame("TextFrame/u1".to_string()),
                property: PropertyKey::FrameBounds,
                value: PropertyValue::Bounds([1.0, 2.0, 3.0, 4.0]),
            })
            .unwrap();

        assert_eq!(counter.get(), 1);
    }
}
