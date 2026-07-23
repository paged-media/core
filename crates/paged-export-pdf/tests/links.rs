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

//! W1.4 — PDF Link annotations. Builds the `markers` gen sample (which
//! always exists — generated in-process, not from the optional corpus)
//! with `collect_link_regions`, exports it, and re-parses the bytes to
//! confirm the page carries `/Annots` Link annotations with a `/URI`
//! action and a `/GoTo` action.

use paged_export_pdf::{
    export_pdf, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles, PdfStandard,
};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};

fn fallback_font() -> Vec<u8> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(path).expect("Inter.ttf fixture")
}

fn export_markers_pdf() -> Vec<u8> {
    let sample = paged_gen::samples::markers::build();
    let bytes = paged_gen::write_idml(&sample).unwrap();
    let document = paged_parse::import_idml_doc(&bytes).unwrap();
    let font = fallback_font();

    let mut opts = PipelineOptions {
        collect_glyph_runs: true,
        collect_link_regions: true,
        ..Default::default()
    };
    opts.font = Some(&font);
    let fonts = FontTable::build(&document, &opts);
    opts.pre_built_font_table = Some(&fonts);
    let doc = pipeline::build_document(&document, &opts).unwrap();
    let palette = document.palette.clone();

    let cmm = paged_color::IccCmm::new(None, paged_color::DisplaySetup::default());
    let input = ExportInput {
        doc: &doc,
        palette: &palette,
        fonts: Some(&fonts),
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
    export_pdf(input).expect("export").bytes
}

#[test]
fn markers_export_carries_link_annotations() {
    let bytes = export_markers_pdf();
    let doc = lopdf::Document::load_mem(&bytes).expect("lopdf re-parse");

    // Collect every Link annotation across all pages.
    let mut uri_actions = 0;
    let mut goto_actions = 0;
    let mut link_annots = 0;
    let mut pages_with_annots = 0;

    for (_, page_id) in doc.get_pages() {
        let page = doc.get_dictionary(page_id).expect("page dict");
        let Ok(annots) = page.get(b"Annots") else {
            continue;
        };
        let arr = annots.as_array().expect("Annots array");
        if !arr.is_empty() {
            pages_with_annots += 1;
        }
        for a in arr {
            let (_, annot) = doc.dereference(a).expect("deref annot");
            let annot = annot.as_dict().expect("annot dict");
            // Subtype must be /Link.
            assert_eq!(
                annot
                    .get(b"Subtype")
                    .and_then(|s| s.as_name())
                    .expect("Subtype"),
                b"Link",
                "annotation must be a Link"
            );
            link_annots += 1;
            let action = annot.get(b"A").expect("Link action /A");
            let (_, action) = doc.dereference(action).expect("deref action");
            let action = action.as_dict().expect("action dict");
            match action
                .get(b"S")
                .and_then(|s| s.as_name())
                .expect("action /S")
            {
                b"URI" => {
                    let uri = action.get(b"URI").and_then(|u| u.as_str()).expect("/URI");
                    assert_eq!(uri, b"https://paged.media", "URI target");
                    uri_actions += 1;
                }
                b"GoTo" => {
                    // /D destination must reference a page object + /Fit.
                    let dest = action.get(b"D").expect("GoTo /D");
                    let dest = dest.as_array().expect("/D array");
                    // [pageRef /Fit] — first item is the target page ref.
                    assert!(
                        matches!(dest.first(), Some(lopdf::Object::Reference(_))),
                        "GoTo destination must point at a page ref"
                    );
                    goto_actions += 1;
                }
                other => panic!("unexpected action type {other:?}"),
            }
        }
    }

    assert_eq!(
        pages_with_annots, 1,
        "only the body page carries annotations"
    );
    assert_eq!(link_annots, 2, "two Link annotations (URL + page jump)");
    assert_eq!(uri_actions, 1, "one /URI action");
    assert_eq!(goto_actions, 1, "one /GoTo action");
}

#[test]
fn markers_export_is_byte_deterministic() {
    let a = export_markers_pdf();
    let b = export_markers_pdf();
    assert_eq!(
        a, b,
        "two exports of the markers scene must be byte-identical"
    );
}
