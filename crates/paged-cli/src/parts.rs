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

//! `paged parts` — the `.paged` container's content parts.
//!
//! A `.paged` file is a ZIP that is also a valid IDML package, carrying
//! native content parts beside the model: a plugin's HTML, a
//! spreadsheet, a vector document, a SQLite database. The engine has had
//! the door since protocol 51 (`ListPagedParts` / `ReadPagedPart` /
//! `WritePagedPart`) and the README says container parts "come for
//! free"; `grep PagedPart crates/paged-cli/src` returned nothing, so
//! from a command line they were unreachable — the one thing in
//! `cli_surface.rs`'s list that a `.paged`-shaped file format cannot be
//! missing and still claim a CLI.
//!
//! Writing goes through the same `WritePagedPart` the plugin adapters
//! use, `caller` and all, so what a CLI can put in a container is
//! exactly what a bundle can.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use paged_canvas::channel::{MainToWorkerKind, WorkerToMainKind};

use crate::engine::Session;
use crate::options::DocumentOptions;

#[derive(Debug, clap::Subcommand)]
pub enum PartsCommand {
    /// List the parts under a path prefix.
    List {
        /// `.paged` container (an IDML package carries no parts).
        doc: PathBuf,
        /// Prefix to list; empty lists everything.
        #[arg(default_value = "")]
        prefix: String,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Read one part's bytes.
    Read {
        doc: PathBuf,
        /// Part path inside the container.
        path: String,
        /// Write the bytes here instead of stdout.
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Write one part and save the container back.
    ///
    /// The write lands in the loaded model; `--save` is what puts it on
    /// disk. Without it the command reports what it would have written
    /// and changes nothing — a container write is not something to do
    /// by accident.
    Write {
        doc: PathBuf,
        /// Part path inside the container.
        path: String,
        /// File whose bytes become the part.
        from: PathBuf,
        /// Name the writing caller, as a bundle adapter does.
        #[arg(long)]
        caller: Option<String>,
        /// Save the container back to this path (or `--save` alone to
        /// overwrite the input).
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        save: Option<String>,
        #[command(flatten)]
        assets: DocumentOptions,
    },
}

/// The three shapes the engine says no in, rendered. They carry
/// different error TYPES — a bare string, `LoadError`, `WorkerError` —
/// so this returns the message rather than a borrow of a common field.
fn failed(reply: &WorkerToMainKind) -> Option<String> {
    match reply {
        WorkerToMainKind::PagedPartFailed { error } => Some(error.clone()),
        WorkerToMainKind::LoadFailed { error } => Some(error.to_string()),
        WorkerToMainKind::MutationFailed { error } => Some(error.to_string()),
        _ => None,
    }
}

pub fn run(what: &PartsCommand) -> Result<()> {
    match what {
        PartsCommand::List {
            doc,
            prefix,
            assets,
        } => {
            let mut session = Session::new();
            assets.open(&mut session, doc)?;
            let reply = session.send(MainToWorkerKind::ListPagedParts {
                prefix: prefix.clone(),
            })?;
            if let Some(e) = failed(&reply) {
                return Err(anyhow!("{e}"));
            }
            match reply {
                WorkerToMainKind::PagedPartList { paths } => {
                    for path in paths {
                        println!("{path}");
                    }
                    Ok(())
                }
                other => Err(anyhow!("the engine answered {other:?}")),
            }
        }
        PartsCommand::Read {
            doc,
            path,
            out,
            assets,
        } => {
            let mut session = Session::new();
            assets.open(&mut session, doc)?;
            let reply = session.send(MainToWorkerKind::ReadPagedPart { path: path.clone() })?;
            if let Some(e) = failed(&reply) {
                return Err(anyhow!("{e}"));
            }
            match reply {
                // `found: false` is an answer, and a distinct one from
                // an empty part — so it exits non-zero rather than
                // printing nothing and claiming success.
                WorkerToMainKind::PagedPartRead { found: false, .. } => {
                    Err(anyhow!("no part at {path:?}"))
                }
                WorkerToMainKind::PagedPartRead { bytes, .. } => match out {
                    Some(dest) => {
                        std::fs::write(dest, bytes.as_slice())?;
                        eprintln!(
                            "{path}: {} bytes → {}",
                            bytes.as_slice().len(),
                            dest.display()
                        );
                        Ok(())
                    }
                    None => {
                        use std::io::Write;
                        std::io::stdout().write_all(bytes.as_slice())?;
                        Ok(())
                    }
                },
                other => Err(anyhow!("the engine answered {other:?}")),
            }
        }
        PartsCommand::Write {
            doc,
            path,
            from,
            caller,
            save,
            assets,
        } => {
            let bytes = std::fs::read(from)?;
            let mut session = Session::new();
            assets.open(&mut session, doc)?;
            let reply = session.send(MainToWorkerKind::WritePagedPart {
                path: path.clone(),
                bytes: bytes.clone().into(),
                caller: caller.clone(),
            })?;
            if let Some(e) = failed(&reply) {
                return Err(anyhow!("{e}"));
            }
            match save {
                None => {
                    eprintln!(
                        "{path}: {} bytes written to the loaded container. \
                         Nothing saved — pass --save to write the file.",
                        bytes.len()
                    );
                    Ok(())
                }
                Some(dest) => {
                    let dest: &Path = if dest.is_empty() {
                        doc
                    } else {
                        Path::new(dest.as_str())
                    };
                    let reply = session.send(MainToWorkerKind::ExportPaged {})?;
                    if let Some(e) = failed(&reply) {
                        return Err(anyhow!("{e}"));
                    }
                    match reply {
                        WorkerToMainKind::PagedExported { bytes } => {
                            std::fs::write(dest, bytes.as_slice())?;
                            eprintln!(
                                "{path}: saved container ({} bytes) → {}",
                                bytes.as_slice().len(),
                                dest.display()
                            );
                            Ok(())
                        }
                        other => Err(anyhow!("the engine answered {other:?}")),
                    }
                }
            }
        }
    }
}
