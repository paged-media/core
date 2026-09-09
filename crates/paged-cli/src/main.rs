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

//! `paged` — the engine on the command line.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use paged_cli::export::PdfOptions;
use paged_cli::options::DocumentOptions;
use paged_cli::script::ScriptOptions;

#[derive(Parser)]
#[command(
    name = "paged",
    version,
    about = "Open, author, render, export and verify paged documents.",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Rasterise a page to PNG.
    Render {
        /// IDML or `.paged` document.
        doc: PathBuf,
        /// Page to render: a 1-based number or a page id. Default: 1.
        #[arg(long)]
        page: Option<String>,
        /// Render every page; `-o` must then be a directory.
        #[arg(long)]
        all: bool,
        /// Resolution. 72 makes one pixel one point.
        #[arg(long, default_value_t = 144.0)]
        dpi: f32,
        /// Output PNG (or directory, with `--all`).
        #[arg(short, long)]
        out: PathBuf,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Report what the engine made of a document.
    Inspect {
        /// IDML or `.paged` document.
        doc: PathBuf,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Write the document out as IDML, `.paged`, or PDF.
    Export {
        /// IDML or `.paged` document.
        doc: PathBuf,
        /// idml | paged | pdf
        #[arg(long, default_value = "pdf")]
        format: String,
        /// Output file.
        #[arg(short, long)]
        out: PathBuf,
        #[command(flatten)]
        pdf: PdfOptions,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Author a document by running a `.js` file against it.
    Script {
        /// IDML or `.paged` document to author into.
        doc: PathBuf,
        /// The script.
        script: PathBuf,
        #[command(flatten)]
        opts: ScriptOptions,
        #[command(flatten)]
        pdf: PdfOptions,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Mint a blank document.
    New {
        /// letter | legal | tabloid | a3 | a4 | a5, or WxH in points.
        #[arg(long, default_value = "letter")]
        size: String,
        /// idml | paged
        #[arg(long, default_value = "paged")]
        format: String,
        /// Output file.
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Emit the built-in corpus fixtures.
    Gen {
        #[command(subcommand)]
        what: GenCommand,
    },
    /// Compare two PNGs — ΔE2000 and SSIM. Exits 1 when they differ
    /// beyond the budget.
    Diff {
        /// The reference, first — the order is load-bearing.
        reference: PathBuf,
        /// The candidate.
        candidate: PathBuf,
        #[arg(long)]
        json: bool,
        /// Also write a ΔE heatmap PNG here.
        #[arg(long)]
        heatmap: Option<PathBuf>,
        /// ΔE mapped to peak heatmap intensity.
        #[arg(long, default_value_t = 5.0)]
        heatmap_scale: f64,
    },
    /// Ask the engine one of its diagnostic questions.
    ///
    /// Seventeen wire reads had no verb here; every one of them now
    /// prints the engine's own reply envelope, the same shape the
    /// NDJSON session emits and the editor receives.
    Read {
        #[command(subcommand)]
        what: paged_cli::read::ReadCommand,
    },
    /// List, read and write a `.paged` container's content parts.
    Parts {
        #[command(subcommand)]
        what: paged_cli::parts::PartsCommand,
    },
    /// Print the capability catalog — what this engine can be asked to do.
    Describe {
        /// One JSON line instead of an indented block.
        #[arg(long)]
        compact: bool,
    },
    /// Print the document's verification digests.
    Digest {
        /// IDML or `.paged` document.
        doc: PathBuf,
        /// One JSON line instead of an indented block.
        #[arg(long)]
        compact: bool,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Speak the headless NDJSON engine protocol on stdin/stdout.
    ///
    /// Byte-for-byte the `paged-run` protocol, from the same code: a
    /// greeting line, then one JSON response per JSON request, in
    /// order. Hosts that already spawn `paged-run` need not change.
    Session,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Render {
            doc,
            page,
            all,
            dpi,
            out,
            assets,
        } => paged_cli::render::run(&doc, &assets, page, all, dpi, &out),
        Command::Inspect { doc, json, assets } => paged_cli::inspect::run(&doc, &assets, json),
        Command::Export {
            doc,
            format,
            out,
            pdf,
            assets,
        } => paged_cli::export::run(&doc, &assets, &format, &pdf, &out),
        Command::Script {
            doc,
            script,
            opts,
            pdf,
            assets,
        } => paged_cli::script::run(&doc, &assets, &script, &opts, &pdf),
        Command::New { size, format, out } => paged_cli::new::run(&size, &out, &format),
        Command::Gen { what } => match what {
            GenCommand::Emit { sample, out } => paged_cli::gen::emit(&sample, &out),
            GenCommand::EmitAll { out } => paged_cli::gen::emit_all(&out),
        },
        Command::Diff {
            reference,
            candidate,
            json,
            heatmap,
            heatmap_scale,
        } => {
            // Exit code, not a panic: a diff that fails its budget is
            // an answer, and `make`/CI reads it as one.
            if paged_cli::diff::run(
                &reference,
                &candidate,
                json,
                heatmap.as_deref(),
                heatmap_scale,
            )? {
                Ok(())
            } else {
                std::process::exit(1)
            }
        }
        Command::Read { what } => paged_cli::read::run(&what),
        Command::Parts { what } => paged_cli::parts::run(&what),
        Command::Describe { compact } => paged_cli::inspect::describe(compact),
        Command::Digest {
            doc,
            compact,
            assets,
        } => paged_cli::inspect::digest(&doc, &assets, compact),
        Command::Session => paged_cli::session::run(),
    }
}

#[derive(Debug, clap::Subcommand)]
enum GenCommand {
    /// Emit one built-in fixture.
    Emit {
        /// Fixture name; an unknown one lists them all.
        #[arg(long)]
        sample: String,
        /// Output directory; the file lands at `<out>/<sample>.idml`.
        #[arg(long, default_value = "corpus/generated")]
        out: PathBuf,
    },
    /// Emit every built-in fixture. Prefer this over a copied name list.
    EmitAll {
        #[arg(long, default_value = "corpus/generated")]
        out: PathBuf,
    },
}
