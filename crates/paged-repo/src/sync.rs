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

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::merge::read_commit;
use crate::package::read_tree;
use crate::store::read;
use crate::{decode, ChunkList, Ctx, EntryKind, Error, Hash, Kind, Repo, Store};

/// Every object reachable from `heads`, walking back through parents but not
/// past a commit `known` says the other side already has: a side that has a
/// commit has everything it reaches.
pub fn reachable(
    store: &impl Store,
    heads: &[Hash],
    known: impl Fn(&Hash) -> bool,
) -> Result<Vec<(Kind, Hash)>, Error> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut queue: VecDeque<Hash> = heads.iter().copied().collect();
    while let Some(c) = queue.pop_front() {
        if known(&c) || !seen.insert(c) {
            continue;
        }
        let commit = read_commit(store, &c)?;
        out.push((Kind::Commit, c));
        if seen.insert(commit.tree) {
            out.push((Kind::Tree, commit.tree));
            for e in read_tree(store, &commit.tree)?.entries {
                if !seen.insert(e.hash) {
                    continue;
                }
                match e.kind {
                    EntryKind::Blob => out.push((Kind::Blob, e.hash)),
                    EntryKind::Chunks => {
                        out.push((Kind::Chunks, e.hash));
                        let list: ChunkList = decode(&read(store, Kind::Chunks, &e.hash)?)?;
                        for ch in list.chunks {
                            if seen.insert(ch) {
                                out.push((Kind::Blob, ch));
                            }
                        }
                    }
                }
            }
        }
        queue.extend(commit.parents);
    }
    Ok(out)
}

/// The offered addresses this store lacks: what a remote answers a push offer with.
pub fn missing(store: &impl Store, offered: &[Hash]) -> Vec<Hash> {
    offered.iter().filter(|h| !store.has(h)).copied().collect()
}

/// Pushes `ref_name` from `local` to `remote`, sending only what the remote
/// lacks, then compare-and-sets the remote ref against the value it holds now.
pub fn push<A: Store, B: Store>(
    local: &Repo<A>,
    remote: &mut Repo<B>,
    ref_name: &str,
    ctx: &Ctx,
) -> Result<Hash, Error> {
    let head = local
        .store
        .get_ref(ref_name)
        .ok_or_else(|| Error::RefConflict {
            name: ref_name.to_string(),
            expected: None,
            found: None,
        })?;
    let expected = remote.store.get_ref(ref_name);
    let offer = reachable(&local.store, &[head], |c| remote.store.has(c))?;
    let kinds: BTreeMap<Hash, Kind> = offer.iter().map(|(k, h)| (*h, *k)).collect();
    let hashes: Vec<Hash> = offer.iter().map(|(_, h)| *h).collect();
    let mut objects = BTreeMap::new();
    for h in missing(&remote.store, &hashes) {
        let kind = kinds[&h];
        objects.insert(h, (kind, read(&local.store, kind, &h)?));
    }
    remote.receive(objects, ref_name, expected, Some(head), ctx, false)?;
    Ok(head)
}
