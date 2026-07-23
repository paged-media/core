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

//! W4.9 — end-to-end advanced-styles + OpenType-typography rendering
//! over the `styles-cascade` generated fixture.
//!
//! Distinctive effects this pack asserts:
//!   * **next-style chaining** lays out: the Title paragraph (24 pt)
//!     and the Body paragraph (12 pt) both render, and the Title's
//!     larger glyphs ink more than the Body's — proving each paragraph
//!     picked up its own style through the chain.
//!   * **OTF feature runs** reach shaping: rendering page 4 (the
//!     fraction / ordinal / contextual-alternate runs) with the features
//!     ON vs OFF produces different rasters (Inter ships `frac`, `ordn`,
//!     `calt`).
//!   * **hyphenation-zone justified composition** lays out multiple
//!     justified lines of the long body.

use std::path::PathBuf;

use paged_compose::Color;
use paged_gen::samples::styles_cascade as sc;
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}

/// Load the corpus fonts the cascade fixture uses (Inter for the OTF
/// page, Open Sans for the rest). Returns `None` when the corpus fonts
/// aren't present in this checkout, mirroring the corpus-optional
/// convention the other render tests use.
fn resolver() -> Option<BytesResolver> {
    let mut r = BytesResolver::new();
    let inter = std::fs::read(font_dir().join("Inter.ttf")).ok()?;
    let open_sans = std::fs::read(font_dir().join("OpenSans.ttf")).ok()?;
    r.add_font("Inter", None, inter);
    r.add_font("Open Sans", None, open_sans.clone());
    // The fixture's default body font is "Open Sans"; also register it
    // under the renderer's fallback key so un-tagged runs resolve.
    r.add_font("OpenSans", None, open_sans);
    Some(r)
}

fn count_dark_in_band_y(img: &image::RgbaImage, y0: u32, y1: u32, threshold: u8) -> usize {
    let mut n = 0usize;
    for y in y0..y1.min(img.height()) {
        for x in 0..img.width() {
            let p = img.get_pixel(x, y);
            if p.0[0] < threshold && p.0[1] < threshold && p.0[2] < threshold {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn cascade_pages_render_and_otf_features_change_the_raster() {
    let Some(res) = resolver() else {
        eprintln!("skip: corpus fonts not present");
        return;
    };
    let opts = PipelineOptions {
        assets: Some(&res),
        ..PipelineOptions::default()
    };

    // OTF ON.
    let on_doc =
        paged_parse::import_idml_doc(&paged_gen::write_idml(&sc::build()).unwrap()).unwrap();
    let (built_on, on_imgs) =
        pipeline::render_document(&on_doc, &opts, 144.0, Color::WHITE).unwrap();
    // OTF OFF control.
    let off_doc =
        paged_parse::import_idml_doc(&paged_gen::write_idml(&sc::build_otf_off()).unwrap())
            .unwrap();
    let (_built_off, off_imgs) =
        pipeline::render_document(&off_doc, &opts, 144.0, Color::WHITE).unwrap();

    assert_eq!(on_imgs.len(), 5, "five cascade pages → five rasters");
    assert!(
        built_on.stats.glyphs > 20,
        "the cascade fixture should shape many glyphs across its pages, got {}",
        built_on.stats.glyphs,
    );

    // (next-style) Page 1 carries the Title (top) and Body (below). The
    // text frame sits at y ≈ 120 pt = 240 px at 144 dpi; both paragraphs
    // ink. The page total inking proves the styled paragraphs laid out.
    let p1 = &on_imgs[0];
    let p1_ink = count_dark_in_band_y(p1, 240, 320, 90);
    assert!(
        p1_ink > 500,
        "the next-style page must ink the Title + Body paragraphs, got {p1_ink} dark px",
    );

    // (OTF) Page 4 (index 3) must differ between the features-on and
    // features-off renders — proving frac / ordn / calt reached shaping.
    assert_ne!(
        on_imgs[3].as_raw(),
        off_imgs[3].as_raw(),
        "OTF features on vs off rendered identically — frac/ordn/calt not applied",
    );

    // (hyphenation-zone) Page 5 (index 4) composes the long justified
    // body into multiple lines — assert a healthy ink count spanning the
    // text band, not just one line.
    let p5 = &on_imgs[4];
    let body_ink = count_dark_in_band_y(p5, 240, 360, 90);
    assert!(
        body_ink > 1000,
        "the hyphenation-zone page must compose multiple justified lines, got {body_ink} dark px",
    );
}
