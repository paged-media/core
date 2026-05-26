//! `Operation` — the single typed primitive every committed mutation
//! flows through. The five variants match the scripting-layer briefing
//! (`docs/verso/scripting-layer.md`): `SetProperty`, `InsertNode`,
//! `RemoveNode`, `MoveNode`, `Batch`. Extensions require deliberation.
//!
//! Every Operation is `Serialize`/`Deserialize` so the same value can
//! cross the WASM/JS boundary, persist into an operation log, or
//! travel over a wire for future collaboration without changing shape.
//!
//! Note on `Value`: this is the *wire-format payload of a `SetProperty`
//! Op*, not the scene-graph `Value<T>` literal-or-binding scaffold in
//! [`idml_scene::Value`]. The two compose — a SetProperty whose value
//! is a `Computed { ... }` binding will encode that intent here and
//! the scene-graph property cell will lift it into its `Value<T>`
//! variant at apply time. For Stage 1 only literal values exist.

use serde::{Deserialize, Serialize};

/// Stable identifier for a scene-graph node. The string payload is the
/// IDML `Self` attribute (e.g. `"TextFrame/u14"`) — stable for the
/// lifetime of the document. Operations reference nodes by ID, never
/// by path or index, so an Op generated on one client applies
/// meaningfully on another even after the tree has shuffled.
///
/// Variants today cover the page-item kinds the inspector mutates plus
/// the structural containers an `InsertNode`/`MoveNode` Op can target
/// as a parent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id")]
pub enum NodeId {
    // Page items.
    TextFrame(String),
    Rectangle(String),
    Oval(String),
    Polygon(String),
    GraphicLine(String),
    Group(String),
    // Structural parents — addressable so InsertNode / MoveNode can
    // name where a node lands.
    Spread(String),
    Page(String),
}

impl NodeId {
    /// Returns the IDML `Self` string for any variant.
    pub fn self_id(&self) -> &str {
        match self {
            NodeId::TextFrame(s)
            | NodeId::Rectangle(s)
            | NodeId::Oval(s)
            | NodeId::Polygon(s)
            | NodeId::GraphicLine(s)
            | NodeId::Group(s)
            | NodeId::Spread(s)
            | NodeId::Page(s) => s,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            NodeId::TextFrame(_) => "TextFrame",
            NodeId::Rectangle(_) => "Rectangle",
            NodeId::Oval(_) => "Oval",
            NodeId::Polygon(_) => "Polygon",
            NodeId::GraphicLine(_) => "GraphicLine",
            NodeId::Group(_) => "Group",
            NodeId::Spread(_) => "Spread",
            NodeId::Page(_) => "Page",
        }
    }
}

/// Typed property path for `SetProperty` Ops. A closed enum (rather
/// than free-form `Vec<String>`) preserves Rust's exhaustiveness
/// guarantee inside `apply`/`invert`, and the `serde` rename lets the
/// wire format read like the dotted path the briefing illustrates
/// (`"fill.color"`) — so JS callers don't need to learn the Rust
/// enum shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyPath {
    /// Frame geometric bounds: `[top, left, bottom, right]`.
    FrameBounds,
    /// Frame fill-colour reference (a swatch self_id, e.g.
    /// `"Color/Red"`). `None` ⇒ no fill.
    FrameFillColor,
    /// Frame stroke-colour reference (analogous to fill).
    FrameStrokeColor,
    /// Frame stroke weight in points. `None` ⇒ inherit document default
    /// (typically 1pt). Setting to a non-None value pins the per-frame
    /// override.
    FrameStrokeWeight,
    /// Frame opacity percent (0..=100). `None` ⇒ inherit document
    /// default (100% fully opaque). Stored as a plain `f32` in
    /// `Length`-tagged `Value` since IDML carries the value in `%`
    /// units already.
    FrameOpacity,
}

impl PropertyPath {
    /// Human-friendly label for diagnostics + descriptor surfaces.
    pub fn label(&self) -> &'static str {
        match self {
            PropertyPath::FrameBounds => "frame.bounds",
            PropertyPath::FrameFillColor => "frame.fillColor",
            PropertyPath::FrameStrokeColor => "frame.strokeColor",
            PropertyPath::FrameStrokeWeight => "frame.strokeWeight",
            PropertyPath::FrameOpacity => "frame.opacity",
        }
    }
}

/// Typed payload for a `SetProperty` Op. Each variant carries a value
/// of a specific kind; the apply layer's `TypeMismatch` error fires if
/// the variant doesn't match what the path expects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum Value {
    Bounds([f32; 4]),
    ColorRef(Option<String>),
    /// Inspector M1 Phase A: a single floating-point number with an
    /// implicit unit (the property's documentation says which — pt
    /// for stroke weight, % for opacity, etc.). `None` represents
    /// "unset / inherit document default" on properties that allow
    /// the absence; a present `Some(_)` is a per-frame override.
    Length(Option<f32>),
}

/// Description of a node about to be inserted. Carries the minimal
/// Stage-1 supported field set — `RemoveNode` → undo → re-insertion
/// round-trips these reliably. Non-essential fields (item_transform,
/// drop_shadow, anchors, …) default on re-insertion; this is a known
/// Stage 1 limitation flagged in the plan and will tighten in later
/// stages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NodeSpec {
    TextFrame {
        self_id: String,
        bounds: [f32; 4],
        #[serde(default)]
        fill_color: Option<String>,
    },
    Rectangle {
        self_id: String,
        bounds: [f32; 4],
        #[serde(default)]
        fill_color: Option<String>,
    },
}

impl NodeSpec {
    pub fn node_id(&self) -> NodeId {
        match self {
            NodeSpec::TextFrame { self_id, .. } => NodeId::TextFrame(self_id.clone()),
            NodeSpec::Rectangle { self_id, .. } => NodeId::Rectangle(self_id.clone()),
        }
    }
}

/// The canonical mutation primitive. Five variants, closed set,
/// extended only with deliberation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Operation {
    SetProperty {
        node: NodeId,
        path: PropertyPath,
        value: Value,
    },
    InsertNode {
        parent: NodeId,
        position: usize,
        node: NodeSpec,
    },
    RemoveNode {
        node: NodeId,
    },
    MoveNode {
        node: NodeId,
        new_parent: NodeId,
        position: usize,
    },
    Batch {
        ops: Vec<Operation>,
    },
}

/// Hint to downstream caches about what the apply touched. Lists
/// instead of a single enum so a Batch aggregates by union without
/// losing per-node detail. Consumers (renderer, glyph cache, layout
/// cache) decide which lists to honour. Stays advisory — nothing in
/// `idml-mutate` invalidates anything itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidationHint {
    pub frame_geometry: Vec<NodeId>,
    pub frame_style: Vec<NodeId>,
    pub text_reflow: Vec<NodeId>,
    /// Set when the tree shape changed (any Insert/Remove/Move).
    pub structural: bool,
}

impl InvalidationHint {
    pub fn merge(&mut self, other: InvalidationHint) {
        self.frame_geometry.extend(other.frame_geometry);
        self.frame_style.extend(other.frame_style);
        self.text_reflow.extend(other.text_reflow);
        self.structural |= other.structural;
    }
}

/// Result of a successful `apply`. Holds the original op, the
/// pre-computed inverse op (ready to push onto an undo stack), and
/// the invalidation hint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedOperation {
    pub op: Operation,
    pub inverse: Operation,
    pub invalidation: InvalidationHint,
}
