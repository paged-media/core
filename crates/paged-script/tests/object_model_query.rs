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

//! `paged.query` / `paged.address` (ADR 131): the object model's read
//! doors in Boa, on a real fixture through the same `CanvasModel` the
//! wire answers from.

use std::path::PathBuf;

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_script::execute_script;

fn load() -> CanvasModel {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/generated/geometry-groups.idml");
    let bytes = std::fs::read(path).expect("read fixture");
    CanvasModel::load("doc-om", &bytes, CanvasOptions::default()).expect("load + build")
}

fn run(model: &mut CanvasModel, src: &str) -> Vec<String> {
    let r = execute_script(model, src);
    assert!(r.error.is_none(), "script error: {:?}", r.error);
    r.output
}

#[test]
fn query_returns_canonical_addresses_the_other_doors_accept() {
    let mut model = load();
    let out = run(
        &mut model,
        r#"const all = JSON.parse(paged.query("frame"));
           console.log(all.length > 0);
           console.log(all.every(a => /^(textFrame|rectangle|oval|polygon|graphicLine|group|master):/.test(a)));
           const tf = JSON.parse(paged.query("textFrame"))[0];
           console.log(tf === "textFrame:ua365e1");
           console.log(paged.set(tf, "frameOpacity", 40));
           const pages = JSON.parse(paged.query("page"));
           console.log(pages.length === JSON.parse(paged.pages()).length);
           console.log(JSON.parse(paged.query("doc")).join());"#,
    );
    assert_eq!(
        out,
        [
            "[log] true",
            "[log] true",
            "[log] true",
            "[log] true",
            "[log] true",
            "[log] doc"
        ]
    );
}

#[test]
fn query_composes_with_containment_and_union() {
    let mut model = load();
    let out = run(
        &mut model,
        r#"const p1 = JSON.parse(paged.query("page:#1 frame"));
           const groups = JSON.parse(paged.query("group"));
           const members = JSON.parse(paged.query("group > *"));
           const union = JSON.parse(paged.query("group, group > *"));
           console.log(p1.length > 0, groups.length > 0, members.length > 0);
           // A nested group is both a group and a member: unions dedupe.
           console.log(union.length === new Set(groups.concat(members)).size);"#,
    );
    assert_eq!(out, ["[log] true true true", "[log] true"]);
}

#[test]
fn a_malformed_selector_throws_with_its_offset() {
    let mut model = load();
    let out = run(
        &mut model,
        r#"try { paged.query("frame[nmae=x]"); console.log("no throw"); }
           catch (e) { console.log(e instanceof SyntaxError, String(e.message).includes("at 6")); }"#,
    );
    assert_eq!(out, ["[log] true true"]);
}

#[test]
fn address_resolves_aliases_and_element_objects() {
    let mut model = load();
    let out = run(
        &mut model,
        r#"console.log(paged.address("frame:ua365e1"));
           console.log(paged.address({ kind: "textFrame", id: "ua365e1" }));
           const first = JSON.parse(paged.pages())[0].selfId;
           console.log(paged.address("page:#1") === "page:" + first);
           console.log(paged.address("frame:does-not-exist"));
           console.log(paged.address("not an address"));"#,
    );
    assert_eq!(
        out,
        [
            "[log] textFrame:ua365e1",
            "[log] textFrame:ua365e1",
            "[log] true",
            "[log] null",
            "[log] null"
        ]
    );
}
