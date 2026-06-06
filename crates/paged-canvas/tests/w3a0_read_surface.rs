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

//! W3.A0 — read-surface completion. One wire/unit test per gap closed:
//!   1. the `spreads` collection carries live guides after `InsertGuide`;
//!   2. the TextFrame `nextTextFrame` / `previousTextFrame` entries
//!      reflect a `LinkFrames` mutation, and writing them is rejected;
//!   3. the `frameFlipV` entry round-trips with its `FrameFlipV` apply;
//!   4. an image-bearing Rectangle exposes `imageContentTransform`;
//!   5. the `stories` collection returns summaries carrying overset flags.

use std::io::Write;

use paged_canvas::{
    channel::{CollectionName, GuideSummary, Mutation, SpreadSummary, StorySummary},
    element_selection::ElementId,
    CanvasModel, CanvasOptions,
};
use paged_mutate::operation::GuideOrientationSpec;
use paged_mutate::{PropertyPath, Value};

/// Two threaded-able text frames (each with its own story), an
/// image-bearing rectangle, and a plain colour-swatch rectangle on a
/// single-page spread `s1`.
fn small_idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story2.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tfA" ParentStory="story1" GeometricBounds="100 100 200 300" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tfB" GeometricBounds="300 100 400 300" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tfC" ParentStory="story2" GeometricBounds="500 100 600 300" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="imgR" GeometricBounds="50 350 250 550" ItemTransform="1 0 0 1 0 0">
  <Image Self="img1" ItemTransform="2 0 0 2 10 20">
    <Properties><Profile type="string">$ID/Embedded</Profile></Properties>
    <Link Self="link1" LinkResourceURI="file:///placeholder.jpg"/>
  </Image>
</Rectangle>
<Rectangle Self="plainR" GeometricBounds="450 350 550 550" ItemTransform="1 0 0 1 0 0" FillColor="Color/Red"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Story one body text</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story2.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story2">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Story two body</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    CanvasModel::load("w3a0", &small_idml(), CanvasOptions::default()).expect("load")
}

fn spreads_collection(m: &CanvasModel) -> Vec<SpreadSummary> {
    let v = m.collection(CollectionName::Spreads);
    serde_json::from_value(v).expect("spreads collection deserializes to SpreadSummary[]")
}

fn entry(
    props: &paged_canvas::channel::ElementProperties,
    path: PropertyPath,
) -> &paged_canvas::channel::PropertyEntry {
    props
        .entries
        .iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("expected {path:?} entry; got {:?}", props.entries))
}

// ── Gap 1: live guides on the spreads collection ─────────────────────

#[test]
fn spreads_collection_carries_guides_after_insert_guide() {
    let mut m = model();

    // No guides authored at load.
    let before = spreads_collection(&m);
    assert_eq!(before.len(), 1, "one spread");
    assert!(before[0].guides.is_empty(), "no guides at load");

    // Insert a vertical guide on spread s1.
    let out = m
        .apply_mutation(&Mutation::InsertGuide {
            spread_id: "s1".into(),
            orientation: GuideOrientationSpec::Vertical,
            position: 144.0,
            page_index: 0,
        })
        .expect("insert guide applies");
    assert!(out.applied_seq > 0);

    // The spreads collection now reflects the live guide.
    let after = spreads_collection(&m);
    let guides: &[GuideSummary] = &after[0].guides;
    assert_eq!(guides.len(), 1, "one live guide after insert; got {guides:?}");
    assert_eq!(guides[0].id, "Guide/s1/0", "positional id matches mint");
    assert!((guides[0].position - 144.0).abs() < 1e-3);
    assert_eq!(guides[0].page_index, 0);

    // Undo drops it back out of the collection (the editor re-queries
    // after undo to re-sync its overlay mirror).
    m.undo().expect("undo");
    let restored = spreads_collection(&m);
    assert!(
        restored[0].guides.is_empty(),
        "guide gone after undo; got {:?}",
        restored[0].guides,
    );
}

// ── Gap 2: thread-chain read + read-only contract ────────────────────

#[test]
fn next_and_previous_text_frame_entries_reflect_link_frames() {
    let mut m = model();
    let id_a = ElementId::TextFrame("tfA".into());
    let id_b = ElementId::TextFrame("tfB".into());

    // Unthreaded: both empty.
    let pa = m.element_properties(&id_a).expect("props A");
    assert_eq!(
        entry(&pa, PropertyPath::NextTextFrame).value,
        Some(Value::Text(String::new())),
        "tfA has no next before linking",
    );
    assert_eq!(
        entry(&pa, PropertyPath::PreviousTextFrame).value,
        Some(Value::Text(String::new())),
        "tfA has no previous before linking",
    );

    // Thread tfA -> tfB.
    m.apply_mutation(&Mutation::LinkFrames {
        from: "tfA".into(),
        to: "tfB".into(),
    })
    .expect("link frames applies");

    let pa = m.element_properties(&id_a).expect("props A");
    assert_eq!(
        entry(&pa, PropertyPath::NextTextFrame).value,
        Some(Value::Text("tfB".into())),
        "tfA.next now points at tfB",
    );
    let pb = m.element_properties(&id_b).expect("props B");
    assert_eq!(
        entry(&pb, PropertyPath::PreviousTextFrame).value,
        Some(Value::Text("tfA".into())),
        "tfB.previous derived as tfA",
    );
    assert_eq!(
        entry(&pb, PropertyPath::NextTextFrame).value,
        Some(Value::Text(String::new())),
        "tfB ends the chain",
    );
}

#[test]
fn next_text_frame_is_read_only_via_set_property() {
    let mut m = model();
    // The read-only paths have no apply arm — a SetElementProperty
    // carrying one is rejected (UnsupportedProperty), never silently
    // applied.
    let err = m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::TextFrame("tfA".into()),
        path: PropertyPath::NextTextFrame,
        value: Value::Text("tfB".into()),
    });
    assert!(
        err.is_err(),
        "writing nextTextFrame via SetProperty must be rejected; got {err:?}",
    );
}

// ── Gap 3: frameFlipV read round-trips with its apply ────────────────

#[test]
fn frame_flip_v_entry_present_and_round_trips_through_apply() {
    let mut m = model();
    let id = ElementId::Rectangle("plainR".into());

    // The gap closed here is "the FrameFlipV apply arm has no read-side
    // entry" — so the load-time read carries a `FrameFlipV` entry (it
    // was absent before W3.A0).
    let props = m.element_properties(&id).expect("props");
    let flip_v = entry(&props, PropertyPath::FrameFlipV);
    assert_eq!(flip_v.value, Some(Value::Bool(false)), "no flip at load");

    // The W0.3 FrameFlipV arm applies cleanly with the entry present;
    // re-reading still yields the entry. NOTE: a vertical mirror is not
    // recoverable as `flip_v` from the matrix alone (`decompose_transform`
    // folds any single reflection into `flip_h` + a 180° rotation, so
    // `flip_v` reads `false`); the read entry exists for the editor's
    // toggle, and the H half flips to reflect the negative-determinant
    // matrix.
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path: PropertyPath::FrameFlipV,
        value: Value::Bool(true),
    })
    .expect("flip applies");

    let props = m.element_properties(&id).expect("props after flip");
    assert!(
        props.entries.iter().any(|e| e.path == PropertyPath::FrameFlipV),
        "FrameFlipV entry still present after the apply",
    );
    assert_eq!(
        entry(&props, PropertyPath::FrameFlipH).value,
        Some(Value::Bool(true)),
        "the y-mirror surfaces as flip_h (matrix is reflection-lossy on V)",
    );
}

// ── Gap 4: imageContentTransform on image-bearing rectangles ─────────

#[test]
fn image_content_transform_entry_present_for_image_rect() {
    let m = model();

    // Image-bearing rectangle exposes the inner image transform.
    let img = m
        .element_properties(&ElementId::Rectangle("imgR".into()))
        .expect("props for image rect");
    assert_eq!(
        entry(&img, PropertyPath::ImageContentTransform).value,
        Some(Value::Transform(Some([2.0, 0.0, 0.0, 2.0, 10.0, 20.0]))),
        "imageContentTransform mirrors the parsed <Image> ItemTransform",
    );

    // Plain colour-swatch rectangle does NOT (nothing to grab).
    let plain = m
        .element_properties(&ElementId::Rectangle("plainR".into()))
        .expect("props for plain rect");
    assert!(
        !plain
            .entries
            .iter()
            .any(|e| e.path == PropertyPath::ImageContentTransform),
        "plain swatch rect should not carry imageContentTransform",
    );
}

// ── Gap 5: stories collection with overset flags ─────────────────────

#[test]
fn stories_collection_returns_summaries_with_overset_flags() {
    let m = model();
    let v = m.collection(CollectionName::Stories);
    let stories: Vec<StorySummary> =
        serde_json::from_value(v).expect("stories collection deserializes to StorySummary[]");
    assert_eq!(stories.len(), 2, "two stories; got {stories:?}");

    // Same construction the bespoke accessor / paged.stories() builds
    // (StorySummary isn't PartialEq, so compare via the wire JSON).
    assert_eq!(
        serde_json::to_value(&stories).unwrap(),
        serde_json::to_value(m.stories()).unwrap(),
        "collection reuses stories() exactly",
    );

    // Both stories fit their frames here — overset present and false.
    for s in &stories {
        assert!(!s.self_id.is_empty());
        assert!(!s.overset, "{} should not be overset", s.self_id);
    }

    // The string round-trips through CollectionName::from_str (the
    // path paged.collection("stories") takes).
    assert_eq!(CollectionName::from_str("stories"), Some(CollectionName::Stories));
}
