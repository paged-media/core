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

//! No method `ViewerSession` exports to JS may be an `async fn` taking `self`.
//!
//! wasm-bindgen takes an async method's `&self` / `&mut self` borrow inside
//! the future it hands to `future_to_promise`: on the first poll, held until
//! the future settles. Every other call on the session in that window — a
//! `present` from the next animation frame, a `set_page` from input — throws
//! "recursive use of an object detected which would lead to unsafe aliasing
//! in rust". `render_to_canvas`, `render_to_canvas_main`, `render_to_bytes`
//! and `render_page_to_bytes` were such methods (a thumbnail waiting on the
//! GPU while the viewer presents a frame). A method that has to wait on the
//! browser instead does its work on the session synchronously and returns a
//! `js_sys::Promise` whose future owns no borrow of it; `future_to_promise`
//! requires a `'static` future, so the compiler then rules the borrow out.
//! (`ViewerSession.new()` takes no receiver and borrows nothing.)
//!
//! The behavioural half is `tests/wasm.rs`; this guard is native, so it runs
//! in every `cargo test` and covers what a Node host cannot drive (a
//! readback needs a real GPU device). paged-canvas-wasm's `CanvasWorker`
//! has the same guard.

#[test]
fn no_exported_session_method_is_async() {
    let offenders = async_fns_taking_self(include_str!("../src/lib.rs"));
    assert!(
        offenders.is_empty(),
        "an async method on ViewerSession holds the session's borrow across \
         its awaits; return a js_sys::Promise whose future owns no borrow \
         instead:\n{}",
        offenders.join("\n")
    );
}

/// Every `async fn` in `src` whose signature takes `self`, as
/// `src/lib.rs:<line>: <first line>`. Comment lines are skipped; a signature
/// runs from `async fn` to the `{` (or `;`) that ends it.
fn async_fns_taking_self(src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let mut offenders = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") || !code.contains("async fn") {
            continue;
        }
        let mut signature = String::new();
        for next in &lines[i..] {
            signature.push_str(next);
            if next.contains('{') || next.contains(';') {
                break;
            }
        }
        if signature.contains("self") {
            offenders.push(format!("src/lib.rs:{}: {}", i + 1, code));
        }
    }
    offenders
}
