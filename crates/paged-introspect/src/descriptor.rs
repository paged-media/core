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

//! Typed property descriptors that the inspector's properties pane
//! renders against. Each descriptor names a property on a node,
//! carries its authored value and its post-cascade computed value,
//! and labels both the value kind (drives widget rendering) and the
//! authoring source (drives "inherited from" UI affordances).

use paged_mutate::{NodeId, PropertyPath, Value};
use paged_scene::Document;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct PropertyDescriptor {
    /// The path this descriptor describes.
    ///
    /// This was a 217-variant `PropertyPathJson` mirror with two
    /// exhaustive 217-arm `From` impls — 785 lines whose stated purpose
    /// was to let "the wire format stay stable as new property paths
    /// land". It could not: every variant name was identical and both
    /// types derived `rename_all = "camelCase"`, so the two serialised
    /// to byte-identical JSON. Measured across all 217 before removal,
    /// zero differed. What the mirror bought was the obligation to edit
    /// two enums for one capability.
    ///
    /// `PropertyPath` is itself `Serialize`, so the emitted JSON is
    /// unchanged. The name it emits is the raw-wire spelling, which for
    /// eight paths differs from the advertised one —
    /// `catalog::wire_alias` publishes those and `lookup_path` accepts
    /// both, so a name read off a descriptor can be written back.
    pub path: PropertyPath,
    pub label: String,
    pub kind: PropertyKind,
    pub authored: AuthoredValue,
    pub computed: ComputedValue,
    pub source: PropertySource,
    pub settable: bool,
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
/// `paged_mutate::Value`; serialises so JS can read without learning
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
            Value::Length(Some(n)) => AuthoredValue::Length(n),
            Value::Length(None) => AuthoredValue::None,
            Value::Bool(b) => AuthoredValue::Bool(b),
            Value::Text(s) => AuthoredValue::Text(s),
            // Structural / path-edit payloads (affine transforms, path-point
            // edits, whole-path replacement) have no scalar authored-value
            // form yet — no `PropertyKind` widget renders them, and
            // `describe()` never emits descriptors for those paths — so they
            // collapse to `None` for this exhaustive conversion.
            Value::PluginMetadata { .. } => AuthoredValue::None,
            Value::Transform(_)
            | Value::PathPoint { .. }
            | Value::PathPointInsert { .. }
            | Value::PathPointRemove { .. }
            | Value::PathPointCurveType { .. }
            | Value::FramePath { .. }
            | Value::PathOpenAt { .. }
            | Value::OutlineStroke { .. }
            | Value::OutlineStrokeVariable { .. }
            | Value::OffsetPath { .. }
            | Value::SimplifyPath { .. }
            | Value::ClosePath { .. }
            | Value::GradientFeather(_)
            // W0.2 — whole-struct / whole-list paragraph payloads,
            // like the gradient-feather struct: no scalar
            // authored-value widget renders them, so they collapse
            // to `None` for this exhaustive conversion.
            | Value::ParagraphRule(_)
            | Value::TabStops(_)
            // W1.1 — the per-frame dash-array list (`FrameStrokeDashArray`).
            // Like `TabStops`, no scalar authored-value widget renders a
            // length list, so it collapses to `None` here.
            | Value::Lengths(_) => AuthoredValue::None,
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
            path: PropertyPath::FrameBounds,
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
            path: PropertyPath::FrameFillColor,
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
            path: PropertyPath::FrameBounds,
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
            path: PropertyPath::FrameFillColor,
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
) -> Option<&'a paged_model::TextFrame> {
    document
        .spreads
        .iter()
        .flat_map(|s| &s.spread.text_frames)
        .find(|f| f.self_id.as_deref() == Some(self_id))
}

fn find_rectangle<'a>(document: &'a Document, self_id: &str) -> Option<&'a paged_model::Rectangle> {
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
