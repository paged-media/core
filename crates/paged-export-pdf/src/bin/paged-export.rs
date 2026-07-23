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

//! `paged-export` — IDML → print PDF from the command line.
//!
//! Lives here (not on `paged-inspect`) because the exporter depends
//! on `paged-renderer`; the inspect binary can't link back without a
//! cycle. Build with `cargo run -p paged-export-pdf --features cli
//! --bin paged-export -- <idml> <out.pdf> [--pdfx4 --profile p.icc]`.

use std::path::PathBuf;

use clap::Parser;
use paged_export_pdf::{
    export_pdf, ExportColorPolicy, ExportInkSettings, ExportInput, ExportOptions, ExportProfiles,
    MarkOptions, PdfStandard,
};
use paged_renderer::pipeline::{self, FontTable, PipelineOptions};
use paged_scene::Document;

#[derive(Parser, Debug)]
#[command(
    name = "paged-export",
    version,
    about = "Export an IDML document to print PDF"
)]
struct Args {
    /// IDML file to export.
    file: PathBuf,
    /// Output PDF path.
    out: PathBuf,
    /// Target PDF/X-4 (requires --profile for the output intent).
    #[arg(long)]
    pdfx4: bool,
    /// CMYK ICC profile: the working space AND the X-4 output intent.
    #[arg(long, value_name = "PATH")]
    profile: Option<PathBuf>,
    /// Output condition name for the OutputIntent dict.
    #[arg(long, default_value = "Custom")]
    output_condition: String,
    /// Convert RGB/Lab content to the destination CMYK (default:
    /// preserve numbers).
    #[arg(long)]
    convert_to_destination: bool,
    /// Draw crop marks + registration + colour bars.
    #[arg(long)]
    marks: bool,
    /// Bleed override in pt as top,left,bottom,right (default: the
    /// document's declared bleed).
    #[arg(long, value_name = "T,L,B,R")]
    bleed: Option<String>,
    /// Fallback TTF/OTF for shaping (any unresolved family).
    #[arg(long)]
    font: Option<PathBuf>,
    /// 0-based inclusive page range, e.g. 0-3.
    #[arg(long, value_name = "FROM-TO")]
    pages: Option<String>,
    /// Resample images above this effective ppi.
    #[arg(long)]
    downsample_ppi: Option<f32>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let bytes = std::fs::read(&args.file)?;
    let document = paged_parse::import_idml_doc(&bytes)?;
    let font = args.font.as_ref().map(std::fs::read).transpose()?;
    let profile = args.profile.as_ref().map(std::fs::read).transpose()?;
    if args.pdfx4 && profile.is_none() {
        anyhow::bail!("--pdfx4 requires --profile (the output intent)");
    }

    let mut opts = PipelineOptions {
        collect_glyph_runs: true,
        ..Default::default()
    };
    opts.font = font.as_deref();
    opts.cmyk_icc_profile = profile.as_deref();
    let fonts = FontTable::build(&document, &opts);
    opts.pre_built_font_table = Some(&fonts);
    let built = pipeline::build_document(&document, &opts)?;

    let policy = if args.convert_to_destination {
        ExportColorPolicy::ConvertToDestination
    } else {
        ExportColorPolicy::PreserveNumbers
    };
    let mut cmm =
        paged_color::IccCmm::new(profile.as_deref(), paged_color::DisplaySetup::default());
    cmm.configure_export(profile.as_deref(), policy.into());

    let bleed_override = match &args.bleed {
        Some(s) => {
            let v: Vec<f32> = s
                .split(',')
                .map(|p| p.trim().parse::<f32>())
                .collect::<Result<_, _>>()
                .map_err(|e| anyhow::anyhow!("bad --bleed: {e}"))?;
            anyhow::ensure!(v.len() == 4, "--bleed wants 4 comma-separated values");
            Some([v[0], v[1], v[2], v[3]])
        }
        None => None,
    };
    let page_range = match &args.pages {
        Some(s) => {
            let (from, to) = s
                .split_once('-')
                .ok_or_else(|| anyhow::anyhow!("--pages wants FROM-TO"))?;
            Some((from.trim().parse::<usize>()?, to.trim().parse::<usize>()?))
        }
        None => None,
    };

    let pref = document.designmap.document_preference;
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
            standard: if args.pdfx4 {
                PdfStandard::PdfX4
            } else {
                PdfStandard::Pdf17
            },
            color_policy: policy,
            output_condition: Some(args.output_condition.clone()),
            page_range,
            marks: MarkOptions {
                crop_marks: args.marks,
                registration_marks: args.marks,
                color_bars: args.marks,
                page_info: false,
                offset_pt: 0.0,
                weight_pt: 0.0,
            },
            bleed: paged_export_pdf::BleedOptions {
                override_pt: bleed_override,
            },
            images: paged_export_pdf::ImageOptions {
                downsample_ppi: args.downsample_ppi,
                jpeg_quality: None,
            },
            effect_dpi: 150.0,
            ..Default::default()
        },
        doc_bleed: [
            pref.bleed_top,
            pref.bleed_inside_or_left,
            pref.bleed_bottom,
            pref.bleed_outside_or_right,
        ],
        doc_slug: [
            pref.slug_top,
            pref.slug_inside_or_left,
            pref.slug_bottom,
            pref.slug_right_or_outside,
        ],
    })?;

    std::fs::write(&args.out, &result.bytes)?;
    eprintln!(
        "{}: {} pages, {} bytes{}",
        args.out.display(),
        result.pages_exported,
        result.bytes.len(),
        if result.diagnostics.is_empty() {
            String::new()
        } else {
            format!(
                ", {} diagnostic(s): {:?}",
                result.diagnostics.len(),
                result.diagnostics
            )
        }
    );
    Ok(())
}
