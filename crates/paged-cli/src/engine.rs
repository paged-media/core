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

//! One door to the engine, and it is the editor's own.
//!
//! `WorkerCore::dispatch` is the typed entry point the wasm shell's
//! `handleMessage` wraps: same match arms, same model calls, no JSON in
//! between. Driving the CLI through it means the CLI cannot apply a
//! mutation the editor cannot, or load a document with options the
//! editor cannot express — and it inherits fonts, colour profiles, PDF
//! export, the container-parts door, scripting and undo without this
//! crate implementing any of them.
//!
//! The rule this module exists to hold: **anything that mutates state,
//! or that consumes session state (the font and colour registries,
//! export sessions, the undo stack, container parts), goes through
//! [`Session::send`]. Nothing in the CLI calls `CanvasModel::load` or
//! `apply_mutation` itself.** Pure reads with no wire kind — the digest
//! oracle, page rasterisation — go through [`Session::model`]. Since
//! v63 that includes running a script: the per-run budget `paged
//! script` needs is a wire parameter, not a reason to reach past the
//! dispatcher.

use anyhow::{anyhow, Result};
use paged_canvas::channel::{
    MainToWorker, MainToWorkerKind, ProtocolVersion, ScriptBudgetWire, WorkerToMainKind,
    PROTOCOL_VERSION,
};
use paged_canvas_wasm::dispatch::WorkerCore;

/// A live engine session: one `WorkerCore` for the process, exactly as
/// the editor holds one per worker.
pub struct Session {
    core: WorkerCore,
    seq: u64,
    started: std::time::Instant,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            core: WorkerCore::new(),
            seq: 0,
            started: std::time::Instant::now(),
        }
    }

    /// Milliseconds since the session started — the clock the wasm
    /// shell serves from `js_sys::Date::now`. Only the script budget's
    /// deadline reads it, and that only needs to advance monotonically.
    fn clock_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    /// Send one message and take the reply's payload.
    ///
    /// The `CacheEffect` the dispatcher returns steers the GPU scene
    /// cache, which a headless session does not have; it is read and
    /// dropped rather than ignored silently.
    pub fn send(&mut self, kind: MainToWorkerKind) -> Result<WorkerToMainKind> {
        self.seq += 1;
        let msg = MainToWorker {
            seq: self.seq,
            protocol: PROTOCOL_VERSION,
            kind,
        };
        let started = self.started;
        let clock = move || started.elapsed().as_secs_f64() * 1000.0;
        let (reply, _no_gpu_cache_to_invalidate) = self.core.dispatch(msg, &clock);
        Ok(reply.kind)
    }

    /// The loaded model, for the reads that have no wire kind.
    pub fn model(&self) -> Result<&paged_canvas::CanvasModel> {
        self.core
            .model
            .as_ref()
            .ok_or_else(|| anyhow!("no document loaded"))
    }

    /// Run a script against the loaded model THROUGH THE WIRE.
    ///
    /// `budget` is `None` for the engine's own ceilings (the editor's
    /// 2 s REPL guard) and `Some` to override them per run — a v63 wire
    /// parameter. Until v63 there was no such parameter, so a host that
    /// needed a different ceiling had to call `execute_script_with`
    /// itself; `paged script` did, and was the one command in this
    /// crate not going through `WorkerCore::dispatch`. A second door
    /// for a parameter is still a second door, so the parameter moved
    /// to the wire and the door closed.
    ///
    /// The reply's budget kind is mapped back to `paged-script`'s own
    /// enum so the JSON this crate prints keeps the shape
    /// `session_compat` pins.
    pub fn run_script(
        &mut self,
        source: &str,
        budget: Option<ScriptBudgetWire>,
    ) -> Result<paged_script::ScriptResult> {
        let reply = self.send(MainToWorkerKind::ExecuteScript {
            source: source.to_string(),
            budget,
        })?;
        // `expect_reply!` is defined below in this file, so name it
        // through the crate root rather than textual scope.
        let (output, error, budget_kind) = crate::expect_reply!(
            reply,
            WorkerToMainKind::ScriptResult {
                output,
                error,
                budget_kind,
            } => (output, error, budget_kind),
            "run script"
        )
        .map_err(|e: anyhow::Error| e)?;
        Ok(paged_script::ScriptResult {
            output,
            error,
            budget_kind: budget_kind.map(|kind| {
                use paged_canvas::channel::ScriptBudgetKind as Wire;
                use paged_script::ScriptBudgetKind as Src;
                match kind {
                    Wire::Iterations => Src::Iterations,
                    Wire::Recursion => Src::Recursion,
                    Wire::StackSize => Src::StackSize,
                    Wire::WallClock => Src::WallClock,
                }
            }),
        })
    }

    pub fn protocol(&self) -> ProtocolVersion {
        PROTOCOL_VERSION
    }

    /// Read the clock once so the field is not dead on non-script paths.
    pub fn elapsed_ms(&self) -> f64 {
        self.clock_ms()
    }
}

/// Narrow a reply to the variant a call expects, turning the engine's
/// own failure variants into errors that carry its words rather than
/// this crate's.
#[macro_export]
macro_rules! expect_reply {
    ($reply:expr, $pat:pat => $out:expr, $what:expr) => {
        match $reply {
            $pat => Ok($out),
            ::paged_canvas::channel::WorkerToMainKind::LoadFailed { error } => {
                Err(::anyhow::anyhow!("{}: {error}", $what))
            }
            ::paged_canvas::channel::WorkerToMainKind::MutationFailed { error } => {
                Err(::anyhow::anyhow!("{}: {error}", $what))
            }
            other => Err(::anyhow::anyhow!(
                "{}: the engine answered {other:?}",
                $what
            )),
        }
    };
}
