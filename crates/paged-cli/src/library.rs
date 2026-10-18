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

//! A library of repositories on disk, laid out as the editor keeps its own in
//! the browser (core ADR 180, `docs/reference/paged-repo-format.md`):
//! `repos/<name>/{meta.json, refs.json, evidence.jsonl, packs/<n>.pack|idx}`.
//! `paged library` fills it; `paged mcp` serves it.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use paged_repo::{faulting, Ctx, Error, Loc, PackStore, Policy, Repo};
use serde_json::{json, Value};

/// The person a local library belongs to.
pub const LOCAL_OWNER: &str = "user:local";

pub struct DiskLibrary {
    root: PathBuf,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl DiskLibrary {
    pub fn new(root: impl Into<PathBuf>) -> DiskLibrary {
        DiskLibrary { root: root.into() }
    }

    /// `$PAGED_LIBRARY`, else `~/.paged/library`.
    pub fn default_root() -> PathBuf {
        if let Some(p) = std::env::var_os("PAGED_LIBRARY") {
            return p.into();
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| ".".into());
        home.join(".paged").join("library")
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, name: &str) -> Result<PathBuf> {
        if !valid_name(name) {
            bail!("`{name}` is not a document name: lowercase letters, digits and '-'");
        }
        Ok(self.root.join("repos").join(name))
    }

    pub fn list(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(self.root.join("repos")) else {
            return Ok(out);
        };
        for e in entries.flatten() {
            if let Ok(meta) = fs::read(e.path().join("meta.json")) {
                out.push(serde_json::from_slice(&meta)?);
            }
        }
        out.sort_by_key(|m: &Value| std::cmp::Reverse(m["updatedAt"].as_i64().unwrap_or(0)));
        Ok(out)
    }

    /// The repository `name`, with its commits, trees and chunk lists read in.
    pub fn open(&self, name: &str) -> Result<Repo<PackStore>> {
        let dir = self.dir(name)?;
        if !dir.join("meta.json").exists() {
            bail!("no document `{name}` in {}", self.root.display());
        }
        let mut store = PackStore::new();
        if let Ok(packs) = fs::read_dir(dir.join("packs")) {
            for p in packs.flatten() {
                let file = p.file_name().to_string_lossy().to_string();
                // A pack with no index is a write that did not finish: ignored.
                if let Some(n) = file
                    .strip_suffix(".idx")
                    .and_then(|n| n.parse::<u32>().ok())
                {
                    store
                        .load_index(n, &fs::read(p.path())?)
                        .map_err(|e| anyhow!("{e}"))?;
                }
            }
        }
        if let Ok(refs) = fs::read(dir.join("refs.json")) {
            store.load_refs(&refs).map_err(|e| anyhow!("{e}"))?;
        }
        store
            .load_evidence(&fs::read(dir.join("evidence.jsonl")).unwrap_or_default())
            .map_err(|e| anyhow!("{e}"))?;
        for (h, loc) in store.metadata() {
            let bytes = self.read(name, &loc)?;
            store.provide(h, bytes).map_err(|e| anyhow!("{e}"))?;
        }
        Ok(Repo::new(store, Ok(Policy::local(LOCAL_OWNER))))
    }

    fn read(&self, name: &str, loc: &Loc) -> Result<Vec<u8>> {
        let mut f = fs::File::open(
            self.dir(name)?
                .join("packs")
                .join(format!("{}.pack", loc.pack)),
        )?;
        f.seek(SeekFrom::Start(loc.offset))?;
        let mut buf = vec![0u8; loc.len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Runs `op`, reading from the packs whatever object it turns out to need.
    pub fn run<T>(
        &self,
        name: &str,
        repo: &mut Repo<PackStore>,
        op: impl FnMut(&mut Repo<PackStore>) -> std::result::Result<T, Error>,
    ) -> Result<T> {
        faulting(
            repo,
            |l| {
                self.read(name, l)
                    .map_err(|e| Error::Package(e.to_string()))
            },
            op,
        )
        .map_err(|e| anyhow!("{e}"))
    }

    /// Writes what the last operations changed: pack, then index, then refs.
    pub fn persist(&self, name: &str, repo: &mut Repo<PackStore>) -> Result<()> {
        let dir = self.dir(name)?;
        fs::create_dir_all(dir.join("packs"))?;
        let f = repo.store.flush();
        if let Some(p) = f.pack {
            fs::write(dir.join("packs").join(format!("{}.pack", p.id)), &p.data)?;
            fs::write(dir.join("packs").join(format!("{}.idx", p.id)), &p.index)?;
        }
        let tmp = dir.join("refs.json.tmp");
        fs::write(&tmp, &f.refs)?;
        fs::rename(&tmp, dir.join("refs.json"))?;
        if !f.evidence.is_empty() {
            let mut ev = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("evidence.jsonl"))?;
            for line in f.evidence {
                ev.write_all(&line)?;
                ev.write_all(b"\n")?;
            }
        }
        let meta_path = dir.join("meta.json");
        let mut meta: Value = fs::read(&meta_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| json!({ "id": name, "name": name, "createdAt": now() }));
        meta["updatedAt"] = json!(now());
        meta["head"] = json!(repo.head("main").map(|h| h.to_hex()));
        fs::write(meta_path, serde_json::to_vec(&meta)?)?;
        Ok(())
    }

    /// Opens a file as a new document named `name`.
    pub fn import(&self, name: &str, file: &[u8], file_name: &str) -> Result<String> {
        let dir = self.dir(name)?;
        if dir.join("meta.json").exists() {
            bail!("a document named `{name}` exists");
        }
        fs::create_dir_all(dir.join("packs"))
            .with_context(|| format!("creating {}", dir.display()))?;
        fs::write(
            dir.join("meta.json"),
            serde_json::to_vec(
                &json!({ "id": name, "name": name, "createdAt": now(), "updatedAt": now() }),
            )?,
        )?;
        let mut repo = Repo::new(PackStore::new(), Ok(Policy::local(LOCAL_OWNER)));
        let head = repo
            .import(file, file_name, "main", &Ctx::human(LOCAL_OWNER, now()))
            .map_err(|e| anyhow!("{e}"))?;
        self.persist(name, &mut repo)?;
        Ok(head.to_hex())
    }
}

/// `paged library import <file> [--name]` and `paged library list`.
pub fn import_cmd(root: &Path, file: &Path, name: Option<&str>) -> Result<()> {
    let lib = DiskLibrary::new(root);
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let derived: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let name = name
        .map(String::from)
        .unwrap_or_else(|| derived.trim_matches('-').to_string());
    let bytes = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let file_name = file
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let head = lib.import(&name, &bytes, &file_name)?;
    println!("{name} {head}");
    Ok(())
}

pub fn list_cmd(root: &Path) -> Result<()> {
    for d in DiskLibrary::new(root).list()? {
        println!(
            "{}\t{}",
            d["name"].as_str().unwrap_or(""),
            d["head"].as_str().unwrap_or("-")
        );
    }
    Ok(())
}
