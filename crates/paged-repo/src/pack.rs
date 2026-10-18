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

//! Pack files: how a host with a file system keeps a repository, the same in a
//! browser's private file system, on a disk and in an object store.
//!
//! A repository on disk is append-only packs (records of a kind tag, a 32-byte
//! address, a little-endian u32 length and the bytes — the history bundle's
//! format), one index per pack, `refs.json`, and `evidence.jsonl`. The store
//! holds the indexes, the refs, the evidence and whatever objects the host has
//! handed it; an object it knows of but does not hold makes an operation fail
//! with [`Error::Missing`], the host reads it from its pack ([`PackStore::locate`])
//! and hands it over ([`PackStore::provide`]), and runs the operation again.
//! Operations are safe to run again: nothing moves before every read succeeded.

use std::collections::BTreeMap;

use crate::{address, verify_chain, Error, Hash, Kind, Store};

const INDEX_RECORD: usize = 32 + 1 + 8 + 4;

/// The kind tag's high bit: the record's bytes are deflated. Addresses are
/// always of the uncompressed bytes; compression is only how they are kept.
const DEFLATED: u8 = 0x80;
/// Inflating more than this is refused: a record is a part, a chunk or a tree.
const INFLATE_MAX: usize = 64 * 1024 * 1024;

/// The bytes to keep for `bytes`, deflated when that is smaller.
fn deflate(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 64 {
        return None;
    }
    let packed = miniz_oxide::deflate::compress_to_vec(bytes, 6);
    (packed.len() + 8 < bytes.len()).then_some(packed)
}

fn inflate(stored: &[u8], hash: &Hash) -> Result<Vec<u8>, Error> {
    miniz_oxide::inflate::decompress_to_vec_with_limit(stored, INFLATE_MAX)
        .map_err(|_| Error::Tampered(*hash))
}

/// One object as a host keeps it outside a pack (an object store): a flag
/// byte, then the bytes, deflated when that is smaller.
pub fn keep_bytes(bytes: &[u8]) -> Vec<u8> {
    match deflate(bytes) {
        Some(d) => [&[1u8][..], &d].concat(),
        None => [&[0u8][..], bytes].concat(),
    }
}

/// What [`keep_bytes`] kept, as the object's bytes again.
pub fn kept_bytes(kept: &[u8], hash: &Hash) -> Result<Vec<u8>, Error> {
    match kept.split_first() {
        Some((0, rest)) => Ok(rest.to_vec()),
        Some((1, rest)) => inflate(rest, hash),
        _ => Err(Error::Tampered(*hash)),
    }
}

/// Appends one record; returns where its bytes start, how many were written,
/// and whether they are deflated.
pub fn write_record(out: &mut Vec<u8>, kind: Kind, hash: &Hash, bytes: &[u8]) -> (u64, u32, bool) {
    let packed = deflate(bytes);
    let (tag, body) = match &packed {
        Some(d) => (kind.tag() | DEFLATED, d.as_slice()),
        None => (kind.tag(), bytes),
    };
    out.push(tag);
    out.extend_from_slice(&hash.0);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    let at = out.len() as u64;
    out.extend_from_slice(body);
    (at, body.len() as u32, packed.is_some())
}

/// Every record in `raw`, inflated and verified against its address.
pub fn read_records(raw: &[u8]) -> Result<Vec<(Kind, Hash, Vec<u8>)>, Error> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < raw.len() {
        let short = || Error::Package("truncated pack".into());
        let head = raw.get(at..at + 37).ok_or_else(short)?;
        let kind = Kind::from_tag(head[0] & !DEFLATED).ok_or_else(short)?;
        let hash = Hash(head[1..33].try_into().expect("32 bytes"));
        let len = u32::from_le_bytes(head[33..37].try_into().expect("4 bytes")) as usize;
        let stored = raw.get(at + 37..at + 37 + len).ok_or_else(short)?;
        let bytes = if head[0] & DEFLATED != 0 {
            inflate(stored, &hash)?
        } else {
            stored.to_vec()
        };
        if address(kind, &bytes) != hash {
            return Err(Error::Tampered(hash));
        }
        out.push((kind, hash, bytes));
        at += 37 + len;
    }
    Ok(out)
}

/// Where an object lives: its pack, the offset of its bytes, their length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loc {
    pub pack: u32,
    pub offset: u64,
    /// Bytes kept in the pack (deflated ones are fewer than the object's).
    pub len: u32,
    pub kind: Kind,
    pub deflated: bool,
}

/// A new pack and its index, to be written as two files.
#[derive(Debug, Clone)]
pub struct PackFile {
    pub id: u32,
    pub data: Vec<u8>,
    pub index: Vec<u8>,
}

/// What a host writes after an operation: a new pack if objects were added,
/// the refs (whole), and evidence lines to append.
#[derive(Debug, Clone)]
pub struct Flush {
    pub pack: Option<PackFile>,
    pub refs: Vec<u8>,
    pub evidence: Vec<Vec<u8>>,
}

#[derive(Debug, Default)]
pub struct PackStore {
    index: BTreeMap<Hash, Loc>,
    cache: BTreeMap<Hash, Vec<u8>>,
    pending: Vec<(Kind, Hash)>,
    refs: BTreeMap<String, Hash>,
    evidence: Vec<Vec<u8>>,
    flushed_evidence: usize,
    next_pack: u32,
}

impl PackStore {
    pub fn new() -> PackStore {
        PackStore::default()
    }

    /// Reads one pack's index (records of address, kind tag, u64 offset, u32 length).
    pub fn load_index(&mut self, pack: u32, index: &[u8]) -> Result<(), Error> {
        if !index.len().is_multiple_of(INDEX_RECORD) {
            return Err(Error::Package(format!("index of pack {pack} is truncated")));
        }
        for r in index.chunks(INDEX_RECORD) {
            let hash = Hash(r[..32].try_into().expect("32 bytes"));
            let kind = Kind::from_tag(r[32] & !DEFLATED)
                .ok_or_else(|| Error::Package(format!("index of pack {pack}: bad kind")))?;
            let deflated = r[32] & DEFLATED != 0;
            let offset = u64::from_le_bytes(r[33..41].try_into().expect("8 bytes"));
            let len = u32::from_le_bytes(r[41..45].try_into().expect("4 bytes"));
            self.index.entry(hash).or_insert(Loc {
                pack,
                offset,
                len,
                kind,
                deflated,
            });
        }
        self.next_pack = self.next_pack.max(pack + 1);
        Ok(())
    }

    pub fn load_refs(&mut self, json: &[u8]) -> Result<(), Error> {
        self.refs = serde_json::from_slice(json)?;
        Ok(())
    }

    /// Reads `evidence.jsonl` and verifies the chain.
    pub fn load_evidence(&mut self, jsonl: &[u8]) -> Result<(), Error> {
        let lines: Vec<Vec<u8>> = jsonl
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        verify_chain(&lines)?;
        self.flushed_evidence = lines.len();
        self.evidence = lines;
        Ok(())
    }

    pub fn locate(&self, hash: &Hash) -> Option<Loc> {
        self.index.get(hash).copied()
    }

    /// The objects a host should hand over right after opening: every commit,
    /// tree and chunk list. They are small; blobs are fetched when needed.
    pub fn metadata(&self) -> Vec<(Hash, Loc)> {
        self.index
            .iter()
            .filter(|(h, l)| l.kind != Kind::Blob && !self.cache.contains_key(h))
            .map(|(h, l)| (*h, *l))
            .collect()
    }

    /// Whether the store holds `hash`'s bytes (rather than only knowing where they are).
    pub fn holds(&self, hash: &Hash) -> bool {
        self.cache.contains_key(hash)
    }

    /// Hands over an object read from its pack, checked against its address.
    /// `bytes` are as read from the pack: deflated ones are inflated here.
    pub fn provide(&mut self, hash: Hash, bytes: Vec<u8>) -> Result<(), Error> {
        let loc = self.index.get(&hash).ok_or(Error::Missing(hash))?;
        let bytes = if loc.deflated {
            inflate(&bytes, &hash)?
        } else {
            bytes
        };
        if address(loc.kind, &bytes) != hash {
            return Err(Error::Tampered(hash));
        }
        self.cache.insert(hash, bytes);
        Ok(())
    }

    /// Drops blobs that are safely in a pack, keeping memory to the metadata.
    pub fn evict_blobs(&mut self) {
        let index = &self.index;
        self.cache
            .retain(|h, _| index.get(h).is_none_or(|l| l.kind != Kind::Blob));
    }

    /// Takes what changed since the last flush.
    pub fn flush(&mut self) -> Flush {
        let pack = if self.pending.is_empty() {
            None
        } else {
            let id = self.next_pack;
            self.next_pack += 1;
            let mut data = Vec::new();
            let mut index = Vec::new();
            for (kind, hash) in std::mem::take(&mut self.pending) {
                let bytes = &self.cache[&hash];
                let (offset, len, deflated) = write_record(&mut data, kind, &hash, bytes);
                index.extend_from_slice(&hash.0);
                index.push(if deflated {
                    kind.tag() | DEFLATED
                } else {
                    kind.tag()
                });
                index.extend_from_slice(&offset.to_le_bytes());
                index.extend_from_slice(&len.to_le_bytes());
                self.index.insert(
                    hash,
                    Loc {
                        pack: id,
                        offset,
                        len,
                        kind,
                        deflated,
                    },
                );
            }
            Some(PackFile { id, data, index })
        };
        let evidence = self.evidence[self.flushed_evidence..].to_vec();
        self.flushed_evidence = self.evidence.len();
        Flush {
            pack,
            refs: crate::encode(&self.refs),
            evidence,
        }
    }
}

impl Store for PackStore {
    fn get(&self, hash: &Hash) -> Option<Vec<u8>> {
        self.cache.get(hash).cloned()
    }

    fn has(&self, hash: &Hash) -> bool {
        self.index.contains_key(hash) || self.cache.contains_key(hash)
    }

    fn put(&mut self, kind: Kind, hash: Hash, bytes: Vec<u8>) {
        if self.has(&hash) {
            return;
        }
        self.cache.insert(hash, bytes);
        self.pending.push((kind, hash));
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
        self.index
            .keys()
            .chain(self.cache.keys())
            .copied()
            .collect()
    }

    /// Forgets an object. Its bytes stay in their pack until the host rewrites it.
    fn remove(&mut self, hash: &Hash) {
        self.index.remove(hash);
        self.cache.remove(hash);
        self.pending.retain(|(_, h)| h != hash);
    }
}

/// Runs `op` against `repo`, reading every object it turns out to need from
/// its pack with `read` and running it again, until it no longer misses one
/// this store can locate. How a host with synchronous reads drives a store.
pub fn faulting<T>(
    repo: &mut crate::Repo<PackStore>,
    mut read: impl FnMut(&Loc) -> Result<Vec<u8>, Error>,
    mut op: impl FnMut(&mut crate::Repo<PackStore>) -> Result<T, Error>,
) -> Result<T, Error> {
    loop {
        match op(repo) {
            Err(Error::Missing(h)) => {
                let loc = repo.store.locate(&h).ok_or(Error::Missing(h))?;
                let bytes = read(&loc)?;
                repo.store.provide(h, bytes)?;
            }
            other => return other,
        }
    }
}
