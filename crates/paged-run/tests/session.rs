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

//! End-to-end test of the `paged-run` NDJSON stdio session: spawn the
//! binary, drive it command-by-command, and assert the protocol +
//! round-trips. Uses `new-blank` so the test needs no corpus fixture
//! (the worktree has none).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

/// Write one request line; the session reads one JSON object per line.
fn send(stdin: &mut ChildStdin, req: Value) {
    let line = serde_json::to_string(&req).unwrap();
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
}

/// Read one response line and parse it as JSON.
fn recv(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    let n = stdout.read_line(&mut line).unwrap();
    assert!(n > 0, "session closed unexpectedly (no response line)");
    serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("bad json {line:?}: {e}"))
}

fn spawn() -> Child {
    Command::new(env!("CARGO_BIN_EXE_paged-run"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn paged-run")
}

#[test]
fn ndjson_session_roundtrip() {
    let tmp = std::env::temp_dir();
    let idml_out = tmp.join(format!("paged-run-test-{}.idml", std::process::id()));
    let png_out = tmp.join(format!("paged-run-test-{}.png", std::process::id()));

    let mut child = spawn();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    // 1. Handshake.
    let hello = recv(&mut stdout);
    assert_eq!(hello["ready"], json!(true), "greeting: {hello}");
    assert!(hello["protocol"].is_number(), "greeting carries protocol: {hello}");

    // 2. new-blank → a US-Letter page.
    send(&mut stdin, json!({"cmd": "new-blank", "width": 612.0, "height": 792.0}));
    let blank = recv(&mut stdout);
    assert_eq!(blank["ok"], json!(true), "new-blank: {blank}");
    assert!(blank["pageCount"].as_u64().unwrap() >= 1, "has a page: {blank}");

    // 3. digest is deterministic across repeated calls.
    send(&mut stdin, json!({"cmd": "digest"}));
    let d1 = recv(&mut stdout);
    assert_eq!(d1["ok"], json!(true), "digest: {d1}");
    send(&mut stdin, json!({"cmd": "digest"}));
    let d2 = recv(&mut stdout);
    assert_eq!(d1["combined"], d2["combined"], "digest is deterministic");
    assert_eq!(d1["stateHash"], d2["stateHash"], "state hash is deterministic");

    // 4. run-script: console output is captured; no error.
    send(
        &mut stdin,
        json!({"cmd": "run-script", "source": "console.log('hello-boa'); console.log(paged.tree().length)"}),
    );
    let run = recv(&mut stdout);
    assert_eq!(run["ok"], json!(true), "run-script ok: {run}");
    assert_eq!(run["result"]["error"], Value::Null, "no script error: {run}");
    // console.log lines are captured with a `[log] ` prefix.
    let output = run["result"]["output"].as_array().unwrap();
    assert!(
        output
            .iter()
            .any(|l| l.as_str().is_some_and(|s| s.contains("hello-boa"))),
        "captured console.log: {run}"
    );

    // 5. a script error is reported, and the session SURVIVES it.
    send(&mut stdin, json!({"cmd": "run-script", "source": "this is not valid js ("}));
    let err = recv(&mut stdout);
    assert_eq!(err["ok"], json!(false), "script error surfaces: {err}");
    assert!(err["result"]["error"].is_string(), "error text present: {err}");

    // 6. inspect returns structured state.
    send(&mut stdin, json!({"cmd": "inspect"}));
    let insp = recv(&mut stdout);
    assert_eq!(insp["ok"], json!(true), "inspect: {insp}");
    assert!(insp["sceneTree"].is_array(), "sceneTree is an array: {insp}");
    assert!(insp["meta"].is_object(), "meta is an object: {insp}");

    // 7. export idml → non-empty bytes on disk.
    send(
        &mut stdin,
        json!({"cmd": "export", "format": "idml", "out": idml_out.to_str().unwrap()}),
    );
    let exp = recv(&mut stdout);
    assert_eq!(exp["ok"], json!(true), "export idml: {exp}");
    assert!(exp["bytes"].as_u64().unwrap() > 0, "export produced bytes: {exp}");
    assert!(idml_out.exists(), "idml written to disk");

    // 8. the exported IDML re-loads (round-trip).
    send(&mut stdin, json!({"cmd": "load", "path": idml_out.to_str().unwrap()}));
    let reload = recv(&mut stdout);
    assert_eq!(reload["ok"], json!(true), "reload exported idml: {reload}");
    assert!(reload["pageCount"].as_u64().unwrap() >= 1, "reload has a page: {reload}");

    // 9. render page 0 (CPU path) → a non-empty PNG.
    send(
        &mut stdin,
        json!({"cmd": "render", "page": 0, "dpi": 72.0, "out": png_out.to_str().unwrap()}),
    );
    let render = recv(&mut stdout);
    assert_eq!(render["ok"], json!(true), "render: {render}");
    assert!(render["widthPx"].as_u64().unwrap() > 0, "render has width: {render}");
    assert!(
        std::fs::metadata(&png_out).map(|m| m.len() > 0).unwrap_or(false),
        "png written and non-empty"
    );

    // 10. quit → clean exit.
    send(&mut stdin, json!({"cmd": "quit"}));
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "clean exit");

    let _ = std::fs::remove_file(&idml_out);
    let _ = std::fs::remove_file(&png_out);
}

#[test]
fn commands_before_load_error_cleanly() {
    let mut child = spawn();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let _ = recv(&mut stdout); // greeting

    send(&mut stdin, json!({"cmd": "inspect"}));
    let resp = recv(&mut stdout);
    assert_eq!(resp["ok"], json!(false), "inspect before load errors: {resp}");
    assert!(resp["error"].as_str().unwrap().contains("no document loaded"));

    // a malformed request is reported but does not kill the session
    send(&mut stdin, json!({"cmd": "not-a-real-command"}));
    let bad = recv(&mut stdout);
    assert_eq!(bad["ok"], json!(false), "unknown cmd errors: {bad}");

    // session still alive afterwards
    send(&mut stdin, json!({"cmd": "new-blank", "width": 200.0, "height": 200.0}));
    let ok = recv(&mut stdout);
    assert_eq!(ok["ok"], json!(true), "session survives prior errors: {ok}");

    send(&mut stdin, json!({"cmd": "quit"}));
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
