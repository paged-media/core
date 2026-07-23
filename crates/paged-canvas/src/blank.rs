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

//! In-engine blank-document synthesis for File ▸ New.
//!
//! The wasm worker exposes no "new empty document" entry, and the only
//! IDML writer ([`paged_write::write_idml`]) *patches an existing
//! package* — so a brand-new document has nothing to patch. Rather than
//! hand-build the resolved [`paged_scene::Document`] graph (which would
//! also leave `source_idml` empty and break save-back), a blank document
//! is produced by emitting the smallest valid IDML *package* here — one
//! empty page at the requested size, with the default master / styles /
//! swatches a parsed IDML carries — and feeding it through the normal
//! [`crate::model::CanvasModel::load`] path. That reuses the real
//! parser and pipeline, so the document is well-formed and `source_idml`
//! is populated exactly as for an opened file.
//!
//! The package shape mirrors the editor's proven E2E fixture builder
//! (`tests/e2e/harness/build-min-idml.ts`) minus the page body. The ZIP
//! is assembled with the same `zip` crate the parser reads and the
//! writer emits — no hand-rolled archive.

use std::io::{Cursor, Write};

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

fn designmap(name: &str) -> String {
    xml(&format!(
        "<?aid style=\"50\" type=\"document\" readerVersion=\"6.0\" featureSet=\"257\" product=\"20.0(32)\"?>\n\
<Document xmlns:idPkg=\"{NS}\" DOMVersion=\"20.0\" Self=\"d\" StoryList=\"\" Name=\"{name}\">\n\
<idPkg:Graphic src=\"Resources/Graphic.xml\"/>\n\
<idPkg:Fonts src=\"Resources/Fonts.xml\"/>\n\
<idPkg:Styles src=\"Resources/Styles.xml\"/>\n\
<idPkg:Preferences src=\"Resources/Preferences.xml\"/>\n\
<idPkg:MasterSpread src=\"MasterSpreads/MasterSpread_um.xml\"/>\n\
<idPkg:Spread src=\"Spreads/Spread_us.xml\"/>\n\
<idPkg:BackingStory src=\"XML/BackingStory.xml\"/>\n\
</Document>"
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

/// An empty one-page spread — the blank canvas. No page items.
fn spread(bounds: &str) -> String {
    xml(&format!(
        "<idPkg:Spread xmlns:idPkg=\"{NS}\" DOMVersion=\"20.0\">\n\
<Spread Self=\"us\" PageCount=\"1\" ItemTransform=\"1 0 0 1 0 0\">\n\
<Page Self=\"usp\" Name=\"1\" GeometricBounds=\"{bounds}\" ItemTransform=\"1 0 0 1 0 0\" AppliedMaster=\"um\"/>\n\
</Spread></idPkg:Spread>"
    ))
}

/// Build the bytes of a blank single-page IDML package sized
/// `width_pt` × `height_pt` (points).
///
/// `GeometricBounds` is InDesign's "y0 x0 y1 x1" order, so a
/// `[width, height]` page is `0 0 height width`. The returned bytes
/// parse through [`paged_scene::Document::open`] like any opened file.
pub fn blank_idml(width_pt: f32, height_pt: f32) -> Vec<u8> {
    let bounds = format!("0 0 {height_pt} {width_pt}");
    let designmap = designmap("Untitled.indd");
    let master = master_spread(&bounds);
    let spread = spread(&bounds);

    let mut zip = zip::write::ZipWriter::new(Cursor::new(Vec::<u8>::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // In-memory writes are infallible; `expect` documents that invariant
    // rather than threading a Result through a deterministic builder.
    let mut put = |name: &str, body: &str, stored_entry: bool| {
        let opts = if stored_entry { stored } else { deflated };
        zip.start_file(name, opts).expect("zip start_file");
        zip.write_all(body.as_bytes()).expect("zip write_all");
    };

    // mimetype first + STORED (OCF convention).
    put("mimetype", MIME, true);
    put("designmap.xml", &designmap, false);
    put("META-INF/container.xml", &container(), false);
    put("Resources/Graphic.xml", &graphic(), false);
    put("Resources/Fonts.xml", &empty_pkg("Fonts"), false);
    put("Resources/Styles.xml", &styles(), false);
    put(
        "Resources/Preferences.xml",
        &empty_pkg("Preferences"),
        false,
    );
    put("MasterSpreads/MasterSpread_um.xml", &master, false);
    put("Spreads/Spread_us.xml", &spread, false);
    put("XML/BackingStory.xml", &backing(), false);

    zip.finish().expect("zip finish").into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_idml_parses_to_one_page() {
        let bytes = blank_idml(612.0, 792.0);
        let doc = paged_parse::import_idml(&bytes)
            .expect("blank IDML must parse")
            .0;
        // One spread, one page, no stories (truly empty body).
        assert_eq!(doc.spreads.len(), 1);
        assert_eq!(doc.spreads[0].spread.pages.len(), 1);
        assert!(doc.stories.is_empty());
    }
}
