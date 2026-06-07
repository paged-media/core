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

//! B-09 — runtime-budget verification. A runaway script must come
//! back as a `ScriptResult` ERROR, never hang the (synchronous)
//! worker; ordinary loops far below the budget stay unaffected.

use std::path::PathBuf;

use paged_canvas::{CanvasModel, CanvasOptions};
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
    CanvasModel::load("doc-budget", &bytes, CanvasOptions::default()).expect("load + build")
}

#[test]
fn infinite_loop_terminates_with_an_error() {
    let mut model = load();
    let result = execute_script(&mut model, "while (true) {}");
    let err = result.error.expect("runaway loop must surface an error");
    assert!(
        err.to_lowercase().contains("loop"),
        "error should name the loop budget, got: {err}"
    );
}

#[test]
fn runaway_recursion_terminates_with_an_error() {
    let mut model = load();
    let result = execute_script(&mut model, "function f() { return f(); } f();");
    assert!(
        result.error.is_some(),
        "unbounded recursion must surface an error, not abort"
    );
}

#[test]
fn ordinary_loops_stay_unaffected() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        "let n = 0; for (let i = 0; i < 100000; i++) { n += i; } console.log(String(n));",
    );
    assert!(result.error.is_none(), "error: {:?}", result.error);
    assert_eq!(result.output, vec!["[log] 4999950000".to_string()]);
}
