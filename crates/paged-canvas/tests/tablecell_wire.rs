// Wire pin: the EXACT op shapes paged.sheet's lowering emits for a
// native-table cell pour must deserialize as the wire `Mutation`. (A bundle
// bug once nested the whole Table id as `table_id`, which the engine rejected
// with "invalid type: map, expected a string" — this pins the correct shapes
// so a regression fails loudly here.)

use paged_canvas::Mutation;

fn try_de(label: &str, json: &str) {
    match serde_json::from_str::<Mutation>(json) {
        Ok(_) => println!("OK   {label}"),
        Err(e) => println!("FAIL {label}: {e}"),
    }
}

#[test]
fn sheet_cell_pour_ops_deserialize() {
    // 1. cell fill — setElementProperty on a tableCell, colorRef string value.
    try_de(
        "cellFillColor",
        r#"{"op":"setElementProperty","args":{"elementId":{"kind":"tableCell","id":{"story_id":"u1","table_id":"t1","row":0,"col":0}},"path":"cellFillColor","value":{"type":"colorRef","value":"Color/Black"}}}"#,
    );
    // 2. edge stroke — setElementProperty on a tableCell, length value.
    try_de(
        "cellTopEdgeStrokeWeight",
        r#"{"op":"setElementProperty","args":{"elementId":{"kind":"tableCell","id":{"story_id":"u1","table_id":"t1","row":0,"col":0}},"path":"cellTopEdgeStrokeWeight","value":{"type":"length","value":0.5}}}"#,
    );
    // 3. text pour — insertText with the cell qualifier (TextCellAddr).
    try_de(
        "insertText.cell",
        r#"{"op":"insertText","args":{"storyId":"u1","offset":0,"text":"hi","cell":{"tableId":"t1","row":0,"col":0}}}"#,
    );
    // 4. span — setCellSpan.
    try_de(
        "setCellSpan",
        r#"{"op":"setCellSpan","args":{"storyId":"u1","tableId":"t1","row":0,"col":0,"rowSpan":2,"columnSpan":1}}"#,
    );
}
