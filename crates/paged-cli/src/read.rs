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

//! `paged read` — the engine's diagnostic questions, on the command line.
//!
//! Seventeen wire kinds answered a question the CLI had no way to ask.
//! `cli_surface.rs` had them all under one reason — "a diagnostic read
//! with no subcommand yet: the engine answers it and the CLI has no verb
//! that asks, which is a gap in this surface rather than a property of
//! it" — which is the honest half of a surface that reached 11 of 62
//! message kinds. This is that half closed.
//!
//! Every subcommand is the same three lines: open the document through
//! [`crate::options::DocumentOptions::open`], send one
//! `MainToWorkerKind`, print the reply. Nothing here re-implements a
//! read; the point is that the CLI ASKS the engine rather than reaching
//! past it, which is what makes these answers the same answers the
//! editor gets.
//!
//! **Output is the wire reply, verbatim.** `{"kind": …, "payload": …}`,
//! the same envelope the NDJSON session emits and the editor receives —
//! so a script that pipes `paged read` into `jq` and a plugin reading
//! the same reply are looking at one shape. Pretty-printed for a
//! terminal; `--compact` for a pipe.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use paged_canvas::channel::{CollectionName, MainToWorkerKind, WorkerToMainKind};
use paged_canvas::element_selection::ElementId;

use crate::engine::Session;
use crate::options::DocumentOptions;

/// The document every read opens, and how to find its assets.
#[derive(Debug, Clone, clap::Args)]
pub struct ReadTarget {
    /// IDML or `.paged` document.
    pub doc: PathBuf,
    /// One JSON line instead of an indented block.
    #[arg(long, global = true)]
    pub compact: bool,
    #[command(flatten)]
    pub assets: DocumentOptions,
}

#[derive(Debug, clap::Subcommand)]
pub enum ReadCommand {
    /// Every `<Layer>`, with its visible / locked / printable flags.
    Layers {
        #[command(flatten)]
        on: ReadTarget,
    },
    /// One typed document collection.
    ///
    /// swatches | gradients | colorGroups | paragraphStyles |
    /// characterStyles | objectStyles | cellStyles | tableStyles |
    /// layers | spreads | pages | masterPages | links | articles |
    /// hyperlinks | bookmarks | crossReferences | conditions |
    /// conditionSets | fonts | indexTopics | inks | sections | stories
    Collection {
        #[command(flatten)]
        on: ReadTarget,
        /// Collection name, in the wire spelling.
        name: String,
    },
    /// The frames a story flows through, in order.
    FrameChain {
        #[command(flatten)]
        on: ReadTarget,
        /// Story `Self` id (from `paged read collection <doc> stories`).
        story_id: String,
    },
    /// A story's text content.
    StoryContent {
        #[command(flatten)]
        on: ReadTarget,
        story_id: String,
    },
    /// Plugin placeholder fields the document carries.
    Placeholders {
        #[command(flatten)]
        on: ReadTarget,
    },
    /// A swatch resolved to screen RGB through the colour settings.
    ColorPreview {
        #[command(flatten)]
        on: ReadTarget,
        /// Swatch `Self` id, e.g. `Color/Black`.
        swatch_id: String,
    },
    /// Resolve arbitrary channel values through the document's profiles.
    ColorCompute {
        #[command(flatten)]
        on: ReadTarget,
        /// CMYK | RGB | LAB | Gray.
        space: String,
        /// Channel values, repeatable and in `space` order.
        #[arg(long = "value", required = true)]
        value: Vec<f32>,
        #[arg(long)]
        tint: Option<f32>,
        /// Process | Spot.
        #[arg(long)]
        model: Option<String>,
    },
    /// A gradient's stops, resolved.
    GradientDetail {
        #[command(flatten)]
        on: ReadTarget,
        /// Gradient `Self` id.
        gradient_id: String,
    },
    /// Every property of one element — the Inspector's own read.
    ElementProperties {
        #[command(flatten)]
        on: ReadTarget,
        /// `kind:id` address, e.g. `textFrame:u12`.
        id: String,
    },
    /// Bounds and transform of one or more elements.
    ElementGeometry {
        #[command(flatten)]
        on: ReadTarget,
        /// `kind:id` addresses.
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// The leaf page items inside a group, recursively.
    GroupLeaves {
        #[command(flatten)]
        on: ReadTarget,
        /// Group `Self` id (bare, not an address).
        group_id: String,
    },
    /// A path element's anchors, with their control handles.
    PathAnchors {
        #[command(flatten)]
        on: ReadTarget,
        /// `kind:id` address of a path-carrying element.
        id: String,
    },
    /// The planar arrangement of overlapping paths — the faces the
    /// pathfinder region verbs address.
    PlanarRegions {
        #[command(flatten)]
        on: ReadTarget,
        /// `kind:id` addresses, front to back.
        #[arg(required = true)]
        ids: Vec<String>,
        /// Answer only the face under this page point.
        #[arg(long, num_args = 2, value_names = ["X", "Y"])]
        point: Option<Vec<f32>>,
    },
    /// Measure a string in a registered face.
    MeasureText {
        #[command(flatten)]
        on: ReadTarget,
        family: String,
        text: String,
        #[arg(long, default_value_t = 12.0)]
        size_pt: f32,
        #[arg(long)]
        style: Option<String>,
    },
    /// The bytes of a placed image, as the engine holds them.
    PlacedAsset {
        #[command(flatten)]
        on: ReadTarget,
        /// Element `Self` id of the frame holding the image.
        element_id: String,
        /// Write the bytes here (the reply's metadata still prints).
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// The bytes of a registered font face.
    FontFace {
        #[command(flatten)]
        on: ReadTarget,
        family: String,
        #[arg(long)]
        style: Option<String>,
        /// Write the bytes here.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Export the swatches as an Adobe `.ase` library.
    SwatchLibrary {
        #[command(flatten)]
        on: ReadTarget,
        /// Limit to one colour group.
        #[arg(long)]
        group: Option<String>,
        /// Write the `.ase` here.
        #[arg(short, long)]
        out: PathBuf,
    },
}

impl ReadCommand {
    fn target(&self) -> &ReadTarget {
        match self {
            Self::Layers { on }
            | Self::Collection { on, .. }
            | Self::FrameChain { on, .. }
            | Self::StoryContent { on, .. }
            | Self::Placeholders { on }
            | Self::ColorPreview { on, .. }
            | Self::ColorCompute { on, .. }
            | Self::GradientDetail { on, .. }
            | Self::ElementProperties { on, .. }
            | Self::ElementGeometry { on, .. }
            | Self::GroupLeaves { on, .. }
            | Self::PathAnchors { on, .. }
            | Self::PlanarRegions { on, .. }
            | Self::MeasureText { on, .. }
            | Self::PlacedAsset { on, .. }
            | Self::FontFace { on, .. }
            | Self::SwatchLibrary { on, .. } => on,
        }
    }
}

/// Resolve a `kind:id` address, naming what was wrong when it is not one.
///
/// The grammar is `paged-wire`'s, the same one `paged.set` accepts — see
/// [`paged_canvas::element_selection::ElementId::parse`]. A CLI that
/// invented its own would be the fourth copy of it.
fn address(s: &str) -> Result<ElementId> {
    ElementId::parse(s).ok_or_else(|| {
        anyhow!(
            "{s:?} is not an element address. Expected `kind:id` — textFrame, \
             rectangle, oval, polygon, graphicLine, group — or \
             `storyRange:<storyId>@<start>..<end>`."
        )
    })
}

fn addresses(list: &[String]) -> Result<Vec<ElementId>> {
    list.iter().map(|s| address(s)).collect()
}

/// Print one wire reply, or fail on the two the engine uses to say no.
fn emit(reply: &WorkerToMainKind, compact: bool) -> Result<()> {
    // The three shapes the engine says no in. They carry different
    // error TYPES (`LoadError`, `WorkerError`, a bare string), so they
    // cannot share an arm — and each is a non-zero exit, not a payload
    // to print.
    match reply {
        WorkerToMainKind::LoadFailed { error } => return Err(anyhow!("{error}")),
        WorkerToMainKind::MutationFailed { error } => return Err(anyhow!("{error}")),
        WorkerToMainKind::PagedPartFailed { error } => return Err(anyhow!("{error}")),
        _ => {}
    }
    let json = if compact {
        serde_json::to_string(reply)?
    } else {
        serde_json::to_string_pretty(reply)?
    };
    println!("{json}");
    Ok(())
}

/// Write a byte payload out, and say where it went on stderr so stdout
/// stays a clean JSON stream.
///
/// `found: false` does NOT write. An empty file is the shape of a
/// successful read of an empty thing, and these two answers must not
/// look alike on disk — the caller asked for bytes that are not there,
/// which is an error, not a zero-length result.
fn write_bytes(out: Option<&Path>, found: bool, bytes: &[u8], what: &str) -> Result<()> {
    let Some(path) = out else { return Ok(()) };
    if !found {
        return Err(anyhow!(
            "no {what} to write — the engine answered found: false"
        ));
    }
    std::fs::write(path, bytes)?;
    eprintln!("{what}: {} bytes → {}", bytes.len(), path.display());
    Ok(())
}

pub fn run(what: &ReadCommand) -> Result<()> {
    let target = what.target();
    let compact = target.compact;
    let mut session = Session::new();
    target.assets.open(&mut session, &target.doc)?;

    // Every arm is one message. The byte-carrying replies write their
    // payload out and print the rest, because a hundred kilobytes of
    // `[137,80,78,71,…]` on a terminal is not an answer.
    match what {
        ReadCommand::Layers { .. } => {
            let reply = session.send(MainToWorkerKind::RequestLayers)?;
            emit(&reply, compact)
        }
        ReadCommand::Collection { name, .. } => {
            let Some(collection) = CollectionName::from_str(name) else {
                return Err(anyhow!(
                    "{name:?} is not a collection. See `paged read collection --help`."
                ));
            };
            let reply = session.send(MainToWorkerKind::RequestCollection { name: collection })?;
            emit(&reply, compact)
        }
        ReadCommand::FrameChain { story_id, .. } => {
            let reply = session.send(MainToWorkerKind::RequestFrameChain {
                story_id: story_id.clone(),
            })?;
            emit(&reply, compact)
        }
        ReadCommand::StoryContent { story_id, .. } => {
            let reply = session.send(MainToWorkerKind::RequestStoryContent {
                story_id: story_id.clone(),
            })?;
            emit(&reply, compact)
        }
        ReadCommand::Placeholders { .. } => {
            let reply = session.send(MainToWorkerKind::RequestDocumentPlaceholders)?;
            emit(&reply, compact)
        }
        ReadCommand::ColorPreview { swatch_id, .. } => {
            let reply = session.send(MainToWorkerKind::RequestColorPreview {
                swatch_id: swatch_id.clone(),
            })?;
            emit(&reply, compact)
        }
        ReadCommand::ColorCompute {
            space,
            value,
            tint,
            model,
            ..
        } => {
            let reply = session.send(MainToWorkerKind::RequestColorCompute {
                space: space.clone(),
                value: value.clone(),
                tint: *tint,
                model: model.clone(),
                alternate_space: None,
                alternate_value: None,
            })?;
            emit(&reply, compact)
        }
        ReadCommand::GradientDetail { gradient_id, .. } => {
            let reply = session.send(MainToWorkerKind::RequestGradientDetail {
                gradient_id: gradient_id.clone(),
            })?;
            emit(&reply, compact)
        }
        ReadCommand::ElementProperties { id, .. } => {
            let reply =
                session.send(MainToWorkerKind::RequestElementProperties { id: address(id)? })?;
            emit(&reply, compact)
        }
        ReadCommand::ElementGeometry { ids, .. } => {
            let reply = session.send(MainToWorkerKind::RequestElementGeometry {
                ids: addresses(ids)?,
            })?;
            emit(&reply, compact)
        }
        ReadCommand::GroupLeaves { group_id, .. } => {
            let reply = session.send(MainToWorkerKind::RequestGroupLeaves {
                group_id: group_id.clone(),
            })?;
            emit(&reply, compact)
        }
        ReadCommand::PathAnchors { id, .. } => {
            let reply = session.send(MainToWorkerKind::RequestPathAnchors { id: address(id)? })?;
            emit(&reply, compact)
        }
        ReadCommand::PlanarRegions { ids, point, .. } => {
            // clap's `num_args = 2` gives a Vec; the wire wants a pair.
            let point = point.as_ref().map(|xy| [xy[0], xy[1]]);
            let reply = session.send(MainToWorkerKind::RequestPlanarRegions {
                element_ids: addresses(ids)?,
                point,
            })?;
            emit(&reply, compact)
        }
        ReadCommand::MeasureText {
            family,
            text,
            size_pt,
            style,
            ..
        } => {
            let reply = session.send(MainToWorkerKind::RequestMeasureText {
                family: family.clone(),
                style: style.clone(),
                text: text.clone(),
                size_pt: *size_pt,
            })?;
            emit(&reply, compact)
        }
        ReadCommand::PlacedAsset {
            element_id, out, ..
        } => {
            let reply = session.send(MainToWorkerKind::RequestPlacedAssetBytes {
                element_id: element_id.clone(),
            })?;
            if let WorkerToMainKind::PlacedAssetBytes {
                element_id,
                found,
                uri,
                width,
                height,
                encoded,
            } = &reply
            {
                write_bytes(out.as_deref(), *found, encoded.as_slice(), "placed asset")?;
                return emit(
                    &WorkerToMainKind::PlacedAssetBytes {
                        element_id: element_id.clone(),
                        found: *found,
                        uri: uri.clone(),
                        width: *width,
                        height: *height,
                        encoded: Vec::new().into(),
                    },
                    compact,
                );
            }
            emit(&reply, compact)
        }
        ReadCommand::FontFace {
            family, style, out, ..
        } => {
            let reply = session.send(MainToWorkerKind::RequestFontFaceBytes {
                family: family.clone(),
                style: style.clone(),
            })?;
            if let WorkerToMainKind::FontFaceBytes {
                found,
                family,
                style,
                postscript_name,
                format,
                bytes,
            } = &reply
            {
                write_bytes(out.as_deref(), *found, bytes.as_slice(), "font face")?;
                return emit(
                    &WorkerToMainKind::FontFaceBytes {
                        found: *found,
                        family: family.clone(),
                        style: style.clone(),
                        postscript_name: postscript_name.clone(),
                        format: format.clone(),
                        bytes: Vec::new().into(),
                    },
                    compact,
                );
            }
            emit(&reply, compact)
        }
        ReadCommand::SwatchLibrary { group, out, .. } => {
            let reply = session.send(MainToWorkerKind::ExportSwatchLibrary {
                group_id: group.clone(),
            })?;
            match &reply {
                WorkerToMainKind::SwatchLibraryExported { ase_bytes } => {
                    std::fs::write(out, ase_bytes.as_slice())?;
                    eprintln!(
                        "swatch library: {} bytes → {}",
                        ase_bytes.as_slice().len(),
                        out.display()
                    );
                    Ok(())
                }
                other => emit(other, compact),
            }
        }
    }
}
