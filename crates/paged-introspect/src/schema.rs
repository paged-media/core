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

//! Typed value schemas for every settable thing.
//!
//! A `PropertyPath` used to say only WHERE a value goes. What the value
//! IS — a length in points, a percentage, an enum with eight members, a
//! swatch reference — lived in three hand lists that agreed by accident:
//! the Boa bridge's string-variant match, the catalog's `type_hint`, and
//! the apply arms' `expect_*` calls. A binding layer, a schema-driven
//! panel field or a CLI that wants to coerce `"12mm"` cannot work from a
//! name.
//!
//! [`PropertySchema`] is that missing half. Every path declares one in
//! the `property_paths!` table (`catalog.rs`) — the macro makes a path
//! without a schema a compile error — and every virtual path declares one
//! in [`crate::virtual_paths`]. The catalog publishes them as
//! `settablePaths[].schema` / `virtualPaths[].schema`.
//!
//! `wire` is the load-bearing field: the [`paged_mutate::Value`] variant
//! the apply layer accepts for the path. [`coerce`] uses it, so a surface
//! that holds a plain JSON number or string can build the right `Value`
//! without its own per-path table (`schema_wire_matches_apply` proves the
//! declared form is the one apply takes).

use paged_mutate::Value;
use serde::Serialize;

/// What a value means, independent of how it travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ValueType {
    Bool,
    /// A plain number (percent, degrees, a count, a scale factor).
    Number,
    /// A length in points.
    Length,
    /// A swatch reference (`Color/<id>`, `Swatch/None`, a gradient).
    Color,
    /// One of `members`.
    Enum,
    Text,
    /// `[top, left, bottom, right]` in points.
    Bounds,
    /// `[a, b, c, d, tx, ty]`.
    Transform,
    /// A reference to another document object (`refKind` says which).
    Ref,
    /// A homogeneous list (`item` says of what).
    List,
    /// A whole-struct value (`shape` names the wire struct).
    Struct,
    /// Not a property at all: a path OPERATION carried as a
    /// `PropertyPath` for the apply/invert algebra.
    Operation,
}

/// The unit a number is measured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unit {
    Pt,
    Percent,
    Degrees,
    Count,
}

/// What a `Ref` points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefKind {
    Swatch,
    ParagraphStyle,
    CharacterStyle,
    ObjectStyle,
    CellStyle,
    TableStyle,
    Layer,
    NumberingList,
    Condition,
    Master,
    Font,
    Language,
    Uri,
}

/// The `paged_mutate::Value` variant the apply layer takes, spelled as
/// its serde tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Wire {
    Bool,
    Length,
    Text,
    ColorRef,
    Bounds,
    Transform,
    Lengths,
    TabStops,
    ParagraphRule,
    GradientFeather,
    GradientStops,
    FramePath,
    PathPoint,
    PathPointInsert,
    PathPointRemove,
    PathPointCurveType,
    PathOpenAt,
    OutlineStroke,
    OutlineStrokeVariable,
    OffsetPath,
    SimplifyPath,
    ClosePath,
    PluginMetadata,
}

/// A schema default. Untagged, so it serialises as the bare value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(untagged)]
pub enum DefaultValue {
    Bool(bool),
    Number(f64),
    Text(&'static str),
}

/// The typed description of one settable value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertySchema {
    #[serde(rename = "type")]
    pub value_type: ValueType,
    pub wire: Wire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<Unit>,
    /// Enum members, in IDML spelling. Empty for non-enums.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub members: &'static [&'static str],
    /// `true` when `members` are the KNOWN values but the engine stores
    /// any string (an open vocabulary: blend modes InDesign may extend,
    /// language names). Coercion does not reject outside an open enum.
    #[serde(skip_serializing_if = "is_false")]
    pub open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_kind: Option<RefKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<ValueType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<DefaultValue>,
    /// `null` (or `""` for text) clears the override back to the
    /// inherited value.
    pub nullable: bool,
    /// Readable, never writable (`paged.get` answers, `paged.set` refuses).
    #[serde(skip_serializing_if = "is_false")]
    pub read_only: bool,
    /// Computed from other state rather than stored.
    #[serde(skip_serializing_if = "is_false")]
    pub derived: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

const BASE: PropertySchema = PropertySchema {
    value_type: ValueType::Text,
    wire: Wire::Text,
    unit: None,
    members: &[],
    open: false,
    ref_kind: None,
    item: None,
    shape: None,
    min: None,
    max: None,
    default: None,
    nullable: true,
    read_only: false,
    derived: false,
};

// ---- constructors ---------------------------------------------------

pub const fn bool_() -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Bool,
        wire: Wire::Bool,
        nullable: false,
        ..BASE
    }
}

/// A length in points (travels as `Value::Length`).
pub const fn length() -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Length,
        wire: Wire::Length,
        unit: Some(Unit::Pt),
        ..BASE
    }
}

/// A non-negative length.
pub const fn length_pos() -> PropertySchema {
    length().min(0.0)
}

/// 0–100 %, default 100 (opacity, tint).
pub const fn pct() -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Number,
        wire: Wire::Length,
        unit: Some(Unit::Percent),
        min: Some(0.0),
        max: Some(100.0),
        ..BASE
    }
}

/// Degrees, -360..360.
pub const fn deg() -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Number,
        wire: Wire::Length,
        unit: Some(Unit::Degrees),
        min: Some(-360.0),
        max: Some(360.0),
        ..BASE
    }
}

/// A whole count.
pub const fn count() -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Number,
        wire: Wire::Length,
        unit: Some(Unit::Count),
        min: Some(0.0),
        ..BASE
    }
}

/// A unitless number.
pub const fn number() -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Number,
        wire: Wire::Length,
        ..BASE
    }
}

/// A swatch reference.
pub const fn color() -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Color,
        wire: Wire::ColorRef,
        ref_kind: Some(RefKind::Swatch),
        ..BASE
    }
}

/// A swatch reference carried as `Value::Text` (style definitions and
/// the virtual paths whose ops take a plain id string).
pub const fn color_text() -> PropertySchema {
    PropertySchema {
        wire: Wire::Text,
        ..color()
    }
}

/// A closed enum: coercion rejects anything outside `members` (and `""`,
/// which clears, when nullable).
pub const fn enum_(members: &'static [&'static str]) -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Enum,
        wire: Wire::Text,
        members,
        ..BASE
    }
}

/// An open enum: `members` are the known values, others are stored.
pub const fn enum_open(members: &'static [&'static str]) -> PropertySchema {
    PropertySchema {
        open: true,
        ..enum_(members)
    }
}

pub const fn text() -> PropertySchema {
    BASE
}

pub const fn reference(kind: RefKind) -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Ref,
        ref_kind: Some(kind),
        ..BASE
    }
}

pub const fn bounds() -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Bounds,
        wire: Wire::Bounds,
        unit: Some(Unit::Pt),
        nullable: false,
        ..BASE
    }
}

pub const fn transform() -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Transform,
        wire: Wire::Transform,
        ..BASE
    }
}

pub const fn lengths() -> PropertySchema {
    PropertySchema {
        value_type: ValueType::List,
        wire: Wire::Lengths,
        item: Some(ValueType::Length),
        unit: Some(Unit::Pt),
        ..BASE
    }
}

pub const fn structure(shape: &'static str, wire: Wire) -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Struct,
        wire,
        shape: Some(shape),
        ..BASE
    }
}

pub const fn list_of(shape: &'static str, wire: Wire) -> PropertySchema {
    PropertySchema {
        value_type: ValueType::List,
        wire,
        item: Some(ValueType::Struct),
        shape: Some(shape),
        nullable: false,
        ..BASE
    }
}

/// A path OPERATION, not a property.
pub const fn operation(shape: &'static str, wire: Wire) -> PropertySchema {
    PropertySchema {
        value_type: ValueType::Operation,
        wire,
        shape: Some(shape),
        nullable: false,
        ..BASE
    }
}

impl PropertySchema {
    pub const fn min(self, min: f64) -> Self {
        Self {
            min: Some(min),
            ..self
        }
    }
    pub const fn range(self, min: f64, max: f64) -> Self {
        Self {
            min: Some(min),
            max: Some(max),
            ..self
        }
    }
    pub const fn default_num(self, v: f64) -> Self {
        Self {
            default: Some(DefaultValue::Number(v)),
            ..self
        }
    }
    pub const fn default_bool(self, v: bool) -> Self {
        Self {
            default: Some(DefaultValue::Bool(v)),
            ..self
        }
    }
    pub const fn default_text(self, v: &'static str) -> Self {
        Self {
            default: Some(DefaultValue::Text(v)),
            ..self
        }
    }
    pub const fn read_only(self) -> Self {
        Self {
            read_only: true,
            ..self
        }
    }
    pub const fn derived(self) -> Self {
        Self {
            derived: true,
            read_only: true,
            ..self
        }
    }
    pub const fn required(self) -> Self {
        Self {
            nullable: false,
            ..self
        }
    }
    pub const fn wire(self, wire: Wire) -> Self {
        Self { wire, ..self }
    }
    pub const fn of(self, kind: RefKind) -> Self {
        Self {
            ref_kind: Some(kind),
            ..self
        }
    }
}

// ---- shared vocabularies --------------------------------------------

pub const BLEND_MODES: &[&str] = &[
    "Normal",
    "Multiply",
    "Screen",
    "Overlay",
    "SoftLight",
    "HardLight",
    "ColorDodge",
    "ColorBurn",
    "Darken",
    "Lighten",
    "Difference",
    "Exclusion",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];
pub const JUSTIFICATION: &[&str] = &[
    "LeftAlign",
    "CenterAlign",
    "RightAlign",
    "LeftJustified",
    "CenterJustified",
    "RightJustified",
    "FullyJustified",
    "ToBindingSide",
    "AwayFromBindingSide",
];
pub const VERTICAL_JUSTIFICATION: &[&str] =
    &["TopAlign", "CenterAlign", "BottomAlign", "JustifyAlign"];
pub const ARROWHEADS: &[&str] = &[
    "None",
    "SimpleArrowHead",
    "SimpleWideArrowHead",
    "TriangleArrowHead",
    "TriangleWideArrowHead",
    "BarbedArrowHead",
    "CurvedArrowHead",
    "CircleArrowHead",
    "CircleSolidArrowHead",
    "SquareArrowHead",
    "SquareSolidArrowHead",
    "BarArrowHead",
];
pub const START_PARAGRAPH: &[&str] = &[
    "Anywhere",
    "NextColumn",
    "NextFrame",
    "NextPage",
    "NextOddPage",
    "NextEvenPage",
];
pub const SPAN_COLUMN_TYPE: &[&str] = &["SingleColumn", "SpanColumns", "SplitColumns"];
pub const FIRST_BASELINE: &[&str] = &[
    "AscentOffset",
    "CapHeight",
    "XHeight",
    "EmBoxHeight",
    "LeadingOffset",
    "FixedHeight",
];
pub const AUTO_SIZING: &[&str] = &[
    "Off",
    "HeightOnly",
    "WidthOnly",
    "HeightAndWidth",
    "HeightAndWidthProportionally",
];
pub const REFERENCE_POINTS: &[&str] = &[
    "TopLeftPoint",
    "TopCenterPoint",
    "TopRightPoint",
    "CenterLeftPoint",
    "CenterPoint",
    "CenterRightPoint",
    "BottomLeftPoint",
    "BottomCenterPoint",
    "BottomRightPoint",
];
pub const CORNER_OPTIONS: &[&str] = &[
    "None",
    "RoundedCorner",
    "InverseRoundedCorner",
    "InsetCorner",
    "BeveledCorner",
    "FancyCorner",
];
pub const END_CAPS: &[&str] = &["ButtEndCap", "RoundEndCap", "ProjectingEndCap"];
pub const END_JOINS: &[&str] = &["MiterEndJoin", "RoundEndJoin", "BevelEndJoin"];
pub const STROKE_ALIGNMENT: &[&str] = &["CenterAlignment", "InsideAlignment", "OutsideAlignment"];
pub const TEXT_WRAP_MODES: &[&str] = &[
    "None",
    "BoundingBoxTextWrap",
    "Contour",
    "JumpObjectTextWrap",
    "NextColumnTextWrap",
];
pub const CONTOUR_TYPES: &[&str] = &[
    "BoundingBox",
    "DetectEdges",
    "AlphaChannel",
    "PhotoshopPath",
    "GraphicFrame",
    "SameAsClipping",
    "UserModifiedPath",
];
pub const FITTING: &[&str] = &[
    "None",
    "Proportionally",
    "FillProportionally",
    "ContentToFrame",
    "CenterContent",
    "FrameToContent",
];
pub const DROP_SHADOW_MODES: &[&str] = &["None", "Drop"];
pub const CASES: &[&str] = &["Normal", "AllCaps", "SmallCaps", "OTSmallCaps"];
pub const POSITIONS: &[&str] = &[
    "Normal",
    "Superscript",
    "Subscript",
    "OTSuperscript",
    "OTSubscript",
    "OTNumerator",
    "OTDenominator",
];
pub const KERNING_METHODS: &[&str] = &["Metrics", "Optical", "None"];
pub const LIST_TYPES: &[&str] = &["NoList", "BulletList", "NumberedList"];
pub const COMPOSERS: &[&str] = &[
    "HL Composer",
    "HL Single",
    "Adobe Paragraph Composer",
    "Adobe Single-line Composer",
];
pub const GLOW_SOURCES: &[&str] = &["EdgeSourced", "CenterSourced"];
pub const BEVEL_STYLES: &[&str] = &["OuterBevel", "InnerBevel", "Emboss", "PillowEmboss"];
pub const BEVEL_TECHNIQUES: &[&str] = &["SmoothContour", "ChiselHard", "ChiselSoft"];
pub const BEVEL_DIRECTIONS: &[&str] = &["Up", "Down"];
pub const FEATHER_CORNERS: &[&str] = &["Sharp", "Rounded", "Diffusion"];
pub const ANCHORED_POSITIONS: &[&str] = &["InlinePosition", "AboveLine", "Anchored"];
pub const SWATCH_SPACES: &[&str] = &["CMYK", "RGB", "LAB", "Gray"];
pub const SWATCH_MODELS: &[&str] = &["Process", "Spot"];
pub const GRADIENT_KINDS: &[&str] = &["Linear", "Radial"];
pub const PAGE_NUMBER_STYLES: &[&str] = &[
    "Arabic",
    "UpperRoman",
    "LowerRoman",
    "UpperLetters",
    "LowerLetters",
];
pub const TEXT_VARIABLE_TYPES: &[&str] = &[
    "CustomTextType",
    "FileNameType",
    "PageCountType",
    "CreationDateType",
    "ModificationDateType",
    "OutputDateType",
    "ChapterNumberType",
    "MatchParagraphStyleType",
    "MatchCharacterStyleType",
    "LastPageNumberType",
    "XrefPageNumberType",
    "XrefChapterNumberType",
];
pub const ALTERNATING_FILLS: &[&str] = &["None", "AlternatingRows", "AlternatingColumns"];

// ---- coercion -------------------------------------------------------

/// A surface-neutral input value: what a JSON caller (Boa, the CLI, a
/// binding expression) actually holds before it knows the path's wire
/// form.
#[derive(Debug, Clone, PartialEq)]
pub enum Input<'a> {
    Null,
    Bool(bool),
    Number(f64),
    Text(&'a str),
}

/// Build the `Value` the apply layer takes for a plain JSON scalar, or
/// say why the input does not fit the schema. Structured values
/// (`{ type, value }`, arrays) are the caller's to decode; this covers
/// the scalars, which is where the per-surface tables lived.
///
/// Rules:
/// - `null` clears a nullable path (`Length(None)` / `ColorRef(None)` /
///   `Text("")` by wire form); a required path refuses it.
/// - a number on a numeric path is range-checked and travels in the
///   path's wire form (`Length`), so a percentage is no longer
///   indistinguishable from a length at the bridge — the schema says;
/// - a string on a closed enum must be a member (or `""` to clear);
/// - a bool only fits a bool path, and a bool path takes only a bool.
pub fn coerce(schema: &PropertySchema, input: Input<'_>) -> Result<Value, String> {
    if schema.read_only {
        return Err("read-only".to_string());
    }
    match input {
        Input::Null => {
            if !schema.nullable {
                return Err("null is not accepted: the value is required".to_string());
            }
            Ok(match schema.wire {
                Wire::Length => Value::Length(None),
                Wire::ColorRef => Value::ColorRef(None),
                Wire::Transform => Value::Transform(None),
                Wire::Text => Value::Text(String::new()),
                Wire::Lengths => Value::Lengths(Vec::new()),
                Wire::TabStops => Value::TabStops(Vec::new()),
                Wire::ParagraphRule => Value::ParagraphRule(None),
                Wire::GradientFeather => Value::GradientFeather(None),
                other => return Err(format!("null has no {other:?} form")),
            })
        }
        Input::Bool(b) => match schema.wire {
            Wire::Bool => Ok(Value::Bool(b)),
            _ => Err(format!("expected {:?}, got a boolean", schema.value_type)),
        },
        Input::Number(n) => {
            if !n.is_finite() {
                return Err("expected a finite number".to_string());
            }
            if let Some(min) = schema.min {
                if n < min {
                    return Err(format!("{n} is below the minimum {min}"));
                }
            }
            if let Some(max) = schema.max {
                if n > max {
                    return Err(format!("{n} is above the maximum {max}"));
                }
            }
            match schema.wire {
                Wire::Length => Ok(Value::Length(Some(n as f32))),
                // A number where the engine takes a string: the counts
                // that also accept a keyword (`"All"`) and plain text.
                Wire::Text if matches!(schema.value_type, ValueType::Text | ValueType::Number) => {
                    Ok(Value::Text(format_number(n)))
                }
                _ => Err(format!("expected {:?}, got a number", schema.value_type)),
            }
        }
        Input::Text(s) => match schema.wire {
            Wire::ColorRef => Ok(Value::ColorRef(if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            })),
            Wire::Text => {
                if schema.value_type == ValueType::Enum
                    && !schema.open
                    && !s.is_empty()
                    && !schema.members.contains(&s)
                {
                    return Err(format!(
                        "{s:?} is not one of {}",
                        schema.members.join(" | ")
                    ));
                }
                if s.is_empty() && !schema.nullable {
                    return Err("an empty string is not accepted: the value is required".into());
                }
                Ok(Value::Text(s.to_string()))
            }
            _ => Err(format!("expected {:?}, got a string", schema.value_type)),
        },
    }
}

/// Check a fully-formed wire value against a schema: the value's variant
/// must be the schema's wire form (a typed `Number` / `Enum` normalises
/// first), and scalar contents must satisfy range and membership.
/// Returns the NORMALISED value the apply layer takes.
pub fn validate(schema: &PropertySchema, value: &Value) -> Result<Value, String> {
    if schema.read_only {
        return Err("read-only".to_string());
    }
    match value {
        Value::Number(n) => coerce(schema, Input::Number(*n)),
        Value::Enum(s) | Value::Ref(s) | Value::Text(s) if schema.wire == Wire::Text => {
            coerce(schema, Input::Text(s))
        }
        Value::Enum(s) | Value::Ref(s) => coerce(schema, Input::Text(s)),
        Value::Length(Some(n)) if schema.wire == Wire::Length => {
            coerce(schema, Input::Number(f64::from(*n)))
        }
        Value::Bool(b) => coerce(schema, Input::Bool(*b)),
        other => {
            if wire_of(other) == Some(schema.wire) {
                Ok(other.clone())
            } else {
                Err(format!(
                    "expected a {:?} value, got {:?}",
                    schema.wire,
                    wire_of(other)
                ))
            }
        }
    }
}

/// The wire form a value travels in, `None` for the typed forms that
/// normalise to another (`Number`, `Enum`, `Ref`).
pub fn wire_of(value: &Value) -> Option<Wire> {
    Some(match value {
        Value::Bool(_) => Wire::Bool,
        Value::Length(_) => Wire::Length,
        Value::Text(_) => Wire::Text,
        Value::ColorRef(_) => Wire::ColorRef,
        Value::Bounds(_) => Wire::Bounds,
        Value::Transform(_) => Wire::Transform,
        Value::Lengths(_) => Wire::Lengths,
        Value::TabStops(_) => Wire::TabStops,
        Value::ParagraphRule(_) => Wire::ParagraphRule,
        Value::GradientFeather(_) => Wire::GradientFeather,
        Value::GradientStops(_) => Wire::GradientStops,
        Value::FramePath { .. } => Wire::FramePath,
        Value::PathPoint { .. } => Wire::PathPoint,
        Value::PathPointInsert { .. } => Wire::PathPointInsert,
        Value::PathPointRemove { .. } => Wire::PathPointRemove,
        Value::PathPointCurveType { .. } => Wire::PathPointCurveType,
        Value::PathOpenAt { .. } => Wire::PathOpenAt,
        Value::OutlineStroke { .. } => Wire::OutlineStroke,
        Value::OutlineStrokeVariable { .. } => Wire::OutlineStrokeVariable,
        Value::OffsetPath { .. } => Wire::OffsetPath,
        Value::SimplifyPath { .. } => Wire::SimplifyPath,
        Value::ClosePath { .. } => Wire::ClosePath,
        Value::PluginMetadata { .. } => Wire::PluginMetadata,
        Value::Number(_) | Value::Enum(_) | Value::Ref(_) => return None,
    })
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_enums_refuse_strangers_and_open_ones_do_not() {
        let closed = enum_(JUSTIFICATION);
        assert_eq!(
            coerce(&closed, Input::Text("CenterAlign")),
            Ok(Value::Text("CenterAlign".into()))
        );
        assert!(coerce(&closed, Input::Text("Middle")).is_err());
        // "" clears a nullable enum.
        assert_eq!(
            coerce(&closed, Input::Text("")),
            Ok(Value::Text(String::new()))
        );
        let open = enum_open(BLEND_MODES);
        assert!(coerce(&open, Input::Text("PlusLighter")).is_ok());
    }

    #[test]
    fn numbers_take_the_schema_wire_form_and_range() {
        assert_eq!(
            coerce(&pct(), Input::Number(40.0)),
            Ok(Value::Length(Some(40.0)))
        );
        assert!(coerce(&pct(), Input::Number(140.0)).is_err());
        assert!(coerce(&bool_(), Input::Number(1.0)).is_err());
        assert!(coerce(&color(), Input::Number(1.0)).is_err());
        assert_eq!(
            coerce(&text(), Input::Number(3.0)),
            Ok(Value::Text("3".into()))
        );
    }

    #[test]
    fn typed_forms_normalise_to_the_wire_form() {
        assert_eq!(
            validate(&length(), &Value::Number(12.0)),
            Ok(Value::Length(Some(12.0)))
        );
        assert_eq!(
            validate(&enum_(SPAN_COLUMN_TYPE), &Value::Enum("SpanColumns".into())),
            Ok(Value::Text("SpanColumns".into()))
        );
        assert!(validate(&enum_(SPAN_COLUMN_TYPE), &Value::Enum("Wide".into())).is_err());
        assert!(validate(&bounds(), &Value::Length(Some(1.0))).is_err());
        assert!(validate(&bool_().read_only(), &Value::Bool(true)).is_err());
    }
}
