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

//! Wall-clock benches on the annual-scale workload (see the crate docs for
//! why the workload carries real fonts and photos).
//!
//! | group    | what it times                                                   |
//! |----------|-----------------------------------------------------------------|
//! | `load`   | `annual-base` from IDML; the authored workload from `.paged`    |
//! | `write`  | one keystroke, a paragraph style, a frame write, a 50-edit batch |
//! | `export` | `.paged`, IDML, and the whole PDF                               |
//! | `raster` | one photo page through the CPU rasteriser                       |
//!
//! Writes run on one long-lived model, as an editing session does.
//! Wall-clock numbers are TRENDED (the nightly), not gated: the
//! deterministic gates are the work budgets in `tests/perf_budgets.rs`.
//!
//! ```text
//! cargo bench -p paged-perf                 # all groups
//! cargo bench -p paged-perf -- write        # one group
//! cargo bench -p paged-perf -- --test       # compile + one-iteration smoke
//! ```

use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use paged_canvas::{CanvasModel, Mutation};

const PHOTO_PX: u32 = 1600;

fn protocol() -> u32 {
    paged_canvas::channel::PROTOCOL_VERSION.0
}

fn bench_load(c: &mut Criterion) {
    let idml =
        paged_gen::write_idml(&paged_gen::samples::annual_base::build()).expect("annual-base");
    let paged = paged_perf::build(PHOTO_PX)
        .model
        .export_paged(protocol())
        .expect("export .paged");
    let mut g = c.benchmark_group("load");
    g.sample_size(10).measurement_time(Duration::from_secs(20));
    g.bench_function("annual_base_idml", |b| {
        b.iter(|| CanvasModel::load("bench", &idml, paged_perf::options()).expect("load"))
    });
    g.bench_function("workload_paged", |b| {
        b.iter(|| CanvasModel::load("bench", &paged, paged_perf::options()).expect("load"))
    });
    g.finish();
}

fn bench_write(c: &mut Criterion) {
    let mut w = paged_perf::build(PHOTO_PX);
    let story = w.body_story.clone();
    let frame = w.image_frames[0].clone();
    let mut g = c.benchmark_group("write");
    g.sample_size(20).measurement_time(Duration::from_secs(15));

    // Each edit is undone (untimed) before the next sample, so every
    // sample measures the same document. Without this the edits pile up in
    // one paragraph: after a few hundred samples it holds thousands of
    // characters and the bench times that paragraph, not the edit (the
    // first baseline read 3 s for a 50-edit batch that costs ~100 ms).
    let undo = |model: &mut CanvasModel, story: &str, start: u32, len: u32| {
        model
            .apply_mutation(&Mutation::DeleteRange {
                story_id: story.to_string(),
                start,
                end: start + len,
                cell: None,
            })
            .expect("undo edit");
    };
    g.bench_function("keystroke", |b| {
        b.iter_custom(|iters| {
            let mut spent = Duration::ZERO;
            for _ in 0..iters {
                let t = std::time::Instant::now();
                w.model
                    .apply_mutation(&Mutation::InsertText {
                        story_id: story.clone(),
                        offset: 500,
                        text: "x".into(),
                        cell: None,
                    })
                    .expect("keystroke");
                spent += t.elapsed();
                undo(&mut w.model, &story, 500, 1);
            }
            spent
        })
    });

    let styles = [
        paged_gen::samples::annual_base::STYLE_HEAD_2,
        paged_gen::samples::annual_base::STYLE_BODY,
    ];
    let mut flip = 0usize;
    g.bench_function("paragraph_style", |b| {
        b.iter(|| {
            flip += 1;
            let m: Mutation =
                serde_json::from_value(serde_json::json!({ "op": "applyStyle", "args": {
                "storyId": story, "start": 2000, "end": 2001,
                "style": styles[flip % 2], "scope": "paragraph" } }))
                .expect("wire");
            w.model.apply_mutation(&m).expect("style")
        })
    });

    let colors = ["Color/Black", "Color/Paper"];
    g.bench_function("frame_write", |b| {
        b.iter(|| {
            flip += 1;
            let m: Mutation = serde_json::from_value(serde_json::json!({ "op": "setElementProperty", "args": {
                "elementId": { "kind": "rectangle", "id": frame },
                "path": "frameFillColor", "value": { "type": "colorRef", "value": colors[flip % 2] } } }))
            .expect("wire");
            w.model.apply_mutation(&m).expect("frame write")
        })
    });

    g.bench_function("batch_50_edits", |b| {
        b.iter_custom(|iters| {
            let mut spent = Duration::ZERO;
            for _ in 0..iters {
                let ops = (0..50)
                    .map(|i| Mutation::InsertText {
                        story_id: story.clone(),
                        offset: 600 + i,
                        text: "y".into(),
                        cell: None,
                    })
                    .collect();
                let t = std::time::Instant::now();
                w.model
                    .apply_mutation(&Mutation::Batch { ops })
                    .expect("batch");
                spent += t.elapsed();
                undo(&mut w.model, &story, 600, 50);
            }
            spent
        })
    });
    g.finish();
}

fn bench_export(c: &mut Criterion) {
    let w = paged_perf::build(PHOTO_PX);
    let mut g = c.benchmark_group("export");
    g.sample_size(10).measurement_time(Duration::from_secs(30));
    g.bench_function("paged", |b| {
        b.iter(|| w.model.export_paged(protocol()).expect("paged"))
    });
    g.bench_function("idml", |b| b.iter(|| w.model.export_idml().expect("idml")));
    g.bench_function("pdf", |b| {
        b.iter(|| {
            let wire = paged_canvas::channel::ExportPdfWireOptions::default();
            let (mut session, _) =
                paged_canvas::export::CanvasExportSession::begin(&w.model, &wire).expect("begin");
            loop {
                let (done, total) = session.export_next_page().expect("page");
                if done >= total {
                    break;
                }
            }
            session.finish().expect("finish")
        })
    });
    g.finish();
}

fn bench_raster(c: &mut Criterion) {
    let w = paged_perf::build(PHOTO_PX);
    let page = w
        .model
        .built()
        .pages
        .iter()
        .find(|p| !p.list.images.is_empty())
        .map(|p| p.id.clone())
        .expect("a page with a photo");
    let mut g = c.benchmark_group("raster");
    g.sample_size(10).measurement_time(Duration::from_secs(15));
    g.bench_function("photo_page_1224px", |b| {
        b.iter(|| paged_canvas::render_snapshot_png(&w.model, &page, 1224).expect("raster"))
    });
    g.finish();
}

criterion_group!(benches, bench_load, bench_write, bench_export, bench_raster);
criterion_main!(benches);
