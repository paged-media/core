//! Typed property descriptors that the inspector's properties pane
//! renders against. Each descriptor names a property on a node,
//! carries its authored value and its post-cascade computed value,
//! and labels both the value kind (drives widget rendering) and the
//! authoring source (drives "inherited from" UI affordances).

use idml_mutate::{NodeId, PropertyPath, Value};
use idml_scene::Document;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct PropertyDescriptor {
    pub path: PropertyPathJson,
    pub label: String,
    pub kind: PropertyKind,
    pub authored: AuthoredValue,
    pub computed: ComputedValue,
    pub source: PropertySource,
    pub settable: bool,
}

/// JSON mirror of `idml_mutate::PropertyPath`. Same rationale as
/// `NodeIdJson` — the wire format stays stable as new property
/// paths land.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyPathJson {
    FrameBounds,
    FrameFillColor,
}

impl From<PropertyPath> for PropertyPathJson {
    fn from(value: PropertyPath) -> Self {
        match value {
            PropertyPath::FrameBounds => PropertyPathJson::FrameBounds,
            PropertyPath::FrameFillColor => PropertyPathJson::FrameFillColor,
        }
    }
}

impl From<PropertyPathJson> for PropertyPath {
    fn from(value: PropertyPathJson) -> Self {
        match value {
            PropertyPathJson::FrameBounds => PropertyPath::FrameBounds,
            PropertyPathJson::FrameFillColor => PropertyPath::FrameFillColor,
        }
    }
}

/// Drives widget rendering in the React app. Each variant says
/// "render this with the *Color* picker / *Length* input / ..."
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyKind {
    Bounds,
    Length,
    Color,
    Text,
    Bool,
    Enum,
}

/// JSON form of a property's authored value. Mirrors
/// `idml_mutate::Value`; serialises so JS can read without learning
/// the Rust enum shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum AuthoredValue {
    Bounds([f32; 4]),
    ColorRef(Option<String>),
    Length(f32),
    Text(String),
    Bool(bool),
    Enum(String),
    None,
}

pub type ComputedValue = AuthoredValue;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "name")]
pub enum PropertySource {
    Local,
    InheritedFrom(String),
    Default,
}

impl From<Value> for AuthoredValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Bounds(b) => AuthoredValue::Bounds(b),
            Value::ColorRef(c) => AuthoredValue::ColorRef(c),
        }
    }
}

pub fn describe(document: &Document, node: &NodeId) -> Vec<PropertyDescriptor> {
    match node {
        NodeId::TextFrame(self_id) => describe_text_frame(document, self_id),
        NodeId::Rectangle(self_id) => describe_rectangle(document, self_id),
        _ => Vec::new(),
    }
}

fn describe_text_frame(document: &Document, self_id: &str) -> Vec<PropertyDescriptor> {
    let Some(frame) = find_text_frame(document, self_id) else {
        return Vec::new();
    };
    vec![
        PropertyDescriptor {
            path: PropertyPathJson::FrameBounds,
            label: "Bounds (pt)".to_string(),
            kind: PropertyKind::Bounds,
            authored: AuthoredValue::Bounds([
                frame.bounds.top,
                frame.bounds.left,
                frame.bounds.bottom,
                frame.bounds.right,
            ]),
            computed: AuthoredValue::Bounds([
                frame.bounds.top,
                frame.bounds.left,
                frame.bounds.bottom,
                frame.bounds.right,
            ]),
            source: PropertySource::Local,
            settable: true,
        },
        PropertyDescriptor {
            path: PropertyPathJson::FrameFillColor,
            label: "Fill color".to_string(),
            kind: PropertyKind::Color,
            authored: AuthoredValue::ColorRef(frame.fill_color.clone()),
            computed: AuthoredValue::ColorRef(frame.fill_color.clone()),
            // TODO: when ObjectStyle resolution lands, surface
            // InheritedFrom(style_name) for properties carried by an
            // AppliedObjectStyle rather than the per-frame attribute.
            source: if frame.fill_color.is_some() {
                PropertySource::Local
            } else {
                PropertySource::Default
            },
            settable: true,
        },
    ]
}

fn describe_rectangle(document: &Document, self_id: &str) -> Vec<PropertyDescriptor> {
    let Some(rect) = find_rectangle(document, self_id) else {
        return Vec::new();
    };
    vec![
        PropertyDescriptor {
            path: PropertyPathJson::FrameBounds,
            label: "Bounds (pt)".to_string(),
            kind: PropertyKind::Bounds,
            authored: AuthoredValue::Bounds([
                rect.bounds.top,
                rect.bounds.left,
                rect.bounds.bottom,
                rect.bounds.right,
            ]),
            computed: AuthoredValue::Bounds([
                rect.bounds.top,
                rect.bounds.left,
                rect.bounds.bottom,
                rect.bounds.right,
            ]),
            source: PropertySource::Local,
            settable: true,
        },
        PropertyDescriptor {
            path: PropertyPathJson::FrameFillColor,
            label: "Fill color".to_string(),
            kind: PropertyKind::Color,
            authored: AuthoredValue::ColorRef(rect.fill_color.clone()),
            computed: AuthoredValue::ColorRef(rect.fill_color.clone()),
            source: if rect.fill_color.is_some() {
                PropertySource::Local
            } else {
                PropertySource::Default
            },
            settable: true,
        },
    ]
}

fn find_text_frame<'a>(
    document: &'a Document,
    self_id: &str,
) -> Option<&'a idml_parse::TextFrame> {
    document
        .spreads
        .iter()
        .flat_map(|s| &s.spread.text_frames)
        .find(|f| f.self_id.as_deref() == Some(self_id))
}

fn find_rectangle<'a>(
    document: &'a Document,
    self_id: &str,
) -> Option<&'a idml_parse::Rectangle> {
    document
        .spreads
        .iter()
        .flat_map(|s| &s.spread.rectangles)
        .find(|r| r.self_id.as_deref() == Some(self_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::document_with_one_textframe;

    #[test]
    fn describe_text_frame_lists_bounds_and_fill_color() {
        let doc = document_with_one_textframe("TextFrame/u1");
        let descs = describe(&doc, &NodeId::TextFrame("TextFrame/u1".to_string()));
        assert_eq!(descs.len(), 2);
        assert!(matches!(descs[0].kind, PropertyKind::Bounds));
        assert!(matches!(descs[1].kind, PropertyKind::Color));
    }
}
