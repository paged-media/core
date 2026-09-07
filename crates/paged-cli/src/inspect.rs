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

//! `paged inspect` — what the engine made of a document.
//!
//! Deliberately NOT a second `paged-inspect`: that tool drives the raw
//! pipeline and can resolve external `Links/` off the filesystem, which
//! the canvas model cannot and the editor cannot either. This reports
//! what the CANVAS holds — including a `.paged` container's native
//! model part, which `paged-inspect` never sees — plus the digest that
//! is the workspace's verification oracle.

use std::path::Path;

use anyhow::Result;
use serde_json::json;

use crate::engine::Session;
use crate::options::DocumentOptions;

pub fn run(doc: &Path, opts: &DocumentOptions, as_json: bool) -> Result<()> {
    let mut session = Session::new();
    let handle = opts.open(&mut session, doc)?;
    let model = session.model()?;

    let colour = model.color_settings_state();
    let digests: Vec<String> = handle
        .page_ids
        .iter()
        .map(|id| {
            model
                .display_list_for_page(id)
                .map(|list| format!("{:016x}", list.digest()))
                .unwrap_or_else(|| "-".to_string())
        })
        .collect();

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "file": doc.display().to_string(),
                "protocol": session.protocol().0,
                "pageCount": handle.page_count,
                "pageIds": handle.page_ids,
                "pageSizesPt": handle.page_sizes_pt,
                "cmykProfile": colour.cmyk_profile_name,
                "pageDigests": digests,
            }))?
        );
        return Ok(());
    }

    println!("file        {}", doc.display());
    println!("protocol    {}", session.protocol().0);
    println!("pages       {}", handle.page_count);
    match &colour.cmyk_profile_name {
        Some(name) => println!("cmyk        {name}"),
        // Worth saying out loud: it is the difference between the
        // engine's colours and a print reference's.
        None => println!("cmyk        (none — naive conversion)"),
    }
    for (i, (id, digest)) in handle.page_ids.iter().zip(&digests).enumerate() {
        let size = handle
            .page_sizes_pt
            .get(i)
            .map(|(w, h)| format!("{w:.0} × {h:.0} pt"))
            .unwrap_or_default();
        println!("  page {:>3}  {id}  {size:<16} {digest}", i + 1);
    }
    Ok(())
}
