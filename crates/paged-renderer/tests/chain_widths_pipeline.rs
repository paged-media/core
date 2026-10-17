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

//! A story threaded through frames of different widths (ADR 179), over
//! the generated `chain-widths.idml`: InDesign 20.0.1 sets each frame's
//! lines at that frame's measure (its PDF of the same file is the
//! fixture's fidelity reference, text gate on), so no line is wider than
//! the frame it lands in, and the narrow frame holds lines the wide one
//! would not have broken.

use std::path::PathBuf;

use paged_gen::samples::chain_widths::{cases, story_id};
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

#[test]
fn every_line_fits_the_frame_it_lands_in() {
    let bytes =
        paged_gen::write_idml(&paged_gen::samples::chain_widths::build()).expect("write_idml");
    let document = idml_import::import_idml_doc(&bytes).expect("open");
    let mut resolver = BytesResolver::new();
    let inter = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    resolver.add_font("Inter", None, std::fs::read(inter).expect("Inter"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        collect_breaks: true,
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).expect("build");
    let mut report = Vec::new();
    for (i, case) in cases().iter().enumerate() {
        let story = story_id(i as u32);
        let lines: Vec<_> = built
            .breaks
            .iter()
            .filter(|r| r.story_id == story)
            .collect();
        assert!(!lines.is_empty(), "{}: no lines", case.name);
        for r in &lines {
            let frame_w = case.frames[r.frame_idx as usize].0;
            if r.width_pt > frame_w + 0.5 {
                report.push(format!(
                    "{}: frame {} ({frame_w} pt) holds a {:.1} pt line: {:?}",
                    case.name, r.frame_idx, r.width_pt, r.source_text
                ));
            }
        }
        // Every frame takes lines: the narrow one is not skipped.
        for f in 0..case.frames.len() {
            if !lines.iter().any(|r| r.frame_idx as usize == f) {
                report.push(format!("{}: frame {f} holds no line", case.name));
            }
        }
    }
    assert!(report.is_empty(), "\n{}", report.join("\n"));
}
