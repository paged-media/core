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
//! ## The budget is a parameter, not a second door
//!
//! Every document command in this crate goes through [`Session::send`],
//! and until protocol v63 this one did not: the `ExecuteScript` wire
//! kind hardcoded `ScriptBudget::default()` — a 2 s wall clock,
//! justified in `paged-script` as an editor-REPL guard ("short enough
//! that a stuck native chain in the editor REPL doesn't feel like a
//! hang"). That is the right rule for a REPL and the wrong one for a
//! batch CLI, where authoring a 134-page document is the normal case
//! and two seconds is not a hang, it is the job.
//!
//! So the CLI called `execute_script_with` directly, which the
//! dispatcher's own comment sanctioned for hosts. It still cost
//! something the comment did not price in: `ExecuteScript` became the
//! one wire kind this crate could not be said to reach, and the CLI's
//! script path stopped being the same path the editor takes. A second
//! implementation of "run a script" is a place for the two to drift.
//!
//! v63 moves the budget ONTO the wire as an optional
//! [`ScriptBudgetWire`]: absent means the engine's own ceilings, so the
//! editor's 2 s is untouched and unchanged, and a host that wants a
//! different ceiling says so in the message instead of going around it.
//! The surface is now shared and only the parameter differs — which is
//! what the difference always was.
//!
//! `session`'s `run-script` sends no budget deliberately: the docs gate
//! validates its corpus against the SHIPPED default, and quietly
//! raising it there would let an example that times out in the editor
//! pass the docs gate.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use paged_canvas::channel::ScriptBudgetWire;

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

/// The CLI's flags as the wire's optional budget. `--timeout none`
/// travels as `Some(0)` rather than as an absent field, because absent
/// means "the engine's default" — which is the opposite of "no
/// deadline". The guards the CLI has no flag for (recursion, stack) are
/// left absent so they keep tracking the engine.
fn budget(opts: &ScriptOptions) -> Result<ScriptBudgetWire> {
    let wall_clock_ms = match opts.timeout.trim() {
        "none" | "off" | "0" => 0,
        n => n
            .parse::<u64>()
            .with_context(|| format!("--timeout wants milliseconds or \"none\", got {n:?}"))?,
    };
    Ok(ScriptBudgetWire {
        loop_iterations: Some(opts.max_loop_iterations),
        wall_clock_ms: Some(wall_clock_ms),
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

    let result = session.run_script(&source, Some(budget(opts)?))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The flags, as clap hands them over with nothing typed.
    #[derive(Parser)]
    struct OnlyTheFlags {
        #[command(flatten)]
        opts: ScriptOptions,
    }

    fn parse(args: &[&str]) -> ScriptOptions {
        let mut argv = vec!["script"];
        argv.extend_from_slice(args);
        OnlyTheFlags::parse_from(argv).opts
    }

    /// The whole reason this command once reached past the wire: its
    /// ceiling is 60 s, not the engine's 2 s. Now that the ceiling
    /// travels as a parameter, THIS is where the difference lives, and
    /// it is one assertion rather than a paragraph of prose.
    #[test]
    fn the_default_ceiling_is_the_cli_s_own_minute() {
        let b = budget(&parse(&[])).expect("defaults parse");
        assert_eq!(b.wall_clock_ms, Some(60_000));
        assert_eq!(b.loop_iterations, Some(10_000_000));
        // No flag, so no opinion: these keep tracking the engine.
        assert_eq!(b.recursion_depth, None);
        assert_eq!(b.stack_size, None);
    }

    /// `--timeout none` must travel as an explicit zero. Absent would
    /// mean "the engine's default", which is the opposite of what the
    /// user asked for, and the mistake is invisible until a long script
    /// dies at two seconds.
    #[test]
    fn disabling_the_deadline_is_a_zero_not_an_absence() {
        for spelling in ["none", "off", "0"] {
            let b = budget(&parse(&["--timeout", spelling])).expect("parses");
            assert_eq!(b.wall_clock_ms, Some(0), "--timeout {spelling}");
        }
    }

    #[test]
    fn a_timeout_that_is_not_a_number_says_so() {
        let err = budget(&parse(&["--timeout", "soon"])).expect_err("rejected");
        assert!(err.to_string().contains("milliseconds"), "unhelpful: {err}");
    }
}
