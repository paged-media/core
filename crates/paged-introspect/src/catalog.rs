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

/// One attribute of an IDML element. `settable_path`, when `Some`, is the
/// `paged.set` JS path that writes it (and MUST exist in [`PROPERTY_PATHS`] — a
/// test enforces this), so the docs can cross-link an IDML attribute to the
/// scripting surface that mutates it. `None` = read-only/structural.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementAttr {
    pub name: &'static str,
    /// Human type hint, e.g. `"swatch ref"`, `"[t,l,b,r] points"`, `"boolean"`.
    pub type_hint: &'static str,
    pub settable_path: Option<&'static str>,
    pub summary: &'static str,
}

/// One IDML element the parser recognises, with its notable attributes. The
/// `chapter` is the docs IDML-reference section slug (e.g. `"frames-paths"`) so
/// the generated attribute table can sit under, and link back to, its chapter.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementType {
    pub name: &'static str,
    pub chapter: &'static str,
    pub summary: &'static str,
    pub attributes: Vec<ElementAttr>,
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
    /// IDML elements + their attributes (with the scripting path that mutates
    /// each, where settable). Drives the docs' generated attribute tables.
    pub elements: Vec<ElementType>,
}

/// Assemble the catalog. Cheap; called per `describe`.
#[must_use]
pub fn api_catalog() -> ApiCatalog {
    ApiCatalog {
        host_functions: host_functions(),
        id_grammar: id_grammar(),
        settable_paths: settable_path_names(),
        constraints: constraints(),
        elements: elements(),
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
        f!("paged.insertTextFrame", "(pageId, [t,l,b,r])", "string (created id) | null", "author",
           "Create an empty text-pourable frame at page-local point bounds (mints a story) and select it. Returns the new textFrame:<id> address, or null on failure."),
        f!("paged.insertFrame", "(pageId, [t,l,b,r])", "string (created id) | null", "author",
           "Create an empty graphic (non-text) frame and select it; the usual placeImage target. Returns the new frame's kind:id address, or null."),
        f!("paged.insertPage", "(afterPageId?)", "string (page selfId) | null", "author",
           "Append a page after afterPageId (or at the end), inheriting the default master. Returns the new page's selfId (reusable as the next afterPageId), or null."),
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
        f!("paged.pages", "()", "PageSummary[] JSON", "read",
           "Pages with selfId + 1-based index + sizePt. selfId is the page id for insertFrame/insertTextFrame/insertPage (and the afterPageId of insertPage) — the only way a script can obtain a usable page id."),
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
        // --- complete mutation surface: pages & masters ---
        f!("paged.deletePage", "(pageId)", "bool", "author", "Delete a page."),
        f!("paged.duplicatePage", "(pageId)", "string | null", "author",
           "Duplicate a single-page spread after the source; returns the new page selfId."),
        f!("paged.resizePage", "(pageId, [t,l,b,r])", "bool", "write", "Set a page's GeometricBounds in page-inner points."),
        f!("paged.applyMasterToPage", "(pageId, masterId?)", "bool", "write", "Apply a master to a page (omit/null detaches)."),
        // --- frames & groups ---
        f!("paged.deleteElement", "(id)", "bool", "author", "Delete a page item (kind:id address or bare self id)."),
        f!("paged.dissolveGroup", "(groupId)", "bool", "author", "Ungroup; members return to the group's paint slot."),
        f!("paged.moveFrame", "(frameId, [a,b,c,d,tx,ty])", "bool", "write", "Set a frame's affine placement transform."),
        f!("paged.resizeFrame", "(frameId, [t,l,b,r])", "bool", "write", "Set a frame's content-box bounds (re-paginating resize)."),
        f!("paged.linkFrames", "(fromId, toId)", "bool", "author", "Thread fromId's overflow into the empty frame toId."),
        f!("paged.unlinkFrames", "(frameId)", "bool", "author", "Break the text thread leaving a frame."),
        // --- shape inserts (return the created kind:id address) ---
        f!("paged.insertLine", "(pageId, [x1,y1], [x2,y2])", "string | null", "author", "Insert a two-anchor open GraphicLine."),
        f!("paged.insertOval", "(pageId, [t,l,b,r])", "string | null", "author", "Insert an Oval."),
        f!("paged.insertPath", "(pageId, anchors, open, smooth?)", "string | null", "author",
           "Insert an arbitrary path; anchors = [{anchor:[x,y],left:[x,y],right:[x,y]}, …]."),
        // --- path-point editing ---
        f!("paged.pathPointInsert", "(elemId, index, anchor, subpathStarts?)", "bool", "author", "Insert an anchor into a path's flat PathPointArray at index."),
        f!("paged.pathPointRemove", "(elemId, index)", "bool", "author", "Remove the anchor at flat index."),
        f!("paged.pathPointCurveType", "(elemId, index, smooth)", "bool", "write", "Toggle an anchor between corner and smooth."),
        f!("paged.pathPointSet", "(elemId, index, role, [x,y])", "bool", "write", "Write one Bezier handle (role = anchor|left|right)."),
        f!("paged.pathOpenAt", "(elemId, index)", "bool", "write", "Cut the path at the anchor at flat index."),
        f!("paged.outlineStroke", "(elemId, width, cap, join, miter)", "bool", "write", "Replace the path with its stroke-expansion outline."),
        f!("paged.offsetPath", "(elemId, delta, join, miter)", "bool", "write", "Inset (delta<0) / outset (delta>0) a single closed contour."),
        f!("paged.simplifyPath", "(elemId, tolerance)", "bool", "write", "Re-express the path with fewer anchors within tolerance pt."),
        f!("paged.pathfinderBoolean", "(keptId, [otherIds], kind)", "bool", "author",
           "Pathfinder boolean (kind = union|intersect|subtract|exclude)."),
        // --- fields & images ---
        f!("paged.insertField", "(storyId, offset, fieldKind)", "bool", "author",
           "Insert a field marker; fieldKind = \"pageNumber\" | \"nextPageNumber\" | { placeholder: { plugin, key, value? } }."),
        f!("paged.setFieldValue", "(storyId, offset, value?)", "bool", "write", "Update a placeholder field's cached display value (null ⇒ unresolved)."),
        f!("paged.replaceImageBytes", "(frameId, bytes?)", "bool", "write", "Commit inline image bytes (number[] of u8) on a graphic frame; null clears."),
        // --- tables ---
        f!("paged.insertTable", "(storyId, spec)", "string | null", "author",
           "Create a <Table> at the end of a story; spec = { rows, cols, headerRows?, footerRows?, columnWidths?, rowHeights? }. Returns the table id."),
        f!("paged.setRowHeight", "(storyId, tableId, row, height?)", "bool", "write", "Set/clear a table row height in pt."),
        f!("paged.setColumnWidth", "(storyId, tableId, col, width?)", "bool", "write", "Set/clear a table column width in pt."),
        f!("paged.insertTableRow", "(storyId, tableId, at)", "bool", "author", "Insert an empty body row at index."),
        f!("paged.deleteTableRow", "(storyId, tableId, at)", "bool", "author", "Delete the body row at index."),
        f!("paged.insertTableColumn", "(storyId, tableId, at)", "bool", "author", "Insert an empty column at index."),
        f!("paged.deleteTableColumn", "(storyId, tableId, at)", "bool", "author", "Delete the column at index."),
        f!("paged.insertHeaderRow", "(storyId, tableId)", "bool", "author", "Insert a header-band row."),
        f!("paged.removeHeaderRow", "(storyId, tableId)", "bool", "author", "Remove the first header row."),
        f!("paged.insertFooterRow", "(storyId, tableId)", "bool", "author", "Insert a footer-band row."),
        f!("paged.removeFooterRow", "(storyId, tableId)", "bool", "author", "Remove the last footer row."),
        f!("paged.setCellSpan", "(storyId, tableId, row, col, rowSpan, columnSpan)", "bool", "write", "Set a cell's row/column span."),
        // --- style CRUD (create returns the new id) ---
        f!("paged.createParagraphStyle", "({id?,name?,basedOn?})", "string | null", "author", "Create a paragraph style; returns its selfId."),
        f!("paged.renameParagraphStyle", "(styleId, name)", "bool", "write", "Rename a paragraph style."),
        f!("paged.deleteParagraphStyle", "(styleId)", "bool", "author", "Delete a paragraph style."),
        f!("paged.createCharacterStyle", "({id?,name?,basedOn?})", "string | null", "author", "Create a character style; returns its selfId."),
        f!("paged.renameCharacterStyle", "(styleId, name)", "bool", "write", "Rename a character style."),
        f!("paged.deleteCharacterStyle", "(styleId)", "bool", "author", "Delete a character style."),
        f!("paged.createObjectStyle", "({id?,name?,basedOn?})", "string | null", "author", "Create an object style; returns its selfId."),
        f!("paged.renameObjectStyle", "(styleId, name)", "bool", "write", "Rename an object style."),
        f!("paged.deleteObjectStyle", "(styleId)", "bool", "author", "Delete an object style."),
        f!("paged.createCellStyle", "({id?,name?,basedOn?})", "string | null", "author", "Create a cell style; returns its selfId."),
        f!("paged.renameCellStyle", "(styleId, name)", "bool", "write", "Rename a cell style."),
        f!("paged.deleteCellStyle", "(styleId)", "bool", "author", "Delete a cell style."),
        f!("paged.createTableStyle", "({id?,name?,basedOn?})", "string | null", "author", "Create a table style; returns its selfId."),
        f!("paged.renameTableStyle", "(styleId, name)", "bool", "write", "Rename a table style."),
        f!("paged.deleteTableStyle", "(styleId)", "bool", "author", "Delete a table style."),
        f!("paged.setStyleProperty", "(collection, styleId, path, value)", "bool", "write",
           "Set one property on a style definition (collection = paragraph|character|object|cell|table; path = a settablePaths name)."),
        // --- numbering lists ---
        f!("paged.createNumberingList", "(spec)", "string | null", "author", "Create a <NumberingList>; returns its id."),
        f!("paged.editNumberingList", "(listId, spec)", "bool", "write", "Edit a <NumberingList>."),
        f!("paged.deleteNumberingList", "(listId)", "bool", "author", "Delete a <NumberingList>."),
        // --- sections ---
        f!("paged.insertSection", "(pageId, {prefix?,style?,start?})", "bool", "author", "Anchor a <Section> at a page."),
        f!("paged.editSection", "(sectionId, {prefix?,style?,start?})", "bool", "write", "Edit a <Section>; prefix/start are tri-state (omit ⇒ leave, null ⇒ clear)."),
        f!("paged.deleteSection", "(sectionId)", "bool", "author", "Delete a <Section>."),
        // --- conditions ---
        f!("paged.setConditionVisible", "(conditionId, visible)", "bool", "write", "Flip a condition's visibility."),
        f!("paged.activateConditionSet", "(setId)", "bool", "write", "Activate one <ConditionSet> (\"show only this set\")."),
        // --- layers ---
        f!("paged.layerInsert", "(position, name)", "bool", "author", "Append a layer at the zero-based stacking index."),
        f!("paged.layerRemove", "(layerId)", "bool", "author", "Remove a layer."),
        f!("paged.layerMove", "(layerId, newIndex)", "bool", "write", "Reorder a layer to a new zero-based index."),
        // --- guides ---
        f!("paged.insertGuide", "(spreadId, orientation, position, pageIndex?)", "bool", "author", "Insert a ruler guide (orientation = vertical|horizontal)."),
        f!("paged.moveGuide", "(guideId, position)", "bool", "write", "Move a guide along its perpendicular axis."),
        f!("paged.deleteGuide", "(guideId)", "bool", "author", "Delete a guide."),
        // --- document defaults & colour management ---
        f!("paged.setDocumentDefaults", "({fill?,stroke?,weight?})", "bool", "write", "Set the new-object fill/stroke/weight defaults (whole-triple)."),
        f!("paged.setColorSettings", "({cmykProfileName?,rgbPolicy?,intent?,bpc?})", "bool", "write", "Replace the document colour-management settings."),
        f!("paged.setProofSetup", "({profileName?,simulatePaperWhite?,intent?})", "bool", "write", "Soft-proofing configuration (profileName null turns proofing off)."),
        f!("paged.importSwatchLibrary", "(bytes, groupName?)", "bool", "author", "Import an .ase swatch library (bytes = number[]) as one undoable op."),
        f!("paged.setInkSetting", "(spotId, {convertToProcess?,aliasTo?})", "bool", "write", "Replace one ink's output-time settings."),
        f!("paged.setUseStandardLabForSpots", "(enabled)", "bool", "write", "Prefer spots' Lab primary over their CMYK alternate in previews."),
        // --- plugin metadata & batch ---
        f!("paged.setPluginMetadata", "(elemId, key, value?, caller?)", "bool", "write", "Write one Label key/value pair on a leaf page item (value null deletes)."),
        f!("paged.batch", "([mutations])", "bool", "author", "Apply an array of { op, args } mutation objects as ONE undoable step."),
        // --- selection setters (application state, NOT undoable) ---
        f!("paged.setElementSelection", "([id, ...])", "bool", "write", "Replace the element selection with the parseable ids."),
        f!("paged.clearSelection", "()", "bool", "write", "Clear the element selection."),
        f!("paged.setContentSelection", "({storyId,start,end} | null)", "bool", "write", "Set or clear the text caret/range."),
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
        "Reads return JSON STRINGS, not live values: JSON.parse(paged.selection()/tree()/stories()/pages()/…) before you index. Parsed elements are { kind, id } objects — address one as the string `${kind}:${id}` (e.g. textFrame:u3) for paged.set / inspect / get.",
        "Property writes (paged.set) return a boolean: true = applied, false = rejected (unknown id/path, bad value, or a failed precondition). The structural insert fns are the exception — see below. Always check the result and adapt.",
        "Writes go through the editor's Operation channel, so paged.undo()/paged.redo() work exactly as in the UI.",
        "Runtime budgets: ~10M loop iterations, recursion depth 512, and a ~2s wall-clock checked at every host call. Runaway scripts are aborted (non-catchable).",
        "insertFrame/insertTextFrame return the new element's kind:id address (and auto-select it); insertPage returns the new page's selfId. Pass the returned id straight to paged.set / placeImage / insertText (use paged.pages() for a page id, paged.stories() for the minted story).",
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

/// The IDML elements the parser recognises, with their notable attributes. Hand
/// curated against `paged-parse` (spread.rs page items, plus structural Page /
/// Spread / Layer / Story and text-range styling) and cross-referenced to
/// [`PROPERTY_PATHS`] for the `settable_path` that mutates each — a test asserts
/// every cited path resolves, so the IDML⇄scripting cross-links can't dangle.
fn elements() -> Vec<ElementType> {
    const fn attr(name: &'static str, type_hint: &'static str, settable_path: Option<&'static str>, summary: &'static str) -> ElementAttr {
        ElementAttr { name, type_hint, settable_path, summary }
    }
    vec![
        ElementType {
            name: "TextFrame",
            chapter: "frames-paths",
            summary: "A frame that pours a story. Geometry + fill/stroke like any page item, plus text-frame preferences (columns, inset, vertical justification).",
            attributes: vec![
                attr("Self", "id", None, "The element's IDML id; how everything references it."),
                attr("ParentStory", "story ref", None, "The story this frame pours; the threading link, not a settable property."),
                attr("GeometricBounds", "[t,l,b,r] points", Some("frameBounds"), "Page-local bounds in points."),
                attr("ItemTransform", "affine [a,b,c,d,e,f]", Some("frameTransform"), "The frame's affine placement transform."),
                attr("FillColor", "swatch ref", Some("frameFillColor"), "Fill swatch (e.g. Color/Red)."),
                attr("StrokeColor", "swatch ref", Some("frameStrokeColor"), "Stroke swatch."),
                attr("StrokeWeight", "points", Some("frameStrokeWeight"), "Stroke weight in points."),
            ],
        },
        ElementType {
            name: "Rectangle",
            chapter: "frames-paths",
            summary: "A rectangular graphic frame — a vector shape that can also hold a placed image.",
            attributes: vec![
                attr("Self", "id", None, "Element id."),
                attr("GeometricBounds", "[t,l,b,r] points", Some("frameBounds"), "Page-local bounds in points."),
                attr("ItemTransform", "affine", Some("frameTransform"), "Affine placement transform."),
                attr("FillColor", "swatch ref", Some("frameFillColor"), "Fill swatch."),
                attr("FillTint", "0–100", Some("frameFillTint"), "Fill tint percentage."),
                attr("StrokeColor", "swatch ref", Some("frameStrokeColor"), "Stroke swatch."),
                attr("StrokeWeight", "points", Some("frameStrokeWeight"), "Stroke weight."),
            ],
        },
        ElementType {
            name: "Oval",
            chapter: "frames-paths",
            summary: "An elliptical graphic frame.",
            attributes: vec![
                attr("GeometricBounds", "[t,l,b,r] points", Some("frameBounds"), "Bounding box of the ellipse."),
                attr("ItemTransform", "affine", Some("frameTransform"), "Affine placement transform."),
                attr("FillColor", "swatch ref", Some("frameFillColor"), "Fill swatch."),
                attr("StrokeColor", "swatch ref", Some("frameStrokeColor"), "Stroke swatch."),
            ],
        },
        ElementType {
            name: "Polygon",
            chapter: "frames-paths",
            summary: "An arbitrary closed vector shape; may carry multiple GeometryPathType contours (compound paths).",
            attributes: vec![
                attr("GeometricBounds", "[t,l,b,r] points", Some("frameBounds"), "Bounding box."),
                attr("ItemTransform", "affine", Some("frameTransform"), "Affine placement transform."),
                attr("FillColor", "swatch ref", Some("frameFillColor"), "Fill swatch."),
                attr("StrokeColor", "swatch ref", Some("frameStrokeColor"), "Stroke swatch."),
            ],
        },
        ElementType {
            name: "GraphicLine",
            chapter: "frames-paths",
            summary: "A straight or curved open path.",
            attributes: vec![
                attr("GeometricBounds", "[t,l,b,r] points", Some("frameBounds"), "Bounding box of the line."),
                attr("ItemTransform", "affine", Some("frameTransform"), "Affine placement transform."),
                attr("StrokeColor", "swatch ref", Some("frameStrokeColor"), "Stroke swatch."),
                attr("StrokeWeight", "points", Some("frameStrokeWeight"), "Stroke weight."),
            ],
        },
        ElementType {
            name: "Group",
            chapter: "frames-paths",
            summary: "A grouping container that transforms its children as a unit.",
            attributes: vec![
                attr("Self", "id", None, "Element id."),
                attr("ItemTransform", "affine", Some("frameTransform"), "The group's affine transform, applied to all children."),
            ],
        },
        ElementType {
            name: "Layer",
            chapter: "layers",
            summary: "A document layer; controls visibility, lock, and print state for the items assigned to it.",
            attributes: vec![
                attr("Name", "string", Some("layerName"), "Layer name."),
                attr("Visible", "boolean", Some("layerVisible"), "Whether the layer's items render."),
                attr("Locked", "boolean", Some("layerLocked"), "Whether the layer's items are editable."),
                attr("Printable", "boolean", Some("layerPrintable"), "Whether the layer prints/exports."),
            ],
        },
        ElementType {
            name: "Page",
            chapter: "layout-model",
            summary: "A single page within a spread.",
            attributes: vec![
                attr("Self", "id", None, "Element id; the target for paged.insertTextFrame / insertFrame."),
                attr("GeometricBounds", "[t,l,b,r] points", None, "Page bounds in the spread's coordinate space."),
            ],
        },
        ElementType {
            name: "Spread",
            chapter: "layout-model",
            summary: "A spread: one or more pages laid out together, with its own page-item stacking order.",
            attributes: vec![
                attr("Self", "id", None, "Element id."),
                attr("ItemTransform", "affine", None, "The spread's transform in pasteboard space."),
            ],
        },
        ElementType {
            name: "Story",
            chapter: "stories-text",
            summary: "A flow of text, independent of the frame(s) that pour it. Edited by character offset, not by frame.",
            attributes: vec![
                attr("Self", "id", None, "Story id; the target for paged.insertText / deleteRange / applyStyle."),
            ],
        },
        ElementType {
            name: "ParagraphStyleRange",
            chapter: "styles",
            summary: "A run of paragraphs sharing a paragraph style and overrides, inside a story.",
            attributes: vec![
                attr("AppliedParagraphStyle", "style ref", Some("appliedParagraphStyle"), "The paragraph style applied to the range."),
                attr("Justification", "enum", Some("paragraphJustification"), "Paragraph alignment/justification."),
                attr("SpaceBefore", "points", Some("paragraphSpaceBefore"), "Space above the paragraph."),
                attr("SpaceAfter", "points", Some("paragraphSpaceAfter"), "Space below the paragraph."),
            ],
        },
        ElementType {
            name: "CharacterStyleRange",
            chapter: "styles",
            summary: "A run of characters sharing a character style and overrides, inside a paragraph.",
            attributes: vec![
                attr("AppliedCharacterStyle", "style ref", Some("appliedCharacterStyle"), "The character style applied to the range."),
                attr("PointSize", "points", Some("characterFontSize"), "Font size in points."),
                attr("Leading", "points | Auto", Some("characterLeading"), "Line leading."),
                attr("Tracking", "1/1000 em", Some("characterTracking"), "Letter tracking."),
                attr("FillColor", "swatch ref", Some("characterFillColor"), "Text fill swatch."),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_resolves_and_is_complete() {
        let cat = api_catalog();
        assert_eq!(cat.settable_paths.len(), 179, "settable path count drifted");
        assert!(cat.host_functions.len() >= 20);
        assert!(!cat.elements.is_empty(), "elements section is empty");
        // representative + alias mappings
        assert_eq!(lookup_path("characterFontSize"), Some(P::CharacterFontSize));
        assert_eq!(lookup_path("frameBevel"), Some(P::FrameBevelEnabled));
        assert_eq!(lookup_path("notARealPath"), None);
    }

    /// Every `settable_path` cited by an element attribute must be a real
    /// `paged.set` path — otherwise the docs' IDML⇄scripting cross-link dangles.
    #[test]
    fn element_settable_paths_resolve() {
        for el in elements() {
            for a in &el.attributes {
                if let Some(path) = a.settable_path {
                    assert!(lookup_path(path).is_some(), "{}.{} cites unknown settable path '{path}'", el.name, a.name);
                }
            }
        }
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
