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

use paged_mcp::{block_on, handle, lookup, tools, Host, Outcome, VIEW_URI};
use serde_json::{json, Value};

struct Echo;

impl Host for Echo {
    fn name(&self) -> &str {
        "test"
    }
    fn tools(&self) -> Vec<tools::Tool> {
        tools::select(&[tools::STORE, tools::REFERENCE])
    }
    async fn call(&self, tool: &str, args: Value) -> Outcome {
        Outcome::json(json!({ "tool": tool, "args": args }))
    }
    fn view(&self) -> Option<String> {
        Some("<p>view</p>".into())
    }
}

fn rpc(method: &str, params: Value) -> Value {
    block_on(handle(
        &Echo,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
    ))
    .unwrap()
}

#[test]
fn initialize_negotiates_a_version_we_speak() {
    let r = rpc(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
    );
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    assert!(r["result"]["capabilities"]["resources"].is_object());
    let r = rpc("initialize", json!({ "protocolVersion": "1999-01-01" }));
    assert_eq!(
        r["result"]["protocolVersion"],
        paged_mcp::PROTOCOL_VERSIONS[0]
    );
}

#[test]
fn notifications_get_no_answer() {
    let n = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert_eq!(block_on(handle(&Echo, &n)), None);
}

#[test]
fn tools_list_carries_schemas_and_the_view() {
    let r = rpc("tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), tools::STORE.len() + tools::REFERENCE.len());
    let describe = tools
        .iter()
        .find(|t| t["name"] == "describe_document")
        .unwrap();
    assert_eq!(describe["_meta"]["ui"]["resourceUri"], VIEW_URI);
    assert_eq!(describe["inputSchema"]["required"][0], "document");
    assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
}

#[test]
fn a_call_reaches_the_host_and_an_unknown_tool_does_not() {
    let r = rpc(
        "tools/call",
        json!({ "name": "history", "arguments": { "document": "d" } }),
    );
    assert_eq!(r["result"]["structuredContent"]["args"]["document"], "d");
    assert_eq!(r["result"]["isError"], false);
    let r = rpc(
        "tools/call",
        json!({ "name": "run_script", "arguments": {} }),
    );
    assert_eq!(
        r["error"]["code"], -32602,
        "a tool this host does not offer"
    );
    let r = rpc("no/such", json!({}));
    assert_eq!(r["error"]["code"], -32601);
}

#[test]
fn the_view_is_a_resource() {
    let r = rpc("resources/read", json!({ "uri": VIEW_URI }));
    assert_eq!(r["result"]["contents"][0]["mimeType"], paged_mcp::VIEW_MIME);
    assert_eq!(
        rpc("resources/read", json!({ "uri": "ui://other" }))["error"]["code"],
        -32002
    );
}

#[test]
fn lookup_finds_host_functions_first() {
    let r = lookup("paged.set", 5);
    assert!(r["matches"].as_u64().unwrap() > 0);
    assert_eq!(r["results"][0]["section"], "hostFunctions");
    assert_eq!(lookup("zzqqxx-nothing", 5)["matches"], 0);
    assert!(paged_mcp::constraints()
        .as_array()
        .is_some_and(|c| !c.is_empty()));
}
