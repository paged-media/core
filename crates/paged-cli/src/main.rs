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
//!
//! The command tree itself lives in `paged_cli::cli`, so it can be
//! WALKED as well as parsed — the docs site's command reference is
//! generated from it (`crates/paged-cli/cli.json`). This file is the
//! dispatch and nothing else.

use anyhow::Result;
use clap::Parser;
use paged_cli::cli::{Cli, Command, GenCommand};

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
        Command::Place {
            doc,
            frame,
            image,
            fit,
            transform,
            out,
            pdf,
            assets,
        } => paged_cli::place::run(
            &doc,
            &assets,
            &frame,
            &image,
            fit.as_deref(),
            transform.as_deref(),
            out.as_ref(),
            &pdf,
        ),
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
