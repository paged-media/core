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

//! Element geometry, path anchors and the hit test answer in PAGE space.
//!
//! InDesign centres a spread's coordinate system on the spine: a verso
//! page's origin is `(-w, -h/2)` inside its spread, a recto's `(0, -h/2)`.
//! An item's `bounds` composed through its `ItemTransform` lands in
//! SPREAD space, while everything the host draws or types is page-local
//! (the hit test's `frameBounds`, carets, selection rects, table cells,
//! the inserts' coordinates). `element_geometry` and `path_anchors`
//! handed the raw transform out, so on a verso the selection chrome
//! landed beside the object and the control bar read L −476 / R −232 on
//! a 540 pt page. Both now fold the owning page's origin into the
//! transform; `moveFrame`, which writes that transform back, takes it in
//! the same space, so a read-modify-write moves the item by exactly the
//! step.

use std::io::Write;

use paged_canvas::channel::Mutation;
use paged_canvas::element_selection::ElementId;
use paged_canvas::{CanvasModel, CanvasOptions, PageId};

/// One facing spread as InDesign writes it: verso `p1` and recto `p2`,
/// 612 × 792, origins `(-612, -396)` and `(0, -396)`.
///
/// - `r1`: rectangle on the verso, inner box `(60,100)-(160,200)` placed
///   by its transform → page-local `(60,100)-(160,200)`.
/// - `r2`: rectangle on the verso with no transform, its bounds in
///   spread coordinates → page-local `(300,400)-(400,450)`.
/// - `g1`: triangle on the recto, anchors `(0,0) (50,0) (50,50)` placed
///   at spread `(100, -296)` → page-local `(100,100)`.
/// - `pb`: rectangle on the pasteboard, left of the verso.
fn facing_idml() -> Vec<u8> {
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
<Layer Self="layer-body" Name="Body" Visible="true" Locked="false"/>
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="2" BindingLocation="1">
<Page Self="p1" Name="2" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 -612 -396"/>
<Page Self="p2" Name="3" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 -396"/>
<Rectangle Self="r1" GeometricBounds="0 0 100 100" ItemTransform="1 0 0 1 -552 -296" FillColor="Color/Black"/>
<Rectangle Self="r2" GeometricBounds="4 -312 54 -212" FillColor="Color/Black"/>
<Polygon Self="g1" ItemTransform="1 0 0 1 100 -296" FillColor="Color/Black">
<Properties><PathGeometry><GeometryPathType PathOpen="false"><PathPointArray>
<PathPointType Anchor="0 0" LeftDirection="0 0" RightDirection="0 0"/>
<PathPointType Anchor="50 0" LeftDirection="50 0" RightDirection="50 0"/>
<PathPointType Anchor="50 50" LeftDirection="50 50" RightDirection="50 50"/>
</PathPointArray></GeometryPathType></PathGeometry></Properties>
</Polygon>
<Rectangle Self="pb" GeometricBounds="0 0 40 40" ItemTransform="1 0 0 1 -800 -100" FillColor="Color/Black"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn load() -> CanvasModel {
    CanvasModel::load("facing", &facing_idml(), CanvasOptions::default()).expect("load")
}

/// `[left, top, right, bottom]` of `bounds` (`[t, l, b, r]`) through `t`.
fn composed(bounds: [f32; 4], t: Option<[f32; 6]>) -> [f32; 4] {
    let [a, b, c, d, e, f] = t.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let [top, left, bottom, right] = bounds;
    let pts = [(left, top), (right, top), (left, bottom), (right, bottom)];
    let xs = pts.map(|(x, y)| a * x + c * y + e);
    let ys = pts.map(|(x, y)| b * x + d * y + f);
    let min = |v: [f32; 4]| v.iter().copied().fold(f32::INFINITY, f32::min);
    let max = |v: [f32; 4]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    [min(xs), min(ys), max(xs), max(ys)]
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
}

fn geometry(m: &CanvasModel, id: ElementId) -> paged_canvas::channel::ElementGeometryItem {
    m.element_geometry(std::slice::from_ref(&id))
        .pop()
        .unwrap_or_else(|| panic!("{id:?} answers geometry"))
}

fn rect(id: &str) -> ElementId {
    ElementId::Rectangle(id.to_string())
}

#[test]
fn the_pages_sit_where_indesign_puts_them() {
    let m = load();
    let origins: Vec<_> = m
        .built()
        .pages
        .iter()
        .map(|p| (p.id.as_str().to_string(), p.spread_origin))
        .collect();
    assert_eq!(
        origins,
        [
            ("p1".to_string(), (-612.0, -396.0)),
            ("p2".to_string(), (0.0, -396.0))
        ]
    );
    // The facing spread reads as its pages' folios.
    assert_eq!(m.spreads()[0].label, "2–3");
}

#[test]
fn element_geometry_composes_to_the_page_the_hit_test_answers_in() {
    let m = load();
    for (id, page, want) in [
        ("r1", "p1", [60.0, 100.0, 160.0, 200.0]),
        ("r2", "p1", [300.0, 400.0, 400.0, 450.0]),
    ] {
        let g = geometry(&m, rect(id));
        assert_eq!(g.page_id.as_ref().map(|p| p.as_str()), Some(page), "{id}");
        let got = composed(g.bounds, g.item_transform);
        assert!(
            close(got, want),
            "{id}: geometry composes to {got:?}, the page has it at {want:?}"
        );
        // The stored transform is the page-space one with the origin back.
        assert_eq!(g.page_origin, Some([-612.0, -396.0]), "{id}");
        let stored = m.scene().spreads[0]
            .spread
            .rectangles
            .iter()
            .find(|r| r.self_id.as_deref() == Some(id))
            .unwrap()
            .item_transform
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let [a, b, c, d, e, f] = g.item_transform.unwrap();
        assert_eq!(stored, [a, b, c, d, e - 612.0, f - 396.0], "{id}");

        // The hit test agrees: its page-local box, and its own bounds +
        // transform, are the same box.
        let centre = ((want[0] + want[2]) / 2.0, (want[1] + want[3]) / 2.0);
        let hit = m.hit_test(&PageId(page.to_string()), centre);
        assert_eq!(hit.element, Some(rect(id)), "{id}: the hit finds it");
        let fb = hit.frame_bounds.expect("frame bounds");
        assert!(close(fb, want), "{id}: frameBounds {fb:?}");
        let hb = composed(hit.bounds.expect("bounds"), hit.item_transform);
        assert!(
            close(hb, want),
            "{id}: hit bounds through its transform {hb:?}"
        );
    }
}

#[test]
fn path_anchors_compose_to_the_page() {
    let m = load();
    let r = m
        .path_anchors(&ElementId::Polygon("g1".to_string()))
        .expect("anchors");
    assert_eq!(r.page_id.as_ref().map(|p| p.as_str()), Some("p2"));
    let [a, b, c, d, e, f] = r.item_transform.expect("a placed path");
    let on_page: Vec<[f32; 2]> = r
        .anchors
        .iter()
        .map(|p| {
            let [x, y] = p.anchor;
            [a * x + c * y + e, b * x + d * y + f]
        })
        .collect();
    assert_eq!(on_page, [[100.0, 100.0], [150.0, 100.0], [150.0, 150.0]]);
}

#[test]
fn a_pasteboard_item_stays_in_spread_space() {
    let m = load();
    let g = geometry(&m, rect("pb"));
    assert_eq!(g.page_id, None);
    assert!(close(
        composed(g.bounds, g.item_transform),
        [-800.0, -100.0, -760.0, -60.0]
    ));
}

#[test]
fn move_frame_takes_the_transform_geometry_reads() {
    let mut m = load();
    for id in ["r1", "r2"] {
        let before = geometry(&m, rect(id));
        let [a, b, c, d, e, f] = before
            .item_transform
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        // The nudge: read, translate by a step, write back.
        m.apply_mutation(&Mutation::MoveFrame {
            frame_id: id.to_string(),
            transform: [a, b, c, d, e + 10.0, f + 5.0],
        })
        .expect("move");
        let after = geometry(&m, rect(id));
        assert_eq!(after.page_id, before.page_id, "{id}: still on its page");
        let was = composed(before.bounds, before.item_transform);
        let now = composed(after.bounds, after.item_transform);
        let want = [was[0] + 10.0, was[1] + 5.0, was[2] + 10.0, was[3] + 5.0];
        assert!(
            close(now, want),
            "{id}: moved to {now:?}, the step puts it at {want:?}"
        );
        // What the document stores is still the spread placement.
        let page = before.page_id.clone().unwrap();
        let hit = m.hit_test(&page, ((now[0] + now[2]) / 2.0, (now[1] + now[3]) / 2.0));
        assert!(
            close(hit.frame_bounds.expect("hit"), want),
            "{id}: the hit agrees"
        );
        m.undo().expect("undo");
        let back = geometry(&m, rect(id));
        assert!(
            close(composed(back.bounds, back.item_transform), was),
            "{id}: undo"
        );
    }
}
