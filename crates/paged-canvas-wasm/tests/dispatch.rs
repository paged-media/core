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

//! Native end-to-end coverage of the worker dispatch envelope (audit
//! B11). The wasm-bindgen surface in `paged-canvas-wasm` used to be the
//! ONLY home of the parse → dispatch → serialise message handling, and
//! it compiled solely under `target_arch = "wasm32"` — invisible to
//! `cargo test`. The dispatch is now extracted into
//! `paged_canvas_wasm::dispatch::WorkerCore`, which is target-agnostic,
//! so these tests drive the exact same envelope the browser worker does:
//! a JSON `MainToWorker` string in, a JSON `WorkerToMain` string out.
//!
//! Every case below asserts on the *wire* (the serialised reply) rather
//! than internal state, because the wire is the contract the editor
//! client correlates against. The `rebuild_ms` clock is stubbed so the
//! tests stay deterministic.

use std::io::Write;

use paged_canvas_wasm::dispatch::{CacheEffect, WorkerCore};

/// Deterministic clock: every call returns the same instant, so
/// `rebuild_ms` is always 0.0 and replies are byte-stable.
fn frozen_clock() -> f64 {
    0.0
}

/// Run one message through the full envelope and parse the reply back
/// into a `serde_json::Value` so cases can assert on wire fields.
fn roundtrip(core: &mut WorkerCore, msg: &serde_json::Value) -> serde_json::Value {
    let input = serde_json::to_string(msg).unwrap();
    let (reply, _effect) = core.handle_message(&input, &frozen_clock);
    serde_json::from_str(&reply).expect("reply must be valid JSON")
}

/// Same as `roundtrip` but also returns the GPU cache effect so the
/// scene-cache-scoping cases can assert on it.
fn roundtrip_with_effect(
    core: &mut WorkerCore,
    msg: &serde_json::Value,
) -> (serde_json::Value, CacheEffect) {
    let input = serde_json::to_string(msg).unwrap();
    let (reply, effect) = core.handle_message(&input, &frozen_clock);
    (serde_json::from_str(&reply).unwrap(), effect)
}

/// A one-page, one-story IDML so mutate / caret / word-bounds /
/// export-idml have real content to operate on. Story `story1` carries
/// "Hello world" in frame `tf1` on page `p1`.
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

/// Build a `loadDocument` message carrying `small_idml()`.
fn load_msg(seq: u64) -> serde_json::Value {
    let bytes: Vec<u8> = small_idml();
    serde_json::json!({
        "seq": seq,
        "protocol": protocol(),
        "kind": "loadDocument",
        "payload": { "bytes": bytes }
    })
}

/// The protocol version the dispatch stamps on every reply. Read it off
/// a `Hello`/`Ready` round-trip so the test never hardcodes a stale
/// number (it rides whatever `PROTOCOL_VERSION` the build carries).
fn protocol() -> u64 {
    let mut core = WorkerCore::new();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({ "seq": 0, "protocol": 0, "kind": "hello" }),
    );
    reply["protocol"].as_u64().unwrap()
}

/// Load `small_idml()` into a fresh core and return it ready for the
/// content-bearing cases. Asserts the load succeeded.
fn loaded_core() -> WorkerCore {
    let mut core = WorkerCore::new();
    let reply = roundtrip(&mut core, &load_msg(1));
    assert_eq!(
        reply["kind"], "documentLoaded",
        "fixture must load: {reply}"
    );
    core
}

// ---------------------------------------------------------------------
// 1. Handshake
// ---------------------------------------------------------------------

#[test]
fn hello_replies_ready_with_protocol_echo() {
    let mut core = WorkerCore::new();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({ "seq": 0, "protocol": 0, "kind": "hello" }),
    );
    assert_eq!(reply["kind"], "ready");
    // Ready carries the protocol both in the envelope and the payload.
    let p = protocol();
    assert_eq!(reply["protocol"].as_u64().unwrap(), p);
    assert_eq!(reply["payload"]["protocol"].as_u64().unwrap(), p);
    // The seq is echoed back so the client can correlate.
    assert_eq!(reply["seq"].as_u64().unwrap(), 0);
}

// ---------------------------------------------------------------------
// 2. Load
// ---------------------------------------------------------------------

#[test]
fn load_document_replies_document_loaded_with_handle() {
    let mut core = WorkerCore::new();
    let reply = roundtrip(&mut core, &load_msg(7));
    assert_eq!(reply["kind"], "documentLoaded");
    assert_eq!(reply["seq"].as_u64().unwrap(), 7);
    // Handle carries the one page + its Letter dimensions.
    let handle = &reply["payload"];
    assert_eq!(handle["pageCount"].as_u64().unwrap(), 1);
    assert_eq!(handle["pageIds"][0].as_str().unwrap(), "p1");
    let (w, h) = (
        handle["pageSizesPt"][0][0].as_f64().unwrap(),
        handle["pageSizesPt"][0][1].as_f64().unwrap(),
    );
    assert!((w - 612.0).abs() < 0.1, "width {w}");
    assert!((h - 792.0).abs() < 0.1, "height {h}");
}

#[test]
fn load_document_clears_the_gpu_scene_cache() {
    let mut core = WorkerCore::new();
    let (_reply, effect) = roundtrip_with_effect(&mut core, &load_msg(1));
    assert_eq!(effect, CacheEffect::ClearAll);
}

#[test]
fn load_document_with_garbage_bytes_replies_load_failed() {
    let mut core = WorkerCore::new();
    let msg = serde_json::json!({
        "seq": 2,
        "protocol": protocol(),
        "kind": "loadDocument",
        "payload": { "bytes": [0u8, 1, 2, 3, 4] }
    });
    let reply = roundtrip(&mut core, &msg);
    assert_eq!(reply["kind"], "loadFailed");
    assert!(reply["payload"]["error"].is_object());
}

#[test]
fn load_stats_report_one_page_one_story() {
    // `setSelection` with no document errors; after a load it returns
    // a Stats reply we can probe for the structural counts.
    let mut core = loaded_core();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 3,
            "protocol": protocol(),
            "kind": "setSelection",
            "payload": { "selection": null }
        }),
    );
    assert_eq!(reply["kind"], "stats");
    assert_eq!(reply["payload"]["pages"].as_u64().unwrap(), 1);
    assert_eq!(reply["payload"]["stories"].as_u64().unwrap(), 1);
}

// ---------------------------------------------------------------------
// 3. Mutate (happy + malformed)
// ---------------------------------------------------------------------

#[test]
fn mutate_insert_text_replies_mutation_applied() {
    let mut core = loaded_core();
    let msg = serde_json::json!({
        "seq": 10,
        "protocol": protocol(),
        "kind": "mutate",
        "payload": {
            "op": "insertText",
            "args": { "storyId": "story1", "offset": 5, "text": "," }
        }
    });
    let (reply, effect) = roundtrip_with_effect(&mut core, &msg);
    assert_eq!(reply["kind"], "mutationApplied", "{reply}");
    assert_eq!(reply["payload"]["clientSeq"].as_u64().unwrap(), 10);
    // rebuild_ms is the frozen-clock delta: exactly 0.
    assert_eq!(
        reply["payload"]["cacheStats"]["rebuildMs"]
            .as_f64()
            .unwrap(),
        0.0
    );
    // A content edit to story1 invalidates the GPU scene cache. The
    // dispatch scopes it to the story's pages when the post-rebuild
    // story→pages map resolves them, and falls back to a full clear
    // when the story has no on-page frames (the documented behaviour).
    // Either way it MUST NOT be a no-op — the edited page can't keep a
    // stale cached scene.
    assert_ne!(
        effect,
        CacheEffect::None,
        "an applied edit must dirty the cache"
    );
}

#[test]
fn mutate_with_no_document_replies_mutation_failed_no_document() {
    let mut core = WorkerCore::new();
    let msg = serde_json::json!({
        "seq": 11,
        "protocol": protocol(),
        "kind": "mutate",
        "payload": {
            "op": "insertText",
            "args": { "storyId": "story1", "offset": 0, "text": "x" }
        }
    });
    let reply = roundtrip(&mut core, &msg);
    assert_eq!(reply["kind"], "mutationFailed");
    assert_eq!(reply["payload"]["error"]["kind"], "noDocument");
}

#[test]
fn malformed_message_with_seq_salvages_as_mutation_failed() {
    // The seq-salvage wire-robustness behaviour: a message the dispatch
    // can't parse but that carries a `seq` must still produce a
    // seq-bearing MutationFailed so the client's pending promise
    // RESOLVES instead of hanging forever.
    let mut core = WorkerCore::new();
    let bad = r#"{ "seq": 42, "kind": "totallyBogusKind" }"#;
    let (reply, effect) = core.handle_message(bad, &frozen_clock);
    let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["seq"].as_u64().unwrap(), 42);
    assert_eq!(v["kind"], "mutationFailed");
    assert_eq!(v["payload"]["error"]["kind"], "notImplemented");
    assert!(v["payload"]["error"]["details"]["what"]
        .as_str()
        .unwrap()
        .contains("malformed message"));
    assert_eq!(effect, CacheEffect::None);
}

#[test]
fn malformed_message_without_seq_salvages_as_warning() {
    // No seq to salvage → there is no pending promise to resolve, so
    // the worker emits an unsolicited protocol Warning instead.
    let mut core = WorkerCore::new();
    let bad = "this is not json at all";
    let (reply, _effect) = core.handle_message(bad, &frozen_clock);
    let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert!(v["seq"].is_null());
    assert_eq!(v["kind"], "warning");
    assert_eq!(v["payload"]["kind"], "protocol");
}

// ---------------------------------------------------------------------
// 4. Read queries (word bounds, caret nav)
// ---------------------------------------------------------------------

#[test]
fn request_word_bounds_returns_span_for_loaded_story() {
    let mut core = loaded_core();
    // Offset 0 falls inside "Hello" → the word span starts at 0.
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 20,
            "protocol": protocol(),
            "kind": "requestWordBounds",
            "payload": { "storyId": "story1", "offset": 0 }
        }),
    );
    assert_eq!(reply["kind"], "wordBoundsResult");
    let bounds = &reply["payload"]["bounds"];
    assert!(bounds.is_object(), "expected a word span, got {bounds}");
    assert_eq!(bounds["start"].as_u64().unwrap(), 0);
    assert!(bounds["end"].as_u64().unwrap() >= 5, "Hello is 5 bytes");
}

#[test]
fn request_word_bounds_with_no_document_fails_cleanly() {
    let mut core = WorkerCore::new();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 21,
            "protocol": protocol(),
            "kind": "requestWordBounds",
            "payload": { "storyId": "story1", "offset": 0 }
        }),
    );
    assert_eq!(reply["kind"], "mutationFailed");
    assert_eq!(reply["payload"]["error"]["kind"], "noDocument");
}

#[test]
fn request_caret_nav_replies_caret_nav_result() {
    let mut core = loaded_core();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 22,
            "protocol": protocol(),
            "kind": "requestCaretNav",
            "payload": { "storyId": "story1", "offset": 0, "direction": "down" }
        }),
    );
    assert_eq!(reply["kind"], "caretNavResult");
    // Single-line story → no line below; offset comes back null. The
    // contract is that the reply SHAPE is well-formed, which is what we
    // assert (the field exists and is null, not missing).
    assert!(reply["payload"].get("offset").is_some());
}

// ---------------------------------------------------------------------
// 5. Page request + unknown page
// ---------------------------------------------------------------------

#[test]
fn request_page_known_replies_display_list_ready() {
    let mut core = loaded_core();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 30,
            "protocol": protocol(),
            "kind": "requestPage",
            "payload": { "pageId": "p1", "lod": "live" }
        }),
    );
    assert_eq!(reply["kind"], "displayListReady");
    assert_eq!(reply["payload"]["pageId"].as_str().unwrap(), "p1");
}

#[test]
fn request_page_unknown_replies_unknown_page() {
    let mut core = loaded_core();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 31,
            "protocol": protocol(),
            "kind": "requestPage",
            "payload": { "pageId": "does-not-exist", "lod": "live" }
        }),
    );
    assert_eq!(reply["kind"], "mutationFailed");
    assert_eq!(reply["payload"]["error"]["kind"], "unknownPage");
    assert_eq!(
        reply["payload"]["error"]["details"]["pageId"]
            .as_str()
            .unwrap(),
        "does-not-exist"
    );
}

// ---------------------------------------------------------------------
// 6. Export IDML
// ---------------------------------------------------------------------

#[test]
fn export_idml_with_document_replies_idml_exported() {
    let mut core = loaded_core();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 40,
            "protocol": protocol(),
            "kind": "exportIdml",
            "payload": {}
        }),
    );
    assert_eq!(reply["kind"], "idmlExported", "{}", reply["kind"]);
    let bytes = reply["payload"]["idmlBytes"].as_array().unwrap();
    assert!(!bytes.is_empty(), "exported IDML must carry bytes");
    // The re-serialised package is a ZIP: first two bytes are 'P','K'.
    assert_eq!(bytes[0].as_u64().unwrap(), b'P' as u64);
    assert_eq!(bytes[1].as_u64().unwrap(), b'K' as u64);
}

#[test]
fn export_idml_without_document_replies_failed() {
    let mut core = WorkerCore::new();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 41,
            "protocol": protocol(),
            "kind": "exportIdml",
            "payload": {}
        }),
    );
    assert_eq!(reply["kind"], "exportIdmlFailed");
    assert!(reply["payload"]["error"]
        .as_str()
        .unwrap()
        .contains("no document"));
}

// ---------------------------------------------------------------------
// 7. Protocol-version sentinel (v34) + unknown kind
// ---------------------------------------------------------------------

#[test]
fn every_reply_stamps_the_current_protocol_version() {
    // v34 sentinel — every reply must carry the build's PROTOCOL_VERSION
    // so the client can detect a stale worker. We assert the constant is
    // 34 (the documented current version) AND that an arbitrary reply
    // stamps it.
    assert_eq!(
        protocol(),
        34,
        "PROTOCOL_VERSION drifted from documented v34"
    );
    let mut core = loaded_core();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 50,
            "protocol": protocol(),
            "kind": "requestDocumentMeta"
        }),
    );
    assert_eq!(reply["protocol"].as_u64().unwrap(), 34);
    assert_eq!(reply["kind"], "documentMetaReply");
}

#[test]
fn unknown_kind_is_handled_as_malformed_not_panic() {
    // An unrecognised `kind` doesn't match any `MainToWorkerKind`
    // variant, so serde rejects it — the dispatch must salvage it as a
    // failure reply rather than panic or hang. With a seq present that
    // is a seq-bearing MutationFailed (covered above); here we confirm
    // the dispatch never panics on a structurally-valid-but-unknown kind.
    let mut core = WorkerCore::new();
    let bad = r#"{ "seq": 99, "protocol": 34, "kind": "noSuchKind", "payload": {} }"#;
    let (reply, effect) = core.handle_message(bad, &frozen_clock);
    let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["seq"].as_u64().unwrap(), 99);
    assert_eq!(v["kind"], "mutationFailed");
    assert_eq!(effect, CacheEffect::None);
}

// ---------------------------------------------------------------------
// 8. Undo / redo round-trip (cache-scoping + page-structure flags)
// ---------------------------------------------------------------------

#[test]
fn undo_after_insert_replies_undo_applied() {
    let mut core = loaded_core();
    // Apply an edit so there is something to undo.
    let _ = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 60,
            "protocol": protocol(),
            "kind": "mutate",
            "payload": {
                "op": "insertText",
                "args": { "storyId": "story1", "offset": 5, "text": "," }
            }
        }),
    );
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 61,
            "protocol": protocol(),
            "kind": "undo"
        }),
    );
    assert_eq!(reply["kind"], "undoApplied", "{reply}");
    // A content-only undo doesn't change the page list.
    assert!(!reply["payload"]["pageStructureChanged"].as_bool().unwrap());
}

#[test]
fn undo_with_empty_log_replies_mutation_failed() {
    let mut core = loaded_core();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 62,
            "protocol": protocol(),
            "kind": "undo"
        }),
    );
    assert_eq!(reply["kind"], "mutationFailed");
    assert_eq!(reply["payload"]["error"]["kind"], "notImplemented");
}

#[test]
fn redo_with_empty_log_replies_mutation_failed() {
    let mut core = loaded_core();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 63,
            "protocol": protocol(),
            "kind": "redo"
        }),
    );
    assert_eq!(reply["kind"], "mutationFailed");
}

// ---------------------------------------------------------------------
// 9. Font registry (state that survives across loads)
// ---------------------------------------------------------------------

#[test]
fn register_then_clear_font_registry_round_trips() {
    let mut core = WorkerCore::new();
    let reg = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 70,
            "protocol": protocol(),
            "kind": "registerFont",
            "payload": { "family": "Inter", "bytes": [1u8, 2, 3] }
        }),
    );
    assert_eq!(reg["kind"], "fontRegistered");
    assert_eq!(reg["payload"]["family"].as_str().unwrap(), "Inter");
    assert_eq!(core.font_registry.len(), 1);

    let cleared = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 71,
            "protocol": protocol(),
            "kind": "clearFontRegistry"
        }),
    );
    assert_eq!(cleared["kind"], "fontRegistryCleared");
    assert!(core.font_registry.is_empty());
}

// ---------------------------------------------------------------------
// 10. Stateless query with no document degrades gracefully
// ---------------------------------------------------------------------

#[test]
fn request_layers_with_no_document_returns_empty() {
    let mut core = WorkerCore::new();
    let reply = roundtrip(
        &mut core,
        &serde_json::json!({
            "seq": 80,
            "protocol": protocol(),
            "kind": "requestLayers"
        }),
    );
    assert_eq!(reply["kind"], "layers");
    assert!(reply["payload"]["items"].as_array().unwrap().is_empty());
}

#[test]
fn read_only_query_produces_no_cache_effect() {
    let mut core = loaded_core();
    let (_reply, effect) = roundtrip_with_effect(
        &mut core,
        &serde_json::json!({
            "seq": 81,
            "protocol": protocol(),
            "kind": "requestPage",
            "payload": { "pageId": "p1", "lod": "live" }
        }),
    );
    assert_eq!(effect, CacheEffect::None);
}
