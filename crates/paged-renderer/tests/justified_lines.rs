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

//! Justified text breaks where InDesign 20.0.1 breaks it (ADR 144): the
//! `justified-probe` paragraphs against the lines InDesign set for them
//! (`tools/indesign-export/justified-lines-probe.json`). The breaker was
//! fitted on this probe; the count of paragraphs that match line for line
//! may only grow.

use paged_gen::samples::justified_probe::{story_id, TEXTS, WIDTHS};
use paged_renderer::{pipeline, PipelineOptions};

/// Paragraphs (of 160) whose every line matches InDesign's.
const MATCHING: usize = 99;

fn open_sans() -> Vec<u8> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts/OpenSans.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read OpenSans.ttf: {e}"))
}

fn norm(s: &str) -> String {
    s.replace('\u{ad}', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn justified_lines_break_where_indesign_breaks_them() {
    let probe: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/indesign-export/justified-lines-probe.json"
    ))
    .expect("probe json");
    let bytes = paged_gen::write_idml(&paged_gen::samples::justified_probe::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = open_sans();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    let rows = probe["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 2 * TEXTS.len() * WIDTHS.len());
    let mut matching = 0;
    let mut differ = Vec::new();
    for row in rows {
        let t = row["text"].as_u64().unwrap() as usize;
        let width = row["width"].as_f64().unwrap() as f32;
        let c = WIDTHS
            .iter()
            .position(|w| *w == width)
            .expect("a probe width");
        let page = if row["hyphenate"].as_bool().unwrap() {
            0
        } else {
            1
        };
        let theirs: Vec<String> = row["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| norm(l.as_str().unwrap()))
            .collect();
        let ours: Vec<String> = built
            .story_layout(&story_id(page, t as u32, c as u32))
            .iter()
            .map(|l| norm(&TEXTS[t][l.byte_range.start as usize..l.byte_range.end as usize]))
            .collect();
        if ours == theirs {
            matching += 1;
        } else {
            let at = ours
                .iter()
                .zip(&theirs)
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            differ.push(format!(
                "text {t} at {width} pt, hyphenation {}: line {at} ours {:?}, InDesign {:?}",
                page == 0,
                ours.get(at),
                theirs.get(at)
            ));
        }
    }
    assert!(
        matching >= MATCHING,
        "{matching} of {} paragraphs match InDesign (ratchet {MATCHING}):\n{}",
        rows.len(),
        differ.join("\n")
    );
    if matching > MATCHING {
        eprintln!("{matching} paragraphs match: raise MATCHING");
    }
}
