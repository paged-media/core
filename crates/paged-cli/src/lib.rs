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

//! `paged` — the engine on the command line.
//!
//! A library as well as a binary, because `paged-run` is a shim over
//! [`session::run`]: that binary has three independent consumers who
//! spawn it by name, with no argv, from `target/<profile>/paged-run`
//! (editor-server's automation lane, the docs scripting gate, and this
//! workspace's own `CARGO_BIN_EXE_paged-run` test), so it keeps
//! existing exactly as it is while `paged session` speaks the same
//! protocol from the same code.

pub mod engine;
pub mod inspect;
pub mod options;
pub mod render;
pub mod session;
