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
//! - Re-entrancy (with `--features gpu`, the shipped build): the worker
//!   stays callable while `initGpu` waits on the browser, which only the
//!   exported JS shims' borrow checks can show.
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
    assert!(
        rebuild_ms >= 0.0 && rebuild_ms.is_finite(),
        "rebuildMs = {rebuild_ms}"
    );
}

/// A fake WebGPU dedicated-worker scope for Node. wgpu finds
/// `navigator.gpu` through `globalThis.WorkerGlobalScope` and checks the
/// canvas context with `instanceof GPUCanvasContext`; `requestAdapter`
/// stays pending until `release()`, so a test controls exactly how long
/// `initGpu` is suspended. `restore()` puts the globals back.
#[cfg(feature = "gpu")]
const FAKE_WEBGPU_SCOPE: &str = r#"
    const g = globalThis;
    const had = { scope: "WorkerGlobalScope" in g, ctx: "GPUCanvasContext" in g };
    const nav = g.navigator ?? (g.navigator = {});
    g.WorkerGlobalScope = class WorkerGlobalScope {};
    g.GPUCanvasContext = class GPUCanvasContext {};
    let release;
    const gate = new Promise((resolve) => { release = resolve; });
    Object.defineProperty(nav, "gpu", {
        configurable: true,
        value: {
            requestAdapter: () => gate.then(() => null),
            getPreferredCanvasFormat: () => "bgra8unorm",
        },
    });
    return {
        canvas: { width: 1, height: 1, getContext: () => new g.GPUCanvasContext() },
        release: () => release(),
        restore: () => {
            delete nav.gpu;
            if (!had.scope) delete g.WorkerGlobalScope;
            if (!had.ctx) delete g.GPUCanvasContext;
        },
    };
"#;

/// Resolve after `ms` on the host's timer queue: a macrotask, the queue
/// the editor's worker fires its render loop and debounced timers from.
#[cfg(feature = "gpu")]
async fn sleep_ms(ms: i32) {
    use wasm_bindgen::JsCast;
    let set_timeout: js_sys::Function =
        js_sys::Reflect::get(&js_sys::global(), &"setTimeout".into())
            .unwrap()
            .unchecked_into();
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        set_timeout
            .call2(&wasm_bindgen::JsValue::NULL, &resolve, &ms.into())
            .unwrap();
    });
    wasm_bindgen_futures::JsFuture::from(promise).await.unwrap();
}

/// `initGpu` must not hold the worker while it waits for the browser.
///
/// The editor's worker serialises its messages, but its render loop and
/// its debounced resolver (`runResolveJson`) run from timers. When
/// `initGpu` was an `async fn(&mut self)`, wasm-bindgen took that borrow on
/// the future's first poll and kept it until the adapter and device
/// requests resolved; a timer landing in that window threw "recursive use
/// of an object detected which would lead to unsafe aliasing in rust" and
/// killed the worker. The window is as long as the browser takes to answer
/// `requestAdapter`, which on a loaded machine is hundreds of milliseconds.
///
/// The calls go through the JS class (`JsValue::from(worker)`) on purpose:
/// the borrow check lives in the exported shims, which a direct Rust
/// method call would bypass.
#[cfg(feature = "gpu")]
#[wasm_bindgen_test]
async fn init_gpu_leaves_the_worker_callable_while_the_adapter_is_pending() {
    use wasm_bindgen::{JsCast, JsValue};

    let get = |obj: &JsValue, key: &str| js_sys::Reflect::get(obj, &key.into()).unwrap();
    let env = js_sys::Function::new_no_args(FAKE_WEBGPU_SCOPE)
        .call0(&JsValue::NULL)
        .unwrap();
    let worker = JsValue::from(CanvasWorker::new());
    let call = |name: &str, args: &js_sys::Array| {
        get(&worker, name)
            .unchecked_into::<js_sys::Function>()
            .apply(&worker, args)
    };

    let pending: js_sys::Promise = call(
        "initGpu",
        &js_sys::Array::of3(&get(&env, "canvas"), &1.into(), &1.into()),
    )
    .unwrap()
    .unchecked_into();
    // A macrotask later the init future has run and is parked on
    // `requestAdapter` — the window a render tick or a timer falls into.
    sleep_ms(10).await;

    let mut refused = Vec::new();
    for name in ["runResolveJson", "pageCount", "gpuReady", "sceneCacheSize"] {
        if let Err(e) = call(name, &js_sys::Array::new()) {
            let message = js_sys::Reflect::get(&e, &"message".into())
                .ok()
                .and_then(|m| m.as_string())
                .unwrap_or_else(|| format!("{e:?}"));
            refused.push(format!("{name}: {message}"));
        }
    }

    // Settle and clean up before asserting, so a failure leaves neither a
    // parked future nor the fake globals behind for the next test.
    get(&env, "release")
        .unchecked_into::<js_sys::Function>()
        .call0(&JsValue::NULL)
        .unwrap();
    let settled = wasm_bindgen_futures::JsFuture::from(pending).await;
    get(&env, "restore")
        .unchecked_into::<js_sys::Function>()
        .call0(&JsValue::NULL)
        .unwrap();

    assert!(
        refused.is_empty(),
        "calls refused while initGpu was pending: {refused:?}"
    );
    // No adapter: the presenter is not installed and the worker stays on
    // the CPU path.
    assert_eq!(settled.unwrap(), JsValue::FALSE);
    assert_eq!(
        call("gpuReady", &js_sys::Array::new()).unwrap(),
        JsValue::FALSE
    );
}
