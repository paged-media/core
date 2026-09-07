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

//! The chain a document actually travels: `new` → `script` → `export`
//! → `render` → `diff`, each step through the binary, because that is
//! the seam a user has and the one argv parsing can break while every
//! library test stays green.
//!
//! The script is `dtp_examples.rs`'s `workflow_two_column_article`,
//! unchanged. It passes there against a `CanvasModel` built in-process;
//! asserting the same source through the shell is what proves the CLI
//! reaches the same engine.

use std::path::{Path, PathBuf};
use std::process::Command;

const SCRIPT: &str = r#"const pid = JSON.parse(paged.pages())[0].selfId;
const frame = paged.insertTextFrame(pid, [72, 72, 720, 540]);
const sid = JSON.parse(paged.stories())[0].selfId;
paged.insertText(sid, 0, 'Headline goes here\nBody copy flows beneath the headline and fills both columns with continuous text set at a comfortable reading size.');
paged.set(frame, 'textFrameColumnCount', 2);
paged.set(frame, 'textFrameColumnGutter', 14);
const heading = paged.createParagraphStyle({ name: 'Article Heading' });
paged.applyStyle(sid, 0, 17, heading);
console.log('laid out in', frame);"#;

fn paged(args: &[&str]) -> std::process::Output {
    let out = Command::new(env!("CARGO_BIN_EXE_paged"))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run paged {args:?}: {e}"));
    assert!(
        out.status.success(),
        "paged {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// A face to shape with. The document the script authors names no
/// family at all, and core ships no fallback, so without one the page
/// is legitimately blank — see `a_document_that_names_no_font_says_so`.
fn a_font() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts");
    root.join("Inter.ttf")
}

/// A directory of this test's own. Every test in a binary shares a
/// pid, so keying on that alone let one test's cleanup delete the
/// directory another was still writing into.
fn dir(who: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("paged-cli-{who}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

#[test]
fn new_then_script_then_export_then_render_then_diff() {
    if !a_font().exists() {
        eprintln!("skipping: corpus/fonts absent");
        return;
    }
    let d = dir("chain");
    let (blank, js) = (d.join("blank.paged"), d.join("article.js"));
    std::fs::write(&js, SCRIPT).unwrap();
    let font = a_font();
    let (font, blank, js) = (
        font.to_str().unwrap(),
        blank.to_str().unwrap(),
        js.to_str().unwrap(),
    );

    paged(&["new", "--size", "612x792", "-o", blank]);

    // Author, and write the result out in BOTH native formats plus a
    // render — the composition the plan specifies in place of a
    // filesystem verb inside the script bridge.
    let authored = d.join("article.paged");
    let png = d.join("article.png");
    let out = paged(&[
        "script",
        blank,
        js,
        "-o",
        authored.to_str().unwrap(),
        "--render",
        png.to_str().unwrap(),
        "--dpi",
        "96",
        "--font",
        font,
    ]);
    let log = String::from_utf8_lossy(&out.stdout);
    assert!(log.contains("laid out in"), "script console output: {log}");

    // The script's work must survive the round trip: reopening the
    // written document and re-rendering it must give the same pixels
    // as the in-session render. A `.paged` writer that dropped the
    // authored frame would still produce a PNG — just a blank one —
    // so compare, don't merely check the file exists.
    let reopened = d.join("reopened.png");
    paged(&[
        "render",
        authored.to_str().unwrap(),
        "--page",
        "1",
        "--dpi",
        "96",
        "--font",
        font,
        "-o",
        reopened.to_str().unwrap(),
    ]);
    let same = Command::new(env!("CARGO_BIN_EXE_paged"))
        .args(["diff", png.to_str().unwrap(), reopened.to_str().unwrap()])
        .output()
        .expect("diff");
    assert!(
        same.status.success(),
        "the reopened document renders differently: {}",
        String::from_utf8_lossy(&same.stdout)
    );

    // ...and it is not blank, which is the failure the comparison
    // above cannot see on its own.
    let text = paged(&["inspect", authored.to_str().unwrap(), "--font", font]);
    let report = String::from_utf8_lossy(&text.stdout);
    assert!(report.contains("1 frame(s)"), "inspect: {report}");
    assert!(
        !report.contains("0 glyph(s)"),
        "the authored text shaped no glyphs: {report}"
    );

    // IDML and PDF are the other two exits. An IDML is a ZIP, so its
    // markup is compressed and grepping the file for `<TextFrame` finds
    // nothing whether or not the frame is there — reopen it instead,
    // which is the assertion that actually means something.
    let idml = d.join("out.idml");
    paged(&[
        "export",
        authored.to_str().unwrap(),
        "--format",
        "idml",
        "--font",
        font,
        "-o",
        idml.to_str().unwrap(),
    ]);
    let reread = paged(&["inspect", idml.to_str().unwrap(), "--font", font]);
    let reread = String::from_utf8_lossy(&reread.stdout);
    assert!(
        reread.contains("1 frame(s)") && !reread.contains("0 glyph(s)"),
        "the exported IDML lost the authored frame: {reread}"
    );

    let pdf = d.join("out.pdf");
    paged(&[
        "export",
        authored.to_str().unwrap(),
        "--format",
        "pdf",
        "--font",
        font,
        "-o",
        pdf.to_str().unwrap(),
    ]);
    let bytes = std::fs::read(&pdf).unwrap();
    assert!(
        bytes.starts_with(b"%PDF"),
        "not a PDF ({} bytes)",
        bytes.len()
    );
    // A PDF of a blank page is ~700 bytes; one carrying this article is
    // far larger. Without a floor the assertion above passes on an
    // empty page, which is the exact failure this file exists to catch.
    assert!(
        bytes.len() > 5_000,
        "the PDF looks empty ({} bytes) — the text did not reach it",
        bytes.len()
    );

    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_document_that_names_no_font_says_so() {
    let d = dir("nofont");
    let (blank, js) = (d.join("nf.paged"), d.join("nf.js"));
    std::fs::write(&js, SCRIPT).unwrap();
    paged(&["new", "-o", blank.to_str().unwrap()]);

    // No --font, no --fonts: the text cannot be shaped, and the whole
    // point is that the CLI says why instead of writing a white page.
    let out = paged(&[
        "script",
        blank.to_str().unwrap(),
        js.to_str().unwrap(),
        "-o",
        d.join("nf-out.paged").to_str().unwrap(),
    ]);
    let warning = String::from_utf8_lossy(&out.stderr);
    assert!(
        warning.contains("shaped 0 glyphs"),
        "expected the no-font warning on stderr, got: {warning}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn gen_emits_the_same_bytes_as_paged_gen() {
    let d = dir("gen");
    paged(&[
        "gen",
        "emit",
        "--sample",
        "geometry",
        "--out",
        d.to_str().unwrap(),
    ]);
    let mine = std::fs::read(d.join("geometry.idml")).expect("emitted fixture");
    let theirs = paged_gen::write_idml(&paged_gen::samples::build("geometry").unwrap()).unwrap();
    assert_eq!(
        mine, theirs,
        "`paged gen` must emit the fixture the hard gate measures, byte for byte"
    );
    let _ = std::fs::remove_dir_all(&d);
}
