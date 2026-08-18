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

//! Module-level pins for the exporter's PDF fragments (audit A4 — the
//! gstate/marks/page/writer/transparency modules carried no tests at
//! this altitude). Everything here asserts on OBSERVABLE output — the
//! content-stream operator text or the re-parsed document structure —
//! at the seam the export-diff.sh golden lane does NOT cover (that
//! lane compares rasterised pixels; these pin the PDF *encoding*).
//!
//! Self-contained: every fixture builds in-memory through `paged-gen`,
//! so nothing here skips on a fresh checkout (unlike the
//! corpus/generated-gated lanes in export_x4.rs).

use paged_export_pdf::marks::{emit_marks, MarkGeometry};
use paged_export_pdf::writer::{DocState, PageResources};
use paged_export_pdf::{
    export_pdf, BleedOptions, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles,
    MarkOptions,
};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};

struct Built {
    doc: paged_renderer::BuiltDocument,
    fonts: FontTable,
    palette: paged_model::Graphic,
}

fn build_sample(sample: &paged_gen::Sample) -> Built {
    let bytes = paged_gen::write_idml(sample).expect("emit sample IDML");
    let document = idml_import::import_idml_doc(&bytes).expect("import sample");
    let opts = PipelineOptions {
        collect_glyph_runs: true,
        ..Default::default()
    };
    let fonts = FontTable::build(&document, &opts);
    let doc = {
        let mut opts2 = PipelineOptions {
            collect_glyph_runs: true,
            ..Default::default()
        };
        opts2.pre_built_font_table = Some(&fonts);
        pipeline::build_document(&document, &opts2).expect("build document")
    };
    let palette = document.palette.clone();
    Built {
        doc,
        fonts,
        palette,
    }
}

fn input_for<'a>(
    built: &'a Built,
    cmm: &'a paged_color::IccCmm,
    options: ExportOptions,
) -> ExportInput<'a> {
    ExportInput {
        doc: &built.doc,
        palette: &built.palette,
        fonts: Some(&built.fonts),
        cmm,
        profiles: ExportProfiles::default(),
        inks: ExportInkSettings::default(),
        options,
        doc_bleed: [0.0; 4],
        doc_slug: [0.0; 4],
    }
}

fn export_with(built: &Built, options: ExportOptions) -> Vec<u8> {
    let cmm = paged_color::IccCmm::new(None, paged_color::DisplaySetup::default());
    let result = export_pdf(input_for(built, &cmm, options)).expect("export");
    result.bytes
}

fn box_of(page: &lopdf::Dictionary, key: &[u8]) -> [f32; 4] {
    let arr = page
        .get(key)
        .unwrap_or_else(|_| panic!("missing {}", String::from_utf8_lossy(key)))
        .as_array()
        .expect("box array");
    let f = |o: &lopdf::Object| o.as_float().unwrap_or(0.0);
    [f(&arr[0]), f(&arr[1]), f(&arr[2]), f(&arr[3])]
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

// ---------------------------------------------------------------- page

/// page.rs — MediaBox/BleedBox/TrimBox geometry: the bleed override
/// expands symmetrically around the trim, marks add their fixed
/// margin outside the bleed, and the boxes nest Media ⊇ Bleed ⊇ Trim.
#[test]
fn page_boxes_honor_bleed_and_marks_margins() {
    let built = build_sample(&paged_gen::samples::geometry::build());
    let trim_w = built.doc.pages[0].width_pt;
    let trim_h = built.doc.pages[0].height_pt;

    // Bleed 9 pt, no marks: BleedBox IS the MediaBox, trim inset 9.
    let bytes = export_with(
        &built,
        ExportOptions {
            bleed: BleedOptions {
                override_pt: Some([9.0; 4]),
            },
            ..Default::default()
        },
    );
    let doc = lopdf::Document::load_mem(&bytes).expect("re-parse");
    let (_, first) = doc.get_pages().into_iter().next().expect("page");
    let page = doc.get_dictionary(first).expect("page dict");
    let media = box_of(page, b"MediaBox");
    let bleedb = box_of(page, b"BleedBox");
    let trim = box_of(page, b"TrimBox");
    assert_eq!(media, bleedb, "no marks: bleed box fills the media box");
    assert!(approx(media[2] - media[0], trim_w + 18.0));
    assert!(approx(media[3] - media[1], trim_h + 18.0));
    for i in 0..2 {
        assert!(approx(trim[i] - bleedb[i], 9.0), "trim inset from bleed");
        assert!(approx(bleedb[i + 2] - trim[i + 2], 9.0));
    }
    // CropBox mirrors the MediaBox (viewer-visible area).
    assert_eq!(box_of(page, b"CropBox"), media);

    // Same bleed + crop marks: the marks margin (offset 6 + mark 18 +
    // slack 6 = 30 pt) wraps AROUND the bleed on every side.
    let bytes = export_with(
        &built,
        ExportOptions {
            bleed: BleedOptions {
                override_pt: Some([9.0; 4]),
            },
            marks: MarkOptions {
                crop_marks: true,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let doc = lopdf::Document::load_mem(&bytes).expect("re-parse");
    let (_, first) = doc.get_pages().into_iter().next().expect("page");
    let page = doc.get_dictionary(first).expect("page dict");
    let media = box_of(page, b"MediaBox");
    let bleedb = box_of(page, b"BleedBox");
    let trim = box_of(page, b"TrimBox");
    for i in 0..2 {
        assert!(approx(bleedb[i] - media[i], 30.0), "marks margin");
        assert!(approx(media[i + 2] - bleedb[i + 2], 30.0));
        assert!(approx(trim[i] - bleedb[i], 9.0), "trim still inset 9");
        assert!(approx(bleedb[i + 2] - trim[i + 2], 9.0));
    }
    assert!(approx(media[2] - media[0], trim_w + 18.0 + 60.0));
    assert!(approx(media[3] - media[1], trim_h + 18.0 + 60.0));
}

// --------------------------------------------------------------- marks

/// marks.rs — drive `emit_marks` directly and read the raw operator
/// text: registration content selects the `/Separation /All` space,
/// every mark coordinate lands OUTSIDE the trim (inside the media),
/// and the crop-mark geometry is exact.
#[test]
fn marks_paint_outside_trim_in_registration_all() {
    // A DocState only exists relative to an ExportInput; build the
    // smallest real one.
    let built = build_sample(&paged_gen::samples::geometry::build());
    let cmm = paged_color::IccCmm::new(None, paged_color::DisplaySetup::default());
    let input = input_for(&built, &cmm, ExportOptions::default());
    let mut state = DocState::new(&input);
    let mut resources = PageResources::default();
    let mut content = pdf_writer::Content::new();

    let geo = MarkGeometry {
        media_w: 700.0,
        media_h: 900.0,
        trim: [50.0, 50.0, 650.0, 850.0],
        bleed: [41.0, 41.0, 659.0, 859.0],
    };
    let opts = MarkOptions {
        crop_marks: true,
        registration_marks: true,
        color_bars: true,
        page_info: false,
        weight_pt: 0.0, // → default 0.25
        offset_pt: 0.0, // → default 6
    };
    emit_marks(&mut content, &mut state, &mut resources, &geo, &opts);
    let bytes = content.finish();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let tokens: Vec<&str> = text.split_whitespace().collect();

    // Registration colour space interned under its page name.
    assert!(
        resources.color_spaces.contains_key("CsAll"),
        "registration marks must intern the /Separation /All space"
    );
    assert!(text.contains("/CsAll"), "content must select /CsAll");

    // Default stroke weight.
    let has_weight = tokens.windows(2).any(|w| w[0] == "0.25" && w[1] == "w");
    assert!(has_weight, "default mark weight 0.25 w missing:\n{text}");

    // Every path coordinate must stay OUT of the trim interior and
    // inside the media. (m/l carry 2 coords, c carries 6.)
    let num = |t: &str| t.parse::<f32>().ok();
    let mut points: Vec<(f32, f32)> = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        let n = match *t {
            "m" | "l" => 1,
            "c" => 3,
            _ => continue,
        };
        for p in 0..n {
            let x = num(tokens[i - 2 * (n - p)]);
            let y = num(tokens[i - 2 * (n - p) + 1]);
            if let (Some(x), Some(y)) = (x, y) {
                points.push((x, y));
            }
        }
    }
    assert!(!points.is_empty(), "no mark geometry emitted:\n{text}");
    let [tx0, ty0, tx1, ty1] = geo.trim;
    for (x, y) in &points {
        let inside_trim = *x > tx0 + 0.5 && *x < tx1 - 0.5 && *y > ty0 + 0.5 && *y < ty1 - 0.5;
        assert!(!inside_trim, "mark point ({x}, {y}) inside the trim box");
        assert!(
            (-0.01..=geo.media_w + 0.01).contains(x) && (-0.01..=geo.media_h + 0.01).contains(y),
            "mark point ({x}, {y}) outside the media box"
        );
    }

    // Exact crop-mark pin: the bottom-left vertical mark runs from
    // (tx0, bleed_bottom − offset) down `MARK_LEN`: (50, 35) → (50, 17).
    let has_bl_mark = tokens.windows(6).any(|w| {
        w[2] == "m"
            && w[5] == "l"
            && num(w[0]) == Some(50.0)
            && num(w[1]) == Some(35.0)
            && num(w[3]) == Some(50.0)
            && num(w[4]) == Some(17.0)
    });
    assert!(has_bl_mark, "bottom-left crop mark not found:\n{text}");

    // Colour bar: 4 process channels × 2 tints = 8 patches, each a
    // CMYK fill (`k`) + rect (`re`).
    let re_count = tokens.iter().filter(|t| **t == "re").count();
    let k_count = tokens.iter().filter(|t| **t == "k").count();
    assert_eq!(re_count, 8, "colour bar patch count");
    assert_eq!(k_count, 8, "colour bar fill count");

    // All-off options emit NOTHING (no stray q/Q pollution).
    let mut empty = pdf_writer::Content::new();
    emit_marks(
        &mut empty,
        &mut state,
        &mut resources,
        &geo,
        &MarkOptions::default(),
    );
    assert!(empty.finish().is_empty(), "marks off must emit no ops");
}

// -------------------------------------------------------------- writer

/// writer.rs — object numbering + xref integrity on a real document:
/// lopdf re-parses (a broken xref fails the load), object ids are
/// DENSE (RefAllocator hands out 1..=N with every id written), the
/// page tree is consistent, and the whole byte stream is
/// deterministic across identical exports.
#[test]
fn writer_produces_dense_deterministic_object_graph() {
    let built = build_sample(&paged_gen::samples::geometry::build());
    let bytes = export_with(&built, ExportOptions::default());
    let doc = lopdf::Document::load_mem(&bytes).expect("re-parse (xref integrity)");

    // Dense numbering: max object id == object count, all gen 0.
    let max_id = doc.objects.keys().map(|(n, _)| *n).max().expect("objects");
    assert_eq!(
        max_id as usize,
        doc.objects.len(),
        "object ids must be dense (an allocated-but-unwritten ref leaves a hole)"
    );
    assert!(doc.objects.keys().all(|(_, gen)| *gen == 0));

    // Page tree: /Count matches, every page's content stream inflates.
    let pages = doc.get_pages();
    assert_eq!(pages.len(), built.doc.pages.len());
    let catalog = doc.catalog().expect("catalog");
    let (_, tree) = doc
        .dereference(catalog.get(b"Pages").expect("Pages"))
        .expect("deref page tree");
    let count = tree
        .as_dict()
        .expect("dict")
        .get(b"Count")
        .and_then(|c| c.as_i64())
        .expect("Count");
    assert_eq!(count as usize, pages.len());
    for page_id in pages.values() {
        let page = doc.get_dictionary(*page_id).expect("page dict");
        let (_, contents) = doc
            .dereference(page.get(b"Contents").expect("Contents"))
            .expect("deref contents");
        let stream = contents.as_stream().expect("content stream");
        let data = stream.decompressed_content().expect("FlateDecode");
        assert!(!data.is_empty(), "empty page content stream");
    }

    // Producer pin (Info dict written at finish).
    let (_, info) = doc
        .dereference(doc.trailer.get(b"Info").expect("Info"))
        .expect("deref Info");
    let producer = info
        .as_dict()
        .expect("dict")
        .get(b"Producer")
        .and_then(|p| p.as_str())
        .expect("Producer");
    assert_eq!(producer, b"paged-export-pdf");

    // Determinism, self-contained (the corpus-gated twin in
    // export_x4.rs skips on fresh checkouts; this one always runs).
    let again = export_with(&built, ExportOptions::default());
    assert_eq!(bytes, again, "same input must yield identical bytes");
}

// -------------------------------------------------- transparency/page

/// transparency.rs + page.rs — every transparency-group Form XObject
/// shares the PAGE's single indirect /Resources dictionary, and every
/// resource name used in any content stream (`gs`, `Do`, `sh`, `Tf`,
/// `cs`/`CS`) resolves in that dictionary — the interning/naming
/// discipline that keeps captured groups renderable.
#[test]
fn transparency_groups_share_page_resources_and_names_resolve() {
    let built = build_sample(&paged_gen::samples::effects::build());
    let bytes = export_with(&built, ExportOptions::default());
    let doc = lopdf::Document::load_mem(&bytes).expect("re-parse");

    // Page resources are indirect; collect their refs.
    let mut resource_refs = std::collections::HashSet::new();
    for (_, page_id) in doc.get_pages() {
        let page = doc.get_dictionary(page_id).expect("page dict");
        let r = page
            .get(b"Resources")
            .and_then(|r| r.as_reference())
            .expect("indirect page /Resources");
        resource_refs.insert(r);
    }

    // Transparency-group forms exist and point at a page's resources.
    let mut groups = 0;
    for obj in doc.objects.values() {
        let Ok(stream) = obj.as_stream() else {
            continue;
        };
        if stream.dict.get(b"Subtype").and_then(|s| s.as_name()).ok() != Some(b"Form".as_slice()) {
            continue;
        }
        let Ok(group) = stream.dict.get(b"Group") else {
            continue;
        };
        groups += 1;
        let group = doc.dereference(group).expect("group").1.as_dict().unwrap();
        assert_eq!(
            group.get(b"S").and_then(|s| s.as_name()).expect("S"),
            b"Transparency"
        );
        let res = stream
            .dict
            .get(b"Resources")
            .and_then(|r| r.as_reference())
            .expect("form /Resources must be the shared indirect dict");
        assert!(
            resource_refs.contains(&res),
            "form resources must be a PAGE's resources dict"
        );
    }
    assert!(groups > 0, "effects must export transparency groups");

    // Name-resolution sweep over every content stream (pages + forms).
    for (_, page_id) in doc.get_pages() {
        let page = doc.get_dictionary(page_id).expect("page dict");
        let res_ref = page
            .get(b"Resources")
            .and_then(|r| r.as_reference())
            .unwrap();
        let res = doc.get_dictionary(res_ref).expect("resources dict");
        let names_in = |cat: &[u8]| -> std::collections::HashSet<Vec<u8>> {
            match res.get(cat).and_then(|d| d.as_dict()) {
                Ok(d) => d.iter().map(|(k, _)| k.to_vec()).collect(),
                Err(_) => Default::default(),
            }
        };
        let ext_g = names_in(b"ExtGState");
        let xobjects = names_in(b"XObject");
        let shadings = names_in(b"Shading");
        let fonts = names_in(b"Font");
        let spaces = names_in(b"ColorSpace");

        let mut streams: Vec<Vec<u8>> = Vec::new();
        let (_, contents) = doc
            .dereference(page.get(b"Contents").unwrap())
            .expect("contents");
        streams.push(
            contents
                .as_stream()
                .unwrap()
                .decompressed_content()
                .unwrap(),
        );
        for obj in doc.objects.values() {
            if let Ok(s) = obj.as_stream() {
                if s.dict.get(b"Resources").and_then(|r| r.as_reference()).ok() == Some(res_ref) {
                    streams.push(s.decompressed_content().unwrap_or_default());
                }
            }
        }
        for data in streams {
            let text = String::from_utf8_lossy(&data).into_owned();
            let tokens: Vec<&str> = text.split_whitespace().collect();
            for (i, t) in tokens.iter().enumerate() {
                let (name_idx, pool, what) = match *t {
                    "gs" => (i - 1, &ext_g, "ExtGState"),
                    "Do" => (i - 1, &xobjects, "XObject"),
                    "sh" => (i - 1, &shadings, "Shading"),
                    "Tf" => (i - 2, &fonts, "Font"),
                    "cs" | "CS" => (i - 1, &spaces, "ColorSpace"),
                    _ => continue,
                };
                let Some(name) = tokens[name_idx].strip_prefix('/') else {
                    // Device space selects (`/DeviceRGB cs`) or inline
                    // operands are fine to skip; only named lookups
                    // must resolve.
                    continue;
                };
                if what == "ColorSpace" && name.starts_with("Device") {
                    continue;
                }
                assert!(
                    pool.contains(name.as_bytes()),
                    "content uses /{name} {t} but the page {what} dict lacks it"
                );
            }
        }
    }
}
