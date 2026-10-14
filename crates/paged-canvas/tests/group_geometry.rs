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

//! A selected group answers the box InDesign draws around it.
//!
//! `element_geometry` answered nothing for a group id, so a selected group
//! drew no selection box on any path. It now answers the union of the
//! members' outlines in the group's own space, placed by the group's
//! composed transform (page-local like every other item): an axis-aligned
//! group reads as the union of its members, a rotated group as a rotated
//! box around them, a nested group inside its parent's box.

use std::io::Write;

use paged_canvas::channel::{ElementGeometryItem, Mutation};
use paged_canvas::element_selection::ElementId;
use paged_canvas::{CanvasModel, CanvasOptions};

/// One facing spread (verso `p1` at spread `(-612, -396)`, recto `p2` at
/// `(0, -396)`), so page-local and spread space differ.
///
/// - `ga` (recto, no transform): `a1` inner `(0,0)-(100,50)` placed at
///   spread `(100, -296)`; `a2` `(0,0)-(40,40)` at `(250, -246)`; and a
///   100 × 50 oval rotated 45° about its centre at spread `(160, -146)`.
/// - `gr` (recto, rotated 30° about its origin, placed at spread
///   `(300, 0)`): members `r1` group-local `(0,0)-(80,20)` and `r2`
///   `(0,40)-(30,70)`. The members' stored transforms are composed
///   into spread space, as the parser stores them.
/// - `gn` (verso, translated `(-500, -300)`): holds `n1` group-local
///   `(0,0)-(20,20)` and the nested group `gi`, rotated 90° and placed at
///   group-local `(100, 0)`, which holds `i1` inner `(0,0)-(10,40)`.
fn idml() -> Vec<u8> {
    let (s, c) = 30f32.to_radians().sin_cos();
    let r = format!("{c} {s} {} {c}", -s);
    let k = std::f32::consts::FRAC_1_SQRT_2;
    // Oval inner box centred on its own origin, so the rotation turns it in
    // place: `rotate(45°)` then `translate(160, -146)`.
    let oval = format!("{k} {k} {} {k} 160 -146", -k);
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="2" BindingLocation="1">
<Page Self="p1" Name="2" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 -612 -396"/>
<Page Self="p2" Name="3" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 -396"/>
<Group Self="ga">
  <Rectangle Self="a1" GeometricBounds="0 0 50 100" ItemTransform="1 0 0 1 100 -296" FillColor="Color/Black"/>
  <Rectangle Self="a2" GeometricBounds="0 0 40 40" ItemTransform="1 0 0 1 250 -246" FillColor="Color/Black"/>
  <Oval Self="o1" GeometricBounds="-25 -50 25 50" ItemTransform="{oval}" FillColor="Color/Black"/>
</Group>
<Group Self="gr" ItemTransform="{r} 300 0">
  <Rectangle Self="r1" GeometricBounds="0 0 20 80" FillColor="Color/Black"/>
  <Rectangle Self="r2" GeometricBounds="40 0 70 30" FillColor="Color/Black"/>
</Group>
<Group Self="gn" ItemTransform="1 0 0 1 -500 -300">
  <Rectangle Self="n1" GeometricBounds="0 0 20 20" FillColor="Color/Black"/>
  <Group Self="gi" ItemTransform="0 1 -1 0 100 0">
    <Rectangle Self="i1" GeometricBounds="0 0 40 10" FillColor="Color/Black"/>
  </Group>
</Group>
</Spread></idPkg:Spread>"#,
        r = r,
        oval = oval
    );
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
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
        zip.write_all(spread.as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn load() -> CanvasModel {
    CanvasModel::load("groups", &idml(), CanvasOptions::default()).expect("load")
}

fn geometry(m: &CanvasModel, id: ElementId) -> ElementGeometryItem {
    m.element_geometry(std::slice::from_ref(&id))
        .pop()
        .unwrap_or_else(|| panic!("{id:?} answers geometry"))
}

fn group(id: &str) -> ElementId {
    ElementId::Group(id.to_string())
}

fn apply(t: Option<[f32; 6]>, (x, y): (f32, f32)) -> (f32, f32) {
    let [a, b, c, d, e, f] = t.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    (a * x + c * y + e, b * x + d * y + f)
}

/// The four corners of a geometry answer on its page, clockwise from the
/// box's own top-left.
fn corners(g: &ElementGeometryItem) -> [(f32, f32); 4] {
    let [t, l, b, r] = g.bounds;
    [(l, t), (r, t), (r, b), (l, b)].map(|p| apply(g.item_transform, p))
}

fn near(a: f32, b: f32, what: &str) {
    assert!((a - b).abs() < 1e-2, "{what}: {a} vs {b}");
}

fn near4(a: [f32; 4], b: [f32; 4], what: &str) {
    for (x, y) in a.iter().zip(b) {
        near(*x, y, what);
    }
}

#[test]
fn an_axis_aligned_group_reads_as_the_union_of_its_members() {
    let m = load();
    let g = geometry(&m, group("ga"));
    assert_eq!(g.page_id.as_ref().map(|p| p.as_str()), Some("p2"));
    assert_eq!(g.page_origin, Some([0.0, -396.0]));
    // a1 spans spread x 100..200, y -296..-246; a2 250..290, -246..-206;
    // the oval's true extent is ±39.53 about (160, -146), so it reaches
    // y -106.47 at the bottom. The rotated bounding rectangle of the oval
    // would claim ±53.03 instead.
    let half = ((50.0f32 * 50.0 + 25.0 * 25.0) / 2.0).sqrt();
    let spread = [-296.0, 100.0, -146.0 + half, 290.0];
    // Bounds are in the group's own space (here: spread space), the
    // transform carries the page origin like every page item's.
    near4(g.bounds, spread, "ga bounds");
    let [tl, _, br, _] = corners(&g);
    near4(
        [tl.1, tl.0, br.1, br.0],
        [100.0, 100.0, 250.0 + half, 290.0],
        "ga on its page",
    );
    assert!(!g.has_image);
    assert_eq!(g.story_id, None);
}

#[test]
fn a_rotated_group_reads_as_an_oriented_box_around_its_members() {
    let m = load();
    let g = geometry(&m, group("gr"));
    assert_eq!(g.page_id.as_ref().map(|p| p.as_str()), Some("p2"));
    // In the group's own space the members span (0,0)-(80,70).
    near4(g.bounds, [0.0, 0.0, 70.0, 80.0], "gr bounds in group space");
    // The transform is the group's: rotated 30°, page origin folded in.
    let t = g.item_transform.expect("a rotated group has a transform");
    let (s, c) = 30f32.to_radians().sin_cos();
    near4([t[0], t[1], t[2], t[3]], [c, s, -s, c], "gr rotation");
    near(t[4], 300.0, "gr tx");
    near(t[5], 396.0, "gr ty (page-local)");
    // Every member corner lies inside the oriented box and the box is
    // tight: each of its edges touches a member.
    let inv = |(x, y): (f32, f32)| {
        let (dx, dy) = (x - t[4], y - t[5]);
        (c * dx + s * dy, -s * dx + c * dy)
    };
    for (id, rect) in [
        ("r1", [0.0, 0.0, 20.0, 80.0]),
        ("r2", [40.0, 0.0, 70.0, 30.0]),
    ] {
        let leaf = geometry(&m, ElementId::Rectangle(id.to_string()));
        assert_eq!(leaf.bounds, rect);
        for p in corners(&leaf) {
            let (u, v) = inv(p);
            assert!(
                (-1e-2..=80.0 + 1e-2).contains(&u) && (-1e-2..=70.0 + 1e-2).contains(&v),
                "{id} corner {p:?} → group-local ({u}, {v}) escapes the box"
            );
        }
    }
}

#[test]
fn a_nested_group_reads_in_its_own_frame_and_inside_its_parent() {
    let m = load();
    let outer = geometry(&m, group("gn"));
    let inner = geometry(&m, group("gi"));
    assert_eq!(outer.page_id.as_ref().map(|p| p.as_str()), Some("p1"));
    // gi rotates i1 (10 wide, 40 tall) by 90° about gi's origin, placed at
    // gn-local (100, 0): it occupies gn-local x 60..100, y 0..10.
    near4(
        inner.bounds,
        [0.0, 0.0, 40.0, 10.0],
        "gi bounds in its own space",
    );
    let ti = inner.item_transform.expect("gi is rotated");
    near4(
        [ti[0], ti[1], ti[2], ti[3]],
        [0.0, 1.0, -1.0, 0.0],
        "gi orientation",
    );
    // gn's box holds n1 (0..20, 0..20) and gi's footprint (60..100, 0..10).
    near4(
        outer.bounds,
        [0.0, 0.0, 20.0, 100.0],
        "gn bounds in its space",
    );
    let to = outer.item_transform.expect("gn is translated");
    // Spread (-500, -300) on the verso whose origin is (-612, -396).
    near4(
        [to[0], to[1], to[2], to[3]],
        [1.0, 0.0, 0.0, 1.0],
        "gn orientation",
    );
    near(to[4], 112.0, "gn tx (page-local)");
    near(to[5], 96.0, "gn ty (page-local)");
    // The inner box's corners land inside the outer box on the page.
    for (x, y) in corners(&inner) {
        let (u, v) = (x - to[4], y - to[5]);
        assert!(
            (-1e-2..=100.0 + 1e-2).contains(&u) && (-1e-2..=20.0 + 1e-2).contains(&v),
            "gi corner ({x}, {y}) escapes gn"
        );
    }
}

/// `moveFrame` and `resizeFrame` name page items, not groups: both are
/// refused for a group id and change nothing. A group moves through
/// `setGroupTransform`, and its box follows.
#[test]
fn a_group_moves_through_its_own_transform_and_its_box_follows() {
    let mut m = load();
    let before = geometry(&m, group("ga"));
    let refused = m.apply_mutation(&Mutation::MoveFrame {
        frame_id: "ga".to_string(),
        transform: [1.0, 0.0, 0.0, 1.0, 10.0, 10.0],
    });
    assert!(refused.is_err(), "moveFrame on a group: {refused:?}");
    let refused = m.apply_mutation(&Mutation::ResizeFrame {
        frame_id: "ga".to_string(),
        bounds: (0.0, 0.0, 10.0, 10.0),
    });
    assert!(refused.is_err(), "resizeFrame on a group: {refused:?}");
    let unchanged = geometry(&m, group("ga"));
    assert_eq!(unchanged.bounds, before.bounds);
    assert_eq!(unchanged.item_transform, before.item_transform);

    m.apply_mutation(&Mutation::SetGroupTransform {
        group_id: "ga".to_string(),
        transform: Some([1.0, 0.0, 0.0, 1.0, 12.0, -7.0]),
    })
    .expect("setGroupTransform");
    let after = geometry(&m, group("ga"));
    let shift = |g: &ElementGeometryItem| apply(g.item_transform, (g.bounds[1], g.bounds[0]));
    let (b, a) = (shift(&before), shift(&after));
    near(a.0 - b.0, 12.0, "box dx");
    near(a.1 - b.1, -7.0, "box dy");
    // Same size: the box moved, it did not grow.
    near(
        after.bounds[3] - after.bounds[1],
        before.bounds[3] - before.bounds[1],
        "box width",
    );
    near(
        after.bounds[2] - after.bounds[0],
        before.bounds[2] - before.bounds[0],
        "box height",
    );
}
