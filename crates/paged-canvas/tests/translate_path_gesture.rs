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

//! C-78 — the translate gesture moves what is DRAWN.
//!
//! An un-rotated item used to translate through its `bounds`
//! (`SetProperty { FrameBounds }`) whatever its kind. For a rectangle,
//! an ellipse and an ordinary text frame the box IS the geometry. A
//! line and a pen path are drawn from their ANCHORS, which that write
//! does not touch: the drag committed, the model's box moved, the path
//! stayed, and zero pixels changed (editor engine-findings §14). The
//! same was true of the two other shapes the renderer draws from a
//! path — a `<Rectangle>` with more than four anchors and a text frame
//! whose path is not a plain box.
//!
//! The rule now: an item the renderer draws from its path translates
//! through its `ItemTransform`; an item it draws from its box keeps
//! translating through the box. What is measured here is where the
//! item's fill and stroke land on the page — the display list's own
//! geometry, in page space — before and after.

use std::io::Write;

use paged_canvas::{
    channel::Mutation, CanvasModel, CanvasOptions, ElementId, GestureModifiers, GestureType, PageId,
};
use paged_compose::{DisplayCommand, PathSegment};
use paged_mutate::operation::PathAnchorSpec;
use paged_mutate::{PropertyPath, Value};

/// One page and, on it: a stroked `<Rectangle>` whose path has FIVE
/// anchors (drawn as a polygon, Q-11) and a filled `<TextFrame>` whose
/// path is a triangle. Lines and pen paths are minted through the wire.
fn idml() -> Vec<u8> {
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
<idPkg:Graphic src="Resources/Graphic.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Resources/Graphic.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Color Self="Color/Black" Model="Process" Space="CMYK" ColorValue="0 0 0 100" Name="Black"/>
</idPkg:Graphic>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Rectangle Self="penta" ItemTransform="1 0 0 1 0 0" StrokeColor="Color/Black" StrokeWeight="2">
<Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
<PathPointType Anchor="100 400" LeftDirection="100 400" RightDirection="100 400"/>
<PathPointType Anchor="160 380" LeftDirection="160 380" RightDirection="160 380"/>
<PathPointType Anchor="220 400" LeftDirection="220 400" RightDirection="220 400"/>
<PathPointType Anchor="220 460" LeftDirection="220 460" RightDirection="220 460"/>
<PathPointType Anchor="100 460" LeftDirection="100 460" RightDirection="100 460"/>
</PathPointArray></GeometryPathType></PathGeometry></Properties>
</Rectangle>
<TextFrame Self="tri" ParentStory="story1" ItemTransform="1 0 0 1 0 0" FillColor="Color/Black">
<Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
<PathPointType Anchor="300 400" LeftDirection="300 400" RightDirection="300 400"/>
<PathPointType Anchor="420 400" LeftDirection="420 400" RightDirection="420 400"/>
<PathPointType Anchor="360 500" LeftDirection="360 500" RightDirection="360 500"/>
</PathPointArray></GeometryPathType></PathGeometry></Properties>
</TextFrame>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_story1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1"><ParagraphStyleRange><CharacterStyleRange><Content></Content></CharacterStyleRange></ParagraphStyleRange></Story>
</idPkg:Story>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    CanvasModel::load("c78", &idml(), CanvasOptions::default()).expect("load")
}

fn page(m: &CanvasModel) -> PageId {
    PageId(m.pages()[0].self_id.clone())
}

fn corner(x: f32, y: f32) -> PathAnchorSpec {
    PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    }
}

fn mint(m: &mut CanvasModel, mutation: Mutation) -> ElementId {
    m.apply_mutation(&mutation)
        .expect("insert")
        .created_id
        .expect("created id")
}

fn line(m: &mut CanvasModel) -> ElementId {
    let page_id = page(m);
    mint(
        m,
        Mutation::InsertLine {
            page_id,
            start: (50.0, 50.0),
            end: (150.0, 90.0),
        },
    )
}

/// An open three-point pen path with a visible stroke.
fn pen_path(m: &mut CanvasModel) -> ElementId {
    let page_id = page(m);
    let id = mint(
        m,
        Mutation::InsertPath {
            page_id,
            anchors: vec![
                corner(200.0, 50.0),
                corner(260.0, 110.0),
                corner(320.0, 50.0),
            ],
            open: true,
            smooth: false,
        },
    );
    for (path, value) in [
        (
            PropertyPath::FrameStrokeColor,
            Value::ColorRef(Some("Color/Black".to_string())),
        ),
        (PropertyPath::FrameStrokeWeight, Value::Length(Some(4.0))),
    ] {
        m.apply_mutation(&Mutation::SetElementProperty {
            element_id: id.clone(),
            path,
            value,
        })
        .expect("stroke");
    }
    id
}

/// The page-space box of everything the page's fills and strokes draw
/// whose geometry lies inside `window` (`[x0, y0, x1, y1]`) — the item
/// under test, picked out by where it is.
fn drawn(doc: &paged_renderer::BuiltDocument, window: [f32; 4]) -> Option<[f32; 4]> {
    let page = &doc.pages[0];
    let mut out: Option<[f32; 4]> = None;
    for c in &page.list.commands {
        let (path_id, t) = match c {
            DisplayCommand::FillPath {
                path_id, transform, ..
            }
            | DisplayCommand::StrokePath {
                path_id, transform, ..
            } => (*path_id, transform.0),
            _ => continue,
        };
        let Some(path) = page.list.paths.get(path_id) else {
            continue;
        };
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let mut grow = |x: f32, y: f32| {
            let (px, py) = (t[0] * x + t[2] * y + t[4], t[1] * x + t[3] * y + t[5]);
            b = [b[0].min(px), b[1].min(py), b[2].max(px), b[3].max(py)];
        };
        for seg in &path.segments {
            match *seg {
                PathSegment::MoveTo { x, y }
                | PathSegment::LineTo { x, y }
                | PathSegment::QuadTo { x, y, .. }
                | PathSegment::CubicTo { x, y, .. } => grow(x, y),
                PathSegment::Close => {}
            }
        }
        let inside =
            b[0] >= window[0] && b[1] >= window[1] && b[2] <= window[2] && b[3] <= window[3];
        if inside {
            out = Some(match out {
                None => b,
                Some(o) => [
                    o[0].min(b[0]),
                    o[1].min(b[1]),
                    o[2].max(b[2]),
                    o[3].max(b[3]),
                ],
            });
        }
    }
    out
}

fn shifted(b: [f32; 4], d: (f32, f32)) -> [f32; 4] {
    [b[0] + d.0, b[1] + d.1, b[2] + d.0, b[3] + d.1]
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}

/// Drag `id` by `delta` and check, at every stage, where it is drawn.
/// `window` is a box around the item's home that also contains where
/// the drag takes it and nothing else on the page.
fn drag_moves_what_is_drawn(m: &mut CanvasModel, what: &str, id: ElementId, window: [f32; 4]) {
    const DELTA: (f32, f32) = (40.0, 20.0);
    let home = drawn(m.built(), window).unwrap_or_else(|| panic!("{what} is drawn"));
    let digest_home = m.built().pages[0].list.digest();

    let handle = m
        .begin_gesture(vec![id.clone()], GestureType::Translate, None)
        .expect("begin");
    m.update_gesture(handle, DELTA, GestureModifiers::default())
        .expect("update");
    // The PREVIEW moves it — a drag that only lands on release is not a
    // drag.
    let preview = drawn(m.built(), window).unwrap_or_else(|| panic!("{what} is drawn mid-drag"));
    assert!(
        close(preview, shifted(home, DELTA)),
        "{what}: the preview draws it at {preview:?}; home {home:?} + {DELTA:?} is {:?}",
        shifted(home, DELTA)
    );
    m.commit_gesture(handle).expect("commit");
    let landed = drawn(m.built(), window).unwrap_or_else(|| panic!("{what} is drawn after"));
    assert!(
        close(landed, shifted(home, DELTA)),
        "{what}: after the commit it is drawn at {landed:?}; home {home:?} + {DELTA:?} is {:?}",
        shifted(home, DELTA)
    );
    // …and a cold build of the committed model agrees with the canvas.
    let cold = m.build_for_export().expect("full build");
    assert!(
        close(drawn(&cold, window).expect("drawn cold"), landed),
        "{what}: the committed model paints where the canvas shows it"
    );

    assert!(m.undo().is_some(), "{what}: undo");
    assert_eq!(
        m.built().pages[0].list.digest(),
        digest_home,
        "{what}: undo draws the page exactly as it was"
    );
    assert!(m.redo().is_some(), "{what}: redo");
    assert!(close(
        drawn(m.built(), window).expect("drawn after redo"),
        shifted(home, DELTA)
    ));

    // A cancelled drag leaves it home.
    assert!(m.undo().is_some());
    let handle = m
        .begin_gesture(vec![id], GestureType::Translate, None)
        .expect("begin");
    m.update_gesture(handle, DELTA, GestureModifiers::default())
        .expect("update");
    m.cancel_gesture(handle).expect("cancel");
    assert_eq!(
        m.built().pages[0].list.digest(),
        digest_home,
        "{what}: a cancelled drag restores the page"
    );
}

#[test]
fn dragging_a_line_moves_the_line() {
    let mut m = model();
    let id = line(&mut m);
    drag_moves_what_is_drawn(&mut m, "line", id, [40.0, 40.0, 200.0, 120.0]);
}

#[test]
fn dragging_a_pen_path_moves_the_path() {
    let mut m = model();
    let id = pen_path(&mut m);
    drag_moves_what_is_drawn(&mut m, "pen path", id, [190.0, 40.0, 370.0, 140.0]);
}

/// An ellipse is drawn from its BOX, so its commit was always right —
/// but it had no preview arm at all (nor had a line, nor a polygon's
/// box or transform), so it did not follow the pointer and only jumped
/// on release. Same helper: the preview is one of its assertions.
#[test]
fn dragging_an_ellipse_previews_and_moves_it() {
    let mut m = model();
    let page_id = page(&m);
    let id = mint(
        &mut m,
        Mutation::InsertOval {
            page_id,
            bounds: (200.0, 450.0, 260.0, 540.0),
        },
    );
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path: PropertyPath::FrameFillColor,
        value: Value::ColorRef(Some("Color/Black".to_string())),
    })
    .expect("fill");
    drag_moves_what_is_drawn(&mut m, "ellipse", id, [440.0, 190.0, 600.0, 300.0]);
}

/// The same class, on the two other shapes the renderer draws from a
/// path rather than from the box.
#[test]
fn dragging_a_rectangle_with_a_five_point_path_moves_its_outline() {
    let mut m = model();
    drag_moves_what_is_drawn(
        &mut m,
        "five-point rectangle",
        ElementId::Rectangle("penta".into()),
        [90.0, 370.0, 270.0, 490.0],
    );
}

#[test]
fn dragging_a_text_frame_with_a_triangular_path_moves_its_fill() {
    let mut m = model();
    drag_moves_what_is_drawn(
        &mut m,
        "triangular text frame",
        ElementId::TextFrame("tri".into()),
        [290.0, 390.0, 470.0, 530.0],
    );
}

/// The decision per kind, pinned from the other side: an item drawn
/// from its BOX still translates through the box (its transform is not
/// touched), an item drawn from its path translates through its
/// transform (its anchors and box are not touched, so they still agree).
#[test]
fn the_commit_writes_the_box_for_a_box_and_the_transform_for_a_path() {
    let mut m = model();
    let page_id = page(&m);
    let rect = mint(
        &mut m,
        Mutation::InsertFrame {
            page_id,
            bounds: (600.0, 100.0, 660.0, 200.0),
        },
    );
    let path = pen_path(&mut m);
    let read = |m: &CanvasModel, id: &ElementId, p: PropertyPath| {
        m.element_properties(id)
            .expect("props")
            .entries
            .into_iter()
            .find(|e| e.path == p)
            .and_then(|e| e.value)
    };
    let before_rect = (
        read(&m, &rect, PropertyPath::FrameBounds),
        read(&m, &rect, PropertyPath::FrameTransform),
    );
    let before_path = (
        read(&m, &path, PropertyPath::FrameBounds),
        read(&m, &path, PropertyPath::FrameTransform),
    );
    for id in [&rect, &path] {
        let h = m
            .begin_gesture(vec![id.clone()], GestureType::Translate, None)
            .expect("begin");
        m.update_gesture(h, (40.0, 20.0), GestureModifiers::default())
            .expect("update");
        m.commit_gesture(h).expect("commit");
    }
    // The rectangle: box moved (top/left/bottom/right by 20/40), same
    // transform.
    assert_eq!(
        read(&m, &rect, PropertyPath::FrameBounds),
        Some(Value::Bounds([620.0, 140.0, 680.0, 240.0]))
    );
    assert_eq!(read(&m, &rect, PropertyPath::FrameTransform), before_rect.1);
    // The path: same box, transform carries the move.
    assert_eq!(read(&m, &path, PropertyPath::FrameBounds), before_path.0);
    let Some(Value::Transform(Some(t))) = read(&m, &path, PropertyPath::FrameTransform) else {
        panic!("the path carries a transform after the drag");
    };
    let t0 = match before_path.1 {
        Some(Value::Transform(Some(t))) => (t[4], t[5]),
        _ => (0.0, 0.0),
    };
    assert_eq!((t[4] - t0.0, t[5] - t0.1), (40.0, 20.0));
}
