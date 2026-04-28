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
}
