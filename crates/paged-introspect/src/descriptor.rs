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
    FrameGradientFillAngle,
    FrameGradientFillLength,
    FrameGradientStrokeAngle,
    FrameGradientStrokeLength,
    PathOpenAt,
    OutlineStroke,
    OffsetPath,
    SimplifyPath,
    PageBounds,
    FrameGradientFeather,
    CharacterFontFamily,
    CharacterFontStyle,
    CharacterKerningMethod,
    CharacterCase,
    CharacterPosition,
    CharacterLanguage,
    CharacterBaselineShift,
    CharacterHorizontalScale,
    CharacterVerticalScale,
    CharacterSkew,
    CharacterUnderline,
    CharacterStrikethru,
    CharacterLigatures,
    CharacterOtfFeatures,
    ParagraphLeftIndent,
    ParagraphRightIndent,
    ParagraphDropCapCharacters,
    ParagraphDropCapLines,
    ParagraphHyphenation,
    ParagraphKeepLinesTogether,
    ParagraphKeepWithNext,
    ParagraphRuleAbove,
    ParagraphRuleBelow,
    ParagraphTabStops,
    ParagraphListType,
    ParagraphBulletCharacter,
    ParagraphNumberingFormat,
    // W0.3 — text-frame prefs / wrap / fitting / stroke / corners /
    // transform-decompose / overprint.
    TextFrameColumnCount,
    TextFrameColumnGutter,
    TextFrameColumnBalance,
    TextFrameVerticalJustification,
    TextFrameAutoSizing,
    TextFrameFirstBaseline,
    TextWrapInvert,
    FrameFittingReferencePoint,
    FrameAutoFit,
    FrameStrokeType,
    FrameStrokeJoin,
    FrameStrokeMiterLimit,
    FrameStrokeAlignment,
    FrameStrokeGapColor,
    FrameStrokeGapTint,
    FrameCornerOptionTopLeft,
    FrameCornerOptionTopRight,
    FrameCornerOptionBottomLeft,
    FrameCornerOptionBottomRight,
    FrameCornerRadiusTopLeft,
    FrameCornerRadiusTopRight,
    FrameCornerRadiusBottomLeft,
    FrameCornerRadiusBottomRight,
    FrameRotationAngle,
    FrameScaleX,
    FrameScaleY,
    FrameFlipH,
    FrameFlipV,
    FrameOverprintFill,
    FrameOverprintStroke,
    // W0.4 — transparency effects (gap 18).
    FrameInnerShadowEnabled,
    FrameInnerShadowBlendMode,
    FrameInnerShadowColor,
    FrameInnerShadowOpacity,
    FrameInnerShadowAngle,
    FrameInnerShadowDistance,
    FrameInnerShadowSize,
    FrameInnerShadowChoke,
    FrameInnerShadowNoise,
    FrameOuterGlowEnabled,
    FrameOuterGlowBlendMode,
    FrameOuterGlowColor,
    FrameOuterGlowOpacity,
    FrameOuterGlowSpread,
    FrameOuterGlowSize,
    FrameOuterGlowNoise,
    FrameInnerGlowEnabled,
    FrameInnerGlowBlendMode,
    FrameInnerGlowColor,
    FrameInnerGlowOpacity,
    FrameInnerGlowChoke,
    FrameInnerGlowSize,
    FrameInnerGlowSource,
    FrameInnerGlowNoise,
    FrameBevelEnabled,
    FrameBevelStyle,
    FrameBevelTechnique,
    FrameBevelDepth,
    FrameBevelDirection,
    FrameBevelSize,
    FrameBevelSoften,
    FrameBevelAngle,
    FrameBevelAltitude,
    FrameBevelHighlightColor,
    FrameBevelShadowColor,
    FrameBevelHighlightOpacity,
    FrameBevelShadowOpacity,
    FrameSatinEnabled,
    FrameSatinBlendMode,
    FrameSatinColor,
    FrameSatinOpacity,
    FrameSatinAngle,
    FrameSatinDistance,
    FrameSatinSize,
    FrameSatinInvert,
    FrameFeatherEnabled,
    FrameFeatherWidth,
    FrameFeatherCornerType,
    FrameFeatherNoise,
    FrameFeatherChoke,
    FrameDirectionalFeatherEnabled,
    FrameDirectionalFeatherLeftWidth,
    FrameDirectionalFeatherRightWidth,
    FrameDirectionalFeatherTopWidth,
    FrameDirectionalFeatherBottomWidth,
    FrameDirectionalFeatherAngle,
    FrameDirectionalFeatherNoise,
    FrameDirectionalFeatherChoke,
    FrameBlendMode,
    // W3.A0 — text-frame thread chain (read-only).
    NextTextFrame,
    PreviousTextFrame,
    // W3.A1 — table cell properties.
    CellFillColor,
    CellFillTint,
    CellInsetTop,
    CellInsetLeft,
    CellInsetBottom,
    CellInsetRight,
    CellVerticalJustification,
    // Aftercare-A — table dimensions (read-only).
    TableRowCount,
    TableColumnCount,
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
            PropertyPath::FrameGradientFillAngle => PropertyPathJson::FrameGradientFillAngle,
            PropertyPath::FrameGradientFillLength => PropertyPathJson::FrameGradientFillLength,
            PropertyPath::FrameGradientStrokeAngle => PropertyPathJson::FrameGradientStrokeAngle,
            PropertyPath::FrameGradientStrokeLength => {
                PropertyPathJson::FrameGradientStrokeLength
            }
            PropertyPath::PathOpenAt => PropertyPathJson::PathOpenAt,
            PropertyPath::OutlineStroke => PropertyPathJson::OutlineStroke,
            PropertyPath::OffsetPath => PropertyPathJson::OffsetPath,
            PropertyPath::SimplifyPath => PropertyPathJson::SimplifyPath,
            PropertyPath::PageBounds => PropertyPathJson::PageBounds,
            PropertyPath::FrameGradientFeather => PropertyPathJson::FrameGradientFeather,
            PropertyPath::CharacterFontFamily => PropertyPathJson::CharacterFontFamily,
            PropertyPath::CharacterFontStyle => PropertyPathJson::CharacterFontStyle,
            PropertyPath::CharacterKerningMethod => PropertyPathJson::CharacterKerningMethod,
            PropertyPath::CharacterCase => PropertyPathJson::CharacterCase,
            PropertyPath::CharacterPosition => PropertyPathJson::CharacterPosition,
            PropertyPath::CharacterLanguage => PropertyPathJson::CharacterLanguage,
            PropertyPath::CharacterBaselineShift => PropertyPathJson::CharacterBaselineShift,
            PropertyPath::CharacterHorizontalScale => PropertyPathJson::CharacterHorizontalScale,
            PropertyPath::CharacterVerticalScale => PropertyPathJson::CharacterVerticalScale,
            PropertyPath::CharacterSkew => PropertyPathJson::CharacterSkew,
            PropertyPath::CharacterUnderline => PropertyPathJson::CharacterUnderline,
            PropertyPath::CharacterStrikethru => PropertyPathJson::CharacterStrikethru,
            PropertyPath::CharacterLigatures => PropertyPathJson::CharacterLigatures,
            PropertyPath::CharacterOtfFeatures => PropertyPathJson::CharacterOtfFeatures,
            PropertyPath::ParagraphLeftIndent => PropertyPathJson::ParagraphLeftIndent,
            PropertyPath::ParagraphRightIndent => PropertyPathJson::ParagraphRightIndent,
            PropertyPath::ParagraphDropCapCharacters => {
                PropertyPathJson::ParagraphDropCapCharacters
            }
            PropertyPath::ParagraphDropCapLines => PropertyPathJson::ParagraphDropCapLines,
            PropertyPath::ParagraphHyphenation => PropertyPathJson::ParagraphHyphenation,
            PropertyPath::ParagraphKeepLinesTogether => {
                PropertyPathJson::ParagraphKeepLinesTogether
            }
            PropertyPath::ParagraphKeepWithNext => PropertyPathJson::ParagraphKeepWithNext,
            PropertyPath::ParagraphRuleAbove => PropertyPathJson::ParagraphRuleAbove,
            PropertyPath::ParagraphRuleBelow => PropertyPathJson::ParagraphRuleBelow,
            PropertyPath::ParagraphTabStops => PropertyPathJson::ParagraphTabStops,
            PropertyPath::ParagraphListType => PropertyPathJson::ParagraphListType,
            PropertyPath::ParagraphBulletCharacter => PropertyPathJson::ParagraphBulletCharacter,
            PropertyPath::ParagraphNumberingFormat => PropertyPathJson::ParagraphNumberingFormat,
            // W0.3.
            PropertyPath::TextFrameColumnCount => PropertyPathJson::TextFrameColumnCount,
            PropertyPath::TextFrameColumnGutter => PropertyPathJson::TextFrameColumnGutter,
            PropertyPath::TextFrameColumnBalance => PropertyPathJson::TextFrameColumnBalance,
            PropertyPath::TextFrameVerticalJustification => {
                PropertyPathJson::TextFrameVerticalJustification
            }
            PropertyPath::TextFrameAutoSizing => PropertyPathJson::TextFrameAutoSizing,
            PropertyPath::TextFrameFirstBaseline => PropertyPathJson::TextFrameFirstBaseline,
            PropertyPath::TextWrapInvert => PropertyPathJson::TextWrapInvert,
            PropertyPath::FrameFittingReferencePoint => {
                PropertyPathJson::FrameFittingReferencePoint
            }
            PropertyPath::FrameAutoFit => PropertyPathJson::FrameAutoFit,
            PropertyPath::FrameStrokeType => PropertyPathJson::FrameStrokeType,
            PropertyPath::FrameStrokeJoin => PropertyPathJson::FrameStrokeJoin,
            PropertyPath::FrameStrokeMiterLimit => PropertyPathJson::FrameStrokeMiterLimit,
            PropertyPath::FrameStrokeAlignment => PropertyPathJson::FrameStrokeAlignment,
            PropertyPath::FrameStrokeGapColor => PropertyPathJson::FrameStrokeGapColor,
            PropertyPath::FrameStrokeGapTint => PropertyPathJson::FrameStrokeGapTint,
            PropertyPath::FrameCornerOptionTopLeft => {
                PropertyPathJson::FrameCornerOptionTopLeft
            }
            PropertyPath::FrameCornerOptionTopRight => {
                PropertyPathJson::FrameCornerOptionTopRight
            }
            PropertyPath::FrameCornerOptionBottomLeft => {
                PropertyPathJson::FrameCornerOptionBottomLeft
            }
            PropertyPath::FrameCornerOptionBottomRight => {
                PropertyPathJson::FrameCornerOptionBottomRight
            }
            PropertyPath::FrameCornerRadiusTopLeft => {
                PropertyPathJson::FrameCornerRadiusTopLeft
            }
            PropertyPath::FrameCornerRadiusTopRight => {
                PropertyPathJson::FrameCornerRadiusTopRight
            }
            PropertyPath::FrameCornerRadiusBottomLeft => {
                PropertyPathJson::FrameCornerRadiusBottomLeft
            }
            PropertyPath::FrameCornerRadiusBottomRight => {
                PropertyPathJson::FrameCornerRadiusBottomRight
            }
            PropertyPath::FrameRotationAngle => PropertyPathJson::FrameRotationAngle,
            PropertyPath::FrameScaleX => PropertyPathJson::FrameScaleX,
            PropertyPath::FrameScaleY => PropertyPathJson::FrameScaleY,
            PropertyPath::FrameFlipH => PropertyPathJson::FrameFlipH,
            PropertyPath::FrameFlipV => PropertyPathJson::FrameFlipV,
            PropertyPath::FrameOverprintFill => PropertyPathJson::FrameOverprintFill,
            PropertyPath::FrameOverprintStroke => PropertyPathJson::FrameOverprintStroke,
            // W0.4 — transparency effects.
            PropertyPath::FrameInnerShadowEnabled => PropertyPathJson::FrameInnerShadowEnabled,
            PropertyPath::FrameInnerShadowBlendMode => PropertyPathJson::FrameInnerShadowBlendMode,
            PropertyPath::FrameInnerShadowColor => PropertyPathJson::FrameInnerShadowColor,
            PropertyPath::FrameInnerShadowOpacity => PropertyPathJson::FrameInnerShadowOpacity,
            PropertyPath::FrameInnerShadowAngle => PropertyPathJson::FrameInnerShadowAngle,
            PropertyPath::FrameInnerShadowDistance => PropertyPathJson::FrameInnerShadowDistance,
            PropertyPath::FrameInnerShadowSize => PropertyPathJson::FrameInnerShadowSize,
            PropertyPath::FrameInnerShadowChoke => PropertyPathJson::FrameInnerShadowChoke,
            PropertyPath::FrameInnerShadowNoise => PropertyPathJson::FrameInnerShadowNoise,
            PropertyPath::FrameOuterGlowEnabled => PropertyPathJson::FrameOuterGlowEnabled,
            PropertyPath::FrameOuterGlowBlendMode => PropertyPathJson::FrameOuterGlowBlendMode,
            PropertyPath::FrameOuterGlowColor => PropertyPathJson::FrameOuterGlowColor,
            PropertyPath::FrameOuterGlowOpacity => PropertyPathJson::FrameOuterGlowOpacity,
            PropertyPath::FrameOuterGlowSpread => PropertyPathJson::FrameOuterGlowSpread,
            PropertyPath::FrameOuterGlowSize => PropertyPathJson::FrameOuterGlowSize,
            PropertyPath::FrameOuterGlowNoise => PropertyPathJson::FrameOuterGlowNoise,
            PropertyPath::FrameInnerGlowEnabled => PropertyPathJson::FrameInnerGlowEnabled,
            PropertyPath::FrameInnerGlowBlendMode => PropertyPathJson::FrameInnerGlowBlendMode,
            PropertyPath::FrameInnerGlowColor => PropertyPathJson::FrameInnerGlowColor,
            PropertyPath::FrameInnerGlowOpacity => PropertyPathJson::FrameInnerGlowOpacity,
            PropertyPath::FrameInnerGlowChoke => PropertyPathJson::FrameInnerGlowChoke,
            PropertyPath::FrameInnerGlowSize => PropertyPathJson::FrameInnerGlowSize,
            PropertyPath::FrameInnerGlowSource => PropertyPathJson::FrameInnerGlowSource,
            PropertyPath::FrameInnerGlowNoise => PropertyPathJson::FrameInnerGlowNoise,
            PropertyPath::FrameBevelEnabled => PropertyPathJson::FrameBevelEnabled,
            PropertyPath::FrameBevelStyle => PropertyPathJson::FrameBevelStyle,
            PropertyPath::FrameBevelTechnique => PropertyPathJson::FrameBevelTechnique,
            PropertyPath::FrameBevelDepth => PropertyPathJson::FrameBevelDepth,
            PropertyPath::FrameBevelDirection => PropertyPathJson::FrameBevelDirection,
            PropertyPath::FrameBevelSize => PropertyPathJson::FrameBevelSize,
            PropertyPath::FrameBevelSoften => PropertyPathJson::FrameBevelSoften,
            PropertyPath::FrameBevelAngle => PropertyPathJson::FrameBevelAngle,
            PropertyPath::FrameBevelAltitude => PropertyPathJson::FrameBevelAltitude,
            PropertyPath::FrameBevelHighlightColor => PropertyPathJson::FrameBevelHighlightColor,
            PropertyPath::FrameBevelShadowColor => PropertyPathJson::FrameBevelShadowColor,
            PropertyPath::FrameBevelHighlightOpacity => {
                PropertyPathJson::FrameBevelHighlightOpacity
            }
            PropertyPath::FrameBevelShadowOpacity => PropertyPathJson::FrameBevelShadowOpacity,
            PropertyPath::FrameSatinEnabled => PropertyPathJson::FrameSatinEnabled,
            PropertyPath::FrameSatinBlendMode => PropertyPathJson::FrameSatinBlendMode,
            PropertyPath::FrameSatinColor => PropertyPathJson::FrameSatinColor,
            PropertyPath::FrameSatinOpacity => PropertyPathJson::FrameSatinOpacity,
            PropertyPath::FrameSatinAngle => PropertyPathJson::FrameSatinAngle,
            PropertyPath::FrameSatinDistance => PropertyPathJson::FrameSatinDistance,
            PropertyPath::FrameSatinSize => PropertyPathJson::FrameSatinSize,
            PropertyPath::FrameSatinInvert => PropertyPathJson::FrameSatinInvert,
            PropertyPath::FrameFeatherEnabled => PropertyPathJson::FrameFeatherEnabled,
            PropertyPath::FrameFeatherWidth => PropertyPathJson::FrameFeatherWidth,
            PropertyPath::FrameFeatherCornerType => PropertyPathJson::FrameFeatherCornerType,
            PropertyPath::FrameFeatherNoise => PropertyPathJson::FrameFeatherNoise,
            PropertyPath::FrameFeatherChoke => PropertyPathJson::FrameFeatherChoke,
            PropertyPath::FrameDirectionalFeatherEnabled => {
                PropertyPathJson::FrameDirectionalFeatherEnabled
            }
            PropertyPath::FrameDirectionalFeatherLeftWidth => {
                PropertyPathJson::FrameDirectionalFeatherLeftWidth
            }
            PropertyPath::FrameDirectionalFeatherRightWidth => {
                PropertyPathJson::FrameDirectionalFeatherRightWidth
            }
            PropertyPath::FrameDirectionalFeatherTopWidth => {
                PropertyPathJson::FrameDirectionalFeatherTopWidth
            }
            PropertyPath::FrameDirectionalFeatherBottomWidth => {
                PropertyPathJson::FrameDirectionalFeatherBottomWidth
            }
            PropertyPath::FrameDirectionalFeatherAngle => {
                PropertyPathJson::FrameDirectionalFeatherAngle
            }
            PropertyPath::FrameDirectionalFeatherNoise => {
                PropertyPathJson::FrameDirectionalFeatherNoise
            }
            PropertyPath::FrameDirectionalFeatherChoke => {
                PropertyPathJson::FrameDirectionalFeatherChoke
            }
            PropertyPath::FrameBlendMode => PropertyPathJson::FrameBlendMode,
            PropertyPath::NextTextFrame => PropertyPathJson::NextTextFrame,
            PropertyPath::PreviousTextFrame => PropertyPathJson::PreviousTextFrame,
            // W3.A1 — table cell properties.
            PropertyPath::CellFillColor => PropertyPathJson::CellFillColor,
            PropertyPath::CellFillTint => PropertyPathJson::CellFillTint,
            PropertyPath::CellInsetTop => PropertyPathJson::CellInsetTop,
            PropertyPath::CellInsetLeft => PropertyPathJson::CellInsetLeft,
            PropertyPath::CellInsetBottom => PropertyPathJson::CellInsetBottom,
            PropertyPath::CellInsetRight => PropertyPathJson::CellInsetRight,
            PropertyPath::CellVerticalJustification => {
                PropertyPathJson::CellVerticalJustification
            }
            // Aftercare-A — table dimensions (read-only).
            PropertyPath::TableRowCount => PropertyPathJson::TableRowCount,
            PropertyPath::TableColumnCount => PropertyPathJson::TableColumnCount,
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
            PropertyPathJson::FrameGradientFillAngle => PropertyPath::FrameGradientFillAngle,
            PropertyPathJson::FrameGradientFillLength => PropertyPath::FrameGradientFillLength,
            PropertyPathJson::FrameGradientStrokeAngle => PropertyPath::FrameGradientStrokeAngle,
            PropertyPathJson::FrameGradientStrokeLength => {
                PropertyPath::FrameGradientStrokeLength
            }
            PropertyPathJson::PathOpenAt => PropertyPath::PathOpenAt,
            PropertyPathJson::OutlineStroke => PropertyPath::OutlineStroke,
            PropertyPathJson::OffsetPath => PropertyPath::OffsetPath,
            PropertyPathJson::SimplifyPath => PropertyPath::SimplifyPath,
            PropertyPathJson::PageBounds => PropertyPath::PageBounds,
            PropertyPathJson::FrameGradientFeather => PropertyPath::FrameGradientFeather,
            PropertyPathJson::CharacterFontFamily => PropertyPath::CharacterFontFamily,
            PropertyPathJson::CharacterFontStyle => PropertyPath::CharacterFontStyle,
            PropertyPathJson::CharacterKerningMethod => PropertyPath::CharacterKerningMethod,
            PropertyPathJson::CharacterCase => PropertyPath::CharacterCase,
            PropertyPathJson::CharacterPosition => PropertyPath::CharacterPosition,
            PropertyPathJson::CharacterLanguage => PropertyPath::CharacterLanguage,
            PropertyPathJson::CharacterBaselineShift => PropertyPath::CharacterBaselineShift,
            PropertyPathJson::CharacterHorizontalScale => PropertyPath::CharacterHorizontalScale,
            PropertyPathJson::CharacterVerticalScale => PropertyPath::CharacterVerticalScale,
            PropertyPathJson::CharacterSkew => PropertyPath::CharacterSkew,
            PropertyPathJson::CharacterUnderline => PropertyPath::CharacterUnderline,
            PropertyPathJson::CharacterStrikethru => PropertyPath::CharacterStrikethru,
            PropertyPathJson::CharacterLigatures => PropertyPath::CharacterLigatures,
            PropertyPathJson::CharacterOtfFeatures => PropertyPath::CharacterOtfFeatures,
            PropertyPathJson::ParagraphLeftIndent => PropertyPath::ParagraphLeftIndent,
            PropertyPathJson::ParagraphRightIndent => PropertyPath::ParagraphRightIndent,
            PropertyPathJson::ParagraphDropCapCharacters => {
                PropertyPath::ParagraphDropCapCharacters
            }
            PropertyPathJson::ParagraphDropCapLines => PropertyPath::ParagraphDropCapLines,
            PropertyPathJson::ParagraphHyphenation => PropertyPath::ParagraphHyphenation,
            PropertyPathJson::ParagraphKeepLinesTogether => {
                PropertyPath::ParagraphKeepLinesTogether
            }
            PropertyPathJson::ParagraphKeepWithNext => PropertyPath::ParagraphKeepWithNext,
            PropertyPathJson::ParagraphRuleAbove => PropertyPath::ParagraphRuleAbove,
            PropertyPathJson::ParagraphRuleBelow => PropertyPath::ParagraphRuleBelow,
            PropertyPathJson::ParagraphTabStops => PropertyPath::ParagraphTabStops,
            PropertyPathJson::ParagraphListType => PropertyPath::ParagraphListType,
            PropertyPathJson::ParagraphBulletCharacter => PropertyPath::ParagraphBulletCharacter,
            PropertyPathJson::ParagraphNumberingFormat => PropertyPath::ParagraphNumberingFormat,
            // W0.3.
            PropertyPathJson::TextFrameColumnCount => PropertyPath::TextFrameColumnCount,
            PropertyPathJson::TextFrameColumnGutter => PropertyPath::TextFrameColumnGutter,
            PropertyPathJson::TextFrameColumnBalance => PropertyPath::TextFrameColumnBalance,
            PropertyPathJson::TextFrameVerticalJustification => {
                PropertyPath::TextFrameVerticalJustification
            }
            PropertyPathJson::TextFrameAutoSizing => PropertyPath::TextFrameAutoSizing,
            PropertyPathJson::TextFrameFirstBaseline => PropertyPath::TextFrameFirstBaseline,
            PropertyPathJson::TextWrapInvert => PropertyPath::TextWrapInvert,
            PropertyPathJson::FrameFittingReferencePoint => {
                PropertyPath::FrameFittingReferencePoint
            }
            PropertyPathJson::FrameAutoFit => PropertyPath::FrameAutoFit,
            PropertyPathJson::FrameStrokeType => PropertyPath::FrameStrokeType,
            PropertyPathJson::FrameStrokeJoin => PropertyPath::FrameStrokeJoin,
            PropertyPathJson::FrameStrokeMiterLimit => PropertyPath::FrameStrokeMiterLimit,
            PropertyPathJson::FrameStrokeAlignment => PropertyPath::FrameStrokeAlignment,
            PropertyPathJson::FrameStrokeGapColor => PropertyPath::FrameStrokeGapColor,
            PropertyPathJson::FrameStrokeGapTint => PropertyPath::FrameStrokeGapTint,
            PropertyPathJson::FrameCornerOptionTopLeft => {
                PropertyPath::FrameCornerOptionTopLeft
            }
            PropertyPathJson::FrameCornerOptionTopRight => {
                PropertyPath::FrameCornerOptionTopRight
            }
            PropertyPathJson::FrameCornerOptionBottomLeft => {
                PropertyPath::FrameCornerOptionBottomLeft
            }
            PropertyPathJson::FrameCornerOptionBottomRight => {
                PropertyPath::FrameCornerOptionBottomRight
            }
            PropertyPathJson::FrameCornerRadiusTopLeft => {
                PropertyPath::FrameCornerRadiusTopLeft
            }
            PropertyPathJson::FrameCornerRadiusTopRight => {
                PropertyPath::FrameCornerRadiusTopRight
            }
            PropertyPathJson::FrameCornerRadiusBottomLeft => {
                PropertyPath::FrameCornerRadiusBottomLeft
            }
            PropertyPathJson::FrameCornerRadiusBottomRight => {
                PropertyPath::FrameCornerRadiusBottomRight
            }
            PropertyPathJson::FrameRotationAngle => PropertyPath::FrameRotationAngle,
            PropertyPathJson::FrameScaleX => PropertyPath::FrameScaleX,
            PropertyPathJson::FrameScaleY => PropertyPath::FrameScaleY,
            PropertyPathJson::FrameFlipH => PropertyPath::FrameFlipH,
            PropertyPathJson::FrameFlipV => PropertyPath::FrameFlipV,
            PropertyPathJson::FrameOverprintFill => PropertyPath::FrameOverprintFill,
            PropertyPathJson::FrameOverprintStroke => PropertyPath::FrameOverprintStroke,
            // W0.4 — transparency effects.
            PropertyPathJson::FrameInnerShadowEnabled => PropertyPath::FrameInnerShadowEnabled,
            PropertyPathJson::FrameInnerShadowBlendMode => PropertyPath::FrameInnerShadowBlendMode,
            PropertyPathJson::FrameInnerShadowColor => PropertyPath::FrameInnerShadowColor,
            PropertyPathJson::FrameInnerShadowOpacity => PropertyPath::FrameInnerShadowOpacity,
            PropertyPathJson::FrameInnerShadowAngle => PropertyPath::FrameInnerShadowAngle,
            PropertyPathJson::FrameInnerShadowDistance => PropertyPath::FrameInnerShadowDistance,
            PropertyPathJson::FrameInnerShadowSize => PropertyPath::FrameInnerShadowSize,
            PropertyPathJson::FrameInnerShadowChoke => PropertyPath::FrameInnerShadowChoke,
            PropertyPathJson::FrameInnerShadowNoise => PropertyPath::FrameInnerShadowNoise,
            PropertyPathJson::FrameOuterGlowEnabled => PropertyPath::FrameOuterGlowEnabled,
            PropertyPathJson::FrameOuterGlowBlendMode => PropertyPath::FrameOuterGlowBlendMode,
            PropertyPathJson::FrameOuterGlowColor => PropertyPath::FrameOuterGlowColor,
            PropertyPathJson::FrameOuterGlowOpacity => PropertyPath::FrameOuterGlowOpacity,
            PropertyPathJson::FrameOuterGlowSpread => PropertyPath::FrameOuterGlowSpread,
            PropertyPathJson::FrameOuterGlowSize => PropertyPath::FrameOuterGlowSize,
            PropertyPathJson::FrameOuterGlowNoise => PropertyPath::FrameOuterGlowNoise,
            PropertyPathJson::FrameInnerGlowEnabled => PropertyPath::FrameInnerGlowEnabled,
            PropertyPathJson::FrameInnerGlowBlendMode => PropertyPath::FrameInnerGlowBlendMode,
            PropertyPathJson::FrameInnerGlowColor => PropertyPath::FrameInnerGlowColor,
            PropertyPathJson::FrameInnerGlowOpacity => PropertyPath::FrameInnerGlowOpacity,
            PropertyPathJson::FrameInnerGlowChoke => PropertyPath::FrameInnerGlowChoke,
            PropertyPathJson::FrameInnerGlowSize => PropertyPath::FrameInnerGlowSize,
            PropertyPathJson::FrameInnerGlowSource => PropertyPath::FrameInnerGlowSource,
            PropertyPathJson::FrameInnerGlowNoise => PropertyPath::FrameInnerGlowNoise,
            PropertyPathJson::FrameBevelEnabled => PropertyPath::FrameBevelEnabled,
            PropertyPathJson::FrameBevelStyle => PropertyPath::FrameBevelStyle,
            PropertyPathJson::FrameBevelTechnique => PropertyPath::FrameBevelTechnique,
            PropertyPathJson::FrameBevelDepth => PropertyPath::FrameBevelDepth,
            PropertyPathJson::FrameBevelDirection => PropertyPath::FrameBevelDirection,
            PropertyPathJson::FrameBevelSize => PropertyPath::FrameBevelSize,
            PropertyPathJson::FrameBevelSoften => PropertyPath::FrameBevelSoften,
            PropertyPathJson::FrameBevelAngle => PropertyPath::FrameBevelAngle,
            PropertyPathJson::FrameBevelAltitude => PropertyPath::FrameBevelAltitude,
            PropertyPathJson::FrameBevelHighlightColor => PropertyPath::FrameBevelHighlightColor,
            PropertyPathJson::FrameBevelShadowColor => PropertyPath::FrameBevelShadowColor,
            PropertyPathJson::FrameBevelHighlightOpacity => {
                PropertyPath::FrameBevelHighlightOpacity
            }
            PropertyPathJson::FrameBevelShadowOpacity => PropertyPath::FrameBevelShadowOpacity,
            PropertyPathJson::FrameSatinEnabled => PropertyPath::FrameSatinEnabled,
            PropertyPathJson::FrameSatinBlendMode => PropertyPath::FrameSatinBlendMode,
            PropertyPathJson::FrameSatinColor => PropertyPath::FrameSatinColor,
            PropertyPathJson::FrameSatinOpacity => PropertyPath::FrameSatinOpacity,
            PropertyPathJson::FrameSatinAngle => PropertyPath::FrameSatinAngle,
            PropertyPathJson::FrameSatinDistance => PropertyPath::FrameSatinDistance,
            PropertyPathJson::FrameSatinSize => PropertyPath::FrameSatinSize,
            PropertyPathJson::FrameSatinInvert => PropertyPath::FrameSatinInvert,
            PropertyPathJson::FrameFeatherEnabled => PropertyPath::FrameFeatherEnabled,
            PropertyPathJson::FrameFeatherWidth => PropertyPath::FrameFeatherWidth,
            PropertyPathJson::FrameFeatherCornerType => PropertyPath::FrameFeatherCornerType,
            PropertyPathJson::FrameFeatherNoise => PropertyPath::FrameFeatherNoise,
            PropertyPathJson::FrameFeatherChoke => PropertyPath::FrameFeatherChoke,
            PropertyPathJson::FrameDirectionalFeatherEnabled => {
                PropertyPath::FrameDirectionalFeatherEnabled
            }
            PropertyPathJson::FrameDirectionalFeatherLeftWidth => {
                PropertyPath::FrameDirectionalFeatherLeftWidth
            }
            PropertyPathJson::FrameDirectionalFeatherRightWidth => {
                PropertyPath::FrameDirectionalFeatherRightWidth
            }
            PropertyPathJson::FrameDirectionalFeatherTopWidth => {
                PropertyPath::FrameDirectionalFeatherTopWidth
            }
            PropertyPathJson::FrameDirectionalFeatherBottomWidth => {
                PropertyPath::FrameDirectionalFeatherBottomWidth
            }
            PropertyPathJson::FrameDirectionalFeatherAngle => {
                PropertyPath::FrameDirectionalFeatherAngle
            }
            PropertyPathJson::FrameDirectionalFeatherNoise => {
                PropertyPath::FrameDirectionalFeatherNoise
            }
            PropertyPathJson::FrameDirectionalFeatherChoke => {
                PropertyPath::FrameDirectionalFeatherChoke
            }
            PropertyPathJson::FrameBlendMode => PropertyPath::FrameBlendMode,
            PropertyPathJson::NextTextFrame => PropertyPath::NextTextFrame,
            PropertyPathJson::PreviousTextFrame => PropertyPath::PreviousTextFrame,
            // W3.A1 — table cell properties.
            PropertyPathJson::CellFillColor => PropertyPath::CellFillColor,
            PropertyPathJson::CellFillTint => PropertyPath::CellFillTint,
            PropertyPathJson::CellInsetTop => PropertyPath::CellInsetTop,
            PropertyPathJson::CellInsetLeft => PropertyPath::CellInsetLeft,
            PropertyPathJson::CellInsetBottom => PropertyPath::CellInsetBottom,
            PropertyPathJson::CellInsetRight => PropertyPath::CellInsetRight,
            PropertyPathJson::CellVerticalJustification => {
                PropertyPath::CellVerticalJustification
            }
            // Aftercare-A — table dimensions (read-only).
            PropertyPathJson::TableRowCount => PropertyPath::TableRowCount,
            PropertyPathJson::TableColumnCount => PropertyPath::TableColumnCount,
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
            | Value::FramePath { .. }
            | Value::PathOpenAt { .. }
            | Value::OutlineStroke { .. }
            | Value::OffsetPath { .. }
            | Value::SimplifyPath { .. }
            | Value::GradientFeather(_)
            // W0.2 — whole-struct / whole-list paragraph payloads,
            // like the gradient-feather struct: no scalar
            // authored-value widget renders them, so they collapse
            // to `None` for this exhaustive conversion.
            | Value::ParagraphRule(_)
            | Value::TabStops(_) => AuthoredValue::None,
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
