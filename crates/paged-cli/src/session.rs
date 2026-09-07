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
use paged_canvas::{CanvasModel, CanvasOptions, PageId};
use serde::Deserialize;
use serde_json::{json, Value};

/// The default-document id; the session is single-document so the id is
/// cosmetic (surfaces only in diagnostics).
const DOC_ID: &str = "paged-run";

/// Fonts and colour profiles registered for the session.
///
/// They live BESIDE the model, not in it, and are folded into
/// `CanvasOptions` at every load — the same rule `WorkerCore` holds
/// (`dispatch.rs`: the registries survive across loads and are cloned
/// into the options at `LoadDocument`). That is why `register-font`
/// takes effect on the next load rather than the current document: the
/// registry seeds shaping when the document is built, and a later
/// registration cannot retroactively shape text that has already been
/// laid out.
#[derive(Default)]
struct Registries {
    /// Fallback faces for text that names a family nothing answers for.
    fallback: Vec<Vec<u8>>,
    fonts: Vec<paged_canvas::FontEntry>,
    profiles: Vec<paged_canvas::ColorProfileEntry>,
    /// The profile to hand the load directly, when one was named as a
    /// path or resolved from the host's installed set.
    cmyk_bytes: Option<Vec<u8>>,
}

impl Registries {
    fn options(&self) -> CanvasOptions {
        CanvasOptions {
            fonts: self.fallback.clone(),
            font_registry: self.fonts.clone(),
            cmyk_icc_profile: self.cmyk_bytes.clone(),
            color_profiles: self.profiles.clone(),
        }
    }

    /// Fold a `load` / `new-blank` request's inline asset fields in.
    /// Scanned directories first, then explicit `fontFamily` bindings,
    /// so an explicit one replaces a scanned face — `paged`'s rule.
    fn absorb(
        &mut self,
        fonts: &[String],
        font_family: &[String],
        cmyk_profile: Option<&str>,
        default_font: Option<&str>,
    ) -> Result<()> {
        if let Some(path) = default_font {
            let bytes = std::fs::read(path).with_context(|| format!("read font {path}"))?;
            // First entry still wins downstream, so a second call
            // replaces rather than shadows.
            self.fallback.clear();
            self.fallback.push(bytes);
        }
        if !fonts.is_empty() {
            let paths: Vec<std::path::PathBuf> = fonts.iter().map(Into::into).collect();
            for entry in paged_canvas::font_registry_from_paths(&paths) {
                self.add_font(entry);
            }
        }
        for spec in font_family {
            let (name, path) = spec
                .split_once('=')
                .with_context(|| format!("fontFamily wants NAME[/STYLE]=PATH, got {spec:?}"))?;
            let (family, style) = match name.split_once('/') {
                Some((f, st)) => (f.trim().to_string(), Some(st.trim().to_string())),
                None => (name.trim().to_string(), None),
            };
            let bytes = std::fs::read(path).with_context(|| format!("read font {path}"))?;
            self.add_font(paged_canvas::FontEntry {
                family,
                style,
                bytes,
            });
        }
        if let Some(spec) = cmyk_profile {
            let (name, bytes) = resolve_profile(spec)?;
            self.cmyk_bytes = Some(bytes.clone());
            self.add_profile(paged_canvas::ColorProfileEntry { name, bytes });
        }
        Ok(())
    }

    fn add_font(&mut self, entry: paged_canvas::FontEntry) {
        self.fonts
            .retain(|e| !(e.family == entry.family && e.style == entry.style));
        self.fonts.push(entry);
    }

    fn add_profile(&mut self, entry: paged_canvas::ColorProfileEntry) {
        self.profiles.retain(|e| e.name != entry.name);
        self.profiles.push(entry);
    }
}

/// A profile named as a path, or as a name to resolve against the
/// host's installed profiles — the one rule `paged_color::profiles`
/// holds for the whole workspace.
fn resolve_profile(spec: &str) -> Result<(String, Vec<u8>)> {
    let path = std::path::Path::new(spec);
    if path.is_file() {
        let bytes = std::fs::read(path).with_context(|| format!("read profile {spec}"))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| spec.to_string());
        return Ok((name, bytes));
    }
    let bytes = paged_color::profiles::resolve_by_name(spec)
        .with_context(|| format!("no installed CMYK profile named {spec:?}"))?;
    Ok((spec.to_string(), bytes))
}

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
    let mut model: Option<CanvasModel> = None;
    let mut registries = Registries::default();

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
        let resp = handle(&mut model, &mut registries, req)
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

fn handle(model: &mut Option<CanvasModel>, reg: &mut Registries, req: Request) -> Result<Value> {
    match req {
        Request::Load {
            path,
            fonts,
            font_family,
            cmyk_profile,
            default_font,
        } => {
            reg.absorb(
                &fonts,
                &font_family,
                cmyk_profile.as_deref(),
                default_font.as_deref(),
            )?;
            let bytes = std::fs::read(&path).with_context(|| format!("read {path}"))?;
            let m = CanvasModel::load(DOC_ID, &bytes, reg.options())
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
        Request::NewBlank {
            width,
            height,
            fonts,
            font_family,
            cmyk_profile,
            default_font,
        } => {
            reg.absorb(
                &fonts,
                &font_family,
                cmyk_profile.as_deref(),
                default_font.as_deref(),
            )?;
            let m = CanvasModel::new_blank(DOC_ID, width, height, reg.options())
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
        Request::RegisterFont {
            family,
            style,
            path,
        } => {
            let bytes = std::fs::read(&path).with_context(|| format!("read font {path}"))?;
            reg.add_font(paged_canvas::FontEntry {
                family: family.clone(),
                style: style.clone(),
                bytes,
            });
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
            let bytes = std::fs::read(&path).with_context(|| format!("read profile {path}"))?;
            reg.add_profile(paged_canvas::ColorProfileEntry {
                name: name.clone(),
                bytes: bytes.clone(),
            });
            reg.cmyk_bytes = Some(bytes);
            Ok(json!({
                "ok": true,
                "name": name,
                "appliesTo": "the next load / new-blank",
            }))
        }
        Request::RunScript { source } => {
            let m = doc_mut(model)?;
            // Every write inside `source` funnels through the `paged.*`
            // bridge → `apply_mutation`; budgets (loop/recursion/stack/
            // 2s wall-clock) are enforced by `execute_script`'s default.
            let result = paged_script::execute_script(m, &source);
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
        Request::Export {
            format,
            out,
            options,
        } => {
            let m = doc_ref(model)?;
            let bytes = match format.as_str() {
                "idml" => m.export_idml().map_err(|e| anyhow!("export idml: {e}"))?,
                // The container scheme tracks the engine wire protocol;
                // export at the binary's own PROTOCOL_VERSION.
                "paged" => m
                    .export_paged(paged_canvas::channel::PROTOCOL_VERSION.0)
                    .map_err(|e| anyhow!("export paged: {e}"))?,
                "pdf" => export_pdf(m, options)?,
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
fn export_pdf(model: &CanvasModel, options: Option<Value>) -> Result<Vec<u8>> {
    let wire: paged_canvas::channel::ExportPdfWireOptions = match options {
        Some(v) => serde_json::from_value(v).context("`options` is not ExportPdfWireOptions")?,
        None => Default::default(),
    };
    let (mut session, page_count) = paged_canvas::export::CanvasExportSession::begin(model, &wire)
        .map_err(|e| anyhow!("begin pdf export: {e}"))?;
    for _ in 0..page_count {
        session
            .export_next_page()
            .map_err(|e| anyhow!("export pdf page: {e}"))?;
    }
    let finished = session.finish().map_err(|e| anyhow!("finish pdf: {e}"))?;
    Ok(finished.pdf_bytes)
}

/// Borrow the loaded document immutably, or error if none is loaded yet.
fn doc_ref(model: &Option<CanvasModel>) -> Result<&CanvasModel> {
    model
        .as_ref()
        .ok_or_else(|| anyhow!("no document loaded (issue `load` or `new-blank` first)"))
}

/// Borrow the loaded document mutably, or error if none is loaded yet.
fn doc_mut(model: &mut Option<CanvasModel>) -> Result<&mut CanvasModel> {
    model
        .as_mut()
        .ok_or_else(|| anyhow!("no document loaded (issue `load` or `new-blank` first)"))
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
