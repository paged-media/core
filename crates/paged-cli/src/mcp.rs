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

//! `paged mcp`: the paged MCP tools over stdio, for Claude Desktop and Claude
//! Code, with nothing deployed. Store tools work on the disk library
//! (`paged library`); engine tools run the engine here, natively, through the
//! same session door `paged session` uses.
//!
//! Writes are an agent's, as on paged.server: `agent:local` acting for
//! `user:local`, on `agent/<run>/…` branches, with provenance; the local
//! library's owner merges them (the editor, or `paged`).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use paged_mcp::{tools, Content, Host, Outcome};
use paged_repo::{Actor, Commit, Ctx, Hash, PackStore, Provenance, Repo};
use serde_json::{json, Value};

use crate::library::{valid_name, DiskLibrary, LOCAL_OWNER};
use crate::options::DocumentOptions;
use crate::session::EngineSession;

struct Opened {
    document: String,
    version: Hash,
    branch: Option<String>,
}

pub struct LocalHost {
    lib: DiskLibrary,
    engine: RefCell<EngineSession>,
    opened: RefCell<Option<Opened>>,
    run: String,
    scratch: PathBuf,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn reference(args: &Value, key: &str) -> String {
    args[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_string()
}

fn fail(e: impl std::fmt::Display) -> Outcome {
    Outcome::error(e.to_string())
}

impl LocalHost {
    pub fn new(library: PathBuf, assets: DocumentOptions) -> LocalHost {
        let run = format!("local-{:x}", now() as u64 & 0xff_ffff_ffff);
        let scratch = std::env::temp_dir().join(format!("paged-mcp-{}", std::process::id()));
        LocalHost {
            lib: DiskLibrary::new(library),
            engine: RefCell::new(EngineSession::new(assets)),
            opened: RefCell::new(None),
            run,
            scratch,
        }
    }

    fn ctx(&self) -> Ctx {
        Ctx {
            actor: Actor::agent("agent:local"),
            ts: now(),
            provenance: Some(Provenance {
                client: "paged mcp".into(),
                model: None,
                run: self.run.clone(),
                on_behalf_of: LOCAL_OWNER.into(),
            }),
        }
    }

    fn document(&self, args: &Value) -> Result<(String, Repo<PackStore>), Outcome> {
        let d = args["document"].as_str().unwrap_or("");
        if !valid_name(d) {
            return Err(Outcome::error(format!(
                "`{d}` is not a document name in this library"
            )));
        }
        self.lib.open(d).map(|r| (d.to_string(), r)).map_err(fail)
    }

    fn version(repo: &Repo<PackStore>, r: &str) -> Option<Hash> {
        repo.head(r).or_else(|| Hash::from_hex(r))
    }

    fn tree(
        &self,
        name: &str,
        repo: &mut Repo<PackStore>,
        at: Hash,
    ) -> anyhow::Result<paged_repo::Tree> {
        self.lib.run(name, repo, |r| {
            let c: Commit = r.read_commit(&at)?;
            r.read_tree(&c.tree)
        })
    }

    fn file(&self, ext: &str) -> PathBuf {
        let _ = std::fs::create_dir_all(&self.scratch);
        self.scratch.join(format!("{}-{}.{ext}", self.run, now()))
    }

    fn store_tool(&self, tool: &str, args: &Value) -> Outcome {
        if tool == "list_documents" {
            return match self.lib.list() {
                Ok(d) => Outcome::json(json!({ "documents": d, "library": self.lib.root() })),
                Err(e) => fail(e),
            };
        }
        let (name, mut repo) = match self.document(args) {
            Ok(x) => x,
            Err(o) => return o,
        };
        let ctx = self.ctx();
        let out = (|| -> anyhow::Result<Value> {
            match tool {
                "describe_document" => {
                    let at = Self::version(&repo, &reference(args, "ref"))
                        .ok_or_else(|| anyhow::anyhow!("no such ref"))?;
                    let t = self.tree(&name, &mut repo, at)?;
                    let refs: BTreeMap<String, String> = paged_repo::Store::refs(&repo.store)
                        .into_iter()
                        .map(|(k, v)| (k, v.to_hex()))
                        .collect();
                    Ok(json!({ "version": at.to_hex(), "refs": refs,
                               "parts": t.entries.iter().map(|e| json!({ "path": e.path, "size": e.size })).collect::<Vec<_>>() }))
                }
                "read_part" => {
                    let at = Self::version(&repo, &reference(args, "ref"))
                        .ok_or_else(|| anyhow::anyhow!("no such ref"))?;
                    let path = args["path"].as_str().unwrap_or("");
                    let t = self.tree(&name, &mut repo, at)?;
                    let e = t
                        .get(path)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("no part {path}"))?;
                    let bytes = self
                        .lib
                        .run(&name, &mut repo, |r| paged_repo::read_part(&r.store, &e))?;
                    Ok(match String::from_utf8(bytes) {
                        Ok(t) => Value::String(t),
                        Err(b) => {
                            json!({ "path": path, "bytes": b.as_bytes().len(), "text": false })
                        }
                    })
                }
                "history" => {
                    let log = self
                        .lib
                        .run(&name, &mut repo, |r| r.log(&reference(args, "ref")))?;
                    Ok(json!({ "versions": log.iter().map(|(h, c)| json!({
                        "version": h.to_hex(), "message": c.message, "author": c.author.id,
                        "agent": c.provenance.is_some(), "ts": c.ts,
                        "for": c.provenance.as_ref().map(|p| p.on_behalf_of.clone()),
                    })).collect::<Vec<_>>() }))
                }
                "diff" => {
                    let a = Self::version(&repo, &reference(args, "from"))
                        .ok_or_else(|| anyhow::anyhow!("no such ref"))?;
                    let b = Self::version(&repo, &reference(args, "to"))
                        .ok_or_else(|| anyhow::anyhow!("no such ref"))?;
                    let (ta, tb) = (
                        self.tree(&name, &mut repo, a)?,
                        self.tree(&name, &mut repo, b)?,
                    );
                    Ok(json!(paged_repo::diff_trees(&ta, &tb).iter()
                        .map(|c| json!({ "path": c.path, "change": format!("{:?}", c.kind).to_lowercase() }))
                        .collect::<Vec<_>>()))
                }
                "create_branch" => {
                    let short = args["name"].as_str().unwrap_or("");
                    if !valid_name(short) {
                        anyhow::bail!("a branch name is lowercase letters, digits and '-'");
                    }
                    let to = format!("agent/{}/{short}", self.run);
                    let at = self.lib.run(&name, &mut repo, |r| {
                        r.branch(&reference(args, "from"), &to, &ctx)
                    })?;
                    self.lib.persist(&name, &mut repo)?;
                    Ok(json!({ "ref": to, "at": at.to_hex() }))
                }
                "commit_parts" => {
                    let branch = args["branch"].as_str().unwrap_or("").to_string();
                    let head = repo
                        .head(&branch)
                        .ok_or_else(|| anyhow::anyhow!("no such branch: start one first"))?;
                    let base = self.tree(&name, &mut repo, head)?;
                    let mut entries: BTreeMap<String, paged_repo::TreeEntry> = base
                        .entries
                        .into_iter()
                        .map(|e| (e.path.clone(), e))
                        .collect();
                    let mut objects = BTreeMap::new();
                    for p in args["parts"].as_array().into_iter().flatten() {
                        let path = p["path"].as_str().unwrap_or("").to_string();
                        if p["delete"] == true {
                            entries.remove(&path);
                            continue;
                        }
                        let bytes = match (p["text"].as_str(), p["base64"].as_str()) {
                            (Some(t), _) => t.as_bytes().to_vec(),
                            (None, Some(b)) => base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                b.trim(),
                            )?,
                            _ => anyhow::bail!("`{path}` needs text, base64 or delete"),
                        };
                        entries.insert(
                            path.clone(),
                            paged_repo::part_entry(path, bytes, &mut objects),
                        );
                    }
                    let tree = paged_repo::Tree {
                        v: 1,
                        entries: entries.into_values().collect(),
                    };
                    tree.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
                    let tb = paged_repo::encode(&tree);
                    let th = paged_repo::address(paged_repo::Kind::Tree, &tb);
                    objects.insert(th, (paged_repo::Kind::Tree, tb));
                    let message = args["message"].as_str().unwrap_or("").to_string();
                    let at = self.lib.run(&name, &mut repo, |r| {
                        r.commit(objects.clone(), th, &branch, &ctx, &message, None)
                    })?;
                    self.lib.persist(&name, &mut repo)?;
                    Ok(json!({ "ref": branch, "at": at.to_hex() }))
                }
                "propose_change" | "list_changes" => {
                    let path = self
                        .lib
                        .root()
                        .join("repos")
                        .join(&name)
                        .join("changes.json");
                    let mut list: Vec<Value> = std::fs::read(&path)
                        .ok()
                        .and_then(|b| serde_json::from_slice(&b).ok())
                        .unwrap_or_default();
                    if tool == "list_changes" {
                        return Ok(json!(list));
                    }
                    let source = args["branch"].as_str().unwrap_or("");
                    repo.head(source)
                        .ok_or_else(|| anyhow::anyhow!("no such branch {source}"))?;
                    let cr = json!({ "id": list.len() + 1, "source": source, "target": reference(args, "target"),
                                     "title": args["title"], "author": ctx.actor.id, "status": "open", "created": now() });
                    list.push(cr.clone());
                    std::fs::write(&path, serde_json::to_vec_pretty(&list)?)?;
                    Ok(
                        json!({ "change_request": cr, "next": "The library's owner reviews and merges it; it is not on main yet." }),
                    )
                }
                "download_link" => {
                    let at = Self::version(&repo, &reference(args, "ref"))
                        .ok_or_else(|| anyhow::anyhow!("no such ref"))?;
                    let target = reference(args, "ref");
                    let bytes = self.lib.run(&name, &mut repo, |r| {
                        r.export_commit(at, &target, &ctx, false)
                    })?;
                    self.lib.persist(&name, &mut repo)?;
                    let out = self
                        .lib
                        .root()
                        .join("exports")
                        .join(format!("{name}-{}.paged", &at.to_hex()[..12]));
                    std::fs::create_dir_all(out.parent().expect("has a parent"))?;
                    std::fs::write(&out, bytes)?;
                    Ok(json!({ "url": format!("file://{}", out.display()) }))
                }
                other => anyhow::bail!("no tool {other}"),
            }
        })();
        match out {
            Ok(v) => Outcome::json(v),
            Err(e) => fail(e),
        }
    }

    fn engine_tool(&self, tool: &str, args: &Value) -> Outcome {
        let mut engine = self.engine.borrow_mut();
        let out = (|| -> anyhow::Result<Outcome> {
            match tool {
                "open_document" => {
                    let (name, mut repo) = self
                        .document(args)
                        .map_err(|o| anyhow::anyhow!("{:?}", o.content))?;
                    let r = reference(args, "ref");
                    let at = Self::version(&repo, &r)
                        .ok_or_else(|| anyhow::anyhow!("no such ref {r}"))?;
                    let ctx = self.ctx();
                    let bytes = self
                        .lib
                        .run(&name, &mut repo, |x| x.export_commit(at, &r, &ctx, false))?;
                    self.lib.persist(&name, &mut repo)?;
                    let path = self.file("paged");
                    std::fs::write(&path, bytes)?;
                    let loaded = engine.request(json!({ "cmd": "load", "path": path }))?;
                    let branch = r.starts_with("agent/").then_some(r.clone());
                    *self.opened.borrow_mut() = Some(Opened {
                        document: name.clone(),
                        version: at,
                        branch,
                    });
                    Ok(Outcome::json(
                        json!({ "document": name, "version": at.to_hex(), "loaded": loaded }),
                    ))
                }
                "run_script" => {
                    let r =
                        engine.request(json!({ "cmd": "run-script", "source": args["source"] }))?;
                    let failed = r["result"]["error"].is_string();
                    Ok(Outcome {
                        is_error: failed,
                        ..Outcome::json(r)
                    })
                }
                "render_page" => {
                    let page = args["page"].as_u64().unwrap_or(1).max(1) - 1;
                    let path = self.file("png");
                    engine.request(
                        json!({ "cmd": "render", "page": page, "dpi": 96, "out": path }),
                    )?;
                    let png = std::fs::read(&path)?;
                    let data =
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png);
                    Ok(Outcome {
                        content: vec![Content::Image {
                            data,
                            mime: "image/png".into(),
                        }],
                        ..Default::default()
                    })
                }
                "save_document" => {
                    let mut opened = self.opened.borrow_mut();
                    let o = opened
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("open a document first"))?;
                    let path = self.file("paged");
                    engine.request(json!({ "cmd": "export", "format": "paged", "out": path }))?;
                    let snap = paged_repo::import(&std::fs::read(&path)?, "save")
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let mut repo = self.lib.open(&o.document)?;
                    let ctx = self.ctx();
                    let branch = match &o.branch {
                        Some(b) => b.clone(),
                        None => {
                            let b = format!("agent/{}/edit-{}", self.run, &o.version.to_hex()[..8]);
                            let v = o.version;
                            self.lib.run(&o.document, &mut repo, |r| {
                                r.receive(BTreeMap::new(), &b, None, Some(v), &ctx, false)
                            })?;
                            o.branch = Some(b.clone());
                            b
                        }
                    };
                    let message = args["message"].as_str().unwrap_or("Save").to_string();
                    let at = self.lib.run(&o.document, &mut repo, |r| {
                        r.commit(
                            snap.objects.clone(),
                            snap.tree,
                            &branch,
                            &ctx,
                            &message,
                            None,
                        )
                    })?;
                    self.lib.persist(&o.document, &mut repo)?;
                    o.version = at;
                    Ok(Outcome::json(json!({ "ref": branch, "at": at.to_hex(),
                        "next": "On your branch; propose_change asks for it to reach main." })))
                }
                other => anyhow::bail!("no tool {other}"),
            }
        })();
        out.unwrap_or_else(fail)
    }
}

impl Host for LocalHost {
    fn name(&self) -> &str {
        "paged (local)"
    }

    fn tools(&self) -> Vec<tools::Tool> {
        tools::select(&[tools::STORE, tools::REFERENCE, tools::ENGINE])
    }

    async fn call(&self, tool: &str, args: Value) -> Outcome {
        if tool == "lookup_api" {
            return Outcome::json(paged_mcp::lookup(args["query"].as_str().unwrap_or(""), 12));
        }
        if tools::ENGINE.contains(&tool) {
            return self.engine_tool(tool, &args);
        }
        self.store_tool(tool, &args)
    }
}

pub fn run(library: PathBuf, assets: DocumentOptions) -> anyhow::Result<()> {
    let host = LocalHost::new(library, assets);
    paged_mcp::serve_stdio(&host)?;
    let _ = std::fs::remove_dir_all(&host.scratch);
    Ok(())
}
