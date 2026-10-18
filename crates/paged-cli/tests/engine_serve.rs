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

//! `paged engine-serve` end to end: the real binary on a port, one stateless
//! call that loads a document, runs a script, renders and exports.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};

fn call(port: u16, body: &Value) -> (u16, Value) {
    let text = body.to_string();
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "POST /run HTTP/1.1\r\nhost: x\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{text}", text.len()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status: u16 = out.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = out.split_once("\r\n\r\n").unwrap().1;
    (status, serde_json::from_str(body).unwrap())
}

#[test]
fn one_call_loads_scripts_renders_and_exports() {
    let dir = std::env::temp_dir().join(format!("paged-engine-serve-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let doc = dir.join("d.paged");
    assert!(Command::new(env!("CARGO_BIN_EXE_paged"))
        .args(["new", "--size", "a5", "--out", doc.to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    let port = 20000 + (std::process::id() % 20000) as u16;
    let mut child = Command::new(env!("CARGO_BIN_EXE_paged"))
        .args(["engine-serve", "--port", &port.to_string()])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let file = b64.encode(std::fs::read(&doc).unwrap());
    let script = r#"
        const p = paged.pages(); const list = typeof p === "string" ? JSON.parse(p) : p;
        paged.insertTextFrame(list[0].selfId, [72, 72, 200, 300]);
        console.log("pages:" + list.length);
    "#;
    let (status, out) = call(
        port,
        &json!({ "file": file, "script": script, "render": { "page": 1 }, "export": true }),
    );
    assert_eq!(status, 200, "{out}");
    assert!(out["error"].is_null(), "{out}");
    assert!(out["output"].to_string().contains("pages:1"));
    assert!(b64
        .decode(out["png"].as_str().unwrap())
        .unwrap()
        .starts_with(b"\x89PNG"));
    let edited = b64.decode(out["file"].as_str().unwrap()).unwrap();
    assert!(edited.starts_with(b"PK"));
    assert_ne!(
        edited,
        std::fs::read(&doc).unwrap(),
        "the script's frame is in the export"
    );

    let (status, _) = call(port, &json!({ "script": "1" }));
    assert_eq!(status, 422, "a call without a document is refused");
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
