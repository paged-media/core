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

//! The grid matrix — 30 InDesign grid layouts × 6 page sizes.
//!
//! One Envato pack ("gridtastic") ships 180 IDMLs that are the same 30
//! designs re-cut for 6x9, 8.5x11, 9.5x12, A3, A4 and A5. Until the
//! 2026-08 corpus campaign only ONE of them was extracted; the other 179
//! sat in the zip. They cost 11 MB and are structurally minimal — pure
//! master spreads, margins, column/gutter geometry, no text, no images —
//! which makes them an unusually cheap regression matrix for exactly the
//! layout maths that has no other real-document coverage.
//!
//! The point is the ORTHOGONALITY: the same design at six page sizes
//! isolates page-size handling from layout handling. A parser that
//! mishandles one axis shows up as a whole row or column failing, not a
//! scatter.
//!
//! OPT-IN — the assets live in the private corpus checkout:
//!
//! ```text
//! PAGED_GRID_CORPUS=1 cargo test -p paged-scene --test grid_matrix_corpus -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Every extracted gridtastic IDML, or `None` with a printed reason.
fn matrix_files() -> Option<Vec<PathBuf>> {
    let Some(switch) = std::env::var_os("PAGED_GRID_CORPUS") else {
        eprintln!(
            "SKIP grid matrix: PAGED_GRID_CORPUS unset \
             (set it to 1, or to a corpus root, and run with --ignored)"
        );
        return None;
    };
    let switch = switch.to_string_lossy().into_owned();
    let root = if switch == "1" || switch.is_empty() {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../corpus")
    } else {
        PathBuf::from(switch)
    };
    let dir = root.join("idml/packs/gridtastic-grid-kit/assets/idml");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!(
            "SKIP grid matrix: {} not readable — run corpus/harness/unpack.sh",
            dir.display()
        );
        return None;
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("idml"))
        })
        .collect();
    out.sort();
    if out.is_empty() {
        eprintln!("SKIP grid matrix: no IDMLs under {}", dir.display());
        return None;
    }
    Some(out)
}

/// `A4-07.idml` → ("A4", "07"); `07.idml` → ("6x9", "07").
///
/// The bare names are the pack's first page-size directory, which the
/// extractor leaves undecorated because there was nothing to disambiguate
/// yet — see the collision rule in `corpus/harness/unpack.sh`.
fn axes(path: &std::path::Path) -> (String, String) {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match stem.rsplit_once('-') {
        Some((size, design)) => (size.to_string(), design.to_string()),
        None => ("6x9".to_string(), stem),
    }
}

#[test]
#[ignore = "grid matrix: opt-in (PAGED_GRID_CORPUS=1 + the private corpus mount)"]
fn every_grid_layout_parses_at_every_page_size() {
    let Some(files) = matrix_files() else {
        return;
    };
    println!("grid matrix: {} IDMLs", files.len());

    // design → page size → page count. A BTreeMap so the failure report
    // reads in a stable order.
    let mut grid: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();

    for path in &files {
        let (size, design) = axes(path);
        let bytes = std::fs::read(path).expect("read grid idml");
        match idml_import::import_idml_doc(&bytes) {
            Ok(doc) => {
                let pages: usize = doc.spreads.iter().map(|s| s.spread.pages.len()).sum();
                assert!(
                    pages > 0,
                    "{}: parsed with ZERO pages — a grid layout always has at least one",
                    path.display()
                );
                grid.entry(design).or_default().insert(size, pages);
            }
            Err(e) => failures.push(format!("{}: {e:?}", path.display())),
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} grid layouts failed to parse:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );

    // The orthogonality check, and the reason this matrix is worth having:
    // the SAME design at different page sizes must produce the same page
    // COUNT. Page size changes geometry, never pagination — a parser that
    // conflates the two shows up here as one design disagreeing with
    // itself across the row.
    let mut inconsistent: Vec<String> = Vec::new();
    for (design, by_size) in &grid {
        let counts: std::collections::BTreeSet<usize> = by_size.values().copied().collect();
        if counts.len() > 1 {
            inconsistent.push(format!("  design {design}: {by_size:?}"));
        }
    }
    assert!(
        inconsistent.is_empty(),
        "the same design paginated differently across page sizes — page size must \
         change geometry, not page count:\n{}",
        inconsistent.join("\n")
    );

    let sizes: std::collections::BTreeSet<&String> = grid.values().flat_map(|m| m.keys()).collect();
    println!(
        "grid matrix: {} designs × {} page sizes, all parsed, pagination consistent",
        grid.len(),
        sizes.len()
    );
}
