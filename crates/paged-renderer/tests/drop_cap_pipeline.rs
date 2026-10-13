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

//! A drop cap is sized by the paragraph's leading and its face's cap
//! height, as InDesign 20.0.1 set the `nested-styles` paged-gen fixture's
//! case n13: a two-line cap in 11/14 Inter, read from InDesign's PDF.

use paged_renderer::{pipeline, PipelineOptions};

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

#[test]
fn a_drop_cap_spans_the_stated_leading_at_the_faces_cap_height() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::nested_styles::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    // Case n13 (the 13th frame): "Drop cap paragraph with nested styles
    // across two lines of text." under a one-letter, two-line cap.
    let story = paged_gen::ids::self_id("nested-styles", "BodyStory", 12);
    let lines = built.story_layout(&story);
    // The line ranges index the text after the dropped letter.
    let text = "rop cap paragraph with nested styles across two lines of text.";
    let set: Vec<&str> = lines
        .iter()
        .map(|l| text[l.byte_range.start as usize..l.byte_range.end as usize].trim())
        .collect();
    // InDesign sets the cap at 30.24 pt (14 pt of leading over Inter's
    // 0.7275 cap height, plus the body's 11 pt), so the body starts
    // 19.17 pt into the 250 pt frame and "across" would overrun it by
    // 0.43 pt: it goes to the second line.
    assert_eq!(
        set,
        [
            "rop cap paragraph with nested styles",
            "across two lines of text."
        ]
    );
    // The body beside the cap starts where InDesign's PDF has "rop":
    // 55.168 pt (the frame's left edge is at 36 pt).
    let r = lines[0]
        .clusters
        .iter()
        .find(|c| c.byte == 0)
        .expect("the first body letter");
    assert!((r.x_pt - 55.168).abs() < 0.02, "body starts at {}", r.x_pt);
}
