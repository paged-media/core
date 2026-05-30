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

//! Phase-2 mega-file: `images.idml`.
//!
//! Five A4-portrait pages, each placing the same 128×128 PNG fixture
//! (`corpus/generated-fixtures/checker-128.png`) inside a 200×200 pt
//! rectangle. The variants exercise §4.4 "Page Items — Graphics":
//!
//!   * `FrameFittingOption FittingOnEmptyFrame="None"` — image at
//!     native pixel size, anchored to the top-left of the frame.
//!   * `FitContentToFrame` — image stretched to fill the frame
//!     exactly (anisotropic for non-square frames; square here).
//!   * `FillProportionally` — image scaled to cover the frame; the
//!     overflow axis grows past the frame edge via negative crops.
//!   * `CenterContent` — image at native size, centred in the frame.
//!   * `FitContentToFrame + 30°` — same as fit-to-frame, but the
//!     rectangle's `ItemTransform` rotates 30° around its own
//!     top-left so the renderer composes the image's local rect
//!     with the parent rotation correctly.
//!
//! Asset resolution: the IDML emits `LinkResourceURI="file:checker-128.png"`.
//! The renderer's `paged-inspect` resolves this to the basename and
//! looks it up under any `--links-dir` it was launched with — so the
//! invocation passes `--links-dir corpus/generated-fixtures/`.
//!
//! Note on FrameFittingOption: the renderer (as of this slice) only
//! reads the four crop attributes from the element — the
//! `FittingOnEmptyFrame` enum itself is descriptive, not authoritative.
//! So each variant's *visible behaviour* is driven by the explicit
//! crops we emit here, not the enum string. The string is still
//! emitted for fidelity (and for any future renderer that branches on
//! it).

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PlacedImage, Rect},
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{compose, rotate_deg, scale, skew_x_deg, translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "images";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const FRAME_W_PT: f32 = 200.0;
const FRAME_H_PT: f32 = 200.0;
const LABEL_W_PT: f32 = 420.0;
const LABEL_H_PT: f32 = 24.0;

/// Native pixel size of the fixture PNG. At IDML's implicit 72-DPI
/// canvas one pixel of source maps to one point of frame, so the
/// "no fitting" variants display a 128 pt × 128 pt image inside the
/// 200 pt × 200 pt frame.
const IMG_NATIVE_PX: f32 = 128.0;

/// Fixture URI. The renderer's resolver strips the scheme, takes the
/// basename, and looks it up under `--links-dir` — so this value is
/// stable regardless of where the IDML lives on disk.
const LINK_URI: &str = "file:checker-128.png";

struct Variant {
    name: &'static str,
    /// Enum string for `FittingOnEmptyFrame`. Captured even though
    /// the renderer doesn't currently branch on it, so the IDML
    /// reflects what InDesign would have written for each fit.
    fitting: &'static str,
    /// Per-side crops in pt against a 200×200 pt frame. See module
    /// docs for the per-variant rationale.
    crops: (f32, f32, f32, f32), // (left, top, right, bottom)
    /// Optional extra rotation/transform applied around the frame's
    /// top-left corner *after* the centring translate.
    extra_transform: Option<Matrix>,
    /// Override LinkResourceURI for the variant. None ⇒ the default
    /// checker-128 fixture. The transform-focused variants point at
    /// `corpus/samples/media/photo.webp` via an absolute file URI so
    /// the diff harness picks the asset up without an extra
    /// `--links-dir` flag.
    link_uri: Option<&'static str>,
    /// Native pixel dimensions of the variant's image — used for the
    /// inner `<Image>` PathGeometry. Defaults to `IMG_NATIVE_PX`
    /// (128×128). photo.webp is 2000×1333 pt at native size; the
    /// transform variants pass that.
    image_native_pt: Option<(f32, f32)>,
    /// Optional `<Image ItemTransform>` override. Used by the
    /// transform-focused variants so the image's local transform
    /// composes on top of the frame's. None ⇒ identity.
    image_transform: Option<Matrix>,
    /// Optional `EffectivePpi` x/y pair on the inner `<Image>`. Used
    /// by the low-res variant to advertise a non-72 ppi.
    effective_ppi: Option<(f32, f32)>,
}

fn variants() -> Vec<Variant> {
    // Frame is 200×200 pt; native image is 128×128 pt. Crop equation:
    //   image bounds = (frame_left + left, frame_top + top,
    //                   w - left - right, h - top - bottom)
    // so positive crops shrink the displayed image and negative crops
    // grow it past the frame edge.

    // Helpers for the transform-focused photo.webp variants. The
    // photo's native size is 2000 × 1333 pt — so an Image at 1× would
    // dwarf the 200 × 200 pt frame. The "1to1" baseline scales the
    // image so a single point of frame maps to a single pixel of
    // image (≈ 0.1 × scale). Subsequent variants compose extra
    // scale / rotate / translate on top of that baseline so the
    // *visible* effect is decoupled from the image's intrinsic size.
    const PHOTO_W: f32 = 2000.0;
    const PHOTO_H: f32 = 1333.0;
    const FIT_S: f32 = FRAME_W_PT / PHOTO_W; // ≈ 0.1
    let fit_to_frame = scale(FIT_S, FIT_S);
    vec![
        // None — image at native 128 pt anchored to the frame's
        // top-left. Right + bottom crops collapse the unused space
        // (200 − 128 = 72 pt on each free side).
        Variant {
            name: "images · fit · None",
            fitting: "None",
            crops: (0.0, 0.0, FRAME_W_PT - IMG_NATIVE_PX, FRAME_H_PT - IMG_NATIVE_PX),
            extra_transform: None,
            link_uri: None,
            image_native_pt: None,
            image_transform: None,
            effective_ppi: None,
        },
        // FitContentToFrame — image stretched to the entire frame.
        // Crops zero on every side ⇒ image bounds = frame bounds.
        Variant {
            name: "images · fit · FitContentToFrame",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
            link_uri: None,
            image_native_pt: None,
            image_transform: None,
            effective_ppi: None,
        },
        // FillProportionally — for a 1:1 image inside a 1:1 frame
        // this is identical to fit-to-frame. To keep the variant
        // visually distinct we exercise the negative-crop overflow
        // path: image is scaled up so it overflows by 24 pt on the
        // horizontal axis (left + right). Vertical stays flush.
        Variant {
            name: "images · fit · FillProportionally",
            fitting: "FillProportionally",
            crops: (-24.0, 0.0, -24.0, 0.0),
            extra_transform: None,
            link_uri: None,
            image_native_pt: None,
            image_transform: None,
            effective_ppi: None,
        },
        // CenterContent — image at native size, equal margins on all
        // sides ((200 − 128) / 2 = 36 pt).
        Variant {
            name: "images · fit · CenterContent",
            fitting: "CenterContent",
            crops: (
                (FRAME_W_PT - IMG_NATIVE_PX) * 0.5,
                (FRAME_H_PT - IMG_NATIVE_PX) * 0.5,
                (FRAME_W_PT - IMG_NATIVE_PX) * 0.5,
                (FRAME_H_PT - IMG_NATIVE_PX) * 0.5,
            ),
            extra_transform: None,
            link_uri: None,
            image_native_pt: None,
            image_transform: None,
            effective_ppi: None,
        },
        // Rotated 30° with FitContentToFrame — the frame's
        // ItemTransform rotates around its own origin (top-left),
        // and the image fills that rotated frame. Catches renderers
        // that compose image-local transforms incorrectly with the
        // parent rectangle's transform.
        Variant {
            name: "images · transform · rotated-30",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: Some(rotate_deg(30.0)),
            link_uri: None,
            image_native_pt: None,
            image_transform: None,
            effective_ppi: None,
        },
        // ── transform-on-Image variants (photo.webp) ────────────
        // These exercise the *inner* `<Image ItemTransform>` knob
        // rather than the frame's transform. Each one composes a
        // different scale / translate / rotate / skew on top of the
        // 1:1-fit baseline so the renderer's image-transform path is
        // covered independently from frame placement.
        Variant {
            name: "images · 1to1 · photo-fit",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
            link_uri: Some(PHOTO_URI),
            image_native_pt: Some((PHOTO_W, PHOTO_H)),
            image_transform: Some(fit_to_frame),
            effective_ppi: None,
        },
        // 50 % scale of the photo — image fills only the centre
        // quarter of the frame. The composed inner transform is
        // (fit · 0.5).
        Variant {
            name: "images · scale · 50pct",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
            link_uri: Some(PHOTO_URI),
            image_native_pt: Some((PHOTO_W, PHOTO_H)),
            image_transform: Some(scale(FIT_S * 0.5, FIT_S * 0.5)),
            effective_ppi: None,
        },
        // 200 % scale — image overflows the frame by 100 % on every
        // edge so the visible centre is the photo's middle quarter.
        Variant {
            name: "images · scale · 200pct",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
            link_uri: Some(PHOTO_URI),
            image_native_pt: Some((PHOTO_W, PHOTO_H)),
            image_transform: Some(scale(FIT_S * 2.0, FIT_S * 2.0)),
            effective_ppi: None,
        },
        // Translate the image +50 pt on each axis inside the frame
        // (after the 1:1 fit). The visible photo crops by 50 pt on
        // the top-left and reveals 50 pt of empty frame on the
        // bottom-right.
        Variant {
            name: "images · translate · offset-positive",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
            link_uri: Some(PHOTO_URI),
            image_native_pt: Some((PHOTO_W, PHOTO_H)),
            // compose(scale, translate) — translate then scale per
            // our `compose(a, b) = b ∘ a` semantics; we want the
            // image origin shifted *after* the scale, so the
            // translate value is in frame pt.
            image_transform: Some(compose(fit_to_frame, translate(50.0, 50.0))),
            effective_ppi: None,
        },
        // Symmetric negative translate.
        Variant {
            name: "images · translate · offset-negative",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
            link_uri: Some(PHOTO_URI),
            image_native_pt: Some((PHOTO_W, PHOTO_H)),
            image_transform: Some(compose(fit_to_frame, translate(-50.0, -50.0))),
            effective_ppi: None,
        },
        // 30° rotation on the *image* — distinct from the existing
        // "rotated-30" variant which rotates the *frame*. Composes
        // (fit · rotate) so the rotated photo is still scaled to the
        // frame's pt extent.
        Variant {
            name: "images · rotate · img-30deg",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
            link_uri: Some(PHOTO_URI),
            image_native_pt: Some((PHOTO_W, PHOTO_H)),
            image_transform: Some(compose(fit_to_frame, rotate_deg(30.0))),
            effective_ppi: None,
        },
        // Horizontal skew on the image — exercises a non-orthogonal
        // image-local matrix. tan(15°) ≈ 0.268, so a 200-pt-wide
        // photo slants ~54 pt at the top.
        Variant {
            name: "images · skew · x-15deg",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
            link_uri: Some(PHOTO_URI),
            image_native_pt: Some((PHOTO_W, PHOTO_H)),
            image_transform: Some(compose(fit_to_frame, skew_x_deg(15.0))),
            effective_ppi: None,
        },
        // Combined scale × translate × rotate on the same image.
        // Tests that the renderer composes the inner transform in
        // the right order (the standard left-to-right our `compose`
        // walks).
        Variant {
            name: "images · combo · scale-translate-rotate",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
            link_uri: Some(PHOTO_URI),
            image_native_pt: Some((PHOTO_W, PHOTO_H)),
            image_transform: Some(compose(
                compose(scale(FIT_S * 0.75, FIT_S * 0.75), rotate_deg(15.0)),
                translate(20.0, 20.0),
            )),
            effective_ppi: None,
        },
        // Low-resolution photo — claims EffectivePpi=144 even though
        // the image data is the same. Renderers that use this hint
        // for downsampling decisions will branch differently here.
        Variant {
            name: "images · effective-ppi · low-res-144",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
            link_uri: Some(PHOTO_URI),
            image_native_pt: Some((PHOTO_W, PHOTO_H)),
            image_transform: Some(fit_to_frame),
            effective_ppi: Some((144.0, 144.0)),
        },
    ]
}

/// Absolute path so the renderer's resolver can find photo.webp
/// without an explicit `--links-dir`. Real InDesign exports use
/// `file:` URIs of this same shape.
const PHOTO_URI: &str = "file:/Users/drietsch/idml/corpus/samples/media/photo.webp";

pub fn build() -> Sample {
    let variants = variants();

    let mut master_spreads = Vec::with_capacity(variants.len());
    let mut spreads = Vec::with_capacity(variants.len());
    let mut stories = Vec::with_capacity(variants.len());
    let mut master_refs = Vec::with_capacity(variants.len());
    let mut spread_refs = Vec::with_capacity(variants.len());
    let mut story_refs = Vec::with_capacity(variants.len());

    for (i, variant) in variants.iter().enumerate() {
        let seq = i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let story_id = self_id(SAMPLE, "Story", seq);
        let label_frame_id = self_id(SAMPLE, "TextFrame", seq);
        let frame_id = self_id(SAMPLE, "Rectangle", seq);
        let image_id = self_id(SAMPLE, "Image", seq);

        master_spreads.push((
            master_id.clone(),
            write_master(&Master {
                self_id: format!("MasterSpread/{master_id}"),
                page_self_id: master_page_id.clone(),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
            }),
        ));
        master_refs.push(master_id.clone());

        stories.push((
            story_id.clone(),
            write_story(&Story {
                self_id: story_id.clone(),
                paragraphs: vec![Paragraph::plain(variant.name)],
            }),
        ));
        story_refs.push(story_id.clone());

        // Label TextFrame top-left.
        let label = Rect {
            self_id: label_frame_id,
            width_pt: LABEL_W_PT,
            height_pt: LABEL_H_PT,
            item_transform: translate(36.0, 36.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(story_id.clone()),
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
        };

        // Centre the 200×200 frame on the page, then apply the
        // optional variant transform around the frame's *centre*
        // rather than its origin so a rotated frame stays on-page.
        let cx = (PAGE_W_PT - FRAME_W_PT) * 0.5;
        let cy = (PAGE_H_PT - FRAME_H_PT) * 0.5;
        let frame_transform: Matrix = match variant.extra_transform {
            None => translate(cx, cy),
            Some(m) => {
                // Pivot around the frame centre: shift to centre,
                // rotate, shift back, then translate to page slot.
                // Composed left-to-right so the centre-pivot matrix
                // is applied first.
                let half_w = FRAME_W_PT * 0.5;
                let half_h = FRAME_H_PT * 0.5;
                compose(
                    compose(translate(-half_w, -half_h), m),
                    translate(cx + half_w, cy + half_h),
                )
            }
        };

        let (lc, tc, rc, bc) = variant.crops;
        let frame = Rect {
            self_id: frame_id,
            width_pt: FRAME_W_PT,
            height_pt: FRAME_H_PT,
            item_transform: frame_transform,
            // Paper fill so the empty corners of the "None" /
            // "CenterContent" variants render as white instead of
            // letting the page background bleed through.
            fill_color: Some("Color/Paper".to_string()),
            // Thin black stroke makes the frame edge visible — handy
            // for the rotated variant where you need to see exactly
            // where the frame went.
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(0.5),
            parent_story: None,
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: Some(PlacedImage {
                link_resource_uri: variant
                    .link_uri
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| LINK_URI.to_string()),
                fitting: variant.fitting,
                left_crop: lc,
                top_crop: tc,
                right_crop: rc,
                bottom_crop: bc,
                image_self_id: image_id,
                image_w_pt: variant.image_native_pt.map(|(w, _)| w).unwrap_or(IMG_NATIVE_PX),
                image_h_pt: variant.image_native_pt.map(|(_, h)| h).unwrap_or(IMG_NATIVE_PX),
                image_item_transform: variant.image_transform,
                effective_ppi: variant.effective_ppi,
            }),
            text_wrap: None,
            anchored_setting: None,
        };

        spreads.push((
            spread_id.clone(),
            write_spread(&Spread {
                self_id: spread_id.clone(),
                page_self_id: page_id,
                page_name: variant.name.to_string(),
                applied_master: format!("MasterSpread/{master_id}"),
                page_width_pt: PAGE_W_PT,
                page_height_pt: PAGE_H_PT,
                page_items: vec![label.into(), frame.into()],
            }),
        ));
        spread_refs.push(spread_id);
    }

    let designmap = write_designmap(&DesignMap {
        self_id: "d".to_string(),
        master_spreads: master_refs,
        spreads: spread_refs,
        stories: story_refs,
    });

    // Suppress the unused-import warning when no variant uses IDENTITY.
    let _: Matrix = IDENTITY;

    Sample {
        container_xml: container_xml(),
        designmap_xml: designmap,
        graphic_xml: graphic_xml(),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}
