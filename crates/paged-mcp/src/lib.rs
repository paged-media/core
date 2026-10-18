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

//! The paged MCP surface (Model Context Protocol): the tool catalog every host
//! advertises, and the protocol core that answers JSON-RPC 2.0 messages.
//!
//! Two hosts implement [`Host`]: paged.server's `/mcp` (Streamable HTTP, one
//! JSON response per POST, no server-initiated stream) and `paged mcp` (stdio,
//! newline-delimited JSON, the engine running natively). They advertise the
//! same tools with the same schemas ([`tools::catalog`]); what differs is
//! where a document lives and where the engine runs.
//!
//! The protocol subset is what a tools-and-resources server needs:
//! `initialize`, `notifications/initialized`, `ping`, `tools/list`,
//! `tools/call`, `resources/list`, `resources/read`. Tools whose result has a
//! view declare `_meta.ui.resourceUri` (the MCP Apps extension), and
//! [`VIEW_URI`] is that view.

mod catalog;
pub mod tools;

use std::future::Future;

use serde_json::{json, Value};

pub use catalog::{constraints, lookup};

/// Protocol revisions this core speaks, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

/// The MCP Apps view: one HTML resource rendered next to tool results.
pub const VIEW_URI: &str = "ui://paged/view";
pub const VIEW_MIME: &str = "text/html;profile=mcp-app";

/// One piece of a tool result.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(String),
    /// Base64 image bytes and their media type.
    Image {
        data: String,
        mime: String,
    },
}

/// What a tool call returns. A tool that fails reports it here (`is_error`),
/// so the model reads the reason; protocol errors are for malformed calls.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub content: Vec<Content>,
    pub structured: Option<Value>,
    pub is_error: bool,
}

impl Outcome {
    pub fn ok(text: impl Into<String>) -> Outcome {
        Outcome {
            content: vec![Content::Text(text.into())],
            ..Default::default()
        }
    }

    pub fn json(v: Value) -> Outcome {
        Outcome {
            content: vec![Content::Text(
                serde_json::to_string_pretty(&v).unwrap_or_default(),
            )],
            structured: Some(v),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Outcome {
        Outcome {
            content: vec![Content::Text(text.into())],
            structured: None,
            is_error: true,
        }
    }

    fn to_json(&self) -> Value {
        let content: Vec<Value> = self
            .content
            .iter()
            .map(|c| match c {
                Content::Text(t) => json!({ "type": "text", "text": t }),
                Content::Image { data, mime } => {
                    json!({ "type": "image", "data": data, "mimeType": mime })
                }
            })
            .collect();
        let mut out = json!({ "content": content, "isError": self.is_error });
        if let Some(s) = &self.structured {
            out["structuredContent"] = s.clone();
        }
        out
    }
}

/// A place the tools run. Calls are `async` so a server host can await its
/// storage; a stdio host's futures are ready at once.
#[allow(async_fn_in_trait)]
pub trait Host {
    /// Who this host is, for `serverInfo`.
    fn name(&self) -> &str;
    /// The tools this host offers: the catalog, or a subset of it.
    fn tools(&self) -> Vec<tools::Tool>;
    async fn call(&self, tool: &str, args: Value) -> Outcome;
    /// The HTML of [`VIEW_URI`], when the host serves the view.
    fn view(&self) -> Option<String> {
        None
    }
    /// Read once, at `initialize`, for the model.
    fn instructions(&self) -> String {
        tools::INSTRUCTIONS.to_string()
    }
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn result(id: &Value, r: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": r })
}

/// Answers one JSON-RPC message, or nothing for a notification.
pub async fn handle(host: &impl Host, msg: &Value) -> Option<Value> {
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return Some(error(&Value::Null, -32600, "not a JSON-RPC request"));
    };
    let id = msg.get("id")?.clone();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let version = PROTOCOL_VERSIONS
                .iter()
                .find(|v| **v == asked)
                .unwrap_or(&PROTOCOL_VERSIONS[0]);
            let mut capabilities = json!({ "tools": { "listChanged": false } });
            if host.view().is_some() {
                capabilities["resources"] = json!({ "listChanged": false });
            }
            result(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": capabilities,
                    "serverInfo": { "name": host.name(), "version": env!("CARGO_PKG_VERSION") },
                    "instructions": host.instructions(),
                }),
            )
        }
        "ping" => result(&id, json!({})),
        "tools/list" => {
            let list: Vec<Value> = host.tools().iter().map(tools::Tool::to_json).collect();
            result(&id, json!({ "tools": list }))
        }
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Some(error(&id, -32602, "tools/call needs a name"));
            };
            if !host.tools().iter().any(|t| t.name == name) {
                return Some(error(&id, -32602, &format!("no tool {name}")));
            }
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            result(&id, host.call(name, args).await.to_json())
        }
        "resources/list" => {
            let list = match host.view() {
                Some(_) => json!([{ "uri": VIEW_URI, "name": "paged", "mimeType": VIEW_MIME,
                                    "description": "Pages, history and changes of a paged document" }]),
                None => json!([]),
            };
            result(&id, json!({ "resources": list }))
        }
        "resources/read" => match (params["uri"].as_str(), host.view()) {
            (Some(VIEW_URI), Some(html)) => result(
                &id,
                json!({ "contents": [{ "uri": VIEW_URI, "mimeType": VIEW_MIME, "text": html }] }),
            ),
            _ => error(&id, -32002, "no such resource"),
        },
        other => error(&id, -32601, &format!("no method {other}")),
    })
}

/// Drives a future whose host is synchronous to completion (the stdio host's
/// calls never wait on anything).
pub fn block_on<F: Future>(f: F) -> F::Output {
    use std::pin::pin;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    // SAFETY: the vtable's functions ignore the data pointer.
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// The stdio transport: one JSON-RPC message per line in, one per line out.
pub fn serve_stdio(host: &impl Host) -> std::io::Result<()> {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => block_on(handle(host, &msg)),
            Err(e) => Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = reply {
            serde_json::to_writer(&mut out, &r)?;
            out.write_all(b"\n")?;
            out.flush()?;
        }
    }
    Ok(())
}
