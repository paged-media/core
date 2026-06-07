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

//! W1.24 (audit B17) — the engine's first criterion bench lane.
//!
//! Why this exists: the canvas re-runs the FULL pipeline
//! (`build_document` → display list) after every mutation — the editor
//! types a character and the whole document relays out. Before W1.24
//! there was no benchmark lane, so a relayout-cost regression (or the
//! win from a future incremental pass — see
//! `crates/paged-canvas/INCREMENTAL-RELAYOUT.md`) was invisible to CI
//! and to a developer doing a local A/B.
//!
//! ## What it measures
//!
//! | bench group        | what it times                                  |
//! |--------------------|------------------------------------------------|
//! | `build_document`   | cold full pipeline on a paged-gen fixture      |
//! | `rebuild`          | mutation round-trip: InsertText + full rebuild |
//! | `hit_test`         | one pointer hit-test on a built document        |
//! | `digest`           | `DisplayList::digest` over a built page         |
//!
//! `build_document` and `rebuild` run on two fixture sizes: `text`
//! (small — short paragraphs, one frame per page) and `tables` (medium
//! — multi-cell table layout). Both come from `paged-gen`'s sample
//! generators, regenerated in-process at bench-setup time (see
//! `fixture` below) so NO fixture bytes are committed — the same
//! regenerate-don't-store posture the fidelity corpus uses.
//!
//! ## How to run
//!
//! ```text
//! make bench                 # the whole lane (NOT part of `make verify`)
//! cargo bench -p paged-canvas
//! cargo bench -p paged-canvas -- --test   # compile + one-iteration smoke
//! cargo bench -p paged-canvas rebuild     # one group by name
//! ```
//!
//! The lane is intentionally NOT wired into `verify` — benchmarks are a
//! profiling tool, not a pass/fail gate. The `-- --test` smoke is what
//! the W1.24 gate run used to prove the benches compile + run.

use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

use paged_canvas::channel::Mutation;
use paged_canvas::{CanvasModel, CanvasOptions};
use paged_renderer::{pipeline, Document, PipelineOptions};

/// Load the license-clear Inter face the text fixtures declare. Same
/// corpus path the layout-cache integration test uses.
fn inter_font() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join("Inter.ttf");
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("read bench font fixture {}: {e}", path.display()))
}

/// Regenerate one `paged-gen` sample to in-memory IDML bytes. Nothing
/// is written to disk — the bench owns the bytes for its lifetime, so
/// the fixtures stay gitignored-regenerable (they are never committed).
fn fixture(name: &str) -> Vec<u8> {
    let sample = match name {
        "text" => paged_gen::samples::text::build(),
        "tables" => paged_gen::samples::tables::build(),
        other => panic!("unknown bench fixture {other:?}"),
    };
    paged_gen::write_idml(&sample).expect("emit bench IDML")
}

/// A loaded model + its canvas options, reused across the rebuild /
/// hit-test / digest benches.
fn load_model(idml: &[u8], font: &[u8]) -> CanvasModel {
    let opts = CanvasOptions {
        fonts: vec![font.to_vec()],
        ..Default::default()
    };
    CanvasModel::load("bench", idml, opts).expect("bench fixture loads")
}

/// Cold full-pipeline build straight through `paged-renderer`, no model
/// caches — the floor cost of laying out the whole document.
fn bench_build_document(c: &mut Criterion) {
    let font = inter_font();
    let mut group = c.benchmark_group("build_document");
    for name in ["text", "tables"] {
        let idml = fixture(name);
        let scene = Document::open(&idml).expect("parse bench fixture");
        group.bench_function(name, |b| {
            b.iter(|| {
                let options = PipelineOptions {
                    font: Some(font.as_slice()),
                    ..PipelineOptions::default()
                };
                let built = pipeline::build_document(&scene, &options).expect("build");
                std::hint::black_box(built.pages.len())
            })
        });
    }
    group.finish();
}

/// The editor hot path: apply one `InsertText` then run the full
/// `rebuild_after_mutation`. Uses `iter_batched` with a fresh model per
/// iteration so the mutation doesn't accumulate (each sample measures
/// ONE edit→relayout, the thing a keystroke triggers). The model's
/// persistent layout / emit caches are warm from `load`, so this is the
/// realistic incremental-with-caches number, not the cold floor above.
fn bench_rebuild(c: &mut Criterion) {
    let font = inter_font();
    let mut group = c.benchmark_group("rebuild");
    for name in ["text", "tables"] {
        let idml = fixture(name);
        group.bench_function(name, |b| {
            b.iter_batched(
                || load_model(&idml, &font),
                |mut model| {
                    // Insert into the first story at offset 0 — valid for
                    // any text fixture, exercises edit + relayout.
                    let story_id = first_story_id(&model);
                    let out = model.apply_mutation(&Mutation::InsertText {
                        story_id,
                        offset: 0,
                        text: "x".into(),
                        cell: None,
                    });
                    std::hint::black_box(out.is_ok())
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

/// One pointer hit-test against a built document — the per-mousemove
/// cost the select/text tools pay. Points at the centre of the first
/// page; the result (hit or miss) doesn't matter, only the traversal.
fn bench_hit_test(c: &mut Criterion) {
    let font = inter_font();
    let idml = fixture("text");
    let model = load_model(&idml, &font);
    let page_id = model.page_ids().next().expect("at least one page").clone();
    let page = model.page(&page_id).expect("page present");
    let mid = (page.width_pt / 2.0, page.height_pt / 2.0);
    c.bench_function("hit_test", |b| {
        b.iter(|| {
            let r = model.hit_test(&page_id, std::hint::black_box(mid));
            std::hint::black_box(r)
        })
    });
}

/// `DisplayList::digest` over the first built page — the "same code,
/// same scene" tripwire the viewer + cross-artifact equivalence tests
/// run. Benched so a digest-algorithm change's cost is visible.
fn bench_digest(c: &mut Criterion) {
    let font = inter_font();
    let idml = fixture("tables");
    let model = load_model(&idml, &font);
    let page_id = model.page_ids().next().expect("a page").clone();
    let list = model
        .built()
        .display_list_for_page(&page_id)
        .expect("display list");
    c.bench_function("digest", |b| b.iter(|| std::hint::black_box(list.digest())));
}

/// First story id in the loaded scene — the rebuild bench inserts here.
fn first_story_id(model: &CanvasModel) -> String {
    model
        .scene()
        .stories
        .first()
        .map(|s| s.self_id.clone())
        .expect("bench fixture has at least one story")
}

criterion_group!(
    benches,
    bench_build_document,
    bench_rebuild,
    bench_hit_test,
    bench_digest
);
criterion_main!(benches);
