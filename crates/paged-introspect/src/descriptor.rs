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
    pub path: PropertyPathJson,
    pub label: String,
    pub kind: PropertyKind,
    pub authored: AuthoredValue,
    pub computed: ComputedValue,
    pub source: PropertySource,
    pub settable: bool,
}

/// JSON mirror of `paged_mutate::PropertyPath`. Same rationale as
/// `NodeIdJson` — the wire format stays stable as new property
/// paths land. Kept in 1:1 sync with `PropertyPath`; the two `From`
/// impls below stay exhaustive, so a new `PropertyPath` variant fails
/// to compile here until it is mirrored.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyPathJson {
    FrameBounds,
    FrameFillColor,
    FrameStrokeColor,
    FrameStrokeWeight,
    FrameOpacity,
    FrameTransform,
    ImageContentTransform,
    FramePathPoint,
    PathPointInsert,
    PathPointRemove,
    PathPointCurveType,
    LayerVisible,
    LayerLocked,
    LayerPrintable,
    LayerName,
    CharacterFontSize,
    CharacterLeading,
    CharacterTracking,
    CharacterFillColor,
    ParagraphSpaceBefore,
    ParagraphSpaceAfter,
    ParagraphFirstLineIndent,
    AppliedParagraphStyle,
    AppliedCharacterStyle,
    AppliedObjectStyle,
    AppliedCellStyle,
    AppliedTableStyle,
    FramePath,
    FrameNonprinting,
    FrameFillTint,
    FrameDropShadowMode,
    FrameDropShadowXOffset,
    FrameDropShadowYOffset,
    FrameDropShadowSize,
    FrameDropShadowOpacity,
    FrameDropShadowColor,
    FrameDropShadow,
    FrameFittingCrops,
    FrameFittingType,
    FrameTextWrapMode,
    FrameTextWrapOffsets,
    ParagraphJustification,
    FrameStrokeEndCap,
    FrameInsetSpacing,
    AppliedConditions,
}

impl From<PropertyPath> for PropertyPathJson {
    fn from(value: PropertyPath) -> Self {
        match value {
            PropertyPath::FrameBounds => PropertyPathJson::FrameBounds,
            PropertyPath::FrameFillColor => PropertyPathJson::FrameFillColor,
            PropertyPath::FrameStrokeColor => PropertyPathJson::FrameStrokeColor,
            PropertyPath::FrameStrokeWeight => PropertyPathJson::FrameStrokeWeight,
            PropertyPath::FrameOpacity => PropertyPathJson::FrameOpacity,
            PropertyPath::FrameTransform => PropertyPathJson::FrameTransform,
            PropertyPath::ImageContentTransform => PropertyPathJson::ImageContentTransform,
            PropertyPath::FramePathPoint => PropertyPathJson::FramePathPoint,
            PropertyPath::PathPointInsert => PropertyPathJson::PathPointInsert,
            PropertyPath::PathPointRemove => PropertyPathJson::PathPointRemove,
            PropertyPath::PathPointCurveType => PropertyPathJson::PathPointCurveType,
            PropertyPath::LayerVisible => PropertyPathJson::LayerVisible,
            PropertyPath::LayerLocked => PropertyPathJson::LayerLocked,
            PropertyPath::LayerPrintable => PropertyPathJson::LayerPrintable,
            PropertyPath::LayerName => PropertyPathJson::LayerName,
            PropertyPath::CharacterFontSize => PropertyPathJson::CharacterFontSize,
            PropertyPath::CharacterLeading => PropertyPathJson::CharacterLeading,
            PropertyPath::CharacterTracking => PropertyPathJson::CharacterTracking,
            PropertyPath::CharacterFillColor => PropertyPathJson::CharacterFillColor,
            PropertyPath::ParagraphSpaceBefore => PropertyPathJson::ParagraphSpaceBefore,
            PropertyPath::ParagraphSpaceAfter => PropertyPathJson::ParagraphSpaceAfter,
            PropertyPath::ParagraphFirstLineIndent => PropertyPathJson::ParagraphFirstLineIndent,
            PropertyPath::AppliedParagraphStyle => PropertyPathJson::AppliedParagraphStyle,
            PropertyPath::AppliedCharacterStyle => PropertyPathJson::AppliedCharacterStyle,
            PropertyPath::AppliedObjectStyle => PropertyPathJson::AppliedObjectStyle,
            PropertyPath::AppliedCellStyle => PropertyPathJson::AppliedCellStyle,
            PropertyPath::AppliedTableStyle => PropertyPathJson::AppliedTableStyle,
            PropertyPath::FramePath => PropertyPathJson::FramePath,
            PropertyPath::FrameNonprinting => PropertyPathJson::FrameNonprinting,
            PropertyPath::FrameFillTint => PropertyPathJson::FrameFillTint,
            PropertyPath::FrameDropShadowMode => PropertyPathJson::FrameDropShadowMode,
            PropertyPath::FrameDropShadowXOffset => PropertyPathJson::FrameDropShadowXOffset,
            PropertyPath::FrameDropShadowYOffset => PropertyPathJson::FrameDropShadowYOffset,
            PropertyPath::FrameDropShadowSize => PropertyPathJson::FrameDropShadowSize,
            PropertyPath::FrameDropShadowOpacity => PropertyPathJson::FrameDropShadowOpacity,
            PropertyPath::FrameDropShadowColor => PropertyPathJson::FrameDropShadowColor,
            PropertyPath::FrameDropShadow => PropertyPathJson::FrameDropShadow,
            PropertyPath::FrameFittingCrops => PropertyPathJson::FrameFittingCrops,
            PropertyPath::FrameFittingType => PropertyPathJson::FrameFittingType,
            PropertyPath::FrameTextWrapMode => PropertyPathJson::FrameTextWrapMode,
            PropertyPath::FrameTextWrapOffsets => PropertyPathJson::FrameTextWrapOffsets,
            PropertyPath::ParagraphJustification => PropertyPathJson::ParagraphJustification,
            PropertyPath::FrameStrokeEndCap => PropertyPathJson::FrameStrokeEndCap,
            PropertyPath::FrameInsetSpacing => PropertyPathJson::FrameInsetSpacing,
            PropertyPath::AppliedConditions => PropertyPathJson::AppliedConditions,
        }
    }
}

impl From<PropertyPathJson> for PropertyPath {
    fn from(value: PropertyPathJson) -> Self {
        match value {
            PropertyPathJson::FrameBounds => PropertyPath::FrameBounds,
            PropertyPathJson::FrameFillColor => PropertyPath::FrameFillColor,
            PropertyPathJson::FrameStrokeColor => PropertyPath::FrameStrokeColor,
            PropertyPathJson::FrameStrokeWeight => PropertyPath::FrameStrokeWeight,
            PropertyPathJson::FrameOpacity => PropertyPath::FrameOpacity,
            PropertyPathJson::FrameTransform => PropertyPath::FrameTransform,
            PropertyPathJson::ImageContentTransform => PropertyPath::ImageContentTransform,
            PropertyPathJson::FramePathPoint => PropertyPath::FramePathPoint,
            PropertyPathJson::PathPointInsert => PropertyPath::PathPointInsert,
            PropertyPathJson::PathPointRemove => PropertyPath::PathPointRemove,
            PropertyPathJson::PathPointCurveType => PropertyPath::PathPointCurveType,
            PropertyPathJson::LayerVisible => PropertyPath::LayerVisible,
            PropertyPathJson::LayerLocked => PropertyPath::LayerLocked,
            PropertyPathJson::LayerPrintable => PropertyPath::LayerPrintable,
            PropertyPathJson::LayerName => PropertyPath::LayerName,
            PropertyPathJson::CharacterFontSize => PropertyPath::CharacterFontSize,
            PropertyPathJson::CharacterLeading => PropertyPath::CharacterLeading,
            PropertyPathJson::CharacterTracking => PropertyPath::CharacterTracking,
            PropertyPathJson::CharacterFillColor => PropertyPath::CharacterFillColor,
            PropertyPathJson::ParagraphSpaceBefore => PropertyPath::ParagraphSpaceBefore,
            PropertyPathJson::ParagraphSpaceAfter => PropertyPath::ParagraphSpaceAfter,
            PropertyPathJson::ParagraphFirstLineIndent => PropertyPath::ParagraphFirstLineIndent,
            PropertyPathJson::AppliedParagraphStyle => PropertyPath::AppliedParagraphStyle,
            PropertyPathJson::AppliedCharacterStyle => PropertyPath::AppliedCharacterStyle,
            PropertyPathJson::AppliedObjectStyle => PropertyPath::AppliedObjectStyle,
            PropertyPathJson::AppliedCellStyle => PropertyPath::AppliedCellStyle,
            PropertyPathJson::AppliedTableStyle => PropertyPath::AppliedTableStyle,
            PropertyPathJson::FramePath => PropertyPath::FramePath,
            PropertyPathJson::FrameNonprinting => PropertyPath::FrameNonprinting,
            PropertyPathJson::FrameFillTint => PropertyPath::FrameFillTint,
            PropertyPathJson::FrameDropShadowMode => PropertyPath::FrameDropShadowMode,
            PropertyPathJson::FrameDropShadowXOffset => PropertyPath::FrameDropShadowXOffset,
            PropertyPathJson::FrameDropShadowYOffset => PropertyPath::FrameDropShadowYOffset,
            PropertyPathJson::FrameDropShadowSize => PropertyPath::FrameDropShadowSize,
            PropertyPathJson::FrameDropShadowOpacity => PropertyPath::FrameDropShadowOpacity,
            PropertyPathJson::FrameDropShadowColor => PropertyPath::FrameDropShadowColor,
            PropertyPathJson::FrameDropShadow => PropertyPath::FrameDropShadow,
            PropertyPathJson::FrameFittingCrops => PropertyPath::FrameFittingCrops,
            PropertyPathJson::FrameFittingType => PropertyPath::FrameFittingType,
            PropertyPathJson::FrameTextWrapMode => PropertyPath::FrameTextWrapMode,
            PropertyPathJson::FrameTextWrapOffsets => PropertyPath::FrameTextWrapOffsets,
            PropertyPathJson::ParagraphJustification => PropertyPath::ParagraphJustification,
            PropertyPathJson::FrameStrokeEndCap => PropertyPath::FrameStrokeEndCap,
            PropertyPathJson::FrameInsetSpacing => PropertyPath::FrameInsetSpacing,
            PropertyPathJson::AppliedConditions => PropertyPath::AppliedConditions,
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
            Value::Transform(_)
            | Value::PathPoint { .. }
            | Value::PathPointInsert { .. }
            | Value::PathPointRemove { .. }
            | Value::PathPointCurveType { .. }
            | Value::FramePath { .. } => AuthoredValue::None,
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
) -> Option<&'a paged_parse::TextFrame> {
    document
        .spreads
        .iter()
        .flat_map(|s| &s.spread.text_frames)
        .find(|f| f.self_id.as_deref() == Some(self_id))
}

fn find_rectangle<'a>(
    document: &'a Document,
    self_id: &str,
) -> Option<&'a paged_parse::Rectangle> {
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
