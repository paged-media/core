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

//! `paged gen` — emit the built-in corpus fixtures.
//!
//! The same two verbs `paged-gen` has, over the same library calls, so
//! the fixture the hard gate measures and the fixture a `paged` user
//! emits are the same bytes. `emit-all` exists because a hand-copied
//! name list drifted four times; prefer it.

use std::path::Path;

use anyhow::{Context, Result};

pub fn emit(name: &str, out_dir: &Path) -> Result<()> {
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
    println!(
        "{} — {} bytes, {} spread(s)",
        path.display(),
        bytes.len(),
        sample.spreads.len()
    );
    Ok(())
}

pub fn emit_all(out_dir: &Path) -> Result<()> {
    for name in paged_gen::samples::SAMPLES {
        emit(name, out_dir)?;
    }
    println!(
        "emitted {} sample(s) into {}",
        paged_gen::samples::SAMPLES.len(),
        out_dir.display()
    );
    Ok(())
}
