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

//! An `.idml` exported from a document that was LOADED FROM A `.paged`
//! must be a pure IDML package — no `document.pgm`, no plugin parts, no
//! `manifest.json`.
//!
//! The carry-through writer copied every source entry verbatim, so the
//! export was the container under another name. The engine's own load
//! sniff (`CanvasModel::load`) prefers a carried `document.pgm` over the
//! IDML parts, which is how an IDML-parity gate re-opened an "IDML" and
//! compared the model with itself: zero differing pages, every time.

use std::io::{Cursor, Read, Write};

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};

fn small_idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="20.0" Self="d1" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<idPkg:Spread src="Spreads/Spread_s1.xml"/>
<idPkg:Story src="Stories/Story_story1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", deflated)
            .unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Hello world</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn entry_names(package: &[u8]) -> Vec<String> {
    let zip = zip::ZipArchive::new(Cursor::new(package)).expect("zip");
    zip.file_names().map(|n| n.to_string()).collect()
}

fn entry(package: &[u8], name: &str) -> Option<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(package)).expect("zip");
    let mut e = zip.by_name(name).ok()?;
    let mut out = String::new();
    e.read_to_string(&mut out).unwrap();
    Some(out)
}

fn story_text(model: &CanvasModel) -> String {
    model.scene().stories[0]
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.clone())
        .collect()
}

#[test]
fn an_idml_exported_from_a_loaded_paged_is_pure_idml() {
    // 1. Author a `.paged`: the native model part + a plugin part + manifest.
    let mut authored =
        CanvasModel::load("doc", &small_idml(), CanvasOptions::default()).expect("load idml");
    authored
        .set_paged_part(
            "paged/demo/state.json".to_string(),
            br#"{"plugin":"demo"}"#.to_vec(),
        )
        .expect("plugin part");
    let paged = authored.export_paged(61).expect("export_paged");
    let paged_names = entry_names(&paged);
    for must in [
        paged_store::DOCUMENT_PGM_PATH,
        "paged/demo/state.json",
        idml_export::MANIFEST_NAME,
    ] {
        assert!(
            paged_names.iter().any(|n| n == must),
            "the .paged fixture must carry {must}; has {paged_names:?}"
        );
    }

    // 2. Load THAT (the sniff takes the native part), edit the story so
    //    the IDML parts must be re-derived, and export as `.idml`.
    let mut model = CanvasModel::load("doc", &paged, CanvasOptions::default()).expect("load paged");
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "story1".into(),
            offset: 0,
            text: "Edited ".into(),
            cell: None,
        })
        .expect("insert text");
    let idml = model.export_idml().expect("export_idml");

    // 3. Pure: no container part survives.
    let names = entry_names(&idml);
    assert!(
        names.iter().all(|n| !idml_export::is_container_part(n)),
        "an .idml must carry no `paged/` part and no manifest.json; got {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == paged_store::DOCUMENT_PGM_PATH),
        "document.pgm must not ride along"
    );
    assert!(!names.iter().any(|n| n == "paged/demo/state.json"));
    assert!(!names.iter().any(|n| n == idml_export::MANIFEST_NAME));
    assert_eq!(names[0], "mimetype");

    // 4. The IDML parts ARE the truth: the edit is in the story XML and a
    //    reload — which now has no native part to prefer — sees it.
    let story_xml = entry(&idml, "Stories/Story_story1.xml").expect("story part");
    assert!(
        story_xml.contains("Edited Hello world"),
        "the story part must carry the edit: {story_xml}"
    );
    let archive = idml_import::open_source_archive(&idml).expect("archive");
    assert!(
        archive.entry(paged_store::DOCUMENT_PGM_PATH).is_none(),
        "the load sniff must find no native part in a pure .idml"
    );
    let reloaded = CanvasModel::load("doc", &idml, CanvasOptions::default()).expect("reload idml");
    assert_eq!(story_text(&reloaded), "Edited Hello world");
}
