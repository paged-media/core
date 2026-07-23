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

//! W3.B2 — the worker-side IDML save-back, exercised headlessly (no
//! wasm). `CanvasModel::export_idml` hands the retained source bytes to
//! `paged_write::write_idml`; the wasm dispatch is a thin map around
//! exactly this call. Two guarantees:
//!
//! 1. An **unmutated** load → export round-trips byte-identically to the
//!    source package (the carry-through writer is a pure pass-through
//!    when nothing diverged from the model).
//! 2. A **mutated** load (one `SetProperty`, via `Mutation::ResizeFrame`)
//!    → export re-parses with the mutation present, and the exported
//!    bytes are still a valid, re-openable IDML package.

use std::io::Write;

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};
use paged_scene::Document;

fn small_idml() -> Vec<u8> {
    idml_with_story_text("Hello world")
}

/// Build a one-spread / one-story IDML package whose body story carries
/// `content`. mimetype is STORED + first; everything else is DEFLATED —
/// the shape a real `.idml` ships in, so the writer's verbatim
/// `raw_copy_file` path is exercised against compressed entries.
fn idml_with_story_text(content: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();

        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("META-INF/container.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="r1" GeometricBounds="50 50 200 200" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", deflated)
            .unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>{content}</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#
            )
            .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

/// Fetch tf1's bounds (top, left, bottom, right) from a parsed package.
fn frame_bounds(doc: &Document) -> (f32, f32, f32, f32) {
    let f = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some("tf1"))
        .expect("tf1 present");
    (f.bounds.top, f.bounds.left, f.bounds.bottom, f.bounds.right)
}

#[test]
fn unmutated_export_is_byte_identical_to_source() {
    let source = small_idml();
    let model = CanvasModel::load("doc1", &source, CanvasOptions::default()).expect("load");

    let exported = model.export_idml().expect("export");

    // The whole package is reproduced — entry order, compression, the
    // stored-mimetype rule — because the carry-through writer is a pure
    // pass-through when nothing diverged from the model.
    assert_eq!(
        source, exported,
        "unmutated export must be byte-identical to the source package"
    );

    // And it is, of course, still a parseable IDML.
    paged_parse::import_idml(&exported)
        .expect("exported package re-parses")
        .0;
}

#[test]
fn mutated_export_reparses_with_the_mutation_present() {
    let source = small_idml();
    let mut model = CanvasModel::load("doc1", &source, CanvasOptions::default()).expect("load");

    // One SetProperty: ResizeFrame routes through
    // `Operation::SetProperty { FrameTransform/bounds }` (see the
    // frame_mutation_bridge integration test). Channel coords are
    // (top, left, bottom, right).
    let new_bounds = (120.0, 130.0, 420.0, 430.0);
    model
        .apply_mutation(&Mutation::ResizeFrame {
            frame_id: "tf1".into(),
            bounds: new_bounds,
        })
        .expect("resize");

    let exported = model.export_idml().expect("export");

    // The mutation diverged the spread, so the export is NOT byte-
    // identical to the source.
    assert_ne!(
        source, exported,
        "a mutated export must differ from the source"
    );

    // Re-parse: the new bounds are present, story text untouched.
    let re = paged_parse::import_idml(&exported)
        .expect("mutated export re-parses")
        .0;
    let (top, left, bottom, right) = frame_bounds(&re);
    assert!((top - new_bounds.0).abs() < 1e-3, "top: {top}");
    assert!((left - new_bounds.1).abs() < 1e-3, "left: {left}");
    assert!((bottom - new_bounds.2).abs() < 1e-3, "bottom: {bottom}");
    assert!((right - new_bounds.3).abs() < 1e-3, "right: {right}");

    let story_text: String = re
        .stories
        .iter()
        .flat_map(|s| s.story.paragraphs.iter())
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.clone())
        .collect();
    assert_eq!(story_text, "Hello world", "story content survived the edit");
}

#[test]
fn export_reflects_the_latest_load_not_a_stale_package() {
    // The retained source bytes are replaced wholesale on each load, so
    // exporting after a re-load reflects the new package, never the old.
    let first = small_idml();
    let model = CanvasModel::load("doc1", &first, CanvasOptions::default()).expect("load");
    assert_eq!(model.export_idml().expect("export"), first);

    // A second, genuinely-different package: the export must match the
    // SECOND source, proving the retained bytes were replaced.
    let second = idml_with_story_text("Goodbye all");
    assert_ne!(
        first, second,
        "fixtures must differ for this test to mean anything"
    );

    let model2 = CanvasModel::load("doc2", &second, CanvasOptions::default()).expect("reload");
    assert_eq!(
        model2.export_idml().expect("export"),
        second,
        "export must reflect the most recently loaded package"
    );
}
