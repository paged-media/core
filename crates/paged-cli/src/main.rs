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

use anyhow::Result;
use clap::{Parser, Subcommand};

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
    /// Speak the headless NDJSON engine protocol on stdin/stdout.
    ///
    /// Byte-for-byte the `paged-run` protocol, from the same code: a
    /// greeting line, then one JSON response per JSON request, in
    /// order. Hosts that already spawn `paged-run` need not change.
    Session,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Session => paged_cli::session::run(),
    }
}
