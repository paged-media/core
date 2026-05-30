//! `XML/*.xml` — InDesign expects these present even when empty.
//! Generated samples never use XML structure tagging, so all three
//! files are minimal stubs.

use crate::xml::XmlBuilder;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

pub fn backing_story_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:BackingStory", &[PKG_NS, DOM_VERSION]);
    b.empty(
        "XmlStory",
        &[
            ("Self", "BackingStory"),
            ("AppliedXMLTag", "XMLTag/$ID/Root"),
        ],
    );
    b.end("idPkg:BackingStory");
    b.into_bytes()
}

pub fn tags_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Tags", &[PKG_NS, DOM_VERSION]);
    b.empty("XMLTag", &[("Self", "XMLTag/$ID/Root"), ("Name", "Root")]);
    b.end("idPkg:Tags");
    b.into_bytes()
}

pub fn mapping_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.empty("idPkg:Mapping", &[PKG_NS, DOM_VERSION]);
    b.into_bytes()
}
