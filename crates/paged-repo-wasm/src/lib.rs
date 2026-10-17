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

//! `@paged-media/repo-wasm`: one document repository (ADR 180) whose host keeps
//! the packs. The host loads the pack indexes, refs and evidence, hands over
//! the metadata objects, and writes what [`Repository::flush`] returns after
//! each change. An operation that needs an object the host has not handed over
//! fails with a message `missing:<address>`; the host reads it from the pack
//! [`Repository::locate`] names, calls [`Repository::provide`], and runs the
//! operation again.

use paged_repo::{diff_trees, Actor, ChangeKind, Ctx, Error, Hash, PackStore, Policy, Repo, Store};
use serde::Serialize;
use wasm_bindgen::prelude::*;

fn js(e: Error) -> JsError {
    match e {
        Error::Missing(h) => JsError::new(&format!("missing:{h}")),
        other => JsError::new(&other.to_string()),
    }
}

fn hash(s: &str) -> Result<Hash, JsError> {
    Hash::from_hex(s).ok_or_else(|| JsError::new(&format!("not an address: {s}")))
}

fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("serializes")
}

#[derive(Serialize)]
struct LocJs {
    hash: String,
    pack: u32,
    offset: f64,
    len: u32,
}

#[derive(Serialize)]
struct CommitJs {
    hash: String,
    parents: Vec<String>,
    tree: String,
    author: String,
    agent: bool,
    message: String,
    ts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    imported_from: Option<String>,
}

/// What to write after a change: a new pack (when objects were added), the
/// refs file whole, and lines to append to the evidence file.
#[wasm_bindgen]
pub struct Flush {
    pack_id: Option<u32>,
    pack_data: Vec<u8>,
    pack_index: Vec<u8>,
    refs: Vec<u8>,
    evidence: Vec<u8>,
}

#[wasm_bindgen]
impl Flush {
    #[wasm_bindgen(getter, js_name = packId)]
    pub fn pack_id(&self) -> Option<u32> {
        self.pack_id
    }
    #[wasm_bindgen(getter, js_name = packData)]
    pub fn pack_data(&self) -> Vec<u8> {
        self.pack_data.clone()
    }
    #[wasm_bindgen(getter, js_name = packIndex)]
    pub fn pack_index(&self) -> Vec<u8> {
        self.pack_index.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn refs(&self) -> Vec<u8> {
        self.refs.clone()
    }
    /// Evidence lines to append, each ending in `\n`.
    #[wasm_bindgen(getter)]
    pub fn evidence(&self) -> Vec<u8> {
        self.evidence.clone()
    }
}

#[wasm_bindgen]
pub struct Repository {
    repo: Repo<PackStore>,
}

#[wasm_bindgen]
impl Repository {
    /// An empty repository owned by `owner`, the device's one human.
    #[wasm_bindgen(constructor)]
    pub fn new(owner: &str) -> Repository {
        Repository {
            repo: Repo::new(PackStore::new(), Ok(Policy::local(owner))),
        }
    }

    #[wasm_bindgen(js_name = loadIndex)]
    pub fn load_index(&mut self, pack: u32, index: &[u8]) -> Result<(), JsError> {
        self.repo.store.load_index(pack, index).map_err(js)
    }

    #[wasm_bindgen(js_name = loadRefs)]
    pub fn load_refs(&mut self, json: &[u8]) -> Result<(), JsError> {
        self.repo.store.load_refs(json).map_err(js)
    }

    #[wasm_bindgen(js_name = loadEvidence)]
    pub fn load_evidence(&mut self, jsonl: &[u8]) -> Result<(), JsError> {
        self.repo.store.load_evidence(jsonl).map_err(js)
    }

    /// Where the commits, trees and chunk lists are, as JSON
    /// `[{hash, pack, offset, len}]`: hand them over right after loading.
    pub fn metadata(&self) -> String {
        let locs: Vec<LocJs> = self
            .repo
            .store
            .metadata()
            .into_iter()
            .map(|(h, l)| LocJs {
                hash: h.to_hex(),
                pack: l.pack,
                offset: l.offset as f64,
                len: l.len,
            })
            .collect();
        json(&locs)
    }

    /// Where one object is, as JSON `{hash, pack, offset, len}`.
    pub fn locate(&self, address: &str) -> Result<Option<String>, JsError> {
        let h = hash(address)?;
        Ok(self.repo.store.locate(&h).map(|l| {
            json(&LocJs {
                hash: h.to_hex(),
                pack: l.pack,
                offset: l.offset as f64,
                len: l.len,
            })
        }))
    }

    /// Every object an export of `target` will read that has not been handed
    /// over, as JSON `[{hash, pack, offset, len}]`: read them in one pass
    /// rather than faulting them in one export at a time.
    pub fn wanted(&self, target: &str, with_history: bool) -> Result<String, JsError> {
        let heads: Vec<Hash> = if with_history {
            self.repo.store.refs().into_iter().map(|(_, h)| h).collect()
        } else {
            vec![match self.repo.head(target) {
                Some(h) => h,
                None => hash(target)?,
            }]
        };
        // Only the target's own tree unless the history travels too.
        let objects = if with_history {
            paged_repo::reachable(&self.repo.store, &heads, |_| false).map_err(js)?
        } else {
            paged_repo::reachable(&self.repo.store, &heads, |c| *c != heads[0]).map_err(js)?
        };
        let locs: Vec<LocJs> = objects
            .into_iter()
            .filter(|(_, h)| !self.repo.store.holds(h))
            .filter_map(|(_, h)| {
                self.repo.store.locate(&h).map(|l| LocJs {
                    hash: h.to_hex(),
                    pack: l.pack,
                    offset: l.offset as f64,
                    len: l.len,
                })
            })
            .collect();
        Ok(json(&locs))
    }

    pub fn provide(&mut self, address: &str, bytes: Vec<u8>) -> Result<(), JsError> {
        self.repo.store.provide(hash(address)?, bytes).map_err(js)
    }

    #[wasm_bindgen(js_name = evictBlobs)]
    pub fn evict_blobs(&mut self) {
        self.repo.store.evict_blobs();
    }

    pub fn flush(&mut self) -> Flush {
        let f = self.repo.store.flush();
        let mut evidence = Vec::new();
        for line in f.evidence {
            evidence.extend_from_slice(&line);
            evidence.push(b'\n');
        }
        let (pack_id, pack_data, pack_index) = match f.pack {
            Some(p) => (Some(p.id), p.data, p.index),
            None => (None, Vec::new(), Vec::new()),
        };
        Flush {
            pack_id,
            pack_data,
            pack_index,
            refs: f.refs,
            evidence,
        }
    }

    /// Opens a file into `ref_name`, restoring the history it carries when this
    /// repository is empty. Returns the head commit.
    #[wasm_bindgen(js_name = importFile)]
    pub fn import_file(
        &mut self,
        file: &[u8],
        name: &str,
        ref_name: &str,
        actor: &str,
        ts: f64,
    ) -> Result<String, JsError> {
        let ctx = Ctx::human(actor, ts as i64);
        self.repo
            .import(file, name, ref_name, &ctx)
            .map(|h| h.to_hex())
            .map_err(js)
    }

    /// Commits a saved file on `ref_name`. A save that changed nothing makes
    /// no commit and returns the head.
    pub fn save(
        &mut self,
        file: &[u8],
        ref_name: &str,
        actor: &str,
        message: &str,
        ts: f64,
    ) -> Result<String, JsError> {
        let snap = paged_repo::import(file, "save").map_err(js)?;
        if let Some(head) = self.repo.head(ref_name) {
            if self.repo.read_commit(&head).map_err(js)?.tree == snap.tree {
                return Ok(head.to_hex());
            }
        }
        let ctx = Ctx::human(actor, ts as i64);
        self.repo
            .commit(snap.objects, snap.tree, ref_name, &ctx, message, None)
            .map(|h| h.to_hex())
            .map_err(js)
    }

    /// The `.paged` bytes of a ref or of a commit address.
    pub fn export(
        &mut self,
        target: &str,
        with_history: bool,
        actor: &str,
        ts: f64,
    ) -> Result<Vec<u8>, JsError> {
        let ctx = Ctx::human(actor, ts as i64);
        let commit = match self.repo.head(target) {
            Some(h) => h,
            None => hash(target)?,
        };
        self.repo
            .export_commit(commit, target, &ctx, with_history)
            .map_err(js)
    }

    /// First-parent history of `ref_name`, newest first, as JSON.
    pub fn log(&self, ref_name: &str) -> Result<String, JsError> {
        let log = self.repo.log(ref_name).map_err(js)?;
        let out: Vec<CommitJs> = log
            .into_iter()
            .map(|(h, c)| CommitJs {
                hash: h.to_hex(),
                parents: c.parents.iter().map(Hash::to_hex).collect(),
                tree: c.tree.to_hex(),
                author: c.author.id,
                agent: c.provenance.is_some(),
                message: c.message,
                ts: c.ts,
                imported_from: c.imported_from.map(|f| f.name),
            })
            .collect();
        Ok(json(&out))
    }

    /// `{name: address}` as JSON.
    pub fn refs(&self) -> String {
        let refs: std::collections::BTreeMap<String, String> = self
            .repo
            .store
            .refs()
            .into_iter()
            .map(|(k, v)| (k, v.to_hex()))
            .collect();
        json(&refs)
    }

    pub fn branch(
        &mut self,
        from: &str,
        to: &str,
        actor: &str,
        ts: f64,
    ) -> Result<String, JsError> {
        let ctx = Ctx::human(actor, ts as i64);
        self.repo
            .branch(from, to, &ctx)
            .map(|h| h.to_hex())
            .map_err(js)
    }

    pub fn release(
        &mut self,
        name: &str,
        commit: &str,
        actor: &str,
        ts: f64,
    ) -> Result<(), JsError> {
        let ctx = Ctx::human(actor, ts as i64);
        self.repo.release(name, hash(commit)?, &ctx).map_err(js)
    }

    /// Makes `version` current again as a new commit on `ref_name`.
    pub fn restore(
        &mut self,
        version: &str,
        ref_name: &str,
        actor: &str,
        ts: f64,
    ) -> Result<String, JsError> {
        let ctx = Ctx::human(actor, ts as i64);
        self.repo
            .restore(hash(version)?, ref_name, &ctx)
            .map(|h| h.to_hex())
            .map_err(js)
    }

    /// Merges `source` into `target` (part-level), approved by `approvers`.
    pub fn merge(
        &mut self,
        source: &str,
        target: &str,
        actor: &str,
        approvers: Vec<String>,
        ts: f64,
    ) -> Result<String, JsError> {
        let ctx = Ctx::human(actor, ts as i64);
        let approvals: Vec<Actor> = approvers.into_iter().map(Actor::human).collect();
        self.repo
            .merge(source, target, &ctx, &approvals)
            .map(|h| h.to_hex())
            .map_err(js)
    }

    /// The parts that differ between two commits, as JSON `[{path, change}]`.
    pub fn diff(&self, from: &str, to: &str) -> Result<String, JsError> {
        let tree = |c: &str| -> Result<_, JsError> {
            let commit = self.repo.read_commit(&hash(c)?).map_err(js)?;
            self.repo.read_tree(&commit.tree).map_err(js)
        };
        let changes: Vec<(String, &str)> = diff_trees(&tree(from)?, &tree(to)?)
            .into_iter()
            .map(|c| {
                let kind = match c.kind {
                    ChangeKind::Added => "added",
                    ChangeKind::Removed => "removed",
                    ChangeKind::Modified => "modified",
                };
                (c.path, kind)
            })
            .collect();
        Ok(json(&changes))
    }

    /// Appends a read, export or call to the evidence log.
    pub fn record(&mut self, actor: &str, action: &str, target: &str, result: &str, ts: f64) {
        let ctx = Ctx::human(actor, ts as i64);
        self.repo.record(&ctx, action, target, result);
    }

    /// The evidence log as a JSON array, oldest first.
    pub fn evidence(&self) -> String {
        let entries: Vec<serde_json::Value> = self
            .repo
            .store
            .evidence()
            .iter()
            .map(|b| serde_json::from_slice(b).expect("evidence is JSON"))
            .collect();
        json(&entries)
    }
}
