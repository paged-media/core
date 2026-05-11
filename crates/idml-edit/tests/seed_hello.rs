//! Integration tests that exercise `Project` against the hello seed.
//!
//! The seed lives in `corpus/seeds/hello/source` as plain XML; we
//! pack it into a valid IDML container at run time so the suite has
//! no external fixture dependency. This is the same pattern the
//! renderer tests use.

use std::io::Write;
use std::path::Path;

use idml_edit::{
    command::{ParagraphAttrPatch, RectanglePayloadBounds, RunAttrPatch},
    hit_test_spread, Command, NodeId, ParaId, Project, StoryId,
};
use idml_parse::Justification;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn pack_seed(seed_dir: &Path) -> Vec<u8> {
    let buf = std::io::Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mimetype = std::fs::read(seed_dir.join("mimetype")).expect("mimetype");
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(&mimetype).unwrap();

    fn walk(
        zip: &mut ZipWriter<std::io::Cursor<Vec<u8>>>,
        opts: SimpleFileOptions,
        root: &Path,
        prefix: &str,
    ) {
        for entry in std::fs::read_dir(root).expect("read seed dir") {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == "mimetype" {
                continue;
            }
            let path = entry.path();
            let archive_path = if prefix.is_empty() {
                name_str.to_string()
            } else {
                format!("{prefix}/{name_str}")
            };
            if path.is_dir() {
                walk(zip, opts, &path, &archive_path);
            } else {
                let bytes = std::fs::read(&path).expect("read seed file");
                zip.start_file(&archive_path, opts).unwrap();
                zip.write_all(&bytes).unwrap();
            }
        }
    }
    walk(&mut zip, deflated, seed_dir, "");

    zip.finish().unwrap().into_inner()
}

fn seed_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/seeds/hello/source")
}

fn open_seed_project() -> Project {
    let bytes = pack_seed(&seed_dir());
    Project::open(&bytes).expect("open seed IDML as Project")
}

fn frame_translation(p: &Project, frame_id: &str) -> (f32, f32) {
    let frame = p
        .document()
        .text_frame(frame_id)
        .expect("frame in working doc");
    let it = frame
        .item_transform
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    (it[4], it[5])
}

#[test]
fn project_opens_and_reports_stats() {
    let p = open_seed_project();
    let s = p.stats();
    assert_eq!(s.spreads, 2);
    assert_eq!(s.stories, 2);
    assert_eq!(s.master_spreads, 1);
    assert!(s.text_frames >= 2);
    assert_eq!(p.epoch(), 0);
    assert!(!p.can_undo());
    assert!(!p.can_redo());
}

#[test]
fn move_frame_translates_item_transform() {
    let mut p = open_seed_project();
    let (tx0, ty0) = frame_translation(&p, "ua-body");

    let cmd = Command::MoveFrame {
        frame: NodeId::Frame("ua-body".into()),
        dx_pt: 12.0,
        dy_pt: -7.5,
        transient: false,
    };
    let patch = p.apply(cmd).expect("apply MoveFrame");
    assert_eq!(patch.epoch, 1);
    assert!(!patch.is_empty());

    let (tx1, ty1) = frame_translation(&p, "ua-body");
    assert!((tx1 - (tx0 + 12.0)).abs() < 1e-3);
    assert!((ty1 - (ty0 - 7.5)).abs() < 1e-3);
    assert!(p.can_undo());
}

#[test]
fn undo_redo_round_trips_translation() {
    let mut p = open_seed_project();
    let (tx0, ty0) = frame_translation(&p, "ua-body");

    p.apply(Command::MoveFrame {
        frame: NodeId::Frame("ua-body".into()),
        dx_pt: 5.0,
        dy_pt: 5.0,
        transient: false,
    })
    .unwrap();

    p.undo();
    let (tx1, ty1) = frame_translation(&p, "ua-body");
    assert!((tx1 - tx0).abs() < 1e-3, "undo restores tx");
    assert!((ty1 - ty0).abs() < 1e-3, "undo restores ty");
    assert!(!p.can_undo());
    assert!(p.can_redo());

    p.redo();
    let (tx2, ty2) = frame_translation(&p, "ua-body");
    assert!((tx2 - (tx0 + 5.0)).abs() < 1e-3, "redo re-applies");
    assert!((ty2 - (ty0 + 5.0)).abs() < 1e-3);
}

#[test]
fn transient_move_skips_undo_stack() {
    let mut p = open_seed_project();
    let cmd = Command::MoveFrame {
        frame: NodeId::Frame("ua-body".into()),
        dx_pt: 1.0,
        dy_pt: 1.0,
        transient: true,
    };
    p.apply(cmd).unwrap();
    assert_eq!(p.epoch(), 1);
    assert!(!p.can_undo(), "transient commands stay off undo");
}

#[test]
fn set_frame_bounds_overrides_geometry_and_undoes() {
    let mut p = open_seed_project();
    let (tx0, ty0) = frame_translation(&p, "ua-body");
    let bounds0 = p.document().text_frame("ua-body").expect("frame").bounds;

    p.apply(Command::SetFrameBounds {
        frame: NodeId::Frame("ua-body".into()),
        x_pt: 100.0,
        y_pt: 50.0,
        w_pt: 250.0,
        h_pt: 75.0,
        transient: false,
    })
    .unwrap();

    let f = p.document().text_frame("ua-body").expect("frame");
    assert_eq!(
        f.item_transform.expect("xform"),
        [1.0, 0.0, 0.0, 1.0, 100.0, 50.0]
    );
    assert!((f.bounds.right - f.bounds.left - 250.0).abs() < 1e-3);
    assert!((f.bounds.bottom - f.bounds.top - 75.0).abs() < 1e-3);

    p.undo();
    let f2 = p.document().text_frame("ua-body").expect("frame");
    let it = f2.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert!((it[4] - tx0).abs() < 1e-3);
    assert!((it[5] - ty0).abs() < 1e-3);
    assert_eq!(f2.bounds.top, bounds0.top);
    assert_eq!(f2.bounds.left, bounds0.left);
    assert_eq!(f2.bounds.right, bounds0.right);
    assert_eq!(f2.bounds.bottom, bounds0.bottom);
}

#[test]
fn hit_test_finds_frame_at_its_centre() {
    let p = open_seed_project();
    // ua-body's bbox in spread coords: read it through the
    // hittest helper so we don't depend on knowing the seed's
    // exact numbers.
    let frame = p.document().text_frame("ua-body").expect("frame");
    let bbox = idml_edit::hittest::transformed_bbox(frame.bounds, frame.item_transform);
    let cx = bbox.x + bbox.w * 0.5;
    let cy = bbox.y + bbox.h * 0.5;
    let hit = hit_test_spread(p.document(), 0, cx, cy).expect("hit");
    assert_eq!(hit.frame, NodeId::Frame("ua-body".into()));
    assert_eq!(hit.spread_idx, 0);
}

#[test]
fn hit_test_returns_none_outside_any_frame() {
    let p = open_seed_project();
    let hit = hit_test_spread(p.document(), 0, -1000.0, -1000.0);
    assert!(hit.is_none());
}

// -------------------- text editing -----------------------------------

fn first_story_id(p: &Project) -> StoryId {
    StoryId(p.document().stories[0].self_id.clone())
}

fn paragraph_text(p: &Project, story: &StoryId, para: ParaId) -> String {
    let idx = p
        .document()
        .stories
        .iter()
        .position(|s| s.self_id == story.0)
        .unwrap();
    let parsed = &p.document().stories[idx].story.paragraphs[para.0 as usize];
    parsed.runs.iter().map(|r| r.text.as_str()).collect()
}

#[test]
fn insert_text_appends_and_renders_through_pipeline() {
    let mut p = open_seed_project();
    let story = first_story_id(&p);
    let para = ParaId(0);
    let before = paragraph_text(&p, &story, para);
    let original_len = before.len() as u32;

    p.apply(Command::InsertText {
        story: story.clone(),
        para,
        byte_offset: original_len,
        text: " edited".to_string(),
        coalesce: None,
    })
    .expect("apply InsertText");

    let after = paragraph_text(&p, &story, para);
    assert!(after.ends_with(" edited"), "got: {after:?}");
    assert!(p.can_undo());

    p.undo();
    assert_eq!(paragraph_text(&p, &story, para), before);
    p.redo();
    assert_eq!(paragraph_text(&p, &story, para), after);
}

#[test]
fn delete_range_round_trips_through_undo() {
    let mut p = open_seed_project();
    let story = first_story_id(&p);
    let para = ParaId(0);
    let before = paragraph_text(&p, &story, para);

    p.apply(Command::DeleteRange {
        story: story.clone(),
        para,
        byte_from: 0,
        byte_to: 1,
        coalesce: None,
    })
    .unwrap();

    assert_eq!(paragraph_text(&p, &story, para).len(), before.len() - 1);
    p.undo();
    assert_eq!(paragraph_text(&p, &story, para), before);
}

#[test]
fn typing_coalesces_into_single_undo() {
    let mut p = open_seed_project();
    let story = first_story_id(&p);
    let para = ParaId(0);
    let before = paragraph_text(&p, &story, para);
    let mut off = before.len() as u32;

    let key = Some(42);
    for ch in ['a', 'b', 'c'] {
        p.apply(Command::InsertText {
            story: story.clone(),
            para,
            byte_offset: off,
            text: ch.to_string(),
            coalesce: key,
        })
        .unwrap();
        off += 1;
    }

    let after = paragraph_text(&p, &story, para);
    assert!(after.ends_with("abc"), "got: {after:?}");

    // One undo should revert all three coalesced inserts.
    p.undo();
    assert_eq!(paragraph_text(&p, &story, para), before);
    assert!(!p.can_undo(), "coalesced into a single undo entry");
}

#[test]
fn set_run_attr_changes_font_and_inverts() {
    let mut p = open_seed_project();
    let story = first_story_id(&p);
    let para = ParaId(0);
    let before = paragraph_text(&p, &story, para);

    p.apply(Command::SetRunAttr {
        story: story.clone(),
        para,
        byte_from: 0,
        byte_to: before.len() as u32,
        attr: RunAttrPatch::Font(Some("Helvetica".into())),
    })
    .unwrap();

    // Read the run that covers the start of the paragraph.
    let idx = p
        .document()
        .stories
        .iter()
        .position(|s| s.self_id == story.0)
        .unwrap();
    let runs = &p.document().stories[idx].story.paragraphs[para.0 as usize].runs;
    assert_eq!(runs[0].font.as_deref(), Some("Helvetica"));

    // Text content unchanged.
    assert_eq!(paragraph_text(&p, &story, para), before);

    let prev_font = {
        let original_runs = &p.original().stories[idx].story.paragraphs[para.0 as usize].runs;
        original_runs[0].font.clone()
    };
    p.undo();
    let runs2 = &p.document().stories[idx].story.paragraphs[para.0 as usize].runs;
    assert_eq!(runs2[0].font, prev_font);
}

#[test]
fn set_paragraph_attr_changes_justification_and_inverts() {
    let mut p = open_seed_project();
    let story = first_story_id(&p);
    let para = ParaId(0);
    let idx = p
        .document()
        .stories
        .iter()
        .position(|s| s.self_id == story.0)
        .unwrap();
    let prev = p.document().stories[idx].story.paragraphs[para.0 as usize].justification;

    p.apply(Command::SetParagraphAttr {
        story: story.clone(),
        para,
        attr: ParagraphAttrPatch::Justification(Some(Justification::CenterAlign)),
    })
    .unwrap();
    let now = p.document().stories[idx].story.paragraphs[para.0 as usize].justification;
    assert_eq!(now, Some(Justification::CenterAlign));
    p.undo();
    assert_eq!(
        p.document().stories[idx].story.paragraphs[para.0 as usize].justification,
        prev,
    );
}

#[test]
fn split_then_merge_paragraph_round_trips() {
    let mut p = open_seed_project();
    let story = first_story_id(&p);
    let para = ParaId(0);
    let original_text = paragraph_text(&p, &story, para);
    let original_count = p
        .document()
        .stories
        .iter()
        .find(|s| s.self_id == story.0)
        .map(|s| s.story.paragraphs.len())
        .unwrap();

    let split_at = (original_text.len() / 2) as u32;
    p.apply(Command::SplitParagraph {
        story: story.clone(),
        para,
        byte_offset: split_at,
    })
    .unwrap();

    let after_count = p
        .document()
        .stories
        .iter()
        .find(|s| s.self_id == story.0)
        .map(|s| s.story.paragraphs.len())
        .unwrap();
    assert_eq!(after_count, original_count + 1);

    p.undo();
    let restored_count = p
        .document()
        .stories
        .iter()
        .find(|s| s.self_id == story.0)
        .map(|s| s.story.paragraphs.len())
        .unwrap();
    assert_eq!(restored_count, original_count);
    assert_eq!(paragraph_text(&p, &story, para), original_text);
}

// -------------------- threading -------------------------------------

#[test]
fn link_frames_sets_next_text_frame_and_inverts() {
    let mut p = open_seed_project();
    // The hello seed has two text frames: ua-body and ub-body. They
    // are independent stories at open time.
    let prev = p
        .document()
        .text_frame("ua-body")
        .and_then(|f| f.next_text_frame.clone());

    p.apply(Command::LinkFrames {
        from: NodeId::Frame("ua-body".into()),
        to: NodeId::Frame("ub-body".into()),
    })
    .expect("apply LinkFrames");

    assert_eq!(
        p.document()
            .text_frame("ua-body")
            .and_then(|f| f.next_text_frame.clone()),
        Some("ub-body".into()),
    );

    p.undo();
    assert_eq!(
        p.document()
            .text_frame("ua-body")
            .and_then(|f| f.next_text_frame.clone()),
        prev,
    );
}

#[test]
fn unlink_frames_clears_next_text_frame_and_inverts() {
    let mut p = open_seed_project();
    p.apply(Command::LinkFrames {
        from: NodeId::Frame("ua-body".into()),
        to: NodeId::Frame("ub-body".into()),
    })
    .unwrap();
    p.apply(Command::UnlinkFrames {
        from: NodeId::Frame("ua-body".into()),
    })
    .unwrap();
    assert!(p
        .document()
        .text_frame("ua-body")
        .unwrap()
        .next_text_frame
        .is_none());
    p.undo();
    assert_eq!(
        p.document()
            .text_frame("ua-body")
            .unwrap()
            .next_text_frame
            .as_deref(),
        Some("ub-body"),
    );
}

// -------------------- persistence -----------------------------------

#[test]
fn save_then_load_replays_command_log_to_same_state() {
    let mut p1 = open_seed_project();

    // Apply two committed edits.
    p1.apply(Command::MoveFrame {
        frame: NodeId::Frame("ua-body".into()),
        dx_pt: 12.0,
        dy_pt: -7.5,
        transient: false,
    })
    .unwrap();
    let story = first_story_id(&p1);
    p1.apply(Command::InsertText {
        story: story.clone(),
        para: ParaId(0),
        byte_offset: 0,
        text: "PREFIX ".into(),
        coalesce: None,
    })
    .unwrap();

    let saved = p1.serialize_native().expect("serialize_native");
    let p2 = Project::deserialize_native(&saved).expect("deserialize_native");

    // Same translation on the moved frame.
    let f1 = p1.document().text_frame("ua-body").unwrap();
    let f2 = p2.document().text_frame("ua-body").unwrap();
    assert_eq!(f1.item_transform, f2.item_transform);

    // Same paragraph text.
    let t1 = paragraph_text(&p1, &story, ParaId(0));
    let t2 = paragraph_text(&p2, &story, ParaId(0));
    assert_eq!(t1, t2);
}

// -------------------- create-frame -----------------------------------

#[test]
fn create_rectangle_appends_and_inverts() {
    let mut p = open_seed_project();
    let n0 = p.document().spreads[0].spread.rectangles.len();

    p.apply(Command::CreateRectangle {
        spread_idx: 0,
        self_id: None,
        bounds: RectanglePayloadBounds {
            top: 0.0,
            left: 0.0,
            bottom: 100.0,
            right: 200.0,
        },
        item_transform: Some([1.0, 0.0, 0.0, 1.0, 50.0, 60.0]),
        fill_color: Some("Color/Black".into()),
        stroke_color: None,
        stroke_weight: None,
        applied_object_style: None,
        image_link: None,
    })
    .expect("apply CreateRectangle");

    let n1 = p.document().spreads[0].spread.rectangles.len();
    assert_eq!(n1, n0 + 1);
    let new_rect = p.document().spreads[0].spread.rectangles.last().unwrap();
    assert!(new_rect
        .self_id
        .as_deref()
        .unwrap()
        .starts_with("idml-edit-"));
    assert_eq!(new_rect.fill_color.as_deref(), Some("Color/Black"));

    p.undo();
    assert_eq!(p.document().spreads[0].spread.rectangles.len(), n0);

    p.redo();
    assert_eq!(p.document().spreads[0].spread.rectangles.len(), n0 + 1);
}

// -------------------- frame regression -------------------------------

#[test]
fn move_unknown_frame_returns_node_not_found() {
    let mut p = open_seed_project();
    let res = p.apply(Command::MoveFrame {
        frame: NodeId::Frame("does-not-exist".into()),
        dx_pt: 1.0,
        dy_pt: 1.0,
        transient: false,
    });
    assert!(res.is_err());
}
