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

//! `paged://` references: one part of one version of a repository, the way a
//! document points at a dataset (a repository of data parts) or at a part of
//! another document.
//!
//! `paged://[<workspace>.]<repository>/<path>[?ref=<ref>]`. Names are lowercase
//! letters, digits and `-`, so the `.` cannot be ambiguous; a ref may contain
//! `/` (`agent/<run>/<name>`), so it travels in the query, percent-encoded. The
//! default ref is `main`.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagedRef {
    pub workspace: Option<String>,
    pub repository: String,
    pub path: String,
    pub reference: String,
}

fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn decode(s: &str) -> Option<String> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|c| match c {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (c as char).to_string()
            }
            _ => format!("%{c:02X}"),
        })
        .collect()
}

impl PagedRef {
    /// The document part of the reference: `repo` or `workspace/repo`, as the
    /// MCP tools and the server's routes spell it.
    pub fn document(&self) -> String {
        match &self.workspace {
            Some(ws) => format!("{ws}/{}", self.repository),
            None => self.repository.clone(),
        }
    }

    pub fn parse(url: &str) -> Option<PagedRef> {
        let rest = url.strip_prefix("paged://")?;
        let (main, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, path) = main.split_once('/')?;
        let (workspace, repository) = match authority.split_once('.') {
            Some((ws, repo)) => (Some(ws.to_string()), repo.to_string()),
            None => (None, authority.to_string()),
        };
        if !name_ok(&repository) || workspace.as_deref().is_some_and(|w| !name_ok(w)) {
            return None;
        }
        let path = decode(path)?;
        if path.is_empty()
            || path
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == "..")
        {
            return None;
        }
        let mut reference = "main".to_string();
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            if let Some(v) = pair.strip_prefix("ref=") {
                reference = decode(v)?;
            }
        }
        Some(PagedRef {
            workspace,
            repository,
            path,
            reference,
        })
    }
}

impl fmt::Display for PagedRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "paged://")?;
        if let Some(ws) = &self.workspace {
            write!(f, "{ws}.")?;
        }
        let path: Vec<String> = self.path.split('/').map(encode).collect();
        write!(f, "{}/{}", self.repository, path.join("/"))?;
        if self.reference != "main" {
            write!(f, "?ref={}", encode(&self.reference))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PagedRef;

    #[test]
    fn references_round_trip() {
        for url in [
            "paged://products/catalog.parquet",
            "paged://acme.products/data/2026%20Q4.csv?ref=agent%2Fr1%2Fprices",
            "paged://acme.brochure/Stories/Story_u1.xml?ref=releases%2Fv1",
        ] {
            let r = PagedRef::parse(url).unwrap();
            assert_eq!(r.to_string(), url);
        }
        let r = PagedRef::parse("paged://acme.products/data/2026%20Q4.csv?ref=agent%2Fr1%2Fprices")
            .unwrap();
        assert_eq!(r.document(), "acme/products");
        assert_eq!(r.path, "data/2026 Q4.csv");
        assert_eq!(r.reference, "agent/r1/prices");
        assert_eq!(
            PagedRef::parse("paged://products/a.csv").unwrap().reference,
            "main"
        );
    }

    #[test]
    fn bad_references_are_refused() {
        for url in [
            "http://x/y",
            "paged://Products/a",
            "paged://p/../a",
            "paged://p/",
            "paged://a.b.c/x",
            "paged://p",
        ] {
            assert!(PagedRef::parse(url).is_none(), "{url}");
        }
    }
}
