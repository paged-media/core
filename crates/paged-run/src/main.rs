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

//! `paged-run`: the headless NDJSON engine session, now `paged
//! session` under a second name.
//!
//! The session itself moved to `paged_cli::session` so the `paged`
//! binary can offer it as a subcommand without a second implementation
//! of the protocol. This binary stays because three consumers spawn it
//! by name, with no argv, and would all break on a rename:
//!
//!   * editor-server's document-automation lane spawns
//!     `PAGED_RUN_BIN` (default: the bare string `"paged-run"`) with an
//!     empty argv;
//!   * the docs scripting gate resolves
//!     `core/target/{debug,release}/paged-run` directly;
//!   * this workspace's own `tests/session.rs` uses
//!     `env!("CARGO_BIN_EXE_paged-run")`.
//!
//! It takes no arguments — anything it accepted would be a second,
//! divergent surface for the same protocol.

fn main() -> anyhow::Result<()> {
    paged_cli::session::run()
}
