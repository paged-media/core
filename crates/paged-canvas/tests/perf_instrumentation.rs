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

//! W1.24 (audit B17–B19) — performance-lane instrumentation tests.
//!
//! Covers the two crate-side perf surfaces:
//!   * B18 — `RebuildStats`: present after load, populated + monotone
//!     across rebuilds, op-apply vs view-state timing distinguished.
//!   * B19 — the `applied_log` cap: the log never exceeds
//!     `MAX_APPLIED_LOG`, eviction is oldest-first, and undo of the
//!     freshest entries stays correct after the cap is exceeded.

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};

/// Minimal one-frame, one-story IDML — copied from the determinism
/// suite's helper. A real (parseable) document so `apply_mutation`
/// exercises the full edit→rebuild path rather than a stub.
fn small_idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Hello world</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn load() -> CanvasModel {
    CanvasModel::load("d", &small_idml(), CanvasOptions::default()).expect("load + build")
}

fn insert_at(offset: u32, text: &str) -> Mutation {
    Mutation::InsertText {
        story_id: "story1".into(),
        offset,
        text: text.into(),
        cell: None,
    }
}

// ----- B18: RebuildStats -------------------------------------------------

#[test]
fn rebuild_stats_present_after_cold_load() {
    let model = load();
    let s = model.last_rebuild_stats();
    // The cold build counts as rebuild #1 and must carry plausible
    // sizes (one page, ≥ one paragraph). build_ms is non-negative; we
    // don't assert a floor because a fast machine can round it to 0.
    assert_eq!(s.rebuilds, 1, "cold load is rebuild #1: {s:?}");
    assert_eq!(s.pages, 1, "fixture has exactly one page: {s:?}");
    assert!(s.paragraphs >= 1, "≥ one paragraph laid out: {s:?}");
    assert!(s.build_ms >= 0.0, "build_ms non-negative: {s:?}");
    assert_eq!(s.op_apply_ms, 0.0, "no edit preceded the cold load: {s:?}");
    assert_eq!(s.applied_log_len, 0, "no mutations applied yet: {s:?}");
}

#[test]
fn rebuild_stats_monotone_and_populated_across_mutations() {
    let mut model = load();
    let r0 = model.last_rebuild_stats().rebuilds;

    model.apply_mutation(&insert_at(5, "X")).expect("insert 1");
    let s1 = model.last_rebuild_stats();
    assert!(s1.rebuilds > r0, "rebuild counter must advance: {s1:?}");
    assert_eq!(s1.applied_log_len, 1, "one undoable mutation: {s1:?}");
    assert!(s1.build_ms >= 0.0 && s1.op_apply_ms >= 0.0);

    model.apply_mutation(&insert_at(0, "Y")).expect("insert 2");
    let s2 = model.last_rebuild_stats();
    assert!(
        s2.rebuilds > s1.rebuilds,
        "counter strictly monotone across rebuilds: {s1:?} -> {s2:?}"
    );
    assert_eq!(s2.applied_log_len, 2, "two undoable mutations: {s2:?}");
    assert_eq!(s2.pages, 1, "still one page: {s2:?}");
}

#[test]
fn undo_redo_refresh_rebuild_stats() {
    let mut model = load();
    model.apply_mutation(&insert_at(5, "X")).expect("insert");
    let after_insert = model.last_rebuild_stats().rebuilds;

    model.undo().expect("undo");
    let after_undo = model.last_rebuild_stats();
    assert!(
        after_undo.rebuilds > after_insert,
        "undo runs a rebuild and bumps the counter: {after_undo:?}"
    );
    // Undo shrank the undo log back to empty.
    assert_eq!(after_undo.applied_log_len, 0, "log emptied by undo");

    model.redo().expect("redo");
    let after_redo = model.last_rebuild_stats();
    assert!(
        after_redo.rebuilds > after_undo.rebuilds,
        "redo rebuilds too"
    );
    assert_eq!(after_redo.applied_log_len, 1, "redo re-grew the log");
}

// ----- B19: applied_log cap (end-to-end undo correctness) ---------------
//
// The exhaustive cap-eviction logic (fill 10k+ synthetic records,
// assert oldest-evicted, length saturates) is a fast in-module unit
// test in `model.rs` — it pushes records directly without 10k full
// rebuilds, so it runs in microseconds. Here we assert the *end-to-end*
// correctness contract the cap must not break: a normal undo/redo
// timeline round-trips exactly. The cap never touches this path while
// the log is under the cap (the overwhelming common case), and the unit
// test proves the over-cap path keeps the freshest entries intact.

#[test]
fn undo_redo_round_trips_under_cap() {
    let mut model = load();
    let before = doc_signature(&model);

    // A short timeline: two inserts then two undos returns to start.
    model.apply_mutation(&insert_at(5, "X")).expect("insert 1");
    model.apply_mutation(&insert_at(0, "Y")).expect("insert 2");
    assert_ne!(doc_signature(&model), before, "edits changed the doc");

    model.undo().expect("undo 2");
    model.undo().expect("undo 1");
    assert_eq!(
        doc_signature(&model),
        before,
        "undoing the whole timeline restores the original scene"
    );

    // Redo both: back to the two-edit state, deterministically.
    model.redo().expect("redo 1");
    model.redo().expect("redo 2");
    let two_edits = doc_signature(&model);
    model.undo().expect("undo 2 again");
    model.undo().expect("undo 1 again");
    assert_eq!(
        doc_signature(&model),
        before,
        "second undo pass also restores"
    );
    model.redo().expect("redo 1 again");
    model.redo().expect("redo 2 again");
    assert_eq!(
        doc_signature(&model),
        two_edits,
        "redo is deterministic across passes"
    );
}

/// Canonical content hash of the (possibly-mutated) scene — the exact
/// determinism proxy the determinism suite uses. Equal hashes ⇔
/// byte-identical derived state.
fn doc_signature(model: &CanvasModel) -> [u8; 32] {
    model.current_state_hash()
}
