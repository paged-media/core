//! Phase-1 mega-file: `gradients.idml`.
//!
//! Pages exercise gradient fill rendering — the renderer's
//! gradient-paint path that prior samples never touched. Each variant
//! defines a Gradient swatch in `Resources/Graphic.xml` and
//! references it from a Rectangle's `FillColor`.
//!
//! Variants:
//!   * Linear gradient · 2 stops · CMYK black → paper
//!   * Linear gradient · 3 stops · cyan → magenta → yellow
//!   * Radial gradient · 2 stops · paper → black
//!   * Linear gradient · 2 stops · RGB cyan → magenta
//!   * Linear gradient · 4 stops · evenly spaced rainbow

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::Rect,
    resources::{
        container_xml, fonts_xml, graphic_xml_with_extras_and_gradients, preferences_xml,
        styles_xml, ExtraColor, ExtraGradient, GradientStop,
    },
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::{translate, Matrix, IDENTITY};
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "gradients";
const PAGE_W_PT: f32 = 595.276;
const PAGE_H_PT: f32 = 841.890;
const DEMO_W_PT: f32 = 360.0;
const DEMO_H_PT: f32 = 200.0;
const LABEL_W_PT: f32 = 360.0;
const LABEL_H_PT: f32 = 24.0;

struct Variant {
    name: &'static str,
    /// `Self` id of the gradient swatch this variant fills with —
    /// matched against the entries declared by `gradient_swatches()`.
    fill_gradient: &'static str,
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            name: "gradients · linear · cmyk-black-to-paper",
            fill_gradient: "Gradient/BlackToPaper",
        },
        Variant {
            name: "gradients · linear · cmy-3-stops",
            fill_gradient: "Gradient/CMY3",
        },
        Variant {
            name: "gradients · radial · paper-to-black",
            fill_gradient: "Gradient/RadialPaperToBlack",
        },
        Variant {
            name: "gradients · linear · rgb-cyan-to-magenta",
            fill_gradient: "Gradient/RGBCyanMagenta",
        },
        Variant {
            name: "gradients · linear · 4-stops",
            fill_gradient: "Gradient/FourStops",
        },
    ]
}

fn extra_colors() -> Vec<ExtraColor> {
    vec![
        ExtraColor {
            self_id: "Color/CMYKCyan".to_string(),
            name: "CMYK Cyan".to_string(),
            space: "CMYK",
            value: "100 0 0 0".to_string(),
        },
        ExtraColor {
            self_id: "Color/CMYKMagenta".to_string(),
            name: "CMYK Magenta".to_string(),
            space: "CMYK",
            value: "0 100 0 0".to_string(),
        },
        ExtraColor {
            self_id: "Color/CMYKYellow".to_string(),
            name: "CMYK Yellow".to_string(),
            space: "CMYK",
            value: "0 0 100 0".to_string(),
        },
        ExtraColor {
            self_id: "Color/RGBCyan".to_string(),
            name: "RGB Cyan".to_string(),
            space: "RGB",
            value: "0 200 220".to_string(),
        },
        ExtraColor {
            self_id: "Color/RGBMagenta".to_string(),
            name: "RGB Magenta".to_string(),
            space: "RGB",
            value: "220 40 200".to_string(),
        },
    ]
}

fn gradient_swatches() -> Vec<ExtraGradient> {
    vec![
        ExtraGradient {
            self_id: "Gradient/BlackToPaper".to_string(),
            name: "Black to Paper".to_string(),
            kind: "Linear",
            stops: vec![
                GradientStop {
                    stop_color: "Color/Black".to_string(),
                    location_pct: 0.0,
                },
                GradientStop {
                    stop_color: "Color/Paper".to_string(),
                    location_pct: 100.0,
                },
            ],
        },
        ExtraGradient {
            self_id: "Gradient/CMY3".to_string(),
            name: "CMY 3 Stops".to_string(),
            kind: "Linear",
            stops: vec![
                GradientStop {
                    stop_color: "Color/CMYKCyan".to_string(),
                    location_pct: 0.0,
                },
                GradientStop {
                    stop_color: "Color/CMYKMagenta".to_string(),
                    location_pct: 50.0,
                },
                GradientStop {
                    stop_color: "Color/CMYKYellow".to_string(),
                    location_pct: 100.0,
                },
            ],
        },
        ExtraGradient {
            self_id: "Gradient/RadialPaperToBlack".to_string(),
            name: "Radial Paper to Black".to_string(),
            kind: "Radial",
            stops: vec![
                GradientStop {
                    stop_color: "Color/Paper".to_string(),
                    location_pct: 0.0,
                },
                GradientStop {
                    stop_color: "Color/Black".to_string(),
                    location_pct: 100.0,
                },
            ],
        },
        ExtraGradient {
            self_id: "Gradient/RGBCyanMagenta".to_string(),
            name: "RGB Cyan to Magenta".to_string(),
            kind: "Linear",
            stops: vec![
                GradientStop {
                    stop_color: "Color/RGBCyan".to_string(),
                    location_pct: 0.0,
                },
                GradientStop {
                    stop_color: "Color/RGBMagenta".to_string(),
                    location_pct: 100.0,
                },
            ],
        },
        ExtraGradient {
            self_id: "Gradient/FourStops".to_string(),
            name: "Four Stops".to_string(),
            kind: "Linear",
            stops: vec![
                GradientStop {
                    stop_color: "Color/CMYKCyan".to_string(),
                    location_pct: 0.0,
                },
                GradientStop {
                    stop_color: "Color/CMYKMagenta".to_string(),
                    location_pct: 33.0,
                },
                GradientStop {
                    stop_color: "Color/CMYKYellow".to_string(),
                    location_pct: 66.0,
                },
                GradientStop {
                    stop_color: "Color/Black".to_string(),
                    location_pct: 100.0,
                },
            ],
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
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
        };

        let demo = Rect {
            self_id: demo_id,
            width_pt: DEMO_W_PT,
            height_pt: DEMO_H_PT,
            item_transform: compose_translate(
                (PAGE_W_PT - DEMO_W_PT) * 0.5,
                (PAGE_H_PT - DEMO_H_PT) * 0.5,
            ),
            fill_color: Some(variant.fill_gradient.to_string()),
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: None,
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
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
        graphic_xml: graphic_xml_with_extras_and_gradients(&extra_colors(), &gradient_swatches()),
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

fn compose_translate(tx: f32, ty: f32) -> Matrix {
    let mut m = IDENTITY;
    m[4] = tx;
    m[5] = ty;
    m
}
