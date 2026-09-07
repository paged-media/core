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

//! `paged export` — IDML, `.paged`, or PDF, from the editor's own
//! exporters.
//!
//! PDF goes through the same begin/page/finish session the editor's
//! Export dialog drives, so the CLI and the dialog produce the same
//! file — and, unlike `paged-export`, it exports the loaded MODEL,
//! which means a `.paged` container's native part and anything a
//! script just authored.

use std::path::Path;

use anyhow::{bail, Context, Result};
use paged_canvas::channel::{ExportPdfWireOptions, MainToWorkerKind, WorkerToMainKind};

use crate::engine::Session;
use crate::expect_reply;
use crate::options::DocumentOptions;

/// PDF knobs. Everything else on `ExportPdfWireOptions` keeps its
/// default, which is what the Export dialog sends when its own fields
/// are untouched.
#[derive(Debug, Clone, clap::Args)]
pub struct PdfOptions {
    /// "pdf17" (default) or "pdfx4". X-4 needs a resolvable output
    /// intent — give one here or let the document's profile serve.
    #[arg(long, default_value = "pdf17")]
    pub standard: String,
    /// Output-intent profile NAME, resolved against what this run
    /// registered. Defaults to the working space.
    #[arg(long)]
    pub output_intent: Option<String>,
    /// Human-readable output condition for the OutputIntent dict.
    #[arg(long)]
    pub output_condition: Option<String>,
    /// Crop marks, registration marks, colour bars, page info.
    #[arg(long)]
    pub marks: bool,
    /// 0-based inclusive page range, e.g. `0-9`.
    #[arg(long, value_name = "FROM-TO")]
    pub pages: Option<String>,
}

pub fn run(
    doc: &Path,
    opts: &DocumentOptions,
    format: &str,
    pdf: &PdfOptions,
    out: &Path,
) -> Result<()> {
    let mut session = Session::new();
    opts.open(&mut session, doc)?;
    let bytes = export_bytes(&mut session, format, pdf)?;
    std::fs::write(out, &bytes).with_context(|| format!("write {}", out.display()))?;
    println!("{} — {format}, {} bytes", out.display(), bytes.len());
    Ok(())
}

/// The format a path names, so `-o out.pdf` needs no second flag.
pub fn format_for(path: &Path) -> Result<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("pdf") => Ok("pdf"),
        Some("idml") => Ok("idml"),
        Some("paged") => Ok("paged"),
        _ => bail!(
            "cannot tell the format of {} — name it .pdf, .idml or .paged",
            path.display()
        ),
    }
}

/// Export the session's CURRENT model — which is the point: after
/// `paged script` has authored into it, this writes what the script
/// made, not what was loaded.
pub fn export_bytes(session: &mut Session, format: &str, pdf: &PdfOptions) -> Result<Vec<u8>> {
    let bytes = match format {
        "idml" => {
            let reply = session.send(MainToWorkerKind::ExportIdml { link_base: None })?;
            let (bytes, lost) = expect_reply!(reply,
                WorkerToMainKind::IdmlExported { idml_bytes, lost, .. } => (idml_bytes, lost),
                "export idml")?;
            // The loss ledger is the export's own honesty about what
            // IDML could not carry. Printing it is the difference
            // between a silent downgrade and an informed one.
            for line in &lost {
                eprintln!("lost in translation to IDML: {line}");
            }
            bytes.into_vec()
        }
        "paged" => {
            let reply = session.send(MainToWorkerKind::ExportPaged {})?;
            expect_reply!(reply, WorkerToMainKind::PagedExported { bytes } => bytes.into_vec(),
                "export paged")?
        }
        "pdf" => export_pdf(session, pdf)?,
        other => bail!("unsupported format {other:?} (idml|paged|pdf)"),
    };
    Ok(bytes)
}

fn export_pdf(session: &mut Session, pdf: &PdfOptions) -> Result<Vec<u8>> {
    let (page_from, page_to) = match &pdf.pages {
        Some(spec) => {
            let (a, b) = spec
                .split_once('-')
                .with_context(|| format!("--pages wants FROM-TO, got {spec:?}"))?;
            (Some(a.trim().parse()?), Some(b.trim().parse()?))
        }
        None => (None, None),
    };
    let options = ExportPdfWireOptions {
        standard: Some(pdf.standard.clone()),
        output_intent_profile: pdf.output_intent.clone(),
        output_condition: pdf.output_condition.clone(),
        page_from,
        page_to,
        crop_marks: pdf.marks,
        registration_marks: pdf.marks,
        color_bars: pdf.marks,
        page_info: pdf.marks,
        ..Default::default()
    };

    let reply = session.send(MainToWorkerKind::ExportPdfBegin { options })?;
    let (id, total) = expect_reply!(reply,
        WorkerToMainKind::ExportPdfBegun { session, page_count } => (session, page_count),
        "begin pdf export")?;

    // The editor pumps this a page at a time so its progress bar can
    // move; a CLI pumps it to the end, but through the same session so
    // a page that poisons the writer fails here exactly as it does
    // there.
    for _ in 0..total {
        let reply = session.send(MainToWorkerKind::ExportPdfPage { session: id })?;
        expect_reply!(reply, WorkerToMainKind::ExportPdfProgress { .. } => (), "export pdf page")?;
    }

    let reply = session.send(MainToWorkerKind::ExportPdfFinish { session: id })?;
    let (bytes, diagnostics, findings) = expect_reply!(reply,
        WorkerToMainKind::PdfExported { pdf_bytes, diagnostics, findings } =>
            (pdf_bytes, diagnostics, findings),
        "finish pdf export")?;
    for line in &diagnostics {
        eprintln!("pdf: {line}");
    }
    if !findings.is_empty() {
        eprintln!("pdf: {} preflight finding(s)", findings.len());
    }
    Ok(bytes.into_vec())
}
