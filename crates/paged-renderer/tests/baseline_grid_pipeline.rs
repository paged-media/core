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

//! ADR 140 — align to baseline grid, pinned against InDesign 20.0.1.
//!
//! Every line of every case of `baseline-grid` and `baseline-grid-margin`
//! stands where InDesign set it: the baselines are InDesign's own, read
//! through its DOM by `tools/indesign-export/baseline-grid-probe.sh` into
//! `corpus/generated/<stem>.baselines.json` (page pt from the page top).
//! A case's frame is found there by its top-left corner.

use std::path::PathBuf;

use paged_gen::samples::baseline_grid as sample;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

const TOL: f32 = 0.01;

fn read_font(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read font fixture {}: {e}", p.display()))
}

fn probe(stem: &str) -> serde_json::Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/generated")
        .join(format!("{stem}.baselines.json"));
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).expect("probe json")
}

/// InDesign's baselines for the frame whose top-left is `at` on page
/// `page` (1-based).
fn indesign_baselines(probe: &serde_json::Value, page: usize, at: (f32, f32)) -> Vec<f32> {
    let page = &probe["pages"][page - 1];
    let frame = page["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .find(|f| {
            let b = &f["bounds"];
            (b[0].as_f64().unwrap() as f32 - at.1).abs() < 0.01
                && (b[1].as_f64().unwrap() as f32 - at.0).abs() < 0.01
        })
        .unwrap_or_else(|| panic!("no frame at {at:?} on page {}", page["name"]));
    frame["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|l| l["baseline"].as_f64().unwrap() as f32)
        .collect()
}

fn check(stem: &str, sample: paged_gen::Sample, pages: [Vec<sample::Case>; 2]) {
    let bytes = paged_gen::write_idml(&sample).expect("write_idml");
    let document = idml_import::import_idml_doc(&bytes).expect("open");
    let mut resolver = BytesResolver::new();
    resolver.add_font("Open Sans", None, read_font("OpenSans.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).expect("build_document");
    let probe = probe(stem);
    let mut mismatches = Vec::new();
    let mut compared = 0;
    for (page_idx, cases) in pages.iter().enumerate() {
        for case in cases {
            let want = indesign_baselines(&probe, page_idx + 1, case.slot.at);
            let story = paged_gen::ids::self_id(stem, "Story", case.number);
            let got: Vec<f32> = built
                .story_layout(&story)
                .iter()
                .map(|l| l.baseline_y_pt)
                .collect();
            compared += want.len();
            let same = got.len() == want.len()
                && got.iter().zip(&want).all(|(g, w)| (g - w).abs() <= TOL);
            if !same {
                mismatches.push(format!(
                    "{stem} case {} ({}):\n  engine   {got:?}\n  InDesign {want:?}",
                    case.number, case.label
                ));
            }
        }
    }
    assert!(compared > 40, "{stem}: compared only {compared} lines");
    assert!(
        mismatches.is_empty(),
        "lines off InDesign's baseline grid:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn baseline_grid_from_the_top_of_the_page_and_frame_grids() {
    check(
        sample::SAMPLE,
        sample::build(),
        [sample::page_one_cases(), sample::page_two_cases()],
    );
}

#[test]
fn baseline_grid_from_the_top_margin() {
    check(
        sample::SAMPLE_MARGIN,
        sample::build_margin(),
        [sample::margin_page_one_cases(), sample::margin_page_two_cases()],
    );
}
