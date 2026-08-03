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

//! B-18 nested content (InDesign paste-into) — the paged-gen
//! `paste-into` fixture through the full pipeline. The fixture nests a
//! black rectangle (protruding past the container's left edge and over
//! its rounded top-left corner), a black oval, and a story-bearing
//! text frame (protruding far below the container) inside a
//! rounded-corner container rectangle. Pins:
//!
//! * the display list brackets nested-child content with
//!   `PushClip` / `PopClip` (body pass AND story glyph pass);
//! * pixels: children paint inside the container, are masked outside
//!   its path INCLUDING the corner-effect curve, and the nested text
//!   frame's glyphs never escape the container.

use paged_compose::{Color, DisplayCommand};
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

fn read_font(name: &str) -> Vec<u8> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read font fixture {}: {e}", p.display()))
}

fn document() -> paged_scene::Document {
    let bytes =
        paged_gen::write_idml(&paged_gen::samples::paste_into::build()).expect("write_idml");
    idml_import::import_idml_doc(&bytes).expect("Document::open")
}

fn options(resolver: &BytesResolver) -> PipelineOptions<'_> {
    PipelineOptions {
        assets: Some(resolver),
        ..PipelineOptions::default()
    }
}

/// The body pass brackets the children with a container clip, and the
/// story pass splices a second bracket around the nested text frame's
/// glyph range — so the page carries at least two balanced
/// `PushClip` / `PopClip` pairs.
#[test]
fn nested_children_render_inside_clip_brackets() {
    let document = document();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Open Sans", None, read_font("OpenSans.ttf"));
    let opts = options(&resolver);
    let built = pipeline::build_document(&document, &opts).expect("build_document");
    assert_eq!(built.pages.len(), 1);
    let cmds = &built.pages[0].list.commands;
    let pushes = cmds
        .iter()
        .filter(|c| matches!(c, DisplayCommand::PushClip { .. }))
        .count();
    let pops = cmds
        .iter()
        .filter(|c| matches!(c, DisplayCommand::PopClip(_)))
        .count();
    assert_eq!(pushes, pops, "clip brackets balance");
    assert!(
        pushes >= 2,
        "body bracket + glyph bracket expected, got {pushes} PushClip(s)"
    );
}

/// Pixel probes at 72 dpi (1 pt = 1 px). Fixture geometry (spread
/// space): container x ∈ [140, 440], y ∈ [200, 500], corner radius 60;
/// child rect world [100, 400] × [160, 360] (black); child oval world
/// [320, 400] × [380, 460] (black); child text frame world
/// [160, 360] × [360, 660] with a story long enough to reach past the
/// container's bottom edge (y = 500).
#[test]
fn paste_into_children_clip_to_the_container_path() {
    let document = document();
    let mut resolver = BytesResolver::new();
    resolver.add_font("Open Sans", None, read_font("OpenSans.ttf"));
    let opts = options(&resolver);
    let (_built, images) =
        pipeline::render_document(&document, &opts, 72.0, Color::WHITE).expect("render");
    let img = &images[0];
    let px = |x: u32, y: u32| img.get_pixel(x, y).0;
    let is_dark = |p: [u8; 4]| p[0] < 100 && p[1] < 100 && p[2] < 100;
    let is_light = |p: [u8; 4]| p[0] > 200 && p[1] > 200 && p[2] > 200;

    // Inside container ∩ child rect → the child paints.
    assert!(
        is_dark(px(300, 300)),
        "child rect inside: {:?}",
        px(300, 300)
    );
    // Child rect protrudes past the container's LEFT edge → clipped.
    assert!(
        is_light(px(120, 300)),
        "protrusion left of the container must clip away: {:?}",
        px(120, 300)
    );
    // The container's rounded top-left corner: inside its AABB but
    // outside the corner curve → the child clips to the CORNER PATH.
    assert!(
        is_light(px(146, 206)),
        "corner-effect region must clip away: {:?}",
        px(146, 206)
    );
    // Just inside the corner curve → the child paints.
    assert!(
        is_dark(px(210, 270)),
        "inside the corner curve the child paints: {:?}",
        px(210, 270)
    );
    // The second child (oval) paints too.
    assert!(is_dark(px(390, 420)), "child oval: {:?}", px(390, 420));

    // Glyphs of the nested text frame paint INSIDE the container…
    let mut glyph_inside = false;
    for y in (370..490).step_by(2) {
        for x in (175..310).step_by(2) {
            if is_dark(px(x, y)) {
                glyph_inside = true;
                break;
            }
        }
    }
    assert!(
        glyph_inside,
        "the nested story shapes glyphs inside the container"
    );

    // …and NEVER below its bottom edge (y = 500): the whole band the
    // text frame protrudes into must stay paper-white.
    for y in (515..640).step_by(3) {
        for x in (170..350).step_by(3) {
            let p = px(x, y);
            assert!(
                is_light(p),
                "glyph escaped the container clip at ({x}, {y}): {p:?}"
            );
        }
    }
}
