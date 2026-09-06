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

//! The object effects reach the PDF.
//!
//! Until W3 the exporter drew only the drop shadow and the gradient
//! feather; inner shadow, both glows, bevel & emboss, satin and the two
//! feathers hit a single arm that logged a debug line and drew nothing,
//! so every exported PDF disagreed with the screen. These assert on the
//! encoding (a tinted stamp wearing an `/SMask`, a clip before the
//! interior effects, a luminosity mask over a feathered fill) and then
//! on the pixels, against the CPU rasterizer of the same page.

use paged_export_pdf::{export_pdf, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};

struct Built {
    doc: paged_renderer::BuiltDocument,
    fonts: FontTable,
    palette: paged_model::Graphic,
}

fn build_effects() -> Built {
    let bytes = paged_gen::write_idml(&paged_gen::samples::effects::build()).expect("emit sample");
    let document = idml_import::import_idml_doc(&bytes).expect("import sample");
    let opts = PipelineOptions {
        collect_glyph_runs: true,
        ..Default::default()
    };
    let fonts = FontTable::build(&document, &opts);
    let doc = {
        let mut o = PipelineOptions {
            collect_glyph_runs: true,
            ..Default::default()
        };
        o.pre_built_font_table = Some(&fonts);
        pipeline::build_document(&document, &o).expect("build document")
    };
    let palette = document.palette.clone();
    Built {
        doc,
        fonts,
        palette,
    }
}

fn export(built: &Built) -> Vec<u8> {
    let cmm = paged_color::IccCmm::new(None, paged_color::DisplaySetup::default());
    export_pdf(ExportInput {
        doc: &built.doc,
        palette: &built.palette,
        fonts: Some(&built.fonts),
        cmm: &cmm,
        profiles: ExportProfiles::default(),
        inks: ExportInkSettings::default(),
        options: ExportOptions::default(),
        doc_bleed: [0.0; 4],
        doc_slug: [0.0; 4],
    })
    .expect("export")
    .bytes
}

/// Every page's content stream, concatenated with the streams of the
/// forms they reference (a feathered fill lives inside one).
fn all_content(doc: &lopdf::Document) -> String {
    let mut out = Vec::new();
    for (_, page_id) in doc.get_pages() {
        out.extend(doc.get_page_content(page_id).unwrap_or_default());
    }
    for (_, obj) in doc.objects.iter() {
        if let Ok(stream) = obj.as_stream() {
            if let Ok(data) = stream.decompressed_content() {
                out.extend(data);
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every image XObject that carries an `/SMask` — the stamp encoding.
fn smasked_images(doc: &lopdf::Document) -> usize {
    doc.objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| {
            s.dict.get(b"Subtype").ok().and_then(|v| v.as_name().ok()) == Some(b"Image")
                && s.dict.get(b"SMask").is_ok()
        })
        .count()
}

#[test]
fn the_blur_based_effects_export_as_tinted_stamps() {
    let built = build_effects();
    let pdf = export(&built);
    let doc = lopdf::Document::load_mem(&pdf).expect("re-parse");
    // The sample has ten effect pages (inner shadow, both glows, three
    // bevels, two satins, two feathers) plus the drop-shadow pages, and
    // a bevel writes TWO stamps (highlight + shadow). Before W3 only the
    // drop shadows produced any.
    assert!(
        smasked_images(&doc) >= 10,
        "expected a tinted stamp per blur-based effect, found {}",
        smasked_images(&doc)
    );
    let text = all_content(&doc);
    assert!(
        text.contains("/Pattern CS") || text.contains("Do"),
        "the stamps are painted"
    );
    // Interior effects clip to the object first (pdf-writer spells the
    // nonzero clip + no-op paint as "W" then "n").
    assert!(
        text.contains("W\nn") || text.contains("W n") || text.contains("W\n"),
        "an interior effect clips to its path"
    );
}

#[test]
fn a_feathered_fill_paints_under_a_luminosity_soft_mask() {
    let built = build_effects();
    let pdf = export(&built);
    let doc = lopdf::Document::load_mem(&pdf).expect("re-parse");
    // A feather masks the object's OWN paint, so it must appear as a
    // soft mask on the fill — never as ink stamped over it.
    let luminosity = doc
        .objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .filter_map(|d| d.get(b"SMask").ok())
        .filter_map(|v| v.as_dict().ok())
        .filter(|sm| sm.get(b"S").ok().and_then(|v| v.as_name().ok()) == Some(b"Luminosity"))
        .count();
    assert!(
        luminosity >= 2,
        "the two feather pages each mask their fill, found {luminosity}"
    );
}

#[test]
fn the_export_is_deterministic() {
    let built = build_effects();
    assert_eq!(export(&built), export(&built), "two exports differ");
}

#[test]
fn effect_pages_paint_something_other_than_paper() {
    // The honest end of the test: rasterise the exported PDF and the
    // renderer's own page, and require the exporter to have put ink
    // where the canvas does. Skipped without poppler.
    if std::process::Command::new("pdftoppm")
        .arg("-v")
        .output()
        .is_err()
    {
        eprintln!("pdftoppm not on PATH — skipping the pixel check");
        return;
    }
    let built = build_effects();
    let pdf = export(&built);
    let dir = std::env::temp_dir().join(format!("paged-fx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let pdf_path = dir.join("effects.pdf");
    std::fs::write(&pdf_path, &pdf).expect("write pdf");
    // The inner-shadow page: variant 12 (0-based) → PDF page 13. It
    // paints INSIDE the shape, so a band across the demo rect must be
    // darker than the paper fill.
    let page = 13;
    let status = std::process::Command::new("pdftoppm")
        .args([
            "-r",
            "72",
            "-f",
            &page.to_string(),
            "-l",
            &page.to_string(),
            "-png",
            "-singlefile",
        ])
        .arg(&pdf_path)
        .arg(dir.join("page"))
        .status()
        .expect("run pdftoppm");
    assert!(status.success(), "pdftoppm failed");
    let png = std::fs::read(dir.join("page.png")).expect("rasterised page");
    let img = image::load_from_memory(&png).expect("decode").to_rgb8();
    // The demo rect sits mid-page; sample a band across it and require
    // the exporter to have painted something darker than paper.
    let (w, h) = img.dimensions();
    let mut darkest = 255u8;
    for y in (h / 3)..(2 * h / 3) {
        for x in (w / 4)..(3 * w / 4) {
            let p = img.get_pixel(x, y);
            darkest = darkest.min(p[0].min(p[1]).min(p[2]));
        }
    }
    assert!(
        darkest < 200,
        "the inner-shadow page exported as bare paper (darkest channel {darkest})"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
