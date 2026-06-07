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
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
        Cmd::Emit { sample, out } => emit_sample(&sample, &out),
    }
}

fn emit_sample(name: &str, out_dir: &std::path::Path) -> Result<()> {
    let sample = match name {
        "geometry" => paged_gen::samples::geometry::build(),
        "geometry-groups" => paged_gen::samples::geometry_groups::build(),
        "strokes-fills" => paged_gen::samples::strokes_fills::build(),
        "text" => paged_gen::samples::text::build(),
        "text-advanced" => paged_gen::samples::text_advanced::build(),
        "text-autosize" => paged_gen::samples::text_autosize::build(),
        "text-letterspacing" => paged_gen::samples::text_letterspacing::build(),
        "text-on-path" => paged_gen::samples::text_on_path::build(),
        "text-overset" => paged_gen::samples::text_overset::build(),
        "text-in-shape" => paged_gen::samples::text_in_shape::build(),
        "text-wrap" => paged_gen::samples::text_wrap::build(),
        "effects" => paged_gen::samples::effects::build(),
        "footnotes" => paged_gen::samples::footnotes::build(),
        "gradients" => paged_gen::samples::gradients::build(),
        "tables" => paged_gen::samples::tables::build(),
        "images" => paged_gen::samples::images::build(),
        "image-clipping" => paged_gen::samples::image_clipping::build(),
        "anchored" => paged_gen::samples::anchored::build(),
        "transparency" => paged_gen::samples::transparency::build(),
        "markers" => paged_gen::samples::markers::build(),
        "masters" => paged_gen::samples::masters::build(),
        "corners" => paged_gen::samples::corners::build(),
        "links-broken" => paged_gen::samples::links_broken::build(),
        "numbering" => paged_gen::samples::numbering::build(),
        "variables" => paged_gen::samples::variables::build(),
        "conditions" => paged_gen::samples::conditions::build(),
        "swatches" => paged_gen::samples::swatches::build(),
        other => {
            anyhow::bail!(
                "unknown sample {other:?}; known: geometry, geometry-groups, strokes-fills, text, text-advanced, text-autosize, text-letterspacing, text-on-path, text-overset, text-in-shape, text-wrap, effects, footnotes, gradients, tables, images, image-clipping, anchored, transparency, markers, masters, corners, links-broken, numbering, variables, conditions, swatches, navigation, styles-cascade"
            )
        }
    };
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
