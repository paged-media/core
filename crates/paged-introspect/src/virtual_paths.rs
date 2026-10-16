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

//! ADR 132 — virtual paths: the dedicated property ops, as paths on their
//! address kinds.
//!
//! About 45 of the wire ops are property writes with an argument shape of
//! their own (`editSwatch`, `layerSetName`, `setDocumentDefaults`, …). A
//! surface that wants "set any property" had to know every one. Here each
//! becomes a row `(address kind, path, schema, op)`, and `Mutation::Set`
//! resolves the row back to the op — the resolver lives with the model in
//! `paged-canvas` (it reads the current spec to merge one field), the
//! TABLE lives here so the catalog publishes it.
//!
//! Three kinds of row make up the whole property surface
//! ([`property_schemas`]):
//! - element paths (`settablePaths`), on frame / group / storyRange /
//!   table / cell;
//! - `addressed` paths (`property_paths!`), real `PropertyPath`s whose
//!   target is a layer, page, the document, a text variable or a style
//!   definition (`setProperty` / `setStyleProperty`);
//! - the op-backed rows below ([`VIRTUAL_PATHS`]).
//!
//! Indexed and keyed segments are written `[i]` and `[key]` in the table
//! (`rows[i].height`, `labels[key]`); a call names the concrete segment
//! (`rows[2].height`, `labels[x-paged:oid]`).

use crate::catalog::{addressed_on, schema, wire_name, ADDRESSED_PATHS, PROPERTY_PATHS};
use crate::schema::*;
use paged_mutate::PropertyPath as P;
use serde::Serialize;

/// One op-backed virtual path.
#[derive(Debug, Clone, Copy)]
pub struct VirtualPath {
    /// ADR 131 address kind.
    pub address_kind: &'static str,
    pub path: &'static str,
    pub schema: PropertySchema,
    /// The wire op (`Mutation` tag) a `Set` on this row becomes.
    pub op: &'static str,
    /// The op's argument (or spec field) the value lands in.
    pub field: &'static str,
    /// The op a `null` resolves to when clearing is a different op
    /// (`nextTextFrame: null` unlinks, `opacityMask: null` releases).
    pub clear_op: Option<&'static str>,
}

impl VirtualPath {
    const fn clears_with(self, op: &'static str) -> Self {
        Self {
            clear_op: Some(op),
            ..self
        }
    }
}

const fn v(
    address_kind: &'static str,
    path: &'static str,
    schema: PropertySchema,
    op: &'static str,
    field: &'static str,
) -> VirtualPath {
    VirtualPath {
        address_kind,
        path,
        schema,
        op,
        field,
        clear_op: None,
    }
}

const RENDERING_INTENTS: &[&str] = &[
    "Perceptual",
    "Saturation",
    "RelativeColorimetric",
    "AbsoluteColorimetric",
];
const RGB_POLICIES: &[&str] = &["PreserveEmbedded", "ConvertToWorking", "Off"];

/// The op-backed rows. Generated cockpit tables (`object-model`) count
/// these; `every_virtual_row_names_a_real_op` (catalog_apply_parity)
/// keeps every row's op on the wire, and `paged-canvas`'s
/// `virtual_paths` sweep sets, reads and undoes each one.
pub const VIRTUAL_PATHS: &[VirtualPath] = &[
    // ---- doc
    v(
        "doc",
        "defaultFillColor",
        color_text(),
        "setDocumentDefaults",
        "fillColor",
    ),
    v(
        "doc",
        "defaultStrokeColor",
        color_text(),
        "setDocumentDefaults",
        "strokeColor",
    ),
    v(
        "doc",
        "defaultStrokeWeight",
        length_pos(),
        "setDocumentDefaults",
        "strokeWeight",
    ),
    v(
        "doc",
        "cmykProfile",
        text(),
        "setColorSettings",
        "cmykProfileName",
    ),
    v(
        "doc",
        "rgbPolicy",
        enum_open(RGB_POLICIES),
        "setColorSettings",
        "rgbPolicy",
    ),
    v(
        "doc",
        "renderingIntent",
        enum_(RENDERING_INTENTS),
        "setColorSettings",
        "intent",
    ),
    v(
        "doc",
        "blackPointCompensation",
        bool_().default_bool(true),
        "setColorSettings",
        "bpc",
    ),
    v(
        "doc",
        "proofProfile",
        text(),
        "setProofSetup",
        "profileName",
    ),
    v(
        "doc",
        "proofSimulatePaperWhite",
        bool_().default_bool(false),
        "setProofSetup",
        "simulatePaperWhite",
    ),
    v(
        "doc",
        "proofIntent",
        enum_(RENDERING_INTENTS),
        "setProofSetup",
        "intent",
    ),
    v(
        "doc",
        "useStandardLabForSpots",
        bool_().default_bool(false),
        "setUseStandardLabForSpots",
        "enabled",
    ),
    v(
        "doc",
        "activeConditionSet",
        // Required: activating a set is the only op; there is no
        // "deactivate" to clear to (a read with no matching set is "").
        reference(RefKind::Condition).required(),
        "activateConditionSet",
        "setId",
    ),
    v(
        "doc",
        "labels[key]",
        text().idml(idml![child(Document, Label)]),
        "setDocumentMetadata",
        "value",
    ),
    // ---- swatch (editSwatch is a whole-spec op: read, merge, write)
    v(
        "swatch",
        "name",
        text().idml(idml![attr(Color | Tint, Name)]),
        "editSwatch",
        "name",
    ),
    v(
        "swatch",
        "space",
        enum_(SWATCH_SPACES)
            .required()
            .idml(idml![attr(Color | Tint, Space)]),
        "editSwatch",
        "space",
    ),
    v(
        "swatch",
        "value",
        lengths()
            .required()
            .idml(idml![attr(Color | Tint, ColorValue)]),
        "editSwatch",
        "value",
    ),
    v(
        "swatch",
        "model",
        enum_(SWATCH_MODELS).idml(idml![attr(Color | Tint, Model)]),
        "editSwatch",
        "model",
    ),
    v(
        "swatch",
        "alternateSpace",
        enum_(SWATCH_SPACES).idml(idml![attr(Color | Tint, AlternateSpace)]),
        "editSwatch",
        "alternateSpace",
    ),
    v(
        "swatch",
        "alternateValue",
        lengths().idml(idml![attr(Color | Tint, AlternateColorValue)]),
        "editSwatch",
        "alternateValue",
    ),
    // The colour as one value: a space change with its components,
    // atomically (`space` and `value` written apart pass through a
    // half-written colour).
    v(
        "swatch",
        "color",
        structure("SwatchColorSpec", Wire::SwatchColor).required(),
        "editSwatch",
        "color",
    ),
    v(
        "swatch",
        "tint",
        pct().idml(idml![attr(Color | Tint, TintValue)]),
        "editSwatch",
        "tint",
    ),
    v(
        "swatch",
        "alpha",
        pct().idml(idml![attr(Color | Tint, AlphaPercentage)]),
        "editSwatch",
        "alpha",
    ),
    v(
        "swatch",
        "convertToProcess",
        bool_().default_bool(false),
        "setInkSetting",
        "convertToProcess",
    ),
    v(
        "swatch",
        "inkAliasTo",
        reference(RefKind::Swatch),
        "setInkSetting",
        "aliasTo",
    ),
    // ---- gradient
    v(
        "gradient",
        "name",
        text().idml(idml![attr(Gradient, Name)]),
        "editGradient",
        "name",
    ),
    v(
        "gradient",
        "kind",
        enum_(GRADIENT_KINDS)
            .required()
            .idml(idml![attr(Gradient, Type)]),
        "editGradient",
        "kind",
    ),
    v(
        "gradient",
        "stops",
        list_of("GradientStopSpec", Wire::GradientStops),
        "editGradient",
        "stops",
    ),
    // ---- colour group
    v(
        "colorGroup",
        "name",
        text().idml(idml![attr(ColorGroup, Name)]),
        "editColorGroup",
        "name",
    ),
    v(
        "colorGroup",
        "members",
        list_of("swatch ref", Wire::Text).idml(idml![attr(ColorGroup, ColorGroupSwatches)]),
        "editColorGroup",
        "members",
    ),
    // ---- numbering list
    v(
        "numberingList",
        "name",
        text().idml(idml![attr(NumberingList, Name)]),
        "editNumberingList",
        "name",
    ),
    v(
        "numberingList",
        "continueAcrossStories",
        bool_()
            .default_bool(false)
            .idml(idml![attr(NumberingList, ContinueNumbersAcrossStories)]),
        "editNumberingList",
        "continueAcrossStories",
    ),
    v(
        "numberingList",
        "continueAcrossDocuments",
        bool_()
            .default_bool(false)
            .idml(idml![attr(NumberingList, ContinueNumbersAcrossDocuments)]),
        "editNumberingList",
        "continueAcrossDocuments",
    ),
    // ---- style (the definition's NAME; its properties are the
    // `setStyleProperty` rows `property_schemas` lists per family)
    v(
        "style",
        "name",
        text().required().idml(idml![attr(
            ParagraphStyle | CharacterStyle | ObjectStyle | CellStyle | TableStyle,
            Name
        )]),
        "rename<Family>Style",
        "name",
    ),
    // ---- guide / condition
    v(
        "guide",
        "position",
        length().idml(idml![attr(Guide, Location)]),
        "moveGuide",
        "position",
    ),
    v(
        "condition",
        "visible",
        bool_()
            .default_bool(true)
            .idml(idml![attr(Condition, Visible)]),
        "setConditionVisible",
        "visible",
    ),
    // ---- story
    v(
        "story",
        "flowGrow",
        bool_().default_bool(false),
        "setFlowGrowRule",
        "grow",
    ),
    v(
        "story",
        "flowGrowMaxPages",
        count().min(1.0),
        "setFlowGrowRule",
        "maxPages",
    ),
    v(
        "story",
        "flowGrowCopyFrameOptions",
        bool_().default_bool(false),
        "setFlowGrowRule",
        "copyFrameOptions",
    ),
    // ---- page
    v(
        "page",
        "appliedMaster",
        reference(RefKind::Master).idml(idml![attr(Page, AppliedMaster)]),
        "applyMasterToPage",
        "master",
    ),
    v(
        "page",
        "labels[key]",
        text().idml(idml![child(Page, Label)]),
        "setPageMetadata",
        "value",
    ),
    // ---- master
    v(
        "master",
        "name",
        text().idml(idml![attr(MasterSpread, Name)]),
        "renameMaster",
        "name",
    ),
    // ---- section
    v(
        "section",
        "prefix",
        text().idml(idml![attr(Section, SectionPrefix)]),
        "editSection",
        "prefix",
    ),
    v(
        "section",
        "numberingStyle",
        enum_open(PAGE_NUMBER_STYLES).idml(idml![child(Section, PageNumberStyle)]),
        "editSection",
        "numberingStyle",
    ),
    v(
        "section",
        "startAt",
        count().min(1.0).idml(idml![attr(Section, PageNumberStart)]),
        "editSection",
        "startAt",
    ),
    // ---- table / cell
    v(
        "table",
        "rows[i].height",
        length_pos().idml(idml![attr(Row, SingleRowHeight)]),
        "setRowHeight",
        "height",
    ),
    v(
        "table",
        "columns[i].width",
        length_pos().idml(idml![attr(Column, SingleColumnWidth)]),
        "setColumnWidth",
        "width",
    ),
    v(
        "cell",
        "rowSpan",
        count().min(1.0).required().idml(idml![attr(Cell, RowSpan)]),
        "setCellSpan",
        "rowSpan",
    ),
    v(
        "cell",
        "columnSpan",
        count()
            .min(1.0)
            .required()
            .idml(idml![attr(Cell, ColumnSpan)]),
        "setCellSpan",
        "columnSpan",
    ),
    // ---- field
    v("field", "value", text(), "setFieldValue", "value"),
    // ---- frame / group
    v(
        "frame",
        "imageUri",
        reference(RefKind::Uri).required().idml(idml![attr(
            Image | EPS | PDF | ImportedPage | Link,
            LinkResourceURI
        )]),
        "placeImage",
        "uri",
    ),
    v(
        "frame",
        "labels[key]",
        text().idml(idml![child(
            TextFrame | Rectangle | Oval | GraphicLine | Polygon,
            Label
        )]),
        "setPluginMetadata",
        "value",
    ),
    v(
        "group",
        "groupTransform",
        transform().idml(idml![attr(Group, ItemTransform)]),
        "setGroupTransform",
        "transform",
    ),
    // The content, the threads, the mask and the path points: what a
    // binding needs to write that no property path carried.
    v(
        "frame",
        "imageBytes",
        asset().idml(idml![child(Image, Contents)]),
        "replaceImageBytes",
        "bytes",
    ),
    v(
        "frame",
        "nextTextFrame",
        reference(RefKind::TextFrame).idml(idml![attr(TextFrame, NextTextFrame)]),
        "linkFrames",
        "to",
    )
    .clears_with("unlinkFrames"),
    v(
        "frame",
        "previousTextFrame",
        reference(RefKind::TextFrame),
        "linkFrames",
        "from",
    )
    .clears_with("unlinkFrames"),
    v(
        "frame",
        "opacityMask",
        reference(RefKind::PageItem),
        "applyOpacityMask",
        "maskId",
    )
    .clears_with("releaseOpacityMask"),
    v(
        "frame",
        "points[i].anchor",
        point().idml(idml![attr(PathPointType, Anchor)]),
        "pathPointSet",
        "position",
    ),
    v(
        "frame",
        "points[i].left",
        point().idml(idml![attr(PathPointType, LeftDirection)]),
        "pathPointSet",
        "position",
    ),
    v(
        "frame",
        "points[i].right",
        point().idml(idml![attr(PathPointType, RightDirection)]),
        "pathPointSet",
        "position",
    ),
    v(
        "frame",
        "points[i].curveType",
        enum_(CURVE_TYPES).required(),
        "pathPointCurveType",
        "smooth",
    ),
];

/// Find the op-backed row for `(address kind, path)`. A concrete indexed
/// or keyed path (`rows[3].height`, `labels[x-paged:a]`) matches its
/// pattern row; the caller parses the segment with [`segment`].
pub fn lookup_virtual(address_kind: &str, path: &str) -> Option<&'static VirtualPath> {
    let pattern = pattern_of(path);
    VIRTUAL_PATHS
        .iter()
        .find(|row| row.address_kind == address_kind && row.path == pattern)
}

/// The schema of `path` on an object of `address_kind` (`family` names
/// the style family for `style`): an op-backed row, an addressed path, or
/// an element / style-definition `PropertyPath`. What a surface coerces a
/// plain JSON value against before it sends `Set`.
pub fn schema_for(address_kind: &str, family: Option<&str>, path: &str) -> Option<PropertySchema> {
    if let Some(row) = lookup_virtual(address_kind, path).or_else(|| {
        (address_kind == "group")
            .then(|| lookup_virtual("frame", path))
            .flatten()
    }) {
        return Some(row.schema);
    }
    let kind = match (address_kind, family) {
        ("style", Some(f)) => format!("style:{f}"),
        (k, _) => k.to_string(),
    };
    if let Some((_, p, _)) = ADDRESSED_PATHS
        .iter()
        .find(|(name, _, k)| *name == path && *k == kind)
    {
        return Some(schema(*p));
    }
    crate::catalog::lookup_path(path).map(schema)
}

/// The `[…]` segment of a concrete path, if it has one.
pub fn segment(path: &str) -> Option<&str> {
    let open = path.find('[')?;
    let close = path[open..].find(']')? + open;
    Some(&path[open + 1..close])
}

/// `rows[3].height` → `rows[i].height`; `labels[x]` → `labels[key]`.
fn pattern_of(path: &str) -> String {
    let Some(seg) = segment(path) else {
        return path.to_string();
    };
    let placeholder = if seg.chars().all(|c| c.is_ascii_digit()) && !seg.is_empty() {
        "i"
    } else {
        "key"
    };
    path.replacen(&format!("[{seg}]"), &format!("[{placeholder}]"), 1)
}

/// The `PropertyPath`s `setStyleProperty` takes per style family — the
/// apply arms in `paged-mutate/src/apply/layer.rs`, published.
pub fn style_paths(family: &str) -> &'static [P] {
    match family {
        "paragraph" => &[
            P::CharacterFontSize,
            P::CharacterTracking,
            P::CharacterFillColor,
            P::CharacterFontFamily,
            P::CharacterFontStyle,
            P::CharacterLeading,
            P::ParagraphSpaceBefore,
            P::ParagraphSpaceAfter,
            P::ParagraphFirstLineIndent,
            P::ParagraphJustification,
            P::ParagraphStyleNextStyle,
            P::ParagraphLeftIndent,
            P::ParagraphRightIndent,
            P::ParagraphHyphenationZone,
            P::ParagraphKeepWithNext,
            P::ParagraphKeepLinesTogether,
            P::ParagraphKeepAllLinesTogether,
            P::ParagraphKeepFirstLines,
            P::ParagraphKeepLastLines,
            P::ParagraphStartParagraph,
            P::ParagraphComposer,
            P::ParagraphSpanColumnType,
            P::ParagraphSpanSplitColumnCount,
            P::ParagraphSpanColumnMinSpaceBefore,
            P::ParagraphSpanColumnMinSpaceAfter,
            P::ParagraphSplitColumnInsideGutter,
            P::ParagraphSplitColumnOutsideGutter,
            P::ParagraphHyphenation,
            P::ParagraphListType,
            P::ParagraphNumberingFormat,
            P::ParagraphAppliedNumberingList,
            P::ParagraphBulletsTextAfter,
            P::ParagraphNumberingExpression,
            P::ParagraphNumberingStartAt,
            P::ParagraphNumberingContinue,
            P::ParagraphNumberingLevel,
            P::ParagraphNumberingApplyRestartPolicy,
            P::ParagraphBulletsCharacterStyle,
            P::ParagraphNumberingCharacterStyle,
            P::ParagraphBulletCharacter,
            P::ParagraphTabStops,
            P::CharacterCase,
            P::CharacterPosition,
            P::CharacterBaselineShift,
            P::CharacterUnderline,
            P::CharacterStrikethru,
            P::CharacterLigatures,
            P::CharacterKerningMethod,
            P::ParagraphAutoLeading,
            P::ParagraphSingleWordJustification,
            P::ParagraphLastLineIndent,
            P::ParagraphBalanceRaggedLines,
        ],
        "character" => &[
            P::CharacterFontSize,
            P::CharacterTracking,
            P::CharacterFillColor,
            P::CharacterFontFamily,
            P::CharacterFontStyle,
            P::CharacterLeading,
            P::CharacterCase,
            P::CharacterPosition,
            P::CharacterBaselineShift,
            P::CharacterUnderline,
            P::CharacterStrikethru,
            P::CharacterLigatures,
            P::CharacterKerningMethod,
            P::CharacterNoBreak,
        ],
        "object" => &[
            P::FrameFillColor,
            P::FrameFillTint,
            P::FrameStrokeColor,
            P::FrameStrokeWeight,
            P::ObjectStyleStrokeTint,
            P::ObjectStyleCornerRadius,
            P::ObjectStyleCornerOption,
        ],
        "cell" => &[
            P::CellFillColor,
            P::CellFillTint,
            P::CellVerticalJustification,
            P::CellStyleRotationAngle,
            P::CellTopEdgeStrokeColor,
            P::CellTopEdgeStrokeWeight,
            P::CellBottomEdgeStrokeColor,
            P::CellBottomEdgeStrokeWeight,
            P::CellLeftEdgeStrokeColor,
            P::CellLeftEdgeStrokeWeight,
            P::CellRightEdgeStrokeColor,
            P::CellRightEdgeStrokeWeight,
        ],
        "table" => &[
            P::TableStyleHeaderRegionCellStyle,
            P::TableStyleBodyRegionCellStyle,
            P::TableStyleFooterRegionCellStyle,
            P::TableStyleLeftColumnRegionCellStyle,
            P::TableStyleRightColumnRegionCellStyle,
            P::TableStyleTopBorderStrokeColor,
            P::TableStyleTopBorderStrokeWeight,
            P::TableStyleBottomBorderStrokeColor,
            P::TableStyleBottomBorderStrokeWeight,
            P::TableStyleLeftBorderStrokeColor,
            P::TableStyleLeftBorderStrokeWeight,
            P::TableStyleRightBorderStrokeColor,
            P::TableStyleRightBorderStrokeWeight,
            P::TableStyleAlternatingFills,
            P::TableStyleStartRowFillColor,
            P::TableStyleStartRowFillCount,
            P::TableStyleStartRowFillTint,
            P::TableStyleEndRowFillColor,
            P::TableStyleEndRowFillCount,
            P::TableStyleEndRowFillTint,
        ],
        _ => &[],
    }
}

pub const STYLE_FAMILIES: &[&str] = &["paragraph", "character", "object", "cell", "table"];

/// The address kind of an ELEMENT path (one `ElementId` reaches).
pub fn element_address_kind(path: P) -> &'static str {
    let name = wire_name(path);
    match path {
        P::AppliedParagraphStyle | P::AppliedCharacterStyle | P::AppliedConditions => "storyRange",
        P::AppliedCellStyle => "cell",
        P::AppliedTableStyle => "table",
        _ if name.starts_with("character") || name.starts_with("paragraph") => "storyRange",
        _ if name.starts_with("cell") => "cell",
        _ => "frame",
    }
}

/// One published schema row (ADR 132's normative shape, the parts this
/// engine fills).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaRow {
    pub address_kind: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub applies_to: Vec<&'static str>,
    pub path: &'static str,
    #[serde(rename = "type")]
    pub value_type: TypeJson,
    pub wire: Wire,
    pub nullable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<RangeJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<DefaultValue>,
    pub access: &'static str,
    #[serde(rename = "virtual", skip_serializing_if = "Option::is_none")]
    pub virtual_op: Option<VirtualJson>,
    pub bindable: bool,
    /// The IDML attributes the value is read from, on the elements this
    /// row reaches (a style row names its style element, an element row
    /// the page items or ranges). Omitted when there is no single
    /// attribute.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub idml: Vec<IdmlAttr>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeJson {
    pub kind: ValueType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<Unit>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub members: &'static [&'static str],
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<RefKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub of: Option<ValueType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RangeJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualJson {
    pub op: &'static str,
    pub field: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clear_op: Option<&'static str>,
}

/// The IDML element a style family's definitions are written as.
pub const STYLE_ELEMENTS: &[(&str, &str)] = &[
    ("paragraph", "ParagraphStyle"),
    ("character", "CharacterStyle"),
    ("object", "ObjectStyle"),
    ("cell", "CellStyle"),
    ("table", "TableStyle"),
];

/// A path's IDML spellings narrowed to the elements one row reaches: a
/// style row of one family keeps that family's style element, a row on
/// any style (`style:<family>/<id>` `name`) keeps them all, and every
/// other row drops the style definitions (they are the style rows' own).
fn idml_for_row(address_kind: &str, applies_to: &[&str], s: &PropertySchema) -> Vec<IdmlAttr> {
    let is_style_element = |e: &str| STYLE_ELEMENTS.iter().any(|(_, el)| *el == e);
    match applies_to {
        [family] => {
            let el = STYLE_ELEMENTS
                .iter()
                .find(|(f, _)| f == family)
                .map(|(_, el)| *el);
            s.idml
                .iter()
                .filter(|a| Some(a.element) == el)
                .copied()
                .collect()
        }
        _ if address_kind == "style" => s.idml.to_vec(),
        _ => s
            .idml
            .iter()
            .filter(|a| !is_style_element(a.element))
            .copied()
            .collect(),
    }
}

fn row(
    address_kind: &'static str,
    applies_to: Vec<&'static str>,
    path: &'static str,
    s: PropertySchema,
    virtual_op: Option<VirtualJson>,
) -> SchemaRow {
    let access = if s.derived {
        "derived"
    } else if s.read_only {
        "readOnly"
    } else {
        "readWrite"
    };
    let idml = idml_for_row(address_kind, &applies_to, &s);
    SchemaRow {
        idml,
        address_kind,
        applies_to,
        path,
        value_type: TypeJson {
            kind: s.value_type,
            unit: s.unit,
            members: s.members,
            open: s.open,
            to: s.ref_kind,
            of: s.item,
            shape: s.shape,
        },
        wire: s.wire,
        nullable: s.nullable,
        range: (s.min.is_some() || s.max.is_some()).then_some(RangeJson {
            min: s.min,
            max: s.max,
        }),
        default: s.default,
        access,
        virtual_op,
        bindable: access == "readWrite" && s.value_type != ValueType::Operation,
    }
}

/// Every settable or readable property, as published rows: element
/// paths, addressed paths, style-definition paths per family, and the
/// op-backed virtual paths.
pub fn property_schemas() -> Vec<SchemaRow> {
    let mut out = Vec::new();
    for (name, path) in PROPERTY_PATHS {
        out.push(row(
            element_address_kind(*path),
            Vec::new(),
            name,
            schema(*path),
            None,
        ));
    }
    for (name, path, kind) in ADDRESSED_PATHS {
        // Style-definition-only paths are listed with their family below.
        if kind.starts_with("style:") {
            continue;
        }
        out.push(row(
            kind,
            Vec::new(),
            name,
            schema(*path),
            Some(VirtualJson {
                op: "setProperty",
                field: "value",
                clear_op: None,
            }),
        ));
    }
    for family in STYLE_FAMILIES {
        for path in style_paths(family) {
            debug_assert!(addressed_on(*path).is_none_or(|k| k == format!("style:{family}")));
            out.push(row(
                "style",
                vec![family],
                wire_name(*path),
                schema(*path),
                Some(VirtualJson {
                    op: "setStyleProperty",
                    field: "value",
                    clear_op: None,
                }),
            ));
        }
    }
    for vp in VIRTUAL_PATHS {
        out.push(row(
            vp.address_kind,
            Vec::new(),
            vp.path,
            vp.schema,
            Some(VirtualJson {
                op: vp.op,
                field: vp.field,
                clear_op: vp.clear_op,
            }),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concrete_segments_match_their_pattern_rows() {
        assert_eq!(
            lookup_virtual("table", "rows[3].height").unwrap().op,
            "setRowHeight"
        );
        assert_eq!(
            lookup_virtual("frame", "labels[x-paged:oid]").unwrap().op,
            "setPluginMetadata"
        );
        assert_eq!(segment("labels[x-paged:oid]"), Some("x-paged:oid"));
        assert!(lookup_virtual("swatch", "nope").is_none());
    }

    #[test]
    fn every_addressed_style_path_is_listed_under_its_family() {
        for (name, path, kind) in ADDRESSED_PATHS {
            if let Some(family) = kind.strip_prefix("style:") {
                assert!(
                    style_paths(family).contains(path),
                    "{name} is addressed on {kind} but {family}'s list omits it"
                );
            }
        }
    }

    /// (element, attribute) pairs that legitimately reach more than one
    /// path, each with the exact set of paths it reaches.
    const SHARED_IDML_ATTRIBUTES: &[(&str, &str, &[&str])] = &[
        // A group's own transform: the frame path accepts a group too.
        (
            "Group",
            "ItemTransform",
            &["frameTransform", "group:groupTransform"],
        ),
        // R-10: one `<BlendingSetting>` per transparency target — the
        // object's, and under `<Fill|Stroke|ContentTransparencySetting>`.
        (
            "BlendingSetting",
            "Opacity",
            &[
                "frameOpacity",
                "frameFillOpacity",
                "frameStrokeOpacity",
                "frameContentOpacity",
            ],
        ),
        (
            "BlendingSetting",
            "BlendMode",
            &[
                "frameBlendMode",
                "frameFillBlendMode",
                "frameStrokeBlendMode",
                "frameContentBlendMode",
            ],
        ),
    ];

    fn well_formed(name: &str) -> bool {
        name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && name.chars().all(|c| c.is_ascii_alphanumeric())
    }

    #[test]
    fn idml_mappings_are_well_formed_and_unambiguous() {
        use std::collections::{BTreeMap, BTreeSet};
        let rows = property_schemas();

        // Every declared spelling is well formed, listed once, and lands
        // on at least one published row (a style element on a path no
        // family lists would otherwise vanish silently).
        let declared = crate::catalog::ALL_PATHS
            .iter()
            .map(|p| (wire_name(*p), schema(*p).idml))
            .chain(VIRTUAL_PATHS.iter().map(|v| (v.path, v.schema.idml)));
        for (path, idml) in declared {
            let mut seen = std::collections::HashSet::new();
            for a in idml {
                assert!(
                    well_formed(a.element) && well_formed(a.attribute),
                    "{path}: malformed IDML mapping {a:?}"
                );
                assert!(seen.insert(*a), "{path}: {a:?} listed twice");
                assert!(
                    rows.iter().any(|r| r.path == path && r.idml.contains(a)),
                    "{path}: {a:?} reaches no published row"
                );
            }
        }

        // No (element, attribute) pair names two different paths unless
        // it is listed in SHARED_IDML_ATTRIBUTES with exactly those paths.
        let mut by_pair: BTreeMap<(&str, &str), BTreeSet<String>> = BTreeMap::new();
        for r in &rows {
            for a in &r.idml {
                let path = if r
                    .virtual_op
                    .as_ref()
                    .is_some_and(|v| v.op != "setProperty" && v.op != "setStyleProperty")
                {
                    format!("{}:{}", r.address_kind, r.path)
                } else {
                    r.path.to_string()
                };
                by_pair
                    .entry((a.element, a.attribute))
                    .or_default()
                    .insert(path);
            }
        }
        for ((element, attribute), paths) in &by_pair {
            if paths.len() < 2 {
                continue;
            }
            let allowed = SHARED_IDML_ATTRIBUTES
                .iter()
                .find(|(e, a, _)| e == element && a == attribute)
                .map(|(_, _, p)| p.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>());
            assert_eq!(
                allowed.as_ref(),
                Some(paths),
                "{element}@{attribute} maps to several paths: {paths:?}"
            );
        }
        for (element, attribute, _) in SHARED_IDML_ATTRIBUTES {
            assert!(
                by_pair
                    .get(&(*element, *attribute))
                    .is_some_and(|p| p.len() > 1),
                "{element}@{attribute} is listed as shared but is not"
            );
        }
    }

    #[test]
    fn rows_are_unique_per_address_kind_and_family() {
        let mut seen = std::collections::HashSet::new();
        for r in property_schemas() {
            let key = (r.address_kind, r.applies_to.clone(), r.path);
            assert!(seen.insert(key.clone()), "duplicate schema row {key:?}");
        }
    }
}
