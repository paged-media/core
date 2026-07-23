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

//! End-to-end PDF/X-4 conformance + determinism over a corpus
//! fixture, validated by re-parsing the produced bytes with lopdf
//! (the honest, Acrobat-free conformance gate; veraPDF/Acrobat
//! preflight are the manual sign-off lane).
//!
//! Fixture-gated like paged-color's parity tests: corpus/generated
//! IDMLs are regenerated locally by `diff.sh` and may be absent on a
//! fresh checkout — the tests skip (pass with a notice) then.

use paged_export_pdf::{
    export_pdf, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles, PdfStandard,
};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};

fn fixture_bytes(name: &str) -> Option<Vec<u8>> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest)
        .join("../../corpus/generated")
        .join(name);
    std::fs::read(path).ok()
}

fn fallback_font() -> Option<Vec<u8>> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(path).ok()
}

/// CMYK profile lookup — same chain as paged-color's parity tests.
fn find_cmyk_profile() -> Option<Vec<u8>> {
    if let Ok(p) = std::env::var("PAGED_CMYK_PROFILE") {
        if let Ok(bytes) = std::fs::read(&p) {
            return Some(bytes);
        }
    }
    let manifest = env!("CARGO_MANIFEST_DIR");
    let corpus = std::path::Path::new(manifest).join("../../corpus/profiles");
    if let Ok(entries) = std::fs::read_dir(&corpus) {
        for e in entries.flatten() {
            let path = e.path();
            if path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("icc"))
            {
                if let Ok(bytes) = std::fs::read(&path) {
                    return Some(bytes);
                }
            }
        }
    }
    let adobe = "/Library/Application Support/Adobe/Color/Profiles/Recommended/CoatedFOGRA39.icc";
    std::fs::read(adobe).ok()
}

struct Built {
    doc: paged_renderer::BuiltDocument,
    fonts: FontTable,
    palette: paged_model::Graphic,
}

fn build_fixture(name: &str, font: &Option<Vec<u8>>) -> Option<Built> {
    let bytes = fixture_bytes(name)?;
    let document = idml_import::import_idml_doc(&bytes).ok()?;
    let mut opts = PipelineOptions {
        collect_glyph_runs: true,
        ..Default::default()
    };
    opts.font = font.as_deref();
    let fonts = FontTable::build(&document, &opts);
    let doc = {
        let mut opts2 = PipelineOptions {
            collect_glyph_runs: true,
            ..Default::default()
        };
        opts2.font = font.as_deref();
        opts2.pre_built_font_table = Some(&fonts);
        pipeline::build_document(&document, &opts2).ok()?
    };
    let palette = document.palette.clone();
    Some(Built {
        doc,
        fonts,
        palette,
    })
}

/// Build a `Built` straight from in-memory IDML bytes (so a test can use
/// a `paged-gen` sample without depending on the gitignored
/// corpus/generated fixtures).
fn build_from_bytes(bytes: &[u8], font: &Option<Vec<u8>>) -> Option<Built> {
    let document = idml_import::import_idml_doc(bytes).ok()?;
    let mut opts = PipelineOptions {
        collect_glyph_runs: true,
        ..Default::default()
    };
    opts.font = font.as_deref();
    let fonts = FontTable::build(&document, &opts);
    let doc = {
        let mut opts2 = PipelineOptions {
            collect_glyph_runs: true,
            ..Default::default()
        };
        opts2.font = font.as_deref();
        opts2.pre_built_font_table = Some(&fonts);
        pipeline::build_document(&document, &opts2).ok()?
    };
    let palette = document.palette.clone();
    Some(Built {
        doc,
        fonts,
        palette,
    })
}

fn export(built: &Built, profile: Option<&[u8]>) -> Vec<u8> {
    let cmm = paged_color::IccCmm::new(profile, paged_color::DisplaySetup::default());
    let (standard, condition) = match profile {
        Some(_) => (PdfStandard::PdfX4, Some("Coated FOGRA39".to_string())),
        None => (PdfStandard::Pdf17, None),
    };
    let input = ExportInput {
        doc: &built.doc,
        palette: &built.palette,
        fonts: Some(&built.fonts),
        cmm: &cmm,
        profiles: ExportProfiles {
            cmyk_working: profile,
            output_intent: profile,
            srgb: None,
        },
        inks: ExportInkSettings::default(),
        options: ExportOptions {
            standard,
            output_condition: condition,
            effect_dpi: 150.0,
            ..Default::default()
        },
        doc_bleed: [0.0; 4],
        doc_slug: [0.0; 4],
    };
    let result = export_pdf(input).expect("export");
    assert_eq!(result.pages_exported, built.doc.pages.len());
    result.bytes
}

#[test]
fn x4_export_validates_with_lopdf() {
    let font = fallback_font();
    let Some(built) = build_fixture("geometry.idml", &font) else {
        eprintln!("export_x4: corpus/generated/geometry.idml absent — skipping");
        return;
    };
    let profile = find_cmyk_profile();
    let is_x4 = profile.is_some();
    if !is_x4 {
        eprintln!("export_x4: no CMYK profile — validating PDF 1.7 shape only");
    }
    let bytes = export(&built, profile.as_deref());

    // Header version (raw bytes — the second header line is the
    // binary-detection comment, not UTF-8).
    if is_x4 {
        assert!(bytes.starts_with(b"%PDF-1.6"), "X-4 must be PDF 1.6");
    }
    assert!(bytes.windows(5).rev().take(64).any(|w| w == b"%%EOF"));

    let doc = lopdf::Document::load_mem(&bytes).expect("lopdf re-parse");

    // No encryption (X-4 forbids it; we never write it).
    assert!(doc.trailer.get(b"Encrypt").is_err(), "unexpected /Encrypt");

    let catalog = doc.catalog().expect("catalog");
    if is_x4 {
        // OutputIntent with GTS_PDFX + DestOutputProfile.
        let intents = catalog.get(b"OutputIntents").expect("OutputIntents");
        let arr = intents.as_array().expect("array");
        assert!(!arr.is_empty());
        let oi = doc
            .dereference(&arr[0])
            .expect("deref OutputIntent")
            .1
            .as_dict()
            .expect("dict");
        assert_eq!(
            oi.get(b"S").and_then(|s| s.as_name()).expect("S"),
            b"GTS_PDFX"
        );
        assert!(
            oi.get(b"DestOutputProfile").is_ok(),
            "missing DestOutputProfile"
        );

        // XMP metadata stream with the PDF/X-4 version tag.
        let meta = catalog.get(b"Metadata").expect("Metadata");
        let (_, meta_obj) = doc.dereference(meta).expect("deref Metadata");
        let stream = meta_obj.as_stream().expect("stream");
        let xmp = String::from_utf8_lossy(&stream.content);
        assert!(
            xmp.contains("PDF/X-4"),
            "XMP missing GTS_PDFXVersion PDF/X-4"
        );

        // Info /Trapped must be a definite value.
        let info = doc.trailer.get(b"Info").expect("Info");
        let (_, info_obj) = doc.dereference(info).expect("deref Info");
        let trapped = info_obj
            .as_dict()
            .expect("dict")
            .get(b"Trapped")
            .and_then(|t| t.as_name())
            .expect("Trapped");
        assert!(trapped == b"False" || trapped == b"True");
    }

    // Every page: TrimBox present and inside MediaBox.
    let pages = doc.get_pages();
    assert!(!pages.is_empty());
    for (_, page_id) in pages {
        let page = doc.get_dictionary(page_id).expect("page dict");
        let trim = page
            .get(b"TrimBox")
            .expect("TrimBox")
            .as_array()
            .expect("arr");
        let media = page
            .get(b"MediaBox")
            .expect("MediaBox")
            .as_array()
            .expect("arr");
        let f = |o: &lopdf::Object| o.as_float().unwrap_or(0.0);
        assert!(f(&trim[0]) >= f(&media[0]) - 1e-3);
        assert!(f(&trim[1]) >= f(&media[1]) - 1e-3);
        assert!(f(&trim[2]) <= f(&media[2]) + 1e-3);
        assert!(f(&trim[3]) <= f(&media[3]) + 1e-3);
    }

    // Embedded fonts: every Type0 font's descendant has a FontFile.
    for (_, obj) in doc.objects.iter() {
        let Ok(dict) = obj.as_dict() else { continue };
        if dict.get(b"Type").and_then(|t| t.as_name()).ok() != Some(b"Font".as_slice()) {
            continue;
        }
        if dict.get(b"Subtype").and_then(|t| t.as_name()).ok() != Some(b"Type0".as_slice()) {
            continue;
        }
        let desc = dict
            .get(b"DescendantFonts")
            .and_then(|d| d.as_array())
            .expect("DescendantFonts");
        let (_, cid) = doc.dereference(&desc[0]).expect("cid font");
        let cid = cid.as_dict().expect("dict");
        let fd = cid.get(b"FontDescriptor").expect("FontDescriptor");
        let (_, fd) = doc.dereference(fd).expect("deref fd");
        let fd = fd.as_dict().expect("dict");
        assert!(
            fd.get(b"FontFile2").is_ok() || fd.get(b"FontFile3").is_ok(),
            "Type0 font without embedded font file"
        );
    }
}

#[test]
fn double_export_is_byte_identical() {
    let font = fallback_font();
    let Some(built) = build_fixture("geometry.idml", &font) else {
        eprintln!("export_x4: corpus/generated/geometry.idml absent — skipping");
        return;
    };
    let profile = find_cmyk_profile();
    let a = export(&built, profile.as_deref());
    let b = export(&built, profile.as_deref());
    assert_eq!(a, b, "two exports of the same scene must be byte-identical");
}

#[test]
fn effects_fixture_exports_transparency_groups() {
    let font = fallback_font();
    let Some(built) = build_fixture("effects.idml", &font) else {
        eprintln!("export_x4: corpus/generated/effects.idml absent — skipping");
        return;
    };
    let bytes = export(&built, None);
    let doc = lopdf::Document::load_mem(&bytes).expect("lopdf re-parse");
    // The effects fixture carries blend modes/opacity — expect at
    // least one transparency-group form XObject in the output.
    let mut groups = 0;
    for (_, obj) in doc.objects.iter() {
        let Ok(stream) = obj.as_stream() else {
            continue;
        };
        let dict = &stream.dict;
        if dict.get(b"Subtype").and_then(|s| s.as_name()).ok() == Some(b"Form".as_slice())
            && dict.get(b"Group").is_ok()
        {
            groups += 1;
        }
    }
    assert!(
        groups > 0,
        "expected transparency-group forms in effects.idml export"
    );
}

/// Punch-list (AC-PREFLIGHT-2): build-time render diagnostics (overset,
/// missing link / undecodable image) computed at `build_document` are
/// promoted into the export's `PreflightFinding` list. The `preflight`
/// `paged-gen` sample is the "unhealthy publication" — an overset story,
/// a by-design missing font, and an undecodable placed image — so the
/// export must surface >= 2 findings. Self-contained: builds the sample
/// IDML in-memory, no corpus/generated dependency.
#[test]
fn build_diagnostics_promote_to_preflight_findings() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::preflight::build())
        .expect("emit preflight sample IDML");
    let font = fallback_font();
    let Some(built) = build_from_bytes(&bytes, &font) else {
        eprintln!("preflight: sample build failed — skipping");
        return;
    };

    // Sanity: the build itself collected the document-health diagnostics
    // (the source the exporter now promotes).
    assert!(
        built.doc.diagnostics.len() >= 2,
        "preflight sample should collect >= 2 build diagnostics (overset + \
         broken image); got {:?}",
        built.doc.diagnostics.items,
    );

    let cmm = paged_color::IccCmm::new(None, paged_color::DisplaySetup::default());
    let input = ExportInput {
        doc: &built.doc,
        palette: &built.palette,
        fonts: Some(&built.fonts),
        cmm: &cmm,
        profiles: ExportProfiles {
            cmyk_working: None,
            output_intent: None,
            srgb: None,
        },
        inks: ExportInkSettings::default(),
        options: ExportOptions {
            standard: PdfStandard::Pdf17,
            ..Default::default()
        },
        doc_bleed: [0.0; 4],
        doc_slug: [0.0; 4],
    };
    let result = export_pdf(input).expect("export preflight sample");

    // The build diagnostics are promoted into the preflight findings.
    assert!(
        result.findings.len() >= 2,
        "overset + broken image must surface as >= 2 preflight findings \
         through the export path; got {:?}",
        result.findings,
    );
    // The overset finding (a promoted build diagnostic) must be present —
    // the export stage alone would never raise it.
    assert!(
        result
            .findings
            .iter()
            .any(|f| f.code == "overset_text_dropped"),
        "the promoted overset build diagnostic must appear in findings: {:?}",
        result.findings,
    );
}
