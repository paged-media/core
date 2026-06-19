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

//! `paged-run`: a headless, deterministic engine session driven over
//! line-delimited stdio (NDJSON). One process holds one `CanvasModel`
//! across its whole lifetime so a host (the editor-server `agent_generate`
//! job) can `load` once, then issue many `run-script` / `inspect` /
//! `render` / `export` commands without reloading.
//!
//! This is the execution + verification substrate for agent-driven
//! document automation: the LLM never runs here — it only emits the Boa
//! scripts this binary executes (every write funnels through the same
//! `paged.*` bridge + `apply_mutation` the editor uses). The binary is
//! pure engine: no network, no LLM, no document mutation outside the
//! Boa bridge.
//!
//! ## Protocol
//!
//! Read one JSON request object per line on stdin; write one JSON
//! response object per line on stdout. The first line emitted is a
//! `{"ok":true,"ready":true,"protocol":N}` greeting. Requests are
//! tagged by `cmd`:
//!
//! - `{"cmd":"load","path":"<file.idml|.paged>"}`
//! - `{"cmd":"new-blank","width":612,"height":792}`
//! - `{"cmd":"run-script","source":"<js>"}` → `{ok, result:ScriptResult}`
//! - `{"cmd":"inspect"}` → `{ok, meta, pages, sceneTree}`
//! - `{"cmd":"pages"}` → `{ok, pages}`
//! - `{"cmd":"digest"}` → `{ok, pageDigests, combined, stateHash}`
//! - `{"cmd":"render","page":<index|id>,"dpi":96,"out":"<file.png>"}`
//! - `{"cmd":"export","format":"idml|paged","out":"<file>"}`
//! - `{"cmd":"quit"}`
//!
//! ## Verification oracle: digest first, pixels second
//!
//! `digest` is the **primary** verification signal: per-page
//! `DisplayList::digest()` is the *same* display list the forward
//! WebGPU/Vello backend rasterizes, so it is faithful to what ships,
//! deterministic, and backend-agnostic (no CPU-vs-GPU drift). Use it for
//! structural / regression checks.
//!
//! `render` produces a PNG via the **CPU/tiny-skia** backend — the only
//! headless rasterizer in core (it also backs the fidelity gate). It is a
//! *vision aid* for the agent, NOT a pixel-exact match for the WebGPU
//! output users see. GPU-faithful headless pixels are a tracked follow-up
//! (core's native `vello-backend` is a stub today; `paged-sdk`'s Vello
//! readback is the wasm/WebGPU viewer path that loads from IDML, not a
//! live mutated `CanvasModel`).
//!
//! A malformed request or a command error yields `{"ok":false,"error":...}`
//! and the session stays alive (the host can recover / retry).

use std::io::{self, BufRead, Write};

use anyhow::{anyhow, Context as _, Result};
use paged_canvas::{CanvasModel, CanvasOptions, PageId};
use serde::Deserialize;
use serde_json::{json, Value};

/// The default-document id; the session is single-document so the id is
/// cosmetic (surfaces only in diagnostics).
const DOC_ID: &str = "paged-run";

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
enum Request {
    Load { path: String },
    NewBlank { width: f32, height: f32 },
    RunScript { source: String },
    Inspect,
    Pages,
    Digest,
    Render {
        page: Value,
        #[serde(default = "default_dpi")]
        dpi: f32,
        out: String,
    },
    Export { format: String, out: String },
    Quit,
}

/// 96 dpi ≈ on-screen thumbnail density; the agent's visual oracle does
/// not need print resolution. Hosts override per-`render`.
fn default_dpi() -> f32 {
    96.0
}

fn main() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut model: Option<CanvasModel> = None;

    // Handshake: announce liveness + the engine protocol the host is
    // talking to, so a version mismatch surfaces immediately.
    emit(
        &mut stdout,
        &json!({
            "ok": true,
            "ready": true,
            "protocol": paged_canvas::channel::PROTOCOL_VERSION.0,
        }),
    )?;

    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(line) {
            Ok(req) => req,
            Err(e) => {
                emit(&mut stdout, &json!({"ok": false, "error": format!("bad request: {e}")}))?;
                continue;
            }
        };
        if matches!(req, Request::Quit) {
            break;
        }
        let resp = handle(&mut model, req)
            .unwrap_or_else(|e| json!({"ok": false, "error": e.to_string()}));
        emit(&mut stdout, &resp)?;
    }
    Ok(())
}

/// Serialize one response object as a single NDJSON line and flush, so
/// the host reads a complete record per `readLine`.
fn emit(out: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

fn handle(model: &mut Option<CanvasModel>, req: Request) -> Result<Value> {
    match req {
        Request::Load { path } => {
            let bytes = std::fs::read(&path).with_context(|| format!("read {path}"))?;
            let m = CanvasModel::load(DOC_ID, &bytes, CanvasOptions::default())
                .map_err(|e| anyhow!("load failed: {e}"))?;
            let page_ids: Vec<String> = m.page_ids().map(|p| p.0.clone()).collect();
            let resp = json!({
                "ok": true,
                "loaded": path,
                "pageCount": page_ids.len(),
                "pageIds": page_ids,
            });
            *model = Some(m);
            Ok(resp)
        }
        Request::NewBlank { width, height } => {
            let m = CanvasModel::new_blank(DOC_ID, width, height, CanvasOptions::default())
                .map_err(|e| anyhow!("new-blank failed: {e}"))?;
            let page_ids: Vec<String> = m.page_ids().map(|p| p.0.clone()).collect();
            let resp = json!({
                "ok": true,
                "pageCount": page_ids.len(),
                "pageIds": page_ids,
            });
            *model = Some(m);
            Ok(resp)
        }
        Request::RunScript { source } => {
            let m = doc_mut(model)?;
            // Every write inside `source` funnels through the `paged.*`
            // bridge → `apply_mutation`; budgets (loop/recursion/stack/
            // 2s wall-clock) are enforced by `execute_script`'s default.
            let result = paged_script::execute_script(m, &source);
            Ok(json!({ "ok": result.error.is_none(), "result": result }))
        }
        Request::Inspect => {
            let m = doc_ref(model)?;
            Ok(json!({
                "ok": true,
                "meta": m.document_meta(),
                "pages": m.pages(),
                "sceneTree": m.scene_tree(),
            }))
        }
        Request::Pages => {
            let m = doc_ref(model)?;
            Ok(json!({ "ok": true, "pages": m.pages() }))
        }
        Request::Digest => {
            let m = doc_ref(model)?;
            // Per-page display-list digest = the GPU-faithful, backend-
            // agnostic oracle (same display list the WebGPU backend draws).
            // `combined` folds them order-sensitively into one document-
            // level value; `stateHash` is the canonical pre-render scene
            // hash for a second, independent equality signal.
            let mut page_digests = serde_json::Map::new();
            let mut combined: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a basis
            for page_id in m.page_ids() {
                let digest = m
                    .display_list_for_page(page_id)
                    .map_or(0, |dl| dl.digest());
                combined = combined.wrapping_mul(0x0000_0100_0000_01b3) ^ digest;
                page_digests.insert(page_id.0.clone(), json!(digest));
            }
            Ok(json!({
                "ok": true,
                "pageDigests": page_digests,
                "combined": combined,
                "stateHash": hex(&m.current_state_hash()),
            }))
        }
        Request::Render { page, dpi, out } => {
            let m = doc_ref(model)?;
            let page_id = resolve_page(m, &page)?;
            // CPU/tiny-skia rasterizer — the only headless backend in core,
            // and the fidelity-gate reference. A vision aid for the agent,
            // not a pixel-exact match for the shipped WebGPU output (see the
            // module-level "digest first, pixels second" note).
            let png = paged_canvas::render_snapshot_png_at_dpi(m, &page_id, dpi)
                .map_err(|e| anyhow!("render failed: {e}"))?;
            std::fs::write(&out, &png.png_bytes).with_context(|| format!("write {out}"))?;
            Ok(json!({
                "ok": true,
                "out": out,
                "pageId": page_id.0,
                "widthPx": png.width_px,
                "heightPx": png.height_px,
            }))
        }
        Request::Export { format, out } => {
            let m = doc_ref(model)?;
            let bytes = match format.as_str() {
                "idml" => m.export_idml().map_err(|e| anyhow!("export idml: {e}"))?,
                // The container scheme tracks the engine wire protocol;
                // export at the binary's own PROTOCOL_VERSION.
                "paged" => m
                    .export_paged(paged_canvas::channel::PROTOCOL_VERSION.0)
                    .map_err(|e| anyhow!("export paged: {e}"))?,
                other => {
                    return Err(anyhow!("unsupported export format '{other}' (idml|paged)"))
                }
            };
            let byte_count = bytes.len();
            std::fs::write(&out, &bytes).with_context(|| format!("write {out}"))?;
            Ok(json!({ "ok": true, "out": out, "format": format, "bytes": byte_count }))
        }
        Request::Quit => unreachable!("quit is handled in the main loop"),
    }
}

/// Borrow the loaded document immutably, or error if none is loaded yet.
fn doc_ref(model: &Option<CanvasModel>) -> Result<&CanvasModel> {
    model.as_ref().ok_or_else(|| anyhow!("no document loaded (issue `load` or `new-blank` first)"))
}

/// Borrow the loaded document mutably, or error if none is loaded yet.
fn doc_mut(model: &mut Option<CanvasModel>) -> Result<&mut CanvasModel> {
    model.as_mut().ok_or_else(|| anyhow!("no document loaded (issue `load` or `new-blank` first)"))
}

/// Lower-case hex encoding for the canonical state hash.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Resolve a `render`/page reference that is either a zero-based page
/// index (JSON number, or a numeric string) or a literal page id.
fn resolve_page(model: &CanvasModel, page: &Value) -> Result<PageId> {
    let ids: Vec<PageId> = model.page_ids().cloned().collect();
    let by_index = |i: usize| {
        ids.get(i)
            .cloned()
            .ok_or_else(|| anyhow!("page index {i} out of range (0..{})", ids.len()))
    };
    match page {
        Value::Number(n) => {
            let i = n.as_u64().ok_or_else(|| anyhow!("page index must be a non-negative integer"))?;
            by_index(i as usize)
        }
        Value::String(s) => {
            if let Ok(i) = s.parse::<usize>() {
                by_index(i)
            } else {
                let pid = PageId(s.clone());
                if model.page(&pid).is_some() {
                    Ok(pid)
                } else {
                    Err(anyhow!("no page with id '{s}'"))
                }
            }
        }
        _ => Err(anyhow!("`page` must be an index (number) or a page-id string")),
    }
}
