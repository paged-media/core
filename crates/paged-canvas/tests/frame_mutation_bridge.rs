//! Phase B — `paged_mutate` bridge integration test.
//!
//! Drives the channel `Mutation::ResizeFrame` through `apply_mutation`,
//! confirms it lands as an `paged_mutate::Operation` on the unified
//! undo log, and round-trips through undo/redo.

use std::io::Write;

use paged_canvas::{
    channel::Mutation, CanvasModel, CanvasOptions, LoggedMutation,
};

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
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
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
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
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

#[test]
fn resize_frame_routes_through_idml_mutate_and_logs() {
    let bytes = small_idml();
    let mut model =
        CanvasModel::load("doc1", &bytes, CanvasOptions::default()).expect("load");

    // Move tf1's bounds. Channel coords are (top, left, bottom, right).
    let new_bounds = (110.0, 110.0, 410.0, 410.0);
    let mutation = Mutation::ResizeFrame {
        frame_id: "tf1".into(),
        bounds: new_bounds,
    };
    let outcome = model.apply_mutation(&mutation).expect("apply");
    assert!(outcome.applied_seq > 0);

    // Verify the scene actually moved.
    let frame = model
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some("tf1"))
        .expect("frame found");
    assert!((frame.bounds.top - new_bounds.0).abs() < 1e-3);
    assert!((frame.bounds.right - new_bounds.3).abs() < 1e-3);

    // Confirm the log got the canonical AppliedOperation, not a stray
    // TextOp entry.
    let last = model.applied_log_back();
    let kind = last.expect("log entry").kind.clone();
    match kind {
        LoggedMutation::Frame(applied) => {
            assert!(matches!(
                applied.op,
                paged_mutate::Operation::SetProperty { .. }
            ));
            assert!(matches!(
                applied.inverse,
                paged_mutate::Operation::SetProperty { .. }
            ));
        }
        LoggedMutation::Text { .. } => panic!("expected Frame entry, got Text"),
    }
}

#[test]
fn frame_resize_undo_redo_round_trip_restores_scene() {
    let bytes = small_idml();
    let mut model =
        CanvasModel::load("doc1", &bytes, CanvasOptions::default()).expect("load");

    let mutation = Mutation::ResizeFrame {
        frame_id: "tf1".into(),
        bounds: (110.0, 110.0, 410.0, 410.0),
    };
    let initial_top = 100.0_f32;
    model.apply_mutation(&mutation).expect("apply");

    // Undo restores.
    let undo_outcome = model.undo().expect("undo");
    assert_eq!(undo_outcome.affected_story_id, None);
    let frame = model
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some("tf1"))
        .expect("frame found");
    assert!((frame.bounds.top - initial_top).abs() < 1e-3);

    // Redo re-applies.
    let redo_outcome = model.redo().expect("redo");
    assert_eq!(redo_outcome.affected_story_id, None);
    let frame = model
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some("tf1"))
        .expect("frame found");
    assert!((frame.bounds.top - 110.0_f32).abs() < 1e-3);
}

#[test]
fn unified_undo_alternates_text_and_frame_entries() {
    let bytes = small_idml();
    let mut model =
        CanvasModel::load("doc1", &bytes, CanvasOptions::default()).expect("load");

    // Text mutation first.
    model
        .apply_mutation(&Mutation::InsertText {
            story_id: "story1".into(),
            offset: 5,
            text: "!".into(),
        })
        .expect("text apply");
    // Then a frame resize.
    model
        .apply_mutation(&Mutation::ResizeFrame {
            frame_id: "tf1".into(),
            bounds: (110.0, 110.0, 410.0, 410.0),
        })
        .expect("frame apply");

    // Two entries on the undo log.
    assert_eq!(model.applied_log_len(), 2);

    // Undo pops the frame entry first.
    model.undo().expect("undo frame");
    assert_eq!(model.applied_log_len(), 1);
    // Then the text entry.
    model.undo().expect("undo text");
    assert_eq!(model.applied_log_len(), 0);
}

#[test]
fn rectangle_resize_also_bridges() {
    let bytes = small_idml();
    let mut model =
        CanvasModel::load("doc1", &bytes, CanvasOptions::default()).expect("load");

    model
        .apply_mutation(&Mutation::ResizeFrame {
            frame_id: "r1".into(),
            bounds: (60.0, 60.0, 210.0, 210.0),
        })
        .expect("rect apply");

    let rect = model
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find(|r| r.self_id.as_deref() == Some("r1"))
        .expect("rect found");
    assert!((rect.bounds.top - 60.0_f32).abs() < 1e-3);
}
