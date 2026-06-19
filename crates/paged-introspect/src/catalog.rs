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

//! Machine-readable description of the `paged.*` scripting surface — and the
//! **single source** for the JS-name → `PropertyPath` mapping.
//!
//! [`PROPERTY_PATHS`] is the one table that both [`lookup_path`] (which backs
//! `paged-script`'s `parse_property_path`, across the crate boundary) and
//! [`api_catalog`] read from. There is no second hand-list to drift: the parser
//! and the catalog cannot disagree about which paths are settable, by
//! construction. Lives here (the neutral, published introspection crate) rather
//! than in `paged-script` so every surface — the Boa bridge, the published
//! `introspect-wasm` `describeCatalog`, the plugin SDK, `state` — projects the
//! one contract. This realizes ADR 005's "one descriptor source feeds introspect
//! + script" for the property surface (ADR 019).
//!
//! This is the API *vocabulary* (layer 1). The conceptual mental model + DTP
//! recipes (layers 2/3) live in the consumer's authoring guide, which refers
//! back to this catalog for exact names.

use paged_mutate::PropertyPath as P;
use serde::Serialize;

/// One `paged.*` / `console.*` host function.
#[derive(Debug, Clone, Serialize)]
pub struct HostFn {
    pub name: &'static str,
    /// Parameter list, e.g. `"(storyId, offset, text)"`.
    pub params: &'static str,
    pub returns: &'static str,
    /// `"read" | "write" | "author" | "history" | "console"`.
    pub kind: &'static str,
    pub summary: &'static str,
}

/// One accepted element-id address form.
#[derive(Debug, Clone, Serialize)]
pub struct IdForm {
    pub form: &'static str,
    pub example: &'static str,
    pub note: &'static str,
}

/// The full capability catalog.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiCatalog {
    pub host_functions: Vec<HostFn>,
    pub id_grammar: Vec<IdForm>,
    /// Property names accepted by `paged.set(id, path, value)` / readable via
    /// `paged.get(id, path)` — derived from [`PROPERTY_PATHS`], so always in
    /// sync with the parser.
    pub settable_paths: Vec<&'static str>,
    pub constraints: Vec<&'static str>,
}

/// Assemble the catalog. Cheap; called per `describe`.
#[must_use]
pub fn api_catalog() -> ApiCatalog {
    ApiCatalog {
        host_functions: host_functions(),
        id_grammar: id_grammar(),
        settable_paths: settable_path_names(),
        constraints: constraints(),
    }
}

/// Resolve a JS property-path name to its `PropertyPath`. The single lookup
/// behind `parse_property_path`; the linear scan is fine for a 179-entry table
/// called at human cadence (one per `paged.set`/`get`).
pub fn lookup_path(name: &str) -> Option<P> {
    PROPERTY_PATHS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, path)| *path)
}

/// The settable path names (catalog projection of [`PROPERTY_PATHS`]).
fn settable_path_names() -> Vec<&'static str> {
    PROPERTY_PATHS.iter().map(|(name, _)| *name).collect()
}

fn host_functions() -> Vec<HostFn> {
    macro_rules! f {
        ($name:literal, $params:literal, $returns:literal, $kind:literal, $summary:literal) => {
            HostFn {
                name: $name,
                params: $params,
                returns: $returns,
                kind: $kind,
                summary: $summary,
            }
        };
    }
    vec![
        // --- writes (property mutation) ---
        f!("paged.set", "(id, path, value)", "bool", "write",
           "Set a property (see settablePaths) on the addressed element. null clears."),
        f!("paged.get", "(id, path)", "value | null", "read",
           "Read one property value of the addressed element."),
        // --- authoring (Stage 1/2) ---
        f!("paged.insertText", "(storyId, offset, text)", "bool", "author",
           "Insert plain text at a story body offset; \\n splits paragraphs."),
        f!("paged.deleteRange", "(storyId, start, end)", "bool", "author",
           "Delete the [start, end) character range of a story."),
        f!("paged.insertTextFrame", "(pageId, [t,l,b,r])", "bool", "author",
           "Create an empty text-pourable frame at page-local point bounds. Mints a story (id NOT returned — see constraints)."),
        f!("paged.insertFrame", "(pageId, [t,l,b,r])", "bool", "author",
           "Create an empty graphic (non-text) frame; the usual placeImage target."),
        f!("paged.insertPage", "(afterPageId?)", "bool", "author",
           "Append a page after afterPageId (or at the end), inheriting the default master."),
        f!("paged.placeImage", "(frameId, uri, fit?)", "bool", "author",
           "Place an image into a frame; fit is an optional fitting mode."),
        f!("paged.applyStyle", "(storyId, start, end, styleRef)", "bool", "author",
           "Apply a paragraph/character style to a story range. Scope inferred from the ref prefix (CharacterStyle/… else Paragraph)."),
        f!("paged.createGroup", "([id, ...])", "bool", "author",
           "Group two-or-more elements; <2 valid members returns false."),
        // --- history ---
        f!("paged.undo", "()", "bool", "history", "Undo the last mutation."),
        f!("paged.redo", "()", "bool", "history", "Redo the last undone mutation."),
        // --- reads / introspection (return JSON strings unless noted) ---
        f!("paged.inspect", "(id)", "ElementProperties JSON", "read",
           "Full property snapshot for one element (or storyRange)."),
        f!("paged.tree", "()", "SceneTreeNode[] JSON", "read",
           "Document hierarchy: spreads → pages → frames."),
        f!("paged.stories", "()", "StorySummary[] JSON", "read",
           "Loaded stories with selfId + characterCount + paragraphCount. The source of valid story ids."),
        f!("paged.layers", "()", "LayerSummary[] JSON", "read", "Document layers."),
        f!("paged.swatches", "()", "SwatchSummary[] JSON", "read", "Colour palette (selfId/name/kind)."),
        f!("paged.paragraphStyles", "()", "ParagraphStyleSummary[] JSON", "read",
           "Paragraph styles — the source of valid styleRefs for applyStyle."),
        f!("paged.characterStyles", "()", "CharacterStyleSummary[] JSON", "read", "Character styles."),
        f!("paged.objectStyles", "()", "ObjectStyleSummary[] JSON", "read", "Object styles."),
        f!("paged.gradients", "()", "GradientSummary[] JSON", "read", "Gradients."),
        f!("paged.colorGroups", "()", "ColorGroupSummary[] JSON", "read", "Colour groups."),
        f!("paged.links", "()", "LinkSummary[] JSON", "read", "Placed-asset links."),
        f!("paged.conditions", "()", "ConditionSummary[] JSON", "read", "Conditional-text conditions."),
        f!("paged.conditionSets", "()", "ConditionSetSummary[] JSON", "read", "Condition sets."),
        f!("paged.collection", "(name)", "Summary[] JSON", "read",
           "Generic typed-collection read by name; unknown name → \"[]\" + warning."),
        f!("paged.documentMeta", "()", "DocumentMeta JSON", "read", "Document metadata (name/creator/modified/page count)."),
        f!("paged.selection", "()", "ElementId[] JSON", "read", "Current element selection."),
        f!("paged.contentSelection", "()", "ContentSelection JSON | null", "read", "Current text caret / range."),
        // --- console (captured into the run output) ---
        f!("console.log", "(...)", "undefined", "console", "Append a line to the captured output log (also warn/error/info)."),
    ]
}

fn id_grammar() -> Vec<IdForm> {
    vec![
        IdForm {
            form: "textFrame:<id>",
            example: "textFrame:u123",
            note: "A text frame. Same scheme: rectangle:/oval:/polygon:/graphicLine:/group:.",
        },
        IdForm {
            form: "group:<id>",
            example: "group:u88",
            note: "A group. `paged.set(\"group:<id>\", \"groupTransform\", [a,b,c,d,tx,ty])` moves it as a unit.",
        },
        IdForm {
            form: "storyRange:<storyId>@<start>..<end>",
            example: "storyRange:Story/u1@0..6",
            note: "A character range within a story (half-open). storyId comes from paged.stories()[].selfId.",
        },
    ]
}

fn constraints() -> Vec<&'static str> {
    vec![
        "Scripts run in Boa (pure ECMAScript), NOT Node: no require/import, fetch, fs, setTimeout, or network. Use only paged.* and console.*.",
        "Every paged.* write returns a boolean: true = applied, false = rejected (unknown id/path, bad value, or a failed precondition). Always check it and adapt.",
        "Writes go through the editor's Operation channel, so paged.undo()/paged.redo() work exactly as in the UI.",
        "Runtime budgets: ~10M loop iterations, recursion depth 512, and a ~2s wall-clock checked at every host call. Runaway scripts are aborted (non-catchable).",
        "insertTextFrame/insertFrame create a new frame (and insertTextFrame mints a story) but return only a boolean — NOT the new id. To fill a freshly-created frame you need its story id: prefer inserting into existing template stories, or re-read paged.stories()/paged.tree() after creating to discover the new id.",
        "Bounds are page-local points in [top, left, bottom, right] order. The document works in points (1/72 inch).",
    ]
}

/// THE single source for `paged.set`/`get` property paths: JS name →
/// `PropertyPath`. `parse_property_path` (via [`lookup_path`]) and the catalog
/// both read this — there is no second list. Order is the engine's own
/// grouping (frame geometry/effects, then text/cell/anchored); the catalog
/// preserves it.
pub const PROPERTY_PATHS: &[(&str, P)] = &[
    ("frameBounds", P::FrameBounds),
    ("frameFillColor", P::FrameFillColor),
    ("frameStrokeColor", P::FrameStrokeColor),
    ("frameStrokeWeight", P::FrameStrokeWeight),
    ("frameOpacity", P::FrameOpacity),
    ("frameTransform", P::FrameTransform),
    ("imageContentTransform", P::ImageContentTransform),
    ("framePathPoint", P::FramePathPoint),
    ("pathPointInsert", P::PathPointInsert),
    ("pathPointRemove", P::PathPointRemove),
    ("pathPointCurveType", P::PathPointCurveType),
    ("layerVisible", P::LayerVisible),
    ("layerLocked", P::LayerLocked),
    ("layerPrintable", P::LayerPrintable),
    ("layerName", P::LayerName),
    ("characterFontSize", P::CharacterFontSize),
    ("characterLeading", P::CharacterLeading),
    ("characterTracking", P::CharacterTracking),
    ("characterFillColor", P::CharacterFillColor),
    ("paragraphSpaceBefore", P::ParagraphSpaceBefore),
    ("paragraphSpaceAfter", P::ParagraphSpaceAfter),
    ("paragraphFirstLineIndent", P::ParagraphFirstLineIndent),
    ("appliedParagraphStyle", P::AppliedParagraphStyle),
    ("appliedCharacterStyle", P::AppliedCharacterStyle),
    ("appliedObjectStyle", P::AppliedObjectStyle),
    ("appliedCellStyle", P::AppliedCellStyle),
    ("appliedTableStyle", P::AppliedTableStyle),
    ("appliedConditions", P::AppliedConditions),
    ("frameInsetSpacing", P::FrameInsetSpacing),
    ("paragraphJustification", P::ParagraphJustification),
    ("paragraphStyleNextStyle", P::ParagraphStyleNextStyle),
    ("paragraphAppliedNumberingList", P::ParagraphAppliedNumberingList),
    ("frameStrokeEndCap", P::FrameStrokeEndCap),
    ("frameStrokeStartArrowhead", P::FrameStrokeStartArrowhead),
    ("frameStrokeEndArrowhead", P::FrameStrokeEndArrowhead),
    ("frameTextWrapMode", P::FrameTextWrapMode),
    ("frameTextWrapOffsets", P::FrameTextWrapOffsets),
    ("frameTextWrapContourType", P::FrameTextWrapContourType),
    ("frameTextWrapContourIncludeInside", P::FrameTextWrapContourIncludeInside),
    ("frameFittingCrops", P::FrameFittingCrops),
    ("frameFittingType", P::FrameFittingType),
    ("frameDropShadow", P::FrameDropShadow),
    ("frameDropShadowMode", P::FrameDropShadowMode),
    ("frameDropShadowXOffset", P::FrameDropShadowXOffset),
    ("frameDropShadowYOffset", P::FrameDropShadowYOffset),
    ("frameDropShadowSize", P::FrameDropShadowSize),
    ("frameDropShadowOpacity", P::FrameDropShadowOpacity),
    ("frameDropShadowColor", P::FrameDropShadowColor),
    ("framePath", P::FramePath),
    ("frameFillTint", P::FrameFillTint),
    ("frameNonprinting", P::FrameNonprinting),
    ("frameGradientFillAngle", P::FrameGradientFillAngle),
    ("frameGradientFillLength", P::FrameGradientFillLength),
    ("frameGradientStrokeAngle", P::FrameGradientStrokeAngle),
    ("frameGradientStrokeLength", P::FrameGradientStrokeLength),
    ("textFrameColumnCount", P::TextFrameColumnCount),
    ("textFrameColumnGutter", P::TextFrameColumnGutter),
    ("textFrameColumnBalance", P::TextFrameColumnBalance),
    ("textFrameVerticalJustification", P::TextFrameVerticalJustification),
    ("textFrameAutoSizing", P::TextFrameAutoSizing),
    ("textFrameFirstBaseline", P::TextFrameFirstBaseline),
    ("frameTextWrapInvert", P::TextWrapInvert),
    ("frameFittingReferencePoint", P::FrameFittingReferencePoint),
    ("frameAutoFit", P::FrameAutoFit),
    ("frameStrokeType", P::FrameStrokeType),
    ("frameStrokeJoin", P::FrameStrokeJoin),
    ("frameStrokeMiterLimit", P::FrameStrokeMiterLimit),
    ("frameStrokeAlignment", P::FrameStrokeAlignment),
    ("frameStrokeGapColor", P::FrameStrokeGapColor),
    ("frameStrokeGapTint", P::FrameStrokeGapTint),
    ("frameStrokeDashArray", P::FrameStrokeDashArray),
    ("frameCornerOptionTopLeft", P::FrameCornerOptionTopLeft),
    ("frameCornerOptionTopRight", P::FrameCornerOptionTopRight),
    ("frameCornerOptionBottomLeft", P::FrameCornerOptionBottomLeft),
    ("frameCornerOptionBottomRight", P::FrameCornerOptionBottomRight),
    ("frameCornerRadiusTopLeft", P::FrameCornerRadiusTopLeft),
    ("frameCornerRadiusTopRight", P::FrameCornerRadiusTopRight),
    ("frameCornerRadiusBottomLeft", P::FrameCornerRadiusBottomLeft),
    ("frameCornerRadiusBottomRight", P::FrameCornerRadiusBottomRight),
    ("frameRotationAngle", P::FrameRotationAngle),
    ("frameScaleX", P::FrameScaleX),
    ("frameScaleY", P::FrameScaleY),
    ("frameFlipH", P::FrameFlipH),
    ("frameFlipV", P::FrameFlipV),
    ("frameOverprintFill", P::FrameOverprintFill),
    ("frameOverprintStroke", P::FrameOverprintStroke),
    ("frameInnerShadow", P::FrameInnerShadowEnabled),
    ("frameInnerShadowBlendMode", P::FrameInnerShadowBlendMode),
    ("frameInnerShadowColor", P::FrameInnerShadowColor),
    ("frameInnerShadowOpacity", P::FrameInnerShadowOpacity),
    ("frameInnerShadowAngle", P::FrameInnerShadowAngle),
    ("frameInnerShadowDistance", P::FrameInnerShadowDistance),
    ("frameInnerShadowSize", P::FrameInnerShadowSize),
    ("frameInnerShadowChoke", P::FrameInnerShadowChoke),
    ("frameInnerShadowNoise", P::FrameInnerShadowNoise),
    ("frameOuterGlow", P::FrameOuterGlowEnabled),
    ("frameOuterGlowBlendMode", P::FrameOuterGlowBlendMode),
    ("frameOuterGlowColor", P::FrameOuterGlowColor),
    ("frameOuterGlowOpacity", P::FrameOuterGlowOpacity),
    ("frameOuterGlowSpread", P::FrameOuterGlowSpread),
    ("frameOuterGlowSize", P::FrameOuterGlowSize),
    ("frameOuterGlowNoise", P::FrameOuterGlowNoise),
    ("frameInnerGlow", P::FrameInnerGlowEnabled),
    ("frameInnerGlowBlendMode", P::FrameInnerGlowBlendMode),
    ("frameInnerGlowColor", P::FrameInnerGlowColor),
    ("frameInnerGlowOpacity", P::FrameInnerGlowOpacity),
    ("frameInnerGlowChoke", P::FrameInnerGlowChoke),
    ("frameInnerGlowSize", P::FrameInnerGlowSize),
    ("frameInnerGlowSource", P::FrameInnerGlowSource),
    ("frameInnerGlowNoise", P::FrameInnerGlowNoise),
    ("frameBevel", P::FrameBevelEnabled),
    ("frameBevelStyle", P::FrameBevelStyle),
    ("frameBevelTechnique", P::FrameBevelTechnique),
    ("frameBevelDepth", P::FrameBevelDepth),
    ("frameBevelDirection", P::FrameBevelDirection),
    ("frameBevelSize", P::FrameBevelSize),
    ("frameBevelSoften", P::FrameBevelSoften),
    ("frameBevelAngle", P::FrameBevelAngle),
    ("frameBevelAltitude", P::FrameBevelAltitude),
    ("frameBevelHighlightColor", P::FrameBevelHighlightColor),
    ("frameBevelShadowColor", P::FrameBevelShadowColor),
    ("frameBevelHighlightOpacity", P::FrameBevelHighlightOpacity),
    ("frameBevelShadowOpacity", P::FrameBevelShadowOpacity),
    ("frameSatin", P::FrameSatinEnabled),
    ("frameSatinBlendMode", P::FrameSatinBlendMode),
    ("frameSatinColor", P::FrameSatinColor),
    ("frameSatinOpacity", P::FrameSatinOpacity),
    ("frameSatinAngle", P::FrameSatinAngle),
    ("frameSatinDistance", P::FrameSatinDistance),
    ("frameSatinSize", P::FrameSatinSize),
    ("frameSatinInvert", P::FrameSatinInvert),
    ("frameFeather", P::FrameFeatherEnabled),
    ("frameFeatherWidth", P::FrameFeatherWidth),
    ("frameFeatherCornerType", P::FrameFeatherCornerType),
    ("frameFeatherNoise", P::FrameFeatherNoise),
    ("frameFeatherChoke", P::FrameFeatherChoke),
    ("frameDirectionalFeather", P::FrameDirectionalFeatherEnabled),
    ("frameDirectionalFeatherLeftWidth", P::FrameDirectionalFeatherLeftWidth),
    ("frameDirectionalFeatherRightWidth", P::FrameDirectionalFeatherRightWidth),
    ("frameDirectionalFeatherTopWidth", P::FrameDirectionalFeatherTopWidth),
    ("frameDirectionalFeatherBottomWidth", P::FrameDirectionalFeatherBottomWidth),
    ("frameDirectionalFeatherAngle", P::FrameDirectionalFeatherAngle),
    ("frameDirectionalFeatherNoise", P::FrameDirectionalFeatherNoise),
    ("frameDirectionalFeatherChoke", P::FrameDirectionalFeatherChoke),
    ("frameBlendMode", P::FrameBlendMode),
    ("cellFillColor", P::CellFillColor),
    ("cellFillTint", P::CellFillTint),
    ("cellInsetTop", P::CellInsetTop),
    ("cellInsetLeft", P::CellInsetLeft),
    ("cellInsetBottom", P::CellInsetBottom),
    ("cellInsetRight", P::CellInsetRight),
    ("cellVerticalJustification", P::CellVerticalJustification),
    ("cellTopEdgeStrokeColor", P::CellTopEdgeStrokeColor),
    ("cellTopEdgeStrokeWeight", P::CellTopEdgeStrokeWeight),
    ("cellTopEdgeStrokeTint", P::CellTopEdgeStrokeTint),
    ("cellBottomEdgeStrokeColor", P::CellBottomEdgeStrokeColor),
    ("cellBottomEdgeStrokeWeight", P::CellBottomEdgeStrokeWeight),
    ("cellBottomEdgeStrokeTint", P::CellBottomEdgeStrokeTint),
    ("cellLeftEdgeStrokeColor", P::CellLeftEdgeStrokeColor),
    ("cellLeftEdgeStrokeWeight", P::CellLeftEdgeStrokeWeight),
    ("cellLeftEdgeStrokeTint", P::CellLeftEdgeStrokeTint),
    ("cellRightEdgeStrokeColor", P::CellRightEdgeStrokeColor),
    ("cellRightEdgeStrokeWeight", P::CellRightEdgeStrokeWeight),
    ("cellRightEdgeStrokeTint", P::CellRightEdgeStrokeTint),
    ("tableRowCount", P::TableRowCount),
    ("tableColumnCount", P::TableColumnCount),
    ("pluginMetadata", P::PluginMetadata),
    ("anchoredPosition", P::AnchoredPosition),
    ("anchorPoint", P::AnchorPoint),
    ("anchoredXOffset", P::AnchoredXOffset),
    ("anchoredYOffset", P::AnchoredYOffset),
    ("anchoredHorizontalReference", P::AnchoredHorizontalReference),
    ("anchoredVerticalReference", P::AnchoredVerticalReference),
    ("anchoredHorizontalAlignment", P::AnchoredHorizontalAlignment),
    ("anchoredVerticalAlignment", P::AnchoredVerticalAlignment),
    ("anchoredSpineRelative", P::AnchoredSpineRelative),
    ("anchoredLockPosition", P::AnchoredLockPosition),
    ("elementVisible", P::ElementVisible),
    ("elementLocked", P::ElementLocked),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_resolves_and_is_complete() {
        let cat = api_catalog();
        assert_eq!(cat.settable_paths.len(), 179, "settable path count drifted");
        assert!(cat.host_functions.len() >= 20);
        // representative + alias mappings
        assert_eq!(lookup_path("characterFontSize"), Some(P::CharacterFontSize));
        assert_eq!(lookup_path("frameBevel"), Some(P::FrameBevelEnabled));
        assert_eq!(lookup_path("notARealPath"), None);
    }

    /// Consistency with the wire enum: every catalog path is a real `PropertyPath`
    /// that has a `PropertyPathJson` mirror (the catalog can't list a phantom).
    /// The catalog uses ergonomic JS aliases (`frameBevel`) while `PropertyPathJson`
    /// uses the wire variant name (`frameBevelEnabled`) — distinct by design — but
    /// they project the *same* underlying variant set.
    #[test]
    fn every_catalog_path_has_a_wire_mirror() {
        for (_, path) in PROPERTY_PATHS {
            let _mirror: crate::descriptor::PropertyPathJson = (*path).into();
        }
    }

    /// The committed `catalog.json` build-time artifact (read by the plugin SDK
    /// sync, `state`'s catalog ingest, and docs) must match `api_catalog()`.
    #[test]
    fn catalog_json_artifact_is_current() {
        let generated = serde_json::to_string_pretty(&api_catalog()).unwrap();
        let committed = include_str!("../catalog.json");
        assert_eq!(
            committed.trim_end(),
            generated.trim_end(),
            "catalog.json is stale — regenerate: \
             cargo run -p paged-introspect --example emit-catalog > crates/paged-introspect/catalog.json"
        );
    }
}
