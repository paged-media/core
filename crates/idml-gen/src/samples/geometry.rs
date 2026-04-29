//! Phase-0 mega-file: `geometry.idml`.
//!
//! Five A4-portrait pages, each with one filled black rectangle that
//! exercises one `ItemTransform` variant. The page label (carried as
//! `Page.Name`) describes the variant so the diff harness can report
//! "page 3 / 5 — geometry · rect · rotate-45" without an extra
//! sidecar.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{container_xml, fonts_xml, graphic_xml, preferences_xml, styles_xml},
    spread::{write_spread, Spread},
    story::{write_story, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{compose, rotate_deg, scale, translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "geometry";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const RECT_W_PT: f32 = 100.0;
const RECT_H_PT: f32 = 100.0;
const LABEL_W_PT: f32 = 360.0;
const LABEL_H_PT: f32 = 24.0;

struct Variant {
    name: &'static str,
    transform: Matrix,
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "geometry · rect · identity",
            transform: IDENTITY,
        },
        Variant {
            name: "geometry · rect · translate-72-72",
            transform: translate(72.0, 72.0),
        },
        Variant {
            name: "geometry · rect · rotate-45",
            transform: rotate_deg(45.0),
        },
        Variant {
            name: "geometry · rect · scale-2x-1y",
            transform: scale(2.0, 1.0),
        },
        Variant {
            name: "geometry · rect · rotate-30-then-translate-50-50",
            transform: compose(rotate_deg(30.0), translate(50.0, 50.0)),
        },
    ]
}

/// Build the full `Sample` ready for `write_idml`.
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
        let rect_id = self_id(SAMPLE, "Rectangle", seq);

        // Master.
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

        // Story — single label paragraph, anchored to the
        // top-left text frame.
        stories.push((
            story_id.clone(),
            write_story(&Story {
                self_id: story_id.clone(),
                paragraphs: vec![variant.name.to_string()],
            }),
        ));
        story_refs.push(story_id.clone());

        // Page items: one label TextFrame at top-left, one filled
        // black Rectangle that exercises the variant transform.
        let label = Rect {
            self_id: label_frame_id,
            width_pt: LABEL_W_PT,
            height_pt: LABEL_H_PT,
            item_transform: translate(36.0, 36.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(story_id.clone()),
        };
        let demo_rect = Rect {
            self_id: rect_id,
            width_pt: RECT_W_PT,
            height_pt: RECT_H_PT,
            // Apply the variant transform first (rotates / scales /
            // shears the rect around its local origin) and translate
            // afterwards to position the result near the page centre.
            // Doing it the other way around pivots the centered rect
            // around the page origin and flings it off-page for any
            // non-identity rotation.
            item_transform: compose(
                variant.transform,
                translate((PAGE_W_PT - RECT_W_PT) * 0.5, (PAGE_H_PT - RECT_H_PT) * 0.5),
            ),
            fill_color: Some("Color/Black".into()),
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: None,
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
                page_items: vec![label, demo_rect],
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
