//! `Resources/Styles.xml` — paragraph and character style sheet.
//!
//! IDML's typical layout:
//!
//! ```xml
//! <idPkg:Styles>
//!   <RootCharacterStyleGroup>
//!     <CharacterStyle Self="CharacterStyle/$ID/[No character style]" .../>
//!     <CharacterStyle Self="CharacterStyle/Bold" FontStyle="Bold" .../>
//!   </RootCharacterStyleGroup>
//!   <RootParagraphStyleGroup>
//!     <ParagraphStyle Self="ParagraphStyle/Body"
//!                     AppliedFont="Body Font"
//!                     PointSize="11" .../>
//!   </RootParagraphStyleGroup>
//! </idPkg:Styles>
//! ```
//!
//! Only the cascadable attributes the renderer currently consumes
//! land here (font / style / size / fill / tracking + paragraph
//! geometry knobs). `BasedOn` chains are followed at resolve time;
//! cycles are bounded by `MAX_BASED_ON_DEPTH`.

use std::collections::BTreeMap;

use quick_xml::events::Event;
use serde::Serialize;

use crate::story::TabStop;
use crate::util::attr;
use crate::ParseError;

/// Maximum BasedOn chain length. IDML doesn't forbid cycles, so the
/// resolver short-circuits once it hits this depth — typical real-
/// world chains are 1–3 hops.
const MAX_BASED_ON_DEPTH: usize = 16;

#[derive(Debug, Default, Clone, Serialize)]
pub struct StyleSheet {
    pub character_styles: BTreeMap<String, CharacterStyleDef>,
    pub paragraph_styles: BTreeMap<String, ParagraphStyleDef>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct CharacterStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ParagraphStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub tracking: Option<f32>,
    pub justification: Option<String>,
    pub first_line_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// `<TabList>` parsed from the style. Empty means "no
    /// declaration" — the cascade may inherit from `BasedOn`.
    pub tab_list: Vec<TabStop>,
}

/// Effective character-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedCharacter {
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
}

/// Effective paragraph-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedParagraph {
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub tracking: Option<f32>,
    pub justification: Option<String>,
    pub first_line_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// `<TabList>` from the cascade. Empty means inherited / none.
    pub tab_list: Vec<TabStop>,
}

impl StyleSheet {
    pub fn parse(xml: &[u8]) -> Result<Self, ParseError> {
        let mut reader = quick_xml::Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut out = StyleSheet::default();
        let mut buf = Vec::new();
        // Track the open ParagraphStyle's id so nested <TabStop>
        // children attach to the right entry.
        let mut current_paragraph_style: Option<String> = None;
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) => match e.name().as_ref() {
                    b"CharacterStyle" => {
                        if let Some(s) = parse_character_style(&e) {
                            out.character_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"ParagraphStyle" => {
                        if let Some(s) = parse_paragraph_style(&e) {
                            current_paragraph_style = Some(s.self_id.clone());
                            out.paragraph_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    _ => {}
                },
                Event::Empty(e) => match e.name().as_ref() {
                    b"CharacterStyle" => {
                        if let Some(s) = parse_character_style(&e) {
                            out.character_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"ParagraphStyle" => {
                        if let Some(s) = parse_paragraph_style(&e) {
                            out.paragraph_styles.insert(s.self_id.clone(), s);
                        }
                    }
                    b"TabStop" => {
                        if let (Some(id), Some(stop)) = (
                            current_paragraph_style.as_deref(),
                            parse_tab_stop_styles(&e),
                        ) {
                            if let Some(p) = out.paragraph_styles.get_mut(id) {
                                p.tab_list.push(stop);
                            }
                        }
                    }
                    _ => {}
                },
                Event::End(e) => {
                    if e.name().as_ref() == b"ParagraphStyle" {
                        current_paragraph_style = None;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }

    /// Walk a CharacterStyle's `BasedOn` chain, folding each hop's
    /// unset attributes from its parent. Missing or cyclic chains
    /// short-circuit at `MAX_BASED_ON_DEPTH`.
    pub fn resolve_character(&self, id: &str) -> ResolvedCharacter {
        let mut acc = ResolvedCharacter::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.character_styles.get(&cur_id) else {
                break;
            };
            acc.font = acc.font.or_else(|| s.font.clone());
            acc.font_style = acc.font_style.or_else(|| s.font_style.clone());
            acc.point_size = acc.point_size.or(s.point_size);
            acc.fill_color = acc.fill_color.or_else(|| s.fill_color.clone());
            acc.tracking = acc.tracking.or(s.tracking);
            acc.underline = acc.underline.or(s.underline);
            acc.strikethru = acc.strikethru.or(s.strikethru);
            cursor = s.based_on.clone();
        }
        acc
    }

    pub fn resolve_paragraph(&self, id: &str) -> ResolvedParagraph {
        let mut acc = ResolvedParagraph::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.paragraph_styles.get(&cur_id) else {
                break;
            };
            acc.font = acc.font.or_else(|| s.font.clone());
            acc.font_style = acc.font_style.or_else(|| s.font_style.clone());
            acc.point_size = acc.point_size.or(s.point_size);
            acc.fill_color = acc.fill_color.or_else(|| s.fill_color.clone());
            acc.tracking = acc.tracking.or(s.tracking);
            acc.justification = acc.justification.or_else(|| s.justification.clone());
            acc.first_line_indent = acc.first_line_indent.or(s.first_line_indent);
            acc.space_before = acc.space_before.or(s.space_before);
            acc.space_after = acc.space_after.or(s.space_after);
            acc.underline = acc.underline.or(s.underline);
            acc.strikethru = acc.strikethru.or(s.strikethru);
            // First non-empty TabList wins; later parents don't
            // overwrite a child's explicit declaration.
            if acc.tab_list.is_empty() && !s.tab_list.is_empty() {
                acc.tab_list = s.tab_list.clone();
            }
            cursor = s.based_on.clone();
        }
        acc
    }
}

fn parse_character_style(e: &quick_xml::events::BytesStart) -> Option<CharacterStyleDef> {
    Some(CharacterStyleDef {
        self_id: attr(e, b"Self")?,
        name: attr(e, b"Name"),
        based_on: attr(e, b"BasedOn"),
        font: attr(e, b"AppliedFont"),
        font_style: attr(e, b"FontStyle"),
        point_size: attr(e, b"PointSize").and_then(|s| s.parse().ok()),
        fill_color: attr(e, b"FillColor"),
        tracking: attr(e, b"Tracking").and_then(|s| s.parse().ok()),
        underline: attr(e, b"Underline").and_then(|s| s.parse().ok()),
        strikethru: attr(e, b"StrikeThru").and_then(|s| s.parse().ok()),
    })
}

fn parse_tab_stop_styles(e: &quick_xml::events::BytesStart) -> Option<TabStop> {
    let position = attr(e, b"Position").and_then(|s| s.parse::<f32>().ok())?;
    Some(TabStop {
        position,
        alignment: attr(e, b"Alignment"),
        alignment_character: attr(e, b"AlignmentCharacter"),
        leader: attr(e, b"Leader"),
    })
}

fn parse_paragraph_style(e: &quick_xml::events::BytesStart) -> Option<ParagraphStyleDef> {
    Some(ParagraphStyleDef {
        self_id: attr(e, b"Self")?,
        name: attr(e, b"Name"),
        based_on: attr(e, b"BasedOn"),
        font: attr(e, b"AppliedFont"),
        font_style: attr(e, b"FontStyle"),
        point_size: attr(e, b"PointSize").and_then(|s| s.parse().ok()),
        fill_color: attr(e, b"FillColor"),
        tracking: attr(e, b"Tracking").and_then(|s| s.parse().ok()),
        justification: attr(e, b"Justification"),
        first_line_indent: attr(e, b"FirstLineIndent").and_then(|s| s.parse().ok()),
        space_before: attr(e, b"SpaceBefore").and_then(|s| s.parse().ok()),
        space_after: attr(e, b"SpaceAfter").and_then(|s| s.parse().ok()),
        underline: attr(e, b"Underline").and_then(|s| s.parse().ok()),
        strikethru: attr(e, b"StrikeThru").and_then(|s| s.parse().ok()),
        tab_list: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <RootCharacterStyleGroup>
    <CharacterStyle Self="CharacterStyle/Base"
                    Name="Base"
                    AppliedFont="Body Font"
                    PointSize="11"
                    FillColor="Color/Black"/>
    <CharacterStyle Self="CharacterStyle/Bold"
                    Name="Bold"
                    BasedOn="CharacterStyle/Base"
                    FontStyle="Bold"/>
  </RootCharacterStyleGroup>
  <RootParagraphStyleGroup>
    <ParagraphStyle Self="ParagraphStyle/Body"
                    Name="Body"
                    AppliedFont="Body Font"
                    PointSize="11"
                    Justification="LeftAlign"
                    SpaceAfter="6"/>
    <ParagraphStyle Self="ParagraphStyle/Heading"
                    Name="Heading"
                    BasedOn="ParagraphStyle/Body"
                    PointSize="22"
                    FontStyle="Bold"/>
  </RootParagraphStyleGroup>
</idPkg:Styles>"#;

    #[test]
    fn parses_styles_table() {
        let s = StyleSheet::parse(SAMPLE).unwrap();
        assert_eq!(s.character_styles.len(), 2);
        assert_eq!(s.paragraph_styles.len(), 2);
        let bold = s.character_styles.get("CharacterStyle/Bold").unwrap();
        assert_eq!(bold.based_on.as_deref(), Some("CharacterStyle/Base"));
        assert_eq!(bold.font_style.as_deref(), Some("Bold"));
    }

    #[test]
    fn resolve_character_walks_based_on_chain() {
        let s = StyleSheet::parse(SAMPLE).unwrap();
        let r = s.resolve_character("CharacterStyle/Bold");
        // FontStyle from Bold itself; AppliedFont + PointSize +
        // FillColor inherited from Base.
        assert_eq!(r.font_style.as_deref(), Some("Bold"));
        assert_eq!(r.font.as_deref(), Some("Body Font"));
        assert_eq!(r.point_size, Some(11.0));
        assert_eq!(r.fill_color.as_deref(), Some("Color/Black"));
    }

    #[test]
    fn resolve_paragraph_walks_based_on_chain() {
        let s = StyleSheet::parse(SAMPLE).unwrap();
        let r = s.resolve_paragraph("ParagraphStyle/Heading");
        assert_eq!(r.point_size, Some(22.0)); // override
        assert_eq!(r.font.as_deref(), Some("Body Font")); // inherited
        assert_eq!(r.justification.as_deref(), Some("LeftAlign"));
        assert_eq!(r.space_after, Some(6.0));
    }

    #[test]
    fn resolve_unknown_id_returns_default() {
        let s = StyleSheet::parse(SAMPLE).unwrap();
        let r = s.resolve_character("CharacterStyle/Missing");
        assert!(r.font.is_none());
        assert!(r.point_size.is_none());
    }

    #[test]
    fn resolve_terminates_on_cyclic_based_on() {
        // Two styles BasedOn each other — resolution must not hang.
        let xml =
            br#"<idPkg:Styles xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <RootCharacterStyleGroup>
            <CharacterStyle Self="CharacterStyle/A" BasedOn="CharacterStyle/B" PointSize="10"/>
            <CharacterStyle Self="CharacterStyle/B" BasedOn="CharacterStyle/A" FontStyle="Bold"/>
          </RootCharacterStyleGroup>
        </idPkg:Styles>"#;
        let s = StyleSheet::parse(xml).unwrap();
        let r = s.resolve_character("CharacterStyle/A");
        // Both were folded in once; the depth limiter prevents looping.
        assert_eq!(r.point_size, Some(10.0));
        assert_eq!(r.font_style.as_deref(), Some("Bold"));
    }
}
