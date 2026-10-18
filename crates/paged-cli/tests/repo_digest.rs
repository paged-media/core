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

//! ADR 180's gate: a file opened into a repository and projected back out is
//! the same document to the engine. Every built-in fixture (and a native
//! `.paged`) goes into a library and out again, plain and carrying its history,
//! and `paged digest` — the display-list digests the renderer draws from —
//! must not move.

use std::path::Path;
use std::process::Command;

use paged_cli::library::{DiskLibrary, LOCAL_OWNER};
use paged_repo::Ctx;
use serde_json::Value;

fn paged(args: &[&str]) -> std::process::Output {
    let out = Command::new(env!("CARGO_BIN_EXE_paged"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "paged {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn digest(file: &Path) -> Value {
    let out = paged(&["digest", "--compact", file.to_str().unwrap()]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    serde_json::json!({ "combined": v["combined"], "stateHash": v["stateHash"], "pages": v["pageDigests"] })
}

#[test]
fn a_projection_is_the_same_document_to_the_engine() {
    let dir = std::env::temp_dir().join(format!("paged-repo-digest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let fixtures = dir.join("fixtures");
    std::fs::create_dir_all(&fixtures).unwrap();
    paged(&["gen", "emit-all", "--out", fixtures.to_str().unwrap()]);
    paged(&[
        "new",
        "--size",
        "a5",
        "--out",
        fixtures.join("native.paged").to_str().unwrap(),
    ]);

    let lib = DiskLibrary::new(dir.join("library"));
    let ctx = Ctx::human(LOCAL_OWNER, 1);
    let mut checked = 0;
    let mut docs: Vec<_> = std::fs::read_dir(&fixtures)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|x| x.to_str()),
                Some("idml" | "paged")
            )
        })
        .collect();
    docs.sort();
    for (i, doc) in docs.iter().enumerate() {
        let name = format!("d{i}");
        let bytes = std::fs::read(doc).unwrap();
        lib.import(&name, &bytes, &doc.file_name().unwrap().to_string_lossy())
            .unwrap();
        let mut repo = lib.open(&name).unwrap();
        let plain = lib
            .run(&name, &mut repo, |r| r.export("main", &ctx, false))
            .unwrap();
        let carried = lib
            .run(&name, &mut repo, |r| r.export("main", &ctx, true))
            .unwrap();
        let ext = doc.extension().unwrap().to_string_lossy().to_string();
        let (a, b) = (
            dir.join(format!("{name}-plain.{ext}")),
            dir.join(format!("{name}-history.{ext}")),
        );
        std::fs::write(&a, plain).unwrap();
        std::fs::write(&b, carried).unwrap();
        let original = digest(doc);
        assert_eq!(digest(&a), original, "{}: the projection", doc.display());
        assert_eq!(
            digest(&b),
            original,
            "{}: the projection carrying its history",
            doc.display()
        );
        checked += 1;
    }
    assert!(checked > 10, "the fixture set was not emitted ({checked})");
    eprintln!("{checked} documents: digests equal after a round trip through a repository");
    let _ = std::fs::remove_dir_all(&dir);
}
