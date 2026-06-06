/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

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

/// W1.4 — document-level marker resources (text variables, hyperlinks,
/// destinations) bolted onto a `DesignMap`. Kept separate so the
/// existing samples' 4-field `DesignMap` literals (and their emitted
/// designmaps) stay byte-identical; only the markers sample passes a
/// non-empty set via [`write_designmap_with_markers`].
#[derive(Default)]
pub struct MarkerResources {
    pub text_variables: Vec<TextVariableDef>,
    pub hyperlinks: Vec<HyperlinkDef>,
    pub hyperlink_destinations: Vec<HyperlinkDestinationDef>,
}

/// W1.4 — a `<TextVariable>` to emit. `contents` populates the
/// `<TextVariablePreference Contents="...">` child for custom-text
/// variables; the renderer resolves all other types itself.
pub struct TextVariableDef {
    pub self_id: String,
    pub name: String,
    pub variable_type: String,
    pub contents: Option<String>,
}

/// W1.4 — a `<Hyperlink>` (source span → destination resource).
pub struct HyperlinkDef {
    pub self_id: String,
    pub name: String,
    pub source: String,
    pub destination: String,
}

/// W1.4 — a hyperlink destination resource.
pub enum HyperlinkDestinationDef {
    /// `<HyperlinkURLDestination Self=... DestinationURL=...>`.
    Url { self_id: String, url: String },
    /// `<HyperlinkPageDestination Self=... DestinationPage=...>`.
    Page { self_id: String, page: String },
}

pub fn write_designmap(dm: &DesignMap) -> Vec<u8> {
    write_designmap_with_markers(dm, &MarkerResources::default())
}

/// As [`write_designmap`] but also emits document-level marker
/// resources (W1.4). When `markers` is empty the output is identical
/// to `write_designmap`.
pub fn write_designmap_with_markers(dm: &DesignMap, markers: &MarkerResources) -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    // <?aid?> processing instruction. InDesign's IDML reader rejects
    // documents without it as "format not supported" — even when the
    // DOMVersion is correct and the ZIP is well-formed. Fields:
    //   style="50"          IDML format style
    //   type="document"     top-level document (vs snippet/icml)
    //   readerVersion="6.0" minimum IDML reader version (CS6+)
    //   featureSet="257"    feature bitmask the document uses
    //   product="20.0(32)"  exporter product version
    b.write_pi(
        "aid",
        r#"style="50" type="document" readerVersion="6.0" featureSet="257" product="20.0(32)""#,
    );
    b.start(
        "Document",
        &[
            PKG_NS,
            ("DOMVersion", "20.0"),
            ("Self", dm.self_id.as_str()),
            ("StoryList", &dm.stories.join(" ")),
            ("Name", "generated.indd"),
            // ColorSettings — match InDesign's default ICC profiles so
            // the inspect binary picks up the host's installed Adobe
            // ICC profiles (FOGRA39 / sRGB) and routes CMYK swatches
            // through lcms2. Without these attributes the renderer
            // falls back to naive `(1-cv)*(1-kv)` math which produces
            // pure black for `Color/Black` (CMYK K=100) instead of the
            // ~(35,31,32) sRGB warm dark gray that real K=100 ink
            // prints to. The 0/20 → high-pass-rate jump on
            // `geometry.idml` traces directly to this declaration.
            ("CMYKProfile", "Coated FOGRA39 (ISO 12647-2:2004)"),
            ("RGBProfile", "sRGB IEC61966-2.1"),
            ("SolidColorIntent", "UseColorSettings"),
            ("AfterBlendingIntent", "UseColorSettings"),
            ("DefaultImageIntent", "UseColorSettings"),
        ],
    );
    // W1.4 — marker resources (text variables, hyperlinks,
    // destinations). Emitted before the idPkg refs, matching where
    // InDesign serialises document-level resources. Skipped entirely
    // when empty so existing samples' designmaps stay byte-identical.
    for v in &markers.text_variables {
        b.start(
            "TextVariable",
            &[
                ("Self", v.self_id.as_str()),
                ("Name", v.name.as_str()),
                ("VariableType", v.variable_type.as_str()),
            ],
        );
        // Real exports nest a <TextVariablePreference>; for custom
        // text it carries the literal `Contents`. Emit it for every
        // variable (empty for non-custom) so the parser folds it in.
        let contents = v.contents.as_deref().unwrap_or("");
        b.empty("TextVariablePreference", &[("Contents", contents)]);
        b.end("TextVariable");
    }
    for d in &markers.hyperlink_destinations {
        match d {
            HyperlinkDestinationDef::Url { self_id, url } => {
                b.empty(
                    "HyperlinkURLDestination",
                    &[
                        ("Self", self_id.as_str()),
                        ("Name", url.as_str()),
                        ("DestinationURL", url.as_str()),
                        ("Hidden", "false"),
                    ],
                );
            }
            HyperlinkDestinationDef::Page { self_id, page } => {
                b.empty(
                    "HyperlinkPageDestination",
                    &[
                        ("Self", self_id.as_str()),
                        ("Name", self_id.as_str()),
                        ("DestinationPage", page.as_str()),
                        ("DestinationPageSetting", "FitVisible"),
                        ("Hidden", "false"),
                    ],
                );
            }
        }
    }
    for h in &markers.hyperlinks {
        b.empty(
            "Hyperlink",
            &[
                ("Self", h.self_id.as_str()),
                ("Name", h.name.as_str()),
                ("Source", h.source.as_str()),
                ("Destination", h.destination.as_str()),
                ("Visible", "true"),
                ("Hidden", "false"),
            ],
        );
    }
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
