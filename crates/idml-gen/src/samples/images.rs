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
//! The renderer's `idml-inspect` resolves this to the basename and
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
use crate::geometry::{compose, rotate_deg, translate, Matrix, IDENTITY};
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
}

fn variants() -> Vec<Variant> {
    // Frame is 200×200 pt; native image is 128×128 pt. Crop equation:
    //   image bounds = (frame_left + left, frame_top + top,
    //                   w - left - right, h - top - bottom)
    // so positive crops shrink the displayed image and negative crops
    // grow it past the frame edge.
    vec![
        // None — image at native 128 pt anchored to the frame's
        // top-left. Right + bottom crops collapse the unused space
        // (200 − 128 = 72 pt on each free side).
        Variant {
            name: "images · fit · None",
            fitting: "None",
            crops: (0.0, 0.0, FRAME_W_PT - IMG_NATIVE_PX, FRAME_H_PT - IMG_NATIVE_PX),
            extra_transform: None,
        },
        // FitContentToFrame — image stretched to the entire frame.
        // Crops zero on every side ⇒ image bounds = frame bounds.
        Variant {
            name: "images · fit · FitContentToFrame",
            fitting: "FitContentToFrame",
            crops: (0.0, 0.0, 0.0, 0.0),
            extra_transform: None,
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
        },
    ]
}

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
                link_resource_uri: LINK_URI.to_string(),
                fitting: variant.fitting,
                left_crop: lc,
                top_crop: tc,
                right_crop: rc,
                bottom_crop: bc,
                image_self_id: image_id,
                image_w_pt: IMG_NATIVE_PX,
                image_h_pt: IMG_NATIVE_PX,
            }),
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
