//! Track L.2 — gesture spine drives Group translate.
//!
//! Begins a translate gesture on a Group element, updates with a
//! pointer delta, commits, and verifies:
//!   - the Group's `item_transform` shifted by (dx, dy)
//!   - every member's `item_transform` shifted by (dx, dy) (Track L
//!     forces the transform path on members inside a group-targeted
//!     session, so the parser-baked composition stays consistent)
//!   - Cmd-Z restores the Group and every member to their pre-
//!     gesture transforms

use std::io::Write;
use std::path::PathBuf;

use paged_canvas::{
    channel::Mutation, gesture::GestureAnchor, gesture::GestureType, CanvasModel, CanvasOptions,
    ElementId, GestureModifiers, PageId,
};

fn font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
}
fn read_font(name: &str) -> Vec<u8> {
    std::fs::read(font_dir().join(name))
        .unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
}

/// Synthesise a tiny IDML with one Spread containing a Group that
/// hosts two TextFrames. The group has no item_transform; the
/// leaves carry their bounds directly. Mirrors the simplest real
/// InDesign Group layout.
fn build_idml_with_group() -> Vec<u8> {
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/vnd.adobe.indesign-idml-package")
        .unwrap();
    zip.start_file("designmap.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_st1.xml"/>
  <idPkg:Story src="Stories/Story_st2.xml"/>
</Document>"#,
    )
    .unwrap();
    zip.start_file("Resources/Graphic.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
    )
    .unwrap();
    zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <Group Self="g1">
      <TextFrame Self="leafA" ParentStory="st1" GeometricBounds="0 0 50 50" StrokeWeight="0"/>
      <TextFrame Self="leafB" ParentStory="st2" GeometricBounds="0 60 50 110" StrokeWeight="0"/>
    </Group>
  </Spread>
</idPkg:Spread>"#,
    )
    .unwrap();
    for (story_id, content) in [("st1", "A"), ("st2", "B")] {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="{story_id}">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="12">
        <Content>{content}</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
        );
        zip.start_file(format!("Stories/Story_{story_id}.xml"), deflated)
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn load_model() -> CanvasModel {
    let bytes = build_idml_with_group();
    let opts = CanvasOptions {
        fonts: vec![read_font("Inter.ttf")],
        ..Default::default()
    };
    CanvasModel::load("doc", &bytes, opts).expect("load + build")
}

fn leaf_transform(model: &CanvasModel, self_id: &str) -> Option<[f32; 6]> {
    for parsed in &model.scene().spreads {
        for f in &parsed.spread.text_frames {
            if f.self_id.as_deref() == Some(self_id) {
                return f.item_transform;
            }
        }
    }
    panic!("leaf {self_id} not found");
}

fn leaf_bounds(model: &CanvasModel, self_id: &str) -> (f32, f32, f32, f32) {
    for parsed in &model.scene().spreads {
        for f in &parsed.spread.text_frames {
            if f.self_id.as_deref() == Some(self_id) {
                return (f.bounds.top, f.bounds.left, f.bounds.bottom, f.bounds.right);
            }
        }
    }
    panic!("leaf {self_id} not found");
}

fn group_transform(model: &CanvasModel) -> Option<[f32; 6]> {
    for parsed in &model.scene().spreads {
        for g in &parsed.spread.groups {
            if g.self_id.as_deref() == Some("g1") {
                return g.item_transform;
            }
        }
    }
    panic!("group g1 not found");
}

#[test]
fn translate_group_shifts_group_transform_and_each_member() {
    let mut model = load_model();
    let group_id = ElementId::Group("g1".to_string());

    // Sanity: before any gesture, group has no transform and
    // leaves have their bounds set, no transforms.
    assert_eq!(group_transform(&model), None);
    assert_eq!(leaf_transform(&model, "leafA"), None);
    assert_eq!(leaf_transform(&model, "leafB"), None);
    let leaf_a_bounds_before = leaf_bounds(&model, "leafA");
    let leaf_b_bounds_before = leaf_bounds(&model, "leafB");

    let handle = model
        .begin_gesture(vec![group_id], GestureType::Translate, None)
        .expect("begin");
    let result = model
        .update_gesture(
            handle,
            (10.0, 20.0),
            GestureModifiers {
                shift: false,
                alt: false, disable_snap: false,
            },
        )
        .expect("update");
    assert!(!result.page_ids.is_empty());
    model.commit_gesture(handle).expect("commit");

    // Group's item_transform now carries the (10, 20) translation.
    let g = group_transform(&model).expect("group transform set");
    assert!((g[4] - 10.0).abs() < 1e-4, "tx={}", g[4]);
    assert!((g[5] - 20.0).abs() < 1e-4, "ty={}", g[5]);
    assert!((g[0] - 1.0).abs() < 1e-4); // linear part stays identity
    assert!((g[3] - 1.0).abs() < 1e-4);

    // Each leaf's item_transform also carries the same translation
    // (Track L's force-transform-path-on-group-session rule).
    for leaf in ["leafA", "leafB"] {
        let m = leaf_transform(&model, leaf)
            .unwrap_or_else(|| panic!("{leaf} should have item_transform after group translate"));
        assert!((m[4] - 10.0).abs() < 1e-4, "{leaf} tx={}", m[4]);
        assert!((m[5] - 20.0).abs() < 1e-4, "{leaf} ty={}", m[5]);
    }

    // Bounds untouched on the leaves — Track L specifically
    // routes translates through item_transform inside a group-
    // session so reserialization round-trips against the group's
    // parser-baked transform.
    assert_eq!(leaf_bounds(&model, "leafA"), leaf_a_bounds_before);
    assert_eq!(leaf_bounds(&model, "leafB"), leaf_b_bounds_before);
}

#[test]
fn translate_group_undo_restores_group_and_members() {
    let mut model = load_model();
    let group_id = ElementId::Group("g1".to_string());
    let leaf_a_bounds_before = leaf_bounds(&model, "leafA");

    let handle = model
        .begin_gesture(vec![group_id], GestureType::Translate, None)
        .expect("begin");
    model
        .update_gesture(
            handle,
            (33.0, -17.5),
            GestureModifiers {
                shift: false,
                alt: false, disable_snap: false,
            },
        )
        .expect("update");
    let commit = model.commit_gesture(handle).expect("commit");
    assert!(commit.applied_seq > 0);

    // Undo via the model's undo path (mirrors the channel's Undo
    // dispatch). `apply_mutation` isn't used here because the
    // gesture commit goes through `apply_operation`; the undo
    // log path picks up the captured inverse Batch.
    model.undo().expect("undo");

    // Group transform reset; each leaf back at its prior state
    // (no transform; bounds unchanged).
    assert_eq!(group_transform(&model), None);
    assert_eq!(leaf_transform(&model, "leafA"), None);
    assert_eq!(leaf_transform(&model, "leafB"), None);
    assert_eq!(leaf_bounds(&model, "leafA"), leaf_a_bounds_before);
}

/// The channel-level `Mutation` enum has its own routing for
/// per-leaf moveFrame ops; this test confirms that the gesture's
/// canonical commit Batch is what paged-mutate sees, regardless of
/// which channel-level wrapper a future caller might use.
#[test]
fn translate_group_dispatches_a_single_undo_entry() {
    let mut model = load_model();
    let group_id = ElementId::Group("g1".to_string());
    let _ = Mutation::DeleteFrame {
        frame_id: "ignored".to_string(),
    }; // touch the enum so the import stays.

    let handle = model
        .begin_gesture(vec![group_id], GestureType::Translate, None)
        .expect("begin");
    model
        .update_gesture(
            handle,
            (5.0, 5.0),
            GestureModifiers {
                shift: false,
                alt: false, disable_snap: false,
            },
        )
        .expect("update");
    let outcome = model.commit_gesture(handle).expect("commit");
    // One applied_seq increment for the whole gesture — i.e. the
    // commit pushed a SINGLE entry on the undo log, even though
    // the batch internally contains (group + 2 leaves) = 3
    // SetProperty children.
    assert_eq!(outcome.applied_seq, 1);
}

/// Rotate on a Group target should compose the same rotation onto
/// the Group's `item_transform` AND every member's. The pivot
/// resolves to the union centroid of the snapshots; with the
/// Group's zero-bounds sentinel the centroid lands at the
/// members' centroid (correct intent).
#[test]
fn rotate_group_composes_same_rotation_onto_group_and_members() {
    let mut model = load_model();
    let group_id = ElementId::Group("g1".to_string());
    let anchor = GestureAnchor {
        page_id: PageId("p1".to_string()),
        point_in_page: (55.0, 25.0), // roughly the centroid of (0..50, 0..50) + (60..110, 0..50)
    };

    let handle = model
        .begin_gesture(vec![group_id], GestureType::Rotate, Some(anchor))
        .expect("begin rotate");
    // Phase D's rotate gesture interprets `delta` as a pointer
    // movement; the resulting angle is delta.1 — i.e. drag down
    // = positive rotation. A delta of (0, 30) gives a positive
    // angle in pt-equivalent units.
    model
        .update_gesture(
            handle,
            (0.0, 30.0),
            GestureModifiers {
                shift: false,
                alt: false, disable_snap: false,
            },
        )
        .expect("update");
    model.commit_gesture(handle).expect("commit");

    // Group's transform now has a non-trivial rotation (linear
    // part differs from identity).
    let g = group_transform(&model).expect("group transform set");
    assert!(
        (g[0] - 1.0).abs() > 1e-3 || g[1].abs() > 1e-3,
        "group's linear part should differ from identity after rotate: {:?}",
        g,
    );

    // Each leaf has its own rotated transform — same linear
    // 2×2 as the group (rotation is rigid), but with a different
    // translation reflecting the leaf's pivot offset.
    let m_a = leaf_transform(&model, "leafA").expect("leafA transform after rotate");
    let m_b = leaf_transform(&model, "leafB").expect("leafB transform after rotate");
    for i in 0..4 {
        // Linear part matches the group's linear part to float tol.
        assert!(
            (m_a[i] - g[i]).abs() < 1e-3,
            "leafA m[{i}]={} != g[{i}]={}",
            m_a[i],
            g[i],
        );
        assert!(
            (m_b[i] - g[i]).abs() < 1e-3,
            "leafB m[{i}]={} != g[{i}]={}",
            m_b[i],
            g[i],
        );
    }
}

/// Scale on a Group target composes onto the linear part. The
/// member transforms get the same linear scaling — un-rotated
/// before becomes still-un-rotated but with non-identity
/// diagonal entries.
#[test]
fn scale_group_composes_same_scale_onto_group_and_members() {
    let mut model = load_model();
    let group_id = ElementId::Group("g1".to_string());
    let anchor = GestureAnchor {
        page_id: PageId("p1".to_string()),
        point_in_page: (55.0, 25.0),
    };

    let handle = model
        .begin_gesture(vec![group_id], GestureType::Scale, Some(anchor))
        .expect("begin scale");
    model
        .update_gesture(
            handle,
            (40.0, 0.0),
            GestureModifiers {
                shift: false,
                alt: false, disable_snap: false,
            },
        )
        .expect("update");
    model.commit_gesture(handle).expect("commit");

    let g = group_transform(&model).expect("group transform set");
    // Scale changes the diagonal entries; off-diagonal stays zero
    // for a pure scale.
    assert!((g[0] - 1.0).abs() > 1e-3, "scaled x: {:?}", g);
    // Each leaf's linear part matches the group's.
    for leaf in ["leafA", "leafB"] {
        let m = leaf_transform(&model, leaf).expect("leaf transform after scale");
        for i in 0..4 {
            assert!(
                (m[i] - g[i]).abs() < 1e-3,
                "{leaf} linear part m[{i}]={} != g[{i}]={}",
                m[i],
                g[i],
            );
        }
    }
}
