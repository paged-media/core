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
use paged_cli::options::DocumentOptions;

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
        Command::Session => paged_cli::session::run(),
    }
}
