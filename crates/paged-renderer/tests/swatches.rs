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

//! W4.7 — end-to-end colour / swatch rendering over the `swatches`
//! generated fixture.
//!
//! The distinctive effect this pack asserts: a standalone tint swatch
//! (`Color/InkHalf`, the brand spot ink at swatch-level `TintValue="50"`)
//! resolves to the *tinted* colour in the rendered output — visibly
//! lighter than the full-strength ink (`Color/InkFull`) on the previous
//! page. Both inks preview through the same CMYK alternate; the only
//! difference is the swatch-level tint folded in by
//! `ColorEntry::effective_cmyk`. A third page proves the `<Swatch>`
//! alias resolves one level of indirection to its wrapped colour
//! (`Color/InkFull`) and paints it.

use paged_compose::Color;
use paged_gen::samples::swatches;
use paged_renderer::{pipeline, Document, PipelineOptions};

/// Mean RGB of the centre 10×10 block of a page raster — robust to AA
/// at the frame edges.
fn centre_rgb(img: &image::RgbaImage) -> (f32, f32, f32) {
    let cx = img.width() / 2;
    let cy = img.height() / 2;
    let (mut r, mut g, mut b, mut n) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for y in cy.saturating_sub(5)..(cy + 5).min(img.height()) {
        for x in cx.saturating_sub(5)..(cx + 5).min(img.width()) {
            let p = img.get_pixel(x, y);
            r += p.0[0] as f32;
            g += p.0[1] as f32;
            b += p.0[2] as f32;
            n += 1.0;
        }
    }
    (r / n, g / n, b / n)
}

#[test]
fn tint_swatch_resolves_lighter_than_full_ink() {
    let bytes = paged_gen::write_idml(&swatches::build()).unwrap();
    let document = Document::open(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let (_built, images) = pipeline::render_document(&document, &opts, 72.0, Color::WHITE).unwrap();

    // Three pages: [full ink, half-tint ink, swatch-alias].
    assert_eq!(images.len(), 3, "three swatch pages → three rasters");

    let full = centre_rgb(&images[0]);
    let half = centre_rgb(&images[1]);
    let aliased = centre_rgb(&images[2]);

    // The half-tint swatch (TintValue=50) interpolates toward paper
    // white, so every channel must be lighter (higher) than the full
    // ink. The brand ink is a cyan-heavy CMYK (100 60 0 10), so red is
    // the most discriminating channel — assert a generous margin there
    // and the overall luminance is up.
    assert!(
        half.0 > full.0 + 10.0,
        "half-tint R should be markedly lighter than full ink: half={half:?}, full={full:?}",
    );
    let full_lum = full.0 + full.1 + full.2;
    let half_lum = half.0 + half.1 + half.2;
    assert!(
        half_lum > full_lum + 30.0,
        "half-tint must be overall lighter than full ink: half_lum={half_lum}, full_lum={full_lum}",
    );

    // The full ink must actually paint a saturated colour (not paper):
    // a cyan-heavy ink is far from white, so its luminance is well
    // below 3×255.
    assert!(
        full_lum < 3.0 * 255.0 - 60.0,
        "full ink should be a saturated (non-paper) colour: full={full:?}",
    );

    // The swatch-alias frame resolves `Swatch/BrandAlias` → its wrapped
    // `Color/InkFull`, so its centre paint must match the full-ink page
    // closely — proving the `<Swatch>` indirection resolved a real fill.
    assert!(
        (aliased.0 - full.0).abs() < 8.0
            && (aliased.1 - full.1).abs() < 8.0
            && (aliased.2 - full.2).abs() < 8.0,
        "swatch-alias frame should paint the wrapped full ink: aliased={aliased:?}, full={full:?}",
    );
}
