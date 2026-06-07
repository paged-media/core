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
//!   * W1.2 stroke STYLES (appended after the baked-PDF pages): a
//!     thick-thin striped style (two parallel rules), a wavy style
//!     (sine ribbon), and a dashed style with a gap colour
//!     (under-stroke second pass).
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
    page_item::{Oval, Polygon, PolygonSubPath, Rect},
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras, preferences_xml,
        styles_xml_with_stroke_styles, ExtraColor, StrokeStyleSpec,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
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
        // InDesign's user-facing built-in stroke styles serialise
        // with a "Canned " prefix on the $ID/-namespaced reference.
        // Verifies the renderer strips the prefix before pattern
        // dispatch.
        Variant {
            name: "strokes · type · canned-dotted",
            overrides: vec![("StrokeType", "StrokeStyle/$ID/Canned Dotted")],
            stroke_weight_pt: None,
            fill_color: None,
        },
        Variant {
            name: "strokes · type · japanese-dots",
            overrides: vec![("StrokeType", "StrokeStyle/$ID/Japanese Dots")],
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
        // ---- W1.2 stroke-STYLE variants ----
        //
        // These land AFTER the original 19 pages so the baked reference
        // PDF (capped at 19 in fidelity-thresholds.json) is unaffected —
        // same append-only pattern as tables / images. They have no
        // reference page and so don't participate in the capped diff.
        //
        // Thick-Thin striped: a custom `<StripedStrokeStyle>` with two
        // stripes (60% top rule, 20% bottom rule). Heavier weight so the
        // two parallel rules are clearly resolved.
        Variant {
            name: "strokes · striped · thick-thin",
            overrides: vec![("StrokeType", STRIPED_STYLE_ID)],
            stroke_weight_pt: Some(14.0),
            fill_color: None,
        },
        // Wavy: a custom `<WavyStrokeStyle>` sampled as a sine ribbon.
        Variant {
            name: "strokes · wavy",
            overrides: vec![("StrokeType", WAVY_STYLE_ID)],
            stroke_weight_pt: Some(10.0),
            fill_color: None,
        },
        // Dashed with a gap colour: the gaps are filled by an under-
        // stroke in the custom style's `GapColor` (cyan), beneath the
        // black dash pattern.
        Variant {
            name: "strokes · dashed · gap-color",
            overrides: vec![("StrokeType", GAP_DASH_STYLE_ID)],
            stroke_weight_pt: Some(8.0),
            fill_color: None,
        },
    ]
}

/// Self-ids for the custom stroke styles the W1.2 variants reference.
const STRIPED_STYLE_ID: &str = "StrokeStyle/ThickThin";
const WAVY_STYLE_ID: &str = "StrokeStyle/Wavy";
const GAP_DASH_STYLE_ID: &str = "StrokeStyle/GapDash";

/// Custom `<…StrokeStyle>` resources for the W1.2 variants.
fn stroke_styles() -> Vec<StrokeStyleSpec> {
    vec![
        StrokeStyleSpec {
            self_id: STRIPED_STYLE_ID,
            name: "Thick Thin",
            kind: "Striped",
            pattern: None,
            gap_color: None,
            gap_tint: None,
            // Two rules: a 60%-weight top rule and a 20%-weight bottom
            // rule, separated by a gap. Fractions of the total weight.
            stripes: &[(0.0, 0.6), (0.8, 0.2)],
            wave_width: None,
            wave_length: None,
        },
        StrokeStyleSpec {
            self_id: WAVY_STYLE_ID,
            name: "Wavy",
            kind: "Wavy",
            pattern: None,
            gap_color: None,
            gap_tint: None,
            stripes: &[],
            // Amplitude = 0.5× weight; wavelength = 2× weight.
            wave_width: Some("0.5"),
            wave_length: Some("2"),
        },
        StrokeStyleSpec {
            self_id: GAP_DASH_STYLE_ID,
            name: "Gap Dash",
            kind: "Dashed",
            pattern: Some("8 6"),
            gap_color: Some("Color/RGBCyan"),
            gap_tint: Some("100"),
            stripes: &[],
            wave_width: None,
            wave_length: None,
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
                page_items: Vec::new(),
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

        let label = Rect {
            self_id: label_frame_id,
            width_pt: LABEL_W_PT,
            height_pt: LABEL_H_PT,
            item_transform: translate(36.0, 36.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(story_id.clone()),
            next_text_frame: None,
            previous_text_frame: None,
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
        };

        // Demo rectangle centred on the page. Baseline: black 6pt
        // stroke, paper fill, identity ItemTransform after centering.
        let demo_transform: Matrix =
            compose_translate((PAGE_W_PT - DEMO_W_PT) * 0.5, (PAGE_H_PT - DEMO_H_PT) * 0.5);
        let demo = Rect {
            self_id: demo_id,
            width_pt: DEMO_W_PT,
            height_pt: DEMO_H_PT,
            item_transform: demo_transform,
            fill_color: Some(variant.fill_color.unwrap_or("Color/Paper").to_string()),
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight_pt: Some(variant.stroke_weight_pt.unwrap_or(6.0)),
            parent_story: None,
            next_text_frame: None,
            previous_text_frame: None,
            extra_attrs: variant
                .overrides
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
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
                page_items: vec![label.into(), demo.into()],
                override_list: Vec::new(),
            }),
        ));
        spread_refs.push(spread_id);
    }

    // ---- W1.5 stroke-ALIGNMENT on closed NON-rect shapes ----
    //
    // Appended after the variant pages (and after the W1.2 style pages),
    // so the baked reference PDF — capped in fidelity-thresholds.json —
    // is unaffected. Each page carries a single heavy-stroked shape with
    // Inside / Outside alignment so the offset outline is visible: an
    // oval (Outside, grows the ellipse) and a polygon octagon (Inside,
    // shrinks the outline). These exercise the renderer's
    // `aligned_outline_path` (oval ellipse offset + polygon miter offset)
    // that flat rects never reached.
    let w15_pages = w15_alignment_pages(variants.len() as u32);
    for (master_id, master_xml, story_id, story_xml, spread_id, spread_xml) in w15_pages {
        master_spreads.push((master_id.clone(), master_xml));
        master_refs.push(master_id);
        stories.push((story_id.clone(), story_xml));
        story_refs.push(story_id);
        spreads.push((spread_id.clone(), spread_xml));
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
        styles_xml: styles_xml_with_stroke_styles(&stroke_styles()),
        preferences_xml: preferences_xml(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}

/// Build the W1.5 stroke-alignment pages (oval + polygon). Returns one
/// `(master_id, master_xml, story_id, story_xml, spread_id, spread_xml)`
/// tuple per page, sequenced after the `base_seq` variant pages so ids
/// stay unique and deterministic.
#[allow(clippy::type_complexity)]
fn w15_alignment_pages(base_seq: u32) -> Vec<(String, Vec<u8>, String, Vec<u8>, String, Vec<u8>)> {
    // (label, shape-builder).
    let specs: [(&str, ShapeKind); 2] = [
        ("strokes · alignment · oval-outside", ShapeKind::OvalOutside),
        (
            "strokes · alignment · polygon-inside",
            ShapeKind::PolygonInside,
        ),
    ];
    let mut out = Vec::with_capacity(specs.len());
    for (i, (label, kind)) in specs.iter().enumerate() {
        let seq = base_seq + i as u32;
        let master_id = self_id(SAMPLE, "MasterSpread", seq);
        let master_page_id = self_id(SAMPLE, "MasterPage", seq);
        let spread_id = self_id(SAMPLE, "Spread", seq);
        let page_id = self_id(SAMPLE, "Page", seq);
        let story_id = self_id(SAMPLE, "Story", seq);
        let label_frame_id = self_id(SAMPLE, "TextFrame", seq);
        let shape_id = self_id(SAMPLE, "Shape", seq);

        let master_xml = write_master(&Master {
            self_id: format!("MasterSpread/{master_id}"),
            page_self_id: master_page_id,
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
        });
        let story_xml = write_story(&Story {
            self_id: story_id.clone(),
            paragraphs: vec![Paragraph::plain(*label)],
        });

        let label_item = Rect {
            self_id: label_frame_id,
            width_pt: LABEL_W_PT,
            height_pt: LABEL_H_PT,
            item_transform: translate(36.0, 36.0),
            fill_color: None,
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: Some(story_id.clone()),
            next_text_frame: None,
            previous_text_frame: None,
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
        };

        // Heavy 18pt stroke so the ±9pt alignment offset is obvious.
        let xform = compose_translate((PAGE_W_PT - DEMO_W_PT) * 0.5, (PAGE_H_PT - DEMO_H_PT) * 0.5);
        let shape_item = kind.build(shape_id, xform);

        let spread_xml = write_spread(&Spread {
            self_id: spread_id.clone(),
            page_self_id: page_id,
            page_name: label.to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: vec![label_item.into(), shape_item],
            override_list: Vec::new(),
        });
        out.push((
            master_id, master_xml, story_id, story_xml, spread_id, spread_xml,
        ));
    }
    out
}

/// Which W1.5 alignment shape a page carries.
enum ShapeKind {
    /// An oval with `OutsideAlignment` — the ellipse outline grows.
    OvalOutside,
    /// An octagon polygon with `InsideAlignment` — the outline shrinks.
    PolygonInside,
}

impl ShapeKind {
    fn build(
        &self,
        self_id: String,
        item_transform: Matrix,
    ) -> crate::builders::page_item::PageItem {
        const W: f32 = DEMO_W_PT;
        const H: f32 = DEMO_H_PT;
        match self {
            ShapeKind::OvalOutside => Oval {
                self_id,
                width_pt: W,
                height_pt: H,
                item_transform,
                fill_color: Some("Color/Paper".to_string()),
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight_pt: Some(18.0),
                extra_attrs: vec![(
                    "StrokeAlignment".to_string(),
                    "OutsideAlignment".to_string(),
                )],
            }
            .into(),
            ShapeKind::PolygonInside => {
                // A regular octagon inscribed in the W×H box, centred.
                let cx = W * 0.5;
                let cy = H * 0.5;
                let rx = W * 0.5;
                let ry = H * 0.5;
                let mut anchors = Vec::with_capacity(8);
                for k in 0..8 {
                    // Start at -22.5° so edges sit flat-ish; clockwise.
                    let a = std::f32::consts::FRAC_PI_8 + k as f32 * std::f32::consts::FRAC_PI_4;
                    anchors.push((cx + rx * a.cos(), cy + ry * a.sin()));
                }
                Polygon {
                    self_id,
                    item_transform,
                    fill_color: Some("Color/Paper".to_string()),
                    stroke_color: Some("Color/Black".to_string()),
                    stroke_weight_pt: Some(18.0),
                    extra_attrs: vec![(
                        "StrokeAlignment".to_string(),
                        "InsideAlignment".to_string(),
                    )],
                    subpaths: vec![PolygonSubPath {
                        anchors,
                        closed: true,
                    }],
                }
                .into()
            }
        }
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
