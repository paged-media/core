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

//! `paged mcp` end to end: the real binary over stdio, a library on disk, the
//! engine run natively. What Claude Desktop or Claude Code would do with no
//! server: list, open, script, render, save as an agent's version, propose.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Mcp {
    fn spawn(library: &std::path::Path) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_paged"))
            .args(["mcp", "--library"])
            .arg(library)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Mcp {
            child,
            stdin,
            stdout,
            next: 0,
        }
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params }));
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let r = self.rpc("tools/call", json!({ "name": name, "arguments": args }));
        let res = &r["result"];
        if res["isError"] == true {
            return Err(res.to_string());
        }
        Ok(res
            .get("structuredContent")
            .cloned()
            .unwrap_or_else(|| res["content"].clone()))
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn paged(args: &[&str]) {
    let s = Command::new(env!("CARGO_BIN_EXE_paged"))
        .args(args)
        .status()
        .unwrap();
    assert!(s.success(), "paged {args:?}");
}

#[test]
fn a_model_works_on_a_local_document_with_no_server() {
    let dir = std::env::temp_dir().join(format!("paged-mcp-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("brochure.paged");
    let lib = dir.join("library");
    paged(&["new", "--size", "a5", "--out", file.to_str().unwrap()]);
    paged(&[
        "library",
        "import",
        file.to_str().unwrap(),
        "--library",
        lib.to_str().unwrap(),
    ]);

    let mut mcp = Mcp::spawn(&lib);
    let init = mcp.rpc(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "paged (local)");
    mcp.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    let tools = mcp.rpc("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"run_script") && !names.contains(&"list_live_sessions"));

    let docs = mcp.tool("list_documents", json!({})).unwrap();
    assert_eq!(docs["documents"][0]["name"], "brochure");

    mcp.tool("open_document", json!({ "document": "brochure" }))
        .unwrap();
    let script = r#"
        const pages = paged.pages();
        const list = typeof pages === "string" ? JSON.parse(pages) : pages;
        const frame = paged.insertTextFrame(list[0].selfId, [72, 72, 200, 360]);
        const stories = paged.stories();
        const all = typeof stories === "string" ? JSON.parse(stories) : stories;
        paged.insertText(all[all.length - 1].selfId, 0, "Hello from a model");
        console.log(frame);
    "#;
    let ran = mcp.tool("run_script", json!({ "source": script })).unwrap();
    assert!(ran["result"]["error"].is_null(), "{ran}");

    let page = mcp.rpc(
        "tools/call",
        json!({ "name": "render_page", "arguments": { "page": 1 } }),
    );
    let image = &page["result"]["content"][0];
    assert_eq!(image["type"], "image");
    assert!(
        image["data"].as_str().unwrap().starts_with("iVBOR"),
        "a PNG"
    );

    let saved = mcp
        .tool("save_document", json!({ "message": "Add a headline" }))
        .unwrap();
    let branch = saved["ref"].as_str().unwrap().to_string();
    assert!(branch.starts_with("agent/local-"));
    let history = mcp
        .tool("history", json!({ "document": "brochure", "ref": branch }))
        .unwrap();
    assert_eq!(history["versions"][0]["agent"], true);
    assert_eq!(history["versions"][0]["for"], "user:local");
    assert_eq!(
        mcp.tool("history", json!({ "document": "brochure" }))
            .unwrap()["versions"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "main is untouched"
    );

    let refused = mcp.tool("commit_parts", json!({ "document": "brochure", "branch": "main", "message": "x", "parts": [{ "path": "x.txt", "text": "x" }] }));
    assert!(refused.unwrap_err().contains("may not write"));

    let proposed = mcp
        .tool(
            "propose_change",
            json!({ "document": "brochure", "branch": branch, "title": "Headline" }),
        )
        .unwrap();
    assert_eq!(proposed["change_request"]["status"], "open");
    let api = mcp
        .tool("lookup_api", json!({ "query": "insertTextFrame" }))
        .unwrap();
    assert!(api["matches"].as_u64().unwrap() > 0);
    drop(mcp);
    let _ = std::fs::remove_dir_all(&dir);
}
