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

//! A native [`Document`] as a `.paged` package the engine opens
//! (`host.nativeDocument.open`): an IDML/OCF zip whose
//! [`DOCUMENT_PGM_PATH`](crate::DOCUMENT_PGM_PATH) part carries the model.
//!
//! The IDML half is a one-page FALLBACK skeleton, parsed only if the native
//! part ever fails to decode, so a drift-degraded open is at least the right
//! paper size. The load path reads the pgm. Shared by every plugin that
//! produces documents natively (paged.pdf's mapper, paged.doc's section
//! skeleton) so the container format has one writer.

use std::io::{Cursor, Write};

use paged_scene::Document;

/// IDML/OCF package mimetype. MUST be the first ZIP entry and STORED.
const MIME: &str = "application/vnd.adobe.indesign-idml-package";
const NS: &str = "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging";

fn xml(body: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n{body}")
}

fn empty_pkg(tag: &str) -> String {
    xml(&format!(
        "<idPkg:{tag} xmlns:idPkg=\"{NS}\" DOMVersion=\"20.0\"/>"
    ))
}

fn container() -> String {
    xml(
        "<container xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\" version=\"1.0\">\
<rootfiles><rootfile full-path=\"designmap.xml\" media-type=\"text/xml\"/></rootfiles></container>",
    )
}

fn graphic() -> String {
    xml(&format!(
        "<idPkg:Graphic xmlns:idPkg=\"{NS}\" DOMVersion=\"20.0\">\
<Color Self=\"Color/Black\" Model=\"Process\" Space=\"CMYK\" ColorValue=\"0 0 0 100\" Name=\"Black\"/>\
<Swatch Self=\"Swatch/None\" Name=\"None\"/></idPkg:Graphic>"
    ))
}

fn styles() -> String {
    xml(&format!(
        "<idPkg:Styles xmlns:idPkg=\"{NS}\" DOMVersion=\"20.0\">\
<RootCharacterStyleGroup Self=\"rcs\">\
<CharacterStyle Self=\"CharacterStyle/$ID/[No character style]\" Name=\"$ID/[No character style]\"/>\
</RootCharacterStyleGroup>\
<RootParagraphStyleGroup Self=\"rps\">\
<ParagraphStyle Self=\"ParagraphStyle/$ID/[No paragraph style]\" Name=\"$ID/[No paragraph style]\"/>\
</RootParagraphStyleGroup></idPkg:Styles>"
    ))
}

fn backing() -> String {
    xml(&format!(
        "<idPkg:BackingStory xmlns:idPkg=\"{NS}\" DOMVersion=\"20.0\">\
<XmlStory Self=\"backing\"/></idPkg:BackingStory>"
    ))
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn designmap(name: &str) -> String {
    xml(&format!(
        "<?aid style=\"50\" type=\"document\" readerVersion=\"6.0\" featureSet=\"257\" product=\"20.0(32)\"?>\n\
<Document xmlns:idPkg=\"{NS}\" DOMVersion=\"20.0\" Self=\"d\" StoryList=\"\" Name=\"{}\">\n\
<idPkg:Graphic src=\"Resources/Graphic.xml\"/>\n\
<idPkg:Fonts src=\"Resources/Fonts.xml\"/>\n\
<idPkg:Styles src=\"Resources/Styles.xml\"/>\n\
<idPkg:Preferences src=\"Resources/Preferences.xml\"/>\n\
<idPkg:MasterSpread src=\"MasterSpreads/MasterSpread_um.xml\"/>\n\
<idPkg:Spread src=\"Spreads/Spread_us.xml\"/>\n\
<idPkg:BackingStory src=\"XML/BackingStory.xml\"/>\n\
</Document>",
        escape_attr(name)
    ))
}

fn master_spread(bounds: &str) -> String {
    xml(&format!(
        "<idPkg:MasterSpread xmlns:idPkg=\"{NS}\" DOMVersion=\"20.0\">\
<MasterSpread Self=\"um\" Name=\"A\">\
<Page Self=\"ump\" Name=\"A\" GeometricBounds=\"{bounds}\" ItemTransform=\"1 0 0 1 0 0\"/>\
</MasterSpread></idPkg:MasterSpread>"
    ))
}

fn spread(bounds: &str) -> String {
    xml(&format!(
        "<idPkg:Spread xmlns:idPkg=\"{NS}\" DOMVersion=\"20.0\">\n\
<Spread Self=\"us\" PageCount=\"1\" BindingLocation=\"0\" ItemTransform=\"1 0 0 1 0 0\">\n\
<Page Self=\"usp\" Name=\"1\" GeometricBounds=\"{bounds}\" ItemTransform=\"1 0 0 1 0 0\" AppliedMaster=\"um\"/>\n\
</Spread></idPkg:Spread>"
    ))
}

/// Wrap `doc` in the `.paged` container. `name` is the document name the
/// fallback designmap carries; `fallback_width_pt` x `fallback_height_pt`
/// size its one fallback page.
pub fn wrap_document(
    doc: &Document,
    name: &str,
    fallback_width_pt: f32,
    fallback_height_pt: f32,
) -> Result<Vec<u8>, serde_json::Error> {
    let pgm = crate::to_bytes(doc)?;
    // InDesign's GeometricBounds order is "y0 x0 y1 x1".
    let bounds = format!("0 0 {fallback_height_pt} {fallback_width_pt}");

    let mut zip = zip::write::ZipWriter::new(Cursor::new(Vec::<u8>::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // In-memory writes are infallible; `expect` documents the invariant.
    let mut put = |name: &str, body: &[u8], stored_entry: bool| {
        let opts = if stored_entry { stored } else { deflated };
        zip.start_file(name, opts).expect("zip start_file");
        zip.write_all(body).expect("zip write_all");
    };
    // mimetype first + STORED (OCF convention): the sniff keys on it.
    put("mimetype", MIME.as_bytes(), true);
    put("designmap.xml", designmap(name).as_bytes(), false);
    put("META-INF/container.xml", container().as_bytes(), false);
    put("Resources/Graphic.xml", graphic().as_bytes(), false);
    put("Resources/Fonts.xml", empty_pkg("Fonts").as_bytes(), false);
    put("Resources/Styles.xml", styles().as_bytes(), false);
    put(
        "Resources/Preferences.xml",
        empty_pkg("Preferences").as_bytes(),
        false,
    );
    put(
        "MasterSpreads/MasterSpread_um.xml",
        master_spread(&bounds).as_bytes(),
        false,
    );
    put("Spreads/Spread_us.xml", spread(&bounds).as_bytes(), false);
    put("XML/BackingStory.xml", backing().as_bytes(), false);
    // The native model part: what the load path actually uses.
    put(crate::DOCUMENT_PGM_PATH, &pgm, false);

    Ok(zip.finish().expect("zip finish").into_inner())
}
