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

use std::collections::BTreeMap;

use crate::{address, Error, Hash, Kind};

/// What a host persists. Synchronous on purpose: the kernel does no I/O, and a
/// host with asynchronous storage writes objects first, then moves the ref.
pub trait Store {
    fn get(&self, hash: &Hash) -> Option<Vec<u8>>;
    fn has(&self, hash: &Hash) -> bool {
        self.get(hash).is_some()
    }
    /// Stores an object of `kind`. The caller has verified its address.
    fn put(&mut self, kind: Kind, hash: Hash, bytes: Vec<u8>);
    fn get_ref(&self, name: &str) -> Option<Hash>;
    fn refs(&self) -> Vec<(String, Hash)>;
    /// Moves `name` from `expected` to `new` (`None` = absent), or reports
    /// what it found. Atomic in every implementation.
    fn cas_ref(
        &mut self,
        name: &str,
        expected: Option<Hash>,
        new: Option<Hash>,
    ) -> Result<(), Error>;
    /// Evidence entries in order, as their canonical bytes.
    fn evidence(&self) -> Vec<Vec<u8>>;
    fn append_evidence(&mut self, bytes: Vec<u8>);
    /// Every stored address (for collection).
    fn addresses(&self) -> Vec<Hash>;
    fn remove(&mut self, hash: &Hash);
}

/// Reads an object and refuses it unless its bytes match its address: a store
/// is never trusted.
pub(crate) fn read(store: &impl Store, kind: Kind, hash: &Hash) -> Result<Vec<u8>, Error> {
    let bytes = store.get(hash).ok_or(Error::Missing(*hash))?;
    if address(kind, &bytes) != *hash {
        return Err(Error::Tampered(*hash));
    }
    Ok(bytes)
}

/// The in-memory store: the test oracle every other backend must agree with.
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    pub objects: BTreeMap<Hash, Vec<u8>>,
    pub refs: BTreeMap<String, Hash>,
    pub evidence: Vec<Vec<u8>>,
}

impl Store for MemoryStore {
    fn get(&self, hash: &Hash) -> Option<Vec<u8>> {
        self.objects.get(hash).cloned()
    }

    fn has(&self, hash: &Hash) -> bool {
        self.objects.contains_key(hash)
    }

    fn put(&mut self, _kind: Kind, hash: Hash, bytes: Vec<u8>) {
        self.objects.entry(hash).or_insert(bytes);
    }

    fn get_ref(&self, name: &str) -> Option<Hash> {
        self.refs.get(name).copied()
    }

    fn refs(&self) -> Vec<(String, Hash)> {
        self.refs.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }

    fn cas_ref(
        &mut self,
        name: &str,
        expected: Option<Hash>,
        new: Option<Hash>,
    ) -> Result<(), Error> {
        let found = self.refs.get(name).copied();
        if found != expected {
            return Err(Error::RefConflict {
                name: name.to_string(),
                expected,
                found,
            });
        }
        match new {
            Some(h) => self.refs.insert(name.to_string(), h),
            None => self.refs.remove(name),
        };
        Ok(())
    }

    fn evidence(&self) -> Vec<Vec<u8>> {
        self.evidence.clone()
    }

    fn append_evidence(&mut self, bytes: Vec<u8>) {
        self.evidence.push(bytes);
    }

    fn addresses(&self) -> Vec<Hash> {
        self.objects.keys().copied().collect()
    }

    fn remove(&mut self, hash: &Hash) {
        self.objects.remove(hash);
    }
}
