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

//! ADR 130 — an object's drop shadow is cast from what the object
//! paints, asked of InDesign by the `line-shadows` fixture.
//!
//! Every probe value is InDesign 20.0.1's own: the darkness (0 = paper,
//! 1 = black) of its export of the fixture, rasterised by pdftoppm at
//! 144 dpi, at points where only the shadow lands. The probes are
//! compared as a fraction of the 6 pt line's shadow centre, so the test
//! measures shadow SHAPE and falloff, not how a host's colour management
//! renders 75 % black (the default pipeline options carry no CMYK
//! profile; the fidelity gate's run does). A line casts the
//! shadow of its stroke band; so does a pen path and a stroke-only
//! triangle (the engine cast nothing for those); a filled, stroked oval
//! casts an ellipse that includes the stroke (the engine cast its
//! bounding rectangle).

use paged_compose::Color;
use paged_renderer::pipeline;

const DPI: f32 = 144.0;
/// The engine's relative darkness must sit within this of InDesign's.
const TOLERANCE: f32 = 0.06;
/// InDesign's darkness at the 6 pt line's shadow centre, (150, 96) pt.
const INDESIGN_REFERENCE: f32 = 0.576;

fn darkness_at(img: &image::RgbaImage, x_pt: f32, y_pt: f32) -> f32 {
    let s = DPI / 72.0;
    let p = img.get_pixel((x_pt * s) as u32, (y_pt * s) as u32).0;
    // The probes sit on neutral greys; Rec. 601 luma, as the reference
    // measurement took it.
    let luma = 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
    (255.0 - luma) / 255.0
}

#[test]
fn shadows_follow_what_each_shape_paints() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::line_shadows::build()).expect("idml");
    let document = idml_import::import_idml_doc(&bytes).expect("open line-shadows");
    let opts = pipeline::PipelineOptions::default();
    let (_built, images) =
        pipeline::render_document(&document, &opts, DPI, Color::WHITE).expect("render");
    let img = &images[0];

    // (what, x pt, y pt, InDesign's darkness)
    let probes: [(&str, f32, f32, f32); 7] = [
        ("1 pt line, shadow centre", 350.0, 96.0, 0.129),
        ("pen path, shadow of the left leg", 106.0, 236.0, 0.588),
        (
            "stroke-only triangle, shadow of the base",
            130.0,
            466.0,
            0.576,
        ),
        ("stroke-only triangle, inside: no shadow", 130.0, 430.0, 0.0),
        ("filled triangle, shadow of the base", 300.0, 466.0, 0.306),
        ("oval, shadow past the stroke", 534.0, 416.0, 0.647),
        ("oval, bounding-box corner: no shadow", 532.0, 462.0, 0.0),
    ];
    let engine_reference = darkness_at(img, 150.0, 96.0);
    assert!(engine_reference > 0.3, "the line casts a shadow");
    let mut misses = Vec::new();
    for (what, x, y, indesign) in probes {
        let engine = darkness_at(img, x, y) / engine_reference;
        let indesign = indesign / INDESIGN_REFERENCE;
        if (engine - indesign).abs() > TOLERANCE {
            misses.push(format!(
                "{what} at ({x}, {y}) pt: engine {engine:.3}, InDesign {indesign:.3} \
                 (of the 6 pt line's shadow centre)"
            ));
        }
    }
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}
