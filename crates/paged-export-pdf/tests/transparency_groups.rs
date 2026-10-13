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

//! R-10 — the `transparency-targets` fixture exported to PDF: every
//! target's opacity and blend mode reaches an ExtGState of its own, and a
//! `<Group>`'s `IsolateBlending` / `KnockoutGroup` reach its transparency
//! group dictionary as `/I true` / `/K true` — the way InDesign's own
//! PDF composites them.

use paged_export_pdf::{
    export_pdf, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles, PdfStandard,
};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};

fn exported() -> lopdf::Document {
    let bytes = paged_gen::write_idml(&paged_gen::samples::transparency_targets::build()).unwrap();
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let fonts = FontTable::build(&document, &opts);
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
    let pdf = export_pdf(input).expect("export").bytes;
    lopdf::Document::load_mem(&pdf).expect("lopdf re-parse")
}

/// `(isolated, knockout)` of every transparency-group form XObject.
fn group_flags(doc: &lopdf::Document) -> Vec<(bool, bool)> {
    let flag = |g: &lopdf::Dictionary, k: &[u8]| {
        g.get(k)
            .ok()
            .and_then(|v| v.as_bool().ok())
            .unwrap_or(false)
    };
    doc.objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter_map(|s| s.dict.get(b"Group").ok()?.as_dict().ok().cloned())
        .filter(|g| g.get(b"S").ok().and_then(|s| s.as_name().ok()) == Some(b"Transparency"))
        .map(|g| (flag(&g, b"I"), flag(&g, b"K")))
        .collect()
}

/// `(blend mode, fill opacity)` of every ExtGState that sets either.
fn graphic_states(doc: &lopdf::Document) -> Vec<(Vec<u8>, f32)> {
    doc.objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .filter(|d| d.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"ExtGState"))
        .map(|d| {
            let bm = d
                .get(b"BM")
                .ok()
                .and_then(|b| b.as_name().ok())
                .unwrap_or(b"Normal")
                .to_vec();
            let ca = d
                .get(b"ca")
                .ok()
                .and_then(|c| c.as_float().ok())
                .unwrap_or(1.0);
            (bm, ca)
        })
        .collect()
}

#[test]
fn group_flags_reach_the_pdf_group_dictionary() {
    let flags = group_flags(&exported());
    // Cases 42–44 and 46–48: isolated, knockout, both — each present.
    for want in [(true, false), (false, true), (true, true)] {
        assert!(flags.contains(&want), "no group with {want:?} in {flags:?}");
    }
}

#[test]
fn every_target_gets_its_own_blend_and_opacity() {
    let states = graphic_states(&exported());
    for (bm, ca) in [
        (b"Multiply".as_slice(), 0.5),
        (b"Screen".as_slice(), 0.5),
        (b"Normal".as_slice(), 0.5),
    ] {
        assert!(
            states
                .iter()
                .any(|(b, a)| b.as_slice() == bm && (a - ca).abs() < 1e-3),
            "no ExtGState {} at {ca}: {:?}",
            String::from_utf8_lossy(bm),
            states
                .iter()
                .map(|(b, a)| (String::from_utf8_lossy(b).into_owned(), *a))
                .collect::<Vec<_>>()
        );
    }
}
