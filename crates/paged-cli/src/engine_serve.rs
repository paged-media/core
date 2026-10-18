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

//! `paged engine-serve`: the engine as a small HTTP service, for paged.server's
//! headless lane (a Cloudflare Container runs this binary; local development
//! runs it on a port). Stateless: each call brings the working document and
//! takes back what the engine made of it, so any instance can answer any call.
//!
//! `POST /run` with JSON `{file, script?, render?: {page, dpi?}, export?}`:
//! `file` is the `.paged` (or `.idml`) as base64; the engine loads it, runs the
//! script, renders a page (1-based) and/or exports `.paged`, and answers
//! `{output, error, png?, file?}` with bytes as base64. `GET /healthz` answers
//! `ok`. Nothing else is served, and nothing is kept between calls.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use anyhow::{anyhow, Context as _, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use crate::options::DocumentOptions;
use crate::session::EngineSession;

/// Calls carry whole documents; anything larger is refused.
const BODY_MAX: usize = 256 * 1024 * 1024;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn respond(stream: &mut TcpStream, status: u16, body: &Value) -> Result<()> {
    let text = body.to_string();
    let reason = if status == 200 { "OK" } else { "Error" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        text.len()
    )?;
    Ok(())
}

/// One call: load, script, render, export, in a fresh session.
pub fn run_call(assets: &DocumentOptions, call: &Value) -> Result<Value> {
    let scratch = std::env::temp_dir().join(format!(
        "paged-engine-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&scratch)?;
    let result = (|| -> Result<Value> {
        let bytes = b64().decode(
            call["file"]
                .as_str()
                .ok_or_else(|| anyhow!("`file` is required"))?,
        )?;
        let ext = if bytes.starts_with(b"PK") {
            "paged"
        } else {
            "idml"
        };
        let input = scratch.join(format!("in.{ext}"));
        std::fs::write(&input, bytes)?;
        let mut engine = EngineSession::new(assets.clone());
        engine.request(json!({ "cmd": "load", "path": input }))?;
        let mut out = json!({ "output": [], "error": null });
        if let Some(source) = call["script"].as_str() {
            let r = engine.request(json!({ "cmd": "run-script", "source": source }))?;
            out["output"] = r["result"]["output"].clone();
            out["error"] = r["result"]["error"].clone();
        }
        if let Some(render) = call.get("render").filter(|r| r.is_object()) {
            let page = render["page"].as_u64().unwrap_or(1).max(1) - 1;
            let png = scratch.join("page.png");
            engine.request(json!({ "cmd": "render", "page": page, "dpi": render["dpi"].as_f64().unwrap_or(96.0), "out": png }))?;
            out["png"] = json!(b64().encode(std::fs::read(&png)?));
        }
        if call["export"] == true {
            let file = scratch.join("out.paged");
            engine.request(json!({ "cmd": "export", "format": "paged", "out": file }))?;
            out["file"] = json!(b64().encode(std::fs::read(&file)?));
        }
        Ok(out)
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn rand_suffix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

fn handle(mut stream: TcpStream, assets: &DocumentOptions) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().context("content-length")?;
            }
        }
    }
    match (method, path) {
        ("GET", "/healthz") => respond(&mut stream, 200, &json!("ok")),
        ("POST", "/run") => {
            if length > BODY_MAX {
                return respond(
                    &mut stream,
                    413,
                    &json!({ "error": "the document is too large" }),
                );
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body)?;
            let call: Value = serde_json::from_slice(&body)?;
            match run_call(assets, &call) {
                Ok(out) => respond(&mut stream, 200, &out),
                Err(e) => respond(&mut stream, 422, &json!({ "error": e.to_string() })),
            }
        }
        _ => respond(
            &mut stream,
            404,
            &json!({ "error": "POST /run or GET /healthz" }),
        ),
    }
}

pub fn run(port: u16, bind: &str, assets: DocumentOptions) -> Result<()> {
    let listener =
        TcpListener::bind((bind, port)).with_context(|| format!("binding {bind}:{port}"))?;
    eprintln!("paged engine on {bind}:{port}");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if let Err(e) = handle(s, &assets) {
                    eprintln!("engine call failed: {e}");
                }
            }
            Err(e) => eprintln!("accept failed: {e}"),
        }
    }
    Ok(())
}
