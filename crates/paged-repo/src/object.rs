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

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{package::HISTORY_PREFIX, Error, Hash};

/// Canonical JSON: compact, fields in declaration order, absent options omitted.
pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("objects serialize")
}

/// Decodes `bytes` and refuses them unless they are the canonical encoding.
pub fn decode<T: Serialize + DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    let value: T = serde_json::from_slice(bytes)?;
    if encode(&value) != bytes {
        return Err(Error::NonCanonical);
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkList {
    pub v: u32,
    pub size: u64,
    pub chunks: Vec<Hash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Blob,
    Chunks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeEntry {
    pub path: String,
    pub kind: EntryKind,
    pub hash: Hash,
    pub size: u64,
}

/// One entry per container part, sorted by path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tree {
    pub v: u32,
    pub entries: Vec<TreeEntry>,
}

impl Tree {
    pub fn get(&self, path: &str) -> Option<&TreeEntry> {
        self.entries
            .binary_search_by(|e| e.path.as_str().cmp(path))
            .ok()
            .map(|i| &self.entries[i])
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.v != 1 {
            return Err(Error::Malformed(format!("tree version {}", self.v)));
        }
        for w in self.entries.windows(2) {
            if w[0].path >= w[1].path {
                return Err(Error::Malformed(format!(
                    "tree not sorted at `{}`",
                    w[1].path
                )));
            }
        }
        for e in &self.entries {
            check_path(&e.path)?;
        }
        Ok(())
    }
}

/// A part path is relative, `/`-separated, and never climbs or names history.
pub(crate) fn check_path(path: &str) -> Result<(), Error> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.starts_with(HISTORY_PREFIX)
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..");
    if bad {
        return Err(Error::Package(format!("part path `{path}`")));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorKind {
    Human,
    Service,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub id: String,
    pub kind: ActorKind,
}

impl Actor {
    pub fn human(id: impl Into<String>) -> Actor {
        Actor {
            id: id.into(),
            kind: ActorKind::Human,
        }
    }

    pub fn agent(id: impl Into<String>) -> Actor {
        Actor {
            id: id.into(),
            kind: ActorKind::Agent,
        }
    }
}

/// Who an agent acted for, and with what. Required on agent commits only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub run: String,
    pub on_behalf_of: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedFrom {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Commit {
    pub v: u32,
    pub parents: Vec<Hash>,
    pub tree: Hash,
    pub author: Actor,
    pub message: String,
    /// Unix milliseconds, supplied by the host: the kernel reads no clock.
    pub ts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<ImportedFrom>,
}
