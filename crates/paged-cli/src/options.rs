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

//! Opening a document, in the one order that works.
//!
//! Both halves of this are load-bearing and neither is obvious from a
//! call site, which is why every subcommand opens documents through
//! [`DocumentOptions::open`] and nothing else:
//!
//! * **Fonts before the load.** `RegisterFont` only appends to the
//!   worker's registry; `LoadDocument` clones that registry into the
//!   `CanvasOptions` it builds. Register after, and every styled run
//!   keeps the substitute it was shaped with.
//! * **A profile after the load needs telling.** The model binds its
//!   working space at load, matching the designmap's declared
//!   `CMYKProfile` against the registry, so a profile registered later
//!   is resolvable but not active until a `SetColorSettings` says so.
//!
//! Getting the second one wrong is not hypothetical: the annual's own
//! render harness registered a profile the document did not name and
//! painted 134 pages of CMYK with the naive `255·(1−ink)·(1−k)`
//! fallback — Slate 65/45/30/10 came out (80, 126, 161) against
//! InDesign's (101, 122, 144), the formula digit for digit.

use std::path::PathBuf;

use anyhow::{Context, Result};
use paged_canvas::channel::{MainToWorkerKind, WorkerToMainKind};
use paged_canvas::{DocumentHandle, FontEntry};
use paged_color::profiles::CmykProfileChoice;

use crate::engine::Session;
use crate::expect_reply;

/// The asset flags every document subcommand shares.
#[derive(Debug, Clone, clap::Args)]
pub struct DocumentOptions {
    /// Font file or directory to make available, repeatable.
    ///
    /// A directory is read one level deep and each face is asked for
    /// its own family name, so a corpus directory needs no mapping.
    #[arg(long = "fonts", value_name = "DIR|FILE")]
    pub fonts: Vec<PathBuf>,

    /// Bind one family (and optionally a style) to a file, repeatable:
    /// `--font-family "Fraunces/Italic=/path/Fraunces-Italic.ttf"`.
    ///
    /// Wins over `--fonts` for the same family and style — a scan is a
    /// convenience, this is the instrument.
    #[arg(long = "font-family", value_name = "NAME[/STYLE]=PATH")]
    pub font_family: Vec<String>,

    /// Fallback face for text no registered family answers for.
    #[arg(long = "font", value_name = "FILE")]
    pub font: Option<PathBuf>,

    /// CMYK profile: a file path, or a name to resolve against the
    /// host's installed profiles (e.g. "Coated FOGRA39").
    ///
    /// Absent, the document's own declared profile name is resolved
    /// and made the working space. Absent that too, colour is naive
    /// math and the run says so.
    #[arg(long = "cmyk-profile", value_name = "NAME|FILE")]
    pub cmyk_profile: Option<String>,
}

/// A page with text but no glyphs renders white, and nothing else in
/// the pipeline says why: core ships no fallback face, so text whose
/// family no registered font answers for is simply not shaped. Silence
/// here is how a blank render gets mistaken for a renderer bug — say it
/// out loud.
///
/// Checked after loading AND after a script authors text, because a
/// document born blank has no runs to complain about until the script
/// has added some.
pub fn warn_if_nothing_shaped(stats: &paged_canvas::DocumentStats) {
    if stats.runs > 0 && stats.glyphs == 0 {
        eprintln!(
            "warning: {} text run(s) shaped 0 glyphs — no registered font answered for \
             them, so the text will not appear. Pass --fonts <dir> to register \
             families, or --font <file> as a fallback face for text that names none.",
            stats.runs
        );
    }
}

impl DocumentOptions {
    /// Register the assets, load `path`, and settle the working colour
    /// space — in that order, which is the whole point of this type.
    pub fn open(&self, session: &mut Session, path: &std::path::Path) -> Result<DocumentHandle> {
        for entry in self.font_entries()? {
            let family = entry.family.clone();
            let reply = session.send(MainToWorkerKind::RegisterFont {
                family: family.clone(),
                style: entry.style.clone(),
                bytes: entry.bytes.clone().into(),
            })?;
            expect_reply!(reply, WorkerToMainKind::FontRegistered { .. } => (),
                format!("register font {family:?}"))?;
        }

        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let font = match &self.font {
            Some(p) => Some(
                std::fs::read(p)
                    .with_context(|| format!("read {}", p.display()))?
                    .into(),
            ),
            None => None,
        };

        // A profile given as a FILE is handed to the load directly —
        // that is the one input that outranks the registry.
        let declared = declared_cmyk_profile(&bytes);
        let (explicit_bytes, register_as) = self.resolve_profile(declared.as_deref())?;

        if let Some((name, bytes)) = &register_as {
            let reply = session.send(MainToWorkerKind::RegisterColorProfile {
                name: name.clone(),
                bytes: bytes.clone().into(),
            })?;
            expect_reply!(reply, WorkerToMainKind::ColorProfileRegistered { .. } => (),
                format!("register colour profile {name:?}"))?;
        }

        let reply = session.send(MainToWorkerKind::LoadDocument {
            bytes: bytes.into(),
            font,
            cmyk_icc_profile: explicit_bytes.map(Into::into),
        })?;
        let handle = expect_reply!(reply, WorkerToMainKind::DocumentLoaded(h) => h,
            format!("load {}", path.display()))?;

        // The load activates a registered profile only when the
        // document named it. When we registered one the document did
        // not name, say so explicitly.
        if let Some((name, _)) = register_as {
            let active = session
                .model()
                .ok()
                .and_then(|m| m.color_settings_state().cmyk_profile_name.clone());
            if active.as_deref() != Some(name.as_str()) {
                let reply = session.send(MainToWorkerKind::Mutate(
                    paged_canvas::channel::Mutation::SetColorSettings {
                        cmyk_profile_name: Some(name.clone()),
                        rgb_policy: None,
                        // The calibrated default the fidelity gate
                        // converts with; both lanes must agree.
                        intent: Some("RelativeColorimetric".to_string()),
                        bpc: Some(true),
                    },
                ))?;
                expect_reply!(reply, WorkerToMainKind::MutationApplied { .. } => (),
                    format!("make {name:?} the working space"))?;
            }
        }

        warn_if_nothing_shaped(&handle.stats);
        Ok(handle)
    }

    /// The registry to install: the scan first, then `--font-family`,
    /// so an explicit binding replaces a scanned one.
    fn font_entries(&self) -> Result<Vec<FontEntry>> {
        let mut entries = paged_canvas::font_registry_from_paths(&self.fonts);
        for spec in &self.font_family {
            let (name, path) = spec
                .split_once('=')
                .with_context(|| format!("--font-family wants NAME[/STYLE]=PATH, got {spec:?}"))?;
            let (family, style) = match name.split_once('/') {
                Some((f, s)) => (f.trim().to_string(), Some(s.trim().to_string())),
                None => (name.trim().to_string(), None),
            };
            let bytes = std::fs::read(path).with_context(|| format!("read font {path}"))?;
            entries.retain(|e| !(e.family == family && e.style == style));
            entries.push(FontEntry {
                family,
                style,
                bytes,
            });
        }
        Ok(entries)
    }

    /// `(bytes to hand the load, profile to register under a name)`.
    #[allow(clippy::type_complexity)]
    fn resolve_profile(
        &self,
        declared: Option<&str>,
    ) -> Result<(Option<Vec<u8>>, Option<(String, Vec<u8>)>)> {
        // A `--cmyk-profile` that names a real file is an explicit
        // override; anything else is a profile NAME.
        let as_path = self.cmyk_profile.as_ref().map(PathBuf::from);
        let cli_path = as_path.filter(|p| p.is_file());
        let env = std::env::var("PAGED_CMYK_PROFILE").ok();
        match paged_color::profiles::choose(cli_path.as_deref(), env.as_deref(), declared) {
            CmykProfileChoice::Explicit(path) => {
                let bytes =
                    std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
                Ok((Some(bytes), None))
            }
            CmykProfileChoice::Env { path, overrode } => {
                if let Some(name) = overrode {
                    eprintln!(
                        "colour: PAGED_CMYK_PROFILE overrides the document's declared {name:?}"
                    );
                }
                let bytes =
                    std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
                Ok((Some(bytes), None))
            }
            CmykProfileChoice::Declared(name) => {
                match paged_color::profiles::resolve_by_name(&name) {
                    Some(bytes) => Ok((None, Some((name, bytes)))),
                    None => {
                        eprintln!(
                            "colour: no installed profile matches {name:?}; \
                         converting with naive CMYK math"
                        );
                        Ok((None, None))
                    }
                }
            }
            CmykProfileChoice::Naive => {
                // A NAME was given but named no file and no install.
                if let Some(name) = &self.cmyk_profile {
                    match paged_color::profiles::resolve_by_name(name) {
                        Some(bytes) => return Ok((None, Some((name.clone(), bytes)))),
                        None => anyhow::bail!(
                            "--cmyk-profile {name:?} is neither a file nor an installed profile"
                        ),
                    }
                }
                Ok((None, None))
            }
        }
    }
}

/// The `CMYKProfile` a package's designmap declares, if any. Read
/// straight off the container so it is known BEFORE the load that has
/// to be told about it.
fn declared_cmyk_profile(bytes: &[u8]) -> Option<String> {
    let archive = idml_import::open_source_archive(bytes).ok()?;
    let designmap = archive.entry("designmap.xml")?;
    let text = String::from_utf8_lossy(designmap);
    let at = text.find("CMYKProfile=\"")? + "CMYKProfile=\"".len();
    let rest = &text[at..];
    let end = rest.find('"')?;
    let name = rest[..end].trim();
    (!name.is_empty() && name != "$ID/").then(|| name.to_string())
}
