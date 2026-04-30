//! `Resources/Graphic.xml` — the document's swatch palette.
//!
//! Extracts `<Color>` entries keyed by their `Self` attribute so
//! `FillColor="Color/Red"` on a TextFrame resolves to an actual
//! ColorValue. `<Swatch>` elements are also captured for the "None" /
//! "Paper" / "Registration" special cases.
//!
//! This is a minimal slice: process colours only, no tint / mixed-ink.
//! Spot colour definitions pull through as `Lab` space when present;
//! otherwise they're flagged as unresolved.

use std::collections::BTreeMap;

use quick_xml::events::Event;
use serde::Serialize;

use crate::util::attr;
use crate::ParseError;

#[derive(Debug, Default, Clone, Serialize)]
pub struct Graphic {
    /// All `<Color>` entries, keyed by `Self` (e.g. "Color/Red").
    pub colors: BTreeMap<String, ColorEntry>,
    /// Named `<Swatch>` entries — "None", "Paper", "Black", etc.
    pub swatches: BTreeMap<String, SwatchEntry>,
    /// `<Gradient>` swatches (linear / radial), keyed by `Self`
    /// (e.g. "Gradient/Sky").
    pub gradients: BTreeMap<String, GradientEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColorEntry {
    pub self_id: String,
    pub name: Option<String>,
    pub space: ColorSpace,
    pub value: Vec<f32>,
    /// Optional alpha channel (0..=1, 1 = fully opaque) sourced from
    /// the IDML `Alpha` / `AlphaPercentage` attribute on `<Color>`.
    /// `None` means the swatch carries no alpha; the consumer should
    /// treat the swatch as opaque. Used by the gradient-feather
    /// renderer when a `<GradientStop>` in spec form references a
    /// `<Color>` whose alpha defines the stop's opacity.
    pub alpha: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwatchEntry {
    pub self_id: String,
    pub name: Option<String>,
    /// `Self` reference to the Color this swatch wraps, if any.
    pub color_ref: Option<String>,
}

/// IDML gradient swatch. Stops reference Color entries by `Self` id.
#[derive(Debug, Clone, Serialize)]
pub struct GradientEntry {
    pub self_id: String,
    pub name: Option<String>,
    pub kind: GradientKind,
    pub stops: Vec<GradientStopRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum GradientKind {
    Linear,
    Radial,
    Unknown,
}

/// One stop in a gradient: a Color reference + a normalised location.
#[derive(Debug, Clone, Serialize)]
pub struct GradientStopRef {
    pub stop_color: String,
    /// `Location` attribute, 0..=100 in IDML.
    pub location_pct: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum ColorSpace {
    Cmyk,
    Rgb,
    Lab,
    Gray,
    /// Anything we didn't recognise — callers should treat it as
    /// unresolved and fall back to a sensible default.
    Unknown,
}

impl ColorSpace {
    fn from_attr(s: &str) -> Self {
        match s {
            "CMYK" => ColorSpace::Cmyk,
            "RGB" => ColorSpace::Rgb,
            "LAB" | "Lab" => ColorSpace::Lab,
            "Gray" => ColorSpace::Gray,
            _ => ColorSpace::Unknown,
        }
    }
}

impl Graphic {
    pub fn parse(xml: &[u8]) -> Result<Self, ParseError> {
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

    /// Look up a colour by its `Self` id. Follows a `<Swatch>` indirection
    /// one level if the id names a Swatch rather than a Color directly.
    pub fn resolve(&self, id: &str) -> Option<&ColorEntry> {
        if let Some(c) = self.colors.get(id) {
            return Some(c);
        }
        let swatch = self.swatches.get(id)?;
        let color_ref = swatch.color_ref.as_deref()?;
        self.colors.get(color_ref)
    }

    /// Resolve a swatch's alpha channel (0..=1, 1 = fully opaque).
    /// Used by the gradient-feather renderer when a `<GradientStop>`
    /// in IDML spec form (`StopColor="Color/..."`) references a
    /// `<Color>` swatch whose alpha defines the stop's opacity.
    /// Returns `None` when the swatch carries no alpha (CMYK / RGB
    /// without `AlphaPercentage`) — callers should treat that as
    /// "opaque" and fall back to whatever inline alpha attribute the
    /// stop carries (e.g. the IDML `Alpha` / `Opacity`).
    pub fn resolve_alpha(&self, id: &str) -> Option<f32> {
        self.resolve(id).and_then(|c| c.alpha)
    }
}

fn parse_color(e: &quick_xml::events::BytesStart) -> Option<ColorEntry> {
    let self_id = attr(e, b"Self")?;
    let space = attr(e, b"Space")
        .as_deref()
        .map(ColorSpace::from_attr)
        .unwrap_or(ColorSpace::Unknown);
    let value = attr(e, b"ColorValue")
        .map(|s| {
            s.split_whitespace()
                .filter_map(|t| t.parse::<f32>().ok())
                .collect()
        })
        .unwrap_or_default();
    // Alpha lives on `<Color>` in two competing serialisations.
    // Adobe's reference uses `AlphaPercentage` (0..=100); some
    // tooling emits a plain `Alpha` (0..=100 or 0..=1). Accept
    // either; treat absent as `None`. Values > 1 are interpreted
    // as the percentage form; values in `[0, 1]` are treated as a
    // unit float.
    let alpha = attr(e, b"AlphaPercentage")
        .or_else(|| attr(e, b"Alpha"))
        .and_then(|s| s.parse::<f32>().ok())
        .map(|v| if v > 1.0 { (v / 100.0).clamp(0.0, 1.0) } else { v.clamp(0.0, 1.0) });
    Some(ColorEntry {
        self_id,
        name: attr(e, b"Name"),
        space,
        value,
        alpha,
    })
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
    Some(GradientStopRef {
        stop_color,
        location_pct,
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

/// Convert a [`ColorEntry`] to non-color-managed linear RGB (0..=1).
///
/// This is a stopgap — the proper path goes through `idml-color` with
/// ICC profiles. Fine for exploratory tooling and the fidelity
/// harness's first seed documents.
pub fn to_linear_rgb(c: &ColorEntry) -> Option<[f32; 3]> {
    let v = c.value.as_slice();
    match c.space {
        ColorSpace::Cmyk if v.len() == 4 => {
            // CMYK percentages → naive RGB, then sRGB-linearize.
            let cv = v[0] / 100.0;
            let mv = v[1] / 100.0;
            let yv = v[2] / 100.0;
            let kv = v[3] / 100.0;
            let r = (1.0 - cv) * (1.0 - kv);
            let g = (1.0 - mv) * (1.0 - kv);
            let b = (1.0 - yv) * (1.0 - kv);
            Some([srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)])
        }
        ColorSpace::Rgb if v.len() == 3 => Some([
            srgb_to_linear(v[0] / 255.0),
            srgb_to_linear(v[1] / 255.0),
            srgb_to_linear(v[2] / 255.0),
        ]),
        ColorSpace::Gray if v.len() == 1 => {
            let g = srgb_to_linear(1.0 - v[0] / 100.0);
            Some([g, g, g])
        }
        _ => None,
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
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
        let g = Graphic::parse(SAMPLE).unwrap();
        assert_eq!(g.colors.len(), 3);
        let red = g.resolve("Color/Red").unwrap();
        assert_eq!(red.name.as_deref(), Some("Red"));
        assert_eq!(red.space, ColorSpace::Cmyk);
        assert_eq!(red.value, vec![0.0, 100.0, 100.0, 0.0]);
    }

    #[test]
    fn cmyk_pure_red_converts_to_red_rgb() {
        let g = Graphic::parse(SAMPLE).unwrap();
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
        let g = Graphic::parse(SAMPLE).unwrap();
        let dg = g.resolve("Color/DarkGray").unwrap();
        let rgb = to_linear_rgb(dg).unwrap();
        assert!(rgb[0] > 0.0 && rgb[0] < 1.0);
        assert_eq!(rgb[0], rgb[1]);
        assert_eq!(rgb[1], rgb[2]);
    }

    #[test]
    fn unknown_color_id_resolves_to_none() {
        let g = Graphic::parse(SAMPLE).unwrap();
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
        let g = Graphic::parse(GRADIENT_SAMPLE).unwrap();
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
        let g = Graphic::parse(ALPHA_SAMPLE).unwrap();
        let alpha = g.resolve_alpha("Color/Translucent").expect("alpha set");
        assert!((alpha - 0.40).abs() < 1e-4, "got {}", alpha);
    }

    #[test]
    fn resolve_alpha_accepts_unit_float_form() {
        // Some tooling serialises `Alpha="0.5"` as a unit float.
        let g = Graphic::parse(ALPHA_SAMPLE).unwrap();
        let alpha = g.resolve_alpha("Color/HalfAlpha").expect("alpha set");
        assert!((alpha - 0.5).abs() < 1e-4, "got {}", alpha);
    }

    #[test]
    fn resolve_alpha_returns_none_for_swatch_without_alpha() {
        // Color without an Alpha attribute → None (caller treats as
        // opaque and falls back to inline stop attributes).
        let g = Graphic::parse(ALPHA_SAMPLE).unwrap();
        assert!(g.resolve_alpha("Color/Opaque").is_none());
    }

    #[test]
    fn resolve_alpha_unknown_id_returns_none() {
        let g = Graphic::parse(ALPHA_SAMPLE).unwrap();
        assert!(g.resolve_alpha("Color/NotThere").is_none());
    }
}
