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

//! ONE way to decide which CMYK profile a document renders with.
//!
//! [`super::test_profiles`] already carries this lesson for tests —
//! "Before this existed there were three, and they disagreed" — and the
//! production side had quietly grown a fourth: `paged-inspect` resolved
//! by NAME (an Adobe-filename table probed against the host's install)
//! while `test_profiles` resolved by PATH (`PAGED_CMYK_PROFILE`, then
//! `corpus/profiles/`). Two answers to one question is how a fidelity
//! comparison ends up measuring two colour spaces against each other,
//! so both now live here and every caller reads the same precedence.
//!
//! No profile ships with the engine: they are large and individually
//! licensed by their issuers. `scripts/fetch-profiles.sh` populates the
//! gitignored `corpus/profiles/`.

use std::path::{Path, PathBuf};

/// Which profile a render should use, and why — the "why" is carried so
/// a log can say what displaced what instead of silently changing
/// colour.
#[derive(Debug, PartialEq, Eq)]
pub enum CmykProfileChoice {
    /// An explicit instruction (`--cmyk-profile`) outranks everything.
    Explicit(PathBuf),
    /// `PAGED_CMYK_PROFILE`, carrying the declared name it displaced.
    Env {
        path: PathBuf,
        overrode: Option<String>,
    },
    /// The document's own `CMYKProfile` name, to resolve against a
    /// local install.
    Declared(String),
    /// Nothing to convert with — naive CMYK→sRGB math.
    Naive,
}

/// `PAGED_CMYK_PROFILE` sits ABOVE the document's declared profile, and
/// that ordering is the whole point of it.
///
/// The fidelity harness measures our render against a `pdftoppm`
/// rasterisation of InDesign's PDF, and
/// `corpus/generated/render-diff.sh` forces poppler to
/// `$PAGED_CMYK_PROFILE` when it is set. If the renderer honoured only
/// the document's declared name, the two halves of that comparison
/// would run in different colour spaces on any machine without the
/// declared profile installed — which is exactly the uniform ~4 dE p99
/// that harness's own comment tells you not to chase in the renderer.
/// Both halves now read the same variable, so both move together.
///
/// It is deliberately not a *fallback*: honouring it only when the
/// declared name misses would leave the mismatch in place precisely
/// where the profile IS installed but differs from the one poppler was
/// pointed at.
pub fn choose(cli: Option<&Path>, env: Option<&str>, declared: Option<&str>) -> CmykProfileChoice {
    if let Some(path) = cli {
        return CmykProfileChoice::Explicit(path.to_path_buf());
    }
    // An empty value is how a shell spells "unset" when the variable is
    // exported but never assigned; treat it as absent rather than as a
    // path to "".
    if let Some(env) = env.map(str::trim).filter(|e| !e.is_empty()) {
        // "$ID/" is InDesign's "application default" sentinel, not a
        // profile the document chose, so overriding it displaces
        // nothing worth reporting.
        let overrode = declared
            .map(str::trim)
            .filter(|d| !d.is_empty() && *d != "$ID/")
            .map(str::to_owned);
        return CmykProfileChoice::Env {
            path: PathBuf::from(env),
            overrode,
        };
    }
    match declared {
        Some(name) => CmykProfileChoice::Declared(name.to_owned()),
        None => CmykProfileChoice::Naive,
    }
}

/// Resolve an IDML-declared `CMYKProfile` name (e.g. `"Coated FOGRA39
/// (ISO 12647-2:2004)"`) to ICC bytes, by mapping common Adobe profile
/// names to Adobe's standard `Recommended/` filenames and probing the
/// host's install.
pub fn resolve_by_name(name: &str) -> Option<Vec<u8>> {
    let trimmed = name.trim();
    // "$ID/" is InDesign's sentinel for "use the application default"
    // — no profile was declared in the document. The corpus diff
    // harness forces pdftoppm to FOGRA39 for the reference PDF, so
    // matching that here keeps the candidate render and the reference
    // rasterisation in the same colour space.
    if trimmed == "$ID/" || trimmed.is_empty() {
        return load_installed("CoatedFOGRA39.icc");
    }
    // Try the full declared name first (handles mid-name parentheticals
    // like `"U.S. Web Coated (SWOP) v2"`), then retry with a trailing
    // parenthetical stripped (handles version-note suffixes like
    // `"Coated FOGRA39 (ISO 12647-2:2004)"`).
    if let Some(bytes) = filename_for_name(trimmed).and_then(load_installed) {
        return Some(bytes);
    }
    if let Some(head) = trimmed
        .split_once('(')
        .map(|(h, _)| h.trim())
        .filter(|h| !h.is_empty())
    {
        if let Some(bytes) = filename_for_name(head).and_then(load_installed) {
            return Some(bytes);
        }
    }
    None
}

/// The Adobe `Recommended/` filename a declared profile name maps to.
pub fn filename_for_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "Coated FOGRA39" | "Coated Fogra39" => "CoatedFOGRA39.icc",
        "Coated FOGRA27" => "CoatedFOGRA27.icc",
        "Uncoated FOGRA29" => "UncoatedFOGRA29.icc",
        "Web Coated FOGRA28" => "WebCoatedFOGRA28.icc",
        "Coated GRACoL 2006" | "Coated GRACoL2006" => "CoatedGRACoL2006.icc",
        "U.S. Web Coated (SWOP) v2" | "U.S. Web Coated SWOP v2" => "USWebCoatedSWOP.icc",
        "U.S. Sheetfed Coated v2" => "USSheetfedCoated.icc",
        "U.S. Sheetfed Uncoated v2" => "USSheetfedUncoated.icc",
        "U.S. Web Uncoated v2" => "USWebUncoated.icc",
        "Web Coated SWOP 2006 Grade 3 Paper" => "WebCoatedSWOP2006Grade3.icc",
        "Web Coated SWOP 2006 Grade 5 Paper" => "WebCoatedSWOP2006Grade5.icc",
        "Japan Color 2001 Coated" => "JapanColor2001Coated.icc",
        "Japan Color 2001 Uncoated" => "JapanColor2001Uncoated.icc",
        "Japan Color 2002 Newspaper" => "JapanColor2002Newspaper.icc",
        "Japan Color 2003 Web Coated" => "JapanColor2003WebCoated.icc",
        "Japan Web Coated (Ad)" | "Japan Web Coated" => "JapanWebCoated.icc",
        "US Newsprint (SNAP 2007)" => "USNewsprintSNAP2007.icc",
        _ => return None,
    })
}

/// Per-platform directories a colour-managed application installs its
/// profiles into. Adobe Creative Cloud and the legacy Adobe Color
/// package both use the `Recommended/` path.
pub fn installed_profile_dirs() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["/Library/Application Support/Adobe/Color/Profiles/Recommended"]
    } else if cfg!(target_os = "windows") {
        &["C:/Program Files (x86)/Common Files/Adobe/Color/Profiles/Recommended"]
    } else {
        &["/usr/share/color/icc", "/usr/share/color/icc/colord"]
    }
}

/// The installed path of a profile FILENAME, or `None` when no
/// platform directory has it.
pub fn find_installed(filename: &str) -> Option<PathBuf> {
    installed_profile_dirs()
        .iter()
        .map(|dir| Path::new(dir).join(filename))
        .find(|p| p.is_file())
}

/// [`find_installed`] plus the bytes.
pub fn load_installed(filename: &str) -> Option<Vec<u8>> {
    std::fs::read(find_installed(filename)?).ok()
}

#[cfg(test)]
mod tests {
    use super::{choose, filename_for_name, CmykProfileChoice};
    use std::path::{Path, PathBuf};

    #[test]
    fn cli_profile_outranks_everything() {
        let choice = choose(
            Some(Path::new("/tmp/explicit.icc")),
            Some("/tmp/env.icc"),
            Some("Coated FOGRA39"),
        );
        assert_eq!(
            choice,
            CmykProfileChoice::Explicit(PathBuf::from("/tmp/explicit.icc"))
        );
    }

    #[test]
    fn env_profile_outranks_the_documents_declared_name() {
        let choice = choose(None, Some("/tmp/env.icc"), Some("Coated FOGRA39"));
        assert_eq!(
            choice,
            CmykProfileChoice::Env {
                path: PathBuf::from("/tmp/env.icc"),
                overrode: Some("Coated FOGRA39".to_string()),
            }
        );
    }

    #[test]
    fn an_empty_env_var_is_unset_not_a_path_to_nothing() {
        let choice = choose(None, Some("   "), Some("Coated FOGRA39"));
        assert_eq!(
            choice,
            CmykProfileChoice::Declared("Coated FOGRA39".to_string())
        );
    }

    #[test]
    fn overriding_indesigns_default_sentinel_displaces_nothing() {
        let choice = choose(None, Some("/tmp/env.icc"), Some("$ID/"));
        assert_eq!(
            choice,
            CmykProfileChoice::Env {
                path: PathBuf::from("/tmp/env.icc"),
                overrode: None,
            }
        );
    }

    #[test]
    fn nothing_declared_and_nothing_set_is_naive_math() {
        assert_eq!(choose(None, None, None), CmykProfileChoice::Naive);
    }

    #[test]
    fn a_declared_name_maps_to_adobes_filename_with_or_without_its_suffix() {
        // The document spells it with the standard-number parenthetical;
        // Adobe's file does not.
        assert_eq!(
            filename_for_name("Coated FOGRA39"),
            Some("CoatedFOGRA39.icc")
        );
        // And a name whose parenthetical is part of the name itself
        // must not be truncated to death.
        assert_eq!(
            filename_for_name("U.S. Web Coated (SWOP) v2"),
            Some("USWebCoatedSWOP.icc")
        );
        assert_eq!(filename_for_name("Not A Profile"), None);
    }
}
