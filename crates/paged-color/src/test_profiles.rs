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

//! ONE way to find a CMYK profile for tests.
//!
//! Before this existed there were three, and they disagreed:
//!
//! | site | looked in | outcome |
//! |---|---|---|
//! | `paged-color/tests/parity.rs` | env → `corpus/profiles` → Adobe's install path | passed on a Mac with Adobe, skipped in CI |
//! | `paged-canvas/tests/color_settings.rs` | `corpus/profiles` only | skipped everywhere |
//! | `paged-canvas/tests/ink_coverage.rs` | `corpus/calibration` | skipped everywhere — that directory holds six JSON files and never held an `.icc` |
//!
//! The result (found by the 2026-08-19 corpus audit): **zero `.icc`
//! files exist anywhere in the workspace**, and roughly eight tests
//! covering the qcms engine, CMYK export, PDF/X-4, ink coverage and
//! soft-proofing printed "skipping" and passed. An entire colour
//! subsystem was green and asserting nothing.
//!
//! Profiles are NOT committed. Their licences vary (Adobe's are not
//! redistributable at all) and this is a public repo, so
//! `corpus/profiles/` is gitignored and populated by
//! `scripts/fetch-profiles.sh`, which pulls a freely-licensed profile
//! from a pinned upstream tag — the same approach ci.yml's export-diff
//! gate already uses.

use std::path::{Path, PathBuf};

/// Where a CMYK profile came from, so a skip message can say which
/// lever to pull rather than just "not found".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSource {
    /// `PAGED_CMYK_PROFILE` pointed at it.
    Env(PathBuf),
    /// Found in `corpus/profiles/` (run `scripts/fetch-profiles.sh`).
    Corpus(PathBuf),
    /// An Adobe installation on this machine. Never redistributable —
    /// fine for a local run, absent on every CI runner.
    Adobe(PathBuf),
}

impl ProfileSource {
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Env(p) | Self::Corpus(p) | Self::Adobe(p) => p,
        }
    }
}

/// The message a test should print when no profile is available. Names
/// every lever, in the order they are tried.
pub const NO_PROFILE_HINT: &str = "no CMYK profile: set PAGED_CMYK_PROFILE, \
     or run scripts/fetch-profiles.sh to populate corpus/profiles/";

/// Locate a CMYK profile: `PAGED_CMYK_PROFILE`, then any `.icc` in
/// `corpus/profiles/`, then a local Adobe install.
///
/// `crate_dir` is the caller's `CARGO_MANIFEST_DIR` — the corpus sits
/// two levels above it in every crate of this workspace.
#[must_use]
pub fn find_cmyk_profile(crate_dir: &str) -> Option<ProfileSource> {
    if let Ok(p) = std::env::var("PAGED_CMYK_PROFILE") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(ProfileSource::Env(path));
        }
    }

    let corpus = Path::new(crate_dir).join("../../corpus/profiles");
    if let Ok(entries) = std::fs::read_dir(&corpus) {
        // Sorted so a directory with several profiles picks the same one
        // on every machine — an unstable choice would make ΔE budgets
        // unreproducible.
        let mut icc: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("icc")))
            .collect();
        icc.sort();
        if let Some(first) = icc.into_iter().next() {
            return Some(ProfileSource::Corpus(first));
        }
    }

    let adobe = Path::new(
        "/Library/Application Support/Adobe/Color/Profiles/Recommended/CoatedFOGRA39.icc",
    );
    if adobe.is_file() {
        return Some(ProfileSource::Adobe(adobe.to_path_buf()));
    }
    None
}

/// `find_cmyk_profile` plus the bytes, for callers that just want to
/// build a transform.
#[must_use]
pub fn read_cmyk_profile(crate_dir: &str) -> Option<Vec<u8>> {
    let found = find_cmyk_profile(crate_dir)?;
    std::fs::read(found.path()).ok()
}
