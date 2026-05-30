//! Scripting Stage 2 — end-to-end test of the embedded Boa
//! bridge. Loads a real fixture, runs JS that mutates a frame
//! property via `verso.set`, asserts the change landed in the
//! scene through the Operation channel.

use std::path::PathBuf;

use idml_canvas::{
    element_selection::{ElementId, SelectionMode},
    selection::ContentSelection,
    CanvasModel, CanvasOptions,
};
use idml_script::execute_script;

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
    CanvasModel::load("doc-script", &bytes, CanvasOptions::default())
        .expect("load + build")
}

const TEXT_FRAME_ID: &str = "ua365e1";

fn current_opacity(model: &CanvasModel) -> Option<f32> {
    let id = ElementId::TextFrame(TEXT_FRAME_ID.to_string());
    let props = model.element_properties(&id)?;
    let entry = props
        .entries
        .into_iter()
        .find(|e| matches!(e.path, idml_mutate::PropertyPath::FrameOpacity))?;
    match entry.value {
        Some(idml_mutate::Value::Length(opt)) => opt,
        _ => None,
    }
}

#[test]
fn verso_set_via_js_routes_through_apply_layer() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"verso.set("textFrame:ua365e1", "frameOpacity", 50);"#,
    );
    assert!(
        result.error.is_none(),
        "script error: {:?}",
        result.error
    );
    assert_eq!(current_opacity(&model), Some(50.0));
}

#[test]
fn verso_frame_proxy_sugar_works() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const f = verso.frame("textFrame:ua365e1");
            f.frameOpacity = 30;
        "#,
    );
    assert!(
        result.error.is_none(),
        "script error: {:?}",
        result.error
    );
    assert_eq!(current_opacity(&model), Some(30.0));
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
fn verso_undo_reverts_a_set() {
    let mut model = load();
    let before = current_opacity(&model);
    execute_script(
        &mut model,
        r#"verso.set("textFrame:ua365e1", "frameOpacity", 75);"#,
    );
    assert_eq!(current_opacity(&model), Some(75.0));
    execute_script(&mut model, "verso.undo();");
    assert_eq!(current_opacity(&model), before);
}

#[test]
fn script_syntax_error_surfaces_as_error_field() {
    let mut model = load();
    let result = execute_script(&mut model, "this is not js;");
    assert!(result.error.is_some());
}

// --- AC-2.1: parity diagnostic --------------------------------------
//
// `verso.inspect(id)` and the channel-side `RequestElementProperties`
// reply must serialize from the same Rust source data (`model.
// element_properties`). This test pins that convergence in: any future
// refactor that diverges the two surfaces breaks here loudly.

#[test]
fn verso_inspect_matches_element_properties_json() {
    let mut model = load();
    let id = ElementId::TextFrame(TEXT_FRAME_ID.to_string());

    // Path A — through the script bridge.
    let script_result = execute_script(
        &mut model,
        r#"verso.inspect("textFrame:ua365e1");"#,
    );
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
        .expect("verso.inspect produced no output line");
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
        "verso.inspect output diverged from element_properties:\n\
         script: {from_script}\nrust:   {from_rust}"
    );
}

// --- AC-2.2: verso.selection() reads current element selection ------

#[test]
fn verso_selection_returns_current_element_selection() {
    let mut model = load();
    let target = ElementId::TextFrame(TEXT_FRAME_ID.to_string());
    model
        .element_selection
        .apply_mode(&[target.clone()], SelectionMode::Replace);

    let result = execute_script(&mut model, "verso.selection();");
    assert!(result.error.is_none(), "script error: {:?}", result.error);
    let line = result
        .output
        .into_iter()
        .next()
        .expect("no output line");
    let parsed: Vec<ElementId> =
        serde_json::from_str(&line).expect("selection JSON parses");
    assert_eq!(parsed, vec![target]);
}

#[test]
fn verso_selection_returns_empty_array_when_no_selection() {
    let mut model = load();
    model.element_selection.clear();
    let result = execute_script(&mut model, "verso.selection();");
    assert!(result.error.is_none(), "{:?}", result.error);
    let line = result
        .output
        .into_iter()
        .next()
        .expect("no output line");
    let parsed: Vec<ElementId> =
        serde_json::from_str(&line).expect("selection JSON parses");
    assert!(parsed.is_empty());
}

// --- AC-2.3: verso.contentSelection() reads current text caret ------

#[test]
fn verso_content_selection_returns_caret_when_set() {
    let mut model = load();
    let caret = ContentSelection::caret("story-1", 7);
    model.current_selection = Some(caret.clone());

    let result = execute_script(&mut model, "verso.contentSelection();");
    assert!(result.error.is_none(), "{:?}", result.error);
    let line = result
        .output
        .into_iter()
        .next()
        .expect("no output line");
    let parsed: ContentSelection =
        serde_json::from_str(&line).expect("content selection JSON parses");
    assert_eq!(parsed, caret);
}

#[test]
fn verso_content_selection_returns_null_when_unset() {
    let mut model = load();
    model.current_selection = None;
    let result = execute_script(&mut model, "verso.contentSelection();");
    assert!(result.error.is_none(), "{:?}", result.error);
    // Top-level `null` is a JS value but our formatter renders it as
    // the literal string "null"; that's what scripts see.
    let line = result
        .output
        .into_iter()
        .next()
        .expect("no output line");
    assert_eq!(line, "null");
}

/// SDK Phase 3 — `verso.set("storyRange:Story/u…@0..N", ...)` parses
/// the storyRange address and routes through the apply arm. Phase
/// 3.x landed partial-range run-splitting, so a range that cuts
/// inside a CharacterRun now succeeds (splits the run, applies the
/// property to the middle piece). The script issues a write +
/// verifies the change surfaces via `verso.inspect` end-to-end.
#[test]
fn verso_set_against_story_range_reaches_the_apply_arm() {
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
            const ok = verso.set("storyRange:{story_id}@0..3",
                                  "characterFontSize", 24);
            // Read it back through the snapshot. The range now has
            // a uniform font size after the partial-range split, so
            // the entry should be Some(Length(Some(24))).
            const props = JSON.parse(
                verso.inspect("storyRange:{story_id}@0..3")
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
        result.output.iter().any(|l| l.contains("[log] write ok true")),
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

/// SDK Phase 3 — `verso.inspect("storyRange:<id>@<start>..<end>")`
/// returns a populated `ElementProperties` with character entries.
/// `value` is `Option<Value>` — `None` for "mixed across runs",
/// `Some(...)` when every run in the range agrees (including the
/// "all agree on None" case). This test covers the happy path
/// where the fixture's first story is homogeneous over its first
/// few characters and the snapshot returns concrete values.
#[test]
fn verso_inspect_story_range_returns_character_entries() {
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
                const json = verso.inspect("storyRange:{story_id}@0..3");
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
    // 4 character paths (FontSize / Leading / Tracking / FillColor)
    // + 3 paragraph paths (SpaceBefore / SpaceAfter / FirstLineIndent).
    assert!(entries_line.contains("7"), "got: {entries_line}");
    let path_lines: Vec<&String> = result
        .output
        .iter()
        .filter(|l| l.starts_with("[log] path"))
        .collect();
    assert_eq!(path_lines.len(), 7, "got: {:?}", path_lines);
    for needle in [
        "characterFontSize",
        "characterLeading",
        "characterTracking",
        "characterFillColor",
        "paragraphSpaceBefore",
        "paragraphSpaceAfter",
        "paragraphFirstLineIndent",
    ] {
        assert!(
            path_lines.iter().any(|l| l.contains(needle)),
            "missing {needle} in {path_lines:?}"
        );
    }
}

/// SDK Phase 3 — `verso.stories()` enumerates loaded stories so
/// scripts (and tests) can pick valid StoryRange addresses.
/// Returns a JSON-encoded `StorySummary[]` with selfId +
/// characterCount + paragraphCount per story.
#[test]
fn verso_stories_lists_loaded_stories_with_character_counts() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const stories = JSON.parse(verso.stories());
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
    // The geometry-groups fixture has stories (per the verso.inspect
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
    for needle in ["first selfId string", "first chars number", "first paras number"] {
        assert!(
            result.output.iter().any(|l| l.contains(needle)),
            "missing {needle} in {:?}",
            result.output
        );
    }
}

/// SDK Phase 3 — `verso.swatches()` enumerates the document's
/// colour palette. First implementation of the documentCollection
/// read kind per
/// docs/verso/panel-catalog-and-sdk-extension.md §5.1.
#[test]
fn verso_swatches_lists_palette_entries() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const swatches = JSON.parse(verso.swatches());
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

/// Parser sanity — malformed storyRange addresses return null /
/// false through verso.set rather than panicking the script.
#[test]
fn verso_set_with_malformed_story_range_returns_false() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        r#"
            const a = verso.set("storyRange:no-at-sign", "characterFontSize", 12);
            const b = verso.set("storyRange:Story/u1@notanumber..3", "characterFontSize", 12);
            const c = verso.set("storyRange:Story/u1@5..3", "characterFontSize", 12);  // end <= start
            console.log("results", a, b, c);
        "#,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    // All three should resolve to `false` because parse_element_id
    // returns None and the bridge's `verso.set` returns false on
    // parse failure.
    let line = result
        .output
        .into_iter()
        .find(|l| l.contains("results"))
        .expect("no results line");
    assert!(line.contains("false false false"), "got: {line}");
}
