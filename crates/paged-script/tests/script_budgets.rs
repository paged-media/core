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

//! B-09 / W-08 — runtime-budget verification. A runaway script must
//! come back as a `ScriptResult` ERROR carrying a TYPED
//! `ScriptBudgetKind`, never hang the (synchronous) worker; ordinary
//! loops far below the budget stay unaffected; the engine is reusable
//! after an abort (the worker-survives contract); and the wall-clock
//! deadline terminates a script blocked in a slow host call.
//!
//! These tests build a minimal in-memory IDML package so they do NOT
//! depend on the gitignored fidelity corpus (which is absent in a fresh
//! worktree).

use std::cell::Cell;
use std::io::Write;

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_script::{execute_script, execute_script_with, ScriptBudget, ScriptBudgetKind};

/// A minimal one-page, one-frame IDML package — enough for
/// `CanvasModel::load` to build a document the script bridge can read
/// and mutate. Mirrors the corpus-free helper in
/// `paged-canvas/tests/page_ops.rs`.
fn minimal_idml() -> Vec<u8> {
    let spread = r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#;

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
        zip.write_all(spread.as_bytes()).unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Hello</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn load() -> CanvasModel {
    CanvasModel::load("doc-budget", &minimal_idml(), CanvasOptions::default())
        .expect("load + build")
}

/// A deterministic, advance-on-read clock (ms). Each call to the
/// returned closure advances the virtual time by `step_ms`, so a script
/// that calls host functions in a loop crosses any deadline after a
/// bounded number of host calls — no real sleeping, fully reproducible.
fn advancing_clock(start_ms: f64, step_ms: f64) -> impl Fn() -> f64 {
    let now = Cell::new(start_ms);
    move || {
        let cur = now.get();
        now.set(cur + step_ms);
        cur
    }
}

// -------------------------------------------------------------- loop

#[test]
fn infinite_loop_terminates_with_an_iterations_budget_error() {
    let mut model = load();
    let result = execute_script(&mut model, "while (true) {}");
    let err = result.error.expect("runaway loop must surface an error");
    assert!(
        err.to_lowercase().contains("loop"),
        "error should name the loop budget, got: {err}"
    );
    assert_eq!(
        result.budget_kind,
        Some(ScriptBudgetKind::Iterations),
        "loop runaway must be typed as Iterations; got {:?} / {err}",
        result.budget_kind
    );
}

#[test]
fn loop_iteration_budget_is_configurable() {
    let mut model = load();
    // A tiny loop budget makes even a modest finite loop trip.
    let budget = ScriptBudget {
        loop_iterations: 100,
        ..ScriptBudget::default()
    };
    let clock = advancing_clock(0.0, 0.0);
    let result = execute_script_with(
        &mut model,
        "let n = 0; for (let i = 0; i < 1000000; i++) { n += i; }",
        budget,
        &clock,
    );
    assert_eq!(
        result.budget_kind,
        Some(ScriptBudgetKind::Iterations),
        "a 100-iteration budget must trip on a million-iteration loop: {:?}",
        result.error
    );
}

// ---------------------------------------------------------- recursion

#[test]
fn runaway_recursion_terminates_with_a_recursion_budget_error() {
    let mut model = load();
    let result = execute_script(&mut model, "function f() { return f(); } f();");
    assert!(
        result.error.is_some(),
        "unbounded recursion must surface an error, not abort"
    );
    assert_eq!(
        result.budget_kind,
        Some(ScriptBudgetKind::Recursion),
        "deep recursion must be typed as Recursion; got {:?} / {:?}",
        result.budget_kind,
        result.error
    );
}

#[test]
fn recursion_budget_is_configurable() {
    let mut model = load();
    let budget = ScriptBudget {
        recursion_depth: 8,
        ..ScriptBudget::default()
    };
    let clock = advancing_clock(0.0, 0.0);
    // Recurse deeper than the tightened limit but far less than the
    // default 512 — proves the config (not the default) is in force.
    let result = execute_script_with(
        &mut model,
        "function f(n){ if (n<=0) return 0; return 1 + f(n-1); } f(64);",
        budget,
        &clock,
    );
    assert_eq!(
        result.budget_kind,
        Some(ScriptBudgetKind::Recursion),
        "an 8-deep recursion budget must trip at depth 64: {:?}",
        result.error
    );
}

// -------------------------------------------------------- wall-clock

#[test]
fn wall_clock_deadline_terminates_a_loop_with_host_calls() {
    let mut model = load();
    // 50 ms budget; the clock advances 10 ms per read. The loop calls a
    // host fn each iteration, so the deadline check fires at a host-call
    // boundary and aborts within a few iterations — well before the
    // 10M loop-iteration limit.
    let budget = ScriptBudget {
        wall_clock_ms: Some(50),
        ..ScriptBudget::default()
    };
    let clock = advancing_clock(0.0, 10.0);
    let result = execute_script_with(
        &mut model,
        "while (true) { paged.layers(); }",
        budget,
        &clock,
    );
    assert_eq!(
        result.budget_kind,
        Some(ScriptBudgetKind::WallClock),
        "a loop doing host calls must hit the wall-clock budget, not the loop budget; got {:?} / {:?}",
        result.budget_kind,
        result.error
    );
    let err = result.error.expect("wall-clock abort must carry an error");
    assert!(
        err.to_lowercase().contains("time") || err.contains("wall-clock"),
        "wall-clock error should name the time budget, got: {err}"
    );
}

#[test]
fn wall_clock_deadline_terminates_a_slow_native_call_chain() {
    let mut model = load();
    // Simulates a single deliberately slow host call: the FIRST clock
    // read (deadline seed) is at t=0, then the next read (the host-fn
    // entry guard) jumps past the 100 ms budget. So even one host call
    // after a long native pause aborts at the next boundary.
    let budget = ScriptBudget {
        wall_clock_ms: Some(100),
        ..ScriptBudget::default()
    };
    // start at 0 (seeds deadline=100), then +1000 ms per read.
    let clock = advancing_clock(0.0, 1000.0);
    let result = execute_script_with(
        &mut model,
        // Two host calls: the first one's entry guard already sees the
        // clock past the deadline and aborts.
        "paged.layers(); paged.swatches();",
        budget,
        &clock,
    );
    assert_eq!(
        result.budget_kind,
        Some(ScriptBudgetKind::WallClock),
        "a slow native chain must abort on the wall clock; got {:?} / {:?}",
        result.budget_kind,
        result.error
    );
}

#[test]
fn wall_clock_breach_is_not_catchable_by_user_script() {
    let mut model = load();
    // The wall-clock abort raises a NON-catchable RuntimeLimit, so a
    // user try/catch cannot swallow it: the result must still be a
    // typed WallClock error, and the "survived" marker must NOT print.
    let budget = ScriptBudget {
        wall_clock_ms: Some(10),
        ..ScriptBudget::default()
    };
    let clock = advancing_clock(0.0, 1000.0);
    let result = execute_script_with(
        &mut model,
        r#"
            try {
                paged.layers();
                console.log("survived");
            } catch (e) {
                console.log("caught", String(e));
            }
        "#,
        budget,
        &clock,
    );
    assert_eq!(
        result.budget_kind,
        Some(ScriptBudgetKind::WallClock),
        "try/catch must not swallow the wall-clock budget abort: {:?}",
        result.error
    );
    assert!(
        !result.output.iter().any(|l| l.contains("survived")),
        "script kept running past the deadline: {:?}",
        result.output
    );
    assert!(
        !result.output.iter().any(|l| l.contains("caught")),
        "non-catchable budget error was caught by user script: {:?}",
        result.output
    );
}

#[test]
fn no_wall_clock_budget_means_no_deadline() {
    let mut model = load();
    // With wall_clock_ms = None the deadline is disabled even if the
    // clock races ahead; only loop/recursion/stack guards apply. A
    // small finite loop with host calls must complete cleanly.
    let budget = ScriptBudget {
        wall_clock_ms: None,
        ..ScriptBudget::default()
    };
    let clock = advancing_clock(0.0, 1_000_000.0);
    let result = execute_script_with(
        &mut model,
        "for (let i = 0; i < 5; i++) { paged.layers(); } console.log('done');",
        budget,
        &clock,
    );
    assert!(result.budget_kind.is_none(), "{:?}", result.error);
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(result.output.iter().any(|l| l.contains("done")));
}

// ------------------------------------------------- ordinary scripts

#[test]
fn ordinary_loops_stay_unaffected() {
    let mut model = load();
    let result = execute_script(
        &mut model,
        "let n = 0; for (let i = 0; i < 100000; i++) { n += i; } console.log(String(n));",
    );
    assert!(result.error.is_none(), "error: {:?}", result.error);
    assert!(result.budget_kind.is_none(), "{:?}", result.budget_kind);
    assert_eq!(result.output, vec!["[log] 4999950000".to_string()]);
}

#[test]
fn ordinary_script_error_is_not_a_budget_kind() {
    let mut model = load();
    // A plain throw is an error but NOT a budget exhaustion.
    let result = execute_script(&mut model, "throw new Error('boom');");
    assert!(result.error.is_some());
    assert!(
        result.budget_kind.is_none(),
        "an ordinary throw must not be classified as a budget abort: {:?}",
        result.budget_kind
    );
}

// ------------------------------------- post-exhaustion reusability

#[test]
fn engine_is_reusable_after_a_loop_budget_abort() {
    // The worker-survives contract: a fresh `execute_script` after an
    // abort works normally. `execute_script` builds a fresh Context per
    // call, but the thread-local deadline / model-pointer state must be
    // cleared cleanly so the next call is unaffected.
    let mut model = load();
    let runaway = execute_script(&mut model, "while (true) {}");
    assert_eq!(runaway.budget_kind, Some(ScriptBudgetKind::Iterations));

    // Next script runs cleanly and can still mutate the document.
    let ok = execute_script(
        &mut model,
        r#"paged.set("textFrame:tf1", "frameOpacity", 42); console.log("ok");"#,
    );
    assert!(ok.error.is_none(), "engine not reusable: {:?}", ok.error);
    assert!(ok.budget_kind.is_none());
    assert!(ok.output.iter().any(|l| l.contains("ok")));
}

#[test]
fn engine_is_reusable_after_a_wall_clock_abort() {
    let mut model = load();
    let budget = ScriptBudget {
        wall_clock_ms: Some(5),
        ..ScriptBudget::default()
    };
    let fast = advancing_clock(0.0, 1000.0);
    let aborted = execute_script_with(&mut model, "paged.layers();", budget, &fast);
    assert_eq!(aborted.budget_kind, Some(ScriptBudgetKind::WallClock));

    // A subsequent default-budget call (no fake racing clock) runs to
    // completion — the deadline state from the prior call did not leak.
    let ok = execute_script(&mut model, "paged.layers(); console.log('reused');");
    assert!(
        ok.error.is_none(),
        "deadline leaked across calls: {:?}",
        ok.error
    );
    assert!(ok.budget_kind.is_none());
    assert!(ok.output.iter().any(|l| l.contains("reused")));
}

// ------------------------------------------------- wire round-trip

#[test]
fn budget_kind_serializes_with_camel_case_tag() {
    // The wire contract: the typed kind serializes to a stable camelCase
    // string the editor/headless host matches on.
    let json = serde_json::to_string(&ScriptBudgetKind::WallClock).unwrap();
    assert_eq!(json, "\"wallClock\"");
    let json = serde_json::to_string(&ScriptBudgetKind::Iterations).unwrap();
    assert_eq!(json, "\"iterations\"");
    let json = serde_json::to_string(&ScriptBudgetKind::StackSize).unwrap();
    assert_eq!(json, "\"stackSize\"");
    let json = serde_json::to_string(&ScriptBudgetKind::Recursion).unwrap();
    assert_eq!(json, "\"recursion\"");
}

#[test]
fn ordinary_result_omits_budget_kind_in_json() {
    // Additive-on-the-wire proof: an ordinary result serializes without
    // a `budgetKind` key, so pre-existing consumers are unaffected.
    let mut model = load();
    let result = execute_script(&mut model, "console.log('hi');");
    let json = serde_json::to_string(&result).unwrap();
    assert!(
        !json.contains("budgetKind"),
        "ordinary result must omit budgetKind: {json}"
    );
}
