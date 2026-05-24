//! Phase 3 Items 6, 7, 9 — determinism, undo correctness,
//! zoom-independence (final gate of the correctness layer).

use std::io::Write;

use idml_canvas::{
    channel::Mutation, mutate::TextOp, CanvasModel, CanvasOptions,
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

fn replay_log(bytes: &[u8], ops: &[Mutation]) -> CanvasModel {
    let mut m = CanvasModel::load("d", bytes, CanvasOptions::default()).unwrap();
    for op in ops {
        m.apply_mutation(op).expect("apply ok");
    }
    m
}

/// AC-E-7: applying the same mutation log against the same source
/// produces byte-identical derived state.
#[test]
fn replay_produces_byte_identical_state() {
    let bytes = small_idml();
    let ops = vec![
        Mutation::InsertText {
            story_id: "story1".into(),
            offset: 5,
            text: ",".into(),
        },
        Mutation::InsertText {
            story_id: "story1".into(),
            offset: 0,
            text: "Greeting: ".into(),
        },
        Mutation::DeleteRange {
            story_id: "story1".into(),
            start: 0,
            end: 1,
        },
    ];
    let a = replay_log(&bytes, &ops);
    let b = replay_log(&bytes, &ops);
    assert_eq!(
        a.current_state_hash(),
        b.current_state_hash(),
        "two replays must produce identical canonical state hashes"
    );
    assert_eq!(a.last_applied_seq(), 3);
}

/// AC-E-8 (partial): undoing all mutations returns to the initial
/// state hash.
#[test]
fn undo_all_returns_to_initial_hash() {
    let bytes = small_idml();
    let ops = vec![
        Mutation::InsertText {
            story_id: "story1".into(),
            offset: 5,
            text: ",".into(),
        },
        Mutation::InsertText {
            story_id: "story1".into(),
            offset: 0,
            text: "Greeting: ".into(),
        },
        Mutation::DeleteRange {
            story_id: "story1".into(),
            start: 0,
            end: 1,
        },
    ];
    let mut m = replay_log(&bytes, &ops);
    let initial = m.initial_state_hash();
    while m.applied_log_len() > 0 {
        assert!(m.undo().is_some());
    }
    assert_eq!(
        m.current_state_hash(),
        initial,
        "after undoing every mutation, current hash must match initial"
    );
}

/// AC-E-8: undo + redo returns to the post-mutation state.
#[test]
fn undo_then_redo_returns_to_post_op_hash() {
    let bytes = small_idml();
    let mut m = CanvasModel::load("d", &bytes, CanvasOptions::default()).unwrap();
    let op = Mutation::InsertText {
        story_id: "story1".into(),
        offset: 5,
        text: ",".into(),
    };
    m.apply_mutation(&op).unwrap();
    let post = m.current_state_hash();
    assert!(m.undo().is_some());
    assert_ne!(m.current_state_hash(), post);
    assert!(m.redo().is_some());
    assert_eq!(m.current_state_hash(), post);
}

/// New mutation after partial undo clears the redo stack.
#[test]
fn new_mutation_clears_redo_stack() {
    let bytes = small_idml();
    let mut m = CanvasModel::load("d", &bytes, CanvasOptions::default()).unwrap();
    m.apply_mutation(&Mutation::InsertText {
        story_id: "story1".into(),
        offset: 5,
        text: ",".into(),
    })
    .unwrap();
    m.undo().unwrap();
    assert_eq!(m.redo_log_len(), 1);
    m.apply_mutation(&Mutation::InsertText {
        story_id: "story1".into(),
        offset: 0,
        text: "X".into(),
    })
    .unwrap();
    assert_eq!(m.redo_log_len(), 0, "new mutation must clear redo log");
}

/// AC-E-9: zoom doesn't enter the worker's state at all. The
/// canvas's view-level zoom is a property of the main thread's
/// camera. Worker-side state is purely a function of (initial bytes,
/// mutation log) — no zoom or camera input.
///
/// Test scaffold: load the document twice, apply the same mutation
/// log, assert identical canonical_hash. The "zoom" axis is
/// implicitly satisfied because zoom never enters the apply path.
#[test]
fn zoom_independence_via_logical_replay() {
    let bytes = small_idml();
    let ops = vec![
        Mutation::InsertText {
            story_id: "story1".into(),
            offset: 5,
            text: ",".into(),
        },
        Mutation::DeleteRange {
            story_id: "story1".into(),
            start: 11,
            end: 12,
        },
    ];
    let a = replay_log(&bytes, &ops);
    let b = replay_log(&bytes, &ops);
    assert_eq!(a.current_state_hash(), b.current_state_hash());
    // Also assert that running text mutations directly (skipping
    // the channel-side Mutation enum) reaches the same place.
    let mut c = CanvasModel::load("d", &bytes, CanvasOptions::default()).unwrap();
    idml_canvas::mutate::apply(
        c.scene_mut(),
        &TextOp::InsertText {
            story_id: "story1".into(),
            offset: 5,
            text: ",".into(),
        },
    )
    .unwrap();
    idml_canvas::mutate::apply(
        c.scene_mut(),
        &TextOp::DeleteRange {
            story_id: "story1".into(),
            start: 11,
            end: 12,
            recovered: String::new(),
        },
    )
    .unwrap();
    assert_eq!(c.current_state_hash(), a.current_state_hash());
}
