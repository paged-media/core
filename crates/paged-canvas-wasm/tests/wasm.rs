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

//! `wasm_bindgen_test` lane (audit B11). Covers what ONLY the wasm
//! environment exercises and the native `tests/dispatch.rs` cannot:
//!
//! - `CanvasWorker` construction across the wasm-bindgen boundary.
//! - `handleMessage` round-tripping a JSON envelope inside the wasm VM
//!   (the dispatch logic itself is natively tested; here we confirm the
//!   wasm-bindgen string-in/string-out marshalling works end to end).
//! - The `js_sys::Date` timing path: a `mutationApplied` reply carries a
//!   real `rebuildMs` instrumentation field produced by `Date::now()`,
//!   which has no native equivalent.
//!
//! These compile + run only on `wasm32-unknown-unknown` via
//! `scripts/test-wasm.sh` (Node-hosted `wasm-bindgen-test-runner`); they
//! are a no-op on native `cargo test`.

#![cfg(target_arch = "wasm32")]

use paged_canvas_wasm::CanvasWorker;
use std::io::Write;
use wasm_bindgen_test::*;

// Node-hosted: these tests use no DOM, so the default (non-browser)
// runner is enough. `scripts/test-wasm.sh` drives it headless.

/// One-page, one-story IDML built in-test (no corpus dependency), so the
/// mutate / timing case has real content. Mirrors the native fixture in
/// `tests/dispatch.rs`.
fn small_idml() -> Vec<u8> {
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
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Hello world</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

/// `CanvasWorker` is constructible across the wasm-bindgen boundary and
/// reports the protocol version constant. Pure wasm: the struct only
/// exists under `target_arch = "wasm32"`.
#[wasm_bindgen_test]
fn worker_constructs_and_reports_protocol() {
    let worker = CanvasWorker::new();
    // PROTOCOL_VERSION is 34 at the time of writing; assert it is at
    // least that (the constant only ever moves forward) so the test
    // doesn't churn on every protocol bump while still proving the
    // getter is wired through wasm-bindgen.
    assert!(worker.protocol_version() >= 34);
}

/// `handleMessage` round-trips a JSON envelope through the wasm VM: a
/// `hello` in, a `ready` out, with the protocol echoed. This exercises
/// the wasm-bindgen `&str -> String` marshalling the native dispatch
/// test can't reach.
#[wasm_bindgen_test]
fn handle_message_hello_round_trips_in_wasm() {
    let mut worker = CanvasWorker::new();
    let reply = worker.handle_message(r#"{"seq":0,"protocol":0,"kind":"hello"}"#);
    let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["kind"], "ready");
    assert!(v["protocol"].as_u64().unwrap() >= 34);
    assert_eq!(v["seq"].as_u64().unwrap(), 0);
}

/// The malformed-message seq-salvage path works inside the wasm VM too:
/// a structurally-bad message carrying a seq comes back as a seq-bearing
/// `mutationFailed` (so the JS client's pending promise resolves).
#[wasm_bindgen_test]
fn handle_message_malformed_salvages_seq_in_wasm() {
    let mut worker = CanvasWorker::new();
    let reply = worker.handle_message(r#"{"seq":7,"kind":"bogus"}"#);
    let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["seq"].as_u64().unwrap(), 7);
    assert_eq!(v["kind"], "mutationFailed");
}

/// The `js_sys::Date` timing path: loading + mutating a document inside
/// the wasm VM produces a `mutationApplied` reply whose `rebuildMs`
/// instrumentation field was filled from `Date::now()`. On native this
/// field is stubbed; only the wasm shell drives the real clock.
#[wasm_bindgen_test]
fn mutate_in_wasm_fills_rebuild_ms_from_js_date() {
    let mut worker = CanvasWorker::new();

    // Load the fixture via the JSON channel (bytes ride as a number[]).
    let bytes = small_idml();
    let load = serde_json::json!({
        "seq": 1,
        "protocol": worker.protocol_version(),
        "kind": "loadDocument",
        "payload": { "bytes": bytes }
    });
    let load_reply = worker.handle_message(&load.to_string());
    let lv: serde_json::Value = serde_json::from_str(&load_reply).unwrap();
    assert_eq!(lv["kind"], "documentLoaded", "fixture must load: {lv}");

    // Mutate → mutationApplied with a Date-derived rebuildMs field that
    // is present and finite (>= 0). We don't assert a specific value
    // (wall-clock), only that the JS-timed field is wired through.
    let mutate = serde_json::json!({
        "seq": 2,
        "protocol": worker.protocol_version(),
        "kind": "mutate",
        "payload": {
            "op": "insertText",
            "args": { "storyId": "story1", "offset": 5, "text": "," }
        }
    });
    let reply = worker.handle_message(&mutate.to_string());
    let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["kind"], "mutationApplied", "{v}");
    let rebuild_ms = v["payload"]["cacheStats"]["rebuildMs"].as_f64().unwrap();
    assert!(rebuild_ms >= 0.0 && rebuild_ms.is_finite(), "rebuildMs = {rebuild_ms}");
}
