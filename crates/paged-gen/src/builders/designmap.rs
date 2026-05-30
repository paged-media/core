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

pub fn write_designmap(dm: &DesignMap) -> Vec<u8> {
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
