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

//! `paged query <doc> <selector>` (ADR 131): one canonical address per
//! line, the reply envelope with `--json`, a caret under the offending
//! byte when the selector does not parse.

use std::path::PathBuf;
use std::process::Command;

fn fixture() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/generated/geometry-groups.idml")
        .to_string_lossy()
        .into_owned()
}

fn paged(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_paged"))
        .args(args)
        .output()
        .expect("run paged")
}

#[test]
fn query_prints_one_address_per_line() {
    let doc = fixture();
    let out = paged(&["query", &doc, "textFrame, group"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.contains(&"textFrame:ua365e1"), "{text}");
    assert!(lines.iter().any(|l| l.starts_with("group:")), "{text}");
    assert!(
        lines
            .iter()
            .all(|l| l.starts_with("textFrame:") || l.starts_with("group:")),
        "{text}"
    );
}

#[test]
fn query_json_is_the_engine_reply() {
    let out = paged(&["query", &fixture(), "page:#1", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["kind"], "queryResult");
    assert_eq!(v["payload"]["addresses"].as_array().map(Vec::len), Some(1));
    assert!(v["payload"]["error"].is_null());
}

#[test]
fn a_malformed_selector_fails_with_a_caret() {
    let out = paged(&["query", &fixture(), "page > sprocket"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("\"sprocket\" is not a kind"), "{err}");
    assert!(err.contains("         ^"), "{err}");
}
