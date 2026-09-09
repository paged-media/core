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

//! `paged read` / `paged parts` / `describe` / `digest`, through the
//! binary.
//!
//! `cli_surface.rs` proves the CLI's source NAMES each wire kind, which
//! is what makes the ratchet cheap — and is not the same as the
//! subcommand working. The script surface taught that the hard way one
//! commit ago: three argument shapes were documented from the type
//! definitions and were wrong, and only tests that drove them said so.
//!
//! So every read below is run against a document seeded through the
//! same binary, and asserted on the ANSWER — the layer's flags, the
//! story's text, the swatch that resolves, the face that comes back
//! byte-for-byte — not on the exit code.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The document these reads interrogate, authored through `paged
/// script` so the fixture is the product's own output.
const SEED: &str = r#"const pid = JSON.parse(paged.pages())[0].selfId;
const A = (x, y) => ({ anchor: [x, y], left: [x, y], right: [x, y] });
const tf = paged.insertTextFrame(pid, [72, 72, 300, 400]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'Hello world');
paged.layerInsert(0, 'Art');
const sw = paged.createSwatch({ space: 'CMYK', value: [0, 100, 100, 0], name: 'Vermilion' });
paged.createGradient({ kind: 'Linear', name: 'Dawn', stops: [
  { stopColor: 'Color/Black', locationPct: 0 }, { stopColor: sw, locationPct: 100 } ] });
paged.insertOval(pid, [400, 100, 500, 200]);
paged.insertOval(pid, [450, 150, 550, 250]);
paged.insertPath(pid, [A(100, 600), A(200, 600), A(200, 660)], true, false);
console.log('seeded');"#;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_paged")
}

fn ok(args: &[&str]) -> String {
    let out = Command::new(bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run paged {args:?}: {e}"));
    assert!(
        out.status.success(),
        "paged {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A run expected to FAIL, returning stderr. The error paths matter as
/// much as the answers: a read that cannot find its subject must say so
/// and exit non-zero, not print an empty success.
fn refused(args: &[&str]) -> String {
    let out = Command::new(bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run paged {args:?}: {e}"));
    assert!(
        !out.status.success(),
        "paged {args:?} should have failed, printed:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn fonts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

fn dir(who: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("paged-read-{who}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// Author the fixture and hand back its path.
fn seeded(who: &str) -> (PathBuf, PathBuf) {
    let d = dir(who);
    let blank = d.join("blank.paged");
    let js = d.join("seed.js");
    let doc = d.join("seeded.paged");
    std::fs::write(&js, SEED).unwrap();
    ok(&["new", "--size", "612x792", "-o", blank.to_str().unwrap()]);
    let log = ok(&[
        "script",
        blank.to_str().unwrap(),
        js.to_str().unwrap(),
        "-o",
        doc.to_str().unwrap(),
    ]);
    assert!(log.contains("seeded"), "seed script output: {log}");
    (d, doc)
}

#[test]
fn the_diagnostic_reads_answer_what_the_document_holds() {
    let (_d, doc) = seeded("reads");
    let doc = doc.to_str().unwrap();

    let layers = ok(&["read", "layers", doc, "--compact"]);
    assert!(
        layers.contains(r#""kind":"layers""#) && layers.contains(r#""name":"Art""#),
        "layers: {layers}"
    );

    let swatches = ok(&["read", "collection", doc, "swatches", "--compact"]);
    assert!(
        swatches.contains("Vermilion") && swatches.contains(r#""name":"swatches""#),
        "collection: {swatches}"
    );

    let chain = ok(&["read", "frame-chain", doc, "Story/u0", "--compact"]);
    assert!(
        chain.contains(r#""kind":"frameChainResult""#),
        "chain: {chain}"
    );

    let story = ok(&["read", "story-content", doc, "Story/u0", "--compact"]);
    assert!(story.contains("Hello world"), "story: {story}");

    let holes = ok(&["read", "placeholders", doc, "--compact"]);
    assert!(holes.contains(r#""items":[]"#), "placeholders: {holes}");

    let black = ok(&["read", "color-preview", doc, "Color/Black", "--compact"]);
    assert!(
        black.contains(r##""rgbHex":"#000000""##),
        "preview: {black}"
    );

    let grad = ok(&["read", "gradient-detail", doc, "Gradient/u0", "--compact"]);
    assert!(
        grad.contains("Dawn") && grad.contains("stops"),
        "gradient: {grad}"
    );

    let props = ok(&[
        "read",
        "element-properties",
        doc,
        "textFrame:u1",
        "--compact",
    ]);
    assert!(props.contains("frameBounds"), "properties: {props}");

    let geom = ok(&[
        "read",
        "element-geometry",
        doc,
        "textFrame:u1",
        "oval:u2",
        "--compact",
    ]);
    assert!(
        geom.contains("[72.0,72.0,300.0,400.0]") && geom.contains("[400.0,100.0,500.0,200.0]"),
        "geometry: {geom}"
    );

    let leaves = ok(&["read", "group-leaves", doc, "u9", "--compact"]);
    assert!(leaves.contains(r#""ids":[]"#), "leaves: {leaves}");

    let anchors = ok(&["read", "path-anchors", doc, "polygon:u4", "--compact"]);
    assert!(anchors.contains("[100.0,600.0]"), "anchors: {anchors}");

    let regions = ok(&[
        "read",
        "planar-regions",
        doc,
        "oval:u2",
        "oval:u3",
        "--compact",
    ]);
    assert!(
        regions.contains(r#""found":true"#) && regions.contains(r##""id":"0#0""##),
        "regions: {regions}"
    );

    // CMYK channels are 0..100 on this wire, not 0..1 — the read is the
    // cheapest place to pin that, and getting it wrong silently paints
    // 1% magenta where 100% was meant.
    let red = ok(&[
        "read",
        "color-compute",
        doc,
        "CMYK",
        "--value",
        "0",
        "--value",
        "100",
        "--value",
        "100",
        "--value",
        "0",
        "--compact",
    ]);
    assert!(red.contains(r##""rgbHex":"#ff0000""##), "compute: {red}");
}

/// Two reads serve BYTES, and a byte read that finds nothing must not
/// leave an empty file behind looking like a successful read of an
/// empty thing.
#[test]
fn the_byte_reads_serve_what_is_registered_and_refuse_what_is_not() {
    let inter = fonts_dir().join("Inter.ttf");
    if !inter.exists() {
        eprintln!("skipping: corpus/fonts absent");
        return;
    }
    let (d, doc) = seeded("bytes");
    let doc = doc.to_str().unwrap();
    let face = d.join("face.ttf");
    let inter = inter.to_str().unwrap();

    ok(&[
        "read",
        "font-face",
        doc,
        "Inter",
        "-o",
        face.to_str().unwrap(),
        "--fonts",
        inter,
        "--compact",
    ]);
    let served = std::fs::read(&face).expect("the face was written");
    let source = std::fs::read(inter).expect("read Inter.ttf");
    assert_eq!(
        served, source,
        "the engine must serve the registered face byte for byte"
    );

    let missing = d.join("nothing.ttf");
    let err = refused(&[
        "read",
        "font-face",
        doc,
        "NoSuchFamily",
        "-o",
        missing.to_str().unwrap(),
        "--fonts",
        inter,
    ]);
    assert!(err.contains("found: false"), "stderr: {err}");
    assert!(
        !missing.exists(),
        "a face that was not found must leave no file"
    );

    let ase = d.join("swatches.ase");
    ok(&["read", "swatch-library", doc, "-o", ase.to_str().unwrap()]);
    assert!(
        std::fs::metadata(&ase).expect("ase written").len() > 0,
        "the swatch library should carry the document's swatches"
    );
}

/// The address grammar is `paged-wire`'s, shared with `paged.set` — and
/// a bad one is an error naming the forms, not a panic.
#[test]
fn a_bad_element_address_is_refused_with_the_grammar() {
    let (_d, doc) = seeded("address");
    let err = refused(&[
        "read",
        "element-properties",
        doc.to_str().unwrap(),
        "nonsense",
    ]);
    assert!(
        err.contains("not an element address") && err.contains("storyRange:"),
        "stderr: {err}"
    );
}

/// The container-parts door, which the README claimed came for free and
/// which `grep PagedPart crates/paged-cli/src` used to answer with
/// nothing.
#[test]
fn container_parts_are_written_listed_and_read_back() {
    let (d, doc) = seeded("parts");
    let doc = doc.to_str().unwrap();
    let payload = d.join("part.json");
    std::fs::write(&payload, br#"{"hello":"world"}"#).unwrap();
    let saved = d.join("with-part.paged");

    // Without --save the write lands in the loaded model and nothing
    // reaches disk — a container write is not something to do by
    // accident.
    ok(&[
        "parts",
        "write",
        doc,
        "paged/cli-test/data.json",
        payload.to_str().unwrap(),
        "--caller",
        "cli-test",
    ]);
    assert!(!saved.exists(), "no --save must write no file");

    ok(&[
        "parts",
        "write",
        doc,
        "paged/cli-test/data.json",
        payload.to_str().unwrap(),
        "--caller",
        "cli-test",
        "--save",
        saved.to_str().unwrap(),
    ]);

    let listed = ok(&["parts", "list", saved.to_str().unwrap()]);
    assert!(
        listed.contains("paged/cli-test/data.json") && listed.contains("paged/core/model/"),
        "list: {listed}"
    );

    let read = ok(&[
        "parts",
        "read",
        saved.to_str().unwrap(),
        "paged/cli-test/data.json",
    ]);
    assert_eq!(read.trim(), r#"{"hello":"world"}"#);

    let err = refused(&[
        "parts",
        "read",
        saved.to_str().unwrap(),
        "paged/cli-test/nope.json",
    ]);
    assert!(err.contains("no part at"), "stderr: {err}");

    // The C-34 caller gate travels with the door: a named caller writes
    // only its own subtree, from the CLI exactly as from a bundle.
    let err = refused(&[
        "parts",
        "write",
        doc,
        "paged/someone-else/data.json",
        payload.to_str().unwrap(),
        "--caller",
        "cli-test",
    ]);
    assert!(err.contains("own subtree"), "stderr: {err}");
}

/// `describe` needs no document — it is what the surface can be asked
/// to do — and `digest` is the verification oracle in machine form.
#[test]
fn describe_and_digest_answer_at_subcommand_level() {
    let described = ok(&["describe", "--compact"]);
    let catalog: serde_json::Value = serde_json::from_str(&described).expect("describe is JSON");
    assert!(
        catalog["protocol"].as_u64().unwrap_or(0) > 0,
        "describe carries the protocol: {described}"
    );
    let fns = catalog["catalog"]["hostFunctions"]
        .as_array()
        .expect("hostFunctions");
    assert!(
        fns.len() > 100,
        "the catalog should carry the whole host surface, got {}",
        fns.len()
    );

    let (_d, doc) = seeded("digest");
    let doc = doc.to_str().unwrap();
    let a = ok(&["digest", doc, "--compact"]);
    let b = ok(&["digest", doc, "--compact"]);
    assert_eq!(a, b, "the digest is the oracle; it must be deterministic");
    let parsed: serde_json::Value = serde_json::from_str(&a).expect("digest is JSON");
    assert!(parsed["pageDigests"].is_object(), "digest: {a}");
    assert_eq!(
        parsed["stateHash"].as_str().map(str::len),
        Some(64),
        "stateHash is a sha256 hex string: {a}"
    );
}
