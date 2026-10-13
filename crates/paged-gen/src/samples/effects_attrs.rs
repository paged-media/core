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

//! `effects-attrs.idml` — the effect attributes the engine did not read,
//! asked of InDesign (ADR 120).
//!
//! InDesign writes an attribute only when it differs from its default
//! (measured on a document built through its DOM): a drop shadow's
//! `BlendMode` (default `Multiply`), `Spread`, `Noise`, `KnockedOut`
//! (default `true`), `HonorOtherEffects` (default `false`) and
//! `UseGlobalLight` (default `false`); a glow's `Technique` (default
//! `Softer`); a bevel's `HighlightBlendMode` / `ShadowBlendMode`
//! (`Screen` / `Multiply`); a gradient feather's `HiliteAngle` /
//! `HiliteLength`; a directional feather's `FollowShapeMode` (default
//! `LeadingEdge`). The document's global light is
//! `<TransparencyPreference GlobalLightAngle GlobalLightAltitude>` in
//! `Resources/Preferences.xml` — 120° / 30° by default, 45° / 20° here so
//! a shadow that follows it visibly moves.
//!
//! Every item is a 110 × 80 pt magenta frame over a yellow band that runs
//! under the lower half of its row. Cases, numbered on the page:
//!
//! * page 1 — drop shadows: blend modes 1–4 (cyan shadow), spread and
//!   noise 5–8 (`Size` 10), knocked out over a fill at 40 % 9–10,
//!   honouring a feather 11–12, global light 13–16;
//! * page 2 — glows 17–20, bevel blend modes 21–24, gradient-feather
//!   highlight 25–28, directional feather on a triangle 29–31.

use crate::builders::{
    designmap::{write_designmap, DesignMap},
    master::{write_master, Master},
    page_item::{PageItem, Polygon, PolygonSubPath, Rect, TextFramePref},
    resources::{container_xml, fonts_xml, graphic_xml_with_extras, styles_xml, ExtraColor},
    spread::{write_spread, Spread},
    story::{write_story, Paragraph, Run, Story},
    xml_folder::{backing_story_xml, mapping_xml, tags_xml},
};
use crate::geometry::translate;
use crate::ids::self_id;
use crate::package::Sample;

const SAMPLE: &str = "effects-attrs";
const PAGE_W_PT: f32 = 595.276; // A4 portrait
const PAGE_H_PT: f32 = 841.890;
const FONT: &str = "Inter";
const COLS: [f32; 4] = [40.0, 178.0, 316.0, 454.0];
const ROWS: [f32; 4] = [90.0, 280.0, 470.0, 660.0];
pub const W: f32 = 110.0;
pub const H: f32 = 80.0;
/// The document's global light, `(angle, altitude)` in degrees.
pub const GLOBAL_LIGHT: (f32, f32) = (45.0, 20.0);

/// What a case is drawn as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Rect,
    /// A triangle pointing right, for the directional feather.
    Triangle,
}

#[derive(Clone, Debug)]
pub struct Case {
    pub n: u32,
    pub page: usize,
    pub col: usize,
    pub row: usize,
    pub shape: Shape,
    pub label: &'static str,
    /// The `<TransparencySetting>` children, verbatim.
    pub object: String,
    /// A `<FillTransparencySetting>` block's children, verbatim.
    pub fill: Option<&'static str>,
}

fn shadow(extra: &str) -> String {
    sized_shadow(4.0, extra)
}

fn sized_shadow(size: f32, extra: &str) -> String {
    format!(
        r#"<DropShadowSetting Mode="Drop" XOffset="7" YOffset="7" Size="{size}" Opacity="75" EffectColor="Color/Black"{extra}/>"#
    )
}

fn cyan_shadow(extra: &str) -> String {
    format!(
        r#"<DropShadowSetting Mode="Drop" XOffset="7" YOffset="7" Size="4" Opacity="90" EffectColor="Color/Cyan"{extra}/>"#
    )
}

/// Every case, in number order.
pub fn cases() -> Vec<Case> {
    let feather = r#"<FeatherSetting Mode="Standard" Width="10" ChokeAmount="0" CornerType="Diffusion" Noise="0"/>"#;
    let glow = |el: &str, extra: &str| {
        format!(
            r#"<{el} Applied="true" BlendMode="Normal" Opacity="90" Size="14" EffectColor="Color/Cyan"{extra}/>"#
        )
    };
    let bevel = |extra: &str| {
        format!(
            r#"<BevelAndEmbossSetting Applied="true" Style="InnerBevel" Technique="SmoothContour" Direction="Up" Depth="100" Size="12" Soften="0"{extra}/>"#
        )
    };
    let gradient_feather = |extra: &str| {
        format!(
            r#"<GradientFeatherSetting Applied="true" Type="Radial"{extra}><GradientStop Location="0" Opacity="100"/><GradientStop Location="100" Opacity="0"/></GradientFeatherSetting>"#
        )
    };
    let directional = |extra: &str| {
        format!(
            r#"<DirectionalFeatherSetting Applied="true" LeftWidth="24" RightWidth="0" TopWidth="0" BottomWidth="0" Angle="0"{extra}/>"#
        )
    };
    let raw: Vec<(usize, &'static str, String, Option<&'static str>, Shape)> = vec![
        // Page 1, row 1: shadow blend modes.
        (
            0,
            "shadow Normal",
            cyan_shadow(r#" BlendMode="Normal""#),
            None,
            Shape::Rect,
        ),
        (
            0,
            "shadow Multiply",
            cyan_shadow(r#" BlendMode="Multiply""#),
            None,
            Shape::Rect,
        ),
        (
            0,
            "shadow Screen",
            cyan_shadow(r#" BlendMode="Screen""#),
            None,
            Shape::Rect,
        ),
        (
            0,
            "shadow default blend",
            cyan_shadow(""),
            None,
            Shape::Rect,
        ),
        // Row 2: spread and noise.
        (
            0,
            "Size 10 Spread 0",
            sized_shadow(10.0, ""),
            None,
            Shape::Rect,
        ),
        (
            0,
            "Size 10 Spread 30",
            sized_shadow(10.0, r#" Spread="30""#),
            None,
            Shape::Rect,
        ),
        (
            0,
            "Size 10 Noise 20",
            sized_shadow(10.0, r#" Noise="20""#),
            None,
            Shape::Rect,
        ),
        (
            0,
            "Size 10 Spread 30 Noise 20",
            sized_shadow(10.0, r#" Spread="30" Noise="20""#),
            None,
            Shape::Rect,
        ),
        // Row 3: knocked out under a see-through fill; honouring a feather.
        (
            0,
            "fill 40 %, knocked out",
            shadow(""),
            Some(r#"<BlendingSetting Opacity="40"/>"#),
            Shape::Rect,
        ),
        (
            0,
            "fill 40 %, not knocked out",
            shadow(r#" KnockedOut="false""#),
            Some(r#"<BlendingSetting Opacity="40"/>"#),
            Shape::Rect,
        ),
        (
            0,
            "feather, shadow ignores it",
            format!("{}{feather}", shadow("")),
            None,
            Shape::Rect,
        ),
        (
            0,
            "feather, shadow honours it",
            format!("{}{feather}", shadow(r#" HonorOtherEffects="true""#)),
            None,
            Shape::Rect,
        ),
        // Row 4: global light (document 45° / 20°).
        (0, "7/7 local light", shadow(""), None, Shape::Rect),
        (
            0,
            "7/7 global light",
            shadow(r#" UseGlobalLight="true""#),
            None,
            Shape::Rect,
        ),
        (
            0,
            "10/0 global light",
            shadow(r#" UseGlobalLight="true""#)
                .replace(r#"XOffset="7" YOffset="7""#, r#"XOffset="10" YOffset="0""#),
            None,
            Shape::Rect,
        ),
        (
            0,
            "bevel global light",
            bevel(r#" UseGlobalLight="true""#),
            None,
            Shape::Rect,
        ),
        // Page 2, row 1: glow technique.
        (
            1,
            "outer glow Softer",
            glow("OuterGlowSetting", ""),
            None,
            Shape::Rect,
        ),
        (
            1,
            "outer glow Precise",
            glow("OuterGlowSetting", r#" Technique="Precise""#),
            None,
            Shape::Rect,
        ),
        (
            1,
            "inner glow Softer",
            glow("InnerGlowSetting", ""),
            None,
            Shape::Rect,
        ),
        (
            1,
            "inner glow Precise",
            glow("InnerGlowSetting", r#" Technique="Precise""#),
            None,
            Shape::Rect,
        ),
        // Row 2: bevel blend modes.
        (1, "bevel default modes", bevel(""), None, Shape::Rect),
        (
            1,
            "bevel highlight Normal",
            bevel(r#" HighlightBlendMode="Normal""#),
            None,
            Shape::Rect,
        ),
        (
            1,
            "bevel shadow Normal",
            bevel(r#" ShadowBlendMode="Normal""#),
            None,
            Shape::Rect,
        ),
        (
            1,
            "bevel both Overlay",
            bevel(r#" HighlightBlendMode="Overlay" ShadowBlendMode="Overlay""#),
            None,
            Shape::Rect,
        ),
        // Row 3: gradient-feather highlight.
        (1, "radial feather", gradient_feather(""), None, Shape::Rect),
        (
            1,
            "radial hilite 30 at 45°",
            gradient_feather(r#" HiliteLength="30" HiliteAngle="45""#),
            None,
            Shape::Rect,
        ),
        (
            1,
            "radial hilite 30 at 180°",
            gradient_feather(r#" HiliteLength="30" HiliteAngle="180""#),
            None,
            Shape::Rect,
        ),
        (
            1,
            "radial hilite 0 at 45°",
            gradient_feather(r#" HiliteLength="0" HiliteAngle="45""#),
            None,
            Shape::Rect,
        ),
        // Row 4: directional feather follow-shape mode on a triangle.
        (
            1,
            "dir. feather LeadingEdge",
            directional(""),
            None,
            Shape::Triangle,
        ),
        (
            1,
            "dir. feather AllEdges",
            directional(r#" FollowShapeMode="AllEdges""#),
            None,
            Shape::Triangle,
        ),
        (
            1,
            "dir. feather None",
            directional(r#" FollowShapeMode="None""#),
            None,
            Shape::Triangle,
        ),
    ];
    let mut per_page = [0usize; 2];
    raw.into_iter()
        .enumerate()
        .map(|(i, (page, label, object, fill, shape))| {
            let k = per_page[page];
            per_page[page] += 1;
            Case {
                n: i as u32 + 1,
                page,
                col: k % 4,
                row: k / 4,
                shape,
                label,
                object,
                fill,
            }
        })
        .collect()
}

fn colors() -> Vec<ExtraColor> {
    let c = |id: &str, v: &str| ExtraColor {
        self_id: format!("Color/{id}"),
        name: id.to_string(),
        space: "CMYK",
        value: v.to_string(),
    };
    vec![
        c("Magenta", "0 100 0 0"),
        c("Cyan", "100 0 0 0"),
        c("Yellow", "0 0 100 0"),
    ]
}

/// `Resources/Preferences.xml` with the document's global light.
fn preferences() -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><idPkg:Preferences xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0"><TextPreference UseOpticalSize="false"/><TransparencyPreference BlendingSpace="CMYK" GlobalLightAngle="{}" GlobalLightAltitude="{}"/></idPkg:Preferences>"#,
        GLOBAL_LIGHT.0, GLOBAL_LIGHT.1
    )
    .into_bytes()
}

fn rect(id: String, w: f32, h: f32, x: f32, y: f32) -> Rect {
    Rect {
        self_id: id,
        width_pt: w,
        height_pt: h,
        item_transform: translate(x, y),
        fill_color: None,
        stroke_color: None,
        stroke_weight_pt: None,
        parent_story: None,
        next_text_frame: None,
        previous_text_frame: None,
        extra_attrs: Vec::new(),
        blending: None,
        drop_shadow: None,
        placed_image: None,
        text_wrap: None,
        anchored_setting: None,
        frame_effects: Vec::new(),
        text_frame_pref: None,
        custom_subpaths: None,
    }
}

/// Build the full `Sample` ready for `write_idml`.
/// One page's items and its (story id, label) pairs.
type PageContent = (Vec<PageItem>, Vec<(String, String)>);

pub fn build() -> Sample {
    let master_id = self_id(SAMPLE, "MasterSpread", 0);
    let master_page_id = self_id(SAMPLE, "MasterPage", 0);
    let mut next = 0u32;
    let mut id = |kind: &str| {
        next += 1;
        self_id(SAMPLE, kind, next)
    };
    let mut pages: Vec<PageContent> = (0..2).map(|_| Default::default()).collect();
    let mut stories = Vec::new();
    let mut story_refs = Vec::new();
    for (items, _) in pages.iter_mut() {
        for y in ROWS {
            let mut band = rect(
                id("Rectangle"),
                PAGE_W_PT - 40.0,
                H / 2.0 + 24.0,
                20.0,
                y + H / 2.0,
            );
            band.fill_color = Some("Color/Yellow".into());
            items.push(band.into());
        }
    }
    for c in cases() {
        let (x, y) = (COLS[c.col], ROWS[c.row]);
        let (items, splices) = &mut pages[c.page];
        // The case's number and name above it.
        let story_id = id("Story");
        stories.push((
            story_id.clone(),
            write_story(&Story {
                extra_story_attrs: Vec::new(),
                self_id: story_id.clone(),
                paragraphs: vec![Paragraph {
                    extra_paragraph_attrs: vec![("Hyphenation", "false")],
                    leading: Some(9.0),
                    justification: Some("LeftAlign"),
                    runs: vec![Run {
                        extra_char_attrs: Vec::new(),
                        text: format!("{} · {}", c.n, c.label),
                        point_size: Some(7.5),
                        fill_color: Some("Color/Black".into()),
                        font_style: None,
                        tracking: None,
                        baseline_shift: None,
                        underline: None,
                        applied_font: Some(FONT),
                        anchored_frame: None,
                    }],
                    ..Paragraph::plain("")
                }],
            }),
        ));
        story_refs.push(story_id.clone());
        let mut label = rect(id("TextFrame"), 130.0, 14.0, x, y - 24.0);
        label.parent_story = Some(story_id);
        label.text_frame_pref = Some(TextFramePref {
            inset_spacing: Some([0.0; 4]),
            ..Default::default()
        });
        items.push(label.into());
        // The item itself.
        let item_id;
        match c.shape {
            Shape::Rect => {
                item_id = id("Rectangle");
                let mut r = rect(item_id.clone(), W, H, x, y);
                r.fill_color = Some("Color/Magenta".into());
                items.push(r.into());
            }
            Shape::Triangle => {
                item_id = id("Polygon");
                items.push(
                    Polygon {
                        self_id: item_id.clone(),
                        item_transform: translate(x, y),
                        fill_color: Some("Color/Magenta".into()),
                        stroke_color: None,
                        stroke_weight_pt: Some(0.0),
                        extra_attrs: Vec::new(),
                        subpaths: vec![PolygonSubPath::corners(
                            vec![(0.0, 0.0), (W, H / 2.0), (0.0, H)],
                            true,
                        )],
                        text_path: None,
                    }
                    .into(),
                );
            }
        }
        let mut block = format!("<TransparencySetting>{}</TransparencySetting>", c.object);
        if let Some(fill) = c.fill {
            block.push_str(&format!(
                "<FillTransparencySetting>{fill}</FillTransparencySetting>"
            ));
        }
        splices.push((item_id, block));
    }

    let mut spreads = Vec::new();
    let mut spread_refs = Vec::new();
    let titles = [
        "effects-attrs · drop-shadow attributes, global light",
        "effects-attrs · glows, bevel modes, feather attributes",
    ];
    for (p, (items, splices)) in pages.into_iter().enumerate() {
        let spread_id = self_id(SAMPLE, "Spread", p as u32);
        let xml = write_spread(&Spread {
            self_id: spread_id.clone(),
            page_self_id: self_id(SAMPLE, "Page", p as u32),
            page_name: titles[p].to_string(),
            applied_master: format!("MasterSpread/{master_id}"),
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: items,
            override_list: Vec::new(),
            margins: None,
            item_transform: None,
        });
        // Each block goes right after the item's own `</Properties>`,
        // where InDesign writes it.
        let mut s = String::from_utf8(xml).expect("utf-8");
        for (item, block) in &splices {
            let at = s.find(&format!(r#"Self="{item}""#)).expect("item written");
            let close =
                at + s[at..].find("</Properties>").expect("properties") + "</Properties>".len();
            s.insert_str(close, block);
        }
        spreads.push((spread_id.clone(), s.into_bytes()));
        spread_refs.push(spread_id);
    }

    let master_spreads = vec![(
        master_id.clone(),
        write_master(&Master {
            self_id: format!("MasterSpread/{master_id}"),
            page_self_id: master_page_id,
            page_width_pt: PAGE_W_PT,
            page_height_pt: PAGE_H_PT,
            page_items: Vec::new(),
        }),
    )];

    Sample {
        container_xml: container_xml(),
        designmap_xml: write_designmap(&DesignMap {
            self_id: "d".to_string(),
            master_spreads: vec![master_id],
            spreads: spread_refs,
            stories: story_refs,
        }),
        graphic_xml: graphic_xml_with_extras(&colors()),
        fonts_xml: fonts_xml(),
        styles_xml: styles_xml(),
        preferences_xml: preferences(),
        backing_story_xml: backing_story_xml(),
        tags_xml: tags_xml(),
        mapping_xml: mapping_xml(),
        master_spreads,
        spreads,
        stories,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the engine's own reader: the document's light, and the
    /// attributes each case varies.
    #[test]
    fn the_light_and_the_attributes_read_back() {
        let bytes = crate::package::write_idml(&build()).expect("package");
        let doc = idml_import::import_idml_doc(&bytes).expect("import");
        let light = doc.designmap.transparency_preference;
        assert_eq!(
            (light.global_light_angle, light.global_light_altitude),
            (Some(GLOBAL_LIGHT.0), Some(GLOBAL_LIGHT.1))
        );
        let shadows: Vec<_> = doc.spreads[0]
            .spread
            .rectangles
            .iter()
            .filter_map(|r| r.drop_shadow.as_ref())
            .collect();
        assert_eq!(shadows.len(), 15);
        let modes: Vec<_> = shadows
            .iter()
            .take(4)
            .map(|s| s.blend_mode.as_deref())
            .collect();
        assert_eq!(
            modes,
            [Some("Normal"), Some("Multiply"), Some("Screen"), None]
        );
        assert_eq!(
            shadows
                .iter()
                .filter(|s| s.spread_pct == Some(30.0))
                .count(),
            2
        );
        assert_eq!(
            shadows.iter().filter(|s| s.noise_pct == Some(20.0)).count(),
            2
        );
        assert_eq!(
            shadows
                .iter()
                .filter(|s| s.knocked_out == Some(false))
                .count(),
            1
        );
        assert_eq!(
            shadows
                .iter()
                .filter(|s| s.use_global_light == Some(true))
                .count(),
            2
        );
        let effects: Vec<_> = doc.spreads[1]
            .spread
            .rectangles
            .iter()
            .filter_map(|r| r.effects.as_ref())
            .collect();
        assert!(effects.iter().any(
            |e| e.outer_glow.as_ref().and_then(|g| g.technique.as_deref()) == Some("Precise")
        ));
        assert!(effects.iter().any(|e| {
            e.gradient_feather
                .as_ref()
                .is_some_and(|g| g.hilite_length == Some(30.0) && g.hilite_angle == Some(45.0))
        }));
    }
}
