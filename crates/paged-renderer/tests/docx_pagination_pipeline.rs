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

//! ADR 029 — a Word document lowered the ADR's way (one story per
//! section, the section's margin box as its frame, a grow rule) paginates
//! like WORD. The expected pages are Word's own, from its PDF export of
//! plugin-doc's `pagination_docx()` (`scripts/word-pagination-probe.sh`,
//! 2026-10-01); `paged_gen::samples::docx_pagination` mirrors that document.

use std::path::PathBuf;

use paged_gen::samples::docx_pagination::{section_story_id, sections};
use paged_renderer::{pipeline, PipelineOptions};

/// Word's pagination: (page width, page height, lines, first, last).
const WORD: [(f32, f32, usize, &str, &str); 5] = [
    (612.0, 792.0, 53, "S1 P001", "S1 P053"),
    // S1 P054 carries keepNext: Word moved it to the next page.
    (612.0, 792.0, 54, "S1 P054", "S1 P107"),
    (612.0, 792.0, 13, "S1 P108", "S1 P120"),
    (595.3, 419.55, 28, "S2 P001", "S2 P028"),
    (595.3, 419.55, 12, "S2 P029", "S2 P040"),
];

fn inter_font() -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

#[test]
fn a_lowered_word_document_paginates_like_word() {
    let bytes =
        paged_gen::write_idml(&paged_gen::samples::docx_pagination::build()).expect("write_idml");
    let mut doc = idml_import::import_idml_doc(&bytes).expect("import");
    let section_stories: Vec<String> = (0..sections().len() as u32).map(section_story_id).collect();
    for s in doc.stories.iter_mut() {
        if section_stories.contains(&s.self_id) {
            s.story.grow = Some(paged_model::FlowGrowRule {
                copy_frame_options: true,
                ..Default::default()
            });
        }
    }
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");

    // Per page: size, line count, first and last paragraph label.
    let label = |story: &str, idx: u32| -> String {
        let s = doc.stories.iter().find(|s| s.self_id == story).unwrap();
        let text: String = s.story.paragraphs[idx as usize]
            .runs
            .iter()
            .map(|r| r.text.as_str())
            .collect();
        text.split(" of ").next().unwrap_or("").to_string()
    };
    let mut engine = Vec::new();
    for page in &built.pages {
        let mut lines: Vec<(String, u32)> = Vec::new();
        for story in &section_stories {
            for l in built.story_layout(story) {
                if l.page_id == page.id {
                    lines.push((story.clone(), l.paragraph_idx));
                }
            }
        }
        let first = lines.first().map(|(s, i)| label(s, *i)).unwrap_or_default();
        let last = lines.last().map(|(s, i)| label(s, *i)).unwrap_or_default();
        engine.push((page.width_pt, page.height_pt, lines.len(), first, last));
    }

    let word: Vec<(f32, f32, usize, String, String)> = WORD
        .iter()
        .map(|(w, h, n, f, l)| (*w, *h, *n, f.to_string(), l.to_string()))
        .collect();
    let same = engine.len() == word.len()
        && engine.iter().zip(word.iter()).all(|(e, w)| {
            (e.0 - w.0).abs() < 0.5
                && (e.1 - w.1).abs() < 0.5
                && e.2 == w.2
                && e.3 == w.3
                && e.4 == w.4
        });
    assert!(
        same,
        "pagination differs from Word\n  engine: {engine:#?}\n  word:   {word:#?}"
    );
}
