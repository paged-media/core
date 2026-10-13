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

//! An item takes the text wrap and anchored-object options it leaves out
//! from its object style (ADR 143), pinned against InDesign 20.0.1: the
//! `object-style-options` fixture's lines and objects stand where
//! InDesign's DOM puts them (`tools/indesign-export/object-style-options-probe.json`,
//! read back from the same IDML).

use paged_compose::DisplayCommand;
use paged_gen::ids::self_id;
use paged_renderer::{pipeline, PipelineOptions};

fn open_sans() -> Vec<u8> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts/OpenSans.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read OpenSans.ttf: {e}"))
}

fn probe() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../tools/indesign-export/object-style-options-probe.json"
    ))
    .expect("probe json")
}

#[test]
fn items_take_wrap_and_anchoring_from_their_object_style_as_indesign_does() {
    let bytes =
        paged_gen::write_idml(&paged_gen::samples::object_style_options::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = open_sans();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    let probe = probe();
    let mut wrong = Vec::new();

    // Each case's first two lines: where they start (a wrap pushes them
    // right) and their baseline. InDesign records `[start x, end x,
    // baseline]` in page pt.
    for case in 1..=12u32 {
        let theirs = probe["read_lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == format!("case{case} text"))
            .unwrap_or_else(|| panic!("case {case} in the probe"));
        let ours = built.story_layout(&self_id("object-style-options", "Story", case - 1));
        for (k, line) in theirs["lines"].as_array().unwrap().iter().enumerate() {
            let (x, y) = (
                line[0].as_f64().unwrap() as f32,
                line[2].as_f64().unwrap() as f32,
            );
            let Some(l) = ours.get(k) else {
                wrong.push(format!("case {case} line {k}: missing"));
                continue;
            };
            let start = l.clusters.first().map_or(f32::NAN, |c| c.x_pt);
            if (start - x).abs() > 0.1 || (l.baseline_y_pt - y).abs() > 0.05 {
                wrong.push(format!(
                    "case {case} line {k}: ours ({start}, {}), InDesign ({x}, {y})",
                    l.baseline_y_pt
                ));
            }
        }
    }

    // The anchored cases' 24 × 16 pt boxes: their top-left against
    // InDesign's `[top, left, bottom, right]`.
    let boxes: Vec<(f32, f32)> = built.pages[0]
        .list
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::FillPath { transform, .. }
                if (transform.0[0] - 24.0).abs() < 1e-3 && (transform.0[3] - 16.0).abs() < 1e-3 =>
            {
                Some((transform.0[4], transform.0[5]))
            }
            _ => None,
        })
        .collect();
    for case in 7..=12u32 {
        let item = probe["read"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == format!("case{case} object"))
            .unwrap_or_else(|| panic!("case {case} object in the probe"));
        let b = &item["bounds"];
        let (top, left) = (b[0].as_f64().unwrap() as f32, b[1].as_f64().unwrap() as f32);
        if !boxes
            .iter()
            .any(|(x, y)| (x - left).abs() < 0.05 && (y - top).abs() < 0.05)
        {
            wrong.push(format!(
                "case {case} object: InDesign ({left}, {top}), ours {boxes:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
