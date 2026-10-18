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

//! `paged export --tagged` / `--bookmarks` reach the PDF writer: the
//! flags were wire options with no CLI spelling.

use std::path::PathBuf;
use std::process::Command;

fn export(stem: &str, flags: &[&str]) -> Vec<u8> {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = std::env::temp_dir().join(format!(
        "paged-pdf-flags-{stem}-{}-{}.pdf",
        flags.len(),
        std::process::id()
    ));
    let run = Command::new(env!("CARGO_BIN_EXE_paged"))
        .arg("export")
        .arg(repo.join(format!("corpus/generated/{stem}.idml")))
        .args(["--format", "pdf", "-o", out.to_str().unwrap()])
        .args(flags)
        .output()
        .expect("run paged export");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::read(&out).expect("pdf written")
}

fn has(pdf: &[u8], needle: &str) -> bool {
    pdf.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[test]
fn tagged_flag_writes_a_structure_tree() {
    assert!(!has(&export("text", &[]), "/StructTreeRoot"));
    let pdf = export("text", &["--tagged"]);
    assert!(has(&pdf, "/StructTreeRoot"));
    assert!(has(&pdf, "/MarkInfo"));
}

#[test]
fn bookmarks_flag_writes_the_outline() {
    assert!(has(&export("navigation", &["--bookmarks"]), "/Outlines"));
}
