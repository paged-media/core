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

//! `paged package` — through the binary, asserting on the FOLDER it
//! leaves behind: the document re-pointed at `Links/`, the linked file
//! that exists copied there, the one that does not reported missing,
//! the used family's faces in `Document fonts/`, and the two text files.

use std::io::Read;
use std::path::PathBuf;
use std::process::Command;

fn dir(who: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("paged-package-{who}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn package_collects_links_and_fonts_into_one_folder() {
    let work = dir("collect");
    // `images.idml` links `file:checker-128.png` (relative to the
    // document) and an absolute `photo.webp` that is on no machine.
    let doc = work.join("images.idml");
    std::fs::copy(repo().join("corpus/generated/images.idml"), &doc).expect("copy fixture");
    std::fs::write(work.join("checker-128.png"), b"not decoded by a package").expect("link");
    let out = work.join("pkg");

    let run = Command::new(env!("CARGO_BIN_EXE_paged"))
        .args([
            "package",
            doc.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ])
        .args([
            "--fonts",
            repo().join("corpus/fonts").to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run paged package");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).expect("report json");

    let link = |name: &str| {
        report["links"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["fileName"] == name)
            .unwrap_or_else(|| panic!("no link {name}: {report:#}"))
            .clone()
    };
    assert_eq!(link("checker-128.png")["status"], "copied");
    assert_eq!(link("photo.webp")["status"], "missing");
    assert_eq!(
        std::fs::read(out.join("Links/checker-128.png")).unwrap(),
        b"not decoded by a package"
    );

    let open_sans = report["fonts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["family"] == "Open Sans")
        .expect("Open Sans is used");
    assert_eq!(open_sans["status"], "copied");
    assert!(out.join("Document fonts/OpenSans.ttf").is_file());

    assert!(out.join("Instructions.txt").is_file());
    assert!(out.join("images Report.txt").is_file());

    // The packaged document points at its own Links folder.
    let file = std::fs::File::open(out.join("images.idml")).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut spreads = String::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        if entry.name().unwrap().starts_with("Spreads/") {
            entry.read_to_string(&mut spreads).unwrap();
        }
    }
    let links_dir = out.canonicalize().unwrap().join("Links");
    assert!(
        spreads.contains(&format!("file:{}/checker-128.png", links_dir.display())),
        "links not re-pointed"
    );
}

#[test]
fn package_refuses_a_folder_that_holds_files() {
    let work = dir("refuse");
    std::fs::write(work.join("keep.txt"), b"x").unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_paged"))
        .args(["package"])
        .arg(repo().join("corpus/generated/images.idml"))
        .args(["-o", work.to_str().unwrap()])
        .output()
        .expect("run paged package");
    assert!(!run.status.success());
    assert_eq!(std::fs::read(work.join("keep.txt")).unwrap(), b"x");
}
