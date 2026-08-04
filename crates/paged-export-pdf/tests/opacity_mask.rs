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

//! C-28 — opacity masks survive PDF export as NATIVE soft masks.
//!
//! The display-list bracket lowers onto the same
//! `PendingFormGroup::LuminosityGray` machinery gradient feather has
//! always used (its only producer before C-28): the mask artwork
//! becomes an isolated group Form XObject, an ExtGState's `/SMask`
//! points at it, and the masked content paints under that gs. These
//! tests re-parse the emitted bytes and pin exactly that — no raster
//! fallback, no dropped mask.

use paged_export_pdf::{
    export_pdf, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles, PdfStandard,
};
use paged_model::{FrameRef, OpacityMask, OpacityMaskType};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};

fn masked_pdf(mask_type: OpacityMaskType, invert: bool) -> Vec<u8> {
    let bytes = paged_gen::write_idml(&paged_gen::samples::layers_z::build()).unwrap();
    let mut document = idml_import::import_idml_doc(&bytes).unwrap();
    {
        let spread = &mut document.spreads[0].spread;
        let target = spread.rectangles[0].self_id.clone().unwrap();
        let mask = spread.rectangles[1].self_id.clone().unwrap();
        spread.opacity_masks.insert(
            target,
            OpacityMask {
                mask_item: mask,
                mask_type,
                invert,
            },
        );
        spread
            .frames_in_order
            .retain(|r| *r != FrameRef::Rectangle(1));
    }

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
    export_pdf(input).expect("export").bytes
}

/// Every `/SMask` dict in the file, paired with the group Form
/// XObject it names.
fn soft_masks(doc: &lopdf::Document) -> Vec<(lopdf::Dictionary, lopdf::Dictionary)> {
    let mut out = Vec::new();
    for (_, obj) in doc.objects.iter() {
        let Ok(dict) = obj.as_dict() else { continue };
        let Ok(sm) = dict.get(b"SMask") else { continue };
        let Ok((_, sm)) = doc.dereference(sm) else {
            continue;
        };
        let Ok(sm) = sm.as_dict() else { continue };
        let group = sm
            .get(b"G")
            .ok()
            .and_then(|g| doc.dereference(g).ok())
            .and_then(|(_, o)| o.as_stream().ok().map(|s| s.dict.clone()))
            .expect("/SMask /G must resolve to a form XObject");
        out.push((sm.clone(), group));
    }
    out
}

/// A luminosity mask exports as `/S /Luminosity` over an ISOLATED
/// `/DeviceGray` transparency group, with the black backdrop that
/// makes unpainted area mean "hidden".
#[test]
fn a_luminosity_opacity_mask_exports_as_a_native_pdf_soft_mask() {
    let bytes = masked_pdf(OpacityMaskType::Luminosity, false);
    let doc = lopdf::Document::load_mem(&bytes).expect("lopdf re-parse");
    let masks = soft_masks(&doc);
    assert_eq!(masks.len(), 1, "exactly one /SMask expected");
    let (sm, group) = &masks[0];

    assert_eq!(
        sm.get(b"S").and_then(|s| s.as_name()).expect("/S"),
        b"Luminosity"
    );
    assert!(
        sm.get(b"TR").is_err(),
        "no transfer function without `invert`"
    );
    let backdrop = sm
        .get(b"BC")
        .and_then(|b| b.as_array())
        .expect("/BC backdrop");
    assert_eq!(backdrop.len(), 1);

    // The group is a transparency group, isolated, in DeviceGray.
    let g = group
        .get(b"Group")
        .and_then(|g| g.as_dict())
        .expect("/Group");
    assert_eq!(
        g.get(b"S").and_then(|s| s.as_name()).expect("/S"),
        b"Transparency"
    );
    assert!(g.get(b"I").and_then(|i| i.as_bool()).expect("/I isolated"));
    assert_eq!(
        g.get(b"CS").and_then(|c| c.as_name()).expect("/CS"),
        b"DeviceGray"
    );
}

/// An alpha mask exports as `/S /Alpha` over an isolated group with NO
/// blending colour space (irrelevant by definition for an alpha mask).
#[test]
fn an_alpha_opacity_mask_exports_as_an_alpha_soft_mask() {
    let bytes = masked_pdf(OpacityMaskType::Alpha, false);
    let doc = lopdf::Document::load_mem(&bytes).expect("lopdf re-parse");
    let masks = soft_masks(&doc);
    assert_eq!(masks.len(), 1);
    let (sm, group) = &masks[0];
    assert_eq!(
        sm.get(b"S").and_then(|s| s.as_name()).expect("/S"),
        b"Alpha"
    );
    let g = group
        .get(b"Group")
        .and_then(|g| g.as_dict())
        .expect("/Group");
    assert!(g.get(b"I").and_then(|i| i.as_bool()).expect("/I isolated"));
    assert!(
        g.get(b"CS").is_err(),
        "an alpha mask needs no blending colour space"
    );
}

/// `invert` rides PDF's `/TR` transfer function as a Type-2
/// exponential with C0 = [1], C1 = [0], N = 1 — i.e. `1 − x`.
#[test]
fn invert_exports_as_a_one_minus_x_transfer_function() {
    let bytes = masked_pdf(OpacityMaskType::Luminosity, true);
    let doc = lopdf::Document::load_mem(&bytes).expect("lopdf re-parse");
    let masks = soft_masks(&doc);
    assert_eq!(masks.len(), 1);
    let tr = masks[0].0.get(b"TR").expect("/TR present when inverting");
    let (_, tr) = doc.dereference(tr).expect("deref /TR");
    let tr = tr.as_dict().expect("/TR function dict");
    assert_eq!(
        tr.get(b"FunctionType")
            .and_then(|t| t.as_i64())
            .expect("/FunctionType"),
        2
    );
    let num = |k: &[u8]| -> Vec<f32> {
        tr.get(k)
            .and_then(|v| v.as_array())
            .expect("array")
            .iter()
            .map(|v| v.as_float().expect("float"))
            .collect()
    };
    assert_eq!(num(b"C0"), vec![1.0]);
    assert_eq!(num(b"C1"), vec![0.0]);
    assert_eq!(tr.get(b"N").and_then(|n| n.as_float()).expect("/N"), 1.0);
}

/// A document with no mask emits no `/SMask` at all — the machinery
/// stays off the common path.
#[test]
fn an_unmasked_document_emits_no_soft_mask() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::layers_z::build()).unwrap();
    let document = idml_import::import_idml_doc(&bytes).unwrap();
    let opts = PipelineOptions::default();
    let fonts = FontTable::build(&document, &opts);
    let doc_built = pipeline::build_document(&document, &opts).unwrap();
    let palette = document.palette.clone();
    let cmm = paged_color::IccCmm::new(None, paged_color::DisplaySetup::default());
    let bytes = export_pdf(ExportInput {
        doc: &doc_built,
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
    })
    .expect("export")
    .bytes;
    let doc = lopdf::Document::load_mem(&bytes).expect("lopdf re-parse");
    assert!(soft_masks(&doc).is_empty());
}
