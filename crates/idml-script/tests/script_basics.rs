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
