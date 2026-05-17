//! `designmap.xml` — the root manifest that lists referenced spreads,
//! stories, masters, preferences, and so on.
//!
//! Only a tiny subset of attributes is extracted here — enough to drive
//! seed-corpus round-trips. Full schema coverage lands during Phase 0.

use quick_xml::events::Event;
use serde::Serialize;

use crate::util::attr;
use crate::ParseError;

#[derive(Debug, Default, Clone, Serialize)]
pub struct DesignMap {
    pub spreads: Vec<SpreadRef>,
    pub stories: Vec<StoryRef>,
    pub master_spreads: Vec<String>,
    /// Document-level color management settings, extracted from the
    /// root `<Document>` element. Drives ICC transform construction —
    /// the renderer matches `color_settings.cmyk_profile` against its
    /// bundled profile set and falls back to a naive CMYK→sRGB
    /// approximation when the named profile isn't shipped.
    pub color_settings: ColorSettings,
    /// Document layers, in serialization order (which mirrors the
    /// stacking order — first layer = bottom of the z-stack). Each
    /// page item references its layer via `ItemLayer="<self_id>"`.
    /// The renderer skips items whose layer is hidden or non-printable.
    pub layers: Vec<Layer>,
    /// `<TextVariable>` definitions. Each carries a `VariableType`
    /// (`FileNameVariable`, `RunningHeaderVariable`, `ChapterNumberType`,
    /// `XrefPageNumberType`, etc.) and is referenced from stories via
    /// `<TextVariableInstance AssociatedTextVariable="TextVariable/<id>"
    /// ResultText="..."/>`. The renderer treats `ResultText` as the
    /// authoritative value at the moment InDesign exported the IDML —
    /// "live" recomputation per page is a future task.
    pub text_variables: Vec<TextVariable>,
}

/// IDML `<TextVariable>` declaration. Parsed for completeness; the
/// rendered value comes from each `<TextVariableInstance>`'s
/// `ResultText` attribute, which the parser inlines into the host
/// run's text.
#[derive(Debug, Clone, Serialize)]
pub struct TextVariable {
    pub self_id: String,
    pub name: Option<String>,
    pub variable_type: Option<String>,
}

/// IDML `<Layer>` definition. Only the fields the renderer needs
/// today; visibility / printability decide whether items on that
/// layer are emitted at all.
#[derive(Debug, Clone, Serialize)]
pub struct Layer {
    pub self_id: String,
    pub name: Option<String>,
    /// `Visible="true|false"` — when false the layer is hidden in
    /// InDesign's view and PDF export skips it.
    pub visible: bool,
    /// `Locked="true|false"` — purely an editor concern; the renderer
    /// ignores it but we surface the field so future tooling can
    /// honour it.
    pub locked: bool,
    /// `Printable="true|false"` — InDesign's "Print Layer" checkbox.
    /// Non-printable layers are skipped during rendering.
    pub printable: bool,
}

/// Document-level color management config. Mirrors the attributes that
/// real InDesign exports carry on the `<Document>` element (CS6 / IDML
/// 8.0). Empty defaults match "no opinion" and let the renderer pick
/// a global fallback.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ColorSettings {
    /// `CMYKProfile` attribute, e.g. `"Coated FOGRA39 (ISO 12647-2:2004)"`.
    pub cmyk_profile: Option<String>,
    /// `RGBProfile` attribute, e.g. `"sRGB IEC61966-2.1"`.
    pub rgb_profile: Option<String>,
    /// `SolidColorIntent` — typically `"UseColorSettings"` (use the
    /// document's working spaces) or one of `Perceptual`,
    /// `Saturation`, `RelativeColorimetric`, `AbsoluteColorimetric`.
    pub solid_color_intent: Option<String>,
    /// `AfterBlendingIntent` — same value space as `solid_color_intent`.
    pub after_blending_intent: Option<String>,
    /// `DefaultImageIntent` — same value space.
    pub default_image_intent: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpreadRef {
    pub src: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoryRef {
    pub src: String,
}

impl DesignMap {
    /// Parse a `designmap.xml` byte slice.
    pub fn parse(xml: &[u8]) -> Result<Self, ParseError> {
        let mut reader = quick_xml::Reader::from_reader(xml);
        reader.config_mut().trim_text(false);

        let mut out = DesignMap::default();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) | Event::Empty(e) => {
                    if e.name().as_ref() == b"Document" {
                        out.color_settings = ColorSettings {
                            cmyk_profile: attr(&e, b"CMYKProfile"),
                            rgb_profile: attr(&e, b"RGBProfile"),
                            solid_color_intent: attr(&e, b"SolidColorIntent"),
                            after_blending_intent: attr(&e, b"AfterBlendingIntent"),
                            default_image_intent: attr(&e, b"DefaultImageIntent"),
                        };
                    }
                    if e.name().as_ref() == b"Layer" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            out.layers.push(Layer {
                                self_id,
                                name: attr(&e, b"Name"),
                                visible: attr(&e, b"Visible")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(true),
                                locked: attr(&e, b"Locked")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(false),
                                printable: attr(&e, b"Printable")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(true),
                            });
                        }
                    }
                    if e.name().as_ref() == b"TextVariable" {
                        if let Some(self_id) = attr(&e, b"Self") {
                            out.text_variables.push(TextVariable {
                                self_id,
                                name: attr(&e, b"Name"),
                                variable_type: attr(&e, b"VariableType"),
                            });
                        }
                    }
                    let src = attr(&e, b"src");
                    match e.name().as_ref() {
                        b"idPkg:Spread" => {
                            if let Some(src) = src {
                                out.spreads.push(SpreadRef { src });
                            }
                        }
                        b"idPkg:Story" => {
                            if let Some(src) = src {
                                out.stories.push(StoryRef { src });
                            }
                        }
                        b"idPkg:MasterSpread" => {
                            if let Some(src) = src {
                                out.master_spreads.push(src);
                            }
                        }
                        _ => {}
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:MasterSpread src="MasterSpreads/MasterSpread_ua.xml"/>
  <idPkg:Spread src="Spreads/Spread_u1.xml"/>
  <idPkg:Spread src="Spreads/Spread_u2.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#;

    #[test]
    fn parses_spread_and_story_manifest() {
        let dm = DesignMap::parse(SAMPLE).unwrap();
        assert_eq!(dm.spreads.len(), 2);
        assert_eq!(dm.stories.len(), 1);
        assert_eq!(dm.master_spreads.len(), 1);
        assert_eq!(dm.spreads[0].src, "Spreads/Spread_u1.xml");
        assert_eq!(dm.stories[0].src, "Stories/Story_u10.xml");
    }

    const LAYERS_SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Layer Self="ua" Name="Layer 1" Visible="true" Locked="false" Printable="true"/>
  <Layer Self="ub" Name="Guides" Visible="true" Locked="true" Printable="false"/>
  <Layer Self="uc" Name="Hidden" Visible="false" Printable="true"/>
  <Layer Self="ud" Name="Defaults"/>
</Document>"#;

    #[test]
    fn q17_layer_printable_attribute_round_trips() {
        let dm = DesignMap::parse(LAYERS_SAMPLE).unwrap();
        assert_eq!(dm.layers.len(), 4);
        let printable: Vec<bool> = dm.layers.iter().map(|l| l.printable).collect();
        assert_eq!(printable, vec![true, false, true, true]);
        let visible: Vec<bool> = dm.layers.iter().map(|l| l.visible).collect();
        assert_eq!(visible, vec![true, true, false, true]);
    }
}
