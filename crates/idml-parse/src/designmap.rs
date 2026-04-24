//! `designmap.xml` — the root manifest that lists referenced spreads,
//! stories, masters, preferences, and so on.
//!
//! Only a tiny subset of attributes is extracted here — enough to drive
//! seed-corpus round-trips. Full schema coverage lands during Phase 0.

use quick_xml::events::Event;
use serde::Serialize;

use crate::ParseError;

#[derive(Debug, Default, Clone, Serialize)]
pub struct DesignMap {
    pub spreads: Vec<Spread>,
    pub stories: Vec<StoryRef>,
    pub master_spreads: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Spread {
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
                    let src = attr(&e, b"src");
                    match e.name().as_ref() {
                        b"idPkg:Spread" => {
                            if let Some(src) = src {
                                out.spreads.push(Spread { src });
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

fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| std::str::from_utf8(&a.value).ok().map(str::to_string))
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
