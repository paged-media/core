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

//! ADR 130 — a rectangle's and a text frame's drop shadow is cast from
//! what the frame paints, asked of InDesign by the `rect-shadows`
//! fixture.
//!
//! Every probe value is InDesign 20.0.1's own: the darkness (0 = paper,
//! 1 = black) of its export of the fixture, rasterised by pdftoppm at
//! 144 dpi, at points where only the shadow lands. As in
//! `line_shadows_pipeline.rs`, each is compared as a fraction of a
//! reference shadow — here the filled, unstroked rectangle's, which the
//! engine already matched — so the test measures shadow shape, not how
//! a host's colour management renders 75 % black.
//!
//! InDesign casts a stroke-only rectangle's shadow from its stroke band
//! (the engine cast none), a filled and stroked one's from the outline
//! pushed out by the stroke's outer part (the engine cast the fill
//! rectangle), a rounded one's from the rounded outline (the engine
//! cast the bounding rectangle), and a text frame's from its stroke AND
//! its text when it has no fill (the engine cast nothing).

use paged_compose::Color;
use paged_renderer::pipeline::{self, PipelineOptions};

const DPI: f32 = 144.0;
/// The engine's relative darkness must sit within this of InDesign's.
const TOLERANCE: f32 = 0.06;
/// InDesign's darkness 1 pt below the filled, unstroked rectangle's
/// bottom edge, (470, 501) pt.
const INDESIGN_REFERENCE: f32 = 0.664;

fn darkness_at(img: &image::RgbaImage, x_pt: f32, y_pt: f32) -> f32 {
    let s = DPI / 72.0;
    let p = img.get_pixel((x_pt * s) as u32, (y_pt * s) as u32).0;
    // The probes sit on neutral greys; Rec. 601 luma, as the reference
    // measurement took it.
    let luma = 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
    (255.0 - luma) / 255.0
}

#[test]
fn rectangle_and_text_frame_shadows_follow_what_each_paints() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::rect_shadows::build()).expect("idml");
    let document = idml_import::import_idml_doc(&bytes).expect("open rect-shadows");
    let font = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .expect("read Inter.ttf");
    let opts = PipelineOptions {
        font: Some(&font),
        ..PipelineOptions::default()
    };
    let (_built, images) =
        pipeline::render_document(&document, &opts, DPI, Color::WHITE).expect("render");
    let img = &images[0];

    // (what, x pt, y pt, InDesign's darkness). The filled shapes are
    // probed 1 pt inside their shadow's outer edge as InDesign draws
    // it, where the stroke decides whether there is a shadow at all;
    // the text 6 pt into the shadow of the stems, at mid height, so the
    // probe does not depend on the line's baseline.
    let probes: [(&str, f32, f32, f32); 20] = [
        (
            "stroke-only 6, shadow of the bottom edge",
            130.0,
            166.0,
            0.581,
        ),
        ("stroke-only 6, shadow of the top edge", 130.0, 86.0, 0.577),
        ("stroke-only 6, inside: no shadow", 130.0, 120.0, 0.0),
        (
            "stroke-only 1, shadow of the bottom edge",
            300.0,
            166.0,
            0.133,
        ),
        ("stroke-only 6 outside, shadow", 470.0, 169.0, 0.577),
        (
            "filled 6 centre, shadow past the stroke",
            130.0,
            335.0,
            0.659,
        ),
        ("filled 6 inside, shadow of the fill", 300.0, 333.0, 0.632),
        (
            "filled 6 outside, shadow past the stroke",
            470.0,
            338.0,
            0.660,
        ),
        (
            "rounded filled, shadow past the stroke",
            130.0,
            505.0,
            0.659,
        ),
        ("rounded filled, bounding-box corner", 196.0, 506.0, 0.024),
        (
            "rounded stroke-only, shadow of the bottom",
            300.0,
            506.0,
            0.581,
        ),
        ("rounded stroke-only, inside: no shadow", 300.0, 460.0, 0.0),
        (
            "text, stroke-only, shadow of the stroke",
            130.0,
            676.0,
            0.581,
        ),
        ("text, stroke-only, shadow of the H", 119.0, 628.0, 0.467),
        (
            "text, stroke-only, inside the frame: none",
            100.0,
            615.0,
            0.0,
        ),
        ("text, filled, shadow past the stroke", 300.0, 675.0, 0.659),
        (
            "text, no fill or stroke, shadow of the H",
            459.0,
            628.0,
            0.475,
        ),
        (
            "text, no fill or stroke, shadow of the i",
            481.0,
            628.0,
            0.475,
        ),
        (
            "text, no fill or stroke, frame edge: none",
            470.0,
            676.0,
            0.0,
        ),
        (
            "text, no fill or stroke, frame corner: none",
            536.0,
            676.0,
            0.0,
        ),
    ];
    let engine_reference = darkness_at(img, 470.0, 501.0);
    assert!(
        engine_reference > 0.3,
        "the filled rectangle casts a shadow"
    );
    let mut misses = Vec::new();
    for (what, x, y, indesign) in probes {
        let engine = darkness_at(img, x, y) / engine_reference;
        let indesign = indesign / INDESIGN_REFERENCE;
        if (engine - indesign).abs() > TOLERANCE {
            misses.push(format!(
                "{what} at ({x}, {y}) pt: engine {engine:.3}, InDesign {indesign:.3} \
                 (of the filled rectangle's shadow)"
            ));
        }
    }
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}
