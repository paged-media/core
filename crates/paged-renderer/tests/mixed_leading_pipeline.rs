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

//! A line takes the largest leading among its characters, as InDesign
//! 20.0.1 set the `mixed-leading` paged-gen fixture
//! (`corpus/generated/mixed-leading.pdf`, 2026-10-02): one paragraph per
//! frame, its lines fixed by forced line breaks, each line's baseline read
//! off its 10 pt label.

use paged_gen::samples::mixed_leading::{cases, story_id};
use paged_renderer::{pipeline, PipelineOptions};

/// InDesign's step from each line's baseline to the next, per frame (pt).
/// The forced line break that ENDS a line is one of its characters: in
/// the last frame it carries the next line's 18 pt, so the line set
/// entirely at 12 still steps 18.
const INDESIGN: [&[f32]; 6] = [
    &[12.0, 12.0],
    &[20.0, 20.0, 12.0],
    &[12.0, 9.0],
    &[16.8, 12.0],
    &[24.0, 12.0],
    &[18.0, 18.0, 18.0],
];

/// Frames where the engine still differs: `(frame, the engine's steps)`.
/// - 5: the forced line break ending line 3 carries line 4's 18 pt in
///   InDesign and so counts for line 3. The engine lays each stretch
///   between forced breaks out on its own and the break character is in
///   neither, so the line set at 12 steps 12.
const KNOWN: [(usize, &[f32]); 1] = [(5, &[18.0, 12.0, 18.0])];

fn inter_font() -> Vec<u8> {
    let p =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read Inter.ttf: {e}"))
}

#[test]
fn a_line_takes_the_largest_leading_on_it_as_in_indesign() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::mixed_leading::build()).expect("idml");
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let font = inter_font();
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).expect("build");
    let mut wrong = Vec::new();
    for (i, case) in cases().iter().enumerate() {
        let lines = built.story_layout(&story_id(i as u32));
        let steps: Vec<f32> = lines
            .windows(2)
            .map(|w| w[1].baseline_y_pt - w[0].baseline_y_pt)
            .collect();
        let matches = |want: &[f32]| {
            steps.len() == want.len() && steps.iter().zip(want).all(|(a, b)| (a - b).abs() <= 0.05)
        };
        if let Some((_, known)) = KNOWN.iter().find(|(k, _)| *k == i) {
            assert!(
                matches(known),
                "{}: changed to {steps:?}: update or drop its KNOWN entry",
                case.name
            );
            continue;
        }
        let same = matches(INDESIGN[i]);
        if !same {
            wrong.push(format!(
                "{}: steps {steps:?}, InDesign {:?}",
                case.name, INDESIGN[i]
            ));
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
