//! Bare-minimum `Resources/*.xml` files plus the
//! `META-INF/container.xml` entry. The shapes here are stripped to the
//! smallest set InDesign actually requires to open the package without
//! complaint — Phase 0 samples don't need rich style cascades; the
//! builders make richer resources when later phases need them.

use crate::xml::XmlBuilder;

/// `META-INF/container.xml` — UCF rootfile pointer.
pub fn container_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "container",
        &[
            ("version", "1.0"),
            ("xmlns", "urn:oasis:names:tc:opendocument:xmlns:container"),
        ],
    );
    b.start("rootfiles", &[]);
    b.empty(
        "rootfile",
        &[("full-path", "designmap.xml"), ("media-type", "text/xml")],
    );
    b.end("rootfiles");
    b.end("container");
    b.into_bytes()
}

/// `Resources/Graphic.xml` — registers `Color/Black` and `Color/Paper`,
/// the two swatches every IDML carries by default.
pub fn graphic_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "idPkg:Graphic",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.empty(
        "Color",
        &[
            ("Self", "Color/Black"),
            ("Model", "Process"),
            ("Space", "CMYK"),
            ("ColorValue", "0 0 0 100"),
            ("ColorOverride", "Specialblack"),
            ("Name", "Black"),
            ("ColorEditable", "false"),
            ("ColorRemovable", "false"),
            ("Visible", "true"),
        ],
    );
    b.empty(
        "Color",
        &[
            ("Self", "Color/Paper"),
            ("Model", "Process"),
            ("Space", "CMYK"),
            ("ColorValue", "0 0 0 0"),
            ("ColorOverride", "Specialpaper"),
            ("Name", "Paper"),
            ("ColorEditable", "true"),
            ("ColorRemovable", "false"),
            ("Visible", "true"),
        ],
    );
    b.end("idPkg:Graphic");
    b.into_bytes()
}

/// `Resources/Fonts.xml` — declares the `Open Sans` family. The
/// renderer's existing fixture fonts include OpenSans.ttf so the
/// generated samples render with the same face InDesign substitutes
/// when importing.
pub fn fonts_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "idPkg:Fonts",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.start(
        "FontFamily",
        &[("Self", "FontFamily/OpenSans"), ("Name", "Open Sans")],
    );
    b.empty(
        "Font",
        &[
            ("Self", "Font/OpenSans"),
            ("FontFamily", "Open Sans"),
            ("Name", "Open Sans"),
            ("PostScriptName", "OpenSans"),
            ("Status", "Installed"),
            ("FontStyleName", "Regular"),
            ("FontType", "TrueType"),
        ],
    );
    b.end("FontFamily");
    b.end("idPkg:Fonts");
    b.into_bytes()
}

/// `Resources/Styles.xml` — declares the implicit `[No paragraph
/// style]` and `[No character style]` plus a default Open Sans
/// paragraph style for body text.
pub fn styles_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start(
        "idPkg:Styles",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.start("RootCharacterStyleGroup", &[]);
    b.empty(
        "CharacterStyle",
        &[
            ("Self", "CharacterStyle/$ID/[No character style]"),
            ("Name", "$ID/[No character style]"),
        ],
    );
    b.end("RootCharacterStyleGroup");
    b.start("RootParagraphStyleGroup", &[]);
    b.empty(
        "ParagraphStyle",
        &[
            ("Self", "ParagraphStyle/$ID/[No paragraph style]"),
            ("Name", "$ID/[No paragraph style]"),
            ("AppliedFont", "Open Sans"),
            ("PointSize", "12"),
            ("FillColor", "Color/Black"),
        ],
    );
    b.end("RootParagraphStyleGroup");
    b.end("idPkg:Styles");
    b.into_bytes()
}

/// `Resources/Preferences.xml` — empty manifest. The renderer reads
/// only what the document uses; InDesign opens the file regardless of
/// which preferences are present.
pub fn preferences_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.empty(
        "idPkg:Preferences",
        &[
            (
                "xmlns:idPkg",
                "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
            ),
            ("DOMVersion", "20.0"),
        ],
    );
    b.into_bytes()
}
