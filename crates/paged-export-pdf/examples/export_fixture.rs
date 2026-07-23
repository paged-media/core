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

//! Dev convenience: export a corpus fixture to a PDF on disk.
//! `cargo run -p paged-export-pdf --example export_fixture -- <idml> <out.pdf> [profile.icc]`
//! (The user-facing CLI lands on paged-inspect in M8.)

use paged_export_pdf::{
    export_pdf, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles, PdfStandard,
};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let idml = args
        .next()
        .expect("usage: export_fixture <idml> <out.pdf> [profile.icc]");
    let out = args
        .next()
        .expect("usage: export_fixture <idml> <out.pdf> [profile.icc]");
    let profile = args.next().map(std::fs::read).transpose()?;

    let bytes = std::fs::read(&idml)?;
    let document = idml_import::import_idml_doc(&bytes)?;
    let font = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf"),
    )
    .ok();
    let mut opts = PipelineOptions {
        collect_glyph_runs: true,
        ..Default::default()
    };
    opts.font = font.as_deref();
    let fonts = FontTable::build(&document, &opts);
    opts.pre_built_font_table = Some(&fonts);
    let built = pipeline::build_document(&document, &opts)?;

    let cmm = paged_color::IccCmm::new(profile.as_deref(), paged_color::DisplaySetup::default());
    let result = export_pdf(ExportInput {
        doc: &built,
        palette: &document.palette,
        fonts: Some(&fonts),
        cmm: &cmm,
        profiles: ExportProfiles {
            cmyk_working: profile.as_deref(),
            output_intent: profile.as_deref(),
            srgb: None,
        },
        inks: ExportInkSettings::default(),
        options: ExportOptions {
            standard: if profile.is_some() {
                PdfStandard::PdfX4
            } else {
                PdfStandard::Pdf17
            },
            output_condition: profile.as_ref().map(|_| "Coated FOGRA39".to_string()),
            effect_dpi: 150.0,
            ..Default::default()
        },
        doc_bleed: [0.0; 4],
        doc_slug: [0.0; 4],
    })?;
    eprintln!(
        "exported {} pages, {} bytes, {} diagnostics",
        result.pages_exported,
        result.bytes.len(),
        result.diagnostics.len()
    );
    std::fs::write(&out, result.bytes)?;
    Ok(())
}
