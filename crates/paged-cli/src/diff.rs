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

//! `paged diff` — ΔE2000 + SSIM between two PNGs.
//!
//! The same comparison `paged-diff` makes, in the same argument order
//! (reference first, candidate second — getting it backwards inverts
//! nothing in the numbers but everything in what they mean), so a
//! script can move between the two without re-learning it.

use std::path::Path;

use anyhow::{Context, Result};

pub fn run(
    reference: &Path,
    candidate: &Path,
    as_json: bool,
    heatmap: Option<&Path>,
    heatmap_scale: f64,
) -> Result<bool> {
    let (report, deltas) =
        paged_fidelity::diff::compare_pngs(reference, candidate).with_context(|| {
            format!(
                "compare {} with {}",
                reference.display(),
                candidate.display()
            )
        })?;
    if let Some(path) = heatmap {
        paged_fidelity::diff::heatmap(report.width, report.height, &deltas, heatmap_scale, path)
            .with_context(|| format!("write heatmap {}", path.display()))?;
    }
    if as_json {
        // The same object shape `paged-diff --json` emits, so a script
        // that parses one parses the other.
        println!(
            "{{\"mean_de\":{:.6},\"p99_de\":{:.6},\"max_de\":{:.6},\"ssim\":{:.6},\"passes\":{}}}",
            report.mean_delta_e,
            report.p99_delta_e,
            report.max_delta_e,
            report.ssim,
            report.passes()
        );
    } else {
        println!(
            "{}  mean ΔE={:.3}  p99 ΔE={:.3}  max ΔE={:.3}  SSIM={:.4}  ({}×{})",
            if report.passes() { "PASS" } else { "FAIL" },
            report.mean_delta_e,
            report.p99_delta_e,
            report.max_delta_e,
            report.ssim,
            report.width,
            report.height
        );
    }
    Ok(report.passes())
}
