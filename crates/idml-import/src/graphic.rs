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

//! `Resources/Graphic.xml` — the document's swatch palette.
//!
//! Extracts `<Color>` entries keyed by their `Self` attribute so
//! `FillColor="Color/Red"` on a TextFrame resolves to an actual
//! ColorValue. `<Swatch>` elements are also captured for the "None" /
//! "Paper" / "Registration" special cases.
//!
//! Spot colours are captured with `Model="Spot"` plus optional
//! `AlternateSpace` / `AlternateColorValue` (CMYK fallback used when
//! the spot ink isn't physically applied). A per-swatch `TintValue`
//! (0..=100) records "PANTONE 286 at 50% tint" stored on the swatch
//! itself — distinct from the per-run `FillTint` cascade. We always
//! render spot colours via the CMYK alternate (we don't know the
//! spectral spot ink); the swatch's tint is multiplied into the
//! alternate channels before ICC conversion to match InDesign's
//! preview behaviour. Spot colours whose `AlternateSpace` isn't CMYK
//! (rare in practice) fall back to the swatch's own `Space` /
//! `ColorValue`.
//!
//! The swatch-palette value types (`ColorEntry`, `Gradient*`, `Color*`,
//! `ReservedSwatch`, the colour math) live in `paged-model`; this module
//! owns only the `Graphic` container + the XML parsing and re-exports the
//! types so `idml_import::graphic::*` keeps resolving.

use quick_xml::events::Event;

use crate::util::attr;
use crate::ParseError;

pub use paged_model::{
    to_linear_rgb, ColorEntry, ColorGroupEntry, ColorModel, ColorSpace, GradientEntry,
    GradientKind, GradientStopRef, Graphic, ReservedSwatch, SwatchEntry,
};

/// Parse `Resources/Graphic.xml` into a [`Graphic`] swatch palette.
/// (De-inherented from `Graphic::parse` so the type lives in `paged-model`;
/// the XML parsing stays in the parser — N6.)
pub fn parse_graphic(xml: &[u8]) -> Result<Graphic, ParseError> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(true);

    let mut out = Graphic::default();
    let mut buf = Vec::new();
    // State for the open <Gradient> element. Stops are children
    // of the surrounding <Gradient>; we collect them here and
    // commit once the close tag fires.
    let mut current_gradient: Option<GradientEntry> = None;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                b"Color" => {
                    if let Some(entry) = parse_color(&e) {
                        out.colors.insert(entry.self_id.clone(), entry);
                    }
                }
                b"Swatch" => {
                    if let Some(entry) = parse_swatch(&e) {
                        out.swatches.insert(entry.self_id.clone(), entry);
                    }
                }
                b"Gradient" => {
                    if let Some(entry) = parse_gradient(&e) {
                        current_gradient = Some(entry);
                    }
                }
                b"GradientStop" => {
                    if let (Some(g), Some(stop)) =
                        (current_gradient.as_mut(), parse_gradient_stop(&e))
                    {
                        g.stops.push(stop);
                    }
                }
                b"ColorGroup" => {
                    if let Some(entry) = parse_color_group(&e) {
                        out.color_groups.insert(entry.self_id.clone(), entry);
                    }
                }
                _ => {}
            },
            Event::End(e) => {
                if e.name().as_ref() == b"Gradient" {
                    if let Some(g) = current_gradient.take() {
                        out.gradients.insert(g.self_id.clone(), g);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

fn parse_color(e: &quick_xml::events::BytesStart) -> Option<ColorEntry> {
    let self_id = attr(e, b"Self")?;
    let space = attr(e, b"Space")
        .as_deref()
        .map(ColorSpace::from_attr)
        .unwrap_or(ColorSpace::Unknown);
    let value = attr(e, b"ColorValue")
        .map(parse_color_value)
        .unwrap_or_default();
    let model = attr(e, b"Model")
        .as_deref()
        .map(ColorModel::from_attr)
        .unwrap_or(ColorModel::Process);
    let alternate_space = attr(e, b"AlternateSpace")
        .as_deref()
        .map(ColorSpace::from_attr);
    let alternate_value = attr(e, b"AlternateColorValue")
        .map(parse_color_value)
        .unwrap_or_default();
    // `TintValue` is an IDML float 0..=100. Treat -1 (Adobe's "unset"
    // sentinel) and out-of-range values as absent so a swatch with
    // no swatch-level tint flows the alternate through unscaled.
    let tint = attr(e, b"TintValue")
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| (0.0..=100.0).contains(v));
    // Alpha lives on `<Color>` in two competing serialisations.
    // Adobe's reference uses `AlphaPercentage` (0..=100); some
    // tooling emits a plain `Alpha` (0..=100 or 0..=1). Accept
    // either; treat absent as `None`. Values > 1 are interpreted
    // as the percentage form; values in `[0, 1]` are treated as a
    // unit float.
    let alpha = attr(e, b"AlphaPercentage")
        .or_else(|| attr(e, b"Alpha"))
        .and_then(|s| s.parse::<f32>().ok())
        .map(|v| {
            if v > 1.0 {
                (v / 100.0).clamp(0.0, 1.0)
            } else {
                v.clamp(0.0, 1.0)
            }
        });
    Some(ColorEntry {
        self_id,
        name: attr(e, b"Name"),
        space,
        value,
        model,
        alternate_space,
        alternate_value,
        tint,
        alpha,
    })
}

fn parse_color_value(s: String) -> Vec<f32> {
    s.split_whitespace()
        .filter_map(|t| t.parse::<f32>().ok())
        .collect()
}

fn parse_gradient(e: &quick_xml::events::BytesStart) -> Option<GradientEntry> {
    let self_id = attr(e, b"Self")?;
    let kind = attr(e, b"Type")
        .as_deref()
        .map(|s| match s {
            "Linear" => GradientKind::Linear,
            "Radial" => GradientKind::Radial,
            _ => GradientKind::Unknown,
        })
        .unwrap_or(GradientKind::Linear);
    Some(GradientEntry {
        self_id,
        name: attr(e, b"Name"),
        kind,
        stops: Vec::new(),
    })
}

fn parse_gradient_stop(e: &quick_xml::events::BytesStart) -> Option<GradientStopRef> {
    let stop_color = attr(e, b"StopColor")?;
    let location_pct = attr(e, b"Location")
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(0.0);
    let midpoint_pct = attr(e, b"Midpoint").and_then(|s| s.parse::<f32>().ok());
    Some(GradientStopRef {
        stop_color,
        location_pct,
        midpoint_pct,
    })
}

fn parse_swatch(e: &quick_xml::events::BytesStart) -> Option<SwatchEntry> {
    let self_id = attr(e, b"Self")?;
    Some(SwatchEntry {
        self_id,
        name: attr(e, b"Name"),
        color_ref: attr(e, b"ColorEditorHotGraphic").or_else(|| attr(e, b"Color")),
    })
}

fn parse_color_group(e: &quick_xml::events::BytesStart) -> Option<ColorGroupEntry> {
    let self_id = attr(e, b"Self")?;
    let members = attr(e, b"ColorGroupSwatches")
        .map(|s| {
            s.split_whitespace()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(ColorGroupEntry {
        self_id,
        name: attr(e, b"Name"),
        members,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Red" Name="Red" Model="Process" Space="CMYK" ColorValue="0 100 100 0"/>
    <Color Self="Color/Paper" Name="Paper" Space="RGB" ColorValue="255 255 255"/>
    <Color Self="Color/DarkGray" Name="DarkGray" Space="Gray" ColorValue="60"/>
  </Graphic>
</idPkg:Graphic>"#;

    #[test]
    fn parses_color_entries() {
        let g = parse_graphic(SAMPLE).unwrap();
        assert_eq!(g.colors.len(), 3);
        let red = g.resolve("Color/Red").unwrap();
        assert_eq!(red.name.as_deref(), Some("Red"));
        assert_eq!(red.space, ColorSpace::Cmyk);
        assert_eq!(red.value, vec![0.0, 100.0, 100.0, 0.0]);
    }

    #[test]
    fn cmyk_pure_red_converts_to_red_rgb() {
        let g = parse_graphic(SAMPLE).unwrap();
        let red = g.resolve("Color/Red").unwrap();
        let rgb = to_linear_rgb(red).unwrap();
        // R ≈ 1, G ≈ 0, B ≈ 0 for C=0 M=100 Y=100 K=0. sRGB→linear
        // of 1.0 stays at 1.0; of 0.0 stays at 0.0.
        assert!((rgb[0] - 1.0).abs() < 1e-3, "rgb={:?}", rgb);
        assert!(rgb[1] < 1e-3, "rgb={:?}", rgb);
        assert!(rgb[2] < 1e-3, "rgb={:?}", rgb);
    }

    #[test]
    fn gray_converts_to_achromatic_rgb() {
        let g = parse_graphic(SAMPLE).unwrap();
        let dg = g.resolve("Color/DarkGray").unwrap();
        let rgb = to_linear_rgb(dg).unwrap();
        assert!(rgb[0] > 0.0 && rgb[0] < 1.0);
        assert_eq!(rgb[0], rgb[1]);
        assert_eq!(rgb[1], rgb[2]);
    }

    #[test]
    fn unknown_color_id_resolves_to_none() {
        let g = parse_graphic(SAMPLE).unwrap();
        assert!(g.resolve("Color/NotThere").is_none());
    }

    const GRADIENT_SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Sky"   Name="Sky"   Space="RGB" ColorValue="120 180 255"/>
    <Color Self="Color/Sun"   Name="Sun"   Space="RGB" ColorValue="255 220 100"/>
    <Gradient Self="Gradient/Sky" Name="Sky" Type="Linear">
      <GradientStop StopColor="Color/Sun" Location="0"/>
      <GradientStop StopColor="Color/Sky" Location="100"/>
    </Gradient>
  </Graphic>
</idPkg:Graphic>"#;

    #[test]
    fn parses_linear_gradient_with_two_stops() {
        let g = parse_graphic(GRADIENT_SAMPLE).unwrap();
        let grad = g.gradients.get("Gradient/Sky").expect("gradient parsed");
        assert_eq!(grad.kind, GradientKind::Linear);
        assert_eq!(grad.stops.len(), 2);
        assert_eq!(grad.stops[0].stop_color, "Color/Sun");
        assert_eq!(grad.stops[0].location_pct, 0.0);
        assert_eq!(grad.stops[1].stop_color, "Color/Sky");
        assert_eq!(grad.stops[1].location_pct, 100.0);
    }

    const ALPHA_SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Translucent" Name="T" Space="RGB" ColorValue="120 180 255" AlphaPercentage="40"/>
    <Color Self="Color/HalfAlpha" Name="H" Space="RGB" ColorValue="0 0 0" Alpha="0.5"/>
    <Color Self="Color/Opaque" Name="O" Space="RGB" ColorValue="0 0 0"/>
  </Graphic>
</idPkg:Graphic>"#;

    #[test]
    fn resolve_alpha_reads_alpha_percentage() {
        // AlphaPercentage="40" → 0.40.
        let g = parse_graphic(ALPHA_SAMPLE).unwrap();
        let alpha = g.resolve_alpha("Color/Translucent").expect("alpha set");
        assert!((alpha - 0.40).abs() < 1e-4, "got {}", alpha);
    }

    #[test]
    fn resolve_alpha_accepts_unit_float_form() {
        // Some tooling serialises `Alpha="0.5"` as a unit float.
        let g = parse_graphic(ALPHA_SAMPLE).unwrap();
        let alpha = g.resolve_alpha("Color/HalfAlpha").expect("alpha set");
        assert!((alpha - 0.5).abs() < 1e-4, "got {}", alpha);
    }

    #[test]
    fn resolve_alpha_returns_none_for_swatch_without_alpha() {
        // Color without an Alpha attribute → None (caller treats as
        // opaque and falls back to inline stop attributes).
        let g = parse_graphic(ALPHA_SAMPLE).unwrap();
        assert!(g.resolve_alpha("Color/Opaque").is_none());
    }

    #[test]
    fn resolve_alpha_unknown_id_returns_none() {
        let g = parse_graphic(ALPHA_SAMPLE).unwrap();
        assert!(g.resolve_alpha("Color/NotThere").is_none());
    }

    const SPOT_SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/Pantone286" Name="PANTONE 286 C" Model="Spot"
           Space="LAB" ColorValue="20 25 -70"
           AlternateSpace="CMYK" AlternateColorValue="100 75 0 0"/>
    <Color Self="Color/Pantone286Half" Name="PANTONE 286 C 50%" Model="Spot"
           Space="LAB" ColorValue="20 25 -70"
           AlternateSpace="CMYK" AlternateColorValue="100 75 0 0"
           TintValue="50"/>
    <Color Self="Color/PantonePlain" Name="PANTONE plain" Model="Spot"
           Space="LAB" ColorValue="20 25 -70"/>
    <Color Self="Color/ProcessPureM" Name="Magenta" Model="Process"
           Space="CMYK" ColorValue="0 100 0 0"/>
  </Graphic>
</idPkg:Graphic>"#;

    #[test]
    fn parses_spot_color_with_alternate_cmyk() {
        let g = parse_graphic(SPOT_SAMPLE).unwrap();
        let spot = g.resolve("Color/Pantone286").unwrap();
        assert_eq!(spot.model, ColorModel::Spot);
        assert_eq!(spot.alternate_space, Some(ColorSpace::Cmyk));
        assert_eq!(spot.alternate_value, vec![100.0, 75.0, 0.0, 0.0]);
        assert!(spot.tint.is_none(), "no swatch-level tint here");
    }

    #[test]
    fn parses_swatch_level_tint_value() {
        let g = parse_graphic(SPOT_SAMPLE).unwrap();
        let spot = g.resolve("Color/Pantone286Half").unwrap();
        assert_eq!(spot.tint, Some(50.0));
    }

    #[test]
    fn effective_cmyk_for_process_returns_value_unchanged() {
        // (M=100) → (M=100). No tint, no spot fallback.
        let g = parse_graphic(SPOT_SAMPLE).unwrap();
        let m = g.resolve("Color/ProcessPureM").unwrap();
        assert_eq!(m.effective_cmyk(), Some([0.0, 100.0, 0.0, 0.0]));
    }

    #[test]
    fn effective_cmyk_for_spot_uses_alternate() {
        let g = parse_graphic(SPOT_SAMPLE).unwrap();
        let spot = g.resolve("Color/Pantone286").unwrap();
        assert_eq!(spot.effective_cmyk(), Some([100.0, 75.0, 0.0, 0.0]));
    }

    #[test]
    fn effective_cmyk_for_spot_with_tint_scales_each_channel() {
        // The pinned math: spot at 50% tint mixes 50% toward paper
        // white in CMYK, i.e. multiplies each channel by 0.5.
        let g = parse_graphic(SPOT_SAMPLE).unwrap();
        let spot = g.resolve("Color/Pantone286Half").unwrap();
        assert_eq!(spot.effective_cmyk(), Some([50.0, 37.5, 0.0, 0.0]));
    }

    #[test]
    fn effective_cmyk_for_spot_without_cmyk_alternate_returns_none() {
        // PantonePlain has no AlternateSpace — there's no CMYK to
        // tint, so the renderer must fall back to the swatch's
        // primary `Space` via to_linear_rgb (which will say None
        // because we don't ship Lab→RGB).
        let g = parse_graphic(SPOT_SAMPLE).unwrap();
        let spot = g.resolve("Color/PantonePlain").unwrap();
        assert!(spot.effective_cmyk().is_none());
    }

    #[test]
    fn to_linear_rgb_routes_spot_tint_to_lighter_rgb() {
        // 50% tinted PANTONE 286 (CMYK alt 100,75,0,0) should render
        // visibly lighter / less saturated than the 100% version.
        // Naive CMYK→linear-RGB suffices for the comparison.
        let g = parse_graphic(SPOT_SAMPLE).unwrap();
        let full = to_linear_rgb(g.resolve("Color/Pantone286").unwrap()).unwrap();
        let half = to_linear_rgb(g.resolve("Color/Pantone286Half").unwrap()).unwrap();
        // 100% tint: R = (1-1)(1-0)=0; G = (1-0.75)(1-0)=0.25; B=1.
        // 50% tint: R = (1-0.5)(1-0)=0.5; G=(1-0.375)=0.625; B=1.
        // Each channel of `half` is brighter than `full` in linear:
        assert!(half[0] > full[0], "R lighter: {} > {}", half[0], full[0]);
        assert!(half[1] > full[1], "G lighter: {} > {}", half[1], full[1]);
        // Blue stays at 1.0 in both — no Y / K to scale it down.
        assert!((half[2] - full[2]).abs() < 1e-4);
    }

    #[test]
    fn tint_value_minus_one_is_treated_as_absent() {
        const NEG: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic>
    <Color Self="Color/X" Name="X" Model="Spot" Space="LAB" ColorValue="20 25 -70"
           AlternateSpace="CMYK" AlternateColorValue="100 0 0 0" TintValue="-1"/>
  </Graphic>
</idPkg:Graphic>"#;
        let g = parse_graphic(NEG).unwrap();
        let c = g.resolve("Color/X").unwrap();
        assert!(c.tint.is_none());
        // Effective CMYK is the unscaled alternate.
        assert_eq!(c.effective_cmyk(), Some([100.0, 0.0, 0.0, 0.0]));
    }
}
