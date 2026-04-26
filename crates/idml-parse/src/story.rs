//! Story_*.xml parser.
//!
//! An IDML Story is a tree:
//! ```text
//! <Story>
//!   <ParagraphStyleRange AppliedParagraphStyle="...">
//!     <CharacterStyleRange AppliedCharacterStyle="..." PointSize="12" AppliedFont="...">
//!       <Content>Some text</Content>
//!       <Br/>
//!       <Content>more text</Content>
//!     </CharacterStyleRange>
//!     <CharacterStyleRange ...>
//!       <Content>bold bit</Content>
//!     </CharacterStyleRange>
//!   </ParagraphStyleRange>
//!   <ParagraphStyleRange>...</ParagraphStyleRange>
//! </Story>
//! ```
//!
//! The parser collapses all `<Content>` children of a character range
//! into a single string, preserving paragraph boundaries. Full style
//! resolution (font cascade, local overrides, etc.) is the job of
//! `idml-scene`; this module stays focused on shape extraction.

use quick_xml::events::Event;
use serde::Serialize;

use crate::util::attr;
use crate::ParseError;

#[derive(Debug, Default, Clone, Serialize)]
pub struct Story {
    pub paragraphs: Vec<Paragraph>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Paragraph {
    pub paragraph_style: Option<String>,
    /// `Justification` attribute from IDML. Common values:
    /// `LeftAlign`, `CenterAlign`, `RightAlign`, `FullyJustified`,
    /// `LeftJustified`, `CenterJustified`, `RightJustified`.
    pub justification: Option<String>,
    /// `FirstLineIndent` in pt.
    pub first_line_indent: Option<f32>,
    /// `SpaceBefore` in pt.
    pub space_before: Option<f32>,
    /// `SpaceAfter` in pt.
    pub space_after: Option<f32>,
    /// `<TabList>` parsed from `<Properties>`. Empty when none is
    /// declared on this paragraph (the cascade fills in from the
    /// applied paragraph style if available).
    pub tab_list: Vec<TabStop>,
    pub runs: Vec<CharacterRun>,
}

/// One stop in a paragraph's `<TabList>`. Position is in pt from
/// the column's left edge.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TabStop {
    pub position: f32,
    /// IDML alignment string: `LeftAlign`, `RightAlign`,
    /// `CenterAlign`, `CharacterAlign`.
    pub alignment: Option<String>,
    /// `AlignmentCharacter` for `CharacterAlign` stops (rare).
    pub alignment_character: Option<String>,
    /// `Leader` string rendered in the tab gap.
    pub leader: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CharacterRun {
    pub character_style: Option<String>,
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    /// `FillColor="Color/..."` on the CharacterStyleRange; resolved
    /// against `Graphic`.
    pub fill_color: Option<String>,
    /// `Tracking` in 1/1000 em (InDesign's unit — divide by 1000 to
    /// get the em fraction that should be added to every glyph's
    /// advance).
    pub tracking: Option<f32>,
    /// `Underline="true"` on the CharacterStyleRange.
    pub underline: Option<bool>,
    /// `StrikeThru="true"` on the CharacterStyleRange.
    pub strikethru: Option<bool>,
    pub text: String,
}

impl Story {
    pub fn parse(xml: &[u8]) -> Result<Self, ParseError> {
        let mut reader = quick_xml::Reader::from_reader(xml);
        reader.config_mut().trim_text(false);

        let mut out = Story::default();
        let mut current_paragraph: Option<Paragraph> = None;
        let mut current_run: Option<CharacterRun> = None;
        let mut in_content = false;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) => match e.name().as_ref() {
                    b"ParagraphStyleRange" => {
                        current_paragraph = Some(Paragraph {
                            paragraph_style: attr(&e, b"AppliedParagraphStyle"),
                            justification: attr(&e, b"Justification"),
                            first_line_indent: attr(&e, b"FirstLineIndent")
                                .and_then(|s| s.parse().ok()),
                            space_before: attr(&e, b"SpaceBefore").and_then(|s| s.parse().ok()),
                            space_after: attr(&e, b"SpaceAfter").and_then(|s| s.parse().ok()),
                            tab_list: Vec::new(),
                            runs: Vec::new(),
                        });
                    }
                    b"TabStop" => {
                        // <TabStop Position="..." Alignment="..."/>
                        // appears nested inside <TabList><ListItem>.
                        // Append to the open paragraph's list.
                        if let Some(stop) = parse_tab_stop(&e) {
                            if let Some(p) = current_paragraph.as_mut() {
                                p.tab_list.push(stop);
                            }
                        }
                    }
                    b"CharacterStyleRange" => {
                        current_run = Some(CharacterRun {
                            character_style: attr(&e, b"AppliedCharacterStyle"),
                            font: attr(&e, b"AppliedFont"),
                            font_style: attr(&e, b"FontStyle"),
                            point_size: attr(&e, b"PointSize").and_then(|s| s.parse().ok()),
                            fill_color: attr(&e, b"FillColor"),
                            tracking: attr(&e, b"Tracking").and_then(|s| s.parse().ok()),
                            underline: attr(&e, b"Underline").and_then(|s| s.parse::<bool>().ok()),
                            strikethru: attr(&e, b"StrikeThru")
                                .and_then(|s| s.parse::<bool>().ok()),
                            text: String::new(),
                        });
                    }
                    b"Content" => {
                        in_content = true;
                    }
                    _ => {}
                },
                Event::End(e) => match e.name().as_ref() {
                    b"Content" => {
                        in_content = false;
                    }
                    b"CharacterStyleRange" => {
                        if let (Some(run), Some(para)) =
                            (current_run.take(), current_paragraph.as_mut())
                        {
                            if !run.text.is_empty() {
                                para.runs.push(run);
                            }
                        }
                    }
                    b"ParagraphStyleRange" => {
                        if let Some(para) = current_paragraph.take() {
                            if !para.runs.is_empty() {
                                out.paragraphs.push(para);
                            }
                        }
                    }
                    _ => {}
                },
                Event::Empty(e) => match e.name().as_ref() {
                    // Line breaks inside a paragraph surface as <Br/> — treat
                    // them as a logical newline in the current run.
                    b"Br" => {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push('\n');
                        }
                    }
                    // Tab characters surface as <Tab/>; the layout
                    // pass treats '\t' as wide whitespace until a
                    // proper TabList-aware breaker lands.
                    b"Tab" => {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push('\t');
                        }
                    }
                    // Self-closing <TabStop .../> inside the
                    // paragraph's TabList.
                    b"TabStop" => {
                        if let Some(stop) = parse_tab_stop(&e) {
                            if let Some(p) = current_paragraph.as_mut() {
                                p.tab_list.push(stop);
                            }
                        }
                    }
                    _ => {}
                },
                Event::Text(t) => {
                    if in_content {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push_str(&t.unescape().unwrap_or_default());
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
}

fn parse_tab_stop(e: &quick_xml::events::BytesStart) -> Option<TabStop> {
    let position = attr(e, b"Position").and_then(|s| s.parse::<f32>().ok())?;
    Some(TabStop {
        position,
        alignment: attr(e, b"Alignment"),
        alignment_character: attr(e, b"AlignmentCharacter"),
        leader: attr(e, b"Leader"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
      <CharacterStyleRange AppliedCharacterStyle="CharacterStyle/$ID/[No character style]"
                           AppliedFont="Minion Pro" PointSize="11">
        <Content>Hello, </Content>
      </CharacterStyleRange>
      <CharacterStyleRange FontStyle="Bold" AppliedFont="Minion Pro" PointSize="11">
        <Content>world</Content>
      </CharacterStyleRange>
      <CharacterStyleRange AppliedFont="Minion Pro" PointSize="11">
        <Content>.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
      <CharacterStyleRange AppliedFont="Minion Pro" PointSize="11">
        <Content>Second paragraph.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;

    #[test]
    fn extracts_paragraphs_and_runs() {
        let s = Story::parse(SAMPLE).unwrap();
        assert_eq!(s.paragraphs.len(), 2);

        let p1 = &s.paragraphs[0];
        assert_eq!(p1.paragraph_style.as_deref(), Some("ParagraphStyle/Body"));
        assert_eq!(p1.runs.len(), 3);
        assert_eq!(p1.runs[0].text, "Hello, ");
        assert_eq!(p1.runs[1].text, "world");
        assert_eq!(p1.runs[1].font_style.as_deref(), Some("Bold"));
        assert_eq!(p1.runs[1].point_size, Some(11.0));
        assert_eq!(p1.runs[2].text, ".");

        let p2 = &s.paragraphs[1];
        assert_eq!(p2.runs.len(), 1);
        assert_eq!(p2.runs[0].text, "Second paragraph.");
    }

    #[test]
    fn br_becomes_newline_in_run_text() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>line one</Content>
              <Br/>
              <Content>line two</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = Story::parse(xml).unwrap();
        assert_eq!(s.paragraphs[0].runs[0].text, "line one\nline two");
    }

    #[test]
    fn tab_element_becomes_tab_character() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>name</Content>
              <Tab/>
              <Content>value</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = Story::parse(xml).unwrap();
        assert_eq!(s.paragraphs[0].runs[0].text, "name\tvalue");
    }

    #[test]
    fn tab_list_attaches_to_paragraph() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <Properties>
              <TabList>
                <ListItem><TabStop Position="36" Alignment="LeftAlign"/></ListItem>
                <ListItem><TabStop Position="144" Alignment="RightAlign" Leader="."/></ListItem>
              </TabList>
            </Properties>
            <CharacterStyleRange>
              <Content>x</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = Story::parse(xml).unwrap();
        let stops = &s.paragraphs[0].tab_list;
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].position, 36.0);
        assert_eq!(stops[0].alignment.as_deref(), Some("LeftAlign"));
        assert_eq!(stops[1].position, 144.0);
        assert_eq!(stops[1].leader.as_deref(), Some("."));
    }
}
