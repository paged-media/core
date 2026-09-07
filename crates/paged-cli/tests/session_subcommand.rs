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

//! `paged session` speaks the `paged-run` protocol.
//!
//! Both binaries call the same `session::run`, so the loop itself is
//! covered by `paged-run`'s own contract test and is not re-asserted
//! here. What IS this crate's to break is the subcommand plumbing
//! around it: argv parsing that swallows a flag, a `main` that writes
//! anything of its own to stdout before the session's greeting, or a
//! future refactor that routes `session` somewhere else. Those would
//! all leave `paged-run` green and every host of `paged session`
//! broken, so they are asserted here.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn the_session_subcommand_greets_and_answers_in_order() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_paged"))
        .arg("session")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn paged session");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let read_line = |stdout: &mut BufReader<_>| -> serde_json::Value {
        let mut line = String::new();
        let n = stdout.read_line(&mut line).unwrap();
        assert!(n > 0, "session closed without answering");
        serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("bad json {line:?}: {e}"))
    };

    // The greeting is the FIRST line on stdout and nothing precedes it:
    // every host consumes it unconditionally before its first reply.
    let hello = read_line(&mut stdout);
    assert_eq!(hello["ready"], serde_json::json!(true), "greeting: {hello}");
    assert!(hello["protocol"].is_number(), "greeting: {hello}");

    // Replies come back one per request, in order.
    for req in [
        serde_json::json!({"cmd": "new-blank", "width": 612.0, "height": 792.0}),
        serde_json::json!({"cmd": "pages"}),
        serde_json::json!({"cmd": "describe"}),
    ] {
        let cmd = req["cmd"].as_str().unwrap().to_string();
        writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).unwrap();
        stdin.flush().unwrap();
        let reply = read_line(&mut stdout);
        assert_eq!(reply["ok"], serde_json::json!(true), "{cmd}: {reply}");
    }

    // A bad request answers and keeps the session alive — both live
    // hosts depend on recovering rather than respawning.
    writeln!(stdin, "{{ not json").unwrap();
    stdin.flush().unwrap();
    let bad = read_line(&mut stdout);
    assert_eq!(bad["ok"], serde_json::json!(false), "bad request: {bad}");
    writeln!(stdin, "{}", serde_json::json!({"cmd": "pages"})).unwrap();
    stdin.flush().unwrap();
    assert_eq!(read_line(&mut stdout)["ok"], serde_json::json!(true));

    writeln!(stdin, "{}", serde_json::json!({"cmd": "quit"})).unwrap();
    stdin.flush().unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success(), "quit exits cleanly");
}

#[test]
fn the_binary_is_named_paged_and_says_what_it_is() {
    let out = Command::new(env!("CARGO_BIN_EXE_paged"))
        .arg("--help")
        .output()
        .expect("paged --help");
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("session"), "help lists the session: {help}");
}
