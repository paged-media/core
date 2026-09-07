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

//! `font_registry_from_paths` — reading a fonts directory the way every
//! host has been spelling it by hand.
//!
//! The editor's showcase harness carries a thirteen-entry
//! family→filename table (`tests/showcase/driver.ts`), and
//! `paged-inspect` makes the caller write the same thing as
//! `--font-family` flags. Both are transcriptions of what the face
//! already says in its `name` table. This pins that the scanner reads
//! back exactly what those tables assert, so the tables can go.

use std::path::PathBuf;

use paged_canvas::{font_face_lookup, font_registry_from_paths};

fn corpus_fonts() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

#[test]
fn a_fonts_directory_names_the_families_the_editor_hardcodes() {
    let dir = corpus_fonts();
    if !dir.is_dir() {
        eprintln!("skipping: {} absent", dir.display());
        return;
    }
    let registry = font_registry_from_paths(&[dir]);
    assert!(!registry.is_empty(), "the corpus directory has faces in it");

    // Every (family, style) pair the showcase harness spells out by
    // hand, verbatim from `driver.ts`'s `faces` table.
    for (family, style) in [
        ("Inter", None),
        ("Open Sans", None),
        ("Open Sans", Some("Italic")),
        ("Source Serif 4", None),
        ("EB Garamond", None),
        ("EB Garamond", Some("Italic")),
        ("Fraunces", None),
        ("Fraunces", Some("Italic")),
        ("JetBrains Mono", None),
        ("JetBrains Mono", Some("Italic")),
        ("Space Grotesk", None),
        ("Noto Sans Arabic", None),
        ("Noto Sans JP", None),
    ] {
        let hit = registry
            .iter()
            .find(|e| e.family == family && e.style.as_deref() == style);
        assert!(
            hit.is_some(),
            "the scan must name {family:?}/{style:?}; it found {:?}",
            registry
                .iter()
                .map(|e| (e.family.as_str(), e.style.as_deref()))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn an_upright_face_answers_a_styled_request_and_the_italic_one_does_not() {
    // The style a scanned face carries is not decoration: `None`
    // registers the family bare so `font_face_lookup`'s fall-through
    // serves every weight from the variable font, while `Some("Italic")`
    // keeps a separate file from answering for the upright one. Get
    // that backwards and a document sets its whole body copy in italic.
    let dir = corpus_fonts();
    if !dir.is_dir() {
        eprintln!("skipping: {} absent", dir.display());
        return;
    }
    let registry = font_registry_from_paths(&[dir]);

    let bold = font_face_lookup(&registry, "Fraunces", Some("Bold"))
        .expect("a bare family answers a weight it has no separate file for");
    assert_eq!(bold.style, None, "answered by the upright file");

    let italic = font_face_lookup(&registry, "Fraunces", Some("Italic"))
        .expect("and the italic file answers Italic");
    assert_eq!(italic.style.as_deref(), Some("Italic"));
    assert_ne!(
        bold.bytes, italic.bytes,
        "two different files, not the same one twice"
    );
}

#[test]
fn licences_and_readmes_are_skipped_not_fatal() {
    // A real fonts directory holds OFL texts and a checksum manifest.
    let dir = corpus_fonts();
    if !dir.is_dir() {
        eprintln!("skipping: {} absent", dir.display());
        return;
    }
    let registry = font_registry_from_paths(&[dir]);
    assert!(
        registry.iter().all(|e| !e.family.is_empty()),
        "no entry is named after a licence file"
    );
    // The directory demonstrably contains non-font files.
    let non_fonts = std::fs::read_dir(corpus_fonts())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.ends_with(".txt") || n.ends_with(".sha256")
        })
        .count();
    assert!(
        non_fonts > 0,
        "the fixture directory must exercise the skip"
    );
}

#[test]
fn a_single_file_path_is_a_registry_of_one() {
    let file = corpus_fonts().join("Inter.ttf");
    if !file.is_file() {
        eprintln!("skipping: {} absent", file.display());
        return;
    }
    let registry = font_registry_from_paths(&[file]);
    assert_eq!(registry.len(), 1);
    assert_eq!(registry[0].family, "Inter");
}
