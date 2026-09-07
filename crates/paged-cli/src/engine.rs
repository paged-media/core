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
//! oracle, page rasterisation — go through [`Session::model`].

use anyhow::{anyhow, Result};
use paged_canvas::channel::{
    MainToWorker, MainToWorkerKind, ProtocolVersion, WorkerToMainKind, PROTOCOL_VERSION,
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
