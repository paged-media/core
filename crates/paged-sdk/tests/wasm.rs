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

//! `wasm_bindgen_test` lane for `ViewerSession` (Node-hosted, run by
//! `scripts/test-wasm.sh` with `--features gpu`). It covers what only the
//! exported JS class can show: the session stays callable while a method
//! that waits on the browser is pending. The borrow check lives in the
//! exported shims, so every call goes through `JsValue::from(session)`
//! rather than the Rust methods.

#![cfg(all(target_arch = "wasm32", feature = "gpu"))]

use paged_sdk::ViewerSession;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::*;

/// A fake WebGPU dedicated-worker scope for Node. wgpu finds
/// `navigator.gpu` through `globalThis.WorkerGlobalScope` and checks the
/// canvas context with `instanceof GPUCanvasContext`; the headless readback
/// path constructs an `OffscreenCanvas`. `requestAdapter` stays pending
/// until `release()`, so a test controls how long the session waits.
/// `restore()` puts the globals back.
const FAKE_WEBGPU_SCOPE: &str = r#"
    const g = globalThis;
    const had = {
        scope: "WorkerGlobalScope" in g,
        ctx: "GPUCanvasContext" in g,
        canvas: "OffscreenCanvas" in g,
    };
    const nav = g.navigator ?? (g.navigator = {});
    g.WorkerGlobalScope = class WorkerGlobalScope {};
    g.GPUCanvasContext = class GPUCanvasContext {};
    if (!had.canvas) {
        g.OffscreenCanvas = class OffscreenCanvas {
            constructor(width, height) { this.width = width; this.height = height; }
            getContext() { return new g.GPUCanvasContext(); }
        };
    }
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
        canvas: new g.OffscreenCanvas(64, 64),
        release: () => release(),
        restore: () => {
            delete nav.gpu;
            if (!had.scope) delete g.WorkerGlobalScope;
            if (!had.ctx) delete g.GPUCanvasContext;
            if (!had.canvas) delete g.OffscreenCanvas;
        },
    };
"#;

fn get(obj: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(obj, &key.into()).unwrap()
}

fn call(obj: &JsValue, name: &str, args: &[JsValue]) -> Result<JsValue, JsValue> {
    let args: js_sys::Array = args.iter().collect();
    get(obj, name)
        .unchecked_into::<js_sys::Function>()
        .apply(obj, &args)
}

/// Resolve after `ms` on the host's timer queue — the queue animation
/// frames and input handlers run from.
async fn sleep_ms(ms: i32) {
    let set_timeout: js_sys::Function = get(&js_sys::global(), "setTimeout").unchecked_into();
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        set_timeout
            .call2(&JsValue::NULL, &resolve, &ms.into())
            .unwrap();
    });
    wasm_bindgen_futures::JsFuture::from(promise).await.unwrap();
}

async fn session() -> JsValue {
    let session = ViewerSession::new()
        .await
        .map_err(|_| ())
        .expect("the fake scope exposes navigator.gpu");
    JsValue::from(session)
}

/// A session waiting on the browser must still answer the viewer's frame.
///
/// The viewer asks for page thumbnails (`render_page_to_bytes`) and binds
/// its canvas (`render_to_canvas*`) while its animation frames call
/// `present` and its input calls `set_page`. When those methods were
/// `async fn(&mut self)`, wasm-bindgen held the session borrowed for the
/// whole wait, and every such call threw "recursive use of an object
/// detected which would lead to unsafe aliasing in rust".
#[wasm_bindgen_test]
async fn present_answers_while_a_render_waits_on_the_gpu() {
    let env = js_sys::Function::new_no_args(FAKE_WEBGPU_SCOPE)
        .call0(&JsValue::NULL)
        .unwrap();

    let thumbnailing = session().await;
    let thumbnail = call(
        &thumbnailing,
        "render_page_to_bytes",
        &[0.into(), 16.into()],
    )
    .unwrap();
    let binding = session().await;
    let bound = call(&binding, "render_to_canvas", &[get(&env, "canvas")]).unwrap();
    // A macrotask later both futures have run and are parked on
    // `requestAdapter` — the window an animation frame falls into.
    sleep_ms(10).await;

    let mut refused = Vec::new();
    for (label, session) in [
        ("render_page_to_bytes", &thumbnailing),
        ("render_to_canvas", &binding),
    ] {
        for (name, args) in [
            (
                "present",
                vec![
                    1.0.into(),
                    0.0.into(),
                    0.0.into(),
                    1.0.into(),
                    JsValue::UNDEFINED,
                ],
            ),
            ("set_page", vec![0.into()]),
            ("page_count", vec![]),
            ("page_layout", vec![]),
            ("resize", vec![10.into(), 10.into(), 1.0.into()]),
        ] {
            if let Err(e) = call(session, name, &args) {
                let message = get(&e, "message")
                    .as_string()
                    .unwrap_or_else(|| format!("{e:?}"));
                refused.push(format!("{label} pending -> {name}: {message}"));
            }
        }
    }

    // Settle and clean up before asserting, so a failure leaves neither a
    // parked future nor the fake globals behind for the next test.
    call(&env, "release", &[]).unwrap();
    let thumbnail = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(thumbnail)).await;
    let bound = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(bound)).await;
    call(&env, "restore", &[]).unwrap();

    assert!(
        refused.is_empty(),
        "calls refused while a render waited on the GPU: {refused:?}"
    );
    // No adapter: the thumbnail rejects and the bind reports it.
    assert!(thumbnail.is_err(), "no adapter, no thumbnail");
    let bound = bound.expect("render_to_canvas reports, never rejects");
    assert_eq!(get(&bound, "ok"), JsValue::FALSE);
}
