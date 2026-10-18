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

//! `lookup_api`: a search over the engine's capability catalog
//! (`paged-introspect/catalog.json`, the same contract every surface is
//! generated from — ADR 019), embedded so a host needs no engine to answer.

use serde_json::{json, Value};

const CATALOG: &str = include_str!("../../paged-introspect/catalog.json");

/// The catalog entries that best match `query`, at most `limit`, as
/// `{section, entry}`. Every word must appear; names count more than prose.
pub fn lookup(query: &str, limit: usize) -> Value {
    let catalog: Value = serde_json::from_str(CATALOG).expect("the catalog is JSON");
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '.' && c != '_')
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut hits: Vec<(usize, Value)> = Vec::new();
    if let Some(sections) = catalog.as_object() {
        for (section, entries) in sections {
            for entry in entries.as_array().into_iter().flatten() {
                let text = entry.to_string().to_lowercase();
                if words.is_empty() || !words.iter().all(|w| text.contains(w.as_str())) {
                    continue;
                }
                let name = entry
                    .get("name")
                    .or_else(|| entry.get("form"))
                    .or_else(|| entry.get("path"))
                    .map(Value::to_string)
                    .unwrap_or_else(|| text.clone())
                    .to_lowercase();
                let score = words.iter().filter(|w| name.contains(w.as_str())).count() * 10
                    + usize::from(section == "hostFunctions") * 3
                    + 1000 / (text.len() + 10);
                hits.push((score, json!({ "section": section, "entry": entry })));
            }
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    let found: Vec<Value> = hits.into_iter().take(limit).map(|(_, v)| v).collect();
    json!({ "query": query, "matches": found.len(), "results": found })
}

/// The scripting constraints, always worth reading before a first script.
pub fn constraints() -> Value {
    let catalog: Value = serde_json::from_str(CATALOG).expect("the catalog is JSON");
    catalog["constraints"].clone()
}
