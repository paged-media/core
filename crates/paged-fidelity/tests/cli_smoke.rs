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

//! End-to-end smoke test: generate two PNGs, run the `paged-diff` binary,
//! and verify both identical and divergent cases.

use std::process::Command;

use image::{ImageBuffer, Rgb, RgbImage};
use tempfile::TempDir;

fn checker(offset: u8) -> RgbImage {
    ImageBuffer::from_fn(64, 64, |x, y| {
        let v = if ((x / 8) + (y / 8)) & 1 == 0 {
            50u8.wrapping_add(offset)
        } else {
            200u8.wrapping_add(offset)
        };
        Rgb([v, v.saturating_sub(20), v.saturating_add(20)])
    })
}

fn idml_diff_path() -> std::path::PathBuf {
    // Cargo exposes the binary path via CARGO_BIN_EXE_<name> for integration
    // tests in the same package.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_paged-diff"))
}

#[test]
fn identical_pngs_return_pass() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a.png");
    let b = tmp.path().join("b.png");
    checker(0).save(&a).unwrap();
    checker(0).save(&b).unwrap();

    let status = Command::new(idml_diff_path())
        .arg(&a)
        .arg(&b)
        .status()
        .unwrap();
    assert!(status.success(), "identical images should PASS");
}

#[test]
fn divergent_pngs_return_fail() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a.png");
    let b = tmp.path().join("b.png");
    checker(0).save(&a).unwrap();
    // Large offset → every pixel shifted; mean ΔE well above threshold.
    checker(60).save(&b).unwrap();

    let status = Command::new(idml_diff_path())
        .arg(&a)
        .arg(&b)
        .status()
        .unwrap();
    assert!(!status.success(), "divergent images should FAIL");
}

#[test]
fn heatmap_is_emitted_when_requested() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a.png");
    let b = tmp.path().join("b.png");
    let heat = tmp.path().join("heat.png");
    checker(0).save(&a).unwrap();
    checker(40).save(&b).unwrap();

    let status = Command::new(idml_diff_path())
        .arg(&a)
        .arg(&b)
        .arg("--heatmap")
        .arg(&heat)
        .status()
        .unwrap();
    // Expected to fail the threshold but still emit the heatmap.
    assert!(!status.success());
    assert!(heat.exists(), "heatmap PNG should exist");
    let meta = std::fs::metadata(&heat).unwrap();
    assert!(meta.len() > 0, "heatmap should not be empty");
}
