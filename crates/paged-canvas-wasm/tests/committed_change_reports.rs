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

//! ADR 159 (v0.71.13, additive on protocol 71) — four reads a consumer
//! needs to follow the document without re-reading all of it, driven
//! through the worker dispatch exactly as the browser worker drives it: a
//! JSON `MainToWorker` in, a JSON `WorkerToMain` out.
//!
//! 1. `gestureCommitted.reflow` — a frame resized by its handles reports
//!    the content-box reflow the equivalent `resizeFrame` reports.
//! 2. `scriptResult.writes` / `scriptResult.pageIds` — whether a script
//!    wrote, and the pages its writes changed.
//! 3. `PageSummary.pageOrigin` / `PageSummary.spreadId` — where a page
//!    sits in its spread.
//! 4. `tableHeaderRowCount` / `tableFooterRowCount` — a table's header
//!    and footer bands, next to `tableRowCount`.

use std::io::Write;

use paged_canvas_wasm::dispatch::WorkerCore;

fn frozen_clock() -> f64 {
    0.0
}

fn roundtrip(core: &mut WorkerCore, msg: &serde_json::Value) -> serde_json::Value {
    let input = serde_json::to_string(msg).unwrap();
    let (reply, _effect) = core.handle_message(&input, &frozen_clock);
    serde_json::from_str(&reply).expect("reply must be valid JSON")
}

/// The protocol the dispatch stamps on every reply, read off a `hello`.
fn protocol() -> u64 {
    let mut core = WorkerCore::new();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({ "seq": 1, "protocol": 0, "kind": "hello", "payload": null }),
    );
    reply["protocol"].as_u64().expect("protocol on every reply")
}

/// Send `kind` + `payload` and return the reply.
fn send(
    core: &mut WorkerCore,
    seq: u64,
    kind: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    roundtrip(
        core,
        &serde_json::json!({ "seq": seq, "protocol": protocol(), "kind": kind, "payload": payload }),
    )
}

/// A minimal IDML package: one spread file, the given stories.
fn idml(spread: &str, stories: &[(&str, String)]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        )
        .unwrap();
        let ns = "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging";
        let story_refs: String = stories
            .iter()
            .map(|(id, _)| {
                format!(r#"<idPkg:Story src="Stories/Story_{id}.xml" xmlns:idPkg="{ns}"/>"#)
            })
            .collect();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="{ns}"/>
{story_refs}
</Document>"#
            )
            .as_bytes(),
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="{ns}" DOMVersion="13.1">
{spread}
</idPkg:Spread>"#
            )
            .as_bytes(),
        )
        .unwrap();
        for (id, body) in stories {
            zip.start_file(format!("Stories/Story_{id}.xml"), opts)
                .unwrap();
            zip.write_all(
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="{ns}" DOMVersion="13.1">
<Story Self="{id}">{body}</Story></idPkg:Story>"#
                )
                .as_bytes(),
            )
            .unwrap();
        }
        zip.finish().unwrap();
    }
    buf
}

fn plain_story(text: &str) -> String {
    format!(
        "<ParagraphStyleRange><CharacterStyleRange><Content>{text}</Content>\
         </CharacterStyleRange></ParagraphStyleRange>"
    )
}

/// Load `bytes` with Inter as the default face, so text lays out and a
/// text edit changes the pages its story is on.
fn load(bytes: Vec<u8>) -> WorkerCore {
    let inter = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("corpus/fonts/Inter.ttf");
    let mut core = WorkerCore::new();
    let reply = send(
        &mut core,
        2,
        "loadDocument",
        serde_json::json!({ "bytes": bytes, "font": inter }),
    );
    assert_eq!(reply["kind"], "documentLoaded", "{reply}");
    core
}

// ---------------------------------------------------------------------
// 1. gestureCommitted.reflow
// ---------------------------------------------------------------------

/// One page; `story1` threads from `tf1` into `tf2`, so resizing `tf1`
/// moves where the story breaks — the case a paginating consumer
/// (a table split across a frame chain) must re-split on.
fn threaded() -> WorkerCore {
    let spread = r#"<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" NextTextFrame="tf2" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf2" ParentStory="story1" GeometricBounds="420 100 700 400" ItemTransform="1 0 0 1 0 0"/>
</Spread>"#;
    let text = "The story runs on from the first frame into the second. ".repeat(40);
    load(idml(spread, &[("story1", plain_story(&text))]))
}

/// Begin `gesture` on `tf1`, move it by `delta` with snapping off, commit.
/// Returns the `gestureCommitted` reply.
fn gesture_on_tf1(
    core: &mut WorkerCore,
    gesture: serde_json::Value,
    delta: (f32, f32),
) -> serde_json::Value {
    let begun = send(
        core,
        10,
        "beginGesture",
        serde_json::json!({
            "nodes": [{ "kind": "textFrame", "id": "tf1" }],
            "gesture": gesture,
        }),
    );
    assert_eq!(begun["kind"], "gestureBegun", "{begun}");
    let handle = begun["payload"]["handle"].clone();
    let updated = send(
        core,
        11,
        "updateGesture",
        serde_json::json!({
            "handle": handle,
            "delta": [delta.0, delta.1],
            "modifiers": { "shift": false, "alt": false, "disableSnap": true },
        }),
    );
    assert_eq!(updated["kind"], "gestureUpdated", "{updated}");
    let committed = send(
        core,
        12,
        "commitGesture",
        serde_json::json!({ "handle": handle }),
    );
    assert_eq!(committed["kind"], "gestureCommitted", "{committed}");
    committed
}

#[test]
fn a_handle_resize_reports_the_reflow_its_resize_frame_reports() {
    // Drag tf1's bottom-right handle 30 pt right and 50 pt down.
    let mut core = threaded();
    let committed = gesture_on_tf1(
        &mut core,
        serde_json::json!({ "kind": "resize", "handle": "southEast" }),
        (30.0, 50.0),
    );
    let reflow = &committed["payload"]["reflow"];
    assert_eq!(reflow["frameId"], "tf1", "{committed}");
    assert_eq!(
        reflow["contentBox"],
        serde_json::json!([100.0, 100.0, 450.0, 430.0]),
        "{committed}"
    );

    // The same edit as a mutation, on a fresh document: the same reflow.
    let mut fresh = threaded();
    let applied = send(
        &mut fresh,
        20,
        "mutate",
        serde_json::json!({
            "op": "resizeFrame",
            "args": { "frameId": "tf1", "bounds": [100.0, 100.0, 450.0, 430.0] },
        }),
    );
    assert_eq!(applied["kind"], "mutationApplied", "{applied}");
    assert_eq!(applied["payload"]["reflow"], *reflow, "{applied}");
}

#[test]
fn a_handle_move_reports_no_reflow() {
    // A move changes display geometry only (the resize-vs-transform
    // ruling): `moveFrame` reports no reflow, and nor does a drag.
    let mut core = threaded();
    let committed = gesture_on_tf1(
        &mut core,
        serde_json::json!({ "kind": "translate" }),
        (30.0, 50.0),
    );
    assert!(
        committed["payload"].get("reflow").is_none(),
        "a move must not re-paginate: {committed}"
    );
}

// ---------------------------------------------------------------------
// 2. scriptResult.writes / scriptResult.pageIds
// ---------------------------------------------------------------------

/// Two pages on one spread; `story1` lives on page 1 (`tf1`) and
/// `story2` on page 2 (`tf2`).
fn two_stories() -> WorkerCore {
    let spread = r#"<Spread Self="s1" PageCount="2">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Page Self="p2" Name="2" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 612 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf2" ParentStory="story2" GeometricBounds="100 712 400 1012" ItemTransform="1 0 0 1 0 0"/>
</Spread>"#;
    load(idml(
        spread,
        &[
            ("story1", plain_story("First page")),
            ("story2", plain_story("Second page")),
        ],
    ))
}

fn run_script(core: &mut WorkerCore, source: &str) -> serde_json::Value {
    let reply = send(
        core,
        30,
        "executeScript",
        serde_json::json!({ "source": source }),
    );
    assert_eq!(reply["kind"], "scriptResult", "{reply}");
    assert_eq!(
        reply["payload"]["error"],
        serde_json::Value::Null,
        "{reply}"
    );
    reply
}

#[test]
fn a_read_only_script_reports_no_write_and_no_page() {
    let mut core = two_stories();
    let reply = run_script(
        &mut core,
        "const m = paged.documentMeta(); paged.get('textFrame:tf1', 'frameBounds'); console.log('read');",
    );
    assert_eq!(reply["payload"]["writes"], 0, "{reply}");
    assert_eq!(
        reply["payload"]["pageIds"],
        serde_json::json!([]),
        "{reply}"
    );
}

#[test]
fn a_writing_script_reports_its_writes_and_the_pages_they_changed() {
    let mut core = two_stories();
    // Two text edits of the story on page 2: the engine lays out only
    // that story's pages afresh, so page 1 is not named.
    let reply = run_script(
        &mut core,
        "paged.insertText('story2', 0, 'The '); paged.insertText('story2', 0, 'On ');",
    );
    assert_eq!(reply["payload"]["writes"], 2, "{reply}");
    assert_eq!(
        reply["payload"]["pageIds"],
        serde_json::json!(["p2"]),
        "{reply}"
    );
}

#[test]
fn a_script_write_the_engine_cannot_narrow_names_every_page() {
    // A frame write rebuilds every page, as `mutationApplied.pageIds`
    // names every page for it: the script reports that, not a guess.
    let mut core = two_stories();
    let reply = run_script(
        &mut core,
        "paged.set('textFrame:tf1', 'frameBounds', [100, 100, 420, 400]);",
    );
    assert_eq!(reply["payload"]["writes"], 1, "{reply}");
    assert_eq!(
        reply["payload"]["pageIds"],
        serde_json::json!(["p1", "p2"]),
        "{reply}"
    );
}

// ---------------------------------------------------------------------
// 3. PageSummary.pageOrigin / PageSummary.spreadId
// ---------------------------------------------------------------------

#[test]
fn a_page_says_where_it_sits_in_its_spread() {
    // Page 2 of a two-page spread starts where page 1 ends.
    let mut core = two_stories();
    let reply = send(
        &mut core,
        40,
        "requestCollection",
        serde_json::json!({ "name": "pages" }),
    );
    assert_eq!(reply["kind"], "collectionReply", "{reply}");
    let pages = reply["payload"]["items"].as_array().expect("pages");
    assert_eq!(pages.len(), 2, "{reply}");
    let width_1 = pages[0]["sizePt"][0].as_f64().unwrap();
    assert_eq!(
        pages[0]["pageOrigin"],
        serde_json::json!([0.0, 0.0]),
        "{reply}"
    );
    assert_eq!(pages[1]["pageOrigin"][0].as_f64(), Some(width_1), "{reply}");
    assert_eq!(pages[1]["pageOrigin"][1].as_f64(), Some(0.0), "{reply}");
    assert_eq!(pages[0]["spreadId"], "s1", "{reply}");
    assert_eq!(pages[1]["spreadId"], "s1", "{reply}");
}

#[test]
fn a_page_origin_matches_an_item_geometrys_page_origin() {
    // The same convention as `ElementGeometryItem.pageOrigin`: the
    // frame on page 2 reports its page's origin, and the page agrees.
    let mut core = two_stories();
    let geometry = send(
        &mut core,
        41,
        "requestElementGeometry",
        serde_json::json!({ "ids": [{ "kind": "textFrame", "id": "tf2" }] }),
    );
    let pages = send(
        &mut core,
        42,
        "requestCollection",
        serde_json::json!({ "name": "pages" }),
    );
    let item = &geometry["payload"]["items"][0];
    assert_eq!(item["pageId"], "p2", "{geometry}");
    assert_eq!(
        pages["payload"]["items"][1]["pageOrigin"], item["pageOrigin"],
        "{pages}"
    );
    assert_eq!(
        pages["payload"]["items"][1]["spreadId"], item["spreadId"],
        "{pages}"
    );
}

// ---------------------------------------------------------------------
// 4. tableHeaderRowCount / tableFooterRowCount
// ---------------------------------------------------------------------

/// `t1`: `header` header rows, two body rows, `footer` footer rows,
/// two columns.
fn table_doc(header: u32, footer: u32) -> WorkerCore {
    let spread = r#"<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 700 500" ItemTransform="1 0 0 1 0 0"/>
</Spread>"#;
    let rows = header + 2 + footer;
    let mut cells = String::new();
    for c in 0..2 {
        for r in 0..rows {
            cells.push_str(&format!(
                r#"<Cell Self="c{r}_{c}" Name="{c}:{r}"><ParagraphStyleRange><CharacterStyleRange><Content>{r}.{c}</Content></CharacterStyleRange></ParagraphStyleRange></Cell>"#
            ));
        }
    }
    let row_xml: String = (0..rows)
        .map(|r| format!(r#"<Row Self="r{r}" Name="{r}" SingleRowHeight="20"/>"#))
        .collect();
    let body = format!(
        r#"<ParagraphStyleRange><CharacterStyleRange>
<Table Self="t1" HeaderRowCount="{header}" BodyRowCount="2" FooterRowCount="{footer}" ColumnCount="2">
{row_xml}<Column Self="col0" Name="0" SingleColumnWidth="100"/><Column Self="col1" Name="1" SingleColumnWidth="100"/>
{cells}
</Table></CharacterStyleRange></ParagraphStyleRange>"#
    );
    load(idml(spread, &[("story1", body)]))
}

/// `RequestGet` one path on `table:t1`; the number it answers.
fn table_count(core: &mut WorkerCore, path: &str) -> f64 {
    let reply = send(
        core,
        50,
        "requestGet",
        serde_json::json!({ "address": "table:t1", "path": path }),
    );
    assert_eq!(reply["kind"], "propertyValue", "{reply}");
    assert_eq!(
        reply["payload"]["error"],
        serde_json::Value::Null,
        "{reply}"
    );
    reply["payload"]["value"]["value"]
        .as_f64()
        .unwrap_or_else(|| panic!("{path} is not a number: {reply}"))
}

#[test]
fn a_table_reads_its_header_and_footer_bands_from_idml() {
    let mut core = table_doc(2, 1);
    assert_eq!(table_count(&mut core, "tableHeaderRowCount"), 2.0);
    assert_eq!(table_count(&mut core, "tableFooterRowCount"), 1.0);
    // The total, for reference: header + body + footer.
    assert_eq!(table_count(&mut core, "tableRowCount"), 5.0);
}

#[test]
fn a_header_row_inserted_by_the_host_is_counted() {
    let mut core = table_doc(0, 0);
    assert_eq!(table_count(&mut core, "tableHeaderRowCount"), 0.0);
    let applied = send(
        &mut core,
        51,
        "mutate",
        serde_json::json!({
            "op": "insertHeaderRow",
            "args": { "storyId": "story1", "tableId": "t1" },
        }),
    );
    assert_eq!(applied["kind"], "mutationApplied", "{applied}");
    assert_eq!(table_count(&mut core, "tableHeaderRowCount"), 1.0);
    assert_eq!(table_count(&mut core, "tableFooterRowCount"), 0.0);
}

#[test]
fn the_inspector_lists_the_bands_with_the_table_counts() {
    let mut core = table_doc(2, 1);
    let reply = send(
        &mut core,
        52,
        "requestElementProperties",
        serde_json::json!({ "id": { "kind": "table", "id": { "story_id": "story1", "table_id": "t1" } } }),
    );
    let entries = reply["payload"]["result"]["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("no entries: {reply}"));
    let value_of = |path: &str| {
        entries
            .iter()
            .find(|e| e["path"] == path)
            .map(|e| e["value"]["value"].clone())
            .unwrap_or_else(|| panic!("{path} not listed: {reply}"))
    };
    assert_eq!(value_of("tableHeaderRowCount"), 2.0);
    assert_eq!(value_of("tableFooterRowCount"), 1.0);
}

#[test]
fn the_band_counts_are_read_only() {
    // Like `tableRowCount`: the bands change through
    // `insertHeaderRow` / `removeHeaderRow` (and the footer twins).
    let mut core = table_doc(1, 0);
    let reply = send(
        &mut core,
        53,
        "mutate",
        serde_json::json!({
            "op": "set",
            "args": { "address": "table:t1", "path": "tableHeaderRowCount", "value": { "type": "number", "value": 3 } },
        }),
    );
    assert_eq!(reply["kind"], "mutationFailed", "{reply}");
    assert_eq!(table_count(&mut core, "tableHeaderRowCount"), 1.0);
}
