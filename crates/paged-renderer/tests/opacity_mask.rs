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

//! C-28 opacity masks through the FULL pipeline.
//!
//! The `layers-z` fixture gives one spread, one page, two overlapping
//! 220×220 rectangles in spread coords (72 dpi ⇒ 1 pt = 1 px):
//!
//!   * black at x ∈ [140, 360], y ∈ [240, 460]  — the masked TARGET
//!   * red   at x ∈ [235, 455], y ∈ [335, 555]  — the mask ARTWORK
//!
//! so the overlap is x ∈ [235, 360], y ∈ [335, 460]. The tests install
//! the relation directly on the model (`paged-mutate` is not a
//! renderer dev-dep; the mutate-layer bookkeeping is pinned by
//! `paged-mutate`'s own `kernel_ops` suite) and pin the two things
//! only the renderer owns: the display-list bracket, and the pixels.

use paged_compose::{Color, DisplayCommand};
use paged_model::{FrameRef, OpacityMask, OpacityMaskType};
use paged_renderer::{pipeline, PipelineOptions};

/// The fixture with rectangle 1 installed as rectangle 0's mask.
/// Returns `(document, target_id, mask_id)`.
fn masked_document(mask_type: OpacityMaskType, invert: bool) -> paged_scene::Document {
    let bytes = paged_gen::write_idml(&paged_gen::samples::layers_z::build()).expect("write_idml");
    let mut doc = idml_import::import_idml_doc(&bytes).expect("import");
    let spread = &mut doc.spreads[0].spread;
    assert_eq!(spread.rectangles.len(), 2, "fixture shape changed");
    let target = spread.rectangles[0].self_id.clone().expect("target id");
    let mask = spread.rectangles[1].self_id.clone().expect("mask id");
    spread.opacity_masks.insert(
        target,
        OpacityMask {
            mask_item: mask,
            mask_type,
            invert,
        },
    );
    // The artwork stops painting on its own — exactly what
    // `Operation::ApplyOpacityMask` does to `frames_in_order`.
    spread
        .frames_in_order
        .retain(|r| *r != FrameRef::Rectangle(1));
    doc
}

fn render(doc: &paged_scene::Document) -> image::RgbaImage {
    let opts = PipelineOptions::default();
    let (_built, images) =
        pipeline::render_document(doc, &opts, 72.0, Color::WHITE).expect("render");
    images.into_iter().next().expect("one page")
}

fn is_dark(p: [u8; 4]) -> bool {
    p[0] < 100 && p[1] < 100 && p[2] < 100
}
fn is_paper(p: [u8; 4]) -> bool {
    p[0] > 200 && p[1] > 200 && p[2] > 200
}

/// The emitter brackets the masked item's whole frame-body emission,
/// and the three markers balance on the page.
#[test]
fn a_masked_item_emits_a_balanced_soft_mask_bracket() {
    let doc = masked_document(OpacityMaskType::Alpha, false);
    let opts = PipelineOptions::default();
    let built = pipeline::build_document(&doc, &opts).expect("build_document");
    assert_eq!(built.pages.len(), 1);
    let cmds = &built.pages[0].list.commands;
    let begins = cmds
        .iter()
        .filter(|c| matches!(c, DisplayCommand::BeginSoftMask { .. }))
        .count();
    let mids = cmds
        .iter()
        .filter(|c| matches!(c, DisplayCommand::BeginMaskedContent(_)))
        .count();
    let ends = cmds
        .iter()
        .filter(|c| matches!(c, DisplayCommand::EndSoftMask(_)))
        .count();
    assert_eq!((begins, mids, ends), (1, 1, 1), "one balanced bracket");

    // Order matters: artwork, then content, then close.
    let bi = cmds
        .iter()
        .position(|c| matches!(c, DisplayCommand::BeginSoftMask { .. }))
        .unwrap();
    let mi = cmds
        .iter()
        .position(|c| matches!(c, DisplayCommand::BeginMaskedContent(_)))
        .unwrap();
    let ei = cmds
        .iter()
        .position(|c| matches!(c, DisplayCommand::EndSoftMask(_)))
        .unwrap();
    assert!(bi < mi && mi < ei, "marker order: {bi} < {mi} < {ei}");
    assert!(mi - bi > 1, "the artwork emitted commands into the bracket");
    assert!(ei - mi > 1, "the target emitted commands into the bracket");
}

/// Pixels. Every probe below FAILS if the mask is ignored:
/// unmasked, the target would paint its full 220×220 and the artwork
/// would paint red over the page.
#[test]
fn the_mask_shows_the_overlap_hides_the_rest_and_never_paints_the_artwork() {
    let doc = masked_document(OpacityMaskType::Alpha, false);
    let img = render(&doc);
    let px = |x: u32, y: u32| img.get_pixel(x, y).0;

    // Inside target ∩ inside artwork ⇒ the target paints.
    assert!(
        is_dark(px(300, 400)),
        "overlap must paint the target: {:?}",
        px(300, 400)
    );
    // Inside target, OUTSIDE the artwork ⇒ hidden. Without the mask
    // this is solid black.
    assert!(
        is_paper(px(180, 280)),
        "target outside the mask must be hidden: {:?}",
        px(180, 280)
    );
    // Inside the artwork, outside the target ⇒ paper. This is the
    // probe that proves the ARTWORK ITSELF never paints — an
    // un-consumed mask item would show red here.
    let art_only = px(420, 520);
    assert!(
        is_paper(art_only),
        "the mask artwork must never paint: {art_only:?}"
    );
}

/// `invert` flips the coverage: the overlap hides and the rest shows.
#[test]
fn invert_flips_the_rendered_coverage() {
    let doc = masked_document(OpacityMaskType::Alpha, true);
    let img = render(&doc);
    let px = |x: u32, y: u32| img.get_pixel(x, y).0;
    assert!(
        is_paper(px(300, 400)),
        "inverted: the overlap must hide: {:?}",
        px(300, 400)
    );
    assert!(
        is_dark(px(180, 280)),
        "inverted: the rest of the target must show: {:?}",
        px(180, 280)
    );
}

/// A LUMINOSITY mask over the same geometry reads the artwork's colour
/// instead of its opacity. The artwork is `Color/RGBRed`, whose
/// luminance is ~0.21, so the overlap paints a light grey — visibly
/// neither the full black an alpha mask gives nor the paper an
/// ignored mask gives.
#[test]
fn luminosity_mode_reads_the_artwork_colour() {
    let doc = masked_document(OpacityMaskType::Luminosity, false);
    let img = render(&doc);
    let p = img.get_pixel(300, 400).0;
    assert!(
        p[0] > 150 && p[0] < 235,
        "red artwork (~21% luminance) must give partial coverage, got {p:?}"
    );
    // Outside the artwork it is still fully hidden.
    assert!(is_paper(img.get_pixel(180, 280).0));
}
