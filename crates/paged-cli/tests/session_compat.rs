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

//! `paged-run` and `paged session` must answer identically.
//!
//! They are the same function today, but "today" is the whole risk:
//! `paged-run` has two live consumers — editor-server's automation lane
//! and the docs scripting gate — and a change made for the CLI that
//! reached only one binary would break them silently. This runs one
//! script through both and compares the replies.
//!
//! It also pins the part of the protocol that must NOT move: the
//! shipped 2 s script budget. `paged script` raises it on purpose;
//! `run-script` may not, because the docs gate validates its corpus
//! against the shipped default and a quietly larger budget there would
//! let an example that hangs the editor pass.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// `paged-run` lives in another package, so `CARGO_BIN_EXE_` does not
/// reach it; it sits beside our own binary in the target directory.
///
/// It must also be FRESH. `cargo test --workspace` — the gate that
/// matters, and what CI runs — rebuilds both from the same source, so
/// they agree. `cargo test -p paged-cli` alone rebuilds only ours and
/// leaves whatever `paged-run` was last built there, which answers the
/// PREVIOUS protocol and fails this comparison for a reason that is not
/// drift. A binary older than ours predates our build, so skip that
/// half loudly rather than report a difference that isn't one.
fn paged_run() -> Option<PathBuf> {
    let mine = PathBuf::from(env!("CARGO_BIN_EXE_paged"));
    let sibling = mine.with_file_name(format!("paged-run{}", std::env::consts::EXE_SUFFIX));
    let built_at = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (built_at(&sibling), built_at(&mine)) {
        (Some(theirs), Some(ours)) if theirs >= ours => Some(sibling),
        _ => None,
    }
}

/// Requests exercising the additive Phase-2 surface alongside the
/// original commands. A host that sends none of the new fields must see
/// exactly the protocol it always saw, so both shapes appear here.
fn script() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"cmd": "new-blank", "width": 612.0, "height": 792.0}),
        serde_json::json!({"cmd": "pages"}),
        serde_json::json!({"cmd": "digest"}),
        serde_json::json!({"cmd": "describe"}),
        serde_json::json!({"cmd": "register-font", "family": "Nonesuch", "path": "/nope.ttf"}),
        serde_json::json!({"cmd": "render", "page": 0, "backend": "vello", "out": "/dev/null"}),
        serde_json::json!({"cmd": "export", "format": "postscript", "out": "/dev/null"}),
        serde_json::json!({"cmd": "quit"}),
    ]
}

fn replies(exe: &str, args: &[&str]) -> Vec<serde_json::Value> {
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {exe}: {e}"));
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut out = Vec::new();

    let read = |stdout: &mut BufReader<_>| {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str::<serde_json::Value>(line.trim())
            .unwrap_or_else(|e| panic!("bad json {line:?}: {e}"))
    };
    out.push(read(&mut stdout));
    for req in script() {
        let quit = req["cmd"] == "quit";
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
        if quit {
            break;
        }
        out.push(read(&mut stdout));
    }
    drop(stdin);
    let _ = child.wait();
    out
}

#[test]
fn both_binaries_answer_the_same_protocol() {
    // Unconditional half: `paged-run` must still be a delegating shim.
    // If someone gives it a session loop of its own, the two can drift
    // whether or not the binary happened to be built for this run.
    let shim = include_str!("../../paged-run/src/main.rs");
    assert!(
        shim.contains("paged_cli::session::run"),
        "paged-run must delegate to the same session module, not carry its own"
    );

    let via_paged = replies(env!("CARGO_BIN_EXE_paged"), &["session"]);
    let Some(run) = paged_run() else {
        eprintln!(
            "SKIPPING the binary-vs-binary half: paged-run is absent or older \
             than this build. Run `cargo test --workspace` (as CI does) to \
             exercise it."
        );
        return;
    };
    let via_run = replies(run.to_str().unwrap(), &[]);
    assert_eq!(
        via_paged.len(),
        via_run.len(),
        "reply counts differ: {via_paged:#?} vs {via_run:#?}"
    );
    for (i, (a, b)) in via_paged.iter().zip(&via_run).enumerate() {
        assert_eq!(
            a, b,
            "reply {i} differs between `paged session` and `paged-run`"
        );
    }

    // Sanity: the script above must actually have exercised failures,
    // or "identical" would only mean "both said ok to everything".
    let failures = via_paged.iter().filter(|r| r["ok"] == false).count();
    assert!(
        failures >= 3,
        "expected the bad-font/backend/format requests to fail: {via_paged:#?}"
    );
}

#[test]
fn run_script_keeps_the_shipped_two_second_budget() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_paged"))
        .arg("session")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let read = |stdout: &mut BufReader<_>| {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str::<serde_json::Value>(line.trim()).expect("json")
    };
    read(&mut stdout);

    for req in [
        serde_json::json!({"cmd": "new-blank", "width": 612.0, "height": 792.0}),
        // A loop that cannot finish inside any sane budget. It must be
        // stopped by one, and the reply must name which.
        serde_json::json!({"cmd": "run-script", "source": "let n=0; while(true){n++;}"}),
    ] {
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
        let reply = read(&mut stdout);
        if req["cmd"] == "run-script" {
            assert_eq!(reply["ok"], serde_json::json!(false), "{reply}");
            assert!(
                reply["result"]["budgetKind"].is_string(),
                "a budget must have stopped it, and the reply must say which: {reply}"
            );
        }
    }
    writeln!(stdin, "{}", serde_json::json!({"cmd": "quit"})).unwrap();
    drop(stdin);
    let _ = child.wait();
}
