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

//! The tool catalog: every tool either host may offer, with its schema.
//!
//! Store tools read and write repositories (core ADR 180). Engine tools need a
//! document open in an engine: the user's editor tab (paged.server relays to
//! it) or the native engine (`paged mcp`). Writes by a model are an agent's:
//! they land on `agent/<run>/…` branches and reach `main` only through a change
//! request a human approves, unless the workspace's policy says otherwise.

use serde_json::{json, Value};

use crate::VIEW_URI;

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub read_only: bool,
    /// The result has a view ([`VIEW_URI`]).
    pub view: bool,
}

impl Tool {
    pub fn to_json(&self) -> Value {
        let mut t = json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": { "readOnlyHint": self.read_only, "destructiveHint": false, "openWorldHint": false },
        });
        if self.view {
            t["_meta"] = json!({ "ui": { "resourceUri": VIEW_URI } });
        }
        t
    }
}

pub const INSTRUCTIONS: &str = "paged is a page-layout system (InDesign-class documents: IDML and .paged). \
A document is a repository: versions (commits), branches and releases; a file is a projection of one version. \
Read with describe_document, history, read_part and diff. To change a document, either edit it in an engine \
(open_document, then run_script with paged.* JavaScript — call lookup_api first, the API is not guessable — \
check with render_page, and keep the result with save_document), or write parts directly with commit_parts. \
Your writes land on agent/ branches; a person merges them after review (propose_change). Never claim a change \
reached main unless the tool said so.";

fn schema(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

fn doc() -> Value {
    json!({ "type": "string", "description": "The document: its repository name, or <workspace>/<repository>." })
}

fn reference() -> Value {
    json!({ "type": "string", "description": "A branch, release or version address. Default: main." })
}

/// Tools that read and write repositories.
pub const STORE: &[&str] = &[
    "list_documents",
    "describe_document",
    "read_part",
    "history",
    "diff",
    "create_branch",
    "commit_parts",
    "propose_change",
    "list_changes",
    "download_link",
];
/// Tools that need a document open in an engine.
pub const ENGINE: &[&str] = &[
    "open_document",
    "run_script",
    "render_page",
    "save_document",
];
/// Tools that need no document.
pub const REFERENCE: &[&str] = &["lookup_api"];
/// Tools that reach the user's open editor tabs (paged.server only).
pub const LIVE: &[&str] = &["list_live_sessions"];
/// Tools that read a workspace's data sources (paged.server only).
pub const DATA: &[&str] = &["query_data"];

/// Every tool, in a stable order.
pub fn catalog() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_documents",
            title: "List documents",
            description: "The documents you can read, with when each last changed.",
            input_schema: schema(json!({ "workspace": { "type": "string", "description": "Only this workspace's documents." } }), &[]),
            read_only: true,
            view: false,
        },
        Tool {
            name: "describe_document",
            title: "Describe a document",
            description: "A document's branches and releases, and the parts (files) of one version with their sizes.",
            input_schema: schema(json!({ "document": doc(), "ref": reference() }), &["document"]),
            read_only: true,
            view: true,
        },
        Tool {
            name: "read_part",
            title: "Read a part",
            description: "One part of a version: text parts as text (stories, styles, JSON), others as their size.",
            input_schema: schema(
                json!({ "document": doc(), "path": { "type": "string", "description": "The part's path, e.g. Stories/Story_u1.xml." }, "ref": reference() }),
                &["document", "path"],
            ),
            read_only: true,
            view: false,
        },
        Tool {
            name: "history",
            title: "History",
            description: "The versions of a branch, newest first: who made each (people and agents), when, and why.",
            input_schema: schema(json!({ "document": doc(), "ref": reference() }), &["document"]),
            read_only: true,
            view: true,
        },
        Tool {
            name: "diff",
            title: "Compare versions",
            description: "Which parts differ between two versions or branches.",
            input_schema: schema(
                json!({ "document": doc(), "from": reference(), "to": reference() }),
                &["document", "from", "to"],
            ),
            read_only: true,
            view: false,
        },
        Tool {
            name: "create_branch",
            title: "Start a branch",
            description: "Starts your branch (agent/<run>/<name>) at a version, to write on without touching main.",
            input_schema: schema(
                json!({ "document": doc(), "name": { "type": "string", "description": "A short name: letters, digits, '-'." }, "from": reference() }),
                &["document", "name"],
            ),
            read_only: false,
            view: false,
        },
        Tool {
            name: "commit_parts",
            title: "Write parts",
            description: "Writes parts on your branch as one new version: each part given as text or base64, or deleted.",
            input_schema: schema(
                json!({
                    "document": doc(),
                    "branch": { "type": "string", "description": "Your branch, from create_branch." },
                    "message": { "type": "string", "description": "Why: one line a person will read." },
                    "parts": { "type": "array", "items": schema(json!({
                        "path": { "type": "string" },
                        "text": { "type": "string" },
                        "base64": { "type": "string" },
                        "delete": { "type": "boolean" }
                    }), &["path"]) }
                }),
                &["document", "branch", "message", "parts"],
            ),
            read_only: false,
            view: false,
        },
        Tool {
            name: "propose_change",
            title: "Propose a change",
            description: "Asks for your branch to be merged into main (or another branch). A person reviews and merges it.",
            input_schema: schema(
                json!({ "document": doc(), "branch": { "type": "string" }, "title": { "type": "string" }, "target": reference() }),
                &["document", "branch", "title"],
            ),
            read_only: false,
            view: false,
        },
        Tool {
            name: "list_changes",
            title: "Change requests",
            description: "Open and past change requests, with their approvals.",
            input_schema: schema(json!({ "document": doc() }), &["document"]),
            read_only: true,
            view: true,
        },
        Tool {
            name: "download_link",
            title: "Download link",
            description: "A short-lived link to the .paged file of a version, for a person to open.",
            input_schema: schema(json!({ "document": doc(), "ref": reference() }), &["document"]),
            read_only: true,
            view: false,
        },
        Tool {
            name: "open_document",
            title: "Open in the engine",
            description: "Opens a version in the engine, for run_script and render_page.",
            input_schema: schema(json!({ "document": doc(), "ref": reference() }), &["document"]),
            read_only: true,
            view: false,
        },
        Tool {
            name: "run_script",
            title: "Run a script",
            description: "Runs paged.* JavaScript (Boa: no fetch, files or timers; about 2 s) against the open document. Returns console output, any error, and overset stories. Call lookup_api first.",
            input_schema: schema(json!({ "source": { "type": "string" } }), &["source"]),
            read_only: false,
            view: false,
        },
        Tool {
            name: "render_page",
            title: "Render a page",
            description: "A PNG of one page of the open document, to check the result.",
            input_schema: schema(json!({ "page": { "type": "integer", "minimum": 1, "description": "1-based. Default: 1." } }), &[]),
            read_only: true,
            view: true,
        },
        Tool {
            name: "save_document",
            title: "Save as a version",
            description: "Keeps the open document as a new version on your branch.",
            input_schema: schema(json!({ "message": { "type": "string" } }), &["message"]),
            read_only: false,
            view: false,
        },
        Tool {
            name: "lookup_api",
            title: "Look up the API",
            description: "Searches the engine's capability catalog: paged.* functions, address grammar, settable properties and their types, constraints.",
            input_schema: schema(json!({ "query": { "type": "string" } }), &["query"]),
            read_only: true,
            view: false,
        },
        Tool {
            name: "query_data",
            title: "Query a data source",
            description: "Runs one of a workspace's named, read-only queries against its data source (a database), with text parameters. Owners define the queries; ad-hoc SQL is not accepted.",
            input_schema: schema(
                json!({
                    "workspace": { "type": "string" },
                    "source": { "type": "string", "description": "The data source's name." },
                    "query": { "type": "string", "description": "The named query." },
                    "params": { "type": "array", "items": { "type": "string" } }
                }),
                &["workspace", "source", "query"],
            ),
            read_only: true,
            view: false,
        },
        Tool {
            name: "list_live_sessions",
            title: "Open editor tabs",
            description: "Your editor tabs connected to this server. Engine tools act in the tab you name; you see and can undo every step there.",
            input_schema: schema(json!({}), &[]),
            read_only: true,
            view: false,
        },
    ]
}

/// The catalog entries named in `names`.
pub fn select(names: &[&[&str]]) -> Vec<Tool> {
    let wanted: Vec<&str> = names.iter().flat_map(|n| n.iter().copied()).collect();
    catalog()
        .into_iter()
        .filter(|t| wanted.contains(&t.name))
        .collect()
}
