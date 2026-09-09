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

//! The headless, deterministic engine session driven over
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
//! - `{"cmd":"load","path":"<file.idml|.paged>"}` — optionally with
//!   `fonts:["dir"]`, `fontFamily:["Name[/Style]=path"]`, `cmykProfile:"name|path"`
//! - `{"cmd":"new-blank","width":612,"height":792}` — same three optional fields
//!   plus `defaultFont:"path"` — the fallback face, which core does not ship
//! - `{"cmd":"register-font","family":"Inter","style":null,"path":"…"}`
//! - `{"cmd":"register-color-profile","name":"Coated FOGRA39","path":"…"}`
//! - `{"cmd":"run-script","source":"<js>"}` → `{ok, result:ScriptResult}`
//! - `{"cmd":"inspect"}` → `{ok, meta, pages, sceneTree}`
//! - `{"cmd":"pages"}` → `{ok, pages}`
//! - `{"cmd":"digest"}` → `{ok, pageDigests, combined, stateHash}`
//! - `{"cmd":"describe"}` → `{ok, protocol, catalog}` (capability catalog for a
//!   consumer/LLM — host fns, id grammar, settable paths, constraints; no doc needed)
//! - `{"cmd":"render","page":<index|id>,"dpi":96,"out":"<file.png>","backend":"cpu"}`
//! - `{"cmd":"export","format":"idml|paged|pdf","out":"<file>","options":{…}}`
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
//! The asset fields and the two `register-*` commands are ADDITIVE: a
//! host that never sends them sees the protocol it always saw. They
//! exist because a session built on `CanvasOptions::default()` has no
//! fonts and no colour management, so it shapes no glyphs and converts
//! CMYK naively — the two things that made the editor and the headless
//! lane disagree. The registries seed shaping AT LOAD, so a
//! `register-*` sent after a `load` applies to the next one; the reply
//! says so rather than looking like it worked.
//!
//! `run-script` keeps the SHIPPED 2 s budget, deliberately: the docs
//! gate validates its corpus against that default, and raising it here
//! would let an example that times out in the editor pass the gate.
//! `paged script` is the batch lane with the larger budget.
//!
//! A malformed request or a command error yields `{"ok":false,"error":...}`
//! and the session stays alive (the host can recover / retry).

use std::io::{self, BufRead, Write};

use anyhow::{anyhow, Context as _, Result};
use paged_canvas::channel::{MainToWorkerKind, WorkerToMainKind};
use paged_canvas::PageId;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::engine::Session;
use crate::expect_reply;
use crate::options::DocumentOptions;

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
enum Request {
    #[serde(rename_all = "camelCase")]
    Load {
        path: String,
        /// Font files or directories, as `paged --fonts`. Additive to
        /// anything `register-font` already put in the registry.
        #[serde(default)]
        fonts: Vec<String>,
        /// `"Family[/Style]=path"`, as `paged --font-family`.
        #[serde(default)]
        font_family: Vec<String>,
        /// A profile file path, or a name resolved against the host's
        /// installed profiles.
        #[serde(default)]
        cmyk_profile: Option<String>,
        /// Fallback face for text that names a family nothing answers
        /// for. Core ships none, so a document that names no font at
        /// all — everything a script authors from blank — shapes zero
        /// glyphs without this.
        #[serde(default)]
        default_font: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    NewBlank {
        width: f32,
        height: f32,
        #[serde(default)]
        fonts: Vec<String>,
        #[serde(default)]
        font_family: Vec<String>,
        #[serde(default)]
        cmyk_profile: Option<String>,
        #[serde(default)]
        default_font: Option<String>,
    },
    /// Add one face to the registry. Takes effect on the NEXT `load` /
    /// `new-blank`: the registry seeds shaping at load, so registering
    /// after one cannot retroactively shape anything.
    RegisterFont {
        family: String,
        #[serde(default)]
        style: Option<String>,
        path: String,
    },
    /// Register a named ICC profile, same ordering rule.
    RegisterColorProfile {
        name: String,
        path: String,
    },
    RunScript {
        source: String,
    },
    Inspect,
    Pages,
    Digest,
    Describe,
    Render {
        page: Value,
        #[serde(default = "default_dpi")]
        dpi: f32,
        out: String,
        /// Accepted so a host can ASK for the GPU backend and get a
        /// straight answer rather than CPU pixels labelled as GPU
        /// ones. Core's native vello-backend is a stub, so anything
        /// but "cpu" is refused rather than silently downgraded.
        #[serde(default)]
        backend: Option<String>,
    },
    Export {
        format: String,
        out: String,
        /// `ExportPdfWireOptions` as a JSON object, for `format:"pdf"`.
        /// Absent = the dialog's defaults.
        #[serde(default)]
        options: Option<Value>,
    },
    Quit,
}

/// 96 dpi ≈ on-screen thumbnail density; the agent's visual oracle does
/// not need print resolution. Hosts override per-`render`.
fn default_dpi() -> f32 {
    96.0
}

/// Run the session until `quit` or EOF. `paged-run` and
/// `paged session` are both this function — one protocol, one
/// implementation, so the two can never answer differently.
pub fn run() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut live = Live::default();

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
                emit(
                    &mut stdout,
                    &json!({"ok": false, "error": format!("bad request: {e}")}),
                )?;
                continue;
            }
        };
        if matches!(req, Request::Quit) {
            break;
        }
        let resp =
            handle(&mut live, req).unwrap_or_else(|e| json!({"ok": false, "error": e.to_string()}));
        emit(&mut stdout, &resp)?;
    }
    Ok(())
}

/// Everything the loop holds between requests.
///
/// A `Session`, not a `CanvasModel` — this command used to keep the
/// model and call `CanvasModel::load`, `export_idml`,
/// `render_snapshot_png_at_dpi` and `CanvasExportSession::begin`
/// itself, while `engine.rs` next door declared the rule it was
/// breaking: *"anything that mutates state, or that consumes session
/// state, goes through `Session::send`. Nothing in the CLI calls
/// `CanvasModel::load`."* Two implementations of load, export and
/// render lived in one crate, and only one of them was the door the
/// editor uses.
struct Live {
    session: Session,
    /// The last load's handle — page ids and sizes, which
    /// `RequestSnapshot` needs to size a raster.
    handle: Option<paged_canvas::DocumentHandle>,
    /// Asset flags that arrived as their own `register-*` commands.
    /// Applied at the next load or blank document, which is exactly
    /// what those replies promise.
    pending: DocumentOptions,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            session: Session::new(),
            handle: None,
            pending: DocumentOptions {
                fonts: Vec::new(),
                font_family: Vec::new(),
                font: None,
                cmyk_profile: None,
            },
        }
    }
}

impl Live {
    /// The flags for this load: the ones that arrived as `register-*`
    /// commands, plus the ones on the request itself.
    fn options(
        &self,
        fonts: &[String],
        font_family: &[String],
        cmyk_profile: Option<&str>,
        default_font: Option<&str>,
    ) -> DocumentOptions {
        let mut opts = self.pending.clone();
        opts.fonts
            .extend(fonts.iter().map(std::path::PathBuf::from));
        opts.font_family.extend(font_family.iter().cloned());
        if let Some(p) = default_font {
            opts.font = Some(p.into());
        }
        if let Some(p) = cmyk_profile {
            opts.cmyk_profile = Some(p.to_string());
        }
        opts
    }
}

/// Serialize one response object as a single NDJSON line and flush, so
/// the host reads a complete record per `readLine`.
fn emit(out: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

fn handle(live: &mut Live, req: Request) -> Result<Value> {
    match req {
        Request::Load {
            path,
            fonts,
            font_family,
            cmyk_profile,
            default_font,
        } => {
            let opts = live.options(
                &fonts,
                &font_family,
                cmyk_profile.as_deref(),
                default_font.as_deref(),
            );
            // The one door: registry, then load, then the working
            // colour space — the ordering rule lives in `open`, and
            // this command used to carry its own copy of it.
            let handle = opts.open(&mut live.session, std::path::Path::new(&path))?;
            let page_ids: Vec<String> = handle.page_ids.iter().map(|p| p.0.clone()).collect();
            let resp = json!({
                "ok": true,
                "loaded": path,
                "pageCount": page_ids.len(),
                "pageIds": page_ids,
            });
            live.handle = Some(handle);
            Ok(resp)
        }
        Request::NewBlank {
            width,
            height,
            fonts,
            font_family,
            cmyk_profile,
            default_font,
        } => {
            let opts = live.options(
                &fonts,
                &font_family,
                cmyk_profile.as_deref(),
                default_font.as_deref(),
            );
            opts.register_fonts(&mut live.session)?;
            let font = opts.fallback_font()?;
            let reply = live.session.send(MainToWorkerKind::NewBlankDocument {
                width_pt: width,
                height_pt: height,
                font,
            })?;
            let handle = expect_reply!(reply, WorkerToMainKind::DocumentLoaded(h) => h,
                "new blank document")?;
            let page_ids: Vec<String> = handle.page_ids.iter().map(|p| p.0.clone()).collect();
            let resp = json!({
                "ok": true,
                "pageCount": page_ids.len(),
                "pageIds": page_ids,
            });
            live.handle = Some(handle);
            Ok(resp)
        }
        Request::RegisterFont {
            family,
            style,
            path,
        } => {
            // Recorded as the `--font-family` spec a load would carry,
            // so ONE code path installs fonts. The reply already
            // promised "the next load"; this is that promise held by
            // construction rather than by a parallel registry.
            let spec = match &style {
                Some(st) => format!("{family}/{st}={path}"),
                None => format!("{family}={path}"),
            };
            std::fs::metadata(&path).with_context(|| format!("read font {path}"))?;
            live.pending.font_family.push(spec);
            Ok(json!({
                "ok": true,
                "family": family,
                "style": style,
                // Say the ordering rule in the reply rather than only in
                // the docs: a host that registers after loading gets an
                // unchanged document and no error, which is the one way
                // this can silently do nothing.
                "appliesTo": "the next load / new-blank",
            }))
        }
        Request::RegisterColorProfile { name, path } => {
            std::fs::metadata(&path).with_context(|| format!("read profile {path}"))?;
            live.pending.cmyk_profile = Some(path);
            Ok(json!({
                "ok": true,
                "name": name,
                "appliesTo": "the next load / new-blank",
            }))
        }
        Request::RunScript { source } => {
            // Every write inside `source` funnels through the `paged.*`
            // bridge → `apply_mutation`; budgets (loop/recursion/stack/
            // 2s wall-clock) are the editor's, kept deliberately so a
            // docs example that passes here cannot hang the REPL.
            let result = live
                .session
                .execute_script(&source, paged_script::ScriptBudget::default());
            Ok(json!({ "ok": result.error.is_none(), "result": result }))
        }
        Request::Describe => {
            // The capability catalog for a consumer/LLM: host fns, the id
            // grammar, the settable property paths, and the constraints —
            // generated from the engine's own definitions (no loaded doc
            // needed). `protocol` lets a consumer detect a stale catalog.
            Ok(json!({
                "ok": true,
                "protocol": paged_canvas::channel::PROTOCOL_VERSION.0,
                "catalog": paged_script::api_catalog(),
            }))
        }
        Request::Inspect => {
            let m = live.session.model()?;
            Ok(json!({
                "ok": true,
                "meta": m.document_meta(),
                "pages": m.pages(),
                "sceneTree": m.scene_tree(),
            }))
        }
        Request::Pages => {
            let m = live.session.model()?;
            Ok(json!({ "ok": true, "pages": m.pages() }))
        }
        Request::Digest => {
            let m = live.session.model()?;
            // Per-page display-list digest = the GPU-faithful, backend-
            // agnostic oracle (same display list the WebGPU backend draws).
            // `combined` folds them order-sensitively into one document-
            // level value; `stateHash` is the canonical pre-render scene
            // hash for a second, independent equality signal.
            let mut page_digests = serde_json::Map::new();
            let mut combined: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a basis
            for page_id in m.page_ids() {
                let digest = m.display_list_for_page(page_id).map_or(0, |dl| dl.digest());
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
        Request::Render {
            page,
            dpi,
            out,
            backend,
        } => {
            // Refuse rather than downgrade. Core's native vello-backend
            // is a stub, so answering a `"vello"` request with tiny-skia
            // pixels would report a GPU parity that was never measured.
            if let Some(b) = backend.as_deref() {
                if !matches!(b, "cpu" | "tiny-skia") {
                    return Err(anyhow!(
                        "backend '{b}' is not available headlessly: core's only \
                         headless rasterizer is the CPU/tiny-skia one (it also backs \
                         the fidelity gate). Omit `backend`, or pass \"cpu\"."
                    ));
                }
            }
            let page_id = {
                let m = live.session.model()?;
                resolve_page(m, &page)?
            };
            // Sized the way `paged render` sizes it, which is the way
            // `pdftoppm -r DPI` does — every reference rasterisation in
            // this workspace is produced that way.
            let width_pt = {
                let m = live.session.model()?;
                m.pages()
                    .iter()
                    .find(|p| p.self_id == page_id.0)
                    .map(|p| p.size_pt[0])
                    .ok_or_else(|| anyhow!("page {} has no size", page_id.0))?
            };
            let target_width_px = (width_pt * dpi / 72.0).round().max(1.0) as u32;
            let reply = live.session.send(MainToWorkerKind::RequestSnapshot {
                page_id: page_id.clone(),
                target_width_px,
                dpi: Some(dpi),
            })?;
            let png = expect_reply!(reply, WorkerToMainKind::SnapshotReady(p) => p,
                format!("render page {}", page_id.0))?;
            std::fs::write(&out, &png.png_bytes).with_context(|| format!("write {out}"))?;
            Ok(json!({
                "ok": true,
                "out": out,
                "pageId": page_id.0,
                "widthPx": png.width_px,
                "heightPx": png.height_px,
            }))
        }
        Request::Export {
            format,
            out,
            options,
        } => {
            let bytes = match format.as_str() {
                "idml" => {
                    let reply = live
                        .session
                        .send(MainToWorkerKind::ExportIdml { link_base: None })?;
                    let (bytes, lost) = expect_reply!(reply,
                        WorkerToMainKind::IdmlExported { idml_bytes, lost, .. } => (idml_bytes, lost),
                        "export idml")?;
                    for line in &lost {
                        eprintln!("lost in translation to IDML: {line}");
                    }
                    bytes.into_vec()
                }
                // The container scheme tracks the engine wire protocol;
                // the export kind stamps the binary's own version.
                "paged" => {
                    let reply = live.session.send(MainToWorkerKind::ExportPaged {})?;
                    expect_reply!(reply, WorkerToMainKind::PagedExported { bytes } => bytes.into_vec(),
                        "export paged")?
                }
                "pdf" => export_pdf(&mut live.session, options)?,
                other => {
                    return Err(anyhow!(
                        "unsupported export format '{other}' (idml|paged|pdf)"
                    ))
                }
            };
            let byte_count = bytes.len();
            std::fs::write(&out, &bytes).with_context(|| format!("write {out}"))?;
            Ok(json!({ "ok": true, "out": out, "format": format, "bytes": byte_count }))
        }
        Request::Quit => unreachable!("quit is handled in the main loop"),
    }
}

/// Export the live model to PDF through the same begin/page/finish
/// session the editor's Export dialog drives, so a page that poisons
/// the writer fails here exactly as it does there.
fn export_pdf(session: &mut Session, options: Option<Value>) -> Result<Vec<u8>> {
    let wire: paged_canvas::channel::ExportPdfWireOptions = match options {
        Some(v) => serde_json::from_value(v).context("`options` is not ExportPdfWireOptions")?,
        None => Default::default(),
    };
    let reply = session.send(MainToWorkerKind::ExportPdfBegin { options: wire })?;
    let (id, page_count) = expect_reply!(reply,
        WorkerToMainKind::ExportPdfBegun { session, page_count } => (session, page_count),
        "begin pdf export")?;
    for _ in 0..page_count {
        let reply = session.send(MainToWorkerKind::ExportPdfPage { session: id })?;
        expect_reply!(reply, WorkerToMainKind::ExportPdfProgress { .. } => (),
            "export pdf page")?;
    }
    let reply = session.send(MainToWorkerKind::ExportPdfFinish { session: id })?;
    let bytes = expect_reply!(reply, WorkerToMainKind::PdfExported { pdf_bytes, .. } => pdf_bytes,
        "finish pdf")?;
    Ok(bytes.into_vec())
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
fn resolve_page(model: &paged_canvas::CanvasModel, page: &Value) -> Result<PageId> {
    let ids: Vec<PageId> = model.page_ids().cloned().collect();
    let by_index = |i: usize| {
        ids.get(i)
            .cloned()
            .ok_or_else(|| anyhow!("page index {i} out of range (0..{})", ids.len()))
    };
    match page {
        Value::Number(n) => {
            let i = n
                .as_u64()
                .ok_or_else(|| anyhow!("page index must be a non-negative integer"))?;
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
        _ => Err(anyhow!(
            "`page` must be an index (number) or a page-id string"
        )),
    }
}
