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

//! `paged-gen` — emit a generated IDML mega-file to disk.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "paged-gen", version, about)]
struct Args {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Emit one of the built-in mega-files into `--out`.
    Emit {
        /// Mega-file name. Phase 0 only ships `geometry`.
        #[arg(long)]
        sample: String,
        /// Output directory. The `.idml` lands at `<out>/<sample>.idml`.
        #[arg(long, default_value = "corpus/generated")]
        out: PathBuf,
    },
    /// Emit EVERY built-in sample into `--out`.
    ///
    /// Consumers should prefer this over a hand-copied name list. Four
    /// copies of that list existed and drifted; `layers-z` and
    /// `paste-into` fell out of the editor's, and its layers specs then
    /// failed on a fixture CI had never emitted.
    EmitAll {
        /// Output directory.
        #[arg(long, default_value = "corpus/generated")]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
        Cmd::Emit { sample, out } => emit_sample(&sample, &out),
        Cmd::EmitAll { out } => {
            for name in paged_gen::samples::SAMPLES {
                emit_sample(name, &out)?;
            }
            eprintln!(
                "emitted {} samples into {}",
                paged_gen::samples::SAMPLES.len(),
                out.display()
            );
            Ok(())
        }
    }
}

fn emit_sample(name: &str, out_dir: &std::path::Path) -> Result<()> {
    let sample = paged_gen::samples::build(name).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown sample {name:?}; known: {}",
            paged_gen::samples::SAMPLES.join(", ")
        )
    })?;
    let bytes = paged_gen::write_idml(&sample).context("write idml")?;
    std::fs::create_dir_all(out_dir).with_context(|| format!("mkdir {}", out_dir.display()))?;
    let path = out_dir.join(format!("{name}.idml"));
    std::fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
    eprintln!(
        "wrote {} ({} bytes, {} pages)",
        path.display(),
        bytes.len(),
        sample.spreads.len()
    );
    Ok(())
}
