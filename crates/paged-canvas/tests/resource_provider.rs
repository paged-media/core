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

//! C-6 (I-06) — the model side of the image-resource provider channel:
//! claim → compose emits `ResourceTilesNeeded` for a claimed image lacking
//! tiles → submit tiles → re-compose consumes them at the right level →
//! release restores the native whole-image fallback lane. Headless +
//! deterministic (no GPU, no async): the harness drives the
//! needed → submit handshake synchronously. The mip-pick math + LRU
//! eviction are unit-tested in `paged-renderer::resource_provider` and
//! `paged-canvas::resource_tiles`; this proves the model wiring + the
//! assembled display-list output.

use paged_canvas::channel::ProviderTileWire;
use paged_canvas::{CanvasModel, CanvasOptions};
use paged_compose::DisplayCommand;
use std::io::Write;

/// One 792×612 page with a Rectangle `plainR` whose content box is
/// `[50,50,250,250]` (a 200×200 pt box — the image surface we claim).
fn small_idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<idPkg:Spread src="Spreads/Spread_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="plainR" GeometricBounds="50 50 250 250" ItemTransform="1 0 0 1 0 0" FillColor="Color/Red"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    CanvasModel::load("c6", &small_idml(), CanvasOptions::default()).expect("load")
}

/// Count `DisplayCommand::Image` entries across all built pages.
fn image_command_count(m: &CanvasModel) -> usize {
    m.built()
        .pages
        .iter()
        .flat_map(|p| p.list.commands.iter())
        .filter(|c| matches!(c, DisplayCommand::Image { .. }))
        .count()
}

/// An opaque RGBA8 tile (`w*h*4` bytes) at grid origin `[x, y]`.
fn tile(x: u32, y: u32, w: u32, h: u32) -> ProviderTileWire {
    ProviderTileWire {
        x,
        y,
        width: w,
        height: h,
        rgba: vec![128u8; (w * h * 4) as usize].into(),
    }
}

#[test]
fn claim_then_submit_then_release_round_trips_the_assembled_image_lane() {
    let mut m = model();
    // Native build: the rectangle has a solid Red fill, no placed image.
    assert_eq!(image_command_count(&m), 0, "no image lane natively");

    // Claim a 512×512 / 256-tile / 3-level pyramid for `plainR`. At
    // render_scale 1.0 the renderer picks level 0 (512×512 → a 2×2 tile
    // grid: [0,0],[256,0],[0,256],[256,256]).
    m.claim_image_resource(
        "x-paged-image:plainR".to_string(),
        3,
        256,
        512,
        512,
        1, // revision / generation
    )
    .expect("claim + rebuild");
    assert!(m.is_resource_claimed("x-paged-image:plainR"));

    // A cold claim assembled NOTHING (no cached tiles) and recorded the
    // four level-0 tiles as needed at level 0.
    assert_eq!(image_command_count(&m), 0, "cold claim assembles no tiles");
    let needed = m.resource_tiles_needed();
    assert_eq!(needed.len(), 1, "one per-image request");
    let req = &needed[0];
    assert_eq!(req.image_id, "x-paged-image:plainR");
    assert_eq!(req.level, 0, "level 0 at scale 1.0");
    assert_eq!(req.generation, 1);
    let mut got = req.tiles.clone();
    got.sort();
    assert_eq!(
        got,
        vec![[0, 0], [0, 256], [256, 0], [256, 256]],
        "the full level-0 tile grid is requested"
    );

    // Submit the four tiles (the host's reply). Re-compose consumes them:
    // four assembled DisplayCommand::Image entries, and nothing left to
    // request.
    let accepted = m
        .submit_resource_tiles(
            "x-paged-image:plainR",
            0,
            vec![
                tile(0, 0, 256, 256),
                tile(256, 0, 256, 256),
                tile(0, 256, 256, 256),
                tile(256, 256, 256, 256),
            ],
            1,
        )
        .expect("submit + rebuild");
    assert!(accepted, "claim matched, tiles accepted");
    assert_eq!(
        image_command_count(&m),
        4,
        "four cached tiles assembled into the frame as Image commands"
    );
    assert!(
        m.resource_tiles_needed().is_empty(),
        "no tiles missing after the full submit"
    );

    // Release restores the native lane: the assembled image lane drops,
    // the rectangle is back to its solid fill (no Image commands).
    m.release_image_resource("x-paged-image:plainR")
        .expect("release + rebuild");
    assert!(!m.is_resource_claimed("x-paged-image:plainR"));
    assert_eq!(
        image_command_count(&m),
        0,
        "whole-image fallback restored (native frame, no tiles)"
    );
    assert_eq!(m.resource_tile_bytes(), 0, "tile cache freed on release");
}

#[test]
fn coarser_scale_picks_a_higher_mip_level_and_requests_its_grid() {
    let mut m = model();
    // Push a coarse camera scale: 0.25 → 1/0.25 = 4 → log2 = 2 → level 2.
    m.set_resource_render_scale(0.25).expect("scale + rebuild");
    m.claim_image_resource(
        "x-paged-image:plainR".to_string(),
        3, // levels 0..=2
        256,
        512,
        512,
        9,
    )
    .expect("claim + rebuild");

    let needed = m.resource_tiles_needed();
    assert_eq!(needed.len(), 1);
    let req = &needed[0];
    // Level 2 of a 512×512 pyramid is 128×128 → a single 256-tile grid
    // cell at origin [0,0].
    assert_eq!(req.level, 2, "floor(log2(1/0.25)) = 2");
    assert_eq!(req.tiles, vec![[0, 0]], "one tile covers the 128px level");

    // Submitting at the WRONG level (0) leaves the chosen level (2) still
    // missing — the re-compose re-requests level 2.
    let accepted = m
        .submit_resource_tiles("x-paged-image:plainR", 0, vec![tile(0, 0, 256, 256)], 9)
        .expect("submit");
    assert!(accepted);
    assert_eq!(
        image_command_count(&m),
        0,
        "a level-0 tile does not satisfy the level-2 pick"
    );
    assert_eq!(
        m.resource_tiles_needed()[0].level,
        2,
        "level 2 still requested"
    );

    // Submit at the chosen level → it assembles.
    m.submit_resource_tiles("x-paged-image:plainR", 2, vec![tile(0, 0, 128, 128)], 9)
        .expect("submit level 2");
    assert_eq!(image_command_count(&m), 1, "the level-2 tile assembles");
}

#[test]
fn stale_generation_submit_is_ignored() {
    let mut m = model();
    m.claim_image_resource("x-paged-image:plainR".to_string(), 1, 256, 256, 256, 42)
        .expect("claim");
    // generation 41 != claim revision 42 → dropped (no rebuild, no tiles).
    let accepted = m
        .submit_resource_tiles("x-paged-image:plainR", 0, vec![tile(0, 0, 256, 256)], 41)
        .expect("submit returns Ok(false)");
    assert!(!accepted, "stale generation dropped");
    assert_eq!(image_command_count(&m), 0);
    assert_eq!(m.resource_tile_bytes(), 0);
}

#[test]
fn claim_against_an_unknown_frame_assembles_nothing_and_requests_nothing() {
    let mut m = model();
    // No frame `ghost` exists; the claim is stored harmlessly but never
    // splices (no frame to render into), so nothing is requested.
    m.claim_image_resource("x-paged-image:ghost".to_string(), 2, 256, 512, 512, 1)
        .expect("claim");
    assert!(m.is_resource_claimed("x-paged-image:ghost"));
    assert_eq!(image_command_count(&m), 0);
    assert!(
        m.resource_tiles_needed().is_empty(),
        "an unmatched claim requests no tiles"
    );
}
