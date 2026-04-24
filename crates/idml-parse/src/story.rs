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

use crate::ParseError;

#[derive(Debug, Default, Clone, Serialize)]
pub struct Story {
    pub paragraphs: Vec<Paragraph>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Paragraph {
    pub paragraph_style: Option<String>,
    pub runs: Vec<CharacterRun>,
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
                            runs: Vec::new(),
                        });
                    }
                    b"CharacterStyleRange" => {
                        current_run = Some(CharacterRun {
                            character_style: attr(&e, b"AppliedCharacterStyle"),
                            font: attr(&e, b"AppliedFont"),
                            font_style: attr(&e, b"FontStyle"),
                            point_size: attr(&e, b"PointSize").and_then(|s| s.parse().ok()),
                            fill_color: attr(&e, b"FillColor"),
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
                Event::Empty(e) => {
                    // Line breaks inside a paragraph surface as <Br/> — treat
                    // them as a logical newline in the current run.
                    if e.name().as_ref() == b"Br" {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push('\n');
                        }
                    }
                }
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
}
