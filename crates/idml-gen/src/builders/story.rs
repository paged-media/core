//! Minimal `Story` builder — one paragraph of one character run.
//! Carries the `Page.Name` descriptor as visible text so the rendered
//! PDF is human-readable too.

use crate::xml::XmlBuilder;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

pub struct Story {
    pub self_id: String,
    pub paragraphs: Vec<String>,
}

pub fn write_story(s: &Story) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Story", &[PKG_NS, DOM_VERSION]);
    b.start("Story", &[("Self", s.self_id.as_str())]);
    for paragraph in &s.paragraphs {
        b.start(
            "ParagraphStyleRange",
            &[(
                "AppliedParagraphStyle",
                "ParagraphStyle/$ID/[No paragraph style]",
            )],
        );
        b.start(
            "CharacterStyleRange",
            &[(
                "AppliedCharacterStyle",
                "CharacterStyle/$ID/[No character style]",
            )],
        );
        b.start("Content", &[]);
        b.text(paragraph);
        b.end("Content");
        b.end("CharacterStyleRange");
        b.end("ParagraphStyleRange");
    }
    b.end("Story");
    b.end("idPkg:Story");
    b.into_bytes()
}
