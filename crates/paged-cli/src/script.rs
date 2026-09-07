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

//! `paged script` — author a document from a `.js` file.
//!
//! ## Why this one path does not go through `ExecuteScript`
//!
//! Every other document command in this crate goes through
//! [`Session::send`], and this one nearly does. The `ExecuteScript` wire
//! kind hardcodes `ScriptBudget::default()` — a 2 s wall clock, justified
//! in `paged-script` as an editor-REPL guard: "short enough that a stuck
//! native chain in the editor REPL doesn't feel like a hang". That is the
//! right rule for a REPL and the wrong one for a batch CLI, where
//! authoring a 134-page document is the normal case and two seconds is
//! not a hang, it is the job.
//!
//! Raising it on the wire would be protocol drift: the editor's budget
//! IS 2 s, and a script that overruns there must overrun here. So the
//! budget becomes caller-supplied exactly where the dispatcher's own
//! comment says it should — "hosts wanting to tighten/loosen call
//! `execute_script_with` with a custom `ScriptBudget`" — and the CLI is
//! a host. Nothing in `paged-script` changed to allow it.
//!
//! The bookkeeping the wire arm does around the call is a GPU
//! scene-cache invalidation, and a headless session has no scene cache,
//! so nothing is skipped by going direct.
//!
//! `session`'s `run-script` keeps the 2 s default deliberately: the docs
//! gate validates its corpus against the SHIPPED default, and quietly
//! raising it there would let an example that times out in the editor
//! pass the docs gate.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::engine::Session;
use crate::export::PdfOptions;
use crate::options::DocumentOptions;

#[derive(Debug, Clone, clap::Args)]
pub struct ScriptOptions {
    /// Wall-clock ceiling in milliseconds, or "none" to disable the
    /// deadline (the loop, recursion and stack guards still apply).
    #[arg(long, default_value = "60000")]
    pub timeout: String,
    /// Loop-iteration ceiling.
    #[arg(long, default_value_t = 10_000_000)]
    pub max_loop_iterations: u64,
    /// Write the authored document here. The format follows the
    /// extension: .paged, .idml or .pdf.
    #[arg(short, long, visible_alias = "export")]
    pub out: Option<PathBuf>,
    /// Also rasterise a page of the result here.
    #[arg(long, value_name = "FILE.png")]
    pub render: Option<PathBuf>,
    /// Which page `--render` takes (1-based, or a page id).
    #[arg(long, default_value = "1")]
    pub page: String,
    /// Render resolution.
    #[arg(long, default_value_t = 144.0)]
    pub dpi: f32,
}

fn budget(opts: &ScriptOptions) -> Result<paged_script::ScriptBudget> {
    let wall_clock_ms = match opts.timeout.trim() {
        "none" | "off" | "0" => None,
        n => Some(
            n.parse::<u64>()
                .with_context(|| format!("--timeout wants milliseconds or \"none\", got {n:?}"))?,
        ),
    };
    Ok(paged_script::ScriptBudget {
        loop_iterations: opts.max_loop_iterations,
        wall_clock_ms,
        ..Default::default()
    })
}

pub fn run(
    doc: &Path,
    assets: &DocumentOptions,
    script: &Path,
    opts: &ScriptOptions,
    pdf: &PdfOptions,
) -> Result<()> {
    let source =
        std::fs::read_to_string(script).with_context(|| format!("read {}", script.display()))?;

    let mut session = Session::new();
    let handle = assets.open(&mut session, doc)?;

    let result = session.execute_script(&source, budget(&opts.clone())?);

    // The script's own console output first: when it fails, the lines
    // it printed before failing are usually the diagnosis.
    for line in &result.output {
        println!("{line}");
    }
    if let Some(error) = &result.error {
        match &result.budget_kind {
            // Naming the budget that tripped turns "it failed" into
            // "raise --timeout" or "your loop does not terminate".
            Some(kind) => bail!(
                "{script:?}: {error} (budget: {kind:?})",
                script = script.display()
            ),
            None => bail!("{}: {error}", script.display()),
        }
    }

    // The document was blank when it loaded, so the load-time check
    // had no runs to look at; the script has just made some.
    if let Ok(model) = session.model() {
        crate::options::warn_if_nothing_shaped(&model.handle().stats);
    }

    if let Some(out) = &opts.out {
        let format = crate::export::format_for(out)?;
        let bytes = crate::export::export_bytes(&mut session, format, pdf)?;
        std::fs::write(out, &bytes).with_context(|| format!("write {}", out.display()))?;
        println!("{} — {format}, {} bytes", out.display(), bytes.len());
    }
    if let Some(png) = &opts.render {
        // Rasterise from the SAME session, so the render is of what the
        // script authored rather than of a reloaded copy of the input.
        crate::render::render_pages(
            &mut session,
            &handle,
            Some(opts.page.clone()),
            false,
            opts.dpi,
            png,
        )?;
    }
    Ok(())
}
