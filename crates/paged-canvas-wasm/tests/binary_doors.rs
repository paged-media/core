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

//! v66 — the binary doors behind the canvas-wasm `…Direct` exports, and
//! `DeletePagedPart`, driven natively through `WorkerCore`.
//!
//! The shell's exports are one-line forwards to these methods (plus the
//! cache effect), so what is pinned here is what a host gets: the reply
//! envelope, the pages whose GPU scene is dropped, and the pixels the
//! next build draws.

use std::io::Write;

use paged_canvas_wasm::dispatch::{CacheEffect, WorkerCore};

fn clock() -> f64 {
    0.0
}

/// Two one-page spreads. Rectangle `r1` sits on the FIRST page only, so a
/// change to its scene image must drop page 0's cached scene and leave
/// page 1's alone.
fn two_page_idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let mut put = |name: &str, body: &str| {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        };
        put("mimetype", "application/vnd.adobe.indesign-idml-package");
        put(
            "META-INF/container.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        );
        put(
            "designmap.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Spread src="Spreads/Spread_s2.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        );
        put(
            "Spreads/Spread_s1.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" GeometricBounds="100 100 300 300" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        );
        put(
            "Spreads/Spread_s2.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s2" PageCount="1">
<Page Self="p2" Name="2" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        );
        zip.finish().unwrap();
    }
    buf
}

fn loaded() -> WorkerCore {
    let mut core = WorkerCore::new();
    let msg = serde_json::json!({
        "seq": 1, "protocol": 0, "kind": "loadDocument",
        "payload": { "bytes": two_page_idml() },
    });
    let (reply, _) = core.handle_message(&msg.to_string(), &clock);
    assert!(
        reply.contains("documentLoaded"),
        "fixture must load: {reply}"
    );
    assert_eq!(core.model.as_ref().unwrap().page_count(), 2);
    core
}

fn json(reply: &paged_canvas::WorkerToMain) -> serde_json::Value {
    serde_json::to_value(reply).unwrap()
}

/// The image the frame's display list draws, as `(width, rgba)`.
fn drawn_image(core: &WorkerCore, page: usize) -> Option<(u32, Vec<u8>)> {
    let m = core.model.as_ref()?;
    let img = m.built().pages.get(page)?.list.images.first()?;
    Some((img.width, img.rgba.to_vec()))
}

fn solid(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
    px.iter()
        .copied()
        .cycle()
        .take((w * h * 4) as usize)
        .collect()
}

#[test]
fn a_scene_image_draws_and_drops_only_its_pages_cached_scene() {
    let mut core = loaded();
    let (reply, effect) = core.submit_scene_image(
        2,
        "r1".into(),
        Some("media.paged.image".into()),
        solid(4, 4, [255, 0, 0, 255]),
        4,
        4,
        (0.0, 0.0, 200.0, 200.0),
    );
    let reply = json(&reply);
    assert_eq!(reply["kind"], "sceneLayerApplied");
    assert_eq!(reply["payload"]["applied"], true);
    assert_eq!(reply["seq"], 2);
    // Page 0 shows the frame; page 1 does not, and keeps its scene. The
    // JSON `SubmitSceneLayer` answers `ClearAll` for the same change.
    assert_eq!(effect, CacheEffect::InvalidatePages(vec![0]));
    // …and says so, so the host repaints that page's tiles only.
    let first = core.model.as_ref().unwrap().built().pages[0].id.clone();
    assert_eq!(reply["payload"]["pageIds"], serde_json::json!([first]));
    let (w, px) = drawn_image(&core, 0).expect("the image is drawn on page 0");
    assert_eq!(w, 4);
    assert_eq!(&px[..4], &[255, 0, 0, 255]);
    assert!(drawn_image(&core, 1).is_none());
}

#[test]
fn a_malformed_or_foreign_scene_image_is_refused() {
    let mut core = loaded();
    // 15 bytes is not 2x2x4.
    let (bad, effect) = core.submit_scene_image(
        2,
        "r1".into(),
        None,
        vec![0; 15],
        2,
        2,
        (0.0, 0.0, 10.0, 10.0),
    );
    assert_eq!(json(&bad)["payload"]["applied"], false);
    assert_eq!(effect, CacheEffect::None);

    let (ok, _) = core.submit_scene_image(
        3,
        "r1".into(),
        Some("media.paged.image".into()),
        solid(2, 2, [1, 2, 3, 255]),
        2,
        2,
        (0.0, 0.0, 10.0, 10.0),
    );
    assert_eq!(json(&ok)["payload"]["applied"], true);
    // C-34 travels with the binary door: another plugin may not replace it.
    let (foreign, _) = core.submit_scene_image(
        4,
        "r1".into(),
        Some("media.paged.sheet".into()),
        solid(2, 2, [9, 9, 9, 255]),
        2,
        2,
        (0.0, 0.0, 10.0, 10.0),
    );
    assert_eq!(json(&foreign)["payload"]["applied"], false);
}

#[test]
fn tiles_patch_the_retained_image_and_nothing_else() {
    let mut core = loaded();
    core.submit_scene_image(
        2,
        "r1".into(),
        Some("media.paged.image".into()),
        solid(4, 4, [0, 0, 0, 255]),
        4,
        4,
        (0.0, 0.0, 200.0, 200.0),
    );
    // One 2x1 tile at (1, 2): two white pixels.
    let (reply, effect) = core.submit_scene_image_tiles(
        3,
        "r1".into(),
        Some("media.paged.image".into()),
        &[1, 2, 2, 1],
        &solid(2, 1, [255, 255, 255, 255]),
    );
    assert_eq!(json(&reply)["payload"]["applied"], true);
    assert_eq!(effect, CacheEffect::InvalidatePages(vec![0]));
    let (_, px) = drawn_image(&core, 0).unwrap();
    let at = |x: usize, y: usize| &px[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
    assert_eq!(at(1, 2), &[255, 255, 255, 255]);
    assert_eq!(at(2, 2), &[255, 255, 255, 255]);
    assert_eq!(at(0, 2), &[0, 0, 0, 255], "outside the tile is untouched");
    assert_eq!(at(3, 2), &[0, 0, 0, 255], "outside the tile is untouched");
    assert_eq!(at(1, 1), &[0, 0, 0, 255], "outside the tile is untouched");
    // The display list shared the buffer, so this patch copied it once
    // inside the engine (copy-on-write), not per tile.
    assert_eq!(core.model.as_ref().unwrap().scene_image_copies(), 1);
}

#[test]
fn a_bad_tile_moves_no_byte() {
    let mut core = loaded();
    core.submit_scene_image(
        2,
        "r1".into(),
        None,
        solid(4, 4, [0, 0, 0, 255]),
        4,
        4,
        (0.0, 0.0, 200.0, 200.0),
    );
    // The second tile runs off the right edge; the first is fine. Neither
    // may land.
    let (reply, effect) = core.submit_scene_image_tiles(
        3,
        "r1".into(),
        None,
        &[0, 0, 1, 1, 3, 0, 2, 1],
        &solid(3, 1, [255, 255, 255, 255]),
    );
    assert_eq!(json(&reply)["payload"]["applied"], false);
    assert_eq!(effect, CacheEffect::None);
    let (_, px) = drawn_image(&core, 0).unwrap();
    assert!(px.chunks(4).all(|p| p == [0, 0, 0, 255]));

    // Wrong byte count, and a frame with no image, are refused too.
    let (short, _) = core.submit_scene_image_tiles(4, "r1".into(), None, &[0, 0, 1, 1], &[1, 2]);
    assert_eq!(json(&short)["payload"]["applied"], false);
    let (none, _) =
        core.submit_scene_image_tiles(5, "nope".into(), None, &[0, 0, 1, 1], &[1, 2, 3, 4]);
    assert_eq!(json(&none)["payload"]["applied"], false);
}

#[test]
fn part_bytes_go_in_and_come_out_without_json() {
    let mut core = loaded();
    let body: Vec<u8> = (0..=255).collect();
    let (reply, _) = core.write_paged_part_bytes(
        2,
        "paged/media.paged.image/px/a.bin".into(),
        Some("media.paged.image".into()),
        body.clone(),
        &clock,
    );
    assert_eq!(json(&reply)["kind"], "pagedPartWritten");
    assert_eq!(
        core.read_paged_part_bytes("paged/media.paged.image/px/a.bin"),
        Some(body)
    );
    assert_eq!(core.read_paged_part_bytes("paged/none"), None);
    // The caller gate is the JSON door's.
    let (refused, _) = core.write_paged_part_bytes(
        3,
        "paged/media.paged.sheet/x".into(),
        Some("media.paged.image".into()),
        vec![1],
        &clock,
    );
    assert_eq!(json(&refused)["kind"], "pagedPartFailed");
}

/// A 1x1 PNG, the smallest bytes `ReplaceImageBytes` will decode.
fn png_1x1() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]
}

#[test]
fn mutate_with_bytes_fills_the_empty_slot_inside_a_batch() {
    let mut core = loaded();
    let batch = serde_json::json!({
        "op": "batch",
        "args": { "ops": [
            { "op": "replaceImageBytes", "args": { "elementId": "r1", "bytes": [] } },
        ] },
    });
    let (reply, _) = core.mutate_with_bytes(2, &batch.to_string(), png_1x1(), &clock);
    let reply = json(&reply);
    assert_eq!(reply["kind"], "mutationApplied", "{reply}");
    let placed = core
        .placed_asset_bytes("r1")
        .expect("the frame now carries inline bytes");
    assert_eq!(placed.3, png_1x1(), "the transferred bytes, byte for byte");

    // No slot: refused rather than applied without the bytes.
    let plain = serde_json::json!({ "op": "replaceImageBytes", "args": { "elementId": "r1" } });
    let (reply, _) = core.mutate_with_bytes(3, &plain.to_string(), png_1x1(), &clock);
    assert_eq!(json(&reply)["kind"], "mutationFailed");
}

#[test]
fn a_deleted_part_leaves_the_saved_container() {
    let mut core = loaded();
    let send = |core: &mut WorkerCore, kind: serde_json::Value| -> serde_json::Value {
        let mut msg = kind;
        msg["protocol"] = serde_json::json!(0);
        let (reply, _) = core.handle_message(&msg.to_string(), &clock);
        serde_json::from_str(&reply).unwrap()
    };
    for (seq, name) in [(2, "keep"), (3, "drop")] {
        let r = send(
            &mut core,
            serde_json::json!({ "seq": seq, "kind": "writePagedPart",
                "payload": { "path": format!("paged/t/{name}"), "bytes": [1, 2] } }),
        );
        assert_eq!(r["kind"], "pagedPartWritten");
    }
    // Save, reload: both parts now ride in the LOADED container — the case
    // an overlay-only delete would resurrect on the next save.
    let saved = send(
        &mut core,
        serde_json::json!({ "seq": 4, "kind": "exportPaged", "payload": {} }),
    );
    let bytes: Vec<u8> = serde_json::from_value(saved["payload"]["bytes"].clone()).unwrap();
    let r = send(
        &mut core,
        serde_json::json!({ "seq": 5, "kind": "loadDocument", "payload": { "bytes": bytes } }),
    );
    assert_eq!(r["kind"], "documentLoaded");

    // A named caller may delete only in its own subtree.
    let refused = send(
        &mut core,
        serde_json::json!({ "seq": 6, "kind": "deletePagedPart",
            "payload": { "path": "paged/t/drop", "caller": "other" } }),
    );
    assert_eq!(refused["kind"], "pagedPartFailed");

    let gone = send(
        &mut core,
        serde_json::json!({ "seq": 7, "kind": "deletePagedPart",
            "payload": { "path": "paged/t/drop", "caller": "t" } }),
    );
    assert_eq!(gone["kind"], "pagedPartDeleted");
    assert_eq!(gone["payload"]["existed"], true);
    let again = send(
        &mut core,
        serde_json::json!({ "seq": 8, "kind": "deletePagedPart",
            "payload": { "path": "paged/t/drop" } }),
    );
    assert_eq!(again["payload"]["existed"], false);

    let listed = send(
        &mut core,
        serde_json::json!({ "seq": 9, "kind": "listPagedParts", "payload": { "prefix": "paged/t/" } }),
    );
    assert_eq!(
        listed["payload"]["paths"],
        serde_json::json!(["paged/t/keep"])
    );
    assert_eq!(core.read_paged_part_bytes("paged/t/drop"), None);

    // The next save leaves it out; the kept part survives.
    let saved = send(
        &mut core,
        serde_json::json!({ "seq": 10, "kind": "exportPaged", "payload": {} }),
    );
    let bytes: Vec<u8> = serde_json::from_value(saved["payload"]["bytes"].clone()).unwrap();
    let zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let names: Vec<&str> = zip.file_names().collect();
    assert!(names.contains(&"paged/t/keep"), "{names:?}");
    assert!(!names.contains(&"paged/t/drop"), "{names:?}");

    // Writing the path again lifts the tombstone.
    let r = send(
        &mut core,
        serde_json::json!({ "seq": 11, "kind": "writePagedPart",
            "payload": { "path": "paged/t/drop", "bytes": [7] } }),
    );
    assert_eq!(r["kind"], "pagedPartWritten");
    assert_eq!(core.read_paged_part_bytes("paged/t/drop"), Some(vec![7]));
}
