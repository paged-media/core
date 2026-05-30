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

//! `paged-diff`: compare two PNGs and report ΔE2000 + SSIM.
//!
//! Exit status is 0 on pass (idea.md §13.2 thresholds), 1 on fail.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use paged_fidelity::diff;

#[derive(Parser, Debug)]
#[command(name = "paged-diff", version, about)]
struct Args {
    /// Reference PNG (rasterised InDesign PDF).
    reference: PathBuf,
    /// Candidate PNG (renderer output).
    candidate: PathBuf,
    /// Optional heatmap output path.
    #[arg(long)]
    heatmap: Option<PathBuf>,
    /// Emit report as JSON to stdout (machine-readable CI output).
    #[arg(long)]
    json: bool,
    /// ΔE value mapped to peak heatmap intensity.
    #[arg(long, default_value_t = 5.0)]
    heatmap_scale: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let (report, deltas) = diff::compare_pngs(&args.reference, &args.candidate)?;

    if let Some(path) = args.heatmap.as_deref() {
        diff::heatmap(
            report.width,
            report.height,
            &deltas,
            args.heatmap_scale,
            path,
        )?;
    }

    if args.json {
        println!(
            "{{\"mean_de\":{:.6},\"p99_de\":{:.6},\"max_de\":{:.6},\"ssim\":{:.6},\"passes\":{}}}",
            report.mean_delta_e,
            report.p99_delta_e,
            report.max_delta_e,
            report.ssim,
            report.passes(),
        );
    } else {
        let verdict = if report.passes() { "PASS" } else { "FAIL" };
        eprintln!(
            "{verdict}  mean ΔE={:.3}  p99 ΔE={:.3}  max ΔE={:.3}  SSIM={:.4}  ({}×{})",
            report.mean_delta_e,
            report.p99_delta_e,
            report.max_delta_e,
            report.ssim,
            report.width,
            report.height,
        );
    }

    if report.passes() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}
