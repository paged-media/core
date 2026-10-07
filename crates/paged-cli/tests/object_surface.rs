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

//! `paged set` / `paged get` (ADR 132) through the binary: a property of
//! the document, a layer and a story range is written with `Mutation::Set`,
//! saved, and read back from the saved file — and a value the schema
//! refuses exits non-zero without writing.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_paged")
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run paged {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn ok(args: &[&str]) -> String {
    let (good, out, err) = run(args);
    assert!(good, "paged {args:?} failed:\n{out}\n{err}");
    out
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("paged-object-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

#[test]
fn set_saves_and_get_reads_it_back_from_the_file() {
    let d = dir();
    let blank = d.join("blank.paged");
    let one = d.join("one.paged");
    let two = d.join("two.paged");
    let (blank, one, two) = (
        blank.to_str().unwrap(),
        one.to_str().unwrap(),
        two.to_str().unwrap(),
    );
    ok(&["new", "--size", "612x792", "-o", blank]);

    // The document: a unit string on a length path.
    let echoed = ok(&["set", blank, "doc", "documentBleedTop", "3mm", "-o", one]);
    assert!(
        echoed.contains("8.50"),
        "set echoes the read-back: {echoed}"
    );
    let read = ok(&["get", one, "doc", "documentBleedTop"]);
    assert!(read.contains("8.50"), "bleed survives the save: {read}");

    // A page, through the target object form.
    let js = d.join("pid.js");
    std::fs::write(
        &js,
        "console.log('pid=' + JSON.parse(paged.pages())[0].selfId);",
    )
    .unwrap();
    let log = ok(&["script", one, js.to_str().unwrap()]);
    let pid = log
        .split("pid=")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .expect("a page id")
        .to_string();
    let page = format!(r#"{{"kind":"page","pageId":"{pid}"}}"#);
    ok(&["set", one, &page, "pageColumnCount", "3", "-o", two]);
    assert_eq!(
        ok(&["get", two, &page, "pageColumnCount"]).trim(),
        r#"{"type":"length","value":3.0}"#
    );

    // v71 polish — `null` clears a nullable row (bare, and as a typed
    // `{ type, value: null }`); a required row refuses it.
    ok(&["set", two, &page, "appliedMaster", "null", "-o", two]);
    assert_eq!(
        ok(&["get", two, &page, "appliedMaster"]).trim(),
        r#"{"type":"text","value":""}"#
    );
    ok(&[
        "set",
        two,
        &page,
        "appliedMaster",
        r#"{"type":"ref","value":null}"#,
        "-o",
        two,
    ]);
    let (good, _, err) = run(&["set", two, "doc", "activeConditionSet", "null"]);
    assert!(!good, "a required row refuses null: {err}");

    // A value the schema refuses: no write, a non-zero exit, a reason.
    let (good, _, err) = run(&["set", two, &page, "pageColumnCount", "0"]);
    assert!(!good, "a zero column count must be refused");
    assert!(err.contains("pageColumnCount"), "{err}");
    let (good, _, err) = run(&["set", two, "doc", "noSuchPath", "1"]);
    assert!(!good && err.contains("not a property"), "{err}");
}
