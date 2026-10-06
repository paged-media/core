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

//! `paged set` / `paged get` — one property of any object (ADR 132).
//!
//! The CLI's first typed setter. A target is an element address
//! (`rectangle:<id>`, `storyRange:<story>@0..5`), `doc`, or the
//! `VirtualTarget` JSON (`{"kind":"layer","layerId":"ua"}`) until ADR 131's
//! string addresses land; the path is any row of the catalog's
//! `propertySchemas` for that kind; the value is JSON when it parses as
//! JSON (`12`, `true`, `null`, `[0,0,100,100]`) and a string otherwise
//! (`3mm`, `CenterAlign`, `Color/Red`), coerced by the path's schema.
//! The write is `Mutation::Set` over the wire — the same door the editor
//! and a script use.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use paged_canvas::channel::{MainToWorkerKind, Mutation, WorkerToMainKind};
use paged_wire::{ElementId, VirtualTarget};

use crate::engine::Session;
use crate::export::PdfOptions;
use crate::options::DocumentOptions;

/// Parse a target argument.
pub fn parse_target(spec: &str) -> Result<VirtualTarget> {
    let spec = spec.trim();
    if spec == "doc" {
        return Ok(VirtualTarget::Doc);
    }
    if spec.starts_with('{') {
        return serde_json::from_str(spec)
            .with_context(|| format!("{spec:?} is not a target object"));
    }
    ElementId::parse(spec)
        .map(|element_id| VirtualTarget::Element { element_id })
        .ok_or_else(|| {
            anyhow!(
                "{spec:?} is not a target — use an element address (rectangle:<id>), \
                 `doc`, or a target object such as {{\"kind\":\"layer\",\"layerId\":\"ua\"}}"
            )
        })
}

/// The value argument as JSON: parsed when it is JSON, a string otherwise.
pub fn parse_value_json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.to_string()))
}

fn schema_of(
    target: &VirtualTarget,
    path: &str,
) -> Result<paged_introspect::schema::PropertySchema> {
    let family = match target {
        VirtualTarget::Style { family, .. } => Some(family.as_str()),
        _ => None,
    };
    paged_introspect::virtual_paths::schema_for(target.address_kind(), family, path).ok_or_else(
        || {
            anyhow!(
                "{path:?} is not a property of a {} (see `paged describe`, propertySchemas)",
                target.address_kind()
            )
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub fn set(
    doc: &Path,
    assets: &DocumentOptions,
    target: &str,
    path: &str,
    value: &str,
    out: Option<&PathBuf>,
    pdf: &PdfOptions,
) -> Result<()> {
    let target = parse_target(target)?;
    let schema = schema_of(&target, path)?;
    let value = paged_introspect::schema::from_json(&schema, &parse_value_json(value))
        .map_err(|e| anyhow!("{path}: {e}"))?;
    let mut session = Session::new();
    assets.open(&mut session, doc)?;
    let reply = session.send(MainToWorkerKind::Mutate(Mutation::Set {
        address: target.clone(),
        path: path.to_string(),
        value,
    }))?;
    match reply {
        WorkerToMainKind::MutationApplied { .. } => {}
        WorkerToMainKind::MutationFailed { error } => bail!("set {path}: {error}"),
        other => bail!("set {path}: the engine answered {other:?}"),
    }
    let read = session.model()?.get_property(&target, path).ok();
    match out {
        None => {
            eprintln!(
                "{path} set in the loaded document. Nothing saved — pass -o to write the result."
            );
        }
        Some(dest) => {
            let format = crate::export::format_for(dest)?;
            let bytes = crate::export::export_bytes(&mut session, format, pdf)?;
            std::fs::write(dest, &bytes).with_context(|| format!("write {}", dest.display()))?;
        }
    }
    if let Some(v) = read {
        println!("{}", serde_json::to_string(&v)?);
    }
    Ok(())
}

pub fn get(doc: &Path, assets: &DocumentOptions, target: &str, path: &str) -> Result<()> {
    let target = parse_target(target)?;
    let mut session = Session::new();
    assets.open(&mut session, doc)?;
    let value = session
        .model()?
        .get_property(&target, path)
        .map_err(|e| anyhow!(e))?;
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}
