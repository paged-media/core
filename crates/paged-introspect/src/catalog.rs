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

use crate::schema::*;
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
    /// The wire op vocabulary — every `Mutation` tag a surface can send,
    /// spelled the way it travels (`"insertText"`, not `"InsertText"`).
    ///
    /// This is the population the whole system measures parity in — the
    /// editor's probed capability table, `state`'s op map, the plugin
    /// host — and the catalog could not carry it, because the ops lived
    /// inside the canvas model and this crate is the light, published
    /// one. They live in `paged-wire` now, so the catalog finally
    /// describes both halves of the vocabulary instead of only the
    /// properties.
    pub operations: Vec<String>,
    /// IDML elements + their attributes (with the scripting path that mutates
    /// each, where settable). Drives the docs' generated attribute tables.
    pub elements: Vec<ElementType>,
    /// The eight settable paths whose RAW WIRE spelling differs from the
    /// advertised one, so a consumer can see both instead of meeting the
    /// second by being rejected. See [`wire_alias`]. Empty when the two
    /// vocabularies agree everywhere, which is the state to aim for.
    pub path_aliases: Vec<PathAlias>,
    /// ADR 132 — the typed schema of every settable or readable property,
    /// one row per (address kind, path): the element paths of
    /// `settablePaths`, the paths that live on a layer / page / the
    /// document / a text variable / a style definition, and the
    /// op-backed virtual paths `Mutation::Set` resolves. `settablePaths`
    /// stays the name projection of the element rows.
    pub property_schemas: Vec<crate::virtual_paths::SchemaRow>,
}

/// One settable path that answers to two names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathAlias {
    /// The advertised name — what `settablePaths` lists and the docs print.
    pub name: &'static str,
    /// The raw-wire spelling: serde's camelCase of the variant, which is
    /// what `Mutation::SetElementProperty` carries and what
    /// `paged.inspect`'s descriptor emits.
    pub wire: String,
}

/// Assemble the catalog. Cheap; called per `describe`.
#[must_use]
pub fn api_catalog() -> ApiCatalog {
    ApiCatalog {
        host_functions: host_functions(),
        id_grammar: id_grammar(),
        settable_paths: settable_path_names(),
        constraints: constraints(),
        operations: operations(),
        elements: elements(),
        path_aliases: path_aliases(),
        property_schemas: crate::virtual_paths::property_schemas(),
    }
}

/// The advertised paths that answer to a second, raw-wire name.
fn path_aliases() -> Vec<PathAlias> {
    PROPERTY_PATHS
        .iter()
        .filter_map(|(name, path)| wire_alias(*path).map(|wire| PathAlias { name, wire }))
        .collect()
}

/// Resolve a JS property-path name to its `PropertyPath`. The single lookup
/// behind `parse_property_path`; the linear scan is fine for a 179-entry table
/// called at human cadence (one per `paged.set`/`get`).
pub fn lookup_path(name: &str) -> Option<P> {
    if let Some(path) = PROPERTY_PATHS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, path)| *path)
    {
        return Some(path);
    }
    // …and the RAW WIRE spelling, for the eight paths where the two
    // differ. See [`wire_alias`]: a caller who read a name off
    // `paged.inspect` or wrote one into `paged.batch` was holding a
    // string `paged.set` rejected, which is one capability with two
    // names and no door that takes both.
    PROPERTY_PATHS
        .iter()
        .find(|(_, path)| wire_alias(*path).is_some_and(|alias| alias == name))
        .map(|(_, path)| *path)
}

/// The RAW WIRE spelling of a path when it differs from the advertised
/// one, else `None`.
///
/// `PropertyPath` derives `Serialize` with `rename_all = "camelCase"`,
/// so the string `Mutation::SetElementProperty` carries — and the one
/// `paged.inspect`'s descriptor emits — is the camelCase of the VARIANT.
/// The catalog's name is chosen by hand. For 209 of the 217 they are the
/// same string and nobody notices; for eight they are not:
/// `TextWrapInvert` travels as `textWrapInvert` and is advertised as
/// `frameTextWrapInvert`, and the seven effect toggles travel as
/// `frameInnerShadowEnabled` and friends while being advertised without
/// the `Enabled`.
///
/// Both now resolve, and this publishes the second spelling rather than
/// leaving a consumer to discover it by being rejected.
pub fn wire_alias(path: P) -> Option<String> {
    let variant = variant_name(path);
    let mut chars = variant.chars();
    let serde_name = match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    (serde_name != wire_name(path)).then_some(serde_name)
}

/// Every wire op tag, from the one roster `Mutation::discriminant` is
/// generated with.
fn operations() -> Vec<String> {
    paged_wire::MUTATION_NAMES
        .iter()
        .map(|n| paged_wire::wire_tag_of(n))
        .collect()
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
        f!("paged.set", "(address, path, value)", "bool", "write",
           "Set one property of any object (Mutation::Set). address is an idGrammar address — an element (rectangle:u12, storyRange:Story/u1@0..6), doc, page:#2, layer:<id>, swatch:Color/Red, style:object/<id>, cell:<table>/0,1, textVariable:<id>, … — and path any row propertySchemas lists for that kind (settablePaths on elements). The value is coerced by the path's schema (enum members, ranges, \"3mm\" on a length). null clears. A selector is not a target: paged.query it first. The protocol-70 target object ({kind:\"layer\",layerId}) is still accepted, deprecated. A plugin:<pluginId>/<kind>/<id> address goes to the host's object registry (ADR 323): it returns a Promise<true>, consecutive plugin sets fold into ONE batched host round trip, a refusal rejects with an Error whose .code is the registry's code (an unawaited refusal fails the script), and core + plugin writes of one script undo as one step. With no host registry it throws code noHost."),
        f!("paged.get", "(address, path)", "value JSON | null", "read",
           "Read one property of any addressed object, as its wire value JSON (the read twin of paged.set; RequestGet on the wire). A plugin: address asks the host's object registry: a Promise of the value's JSON (null when absent or mixed); a refusal rejects with .code."),
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
        f!("paged.duplicateElements", "([id, ...], dx, dy)", "string[] (created ids) JSON | null", "author",
           "Duplicate page items: a whole clone of each (properties, plugin metadata, group members, pasted-in children, a text frame's story), offset by (dx, dy) pt and inserted directly above its source, in one undo step. Selects the clones and returns their kind:id addresses in source order; null when the engine refuses (threaded text frame, anchored object, opacity mask, an element named together with its group)."),
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
        f!("paged.query", "(selector)", "string[] JSON", "read",
           "Canonical addresses of every object a selector matches, document order (ADR 131): `page:#3 > frame`, `frame[name=\"price\"]`, `[label.x-paged:oid=a1b2]`, unions with `,`. Throws SyntaxError (with the offset) on a malformed selector. A selector naming plugin: kinds or objects is evaluated by the host's object registry and returns a Promise of the same JSON."),
        f!("paged.schema", "(addressOrKind)", "PropertySchema[] JSON", "read",
           "The typed property schema rows (ADR 132) of an address's kind (frame:u1, swatch:Color/Red) or a kind name (frame, page). A plugin: address or kind asks the host's object registry: a Promise of its PropertySchema[] JSON."),
        f!("paged.invoke", "(id, args?)", "Promise<command result JSON>", "write",
           "Run a typed command — core.<op> (every catalog operation, args as the op's typed struct; id positions take addresses) or a plugin's — through the host's object registry (RequestHostObjects invoke): a Promise of its result JSON ({ created, undoSteps } for a core command). Commands live in the host's registry only: with no host it throws code noHost; a refusal rejects with .code. Inside paged.batch use { op: \"invoke\", id, args, handle? }."),
        f!("paged.commands", "(owner?)", "Promise<CommandInfo[] JSON>", "read",
           "The typed commands the host's object registry offers (core's first), optionally of one owner (\"core\" or a plugin id): a Promise of the CommandInfo[] JSON. With no host it throws code noHost."),
        f!("paged.address", "(id)", "string|null", "read",
           "Canonical address of an object, from any accepted spelling (`frame:u1`, `page:#2`, a style by name) or a {kind,id} element; null when nothing answers to it."),
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
           "Duplicate a single-page spread after the source — its items, a copy of each frame's story, its margins; returns the new page selfId."),
        f!("paged.resizePage", "(pageId, [t,l,b,r])", "bool", "write", "Set a page's GeometricBounds in page-inner points."),
        f!("paged.applyMasterToPage", "(pageId, masterId?)", "bool", "write", "Apply a master to a page (omit/null detaches)."),
        f!("paged.movePage", "(pageId, afterPageId?)", "bool", "write", "Move a page to follow another (omit/null: to the front)."),
        f!("paged.createMaster", "(masterId, { name?, widthPt?, heightPt?, duplicateOf? })", "bool", "author", "Create a master spread: one page of the given size, or a copy of another master."),
        f!("paged.deleteMaster", "(masterId)", "bool", "author", "Delete a master no page applies."),
        f!("paged.renameMaster", "(masterId, name?)", "bool", "write", "Set (omit/null: clear) a master's name."),
        f!("paged.onMaster", "(masterId, mutation)", "bool", "write", "Apply a mutation ({ op, args }) to a master spread's items."),
        f!("paged.setPageMetadata", "(pageId, key, value?)", "bool", "write", "Set or delete one plugin-metadata entry on a page (value: the JSON envelope)."),
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
           "Insert a field marker; fieldKind = \"pageNumber\" | \"nextPageNumber\" | { placeholder: { plugin, key, value? } } | { textVariable: { variableId } } (an instance of a text variable; InDesign keeps it)."),
        f!("paged.createTextVariable", "(name, contents?, textBefore?, textAfter?)", "string (address) | null", "author",
           "Define a custom text variable; returns textVariable:dTextVariablen<name>. Switch its value with paged.set(address, \"textVariableContents\", v) — every instance re-bakes."),
        f!("paged.deleteTextVariable", "(variableId)", "bool", "author",
           "Remove a text variable definition (refused while an instance is in the text)."),
        f!("paged.setFieldValue", "(storyId, offset, value?)", "bool", "write", "Update a placeholder field's cached display value (null ⇒ unresolved)."),
        f!("paged.replaceImageBytes", "(frameId, bytes?)", "bool", "write", "Commit inline image bytes (number[] of u8) on a graphic frame; null clears."),
        // --- tables ---
        f!("paged.insertTable", "(storyId, spec)", "string | null", "author",
           "Create a <Table> at the end of a story; spec = { rows, cols, headerRows?, footerRows?, columnWidths?, rowHeights? }. Returns the table id."),
        f!("paged.setRowHeight", "(storyId, tableId, row, height?)", "bool", "write", "Set/clear a table row height in pt."),
        f!("paged.setColumnWidth", "(storyId, tableId, col, width?)", "bool", "write", "Set/clear a table column width in pt."),
        f!("paged.insertTableRow", "(storyId, tableId, at)", "bool", "author", "Insert an empty body row at index."),
        f!("paged.deleteTable", "(storyId, tableId)", "bool", "author", "Delete a whole table and its host paragraph; undo restores every cell."),
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
        f!("paged.setFlowGrowRule", "(storyId, grow, maxPages?, copyFrameOptions?)", "bool", "write", "Let a story's frame chain grow: generated pages are added after its last frame's page while it oversets (InDesign's Smart Text Reflow), and removed when empty. copyFrameOptions gives generated frames the chain's frame options (a Word section's mode) instead of InDesign's defaults. grow=false clears the rule."),
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
        f!("paged.setDocumentMetadata", "(key, value?, caller?)", "bool", "write", "Write one document-scoped Label key/value pair (value null deletes); undoable."),
        f!("paged.batch", "([mutations] | [objectOps])", "bool | per-op results JSON", "author", "Apply an array of { op, args } mutation objects as ONE undoable step (bool). An array of ADR 323 object ops ({ op: \"set\", address, path, value }) returns the per-op results JSON ([{ ok } | { ok: false, code, reason }]), all-or-nothing in ONE undo step; when any op names a plugin: address, or is a typed command ({ op: \"invoke\", id, args, handle? }), the whole batch goes to the host's object registry (a Promise of the same JSON), so a command shares the one undo step with the sets."),
        // --- selection setters (application state, NOT undoable) ---
        f!("paged.setElementSelection", "([id, ...])", "bool", "write", "Replace the element selection with the parseable ids."),
        f!("paged.clearSelection", "()", "bool", "write", "Clear the element selection."),
        f!("paged.setContentSelection", "({storyId,start,end} | null)", "bool", "write", "Set or clear the text caret/range."),
        // --- z-order & nesting ---
        f!("paged.reorderElement", "(id, target)", "bool", "author",
           "Raise or lower an element in its parent's z-order. target = front | back | forward | backward | {index: n} (n = the final slot, 0 = backmost)."),
        f!("paged.pasteInto", "(containerId, childId)", "bool", "author",
           "Nest an existing item inside a container frame."),
        f!("paged.releaseFrom", "(childId)", "bool", "author",
           "Lift a nested item back out of its container (inverse of pasteInto)."),
        // --- path topology ---
        f!("paged.closePath", "(id, subpath?)", "bool", "author",
           "Close an open contour; subpath defaults to the whole path."),
        f!("paged.joinPaths", "(id, otherId)", "bool", "author",
           "Weld two open paths into one."),
        // --- pathfinder region verbs ---
        f!("paged.pathfinderDivide", "([id, ...])", "bool", "author",
           "Divide the selection into its planar regions."),
        f!("paged.pathfinderTrim", "([id, ...])", "bool", "author",
           "Trim hidden parts of the lower shapes away."),
        f!("paged.pathfinderMerge", "([id, ...])", "bool", "author",
           "Merge adjacent same-paint regions of the selection."),
        f!("paged.pathfinderCrop", "([id, ...])", "bool", "author",
           "Crop the selection to the topmost shape."),
        f!("paged.pathfinderOutline", "([id, ...])", "bool", "author",
           "Reduce the selection to its outlined edges."),
        f!("paged.pathfinderMinusBack", "([id, ...])", "bool", "author",
           "Subtract the shapes behind from the frontmost one."),
        f!("paged.pathfinderFaces", "([id, ...], [faceId, ...], mode)", "bool", "author",
           "Keep or drop named faces of the planar arrangement; mode = keep | remove. Face ids come from paged.planarRegions."),
        f!("paged.planarRegions", "([id, ...], [x, y]?)", "PlanarRegionsResult JSON", "read",
           "The planar arrangement of the named paths: found, faces (each with an id), inputCount, complete, reason. With a point, only the face under it. This is how a script names a face for pathfinderFaces."),
        // --- opacity masks & text on a path ---
        f!("paged.applyOpacityMask", "(targetId, maskId, {maskType?, invert?}?)", "bool", "author",
           "Mask one element with another."),
        f!("paged.releaseOpacityMask", "(targetId)", "bool", "author",
           "Remove an element's opacity mask."),
        f!("paged.attachTextToPath", "(id, storyId, {pathTypeAlignment?, flipPathEffect?, startBracket?, endBracket?}?)", "bool", "author",
           "Run a story along a path."),
        f!("paged.detachTextFromPath", "(id)", "bool", "author",
           "Take a story back off its path."),
        // --- anchored frames & hyperlinks ---
        f!("paged.insertAnchoredFrame", "(storyId, offset, width, height, imageUri?)", "bool", "author",
           "Anchor a frame in the text at a story offset; with imageUri the frame is created holding that image."),
        f!("paged.insertHyperlink", "(storyId, start, end, url, pageId?)", "bool", "author",
           "Make a character range a clickable link. Read the result with paged.collection(\"hyperlinks\") — paged.links() is the placed-asset list, not this."),
        // --- layer attributes ---
        f!("paged.layerSetVisible", "(layerId, visible)", "bool", "write", "Show or hide a layer."),
        f!("paged.layerSetLocked", "(layerId, locked)", "bool", "write", "Lock or unlock a layer."),
        f!("paged.layerSetPrintable", "(layerId, printable)", "bool", "write",
           "Include a layer in output, or hold it back."),
        f!("paged.layerSetName", "(layerId, name)", "bool", "write", "Rename a layer."),
        // --- colour-resource CRUD ---
        f!("paged.createSwatch", "(spec)", "string (created id) | null", "author",
           "Create a swatch from a SwatchSpec ({space, value: [...], name?, model?, tint?}); returns its selfId."),
        f!("paged.editSwatch", "(swatchId, spec)", "bool", "author", "Replace a swatch's definition."),
        f!("paged.deleteSwatch", "(swatchId)", "bool", "author", "Delete a swatch."),
        f!("paged.createGradient", "(spec)", "string (created id) | null", "author",
           "Create a gradient from a GradientSpec ({kind: \"Linear\"|\"Radial\", stops: [...], name?}); returns its selfId."),
        f!("paged.editGradient", "(gradientId, spec)", "bool", "author", "Replace a gradient's definition."),
        f!("paged.deleteGradient", "(gradientId)", "bool", "author", "Delete a gradient."),
        f!("paged.createColorGroup", "(spec)", "string (created id) | null", "author",
           "Create a colour group from a ColorGroupSpec; returns its selfId."),
        f!("paged.editColorGroup", "(groupId, spec)", "bool", "author", "Replace a colour group's definition."),
        f!("paged.deleteColorGroup", "(groupId)", "bool", "author", "Delete a colour group."),
        // --- console (captured into the run output) ---
        // All four are declared, not folded into one: a generated
        // consumer that lists the catalog was advertising a `console`
        // with a single member while the bridge installs four.
        f!("console.log", "(...)", "undefined", "console", "Append a line to the captured output log."),
        f!("console.warn", "(...)", "undefined", "console", "Append a warn-level line to the captured output log."),
        f!("console.error", "(...)", "undefined", "console", "Append an error-level line to the captured output log."),
        f!("console.info", "(...)", "undefined", "console", "Append an info-level line to the captured output log."),
    ]
}

/// The published id grammar — GENERATED from `paged_wire::Address`'s
/// form table (ADR 131), one entry per address kind, so the parser and
/// the documentation cannot drift: `paged-wire` proves every example
/// parses and prints back byte-identically, and a new kind does not
/// compile until it has a form.
fn id_grammar() -> Vec<IdForm> {
    paged_wire::address_forms()
        .into_iter()
        .map(|f| IdForm {
            form: f.form,
            example: f.example,
            note: f.note,
        })
        .collect()
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

/// THE single source for property-path names: variant → JS name, for ALL
/// of them.
///
/// There used to be two lists. This one carried 176 names for
/// `paged.set` and the catalog; `paged-script`'s `property_path_label`
/// carried 217 for `paged.get` and every human label. They agreed on
/// the 176 they shared — by hand, with nothing checking it — and the 41
/// only the longer list knew about were invisible to `paged.set`, to
/// `catalog.json`, and so to docs.paged.media and the plugin SDK, which
/// generate from it. Every one of those 41 has a working `apply` arm.
///
/// Now the macro below generates both, so a `PropertyPath` variant
/// cannot be named twice, and the `wire_name` match is exhaustive with
/// no `_` arm — a new variant does not compile until it is placed in
/// one group or the other, which is the difference between "routed
/// elsewhere on purpose" and "forgotten".
///
/// `advertised` is what `settable_paths()` publishes and `lookup_path`
/// resolves — unchanged, so `catalog.json` is byte-identical.
/// `hidden` carries the name plus WHY it is not published. Several of
/// those reasons read "no recorded reason", which is the honest state
/// of them and the worklist for promoting them.
macro_rules! property_paths {
    (
        advertised { $($a_variant:ident => $a_name:literal : $a_schema:expr,)* }
        addressed { $($d_variant:ident => $d_name:literal : $d_schema:expr, $d_kind:literal,)* }
        hidden { $($h_variant:ident => $h_name:literal : $h_schema:expr, $h_reason:literal,)* }
    ) => {
        /// The advertised name → `PropertyPath` pairs, in the engine's
        /// own grouping. `parse_property_path` (via [`lookup_path`]) and
        /// `settable_paths()` both read this; there is no second list.
        pub const PROPERTY_PATHS: &[(&str, P)] = &[
            $(($a_name, P::$a_variant),)*
        ];

        /// ADR 132 — the paths that live on a non-element address kind,
        /// with that kind (`"layer"`, `"page"`, `"doc"`, `"textVariable"`,
        /// `"style:object"`, …).
        pub const ADDRESSED_PATHS: &[(&str, P, &str)] = &[
            $(($d_name, P::$d_variant, $d_kind),)*
        ];

        /// Every property path, advertised or not, generated from the
        /// same tokens as the names — so iteration and naming cannot
        /// come apart the way the two hand tables did.
        pub const ALL_PATHS: &[P] = &[
            $(P::$a_variant,)*
            $(P::$d_variant,)*
            $(P::$h_variant,)*
        ];

        /// The JS name of ANY property path, advertised or not. Backs
        /// `paged.get`, every human-facing label, and the reverse of
        /// [`lookup_path`]. Exhaustive by construction.
        pub fn wire_name(path: P) -> &'static str {
            match path {
                $(P::$a_variant => $a_name,)*
                $(P::$d_variant => $d_name,)*
                $(P::$h_variant => $h_name,)*
            }
        }

        /// ADR 132 — the typed schema of ANY property path. A variant
        /// without one does not compile: the macro arm requires the
        /// `: schema` slot on every entry.
        pub fn schema(path: P) -> PropertySchema {
            match path {
                $(P::$a_variant => $a_schema,)*
                $(P::$d_variant => $d_schema,)*
                $(P::$h_variant => $h_schema,)*
            }
        }

        /// The address kind an `addressed` path lives on; `None` for the
        /// element paths and the hidden ones.
        pub fn addressed_on(path: P) -> Option<&'static str> {
            match path {
                $(P::$d_variant => Some($d_kind),)*
                _ => None,
            }
        }

        /// `Some(reason)` when a path has a name but is deliberately not
        /// advertised as settable; `None` when it is published (as an
        /// element path or on its own address kind).
        pub fn unadvertised_reason(path: P) -> Option<&'static str> {
            match path {
                $(P::$a_variant => None,)*
                $(P::$d_variant => None,)*
                $(P::$h_variant => Some($h_reason),)*
            }
        }

        /// The Rust variant's own name. Generated from the same tokens,
        /// so it cannot drift; it exists because serde derives the RAW
        /// WIRE spelling from it (see [`wire_alias`]).
        pub fn variant_name(path: P) -> &'static str {
            match path {
                $(P::$a_variant => stringify!($a_variant),)*
                $(P::$d_variant => stringify!($d_variant),)*
                $(P::$h_variant => stringify!($h_variant),)*
            }
        }
    };
}

property_paths! {
    advertised {

        FrameBounds => "frameBounds" : bounds().idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon, GeometricBounds)]),
        FrameFillColor => "frameFillColor" : color().idml(idml![attr(TextFrame | Rectangle | Oval | Polygon | ObjectStyle, FillColor)]),
        FrameStrokeColor => "frameStrokeColor" : color().idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | ObjectStyle, StrokeColor)]),
        FrameStrokeWeight => "frameStrokeWeight" : length_pos().default_num(1.0).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | ObjectStyle, StrokeWeight)]),
        FrameOpacity => "frameOpacity" : pct().default_num(100.0).idml(idml![attr(BlendingSetting, Opacity)]),
        FrameTransform => "frameTransform" : transform().idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | Group, ItemTransform)]),
        ImageContentTransform => "imageContentTransform" : transform().idml(idml![attr(Image, ItemTransform)]),
        FramePathPoint => "framePathPoint" : operation("PathPoint", Wire::PathPoint),
        PathPointInsert => "pathPointInsert" : operation("PathPointInsert", Wire::PathPointInsert),
        PathPointRemove => "pathPointRemove" : operation("PathPointRemove", Wire::PathPointRemove),
        PathPointCurveType => "pathPointCurveType" : operation("PathPointCurveType", Wire::PathPointCurveType),
        // NO layer* PATHS HERE, deliberately (C-33, 2026-08-07). They were
        // listed and projected into `settable_paths`, and NOTHING COULD USE
        // THEM: `ElementId` has no Layer variant, `id_grammar()` publishes
        // no layer address form, and `parse_element_id` cannot produce one —
        // so `paged.set("layer:ua", "layerVisible", false)` never parsed. A
        // layer's flags ARE mutable, through the seven dedicated `Layer*`
        // mutations, which is a different lane and is where they belong.
        //
        // This mattered beyond tidiness: docs.paged.media and the plugin SDK
        // both GENERATE from this catalog, so the false claim propagated to
        // every consumer that trusted it. `every_settable_path_is_addressable`
        // below now fails if this comes back.
        CharacterFontSize => "characterFontSize" : length().range(0.1, 1296.0).default_num(12.0).idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, PointSize)]),
        CharacterLeading => "characterLeading" : length().min(0.0).idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, Leading), child(CharacterStyleRange | ParagraphStyle | CharacterStyle, Leading)]),
        CharacterTracking => "characterTracking" : number().range(-1000.0, 10000.0).default_num(0.0).idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, Tracking)]),
        CharacterFillColor => "characterFillColor" : color().idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, FillColor)]),
        ParagraphSpaceBefore => "paragraphSpaceBefore" : length().default_num(0.0).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, SpaceBefore)]),
        ParagraphSpaceAfter => "paragraphSpaceAfter" : length().default_num(0.0).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, SpaceAfter)]),
        ParagraphFirstLineIndent => "paragraphFirstLineIndent" : length().default_num(0.0).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, FirstLineIndent)]),
        AppliedParagraphStyle => "appliedParagraphStyle" : reference(RefKind::ParagraphStyle).idml(idml![attr(ParagraphStyleRange, AppliedParagraphStyle)]),
        AppliedCharacterStyle => "appliedCharacterStyle" : reference(RefKind::CharacterStyle).idml(idml![attr(CharacterStyleRange, AppliedCharacterStyle)]),
        AppliedObjectStyle => "appliedObjectStyle" : reference(RefKind::ObjectStyle).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon, AppliedObjectStyle)]),
        // C-35 (v62) — WHICH LAYER an item is on. Note the contrast with
        // the `layer*` paths refused just above: those addressed a LAYER,
        // which has no `ElementId` form, so nothing could name the target.
        // This one addresses the PAGE ITEM (`rectangle:u12`, `textFrame:u7`
        // — forms `id_grammar()` already publishes) and merely carries a
        // layer self_id as its value, so it is addressable and passes
        // `every_settable_path_is_addressable`.
        ItemLayer => "itemLayer" : reference(RefKind::Layer).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon, ItemLayer)]),
        AppliedCellStyle => "appliedCellStyle" : reference(RefKind::CellStyle).idml(idml![attr(Cell, AppliedCellStyle)]),
        AppliedTableStyle => "appliedTableStyle" : reference(RefKind::TableStyle).idml(idml![attr(Table, AppliedTableStyle)]),
        AppliedConditions => "appliedConditions" : reference(RefKind::Condition).idml(idml![attr(CharacterStyleRange, AppliedConditions)]),
        FrameInsetSpacing => "frameInsetSpacing" : bounds().idml(idml![child(TextFramePreference, InsetSpacing), attr(TextFramePreference, InsetSpacing)]),
        ParagraphJustification => "paragraphJustification" : enum_(JUSTIFICATION).default_text("LeftAlign").idml(idml![attr(ParagraphStyleRange | ParagraphStyle, Justification)]),
        ParagraphStyleNextStyle => "paragraphStyleNextStyle" : reference(RefKind::ParagraphStyle).idml(idml![attr(ParagraphStyle, NextStyle)]),
        ParagraphAppliedNumberingList => "paragraphAppliedNumberingList" : reference(RefKind::NumberingList).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, AppliedNumberingList), child(ParagraphStyleRange | ParagraphStyle, AppliedNumberingList)]),
        FrameStrokeEndCap => "frameStrokeEndCap" : enum_(END_CAPS).default_text("ButtEndCap").idml(idml![attr(Rectangle | Oval | GraphicLine | Polygon, EndCap)]),
        FrameStrokeStartArrowhead => "frameStrokeStartArrowhead" : enum_(ARROWHEADS).default_text("None").idml(idml![attr(GraphicLine | Polygon, LeftLineEnd)]),
        FrameStrokeEndArrowhead => "frameStrokeEndArrowhead" : enum_(ARROWHEADS).default_text("None").idml(idml![attr(GraphicLine | Polygon, RightLineEnd)]),
        FrameTextWrapMode => "frameTextWrapMode" : enum_open(TEXT_WRAP_MODES).default_text("None").idml(idml![attr(TextWrapPreference, TextWrapMode)]),
        FrameTextWrapOffsets => "frameTextWrapOffsets" : bounds(),
        FrameTextWrapContourType => "frameTextWrapContourType" : enum_open(CONTOUR_TYPES).idml(idml![attr(ContourOption, ContourType)]),
        FrameTextWrapContourIncludeInside => "frameTextWrapContourIncludeInside" : bool_().default_bool(false).idml(idml![attr(ContourOption, IncludeInsideEdges)]),
        FrameFittingCrops => "frameFittingCrops" : bounds(),
        FrameFittingType => "frameFittingType" : enum_open(FITTING).default_text("None").idml(idml![attr(FrameFittingOption, FittingOnEmptyFrame)]),
        FrameDropShadow => "frameDropShadow" : bool_().default_bool(false),
        FrameDropShadowMode => "frameDropShadowMode" : enum_open(DROP_SHADOW_MODES).idml(idml![attr(DropShadowSetting, Mode)]),
        FrameDropShadowXOffset => "frameDropShadowXOffset" : length().idml(idml![attr(DropShadowSetting, XOffset)]),
        FrameDropShadowYOffset => "frameDropShadowYOffset" : length().idml(idml![attr(DropShadowSetting, YOffset)]),
        FrameDropShadowSize => "frameDropShadowSize" : length_pos().idml(idml![attr(DropShadowSetting, Size)]),
        FrameDropShadowOpacity => "frameDropShadowOpacity" : pct().default_num(75.0).idml(idml![attr(DropShadowSetting, Opacity)]),
        FrameDropShadowColor => "frameDropShadowColor" : color().idml(idml![attr(DropShadowSetting, EffectColor)]),
        FramePath => "framePath" : structure("FramePath", Wire::FramePath).required(),
        FrameFillTint => "frameFillTint" : pct().default_num(100.0).idml(idml![attr(TextFrame | Rectangle | Oval | Polygon | ObjectStyle, FillTint)]),
        FrameNonprinting => "frameNonprinting" : bool_().default_bool(false).idml(idml![attr(TextFrame | Rectangle, Nonprinting)]),
        FrameGradientFillAngle => "frameGradientFillAngle" : deg().idml(idml![attr(TextFrame | Rectangle | Oval | Polygon, GradientFillAngle)]),
        FrameGradientFillLength => "frameGradientFillLength" : length_pos().idml(idml![attr(TextFrame | Rectangle | Oval | Polygon, GradientFillLength)]),
        FrameGradientStrokeAngle => "frameGradientStrokeAngle" : deg().idml(idml![attr(TextFrame | Rectangle | Oval | Polygon, GradientStrokeAngle)]),
        FrameGradientStrokeLength => "frameGradientStrokeLength" : length_pos().idml(idml![attr(TextFrame | Rectangle | Oval | Polygon, GradientStrokeLength)]),
        TextFrameColumnCount => "textFrameColumnCount" : count().range(1.0, 40.0).default_num(1.0).idml(idml![attr(TextFramePreference, TextColumnCount)]),
        TextFrameColumnGutter => "textFrameColumnGutter" : length_pos().default_num(12.0).idml(idml![attr(TextFramePreference, TextColumnGutter)]),
        TextFrameColumnBalance => "textFrameColumnBalance" : bool_().default_bool(false).idml(idml![attr(TextFramePreference, VerticalBalanceColumns)]),
        TextFrameVerticalJustification => "textFrameVerticalJustification" : enum_(VERTICAL_JUSTIFICATION).default_text("TopAlign").idml(idml![attr(TextFramePreference, VerticalJustification)]),
        TextFrameAutoSizing => "textFrameAutoSizing" : enum_(AUTO_SIZING).default_text("Off").idml(idml![attr(TextFramePreference, AutoSizingType)]),
        TextFrameFirstBaseline => "textFrameFirstBaseline" : enum_(FIRST_BASELINE).default_text("AscentOffset").idml(idml![attr(TextFramePreference, FirstBaselineOffset)]),
        TextWrapInvert => "frameTextWrapInvert" : bool_().default_bool(false).idml(idml![attr(TextWrapPreference, Inverse)]),
        FrameFittingReferencePoint => "frameFittingReferencePoint" : enum_open(REFERENCE_POINTS).default_text("TopLeftPoint").idml(idml![attr(FrameFittingOption, FittingAlignment)]),
        FrameAutoFit => "frameAutoFit" : bool_().default_bool(false).idml(idml![attr(FrameFittingOption, AutoFit)]),
        FrameStrokeType => "frameStrokeType" : enum_open(STROKE_TYPES).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon, StrokeType)]),
        FrameStrokeJoin => "frameStrokeJoin" : enum_open(END_JOINS).default_text("MiterEndJoin").idml(idml![attr(Rectangle | GraphicLine | Polygon, EndJoin)]),
        FrameStrokeMiterLimit => "frameStrokeMiterLimit" : number().range(1.0, 500.0).default_num(4.0).idml(idml![attr(Rectangle | GraphicLine | Polygon, MiterLimit)]),
        FrameStrokeAlignment => "frameStrokeAlignment" : enum_open(STROKE_ALIGNMENT).default_text("CenterAlignment").idml(idml![attr(TextFrame | Rectangle | Oval | Polygon, StrokeAlignment)]),
        FrameStrokeGapColor => "frameStrokeGapColor" : color().idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon, GapColor)]),
        FrameStrokeGapTint => "frameStrokeGapTint" : pct().default_num(100.0).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon, GapTint)]),
        FrameStrokeDashArray => "frameStrokeDashArray" : lengths().idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon, StrokeDashAndGap)]),
        FrameCornerOptionTopLeft => "frameCornerOptionTopLeft" : enum_open(CORNER_OPTIONS).default_text("None").idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | Group, TopLeftCornerOption)]),
        FrameCornerOptionTopRight => "frameCornerOptionTopRight" : enum_open(CORNER_OPTIONS).default_text("None").idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | Group, TopRightCornerOption)]),
        FrameCornerOptionBottomLeft => "frameCornerOptionBottomLeft" : enum_open(CORNER_OPTIONS).default_text("None").idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | Group, BottomLeftCornerOption)]),
        FrameCornerOptionBottomRight => "frameCornerOptionBottomRight" : enum_open(CORNER_OPTIONS).default_text("None").idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | Group, BottomRightCornerOption)]),
        FrameCornerRadiusTopLeft => "frameCornerRadiusTopLeft" : length_pos().default_num(12.0).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | Group, TopLeftCornerRadius)]),
        FrameCornerRadiusTopRight => "frameCornerRadiusTopRight" : length_pos().default_num(12.0).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | Group, TopRightCornerRadius)]),
        FrameCornerRadiusBottomLeft => "frameCornerRadiusBottomLeft" : length_pos().default_num(12.0).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | Group, BottomLeftCornerRadius)]),
        FrameCornerRadiusBottomRight => "frameCornerRadiusBottomRight" : length_pos().default_num(12.0).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon | Group, BottomRightCornerRadius)]),
        FrameRotationAngle => "frameRotationAngle" : deg().default_num(0.0),
        FrameScaleX => "frameScaleX" : number().default_num(100.0).wire(Wire::Length),
        FrameScaleY => "frameScaleY" : number().default_num(100.0).wire(Wire::Length),
        FrameFlipH => "frameFlipH" : bool_().default_bool(false),
        FrameFlipV => "frameFlipV" : bool_().default_bool(false),
        FrameOverprintFill => "frameOverprintFill" : bool_().default_bool(false).idml(idml![attr(TextFrame | Rectangle | Oval | Polygon, OverprintFill)]),
        FrameOverprintStroke => "frameOverprintStroke" : bool_().default_bool(false).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon, OverprintStroke)]),
        FrameInnerShadowEnabled => "frameInnerShadow" : bool_().default_bool(false).idml(idml![attr(InnerShadowSetting, Applied)]),
        FrameInnerShadowBlendMode => "frameInnerShadowBlendMode" : enum_open(BLEND_MODES).idml(idml![attr(InnerShadowSetting, BlendMode)]),
        FrameInnerShadowColor => "frameInnerShadowColor" : color().idml(idml![attr(InnerShadowSetting, EffectColor)]),
        FrameInnerShadowOpacity => "frameInnerShadowOpacity" : pct().idml(idml![attr(InnerShadowSetting, Opacity)]),
        FrameInnerShadowAngle => "frameInnerShadowAngle" : deg().idml(idml![attr(InnerShadowSetting, Angle)]),
        FrameInnerShadowDistance => "frameInnerShadowDistance" : length_pos().idml(idml![attr(InnerShadowSetting, Distance)]),
        FrameInnerShadowSize => "frameInnerShadowSize" : length_pos().idml(idml![attr(InnerShadowSetting, Size)]),
        FrameInnerShadowChoke => "frameInnerShadowChoke" : pct().idml(idml![attr(InnerShadowSetting, ChokeAmount)]),
        FrameInnerShadowNoise => "frameInnerShadowNoise" : pct().idml(idml![attr(InnerShadowSetting, Noise)]),
        FrameOuterGlowEnabled => "frameOuterGlow" : bool_().default_bool(false).idml(idml![attr(OuterGlowSetting, Applied)]),
        FrameOuterGlowBlendMode => "frameOuterGlowBlendMode" : enum_open(BLEND_MODES).idml(idml![attr(OuterGlowSetting, BlendMode)]),
        FrameOuterGlowColor => "frameOuterGlowColor" : color().idml(idml![attr(OuterGlowSetting, EffectColor)]),
        FrameOuterGlowOpacity => "frameOuterGlowOpacity" : pct().idml(idml![attr(OuterGlowSetting, Opacity)]),
        FrameOuterGlowSpread => "frameOuterGlowSpread" : pct().idml(idml![attr(OuterGlowSetting, Spread)]),
        FrameOuterGlowSize => "frameOuterGlowSize" : length_pos().idml(idml![attr(OuterGlowSetting, Size)]),
        FrameOuterGlowNoise => "frameOuterGlowNoise" : pct().idml(idml![attr(OuterGlowSetting, Noise)]),
        FrameInnerGlowEnabled => "frameInnerGlow" : bool_().default_bool(false).idml(idml![attr(InnerGlowSetting, Applied)]),
        FrameInnerGlowBlendMode => "frameInnerGlowBlendMode" : enum_open(BLEND_MODES).idml(idml![attr(InnerGlowSetting, BlendMode)]),
        FrameInnerGlowColor => "frameInnerGlowColor" : color().idml(idml![attr(InnerGlowSetting, EffectColor)]),
        FrameInnerGlowOpacity => "frameInnerGlowOpacity" : pct().idml(idml![attr(InnerGlowSetting, Opacity)]),
        FrameInnerGlowChoke => "frameInnerGlowChoke" : pct().idml(idml![attr(InnerGlowSetting, Spread)]),
        FrameInnerGlowSize => "frameInnerGlowSize" : length_pos().idml(idml![attr(InnerGlowSetting, Size)]),
        FrameInnerGlowSource => "frameInnerGlowSource" : enum_(GLOW_SOURCES).idml(idml![attr(InnerGlowSetting, Source)]),
        FrameInnerGlowNoise => "frameInnerGlowNoise" : pct().idml(idml![attr(InnerGlowSetting, Noise)]),
        FrameBevelEnabled => "frameBevel" : bool_().default_bool(false).idml(idml![attr(BevelAndEmbossSetting, Applied)]),
        FrameBevelStyle => "frameBevelStyle" : enum_open(BEVEL_STYLES).idml(idml![attr(BevelAndEmbossSetting, Style)]),
        FrameBevelTechnique => "frameBevelTechnique" : enum_open(BEVEL_TECHNIQUES).idml(idml![attr(BevelAndEmbossSetting, Technique)]),
        FrameBevelDepth => "frameBevelDepth" : pct().range(1.0, 1000.0).idml(idml![attr(BevelAndEmbossSetting, Depth)]),
        FrameBevelDirection => "frameBevelDirection" : enum_open(BEVEL_DIRECTIONS).idml(idml![attr(BevelAndEmbossSetting, Direction)]),
        FrameBevelSize => "frameBevelSize" : length_pos().idml(idml![attr(BevelAndEmbossSetting, Size)]),
        FrameBevelSoften => "frameBevelSoften" : length_pos().idml(idml![attr(BevelAndEmbossSetting, Soften)]),
        FrameBevelAngle => "frameBevelAngle" : deg().idml(idml![attr(BevelAndEmbossSetting, Angle)]),
        FrameBevelAltitude => "frameBevelAltitude" : deg().idml(idml![attr(BevelAndEmbossSetting, Altitude)]),
        FrameBevelHighlightColor => "frameBevelHighlightColor" : color().idml(idml![attr(BevelAndEmbossSetting, HighlightColor)]),
        FrameBevelShadowColor => "frameBevelShadowColor" : color().idml(idml![attr(BevelAndEmbossSetting, ShadowColor)]),
        FrameBevelHighlightOpacity => "frameBevelHighlightOpacity" : pct().idml(idml![attr(BevelAndEmbossSetting, HighlightOpacity)]),
        FrameBevelShadowOpacity => "frameBevelShadowOpacity" : pct().idml(idml![attr(BevelAndEmbossSetting, ShadowOpacity)]),
        FrameSatinEnabled => "frameSatin" : bool_().default_bool(false).idml(idml![attr(SatinSetting, Applied)]),
        FrameSatinBlendMode => "frameSatinBlendMode" : enum_open(BLEND_MODES).idml(idml![attr(SatinSetting, BlendMode)]),
        FrameSatinColor => "frameSatinColor" : color().idml(idml![attr(SatinSetting, EffectColor)]),
        FrameSatinOpacity => "frameSatinOpacity" : pct().idml(idml![attr(SatinSetting, Opacity)]),
        FrameSatinAngle => "frameSatinAngle" : deg().idml(idml![attr(SatinSetting, Angle)]),
        FrameSatinDistance => "frameSatinDistance" : length_pos().idml(idml![attr(SatinSetting, Distance)]),
        FrameSatinSize => "frameSatinSize" : length_pos().idml(idml![attr(SatinSetting, Size)]),
        FrameSatinInvert => "frameSatinInvert" : bool_().default_bool(false).idml(idml![attr(SatinSetting, InvertEffect)]),
        FrameFeatherEnabled => "frameFeather" : bool_().default_bool(false),
        FrameFeatherWidth => "frameFeatherWidth" : length_pos().idml(idml![attr(FeatherSetting, Width)]),
        FrameFeatherCornerType => "frameFeatherCornerType" : enum_open(FEATHER_CORNERS).idml(idml![attr(FeatherSetting, CornerType)]),
        FrameFeatherNoise => "frameFeatherNoise" : pct().idml(idml![attr(FeatherSetting, Noise)]),
        FrameFeatherChoke => "frameFeatherChoke" : pct().idml(idml![attr(FeatherSetting, ChokeAmount)]),
        FrameDirectionalFeatherEnabled => "frameDirectionalFeather" : bool_().default_bool(false).idml(idml![attr(DirectionalFeatherSetting, Applied)]),
        FrameDirectionalFeatherLeftWidth => "frameDirectionalFeatherLeftWidth" : length_pos().idml(idml![attr(DirectionalFeatherSetting, LeftWidth)]),
        FrameDirectionalFeatherRightWidth => "frameDirectionalFeatherRightWidth" : length_pos().idml(idml![attr(DirectionalFeatherSetting, RightWidth)]),
        FrameDirectionalFeatherTopWidth => "frameDirectionalFeatherTopWidth" : length_pos().idml(idml![attr(DirectionalFeatherSetting, TopWidth)]),
        FrameDirectionalFeatherBottomWidth => "frameDirectionalFeatherBottomWidth" : length_pos().idml(idml![attr(DirectionalFeatherSetting, BottomWidth)]),
        FrameDirectionalFeatherAngle => "frameDirectionalFeatherAngle" : deg().idml(idml![attr(DirectionalFeatherSetting, Angle)]),
        FrameDirectionalFeatherNoise => "frameDirectionalFeatherNoise" : pct().idml(idml![attr(DirectionalFeatherSetting, Noise)]),
        FrameDirectionalFeatherChoke => "frameDirectionalFeatherChoke" : pct().idml(idml![attr(DirectionalFeatherSetting, ChokeAmount)]),
        FrameBlendMode => "frameBlendMode" : enum_open(BLEND_MODES).default_text("Normal").idml(idml![attr(BlendingSetting, BlendMode)]),
        // R-10 — the fill, stroke and content targets (the `<BlendingSetting>`
        // under `<Fill|Stroke|ContentTransparencySetting>`, a picture's under
        // its `<Image>`), and how a group's members composite.
        FrameFillOpacity => "frameFillOpacity" : pct().default_num(100.0).idml(idml![attr(BlendingSetting, Opacity)]),
        FrameFillBlendMode => "frameFillBlendMode" : enum_open(BLEND_MODES).default_text("Normal").idml(idml![attr(BlendingSetting, BlendMode)]),
        FrameStrokeOpacity => "frameStrokeOpacity" : pct().default_num(100.0).idml(idml![attr(BlendingSetting, Opacity)]),
        FrameStrokeBlendMode => "frameStrokeBlendMode" : enum_open(BLEND_MODES).default_text("Normal").idml(idml![attr(BlendingSetting, BlendMode)]),
        FrameContentOpacity => "frameContentOpacity" : pct().default_num(100.0).idml(idml![attr(BlendingSetting, Opacity)]),
        FrameContentBlendMode => "frameContentBlendMode" : enum_open(BLEND_MODES).default_text("Normal").idml(idml![attr(BlendingSetting, BlendMode)]),
        GroupIsolateBlending => "groupIsolateBlending" : bool_().default_bool(false).idml(idml![attr(BlendingSetting, IsolateBlending)]),
        GroupKnockout => "groupKnockout" : bool_().default_bool(false).idml(idml![attr(BlendingSetting, KnockoutGroup)]),
        // R-11 — the drop shadow's remaining attributes.
        FrameDropShadowBlendMode => "frameDropShadowBlendMode" : enum_open(BLEND_MODES).default_text("Multiply").idml(idml![attr(DropShadowSetting, BlendMode)]),
        FrameDropShadowSpread => "frameDropShadowSpread" : pct().default_num(0.0).idml(idml![attr(DropShadowSetting, Spread)]),
        FrameDropShadowNoise => "frameDropShadowNoise" : pct().default_num(0.0).idml(idml![attr(DropShadowSetting, Noise)]),
        FrameDropShadowKnockedOut => "frameDropShadowKnockedOut" : bool_().default_bool(true).idml(idml![attr(DropShadowSetting, KnockedOut)]),
        FrameDropShadowUseGlobalLight => "frameDropShadowUseGlobalLight" : bool_().default_bool(false).idml(idml![attr(DropShadowSetting, UseGlobalLight)]),
        // W0.4 — the seventh frame effect. Its six siblings were
        // advertised and it was not, under "no recorded reason"; the
        // apply arm takes the same node kinds as the others and the
        // bridge already encodes its spec. PROMOTED 2026-09-09.
        FrameGradientFeather => "frameGradientFeather" : structure("GradientFeatherSpec", Wire::GradientFeather),
        CellFillColor => "cellFillColor" : color().idml(idml![attr(Cell | CellStyle, FillColor)]),
        CellFillTint => "cellFillTint" : pct().default_num(100.0),
        CellInsetTop => "cellInsetTop" : length_pos().idml(idml![attr(Cell, TextTopInset)]),
        CellInsetLeft => "cellInsetLeft" : length_pos().idml(idml![attr(Cell, TextLeftInset)]),
        CellInsetBottom => "cellInsetBottom" : length_pos().idml(idml![attr(Cell, TextBottomInset)]),
        CellInsetRight => "cellInsetRight" : length_pos().idml(idml![attr(Cell, TextRightInset)]),
        CellVerticalJustification => "cellVerticalJustification" : enum_(VERTICAL_JUSTIFICATION).default_text("TopAlign").idml(idml![attr(Cell | CellStyle, VerticalJustification)]),
        CellTopEdgeStrokeColor => "cellTopEdgeStrokeColor" : color().idml(idml![attr(Cell | CellStyle, TopEdgeStrokeColor)]),
        CellTopEdgeStrokeWeight => "cellTopEdgeStrokeWeight" : length_pos().idml(idml![attr(Cell | CellStyle, TopEdgeStrokeWeight)]),
        CellTopEdgeStrokeTint => "cellTopEdgeStrokeTint" : pct().default_num(100.0).idml(idml![attr(Cell, TopEdgeStrokeTint)]),
        CellBottomEdgeStrokeColor => "cellBottomEdgeStrokeColor" : color().idml(idml![attr(Cell | CellStyle, BottomEdgeStrokeColor)]),
        CellBottomEdgeStrokeWeight => "cellBottomEdgeStrokeWeight" : length_pos().idml(idml![attr(Cell | CellStyle, BottomEdgeStrokeWeight)]),
        CellBottomEdgeStrokeTint => "cellBottomEdgeStrokeTint" : pct().default_num(100.0).idml(idml![attr(Cell, BottomEdgeStrokeTint)]),
        CellLeftEdgeStrokeColor => "cellLeftEdgeStrokeColor" : color().idml(idml![attr(Cell | CellStyle, LeftEdgeStrokeColor)]),
        CellLeftEdgeStrokeWeight => "cellLeftEdgeStrokeWeight" : length_pos().idml(idml![attr(Cell | CellStyle, LeftEdgeStrokeWeight)]),
        CellLeftEdgeStrokeTint => "cellLeftEdgeStrokeTint" : pct().default_num(100.0).idml(idml![attr(Cell, LeftEdgeStrokeTint)]),
        CellRightEdgeStrokeColor => "cellRightEdgeStrokeColor" : color().idml(idml![attr(Cell | CellStyle, RightEdgeStrokeColor)]),
        CellRightEdgeStrokeWeight => "cellRightEdgeStrokeWeight" : length_pos().idml(idml![attr(Cell | CellStyle, RightEdgeStrokeWeight)]),
        CellRightEdgeStrokeTint => "cellRightEdgeStrokeTint" : pct().default_num(100.0).idml(idml![attr(Cell, RightEdgeStrokeTint)]),
        PluginMetadata => "pluginMetadata" : structure("PluginMetadata", Wire::PluginMetadata).required(),
        AnchoredPosition => "anchoredPosition" : enum_open(ANCHORED_POSITIONS).default_text("InlinePosition").idml(idml![attr(AnchoredObjectSetting, AnchoredPosition)]),
        AnchorPoint => "anchorPoint" : enum_open(REFERENCE_POINTS).idml(idml![attr(AnchoredObjectSetting, AnchorPoint)]),
        AnchoredXOffset => "anchoredXOffset" : length().default_num(0.0).idml(idml![attr(AnchoredObjectSetting, AnchorXoffset)]),
        AnchoredYOffset => "anchoredYOffset" : length().default_num(0.0).idml(idml![attr(AnchoredObjectSetting, AnchorYoffset)]),
        AnchoredHorizontalReference => "anchoredHorizontalReference" : text().idml(idml![attr(AnchoredObjectSetting, HorizontalReferencePoint)]),
        AnchoredVerticalReference => "anchoredVerticalReference" : text().idml(idml![attr(AnchoredObjectSetting, VerticalReferencePoint)]),
        AnchoredHorizontalAlignment => "anchoredHorizontalAlignment" : text().idml(idml![attr(AnchoredObjectSetting, HorizontalAlignment)]),
        AnchoredVerticalAlignment => "anchoredVerticalAlignment" : text().idml(idml![attr(AnchoredObjectSetting, VerticalAlignment)]),
        AnchoredSpineRelative => "anchoredSpineRelative" : bool_().default_bool(false).idml(idml![attr(AnchoredObjectSetting, SpineRelative)]),
        AnchoredLockPosition => "anchoredLockPosition" : bool_().default_bool(false).idml(idml![attr(AnchoredObjectSetting, LockPosition)]),
        ElementVisible => "elementVisible" : bool_().default_bool(true).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon, Visible)]),
        ElementLocked => "elementLocked" : bool_().default_bool(false).idml(idml![attr(TextFrame | Rectangle | Oval | GraphicLine | Polygon, Locked)]),

        // W0.1 / W0.2 — the character and paragraph attributes a
        // `storyRange:` address has always been able to set. PROMOTED
        // 2026-09-09: the apply layer routes them through the SAME arm
        // as `characterFontSize` and `paragraphSpaceBefore`, which were
        // advertised all along, and the Boa bridge's `js_value_to_wire`
        // already named every one of them. The only thing stopping a
        // script was that `lookup_path` did not know the name — so
        // `paged.set` answered `false` for the twenty-seven properties a
        // typesetter reaches for first: the font family, the case, the
        // underline, the indents, the drop cap, the tab stops, the
        // hyphenation, the bullets.
        //
        // `paged-script/tests/story_range_text_paths.rs` is the probe the
        // old reason asked for: it sets every one of them through
        // `paged.set` and reads it back.
        CharacterFontFamily => "characterFontFamily" : reference(RefKind::Font).idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, AppliedFont), child(CharacterStyleRange | ParagraphStyle | CharacterStyle, AppliedFont)]),
        CharacterFontStyle => "characterFontStyle" : text().idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, FontStyle), child(CharacterStyleRange, FontStyle)]),
        CharacterKerningMethod => "characterKerningMethod" : enum_open(KERNING_METHODS).default_text("Metrics").idml(idml![attr(CharacterStyleRange | CharacterStyle, KerningMethod)]),
        CharacterCase => "characterCase" : enum_open(CASES).default_text("Normal").idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, Capitalization)]),
        CharacterPosition => "characterPosition" : enum_open(POSITIONS).default_text("Normal").idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, Position)]),
        CharacterLanguage => "characterLanguage" : reference(RefKind::Language).idml(idml![attr(CharacterStyleRange, AppliedLanguage)]),
        CharacterBaselineShift => "characterBaselineShift" : length().default_num(0.0).idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, BaselineShift)]),
        CharacterHorizontalScale => "characterHorizontalScale" : pct().range(1.0, 1000.0).default_num(100.0).idml(idml![attr(CharacterStyleRange, HorizontalScale)]),
        CharacterVerticalScale => "characterVerticalScale" : pct().range(1.0, 1000.0).default_num(100.0).idml(idml![attr(CharacterStyleRange, VerticalScale)]),
        CharacterSkew => "characterSkew" : deg().range(-85.0, 85.0).default_num(0.0).idml(idml![attr(CharacterStyleRange, Skew)]),
        CharacterUnderline => "characterUnderline" : bool_().default_bool(false).idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, Underline)]),
        CharacterStrikethru => "characterStrikethru" : bool_().default_bool(false).idml(idml![attr(CharacterStyleRange | ParagraphStyle | CharacterStyle, StrikeThru)]),
        CharacterLigatures => "characterLigatures" : bool_().default_bool(true).idml(idml![attr(CharacterStyleRange | CharacterStyle, Ligatures)]),
        CharacterOtfFeatures => "characterOtfFeatures" : text(),
        ParagraphLeftIndent => "paragraphLeftIndent" : length().default_num(0.0).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, LeftIndent)]),
        ParagraphRightIndent => "paragraphRightIndent" : length().default_num(0.0).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, RightIndent)]),
        ParagraphDropCapCharacters => "paragraphDropCapCharacters" : count().range(0.0, 150.0).default_num(0.0).idml(idml![attr(ParagraphStyleRange, DropCapCharacters)]),
        ParagraphDropCapLines => "paragraphDropCapLines" : count().range(0.0, 25.0).default_num(0.0).idml(idml![attr(ParagraphStyleRange, DropCapLines)]),
        ParagraphHyphenation => "paragraphHyphenation" : bool_().default_bool(true).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, Hyphenation)]),
        ParagraphHyphenationZone => "paragraphHyphenationZone" : length_pos().idml(idml![attr(ParagraphStyleRange | ParagraphStyle, HyphenationZone)]),
        ParagraphKeepLinesTogether => "paragraphKeepLinesTogether" : bool_().default_bool(false).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, KeepLinesTogether)]),
        ParagraphKeepWithNext => "paragraphKeepWithNext" : count().range(0.0, 5.0).default_num(0.0).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, KeepWithNext)]),
        ParagraphKeepAllLinesTogether => "paragraphKeepAllLinesTogether" : bool_().default_bool(false).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, KeepAllLinesTogether)]),
        ParagraphKeepFirstLines => "paragraphKeepFirstLines" : count().range(1.0, 50.0).default_num(2.0).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, KeepFirstLines)]),
        ParagraphKeepLastLines => "paragraphKeepLastLines" : count().range(1.0, 50.0).default_num(2.0).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, KeepLastLines)]),
        ParagraphStartParagraph => "paragraphStartParagraph" : enum_(START_PARAGRAPH).default_text("Anywhere").idml(idml![attr(ParagraphStyleRange | ParagraphStyle, StartParagraph)]),
        ParagraphSpanColumnType => "paragraphSpanColumnType" : enum_(SPAN_COLUMN_TYPE).default_text("SingleColumn").idml(idml![attr(ParagraphStyleRange | ParagraphStyle, SpanColumnType)]),
        ParagraphSpanSplitColumnCount => "paragraphSpanSplitColumnCount" : count().min(1.0).keywords(&["All"]).idml(idml![child(ParagraphStyleRange | ParagraphStyle, SpanSplitColumnCount)]),
        ParagraphSpanColumnMinSpaceBefore => "paragraphSpanColumnMinSpaceBefore" : length_pos().idml(idml![attr(ParagraphStyleRange | ParagraphStyle, SpanColumnMinSpaceBefore)]),
        ParagraphSpanColumnMinSpaceAfter => "paragraphSpanColumnMinSpaceAfter" : length_pos().idml(idml![attr(ParagraphStyleRange | ParagraphStyle, SpanColumnMinSpaceAfter)]),
        ParagraphSplitColumnInsideGutter => "paragraphSplitColumnInsideGutter" : length_pos().idml(idml![attr(ParagraphStyleRange | ParagraphStyle, SplitColumnInsideGutter)]),
        ParagraphSplitColumnOutsideGutter => "paragraphSplitColumnOutsideGutter" : length_pos().idml(idml![attr(ParagraphStyleRange | ParagraphStyle, SplitColumnOutsideGutter)]),
        ParagraphRuleAbove => "paragraphRuleAbove" : structure("ParagraphRuleSpec", Wire::ParagraphRule),
        ParagraphRuleBelow => "paragraphRuleBelow" : structure("ParagraphRuleSpec", Wire::ParagraphRule),
        ParagraphTabStops => "paragraphTabStops" : list_of("TabStopSpec", Wire::TabStops),
        ParagraphListType => "paragraphListType" : enum_(LIST_TYPES).default_text("NoList").idml(idml![attr(ParagraphStyleRange | ParagraphStyle, BulletsAndNumberingListType)]),
        ParagraphBulletCharacter => "paragraphBulletCharacter" : text().idml(idml![child(ParagraphStyleRange | ParagraphStyle, BulletChar)]),
        ParagraphNumberingFormat => "paragraphNumberingFormat" : text().idml(idml![child(ParagraphStyleRange | ParagraphStyle, NumberingFormat), attr(ParagraphStyle, NumberingFormat)]),
        ParagraphBulletsTextAfter => "paragraphBulletsTextAfter" : text().idml(idml![attr(ParagraphStyleRange | ParagraphStyle, BulletsTextAfter), child(ParagraphStyleRange | ParagraphStyle, BulletsTextAfter)]),
        ParagraphNumberingExpression => "paragraphNumberingExpression" : text().idml(idml![attr(ParagraphStyleRange | ParagraphStyle, NumberingExpression), child(ParagraphStyleRange | ParagraphStyle, NumberingExpression)]),
        ParagraphNumberingStartAt => "paragraphNumberingStartAt" : count().min(0.0).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, NumberingStartAt)]),
        ParagraphNumberingContinue => "paragraphNumberingContinue" : bool_().default_bool(true).tristate().idml(idml![attr(ParagraphStyleRange | ParagraphStyle, NumberingContinue)]),
        ParagraphBulletsCharacterStyle => "paragraphBulletsCharacterStyle" : reference(RefKind::CharacterStyle).idml(idml![child(ParagraphStyleRange | ParagraphStyle, BulletsCharacterStyle), attr(ParagraphStyle, BulletsCharacterStyle)]),
        ParagraphNumberingCharacterStyle => "paragraphNumberingCharacterStyle" : reference(RefKind::CharacterStyle).idml(idml![child(ParagraphStyleRange | ParagraphStyle, NumberingCharacterStyle), attr(ParagraphStyle, BulletsAndNumberingDigitsCharacterStyle)]),
        // ADR 131/132 — the IDML `Name` of a page item; also the `name`
        // virtual path on frame / group targets.
        ItemName => "itemName" : text().idml(idml![attr(TextFrame | Rectangle | Oval | Polygon | GraphicLine | Group, Name)]),
        ParagraphComposer => "paragraphComposer" : enum_open(COMPOSERS).idml(idml![attr(ParagraphStyleRange | ParagraphStyle, Composer)]),
    }

    // ADR 132 — paths whose TARGET is not a page item: a layer, a page,
    // the document, a text variable, a style definition. `ElementId`
    // cannot name those, so they are not `paged.set(element, …)` paths
    // and not in `settablePaths`; they are reached through `Mutation::Set`
    // on the address kind named here, and published in `propertySchemas`.
    addressed {
        LayerVisible => "layerVisible" : bool_().default_bool(true).idml(idml![attr(Layer, Visible)]), "layer",
        LayerLocked => "layerLocked" : bool_().default_bool(false).idml(idml![attr(Layer, Locked)]), "layer",
        LayerPrintable => "layerPrintable" : bool_().default_bool(true).idml(idml![attr(Layer, Printable)]), "layer",
        LayerName => "layerName" : text().required().idml(idml![attr(Layer, Name)]), "layer",
        PageBounds => "pageBounds" : bounds().idml(idml![attr(Page, GeometricBounds)]), "page",
        PageMarginTop => "pageMarginTop" : length_pos().default_num(36.0).idml(idml![attr(MarginPreference, Top)]), "page",
        PageMarginBottom => "pageMarginBottom" : length_pos().default_num(36.0).idml(idml![attr(MarginPreference, Bottom)]), "page",
        PageMarginLeft => "pageMarginLeft" : length_pos().default_num(36.0).idml(idml![attr(MarginPreference, Left)]), "page",
        PageMarginRight => "pageMarginRight" : length_pos().default_num(36.0).idml(idml![attr(MarginPreference, Right)]), "page",
        PageColumnCount => "pageColumnCount" : count().range(1.0, 216.0).default_num(1.0).idml(idml![attr(MarginPreference, ColumnCount)]), "page",
        PageColumnGutter => "pageColumnGutter" : length_pos().default_num(12.0).idml(idml![attr(MarginPreference, ColumnGutter)]), "page",
        DocumentBleedTop => "documentBleedTop" : length_pos().default_num(0.0).idml(idml![attr(DocumentPreference, DocumentBleedTopOffset)]), "doc",
        DocumentBleedBottom => "documentBleedBottom" : length_pos().default_num(0.0).idml(idml![attr(DocumentPreference, DocumentBleedBottomOffset)]), "doc",
        DocumentBleedInsideOrLeft => "documentBleedInsideOrLeft" : length_pos().default_num(0.0).idml(idml![attr(DocumentPreference, DocumentBleedInsideOrLeftOffset)]), "doc",
        DocumentBleedOutsideOrRight => "documentBleedOutsideOrRight" : length_pos().default_num(0.0).idml(idml![attr(DocumentPreference, DocumentBleedOutsideOrRightOffset)]), "doc",
        DocumentSlugTop => "documentSlugTop" : length_pos().default_num(0.0).idml(idml![attr(DocumentPreference, SlugTopOffset)]), "doc",
        DocumentSlugBottom => "documentSlugBottom" : length_pos().default_num(0.0).idml(idml![attr(DocumentPreference, SlugBottomOffset)]), "doc",
        DocumentSlugInsideOrLeft => "documentSlugInsideOrLeft" : length_pos().default_num(0.0).idml(idml![attr(DocumentPreference, SlugInsideOrLeftOffset)]), "doc",
        DocumentSlugRightOrOutside => "documentSlugRightOrOutside" : length_pos().default_num(0.0).idml(idml![attr(DocumentPreference, SlugRightOrOutsideOffset)]), "doc",
        // R-11 — the document's global light.
        DocumentGlobalLightAngle => "documentGlobalLightAngle" : deg().default_num(120.0).idml(idml![attr(TransparencyPreference, GlobalLightAngle)]), "doc",
        DocumentGlobalLightAltitude => "documentGlobalLightAltitude" : deg().default_num(30.0).idml(idml![attr(TransparencyPreference, GlobalLightAltitude)]), "doc",
        TextVariableName => "textVariableName" : text().idml(idml![attr(TextVariable, Name)]), "textVariable",
        TextVariableType => "textVariableType" : enum_open(TEXT_VARIABLE_TYPES).idml(idml![attr(TextVariable, VariableType)]), "textVariable",
        TextVariableContents => "textVariableContents" : text().idml(idml![child(CustomTextVariablePreference, Contents)]), "textVariable",
        TextVariableTextBefore => "textVariableTextBefore" : text().idml(idml![attr(MatchParagraphStylePreference | MatchCharacterStylePreference | PageNumberVariablePreference | ChapterNumberVariablePreference | FileNameVariablePreference | DateVariablePreference | CustomTextVariablePreference, TextBefore)]), "textVariable",
        TextVariableTextAfter => "textVariableTextAfter" : text().idml(idml![attr(MatchParagraphStylePreference | MatchCharacterStylePreference | PageNumberVariablePreference | ChapterNumberVariablePreference | FileNameVariablePreference | DateVariablePreference | CustomTextVariablePreference, TextAfter)]), "textVariable",
        TextVariableDateFormat => "textVariableDateFormat" : text().idml(idml![attr(DateVariablePreference, Format)]), "textVariable",
        ObjectStyleStrokeTint => "objectStyleStrokeTint" : pct().idml(idml![attr(ObjectStyle, StrokeTint)]), "style:object",
        ObjectStyleCornerRadius => "objectStyleCornerRadius" : length_pos().idml(idml![attr(ObjectStyle, CornerRadius)]), "style:object",
        ObjectStyleCornerOption => "objectStyleCornerOption" : enum_open(CORNER_OPTIONS).idml(idml![attr(ObjectStyle, CornerOption)]), "style:object",
        CellStyleRotationAngle => "cellStyleRotationAngle" : deg().idml(idml![attr(CellStyle, RotationAngle)]), "style:cell",
        TableStyleHeaderRegionCellStyle => "tableStyleHeaderRegionCellStyle" : reference(RefKind::CellStyle).idml(idml![attr(TableStyle, HeaderRegionCellStyle)]), "style:table",
        TableStyleBodyRegionCellStyle => "tableStyleBodyRegionCellStyle" : reference(RefKind::CellStyle).idml(idml![attr(TableStyle, BodyRegionCellStyle)]), "style:table",
        TableStyleFooterRegionCellStyle => "tableStyleFooterRegionCellStyle" : reference(RefKind::CellStyle).idml(idml![attr(TableStyle, FooterRegionCellStyle)]), "style:table",
        TableStyleLeftColumnRegionCellStyle => "tableStyleLeftColumnRegionCellStyle" : reference(RefKind::CellStyle).idml(idml![attr(TableStyle, LeftColumnRegionCellStyle)]), "style:table",
        TableStyleRightColumnRegionCellStyle => "tableStyleRightColumnRegionCellStyle" : reference(RefKind::CellStyle).idml(idml![attr(TableStyle, RightColumnRegionCellStyle)]), "style:table",
        TableStyleTopBorderStrokeColor => "tableStyleTopBorderStrokeColor" : color().idml(idml![attr(TableStyle, TopBorderStrokeColor)]), "style:table",
        TableStyleTopBorderStrokeWeight => "tableStyleTopBorderStrokeWeight" : length_pos().idml(idml![attr(TableStyle, TopBorderStrokeWeight)]), "style:table",
        TableStyleBottomBorderStrokeColor => "tableStyleBottomBorderStrokeColor" : color().idml(idml![attr(TableStyle, BottomBorderStrokeColor)]), "style:table",
        TableStyleBottomBorderStrokeWeight => "tableStyleBottomBorderStrokeWeight" : length_pos().idml(idml![attr(TableStyle, BottomBorderStrokeWeight)]), "style:table",
        TableStyleLeftBorderStrokeColor => "tableStyleLeftBorderStrokeColor" : color().idml(idml![attr(TableStyle, LeftBorderStrokeColor)]), "style:table",
        TableStyleLeftBorderStrokeWeight => "tableStyleLeftBorderStrokeWeight" : length_pos().idml(idml![attr(TableStyle, LeftBorderStrokeWeight)]), "style:table",
        TableStyleRightBorderStrokeColor => "tableStyleRightBorderStrokeColor" : color().idml(idml![attr(TableStyle, RightBorderStrokeColor)]), "style:table",
        TableStyleRightBorderStrokeWeight => "tableStyleRightBorderStrokeWeight" : length_pos().idml(idml![attr(TableStyle, RightBorderStrokeWeight)]), "style:table",
        TableStyleAlternatingFills => "tableStyleAlternatingFills" : enum_(ALTERNATING_FILLS).idml(idml![attr(TableStyle, AlternatingFills)]), "style:table",
        TableStyleStartRowFillColor => "tableStyleStartRowFillColor" : color().idml(idml![attr(TableStyle, StartRowFillColor)]), "style:table",
        TableStyleStartRowFillCount => "tableStyleStartRowFillCount" : count().idml(idml![attr(TableStyle, StartRowFillCount)]), "style:table",
        TableStyleStartRowFillTint => "tableStyleStartRowFillTint" : pct().idml(idml![attr(TableStyle, StartRowFillTint)]), "style:table",
        TableStyleEndRowFillColor => "tableStyleEndRowFillColor" : color().idml(idml![attr(TableStyle, EndRowFillColor)]), "style:table",
        TableStyleEndRowFillCount => "tableStyleEndRowFillCount" : count().idml(idml![attr(TableStyle, EndRowFillCount)]), "style:table",
        TableStyleEndRowFillTint => "tableStyleEndRowFillTint" : pct().idml(idml![attr(TableStyle, EndRowFillTint)]), "style:table",
    }

    hidden {
        PathOpenAt => "pathOpenAt" : operation("PathOpenAt", Wire::PathOpenAt),
            "a path OPERATION carried as a `PropertyPath` for the apply/invert algebra; every surface reaches it through its own `Mutation`, not a property write",
        OutlineStroke => "outlineStroke" : operation("OutlineStroke", Wire::OutlineStroke),
            "a path OPERATION carried as a `PropertyPath` for the apply/invert algebra; every surface reaches it through its own `Mutation`, not a property write",
        OutlineStrokeVariable => "outlineStrokeVariable" : operation("OutlineStrokeVariable", Wire::OutlineStrokeVariable),
            "a path OPERATION carried as a `PropertyPath` for the apply/invert algebra; every surface reaches it through its own `Mutation`, not a property write",
        OffsetPath => "offsetPath" : operation("OffsetPath", Wire::OffsetPath),
            "a path OPERATION carried as a `PropertyPath` for the apply/invert algebra; every surface reaches it through its own `Mutation`, not a property write",
        SimplifyPath => "simplifyPath" : operation("SimplifyPath", Wire::SimplifyPath),
            "a path OPERATION carried as a `PropertyPath` for the apply/invert algebra; every surface reaches it through its own `Mutation`, not a property write",
        ClosePath => "closePath" : operation("ClosePath", Wire::ClosePath),
            "a path OPERATION carried as a `PropertyPath` for the apply/invert algebra; every surface reaches it through its own `Mutation`, not a property write",
        TableRowCount => "tableRowCount" : count().derived(),
            "READ-ONLY by contract: `SetProperty` carrying either table count is rejected, and structure edits go through Insert/DeleteTableRow and Insert/DeleteTableColumn. `settablePaths` means settable, so advertising them published a promise the apply layer refuses. `paged.get` still reads them — it matches on `wire_name`, not on this list",
        TableColumnCount => "tableColumnCount" : count().derived(),
            "READ-ONLY by contract: `SetProperty` carrying either table count is rejected, and structure edits go through Insert/DeleteTableRow and Insert/DeleteTableColumn. `settablePaths` means settable, so advertising them published a promise the apply layer refuses. `paged.get` still reads them — it matches on `wire_name`, not on this list",
        NextTextFrame => "nextTextFrame" : text().derived(),
            "NOT settable at all: the apply layer has no arm for it, so the old reason here — `settable on a TextFrame` — was untrue about the engine. And it could not usefully gain one: a forward pointer on its own does nothing, because the chain walk starts from the STORY, and `linkFrames` is what also rewrites the target's ParentStory. A raw property write would apply cleanly, change the model and move no pixels — the defect that lane already shipped once",
        PreviousTextFrame => "previousTextFrame" : text().derived(),
            "NOT settable at all, and nothing to set: `paged_model::TextFrame` carries no back-pointer. IDML has the attribute and the parser drops it, because a singly-linked chain plus the story is all the composer reads",
    }
}

/// The IDML elements the parser recognises, with their notable attributes. Hand
/// curated against `paged-parse` (spread.rs page items, plus structural Page /
/// Spread / Layer / Story and text-range styling) and cross-referenced to
/// [`PROPERTY_PATHS`] for the `settable_path` that mutates each — a test asserts
/// every cited path resolves, so the IDML⇄scripting cross-links can't dangle.
fn elements() -> Vec<ElementType> {
    const fn attr(
        name: &'static str,
        type_hint: &'static str,
        settable_path: Option<&'static str>,
        summary: &'static str,
    ) -> ElementAttr {
        ElementAttr {
            name,
            type_hint,
            settable_path,
            summary,
        }
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
                // `None` — NOT settable through the property-path lane.
                // A layer has no addressable `ElementId`, so these move
                // only through the dedicated `Layer*` mutations. See the
                // note in PROPERTY_PATHS.
                attr("Name", "string", None, "Layer name. Changed by the `layerSetName` mutation, not `paged.set` — a layer has no element address."),
                attr("Visible", "boolean", None, "Whether the layer's items render. Changed by `layerSetVisible`."),
                attr("Locked", "boolean", None, "Whether the layer's items are editable. Changed by `layerSetLocked`. NOTE: enforced at hit-test only — a dispatched mutation is not blocked by it."),
                attr("Printable", "boolean", None, "Whether the layer prints/exports. Changed by `layerSetPrintable`."),
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

    /// The 41 paths that have a name and are not advertised. Every one
    /// is settable — each has a working `apply` arm — so this list is a
    /// WORKLIST, and the ratchet below makes it shrink-only: promoting
    /// one is a deliberate edit, and nobody can quietly add a 42nd.
    ///
    /// It exists at all because the two name tables that preceded it
    /// disagreed silently. `paged.get` knew 217 names, `paged.set` and
    /// `catalog.json` knew 176, and the difference — `characterLanguage`,
    /// `characterUnderline`, `paragraphLeftIndent`, `paragraphTabStops`
    /// and 37 more — was not written down anywhere as either deliberate
    /// or accidental.
    // 41 → 14 on 2026-09-09: the twenty-seven `NodeId::StoryRange`
    // text paths were promoted (see the note in `advertised`). What is
    // left is the honest residue — eleven that are structurally not
    // property writes or have no address form, plus three still
    // carrying "no recorded reason".
    const UNADVERTISED: usize = 10;

    /// Every variant has exactly one name, and no two variants share one.
    ///
    /// `wire_name` is exhaustive by construction (the macro writes a
    /// `match` with no `_` arm), so totality is the compiler's job; what
    /// this adds is uniqueness, which a `match` cannot express — two
    /// arms may return the same string and nothing complains.
    #[test]
    fn every_path_has_one_name_and_no_name_has_two_paths() {
        let mut seen: std::collections::HashMap<&str, P> = std::collections::HashMap::new();
        for (name, path) in PROPERTY_PATHS {
            assert_eq!(
                wire_name(*path),
                *name,
                "advertised table and wire_name disagree for {path:?}"
            );
            if let Some(other) = seen.insert(name, *path) {
                panic!("{name:?} names two paths: {other:?} and {path:?}");
            }
        }
    }

    /// An unadvertised path carries a name AND a reason; an advertised
    /// one carries no reason. Both directions, so a path cannot be
    /// dropped from the published set without saying why, and cannot
    /// keep a stale excuse after it is published.
    #[test]
    fn the_unadvertised_are_named_and_explained_and_only_shrink() {
        let advertised: std::collections::HashSet<P> =
            PROPERTY_PATHS.iter().map(|(_, p)| *p).collect();
        let mut unadvertised = 0usize;
        for (_, path) in PROPERTY_PATHS {
            assert!(
                unadvertised_reason(*path).is_none(),
                "{path:?} is advertised and still carries a reason not to be"
            );
        }
        // Walk the names the macro generated: every path reachable by
        // name is either in the advertised table or has a reason.
        for path in ALL_PATHS {
            let name = wire_name(*path);
            assert!(!name.is_empty(), "{path:?} has an empty name");
            match unadvertised_reason(*path) {
                // Published either as an element path or (ADR 132) on
                // its own address kind.
                None => assert!(
                    advertised.contains(path) || addressed_on(*path).is_some(),
                    "{path:?} has no reason to be unadvertised but is not in the table"
                ),
                Some(reason) => {
                    assert!(
                        !advertised.contains(path),
                        "{path:?} is advertised AND carries a reason"
                    );
                    assert!(
                        reason.len() >= 60,
                        "{path:?}: an exemption costs a real sentence, got {reason:?}"
                    );
                    // "no recorded reason" was the honest state of 30 of
                    // these and the worklist for promoting them; the last
                    // was cleared on 2026-09-09. It may not come back: a
                    // path hidden without a decision is a capability the
                    // catalog withholds and cannot say why, which is the
                    // shape the whole 176-vs-217 split had.
                    assert!(
                        !reason.contains("no recorded reason"),
                        "{path:?} is hidden with no decision behind it. Either promote it \
                         (a probe through `paged.set` is the proof) or write what makes it \
                         unadvertisable: {reason:?}"
                    );
                    unadvertised += 1;
                }
            }
        }
        assert_eq!(
            unadvertised, UNADVERTISED,
            "the unadvertised list may only SHRINK. Promoting a path means \
             moving it into `advertised` and lowering UNADVERTISED in the same \
             commit; a new entry here means a capability was hidden without a \
             decision."
        );
        assert_eq!(
            ALL_PATHS.len(),
            PROPERTY_PATHS.len() + ADDRESSED_PATHS.len() + UNADVERTISED,
            "every named path is advertised, addressed or explained — exactly one"
        );
    }

    #[test]
    fn catalog_resolves_and_is_complete() {
        let cat = api_catalog();
        // 179 -> 175: the four unreachable `layer*` paths were removed
        // (C-33). A DROP in this count is the unusual direction and is
        // the whole point of the change — the catalog stopped claiming
        // four mutations no caller could perform.
        //
        // 175 -> 176: `itemLayer` (C-35, protocol 62). Note this is the
        // OPPOSITE of the C-33 removal above and is not a reversal of
        // it: those four addressed a LAYER, which has no `ElementId`
        // form, so no caller could name the target. This one addresses
        // the PAGE ITEM and carries a layer id as its value, so it is
        // reachable — `every_settable_path_is_addressable` is what
        // tells the two cases apart, and it passes.
        //
        // 203 -> 207: the ADR 028 keep options and break-before rule
        // (`paragraphKeepAllLinesTogether`, `paragraphKeepFirstLines`,
        // `paragraphKeepLastLines`, `paragraphStartParagraph`, protocol
        // 64) — modelled and rendered, but with no setter until now.
        //
        // 207 -> 213: span and split columns (`paragraphSpanColumnType`,
        // `paragraphSpanSplitColumnCount`, `paragraphSpanColumnMinSpace
        // Before` / `After`, `paragraphSplitColumnInside` / `Outside
        // Gutter`, protocol 64) — modelled (ef4cb28) and rendered
        // (a3e5a0c), but with no setter until now.
        //
        // 213 -> 219: the list-marker attributes set on a paragraph or a
        // paragraph style (`paragraphBulletsTextAfter`, `paragraph
        // NumberingExpression`, `paragraphNumberingStartAt`, `paragraph
        // NumberingContinue`, `paragraphBulletsCharacterStyle`,
        // `paragraphNumberingCharacterStyle`, protocol 64) — modelled on
        // the paragraph (d10ffc9) and rendered (6192c72), but with no
        // setter until now.
        //
        // 219 -> 220: `paragraphComposer` (protocol 65) — modelled and
        // laid out since fc2df8a (the Single-line Composer), with no
        // setter until now.
        //
        // 220 -> 221: `itemName` (ADR 131/132) — the IDML `Name` of a page
        // item, the stable handle a stored selector points at. The paths
        // ADR 132 closed on other kinds (page margins, document setup,
        // text variables, style definitions) are not element paths: they
        // are `propertySchemas` rows on their own address kinds.
        //
        // 221 -> 234: ADR 135 (R-10, R-11) — the fill, stroke and content
        // transparency targets' opacity and blend mode (6), a group's
        // isolate and knockout (2), and the drop shadow's blend mode,
        // spread, noise, knocked-out and global-light flag (5). The
        // document's global light is a `doc` schema row, not an element
        // path.
        assert_eq!(cat.settable_paths.len(), 234, "settable path count drifted");
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
                    assert!(
                        lookup_path(path).is_some(),
                        "{}.{} cites unknown settable path '{path}'",
                        el.name,
                        a.name
                    );
                }
            }
        }
    }

    /// C-33 — EVERY SETTABLE PATH MUST BE REACHABLE BY SOME ADDRESS.
    ///
    /// `element_settable_paths_resolve` above checks one link of the
    /// chain: that a cited path exists in `PROPERTY_PATHS`. It never
    /// checked the other, and the gap shipped: four `layer*` paths were
    /// listed as settable while `ElementId` has no Layer variant,
    /// `id_grammar()` publishes no layer form and `parse_element_id`
    /// cannot produce one. So the catalog advertised a mutation nobody
    /// could perform — and because docs.paged.media and the plugin SDK
    /// GENERATE from this catalog, the false claim propagated to every
    /// consumer that trusted it.
    ///
    /// The mapping below is EXPLICIT rather than inferred from the
    /// name, because IDML element names and address forms deliberately
    /// differ: a `ParagraphStyleRange` is written through
    /// `storyRange:<id>@<a>..<b>`. A first cut of this test matched on
    /// the name and reported that as a second bug; it is not one, and
    /// the false positive is why the table is spelled out. The cost of
    /// the table is the point — a NEW element type carrying settable
    /// paths cannot compile past this without someone classifying it.
    #[test]
    fn every_settable_path_is_addressable() {
        /// `Some(form)` — the published id form that reaches this
        /// element. `None` — NOT addressable, which is legal only if the
        /// element declares no settable path at all.
        fn address_form(element: &str) -> Option<&'static str> {
            match element {
                "TextFrame" => Some("textFrame:"),
                "Rectangle" => Some("rectangle:"),
                "Oval" => Some("oval:"),
                "Polygon" => Some("polygon:"),
                "GraphicLine" => Some("graphicLine:"),
                "Group" => Some("group:"),
                // Both style ranges are addressed as a character range
                // within a story — the paragraph paths apply to the
                // paragraphs that range touches.
                "CharacterStyleRange" | "ParagraphStyleRange" => Some("storyRange:"),
                // Structural: no ElementId variant, hence no way to name
                // one as a write target. Each must therefore declare
                // every attribute read-only.
                "Layer" | "Page" | "Spread" | "Story" => None,
                // Unknown element type — fail loudly rather than pass by
                // default, so adding one forces this decision.
                _ => Some("<<unclassified>>"),
            }
        }

        let published: Vec<&str> = id_grammar()
            .iter()
            .map(|f| f.form.split('<').next().unwrap_or_default())
            .collect();

        for el in elements() {
            let settable: Vec<&str> = el
                .attributes
                .iter()
                .filter_map(|a| a.settable_path)
                .collect();
            let form = address_form(el.name);
            match form {
                None => assert!(
                    settable.is_empty(),
                    "element type '{}' is NOT addressable (no ElementId variant, no id \
                     form) but declares settable paths {:?}. Nothing can name a target \
                     to write them. Set those attributes to `None` and document the \
                     mutation that really changes them.",
                    el.name,
                    settable
                ),
                Some(f) => assert!(
                    published.iter().any(|p| p.eq_ignore_ascii_case(f))
                        || published
                            .iter()
                            .any(|p| p.eq_ignore_ascii_case("textFrame:")
                                && matches!(
                                    el.name,
                                    "Rectangle" | "Oval" | "Polygon" | "GraphicLine"
                                )),
                    "element type '{}' maps to address form '{f}', which id_grammar() \
                     does not publish. Either publish it or reclassify the element.",
                    el.name
                ),
            }
        }
    }

    /// One capability, one name — or, where there are two, both doors
    /// take both.
    ///
    /// `PropertyPath` derives its wire spelling from the variant and the
    /// catalog chooses its advertised name by hand, and for eight paths
    /// those disagree. Before this, a script that read `paged.inspect`'s
    /// `frameInnerShadowEnabled` and handed it back to `paged.set` was
    /// told the path did not exist, and a `paged.batch` written with the
    /// advertised `frameInnerShadow` failed to deserialise. Neither door
    /// took the other's word.
    #[test]
    fn every_advertised_path_resolves_under_both_of_its_names() {
        for (name, path) in PROPERTY_PATHS {
            assert_eq!(
                lookup_path(name),
                Some(*path),
                "{name} is advertised and does not resolve"
            );
            if let Some(alias) = wire_alias(*path) {
                assert_eq!(
                    lookup_path(&alias),
                    Some(*path),
                    "{name} travels the wire as {alias} and that spelling is rejected"
                );
            }
        }
    }

    /// The name a caller READS BACK is a name they can WRITE.
    ///
    /// `wire_alias` computes the raw spelling by lowercasing the first
    /// character, which is what `rename_all = "camelCase"` does to a
    /// PascalCase variant — but "is what it does" is an assumption, and
    /// this asks serde itself. `PropertyDescriptor.path` serialises with
    /// that derive, so this is precisely the round trip a script makes
    /// when it inspects an element and then sets one of the properties
    /// it just read.
    #[test]
    fn the_name_serde_emits_is_a_name_lookup_accepts() {
        for (advertised, path) in PROPERTY_PATHS {
            let emitted = serde_json::to_value(path).expect("a path serialises");
            let emitted = emitted.as_str().expect("as a string");
            assert_eq!(
                lookup_path(emitted),
                Some(*path),
                "{advertised} serialises as {emitted}, which `paged.set` rejects"
            );
        }
    }

    /// An alias may not shadow another path's advertised name — that
    /// would silently retarget a write.
    #[test]
    fn no_alias_collides_with_an_advertised_name() {
        for (name, path) in PROPERTY_PATHS {
            let Some(alias) = wire_alias(*path) else {
                continue;
            };
            let clash = PROPERTY_PATHS
                .iter()
                .find(|(other, other_path)| *other == alias && other_path != path);
            assert!(
                clash.is_none(),
                "{name}'s wire alias {alias} is another path's advertised name"
            );
        }
    }

    /// The alias list may only SHRINK: two names for one capability is a
    /// wart, and the cure is to make the catalog name match the wire
    /// spelling (or the reverse) rather than to add a ninth.
    #[test]
    fn the_two_vocabularies_disagree_about_exactly_eight_paths() {
        let aliases = path_aliases();
        let names: Vec<&str> = aliases.iter().map(|a| a.name).collect();
        assert_eq!(
            aliases.len(),
            8,
            "the advertised and wire vocabularies now disagree about {} paths, not 8: {names:?}",
            aliases.len()
        );
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
