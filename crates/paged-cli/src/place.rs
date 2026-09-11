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

//! `paged place` — put an image file into a graphic frame.
//!
//! **Why a verb for one mutation.** Through the `WorkerCore::dispatch`
//! door this CLI drives, a document has exactly one image lane:
//! INLINE BYTES. `build_font_resolver` (`paged-canvas/src/model.rs`)
//! installs a font resolver and nothing else, so the
//! `BytesResolver::link_dirs` lane that would serve a
//! `placeImage(uri)` link is never populated — only the standalone
//! `paged-inspect` binary sets it, from its own `--links-dir`. A
//! `placeImage` link therefore resolves to nothing and the frame
//! renders exactly as before, which is documented behaviour
//! (`apply/place_image.rs`: "a uri the resolver can't serve leaves the
//! frame rendering exactly as before") and a `KNOWN` entry in the
//! render sweep, not a defect.
//!
//! That left `paged.replaceImageBytes(frame, number[])` as the only
//! door, and from a script a photo is a JS array literal of a quarter
//! of a million integers. Recreating a 47-photo brochure that way is
//! ~35 MB of generated JavaScript to say "this JPEG goes in that
//! frame".
//!
//! So: a command that reads the file and sends the SAME mutation. No
//! new wire kind, no new resolver, no protocol change — the engine's
//! vocabulary is unchanged and `Session::send` is still the only door
//! this crate mutates through.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use paged_canvas::channel::{MainToWorkerKind, Mutation, WorkerToMainKind};
use paged_mutate::{PropertyPath, Value};
use paged_wire::ElementId;

use crate::engine::Session;
use crate::export::PdfOptions;
use crate::options::DocumentOptions;

/// Formats the engine decodes, from the workspace `image` features.
/// JPEG 2000 is deliberately absent — it is not in that feature list,
/// so a `.jp2` would be accepted here and render nothing. Better to say
/// so at the door than to leave an empty frame and no reason.
const DECODABLE: &[&str] = &["png", "jpg", "jpeg", "webp", "tif", "tiff", "gif", "bmp"];

fn check_decodable(image: &Path) -> Result<()> {
    let ext = image
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if DECODABLE.contains(&ext.as_str()) {
        return Ok(());
    }
    bail!(
        "the engine decodes {} — {} is {}. Transcode it first; \
         placing bytes it cannot decode leaves the frame blank.",
        DECODABLE.join(" / "),
        image.display(),
        if ext.is_empty() {
            "unnamed".to_string()
        } else {
            format!("a .{ext}")
        }
    )
}

/// `a,b,c,d,tx,ty` — the placed image's own transform, in the same
/// order IDML writes an `ItemTransform`.
fn parse_transform(spec: &str) -> Result<[f32; 6]> {
    let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
    if parts.len() != 6 {
        bail!("--transform wants six comma-separated numbers a,b,c,d,tx,ty, got {spec:?}");
    }
    let mut out = [0.0f32; 6];
    for (slot, text) in out.iter_mut().zip(parts) {
        *slot = text
            .parse::<f32>()
            .with_context(|| format!("--transform component {text:?}"))?;
    }
    Ok(out)
}

fn applied(reply: WorkerToMainKind, what: &str) -> Result<()> {
    match reply {
        WorkerToMainKind::MutationApplied { .. } => Ok(()),
        WorkerToMainKind::MutationFailed { error } => Err(anyhow!("{what}: {error}")),
        other => Err(anyhow!("{what}: the engine answered {other:?}")),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    doc: &Path,
    assets: &DocumentOptions,
    frame: &str,
    image: &Path,
    fit: Option<&str>,
    transform: Option<&str>,
    out: Option<&PathBuf>,
    pdf: &PdfOptions,
) -> Result<()> {
    check_decodable(image)?;
    let bytes = std::fs::read(image).with_context(|| format!("read {}", image.display()))?;
    // Parsed before the session opens: a typo in the address should
    // cost a message, not a document load.
    let element_id = ElementId::parse(frame)
        .ok_or_else(|| anyhow!("{frame:?} is not an element address — try `rectangle:<id>`"))?;
    let transform = transform.map(parse_transform).transpose()?;

    let mut session = Session::new();
    assets.open(&mut session, doc)?;

    // The typed frame mutations match on the BARE self id —
    // `CanvasModel::resolve_frame_node_id` keys off it — while
    // `SetElementProperty` below takes the full address. The Boa bridge
    // reconciles the two with its `bare_id` helper and this does the
    // same, so one address form works at both doors. Handing
    // `ReplaceImageBytes` the prefixed form makes the lowering resolve
    // nothing, which surfaces as a misleading
    // `not implemented: Mutation::ReplaceImageBytes`.
    let reply = session.send(MainToWorkerKind::Mutate(Mutation::ReplaceImageBytes {
        element_id: element_id.raw_id().to_string(),
        bytes: Some(bytes.clone().into()),
    }))?;
    applied(reply, "place the image")?;

    // Fit and transform are ordinary properties; they ride the same
    // `SetElementProperty` a script would use, so `place` adds no
    // vocabulary of its own.
    if let Some(fit) = fit {
        let reply = session.send(MainToWorkerKind::Mutate(Mutation::SetElementProperty {
            element_id: element_id.clone(),
            path: PropertyPath::FrameFittingType,
            value: Value::Text(fit.to_string()),
        }))?;
        applied(reply, "set the fitting")?;
    }
    if let Some(t) = transform {
        let reply = session.send(MainToWorkerKind::Mutate(Mutation::SetElementProperty {
            element_id,
            path: PropertyPath::ImageContentTransform,
            value: Value::Transform(Some(t)),
        }))?;
        applied(reply, "set the image transform")?;
    }

    match out {
        // The mutation landed in the loaded model and the process is
        // about to end, which would throw it away. Say so rather than
        // reporting a success the file will not show.
        None => {
            eprintln!(
                "{frame}: {} bytes placed in the loaded document. \
                 Nothing saved — pass -o to write the result.",
                bytes.len()
            );
            Ok(())
        }
        Some(dest) => {
            let format = crate::export::format_for(dest)?;
            let out_bytes = crate::export::export_bytes(&mut session, format, pdf)?;
            std::fs::write(dest, &out_bytes)
                .with_context(|| format!("write {}", dest.display()))?;
            println!(
                "{frame} ← {} ({} bytes) → {} — {format}, {} bytes",
                image.display(),
                bytes.len(),
                dest.display(),
                out_bytes.len()
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transform_is_six_numbers() {
        assert_eq!(
            parse_transform("0.48, 0, 0, 0.48, -1.01, 91.13").expect("parses"),
            [0.48, 0.0, 0.0, 0.48, -1.01, 91.13]
        );
        assert!(parse_transform("1,0,0,1,0").is_err());
        assert!(parse_transform("1,0,0,1,0,x").is_err());
    }

    #[test]
    fn jpeg_2000_is_refused_at_the_door() {
        // The workspace `image` crate is built without a jpeg2000
        // feature, so these bytes would place and render nothing.
        let err = check_decodable(Path::new("/tmp/photo.jp2")).expect_err("refused");
        assert!(err.to_string().contains("Transcode"), "{err}");
        check_decodable(Path::new("/tmp/photo.JPG")).expect("case-insensitive");
    }
}
