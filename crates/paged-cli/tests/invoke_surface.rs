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

//! `paged invoke` / `paged ops` through the binary: operations that had
//! no verb (`insertTextFrame`, `linkFrames`, `createSwatch`) are applied
//! with their wire arguments, saved, and read back from the saved file —
//! and an op the catalog lacks, an excused op and arguments that do not
//! fit all exit non-zero without writing.

use std::path::PathBuf;
use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_paged"))
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
    let d = std::env::temp_dir().join(format!("paged-invoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn ids(doc: &str, kind: &str) -> Vec<String> {
    ok(&["query", doc, kind])
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix(&format!("{kind}:"))
                .map(str::to_string)
        })
        .collect()
}

#[test]
fn ops_lists_the_catalog_and_invoke_applies_any_of_them() {
    // `paged ops` is the catalog's vocabulary, with the one excused op.
    let listed: serde_json::Value =
        serde_json::from_str(&ok(&["ops", "--json"])).expect("ops --json is JSON");
    let rows = listed.as_array().expect("an array");
    assert!(rows.len() > 100, "only {} ops listed", rows.len());
    let excused: Vec<&serde_json::Value> =
        rows.iter().filter(|r| r["invokable"] == false).collect();
    assert_eq!(excused.len(), 1);
    assert_eq!(excused[0]["op"], "bindCreated");

    let d = dir();
    let blank = d.join("blank.paged");
    let one = d.join("one.paged");
    let two = d.join("two.paged");
    let args = d.join("swatch.json");
    let (blank, one, two) = (
        blank.to_str().unwrap(),
        one.to_str().unwrap(),
        two.to_str().unwrap(),
    );
    ok(&["new", "--size", "612x792", "-o", blank]);
    let page = ids(blank, "page").into_iter().next().expect("a page");

    // Two text frames, inline arguments.
    let frame = |bounds: &str| format!(r#"{{"pageId":"{page}","bounds":{bounds}}}"#);
    let echoed = ok(&[
        "invoke",
        blank,
        "insertTextFrame",
        &frame("[72,72,300,300]"),
        "-o",
        one,
    ]);
    let reply: serde_json::Value = serde_json::from_str(echoed.trim()).expect("a JSON reply");
    assert_eq!(reply["op"], "insertTextFrame");
    assert!(!reply["createdId"].is_null(), "the reply names the frame");
    ok(&[
        "invoke",
        one,
        "insertTextFrame",
        &frame("[320,72,560,300]"),
        "-o",
        one,
    ]);
    let frames = ids(one, "textFrame");
    assert_eq!(frames.len(), 2, "two frames saved: {frames:?}");

    // Thread them, and read the link back through the v71 row.
    ok(&[
        "invoke",
        one,
        "linkFrames",
        &format!(r#"{{"from":"{}","to":"{}"}}"#, frames[0], frames[1]),
        "-o",
        two,
    ]);
    assert_eq!(
        ok(&[
            "get",
            two,
            &format!("textFrame:{}", frames[0]),
            "nextTextFrame"
        ])
        .trim(),
        format!(r#"{{"type":"text","value":"{}"}}"#, frames[1])
    );

    // Arguments from a file.
    std::fs::write(
        &args,
        r#"{"spec":{"selfId":"Color/Invoked","name":"Invoked","space":"CMYK","value":[0,100,0,0]}}"#,
    )
    .unwrap();
    ok(&[
        "invoke",
        two,
        "createSwatch",
        args.to_str().unwrap(),
        "-o",
        two,
    ]);
    assert!(
        ok(&["get", two, "swatch:Color/Invoked", "name"]).contains("Invoked"),
        "the swatch was saved"
    );

    // Refusals: not an op, the excused op, arguments that do not fit.
    let (good, _, err) = run(&["invoke", two, "notAnOp", "{}"]);
    assert!(!good, "an op the catalog lacks is refused: {err}");
    let (good, _, err) = run(&["invoke", two, "bindCreated", r#"{"handle":"a"}"#]);
    assert!(!good, "bindCreated is refused on its own: {err}");
    let (good, _, err) = run(&["invoke", two, "insertTextFrame", r#"{"pageId":"x"}"#]);
    assert!(!good && err.contains("do not fit"), "{err}");
}
