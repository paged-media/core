//! Phase-1 mega-file: `strokes-fills.idml`.
//!
//! Pages exercise stroke + fill rendering paths:
//!   * stroke alignment (Inside / Center / Outside)
//!   * end caps (Butt / Round / Projecting)
//!   * end joins (Miter / Round / Bevel)
//!   * built-in dash / dot stroke styles
//!   * custom stroke weights
//!   * solid fills in CMYK + RGB swatches
//!   * tinted fills
//!
//! Each variant lands on its own page with the descriptor in
//! `Page.Name`, so the diff harness can attribute failure to e.g.
//! "page 4 / N — strokes · cap · round" without an extra sidecar.
//!
//! Note: gradients aren't covered yet — they need richer
//! `Resources/Graphic.xml` (gradient swatches + stops). Queued for
//! a follow-up sample.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml, styles_xml,
        ExtraColor,
    },
    spread::{write_spread, Spread},
    story::{write_story, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "strokes-fills";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const DEMO_W_PT: f32 = 200.0;
const DEMO_H_PT: f32 = 100.0;
const LABEL_W_PT: f32 = 360.0;
const LABEL_H_PT: f32 = 24.0;

struct Variant {
    name: &'static str,
    /// Sample-specific overrides applied to a baseline filled-and-stroked
    /// rectangle. The baseline stroke is `Color/Black` weight 6pt, fill
    /// is `Color/Paper`. Each entry replaces or augments those defaults.
    overrides: Vec<(&'static str, &'static str)>,
    /// Optional baseline override — when set, replaces the stroke
    /// weight (used by the heavy-stroke variant).
    stroke_weight_pt: Option<f32>,
    /// Optional fill override — used by the colour-fill + tint
    /// variants.
    fill_color: Option<&'static str>,
}

fn variants() -> Vec<Variant> {
    vec![
        // Stroke alignment.
        Variant {
            name: "strokes · alignment · center",
            overrides: vec![("StrokeAlignment", "CenterAlignment")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        Variant {
            name: "strokes · alignment · inside",
            overrides: vec![("StrokeAlignment", "InsideAlignment")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        Variant {
            name: "strokes · alignment · outside",
            overrides: vec![("StrokeAlignment", "OutsideAlignment")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        // End cap (visible on open stroke ends — for a closed rectangle
        // the cap shows up where the stroke segments butt at corners).
        Variant {
            name: "strokes · cap · butt",
            overrides: vec![("EndCap", "ButtEndCap")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        Variant {
            name: "strokes · cap · round",
            overrides: vec![("EndCap", "RoundEndCap")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        Variant {
            name: "strokes · cap · projecting",
            overrides: vec![("EndCap", "ProjectingEndCap")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        // Join.
        Variant {
            name: "strokes · join · miter",
            overrides: vec![("EndJoin", "MiterEndJoin")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        Variant {
            name: "strokes · join · round",
            overrides: vec![("EndJoin", "RoundEndJoin")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        Variant {
            name: "strokes · join · bevel",
            overrides: vec![("EndJoin", "BevelEndJoin")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        // Built-in dash patterns. The canonical IDML names live in
        // Resources/Graphic.xml as `StrokeStyle/$ID/<name>`.
        Variant {
            name: "strokes · type · solid",
            overrides: vec![("StrokeType", "StrokeStyle/$ID/Solid")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        Variant {
            name: "strokes · type · dashed",
            overrides: vec![("StrokeType", "StrokeStyle/$ID/Dashed")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        Variant {
            name: "strokes · type · dotted",
            overrides: vec![("StrokeType", "StrokeStyle/$ID/Dotted")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        // Heavy stroke — verifies stroke weight scales correctly and
        // that the rasterizer's path-stroke pass widens the line.
        Variant {
            name: "strokes · weight · 12pt",
            overrides: vec![],
            stroke_weight_pt: Some(12.0),
            fill_color: None,
        },
        // Solid fills — built-in `Color/Black` (CMYK) plus a custom
        // RGB swatch declared via the resources extension.
        Variant {
            name: "fills · solid · black",
            overrides: vec![],
            stroke_weight_pt: None,
            fill_color: Some("Color/Black"),
        },
        Variant {
            name: "fills · solid · rgb-cyan",
            overrides: vec![],
            stroke_weight_pt: None,
            fill_color: Some("Color/RGBCyan"),
        },
        // Tinted fill — `FillTint` between 0..100 dilutes the swatch
        // toward paper white. Renderer applies tint after the colour
        // resolves.
        Variant {
            name: "fills · tint · black-50",
            overrides: vec![("FillTint", "50")],
            stroke_weight_pt: None,
            fill_color: Some("Color/Black"),
        },
        Variant {
            name: "fills · tint · black-20",
            overrides: vec![("FillTint", "20")],
            stroke_weight_pt: None,
            fill_color: Some("Color/Black"),
        },
    ]
}

fn extra_colors() -> Vec<ExtraColor> {
    // Custom RGB swatch the `fills · solid · rgb-cyan` variant
    // references. Pinned RGB so renderer + InDesign agree on the
    // CMYK→RGB conversion path doesn't apply here.
    vec![ExtraColor {
        self_id: "Color/RGBCyan".to_string(),
        name: "RGB Cyan".to_string(),
        space: "RGB",
        value: "0 200 220".to_string(),
    }]
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
        let demo_id = self_id(SAMPLE, "Rectangle", seq);

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
                paragraphs: vec![variant.name.to_string()],
            }),
        ));
        story_refs.push(story_id.clone());

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
        };

        // Demo rectangle centred on the page. Baseline: black 6pt
        // stroke, paper fill, identity ItemTransform after centering.
        let demo_transform: Matrix = compose_translate(
            (PAGE_W_PT - DEMO_W_PT) * 0.5,
            (PAGE_H_PT - DEMO_H_PT) * 0.5,
        );
        let demo = Rect {
            self_id: demo_id,
            width_pt: DEMO_W_PT,
            height_pt: DEMO_H_PT,
            item_transform: demo_transform,
            fill_color: Some(variant.fill_color.unwrap_or("Color/Paper").to_string()),
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(variant.stroke_weight_pt.unwrap_or(6.0)),
            parent_story: None,
            extra_attrs: variant
                .overrides
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
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
                page_items: vec![label, demo],
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
        graphic_xml: graphic_xml_with_extras(&extra_colors()),
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

/// IDENTITY translated to (`tx`, `ty`). Helper that stays inside the
/// sample rather than expanding the public geometry surface — this
/// is the only place we want it.
fn compose_translate(tx: f32, ty: f32) -> Matrix {
    let mut m = IDENTITY;
    m[4] = tx;
    m[5] = ty;
    m
}
