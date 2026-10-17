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
use std::io::{Cursor, Read, Write};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::object::check_path;
use crate::store::read;
use crate::{
    address, decode, encode, verify_chain, ChunkList, EntryKind, Error, EvidenceEntry, Hash,
    ImportedFrom, Kind, Store, Tree, TreeEntry,
};

/// Parts above this size are split by content-defined chunking.
pub const CHUNK_THRESHOLD: usize = 256 * 1024;
const CDC_MIN: usize = 16 * 1024;
const CDC_AVG: usize = 64 * 1024;
const CDC_MAX: usize = 256 * 1024;

/// The container folder that carries history; never part of a tree.
pub const HISTORY_PREFIX: &str = "paged/repo/";
const OBJECTS_PART: &str = "paged/repo/objects.bin";
const REFS_PART: &str = "paged/repo/refs.json";
const EVIDENCE_PART: &str = "paged/repo/evidence.json";

/// A refusal to unpack more than this: a ZIP's declared sizes are not trusted.
const MAX_UNPACKED: u64 = 4 * 1024 * 1024 * 1024;

/// What opening a file yields: a tree and the objects it needs, plus history if
/// the file carried any.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub tree: Hash,
    pub objects: BTreeMap<Hash, (Kind, Vec<u8>)>,
    pub imported_from: ImportedFrom,
    pub history: Option<Bundle>,
}

/// A repository's history as a file carries it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bundle {
    pub objects: BTreeMap<Hash, (Kind, Vec<u8>)>,
    pub refs: BTreeMap<String, Hash>,
    pub evidence: Vec<Vec<u8>>,
}

fn add(objects: &mut BTreeMap<Hash, (Kind, Vec<u8>)>, kind: Kind, bytes: Vec<u8>) -> Hash {
    let h = address(kind, &bytes);
    objects.entry(h).or_insert((kind, bytes));
    h
}

fn entry_for(
    path: String,
    bytes: Vec<u8>,
    objects: &mut BTreeMap<Hash, (Kind, Vec<u8>)>,
) -> TreeEntry {
    let size = bytes.len() as u64;
    if bytes.len() <= CHUNK_THRESHOLD {
        let hash = add(objects, Kind::Blob, bytes);
        return TreeEntry {
            path,
            kind: EntryKind::Blob,
            hash,
            size,
        };
    }
    let chunks = fastcdc::v2020::FastCDC::new(&bytes, CDC_MIN, CDC_AVG, CDC_MAX)
        .map(|c| {
            add(
                objects,
                Kind::Blob,
                bytes[c.offset..c.offset + c.length].to_vec(),
            )
        })
        .collect();
    let list = ChunkList { v: 1, size, chunks };
    let hash = add(objects, Kind::Chunks, encode(&list));
    TreeEntry {
        path,
        kind: EntryKind::Chunks,
        hash,
        size,
    }
}

/// Opens a `.paged` or `.idml` file as a tree. Parts are hashed over their
/// uncompressed bytes, so the ZIP's timestamps, order and compression do not
/// change an address.
pub fn import(file: &[u8], name: &str) -> Result<Snapshot, Error> {
    let pkg = |e: zip::result::ZipError| Error::Package(e.to_string());
    let mut zip = ZipArchive::new(Cursor::new(file)).map_err(pkg)?;
    let mut parts = BTreeMap::new();
    let mut history = BTreeMap::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(pkg)?;
        if f.is_dir() {
            continue;
        }
        let path = f.name().to_string();
        let mut bytes = Vec::new();
        (&mut f)
            .take(MAX_UNPACKED - total + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| Error::Package(e.to_string()))?;
        total += bytes.len() as u64;
        if total > MAX_UNPACKED {
            return Err(Error::Package("unpacks to more than 4 GiB".into()));
        }
        let slot = if path.starts_with(HISTORY_PREFIX) {
            &mut history
        } else {
            &mut parts
        };
        if slot.insert(path.clone(), bytes).is_some() {
            return Err(Error::Package(format!("duplicate part `{path}`")));
        }
    }
    if parts.is_empty() {
        return Err(Error::Package("no parts".into()));
    }

    let mut objects = BTreeMap::new();
    let mut entries = Vec::with_capacity(parts.len());
    for (path, bytes) in parts {
        check_path(&path)?;
        entries.push(entry_for(path, bytes, &mut objects));
    }
    let tree = add(&mut objects, Kind::Tree, encode(&Tree { v: 1, entries }));
    let sha256 = Sha256::digest(file)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let history = if history.is_empty() {
        None
    } else {
        Some(unbundle(&history)?)
    };
    Ok(Snapshot {
        tree,
        objects,
        imported_from: ImportedFrom {
            name: name.to_string(),
            sha256,
        },
        history,
    })
}

/// A part's bytes, reassembled from its chunks and checked against its size.
pub fn read_part(store: &impl Store, entry: &TreeEntry) -> Result<Vec<u8>, Error> {
    let bytes = match entry.kind {
        EntryKind::Blob => read(store, Kind::Blob, &entry.hash)?,
        EntryKind::Chunks => {
            let list: ChunkList = decode(&read(store, Kind::Chunks, &entry.hash)?)?;
            let mut out = Vec::with_capacity(list.size as usize);
            for c in &list.chunks {
                out.extend_from_slice(&read(store, Kind::Blob, c)?);
            }
            out
        }
    };
    if bytes.len() as u64 != entry.size {
        return Err(Error::Tampered(entry.hash));
    }
    Ok(bytes)
}

pub(crate) fn read_tree(store: &impl Store, hash: &Hash) -> Result<Tree, Error> {
    let tree: Tree = decode(&read(store, Kind::Tree, hash)?)?;
    tree.validate()?;
    Ok(tree)
}

/// The `.paged` projection of a tree: `mimetype` first and stored, every other
/// part in tree order and deflated, every timestamp 1980-01-01. Equal trees give
/// equal bytes. `extra` parts (a history bundle) are appended after the tree.
pub fn project(
    store: &impl Store,
    tree: &Hash,
    extra: &[(String, Vec<u8>)],
) -> Result<Vec<u8>, Error> {
    let tree = read_tree(store, tree)?;
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().last_modified_time(DateTime::default());
    let stored = opts.compression_method(CompressionMethod::Stored);
    let deflated = opts.compression_method(CompressionMethod::Deflated);
    let pkg = |e: zip::result::ZipError| Error::Package(e.to_string());
    let io = |e: std::io::Error| Error::Package(e.to_string());

    let mimetype = tree.get("mimetype");
    let rest = tree.entries.iter().filter(|e| e.path != "mimetype");
    for e in mimetype.into_iter().chain(rest) {
        let o = if e.path == "mimetype" {
            stored
        } else {
            deflated
        };
        zip.start_file(e.path.as_str(), o).map_err(pkg)?;
        zip.write_all(&read_part(store, e)?).map_err(io)?;
    }
    for (path, bytes) in extra {
        zip.start_file(path.as_str(), deflated).map_err(pkg)?;
        zip.write_all(bytes).map_err(io)?;
    }
    Ok(zip.finish().map_err(pkg)?.into_inner())
}

/// The parts that carry `bundle` in a file: objects as records of a kind tag,
/// a 32-byte address, a little-endian u32 length and the bytes.
pub fn bundle_parts(bundle: &Bundle) -> Vec<(String, Vec<u8>)> {
    let mut objects = Vec::new();
    for (h, (kind, bytes)) in &bundle.objects {
        objects.push(kind.tag());
        objects.extend_from_slice(&h.0);
        objects.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        objects.extend_from_slice(bytes);
    }
    let evidence: Vec<EvidenceEntry> = bundle
        .evidence
        .iter()
        .map(|b| decode(b).expect("stored evidence is canonical"))
        .collect();
    vec![
        (OBJECTS_PART.to_string(), objects),
        (REFS_PART.to_string(), encode(&bundle.refs)),
        (EVIDENCE_PART.to_string(), encode(&evidence)),
    ]
}

fn unbundle(parts: &BTreeMap<String, Vec<u8>>) -> Result<Bundle, Error> {
    let get = |p: &str| {
        parts
            .get(p)
            .ok_or_else(|| Error::Package(format!("history lacks `{p}`")))
    };
    let raw = get(OBJECTS_PART)?;
    let mut objects = BTreeMap::new();
    let mut at = 0usize;
    while at < raw.len() {
        let short = || Error::Package("truncated history objects".into());
        let head = raw.get(at..at + 37).ok_or_else(short)?;
        let kind = Kind::from_tag(head[0]).ok_or_else(short)?;
        let hash = Hash(head[1..33].try_into().expect("32 bytes"));
        let len = u32::from_le_bytes(head[33..37].try_into().expect("4 bytes")) as usize;
        let bytes = raw.get(at + 37..at + 37 + len).ok_or_else(short)?.to_vec();
        if address(kind, &bytes) != hash {
            return Err(Error::Tampered(hash));
        }
        objects.insert(hash, (kind, bytes));
        at += 37 + len;
    }
    let refs: BTreeMap<String, Hash> = serde_json::from_slice(get(REFS_PART)?)?;
    let entries: Vec<EvidenceEntry> = serde_json::from_slice(get(EVIDENCE_PART)?)?;
    let evidence: Vec<Vec<u8>> = entries.iter().map(encode).collect();
    verify_chain(&evidence)?;
    Ok(Bundle {
        objects,
        refs,
        evidence,
    })
}
