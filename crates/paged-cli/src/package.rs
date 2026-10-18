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

//! `paged package` — InDesign's File ▸ Package: one folder holding the
//! document, every file its images link to, and the fonts it sets in,
//! with a report of what could not be collected.
//!
//! The engine does no I/O (ADR 109), so the collecting happens here:
//! the engine is asked for the IDML with its links pointed into the
//! package's `Links/` folder and for the `fonts`, `links` and `inks`
//! collections; the CLI copies the files. Fonts can only come from the
//! faces this run was given (`--fonts`, `--font-family`, `--font`) —
//! an IDML names its fonts but carries none of them.
//!
//! The folder follows InDesign's own layout: `<name>.idml`, `Links/`,
//! `Document fonts/`, `Instructions.txt` and `<name> Report.txt`. A face
//! whose licence says Restricted (OS/2 `fsType` 2) is not copied and the
//! report says why.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use paged_canvas::channel::{CollectionName, MainToWorkerKind, WorkerToMainKind};
use serde_json::{json, Value};

use crate::engine::Session;
use crate::expect_reply;
use crate::options::DocumentOptions;

/// The folder InDesign's package writes the fonts into.
pub const FONTS_DIR: &str = "Document fonts";
/// The folder InDesign's package writes the links into.
pub const LINKS_DIR: &str = "Links";

pub fn run(
    doc: &Path,
    opts: &DocumentOptions,
    out: &Path,
    instructions: Option<&str>,
    json_out: bool,
) -> Result<()> {
    if out.exists()
        && std::fs::read_dir(out)
            .with_context(|| format!("read {}", out.display()))?
            .next()
            .is_some()
    {
        bail!(
            "{} is not empty — a package goes into a new folder",
            out.display()
        );
    }
    std::fs::create_dir_all(out.join(LINKS_DIR))
        .with_context(|| format!("create {}", out.display()))?;
    let out = out
        .canonicalize()
        .with_context(|| format!("resolve {}", out.display()))?;

    let mut session = Session::new();
    opts.open(&mut session, doc)?;
    let name = doc
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document")
        .to_string();
    let doc_dir = doc
        .canonicalize()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));

    // The document, its links re-pointed at the package's own folder —
    // the spelling InDesign resolves when the package is opened.
    let links_dir = out.join(LINKS_DIR);
    let reply = session.send(MainToWorkerKind::ExportIdml {
        link_base: Some(links_dir.to_string_lossy().into_owned()),
    })?;
    let (idml, lost, links) = expect_reply!(reply,
        WorkerToMainKind::IdmlExported { idml_bytes, lost, links } => (idml_bytes, lost, links),
        "export idml")?;
    let idml_path = out.join(format!("{name}.idml"));
    std::fs::write(&idml_path, idml.as_slice())
        .with_context(|| format!("write {}", idml_path.display()))?;

    let mut link_rows = Vec::new();
    for link in &links {
        let target = links_dir.join(&link.file_name);
        let (status, from) = if link.has_bytes {
            std::fs::write(&target, link.bytes.as_slice())
                .with_context(|| format!("write {}", target.display()))?;
            ("written", None)
        } else {
            match resolve_link(&link.source_uri, &doc_dir) {
                Some(src) if src.is_file() => {
                    std::fs::copy(&src, &target).with_context(|| {
                        format!("copy {} to {}", src.display(), target.display())
                    })?;
                    ("copied", Some(src))
                }
                _ => ("missing", None),
            }
        };
        link_rows.push(json!({
            "fileName": link.file_name,
            "sourceUri": link.source_uri,
            "status": status,
            "from": from.map(|p| p.display().to_string()),
        }));
    }

    // Fonts: every family the document sets in, matched against the
    // faces this run was handed.
    let used = collection(&mut session, CollectionName::Fonts)?;
    let families: BTreeSet<String> = used
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["family"].as_str().map(str::to_string))
        .collect();
    let mut font_rows = Vec::new();
    let mut copied_any = false;
    let available = font_sources(opts)?;
    for family in &families {
        let files: Vec<&paged_canvas::FontFile> =
            available.iter().filter(|f| &f.family == family).collect();
        if files.is_empty() {
            font_rows.push(json!({ "family": family, "status": "missing", "files": [] }));
            continue;
        }
        let mut rows = Vec::new();
        for file in files {
            let file_name = file
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let restricted = file.embedding == Some("restricted");
            if !restricted {
                let dir = out.join(FONTS_DIR);
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join(&file_name), &file.bytes)
                    .with_context(|| format!("write font {file_name}"))?;
                copied_any = true;
            }
            rows.push(json!({
                "file": file_name,
                "from": file.path.display().to_string(),
                "style": file.style,
                "embedding": file.embedding,
                "copied": !restricted,
            }));
        }
        let all_copied = rows.iter().all(|r| r["copied"] == json!(true));
        font_rows.push(json!({
            "family": family,
            "status": if all_copied { "copied" } else { "restricted" },
            "files": rows,
        }));
    }
    if !copied_any {
        let _ = std::fs::remove_dir(out.join(FONTS_DIR));
    }

    let inks = collection(&mut session, CollectionName::Inks)?;
    let report = json!({
        "document": idml_path.display().to_string(),
        "links": link_rows,
        "fonts": font_rows,
        "inks": inks,
        "lost": lost,
    });

    std::fs::write(
        out.join("Instructions.txt"),
        instructions.unwrap_or(
            "Open the .idml in this folder. Its images link into Links/; install \
             the faces in Document fonts/ (or make them available to the \
             application) before opening.\n",
        ),
    )?;
    std::fs::write(
        out.join(format!("{name} Report.txt")),
        report_text(&name, &report),
    )?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report_text(&name, &report));
    }
    Ok(())
}

fn collection(session: &mut Session, name: CollectionName) -> Result<Value> {
    let reply = session.send(MainToWorkerKind::RequestCollection { name })?;
    expect_reply!(reply, WorkerToMainKind::CollectionReply { items, .. } => items,
        "read collection")
}

/// Every face this run was given, with the files they came from:
/// `--fonts` scanned, `--font-family` bound by name, `--font` scanned.
fn font_sources(opts: &DocumentOptions) -> Result<Vec<paged_canvas::FontFile>> {
    let mut files = paged_canvas::font_files_from_paths(&opts.fonts);
    for spec in &opts.font_family {
        let Some((name, path)) = spec.split_once('=') else {
            continue;
        };
        let (family, style) = match name.split_once('/') {
            Some((f, s)) => (f.trim().to_string(), Some(s.trim().to_string())),
            None => (name.trim().to_string(), None),
        };
        let path = PathBuf::from(path);
        let scanned = paged_canvas::font_files_from_paths(std::slice::from_ref(&path));
        let embedding = scanned.first().and_then(|f| f.embedding);
        let bytes =
            std::fs::read(&path).with_context(|| format!("read font {}", path.display()))?;
        files.push(paged_canvas::FontFile {
            path,
            family,
            style,
            embedding,
            bytes,
        });
    }
    if let Some(font) = &opts.font {
        files.extend(paged_canvas::font_files_from_paths(std::slice::from_ref(
            font,
        )));
    }
    // One file answering twice (scanned and bound) is copied once.
    let mut seen = BTreeSet::new();
    files.retain(|f| seen.insert((f.path.clone(), f.family.clone())));
    Ok(files)
}

/// Where a link's file is on this machine: a `file:` URI as the path
/// it spells, anything else relative to the document's folder.
pub fn resolve_link(uri: &str, doc_dir: &Path) -> Option<PathBuf> {
    if uri.is_empty() {
        return None;
    }
    let path = match uri.strip_prefix("file:") {
        Some(rest) => {
            let rest = rest.strip_prefix("//").unwrap_or(rest);
            percent_decode(rest)
        }
        None if uri.contains("://") => return None,
        None => percent_decode(uri),
    };
    let path = PathBuf::from(path);
    Some(if path.is_absolute() {
        path
    } else {
        doc_dir.join(path)
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The report as InDesign's package writes it: a plain text file a
/// printer reads, grouped by what was collected and what was not.
fn report_text(name: &str, report: &Value) -> String {
    let mut s = format!("Package report — {name}\n\n");
    let rows = |key: &str| report[key].as_array().cloned().unwrap_or_default();

    let fonts = rows("fonts");
    s.push_str(&format!("Fonts: {} used\n", fonts.len()));
    for f in &fonts {
        s.push_str(&format!(
            "  {:<32} {}\n",
            f["family"].as_str().unwrap_or(""),
            f["status"].as_str().unwrap_or("")
        ));
        for file in f["files"].as_array().into_iter().flatten() {
            s.push_str(&format!(
                "    {}{}\n",
                file["file"].as_str().unwrap_or(""),
                match file["embedding"].as_str() {
                    Some("restricted") => " — licence restricts copying; not collected",
                    _ => "",
                }
            ));
        }
    }

    let links = rows("links");
    s.push_str(&format!("\nLinks: {}\n", links.len()));
    for l in &links {
        s.push_str(&format!(
            "  {:<32} {}\n",
            l["fileName"].as_str().unwrap_or(""),
            l["status"].as_str().unwrap_or("")
        ));
    }

    let inks = rows("inks");
    s.push_str(&format!("\nSpot inks: {}\n", inks.len()));
    for i in &inks {
        s.push_str(&format!(
            "  {}{}\n",
            i["name"].as_str().unwrap_or(""),
            if i["convertToProcess"] == json!(true) {
                " (converted to process)"
            } else {
                ""
            }
        ));
    }

    let lost = rows("lost");
    if !lost.is_empty() {
        s.push_str(&format!("\nNot carried by IDML: {}\n", lost.len()));
        for l in &lost {
            s.push_str(&format!("  {}\n", l.as_str().unwrap_or("")));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_and_relative_links_resolve() {
        let dir = Path::new("/docs/book");
        assert_eq!(
            resolve_link("file:/Users/me/Links/a%20b.jpg", dir),
            Some(PathBuf::from("/Users/me/Links/a b.jpg"))
        );
        assert_eq!(
            resolve_link("file:///Users/me/c.tif", dir),
            Some(PathBuf::from("/Users/me/c.tif"))
        );
        assert_eq!(
            resolve_link("assets/CODE%201.jpg", dir),
            Some(PathBuf::from("/docs/book/assets/CODE 1.jpg"))
        );
        assert_eq!(resolve_link("https://example.com/x.png", dir), None);
        assert_eq!(resolve_link("", dir), None);
    }
}
