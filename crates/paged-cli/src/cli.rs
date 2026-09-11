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

//! The command tree — the CLI's surface as a TYPE, and as data.
//!
//! These lived in `main.rs`, which meant the only way to ask what the
//! CLI could do was to run `--help` and read prose. docs.paged.media had
//! no `paged` page at all for exactly that reason: the alternative to
//! generating one was writing a second copy of this tree by hand, which
//! drifts the first time a flag is added.
//!
//! So the tree lives in the library, and [`surface`] walks it. The
//! committed artifact `crates/paged-cli/cli.json` is what the docs
//! generate from, and `cli_json_artifact_is_current` fails when the two
//! disagree.

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use serde_json::{json, Value};

use crate::export::PdfOptions;
use crate::options::DocumentOptions;
use crate::script::ScriptOptions;

#[derive(Parser)]
#[command(
    name = "paged",
    version,
    about = "Open, author, render, export and verify paged documents.",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Rasterise a page to PNG.
    Render {
        /// IDML or `.paged` document.
        doc: PathBuf,
        /// Page to render: a 1-based number or a page id. Default: 1.
        #[arg(long)]
        page: Option<String>,
        /// Render every page; `-o` must then be a directory.
        #[arg(long)]
        all: bool,
        /// Resolution. 72 makes one pixel one point.
        #[arg(long, default_value_t = 144.0)]
        dpi: f32,
        /// Output PNG (or directory, with `--all`).
        #[arg(short, long)]
        out: PathBuf,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Report what the engine made of a document.
    Inspect {
        /// IDML or `.paged` document.
        doc: PathBuf,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Write the document out as IDML, `.paged`, or PDF.
    Export {
        /// IDML or `.paged` document.
        doc: PathBuf,
        /// idml | paged | pdf
        #[arg(long, default_value = "pdf")]
        format: String,
        /// Output file.
        #[arg(short, long)]
        out: PathBuf,
        #[command(flatten)]
        pdf: PdfOptions,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Author a document by running a `.js` file against it.
    Script {
        /// IDML or `.paged` document to author into.
        doc: PathBuf,
        /// The script.
        script: PathBuf,
        #[command(flatten)]
        opts: ScriptOptions,
        #[command(flatten)]
        pdf: PdfOptions,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Mint a blank document.
    New {
        /// letter | legal | tabloid | a3 | a4 | a5, or WxH in points.
        #[arg(long, default_value = "letter")]
        size: String,
        /// idml | paged
        #[arg(long, default_value = "paged")]
        format: String,
        /// Output file.
        #[arg(short, long)]
        out: PathBuf,
    },
    /// Emit the built-in corpus fixtures.
    Gen {
        #[command(subcommand)]
        what: GenCommand,
    },
    /// Compare two PNGs — ΔE2000 and SSIM. Exits 1 when they differ
    /// beyond the budget.
    Diff {
        /// The reference, first — the order is load-bearing.
        reference: PathBuf,
        /// The candidate.
        candidate: PathBuf,
        /// Emit the verdict as JSON instead of a line of prose.
        #[arg(long)]
        json: bool,
        /// Also write a ΔE heatmap PNG here.
        #[arg(long)]
        heatmap: Option<PathBuf>,
        /// ΔE mapped to peak heatmap intensity.
        #[arg(long, default_value_t = 5.0)]
        heatmap_scale: f64,
    },
    /// Ask the engine one of its diagnostic questions.
    ///
    /// Seventeen wire reads had no verb here; every one of them now
    /// prints the engine's own reply envelope, the same shape the
    /// NDJSON session emits and the editor receives.
    Read {
        #[command(subcommand)]
        what: crate::read::ReadCommand,
    },
    /// Put an image file into a graphic frame.
    ///
    /// The engine's only image lane through this door is inline bytes
    /// — a `placeImage` link resolves against a resolver the CLI never
    /// populates — and from a script those bytes are a JS array
    /// literal per pixel row. This is the same
    /// `Mutation::ReplaceImageBytes`, handed a file.
    Place {
        /// IDML or `.paged` document.
        doc: PathBuf,
        /// The graphic frame, as `rectangle:<id>` (also oval:/polygon:).
        frame: String,
        /// Image file. PNG / JPEG / WebP / TIFF / GIF / BMP — the
        /// formats the engine decodes.
        image: PathBuf,
        /// `<FrameFittingOption>` fitting mode, e.g. FillProportionally.
        #[arg(long)]
        fit: Option<String>,
        /// The placed image's own transform, `a,b,c,d,tx,ty` — how a
        /// crop is expressed exactly rather than approximated by a fit.
        #[arg(long, value_name = "A,B,C,D,TX,TY")]
        transform: Option<String>,
        /// Write the result here. The format follows the extension.
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[command(flatten)]
        pdf: PdfOptions,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// List, read and write a `.paged` container's content parts.
    Parts {
        #[command(subcommand)]
        what: crate::parts::PartsCommand,
    },
    /// Print the capability catalog — what this engine can be asked to do.
    Describe {
        /// One JSON line instead of an indented block.
        #[arg(long)]
        compact: bool,
    },
    /// Print the document's verification digests.
    Digest {
        /// IDML or `.paged` document.
        doc: PathBuf,
        /// One JSON line instead of an indented block.
        #[arg(long)]
        compact: bool,
        #[command(flatten)]
        assets: DocumentOptions,
    },
    /// Speak the headless NDJSON engine protocol on stdin/stdout.
    ///
    /// Byte-for-byte the `paged-run` protocol, from the same code: a
    /// greeting line, then one JSON response per JSON request, in
    /// order. Hosts that already spawn `paged-run` need not change.
    Session,
}

#[derive(Debug, clap::Subcommand)]
pub enum GenCommand {
    /// Emit one built-in fixture.
    Emit {
        /// Fixture name; an unknown one lists them all.
        #[arg(long)]
        sample: String,
        /// Output directory; the file lands at `<out>/<sample>.idml`.
        #[arg(long, default_value = "corpus/generated")]
        out: PathBuf,
    },
    /// Emit every built-in fixture. Prefer this over a copied name list.
    EmitAll {
        /// Output directory for the whole set.
        #[arg(long, default_value = "corpus/generated")]
        out: PathBuf,
    },
}

// ── the surface, as data ────────────────────────────────────────────

/// `help` and `version` are clap's own and sit on every command;
/// emitting them would repeat two rows fifteen times and tell a reader
/// nothing about `paged`.
fn is_clap_builtin(arg: &clap::Arg) -> bool {
    matches!(arg.get_id().as_str(), "help" | "version")
}

fn arg_json(arg: &clap::Arg) -> Value {
    // `get_num_args` is only Some when a range was set explicitly, so
    // defaulting it to false marks every ordinary `--flag <VALUE>` as
    // taking none — which renders `--size` where `--size <SIZE>` was
    // meant. The ACTION is the reliable answer: Set and Append take a
    // value, SetTrue and Count do not.
    let takes_values = arg
        .get_num_args()
        .map(|range| range.takes_values())
        .unwrap_or_else(|| arg.get_action().takes_values());
    json!({
        "name": arg.get_id().as_str(),
        "positional": arg.is_positional(),
        "required": arg.is_required_set(),
        "repeatable": matches!(arg.get_action(), clap::ArgAction::Append),
        "long": arg.get_long(),
        "short": arg.get_short().map(|c| c.to_string()),
        "takesValues": takes_values,
        "valueNames": arg
            .get_value_names()
            .map(|names| names.iter().map(|n| n.to_string()).collect::<Vec<_>>()),
        "defaults": arg
            .get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        "help": arg.get_help().map(|h| h.to_string()),
    })
}

fn command_json(cmd: &clap::Command) -> Value {
    json!({
        "name": cmd.get_name(),
        "about": cmd.get_about().map(|a| a.to_string()),
        "longAbout": cmd.get_long_about().map(|a| a.to_string()),
        "args": cmd
            .get_arguments()
            .filter(|a| !is_clap_builtin(a))
            .map(arg_json)
            .collect::<Vec<_>>(),
        "subcommands": cmd
            .get_subcommands()
            .filter(|s| s.get_name() != "help")
            .map(command_json)
            .collect::<Vec<_>>(),
    })
}

/// The whole command tree as JSON, walked out of the parser.
///
/// Not only documentation: the surface as DATA is what lets anything
/// else — a capability matrix, a completeness gate — ask what the CLI
/// can do without shelling out and parsing `--help`.
pub fn surface() -> Value {
    let cmd = Cli::command();
    json!({
        "binary": cmd.get_name(),
        "about": cmd.get_about().map(|a| a.to_string()),
        "protocol": paged_canvas::channel::PROTOCOL_VERSION.0,
        "commands": cmd
            .get_subcommands()
            .filter(|s| s.get_name() != "help")
            .map(command_json)
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed artifact is what docs.paged.media generates its
    /// command reference from. A flag added here and not regenerated
    /// there is a page that lies, which is the whole failure mode a
    /// generated reference exists to remove.
    #[test]
    fn cli_json_artifact_is_current() {
        let generated = serde_json::to_string_pretty(&surface()).unwrap();
        let committed = include_str!("../cli.json");
        assert_eq!(
            committed.trim_end(),
            generated.trim_end(),
            "cli.json is stale — regenerate: \
             cargo run -p paged-cli --example emit-cli-surface > crates/paged-cli/cli.json"
        );
    }

    /// Anti-fail-open: a walker that stops matching would emit an empty
    /// tree, the docs page would render empty, and every check above
    /// would still pass.
    #[test]
    fn the_walk_finds_a_real_tree() {
        let s = surface();
        let commands = s["commands"].as_array().expect("commands");
        assert!(
            commands.len() >= 12,
            "only {} top-level commands walked — the extractor is broken, not the CLI",
            commands.len()
        );
        let read = commands
            .iter()
            .find(|c| c["name"] == "read")
            .expect("`read` is a command");
        assert!(
            read["subcommands"].as_array().map(Vec::len).unwrap_or(0) >= 17,
            "`paged read` should carry every diagnostic question"
        );
        // Help strings are the page's prose. A tree with none is a page
        // with none.
        assert!(
            commands.iter().all(|c| c["about"].is_string()),
            "every command needs a doc comment — it is the page's text"
        );

        // And so is every ARGUMENT's. An arg with no help renders as a
        // bare `--flag` on docs.paged.media, which tells a reader less
        // than `--help` does — the one thing a generated page must not
        // manage.
        fn undocumented(cmd: &Value, prefix: &str) -> Vec<String> {
            let path = format!("{prefix} {}", cmd["name"].as_str().unwrap_or("?"));
            let mut out: Vec<String> = cmd["args"]
                .as_array()
                .map(|args| {
                    args.iter()
                        .filter(|a| !a["help"].is_string())
                        .map(|a| format!("{path} :: {}", a["name"].as_str().unwrap_or("?")))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(subs) = cmd["subcommands"].as_array() {
                for sub in subs {
                    out.extend(undocumented(sub, &path));
                }
            }
            out
        }
        let bare: Vec<String> = commands
            .iter()
            .flat_map(|c| undocumented(c, "paged"))
            .collect();
        assert!(
            bare.is_empty(),
            "these arguments carry no doc comment, so the generated page shows a \
             bare flag with nothing beside it: {bare:?}"
        );
    }
}
