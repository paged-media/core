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

use crate::ParseError;

#[derive(Debug, Default, Clone, Serialize)]
pub struct Graphic {
    /// All `<Color>` entries, keyed by `Self` (e.g. "Color/Red").
    pub colors: BTreeMap<String, ColorEntry>,
    /// Named `<Swatch>` entries — "None", "Paper", "Black", etc.
    pub swatches: BTreeMap<String, SwatchEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColorEntry {
    pub self_id: String,
    pub name: Option<String>,
    pub space: ColorSpace,
    pub value: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwatchEntry {
    pub self_id: String,
    pub name: Option<String>,
    /// `Self` reference to the Color this swatch wraps, if any.
    pub color_ref: Option<String>,
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
                    _ => {}
                },
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
    Some(ColorEntry {
        self_id,
        name: attr(e, b"Name"),
        space,
        value,
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

fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| std::str::from_utf8(&a.value).ok().map(str::to_string))
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
}
