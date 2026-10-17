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

//! The **paged repository** (ADR 180): the authoritative state of a document as
//! content-addressed parts, commits, refs and a hash-chained evidence log. A
//! `.paged` or `.idml` file is a projection of a tree; opening one is an import.
//!
//! The crate does no I/O. Hosts (the browser's private file system, the CLI, a
//! server) implement [`Store`]; [`Repo`] runs the governance checks every host
//! shares. The format is specified in `docs/reference/paged-repo-format.md`.

mod error;
mod evidence;
mod hash;
mod merge;
mod object;
mod package;
mod policy;
mod repo;
mod store;
mod sync;

pub use error::{Error, Refusal};
pub use evidence::{verify_chain, EvidenceEntry};
pub use hash::{address, Hash, Kind};
pub use merge::{diff_trees, merge_base, merge_trees, Change, ChangeKind};
pub use object::{
    decode, encode, Actor, ActorKind, ChunkList, Commit, EntryKind, ImportedFrom, Provenance, Tree,
    TreeEntry,
};
pub use package::{
    bundle_parts, import, project, read_part, Bundle, Snapshot, CHUNK_THRESHOLD, HISTORY_PREFIX,
};
pub use policy::{Autonomy, Policy, RefKind, Role};
pub use repo::{Ctx, Repo};
pub use store::{MemoryStore, Store};
pub use sync::{missing, push, reachable};
