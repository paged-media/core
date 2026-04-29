//! `designmap.xml` — root manifest pointing at every Resources/,
//! MasterSpreads/, Spreads/, Stories/ entry the package contains.

use crate::xml::XmlBuilder;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);

pub struct DesignMap {
    /// Self-id of the document — typically `d`.
    pub self_id: String,
    pub master_spreads: Vec<String>,
    pub spreads: Vec<String>,
    pub stories: Vec<String>,
}

pub fn write_designmap(dm: &DesignMap) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "Document",
        &[
            PKG_NS,
            ("DOMVersion", "20.0"),
            ("Self", dm.self_id.as_str()),
            ("StoryList", &dm.stories.join(" ")),
            ("Name", "generated.indd"),
        ],
    );
    b.empty("idPkg:Graphic", &[("src", "Resources/Graphic.xml")]);
    b.empty("idPkg:Fonts", &[("src", "Resources/Fonts.xml")]);
    b.empty("idPkg:Styles", &[("src", "Resources/Styles.xml")]);
    b.empty("idPkg:Preferences", &[("src", "Resources/Preferences.xml")]);
    b.empty("idPkg:Tags", &[("src", "XML/Tags.xml")]);
    for ms in &dm.master_spreads {
        b.empty(
            "idPkg:MasterSpread",
            &[("src", &format!("MasterSpreads/MasterSpread_{ms}.xml"))],
        );
    }
    for s in &dm.spreads {
        b.empty(
            "idPkg:Spread",
            &[("src", &format!("Spreads/Spread_{s}.xml"))],
        );
    }
    for s in &dm.stories {
        b.empty("idPkg:Story", &[("src", &format!("Stories/Story_{s}.xml"))]);
    }
    b.empty("idPkg:BackingStory", &[("src", "XML/BackingStory.xml")]);
    b.end("Document");
    b.into_bytes()
}
