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

//! Scripting Stage 2 — end-to-end test of the embedded Boa
//! bridge. Loads a real fixture, runs JS that mutates a frame
//! property via `paged.set`, asserts the change landed in the
//! scene through the Operation channel.

use std::path::PathBuf;

use paged_canvas::{
    element_selection::{ElementId, SelectionMode},
    selection::ContentSelection,
    CanvasModel, CanvasOptions,
};
use paged_script::execute_script;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry-groups.idml")
}

fn load() -> CanvasModel {
    let bytes = std::fs::read(fixture_path()).expect("read fixture");
    CanvasModel::load("doc-script", &bytes, CanvasOptions::default()).expect("load + build")
}

const TEXT_FRAME_ID: &str = "ua365e1";

fn current_opacity(model: &CanvasModel) -> Option<f32> {
    let id = ElementId::TextFrame(TEXT_FRAME_ID.to_string());
    let props = model.element_properties(&id)?;
    let entry = props
        .entries
        .into_iter()
        .find(|e| matches!(e.path, paged_mutate::PropertyPath::FrameOpacity))?;
    match entry.value {
        Some(paged_mutate::Value::Length(opt)) => opt,
        _ => None,
    }
}

#[test]
// example: set-frame-fill
fn paged_set_via_js_routes_through_apply_layer() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"paged.set("textFrame:ua365e1", "frameOpacity", 50);"#,
    );
    assert!(result.error.is_none(), "script error: {:?}", result.error);
    assert_eq!(current_opacity(&model), Some(50.0));
}

#[test]
fn paged_frame_proxy_sugar_works() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const f = paged.frame("textFrame:ua365e1");
            f.frameOpacity = 30;
        "#,
    );
    assert!(result.error.is_none(), "script error: {:?}", result.error);
    assert_eq!(current_opacity(&model), Some(30.0));
}

/// W0.3 — a numeric frame-scope path (transform decompose) routes
/// through `paged.set` as a `Value::Length`, and an enum-string path
/// routes as `Value::Text`. Confirms the script encoder's path-aware
/// routing covers the new variants.
#[test]
fn w03_frame_paths_route_through_paged_set() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            paged.set("textFrame:ua365e1", "frameRotationAngle", 15);
            paged.set("textFrame:ua365e1", "textFrameVerticalJustification", "CenterAlign");
        "#,
    );
    assert!(result.error.is_none(), "script error: {:?}", result.error);
    let id = ElementId::TextFrame(TEXT_FRAME_ID.to_string());
    let props = model.element_properties(&id).expect("props");
    let angle = props
        .entries
        .iter()
        .find(|e| matches!(e.path, paged_mutate::PropertyPath::FrameRotationAngle))
        .and_then(|e| match &e.value {
            Some(paged_mutate::Value::Length(opt)) => *opt,
            _ => None,
        })
        .expect("rotation angle entry");
    assert!((angle - 15.0).abs() < 1e-2, "angle was {angle}");
    let vj = props
        .entries
        .iter()
        .find(|e| {
            matches!(
                e.path,
                paged_mutate::PropertyPath::TextFrameVerticalJustification
            )
        })
        .and_then(|e| match &e.value {
            Some(paged_mutate::Value::Text(s)) => Some(s.clone()),
            _ => None,
        })
        .expect("vertical justification entry");
    assert_eq!(vj, "CenterAlign");
}

/// W2.5 — the new element-visibility / text-wrap-contour paths route
/// through `paged.set` (bool paths as `Value::Bool`, the contour-type
/// enum string as `Value::Text`) and read back off
/// `element_properties` — proving the script-bridge maps + the
/// descriptor entries are wired together.
#[test]
fn w25_element_and_contour_paths_route_through_paged_set() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            paged.set("textFrame:ua365e1", "elementVisible", false);
            paged.set("textFrame:ua365e1", "elementLocked", true);
            paged.set("textFrame:ua365e1", "frameTextWrapMode", "ContourTextWrap");
            paged.set("textFrame:ua365e1", "frameTextWrapContourType", "DetectEdges");
            paged.set("textFrame:ua365e1", "frameTextWrapContourIncludeInside", true);
        "#,
    );
    assert!(result.error.is_none(), "script error: {:?}", result.error);
    let id = ElementId::TextFrame(TEXT_FRAME_ID.to_string());
    let props = model.element_properties(&id).expect("props");
    let bool_of = |path: paged_mutate::PropertyPath| -> Option<bool> {
        props
            .entries
            .iter()
            .find(|e| e.path == path)
            .and_then(|e| match &e.value {
                Some(paged_mutate::Value::Bool(b)) => Some(*b),
                _ => None,
            })
    };
    assert_eq!(
        bool_of(paged_mutate::PropertyPath::ElementVisible),
        Some(false)
    );
    assert_eq!(
        bool_of(paged_mutate::PropertyPath::ElementLocked),
        Some(true)
    );
    assert_eq!(
        bool_of(paged_mutate::PropertyPath::FrameTextWrapContourIncludeInside),
        Some(true)
    );
    let contour = props
        .entries
        .iter()
        .find(|e| e.path == paged_mutate::PropertyPath::FrameTextWrapContourType)
        .and_then(|e| match &e.value {
            Some(paged_mutate::Value::Text(s)) => Some(s.clone()),
            _ => None,
        })
        .expect("contour type entry");
    assert_eq!(contour, "DetectEdges");
}

#[test]
fn console_log_captured_into_output() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            console.log("hello", 1, true);
            console.warn("oops");
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(result.output.iter().any(|l| l.starts_with("[log] hello")));
    assert!(result.output.iter().any(|l| l.starts_with("[warn] oops")));
}

#[test]
// example: undo-redo
fn paged_undo_reverts_a_set() {
    let mut model = load();
    let before = current_opacity(&model);
    execute_script(
        &mut model,
        r#"paged.set("textFrame:ua365e1", "frameOpacity", 75);"#,
    );
    assert_eq!(current_opacity(&model), Some(75.0));
    execute_script(&mut model, "paged.undo();");
    assert_eq!(current_opacity(&model), before);
}

#[test]
fn script_syntax_error_surfaces_as_error_field() {
    let mut model = load();
    let result = execute_script(&mut model, "this is not js;");
    assert!(result.error.is_some());
}

// --- SDK Phase 5 (Task E): paged.collection / paged.documentMeta ---
//
// `paged.collection(name)` should produce the same JSON shape as the
// legacy hardcoded `paged.swatches()` / `paged.paragraphStyles()`
// etc. — both route through `CanvasModel::collection(name)`. The
// convergence thesis (sdk.md §11.1) says the script-side and the
// UI-side reach one Rust source; this test pins the equivalence.

#[test]
fn paged_collection_matches_named_accessor() {
    let mut model = load();
    let via_named = execute_script(&mut model, r#"paged.swatches();"#);
    assert!(via_named.error.is_none(), "{:?}", via_named.error);
    let via_generic = execute_script(&mut model, r#"paged.collection("swatches");"#);
    assert!(via_generic.error.is_none(), "{:?}", via_generic.error);
    let a = via_named.output.into_iter().next().expect("named output");
    let b = via_generic
        .output
        .into_iter()
        .next()
        .expect("generic output");
    let a_json: serde_json::Value = serde_json::from_str(&a).expect("named JSON");
    let b_json: serde_json::Value = serde_json::from_str(&b).expect("generic JSON");
    assert_eq!(a_json, b_json);
}

#[test]
fn paged_collection_unknown_name_returns_empty_array_with_warning() {
    let mut model = load();
    let result = execute_script(&mut model, r#"paged.collection("xyzNotAThing");"#);
    assert!(result.error.is_none(), "{:?}", result.error);
    // The output captures the warning emitted by the host fn AND the
    // script's terminal expression. Find the [] result.
    let parsed: Option<serde_json::Value> = result
        .output
        .iter()
        .find_map(|l| serde_json::from_str(l).ok());
    assert_eq!(parsed, Some(serde_json::Value::Array(Vec::new())));
    // The warning line is also captured.
    assert!(result
        .output
        .iter()
        .any(|l| l.contains("unknown collection")));
}

#[test]
fn paged_document_meta_returns_six_fields() {
    let mut model = load();
    let result = execute_script(&mut model, r#"paged.documentMeta();"#);
    assert!(result.error.is_none(), "{:?}", result.error);
    let line = result
        .output
        .into_iter()
        .next()
        .expect("documentMeta output");
    let parsed: serde_json::Value = serde_json::from_str(&line).expect("JSON");
    // Required keys per the §5.6 DocumentMeta struct.
    for key in [
        "pageCount",
        "activePage",
        "units",
        "colorMode",
        "documentName",
        "dirty",
    ] {
        assert!(
            parsed.get(key).is_some(),
            "missing documentMeta key {key}; got {parsed}"
        );
    }
    // pageCount should be > 0 for the fixture.
    assert!(parsed["pageCount"].as_u64().unwrap_or(0) >= 1);
}

// --- AC-2.1: parity diagnostic --------------------------------------
//
// `paged.inspect(id)` and the channel-side `RequestElementProperties`
// reply must serialize from the same Rust source data (`model.
// element_properties`). This test pins that convergence in: any future
// refactor that diverges the two surfaces breaks here loudly.

#[test]
fn paged_inspect_matches_element_properties_json() {
    let mut model = load();
    let id = ElementId::TextFrame(TEXT_FRAME_ID.to_string());

    // Path A — through the script bridge.
    let script_result = execute_script(&mut model, r#"paged.inspect("textFrame:ua365e1");"#);
    assert!(
        script_result.error.is_none(),
        "script error: {:?}",
        script_result.error
    );
    // The script's terminal expression is captured into output as a
    // formatted value; the bridge returns it as a JSON string.
    let inspect_line = script_result
        .output
        .into_iter()
        .next()
        .expect("paged.inspect produced no output line");
    let from_script: serde_json::Value =
        serde_json::from_str(&inspect_line).expect("script output is not JSON");

    // Path B — direct Rust accessor (what the channel handler hits).
    let direct = model
        .element_properties(&id)
        .expect("element_properties returned None for known fixture");
    let from_rust: serde_json::Value =
        serde_json::to_value(&direct).expect("element_properties serializes");

    assert_eq!(
        from_script, from_rust,
        "paged.inspect output diverged from element_properties:\n\
         script: {from_script}\nrust:   {from_rust}"
    );
}

// --- AC-2.2: paged.selection() reads current element selection ------

#[test]
fn paged_selection_returns_current_element_selection() {
    let mut model = load();
    let target = ElementId::TextFrame(TEXT_FRAME_ID.to_string());
    model
        .element_selection
        .apply_mode(std::slice::from_ref(&target), SelectionMode::Replace);

    let result = execute_script(&mut model, "paged.selection();");
    assert!(result.error.is_none(), "script error: {:?}", result.error);
    let line = result.output.into_iter().next().expect("no output line");
    let parsed: Vec<ElementId> = serde_json::from_str(&line).expect("selection JSON parses");
    assert_eq!(parsed, vec![target]);
}

#[test]
fn paged_selection_returns_empty_array_when_no_selection() {
    let mut model = load();
    model.element_selection.clear();
    let result = execute_script(&mut model, "paged.selection();");
    assert!(result.error.is_none(), "{:?}", result.error);
    let line = result.output.into_iter().next().expect("no output line");
    let parsed: Vec<ElementId> = serde_json::from_str(&line).expect("selection JSON parses");
    assert!(parsed.is_empty());
}

// --- AC-2.3: paged.contentSelection() reads current text caret ------

#[test]
fn paged_content_selection_returns_caret_when_set() {
    let mut model = load();
    let caret = ContentSelection::caret("story-1", 7);
    model.current_selection = Some(caret.clone());

    let result = execute_script(&mut model, "paged.contentSelection();");
    assert!(result.error.is_none(), "{:?}", result.error);
    let line = result.output.into_iter().next().expect("no output line");
    let parsed: ContentSelection =
        serde_json::from_str(&line).expect("content selection JSON parses");
    assert_eq!(parsed, caret);
}

#[test]
fn paged_content_selection_returns_null_when_unset() {
    let mut model = load();
    model.current_selection = None;
    let result = execute_script(&mut model, "paged.contentSelection();");
    assert!(result.error.is_none(), "{:?}", result.error);
    // Top-level `null` is a JS value but our formatter renders it as
    // the literal string "null"; that's what scripts see.
    let line = result.output.into_iter().next().expect("no output line");
    assert_eq!(line, "null");
}

/// SDK Phase 3 — `paged.set("storyRange:Story/u…@0..N", ...)` parses
/// the storyRange address and routes through the apply arm. Phase
/// 3.x landed partial-range run-splitting, so a range that cuts
/// inside a CharacterRun now succeeds (splits the run, applies the
/// property to the middle piece). The script issues a write +
/// verifies the change surfaces via `paged.inspect` end-to-end.
#[test]
fn paged_set_against_story_range_reaches_the_apply_arm() {
    let mut model = load();
    let story_id = model
        .scene()
        .stories
        .first()
        .map(|s| s.self_id.clone())
        .expect("fixture should contain at least one story");
    let source = format!(
        r#"
            // Write font size 24 to the first 3 chars of the story.
            const ok = paged.set("storyRange:{story_id}@0..3",
                                  "characterFontSize", 24);
            // Read it back through the snapshot. The range now has
            // a uniform font size after the partial-range split, so
            // the entry should be Some(Length(Some(24))).
            const props = JSON.parse(
                paged.inspect("storyRange:{story_id}@0..3")
            );
            const entry = props.entries.find(
                e => e.path === "characterFontSize",
            );
            console.log("write ok", ok);
            console.log("after value", JSON.stringify(entry && entry.value));
        "#,
    );
    let result = execute_script(&mut model, &source);
    assert!(result.error.is_none(), "{:?}", result.error);
    // Write succeeded.
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] write ok true")),
        "write didn't return true: {:?}",
        result.output
    );
    // Inspect shows the new value.
    let after = result
        .output
        .iter()
        .find(|l| l.contains("after value"))
        .expect("no after value line");
    // Either the entry was uniform (Some(Length(24.0))) or — if
    // the fixture's runs disagree past offset 3 — mixed (`null`).
    // The latter shouldn't happen here: we wrote 24 to exactly
    // [0, 3), so the snapshot of [0, 3) sees only the just-written
    // value. Assert it contains "24".
    assert!(
        after.contains("24"),
        "expected 24 in after-value line: {after}"
    );
}

/// SDK Phase 3 — `paged.inspect("storyRange:<id>@<start>..<end>")`
/// returns a populated `ElementProperties` with character entries.
/// `value` is `Option<Value>` — `None` for "mixed across runs",
/// `Some(...)` when every run in the range agrees (including the
/// "all agree on None" case). This test covers the happy path
/// where the fixture's first story is homogeneous over its first
/// few characters and the snapshot returns concrete values.
#[test]
fn paged_inspect_story_range_returns_character_entries() {
    let mut model = load();
    let story_id = model
        .scene()
        .stories
        .first()
        .map(|s| s.self_id.clone())
        .expect("fixture should contain at least one story");
    let result = execute_script(
        &mut model,
        &format!(
            r#"
                const json = paged.inspect("storyRange:{story_id}@0..3");
                const props = JSON.parse(json);
                // Print the entry count + each path so we can grep them.
                console.log("kind", props.kind);
                console.log("entries", props.entries.length);
                for (const e of props.entries) console.log("path", e.path);
            "#,
        ),
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    let kind_line = result
        .output
        .iter()
        .find(|l| l.starts_with("[log] kind"))
        .expect("no kind line");
    assert!(kind_line.contains("StoryRange"), "got: {kind_line}");
    let entries_line = result
        .output
        .iter()
        .find(|l| l.starts_with("[log] entries"))
        .expect("no entries line");
    // Scalar character paths (fontSize/leading/tracking/fillColor) +
    // W0.1 character formatting paths (14) + paragraph paths
    // (spaceBefore/spaceAfter/firstLineIndent/justification) + 2
    // applied-style paths + W0.2 paragraph formatting paths (13) =
    // 37 entries.
    assert!(entries_line.contains("37"), "got: {entries_line}");
    let path_lines: Vec<&String> = result
        .output
        .iter()
        .filter(|l| l.starts_with("[log] path"))
        .collect();
    assert_eq!(path_lines.len(), 37, "got: {:?}", path_lines);
    for needle in [
        "characterFontSize",
        "characterLeading",
        "characterTracking",
        "characterFillColor",
        // W0.1 character formatting.
        "characterFontFamily",
        "characterFontStyle",
        "characterKerningMethod",
        "characterCase",
        "characterPosition",
        "characterLanguage",
        "characterOtfFeatures",
        "characterBaselineShift",
        "characterHorizontalScale",
        "characterVerticalScale",
        "characterSkew",
        "characterUnderline",
        "characterStrikethru",
        "characterLigatures",
        "paragraphSpaceBefore",
        "paragraphSpaceAfter",
        "paragraphFirstLineIndent",
        "paragraphJustification",
        // W0.2 paragraph formatting.
        "paragraphLeftIndent",
        "paragraphRightIndent",
        "paragraphDropCapCharacters",
        "paragraphDropCapLines",
        "paragraphHyphenation",
        "paragraphKeepLinesTogether",
        "paragraphKeepWithNext",
        "paragraphRuleAbove",
        "paragraphRuleBelow",
        "paragraphTabStops",
        "paragraphListType",
        "paragraphBulletCharacter",
        "paragraphNumberingFormat",
        "appliedParagraphStyle",
        "appliedCharacterStyle",
    ] {
        assert!(
            path_lines.iter().any(|l| l.contains(needle)),
            "missing {needle} in {path_lines:?}"
        );
    }
}

/// SDK Phase 3 — `paged.stories()` enumerates loaded stories so
/// scripts (and tests) can pick valid StoryRange addresses.
/// Returns a JSON-encoded `StorySummary[]` with selfId +
/// characterCount + paragraphCount per story.
#[test]
fn paged_stories_lists_loaded_stories_with_character_counts() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const stories = JSON.parse(paged.stories());
            console.log("count", stories.length);
            if (stories.length > 0) {
                const s = stories[0];
                console.log("first selfId", typeof s.selfId);
                console.log("first chars", typeof s.characterCount);
                console.log("first paras", typeof s.paragraphCount);
            }
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    // The geometry-groups fixture has stories (per the paged.inspect
    // story-range tests). Assert the count line surfaced, plus the
    // shape of the first entry's keys.
    let count_line = result
        .output
        .iter()
        .find(|l| l.contains("[log] count"))
        .expect("no count line");
    // Extract the integer count.
    let n: usize = count_line
        .split_whitespace()
        .last()
        .and_then(|s| s.parse().ok())
        .expect("count not parseable");
    assert!(n > 0, "expected at least one story, got {n}");
    // Each entry has selfId (string), characterCount (number),
    // paragraphCount (number).
    for needle in [
        "first selfId string",
        "first chars number",
        "first paras number",
    ] {
        assert!(
            result.output.iter().any(|l| l.contains(needle)),
            "missing {needle} in {:?}",
            result.output
        );
    }
}

/// SDK Phase 3 — `paged.swatches()` enumerates the document's
/// colour palette. First implementation of the documentCollection
/// read kind per
/// docs/paged/panel-catalog-and-sdk-extension.md §5.1.
#[test]
fn paged_swatches_lists_palette_entries() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const swatches = JSON.parse(paged.swatches());
            console.log("count", swatches.length);
            if (swatches.length > 0) {
                const s = swatches[0];
                console.log("first selfId", typeof s.selfId);
                console.log("first name", typeof s.name);
                console.log("first kind", typeof s.kind);
            }
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    // The geometry-groups fixture has at least the built-in
    // specials (None, Paper, Black, Registration).
    let count_line = result
        .output
        .iter()
        .find(|l| l.contains("[log] count"))
        .expect("no count line");
    let n: usize = count_line
        .split_whitespace()
        .last()
        .and_then(|s| s.parse().ok())
        .expect("count not parseable");
    assert!(n > 0, "expected at least one swatch, got {n}");
    // Each entry has selfId / name / kind as strings.
    for needle in [
        "first selfId string",
        "first name string",
        "first kind string",
    ] {
        assert!(
            result.output.iter().any(|l| l.contains(needle)),
            "missing {needle} in {:?}",
            result.output
        );
    }
}

/// SDK Phase 3 — paragraphStyles + characterStyles + gradients
/// host fns return JSON-encoded summary arrays. Same shape as
/// paged.swatches; this test exercises all three so a single
/// regression in the host-fn registration surfaces here.
#[test]
fn paged_collection_host_fns_all_return_arrays() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const calls = [
                ["paragraphStyles", paged.paragraphStyles()],
                ["characterStyles", paged.characterStyles()],
                ["gradients", paged.gradients()],
            ];
            for (const [name, raw] of calls) {
                const parsed = JSON.parse(raw);
                console.log(name, Array.isArray(parsed));
            }
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    for needle in [
        "[log] paragraphStyles true",
        "[log] characterStyles true",
        "[log] gradients true",
    ] {
        assert!(
            result.output.iter().any(|l| l.contains(needle)),
            "missing {needle} in {:?}",
            result.output
        );
    }
}

/// Parser sanity — malformed storyRange addresses return null /
/// false through paged.set rather than panicking the script.
#[test]
fn paged_set_with_malformed_story_range_returns_false() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const a = paged.set("storyRange:no-at-sign", "characterFontSize", 12);
            const b = paged.set("storyRange:Story/u1@notanumber..3", "characterFontSize", 12);
            const c = paged.set("storyRange:Story/u1@5..3", "characterFontSize", 12);  // end <= start
            console.log("results", a, b, c);
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    // All three should resolve to `false` because parse_element_id
    // returns None and the bridge's `paged.set` returns false on
    // parse failure.
    let line = result
        .output
        .into_iter()
        .find(|l| l.contains("results"))
        .expect("no results line");
    assert!(line.contains("false false false"), "got: {line}");
}

/// W1.20 (groups v2) — `paged.set("group:<id>", "groupTransform",
/// [a,b,c,d,tx,ty])` routes to the dedicated `SetGroupTransform`
/// mutation: the group's own transform is set AND its members' effective
/// transforms compose the delta (move as a unit). Distinct from
/// `frameTransform` on a group, which only stores the group's metadata.
#[test]
fn paged_set_group_transform_moves_the_group_as_a_unit() {
    let mut model = load();
    // Discover a parsed group id + one of its leaf members.
    let (group_id, leaf_id, leaf_before) = {
        let mut found = None;
        'spreads: for parsed in &model.scene().spreads {
            let spread = &parsed.spread;
            for g in &spread.groups {
                if let Some(gid) = &g.self_id {
                    if let Some(paged_model::FrameRef::Rectangle(i)) = g
                        .members
                        .iter()
                        .find(|m| matches!(m, paged_model::FrameRef::Rectangle(_)))
                    {
                        if let Some(r) = spread.rectangles.get(*i) {
                            if let Some(rid) = &r.self_id {
                                found = Some((gid.clone(), rid.clone(), r.item_transform));
                                break 'spreads;
                            }
                        }
                    }
                }
            }
        }
        found.expect("geometry-groups fixture has a group with a rectangle member")
    };

    let script = format!(
        r#"const ok = paged.set("group:{group_id}", "groupTransform", [1, 0, 0, 1, 64, 48]); console.log("ok=" + ok);"#
    );
    let result = execute_script(&mut model, &script);
    assert!(result.error.is_none(), "script error: {:?}", result.error);
    assert!(
        result.output.iter().any(|l| l.contains("ok=true")),
        "groupTransform set should succeed: {:?}",
        result.output
    );

    // The group's own transform now carries (64, 48), and the member's
    // effective transform shifted by the same delta from its prior value.
    let mut group_t = None;
    let mut leaf_after = None;
    for parsed in &model.scene().spreads {
        for g in &parsed.spread.groups {
            if g.self_id.as_deref() == Some(group_id.as_str()) {
                group_t = Some(g.item_transform);
            }
        }
        for r in &parsed.spread.rectangles {
            if r.self_id.as_deref() == Some(leaf_id.as_str()) {
                leaf_after = Some(r.item_transform);
            }
        }
    }
    let group_t = group_t.flatten().expect("group transform set");
    assert!(
        (group_t[4] - 64.0).abs() < 1e-3 && (group_t[5] - 48.0).abs() < 1e-3,
        "group own transform set to (64,48): {group_t:?}"
    );
    // delta = g_new * inv(g_old); for the geometry-groups variant the
    // member is identity-local so it lands rigidly shifted. We just
    // assert it CHANGED (the precise composition is covered by the
    // paged-mutate suite).
    let leaf_after = leaf_after.expect("member rectangle still present");
    assert_ne!(
        leaf_before, leaf_after,
        "the group member's effective transform must follow the group move"
    );
}

// ----------------------------------------------------------------- Stage 1
// Text authoring host fns: paged.insertText / deleteRange / insertTextFrame.

#[test]
fn paged_insert_text_adds_characters_to_a_story() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const stories = JSON.parse(paged.stories());
            const sid = stories[0].selfId;
            const before = stories[0].characterCount;
            const ok = paged.insertText(sid, 0, "HELLO");
            const after = JSON.parse(paged.stories()).find(s => s.selfId === sid).characterCount;
            console.log("inserted", ok, after - before);
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] inserted true 5")),
        "expected insertText to add 5 chars; got {:?}",
        result.output
    );
}

#[test]
fn paged_delete_range_removes_characters_from_a_story() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const sid = JSON.parse(paged.stories())[0].selfId;
            paged.insertText(sid, 0, "ABCDE");
            const mid = JSON.parse(paged.stories()).find(s => s.selfId === sid).characterCount;
            const ok = paged.deleteRange(sid, 0, 3);
            const after = JSON.parse(paged.stories()).find(s => s.selfId === sid).characterCount;
            console.log("deleted", ok, mid - after);
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] deleted true 3")),
        "expected deleteRange to remove 3 chars; got {:?}",
        result.output
    );
}

#[test]
fn paged_insert_text_frame_mutates_the_scene() {
    let mut model = load();
    let page_id = model.page_ids().next().expect("a page").0.clone();
    let before = model.current_state_hash();
    let source = format!(
        r#"
            const id = paged.insertTextFrame({page_id:?}, [10, 10, 120, 200]);
            console.log("frame", id);
        "#
    );
    let result = execute_script(&mut model, &source);
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] frame textFrame:")),
        "insertTextFrame should return the new textFrame:<id> address; got {:?}",
        result.output
    );
    // The new frame changes the canonical scene state.
    assert_ne!(
        before,
        model.current_state_hash(),
        "inserting a text frame must change the scene state hash"
    );
}

// ----------------------------------------------------------------- Stage 2
// Structural authoring: insertFrame / insertPage / placeImage / applyStyle /
// createGroup.

#[test]
fn paged_insert_frame_and_page_author_structure() {
    let mut model = load();
    let page_id = model.page_ids().next().expect("a page").0.clone();
    let pages_before = model.page_ids().count();
    let hash_before = model.current_state_hash();
    let source = format!(
        r#"
            const f = paged.insertFrame({page_id:?}, [20, 20, 80, 120]);
            const p = paged.insertPage();
            console.log("frame", f, "page", p);
        "#
    );
    let result = execute_script(&mut model, &source);
    assert!(result.error.is_none(), "{:?}", result.error);
    // insertFrame now returns the new element address (rectangle:<id>) and
    // insertPage the new page selfId — both truthy strings, not bare booleans.
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] frame rectangle:") && !l.contains("page null")),
        "insertFrame should return a kind:id address and insertPage a page selfId; got {:?}",
        result.output
    );
    assert_ne!(
        hash_before,
        model.current_state_hash(),
        "structural edits must change the scene state hash"
    );
    assert!(
        model.page_ids().count() > pages_before,
        "insertPage must grow the page count"
    );
}

#[test]
fn paged_stage2_authoring_fns_are_registered_and_callable() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            // Each call returns a boolean; best-effort args exercise the
            // registration + argument handling without depending on fixture
            // specifics (a real apply may or may not match a style).
            const sid = JSON.parse(paged.stories())[0].selfId;
            const ps = JSON.parse(paged.paragraphStyles());
            const styleRef = ps.length ? ps[0].selfId : "ParagraphStyle/none";
            const a = paged.applyStyle(sid, 0, 1, styleRef);
            const b = paged.placeImage("frame:does-not-exist", "file:///x.png");
            const c = paged.createGroup([]);          // <2 members → false, no throw
            console.log("types", typeof a, typeof b, typeof c);
            console.log("group-empty", c);
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] types boolean boolean boolean")),
        "all three host fns must return booleans: {:?}",
        result.output
    );
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] group-empty false")),
        "createGroup([]) must return false: {:?}",
        result.output
    );
}

// ----------------------------------------------------------------- complete
// mutation-surface host fns: delete / dissolve / tables / style CRUD /
// selection / shape inserts. One representative per family — a single
// regression in the registration or arg handling surfaces here.

#[test]
fn paged_delete_element_removes_a_frame() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const ok = paged.deleteElement("textFrame:ua365e1");
            const after = paged.inspect("textFrame:ua365e1");
            console.log("del", ok, after === null);
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] del true true")),
        "deleteElement should remove the frame: {:?}",
        result.output
    );
}

fn first_group_id(model: &CanvasModel) -> Option<String> {
    for parsed in &model.scene().spreads {
        for g in &parsed.spread.groups {
            if let Some(gid) = &g.self_id {
                return Some(gid.clone());
            }
        }
    }
    None
}

#[test]
fn paged_dissolve_group_ungroups() {
    let mut model = load();
    let gid = first_group_id(&model).expect("geometry-groups fixture has a group");
    let result = execute_script(
        &mut model,
        &format!(r#"console.log("dis", paged.dissolveGroup("group:{gid}"));"#),
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        result.output.iter().any(|l| l.contains("[log] dis true")),
        "dissolveGroup should succeed: {:?}",
        result.output
    );
    assert!(
        first_group_id(&model).as_deref() != Some(gid.as_str())
            || !model
                .scene()
                .spreads
                .iter()
                .flat_map(|s| &s.spread.groups)
                .any(|g| g.self_id.as_deref() == Some(gid.as_str())),
        "the dissolved group must be gone"
    );
}

#[test]
// example: insert-table
fn paged_insert_table_returns_a_table_id() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const sid = JSON.parse(paged.stories())[0].selfId;
            const tid = paged.insertTable(sid, { rows: 3, cols: 2 });
            console.log("table", typeof tid, tid !== null && tid.length > 0);
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] table string true")),
        "insertTable must return a non-empty id string: {:?}",
        result.output
    );
}

#[test]
// example: table-insert-row
fn paged_insert_table_row_extends_a_fresh_table() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const sid = JSON.parse(paged.stories())[0].selfId;
            const tid = paged.insertTable(sid, { rows: 2, cols: 2 });
            const ok = paged.insertTableRow(sid, tid, 1);
            console.log("row", ok);
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        result.output.iter().any(|l| l.contains("[log] row true")),
        "insertTableRow on a fresh table should succeed: {:?}",
        result.output
    );
}

#[test]
// example: create-paragraph-style
fn paged_create_paragraph_style_returns_id_and_appears_in_collection() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const id = paged.createParagraphStyle({ name: "Script Made" });
            const found = JSON.parse(paged.paragraphStyles()).some(s => s.selfId === id);
            console.log("style", typeof id, id !== null && id.length > 0);
            console.log("found", found);
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] style string true")),
        "createParagraphStyle must return a non-empty id: {:?}",
        result.output
    );
    assert!(
        result.output.iter().any(|l| l.contains("[log] found true")),
        "the created style must appear in paragraphStyles(): {:?}",
        result.output
    );
}

#[test]
// example: set-selection
fn paged_set_element_selection_is_reflected_by_paged_selection() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            paged.setElementSelection(["textFrame:ua365e1"]);
            paged.selection();
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    let line = result.output.into_iter().next().expect("no output line");
    let parsed: Vec<ElementId> = serde_json::from_str(&line).expect("selection JSON parses");
    assert_eq!(
        parsed,
        vec![ElementId::TextFrame(TEXT_FRAME_ID.to_string())]
    );
}

#[test]
fn paged_clear_selection_empties_paged_selection() {
    let mut model = load();
    model.element_selection.apply_mode(
        &[ElementId::TextFrame(TEXT_FRAME_ID.to_string())],
        SelectionMode::Replace,
    );
    let result = execute_script(
        &mut model,
        r#"
            paged.clearSelection();
            paged.selection();
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    let line = result.output.into_iter().next().expect("no output line");
    let parsed: Vec<ElementId> = serde_json::from_str(&line).expect("selection JSON parses");
    assert!(parsed.is_empty(), "clearSelection must empty the selection");
}

#[test]
fn paged_insert_oval_and_line_return_addresses() {
    let mut model = load();
    let page_id = model.page_ids().next().expect("a page").0.clone();
    let result = execute_script(
        &mut model,
        &format!(
            r#"
                const o = paged.insertOval({page_id:?}, [10, 10, 60, 80]);
                const l = paged.insertLine({page_id:?}, [0, 0], [50, 50]);
                console.log("oval", o);
                console.log("line", l);
            "#
        ),
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(
        result.output.iter().any(|l| l.contains("[log] oval oval:")),
        "insertOval must return an oval: address: {:?}",
        result.output
    );
    assert!(
        result
            .output
            .iter()
            .any(|l| l.contains("[log] line graphicLine:")),
        "insertLine must return a graphicLine: address: {:?}",
        result.output
    );
}
