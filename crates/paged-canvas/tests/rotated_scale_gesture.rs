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

//! A handle scale on rotated items, driven the way the canvas drives it:
//! the handles are read off `element_geometry` (the box the selection
//! chrome draws), the gesture begins with the grabbed handle as its
//! anchor and the opposite handle as its pivot, then update + commit.
//!
//! InDesign's rule, read from InDesign itself
//! (`tools/indesign-export/scale-frame-probe.jsx`, answers in
//! `scale-frame-probe.json`):
//!
//! - ONE selected item — a frame or a group — has its own box, turned
//!   with it, and a handle scales it along its own axes: it stays a
//!   rectangle at its angle, a side handle changes only the dimension
//!   along that axis (probe cases 1b, 4b, 4c).
//! - SEVERAL selected items have a box aligned to the pasteboard, even
//!   when they share one rotation, and scale as one unit along the page
//!   axes, so a non-uniform scale shears the rotated ones (1c, 1d, 3a, 3b).

use std::io::Write;

use paged_canvas::element_selection::ElementId;
use paged_canvas::gesture::{GestureAnchor, GestureType};
use paged_canvas::{CanvasModel, CanvasOptions, GestureModifiers, PageId};

type M = [f32; 6];
type Pt = (f32, f32);

/// InDesign's rotation sense: `deg` counterclockwise on the page (y down).
fn rot(deg: f32, tx: f32, ty: f32) -> M {
    let (s, c) = deg.to_radians().sin_cos();
    [c, -s, s, c, tx, ty]
}

fn attr(m: M) -> String {
    m.iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `r30` alone: 160 × 60 at 30°. `g30`: a group turned 30° holding `ga`
/// (160 × 60) and `gb` (80 × 40), both unrotated inside it. `m30` /
/// `n30` / `m60`: loose rectangles for the multi-selections, 160 × 60 at
/// 30°, 160 × 60 at 30°, 80 × 40 at 60° — the probe's shapes.
fn model() -> CanvasModel {
    let body = format!(
        r#"<Rectangle Self="r30" GeometricBounds="0 0 60 160" ItemTransform="{r30}" FillColor="Color/Black"/>
<Group Self="g30" ItemTransform="{g30}">
  <Rectangle Self="ga" GeometricBounds="0 0 60 160" ItemTransform="1 0 0 1 0 0" FillColor="Color/Black"/>
  <Rectangle Self="gb" GeometricBounds="100 0 140 80" ItemTransform="1 0 0 1 0 0" FillColor="Color/Black"/>
</Group>
<Rectangle Self="m30" GeometricBounds="0 0 60 160" ItemTransform="{m30}" FillColor="Color/Black"/>
<Rectangle Self="n30" GeometricBounds="0 0 60 160" ItemTransform="{n30}" FillColor="Color/Black"/>
<Rectangle Self="m60" GeometricBounds="0 0 40 80" ItemTransform="{m60}" FillColor="Color/Black"/>"#,
        r30 = attr(rot(30.0, 200.0, 300.0)),
        g30 = attr(rot(30.0, 150.0, 600.0)),
        m30 = attr(rot(30.0, 600.0, 300.0)),
        n30 = attr(rot(30.0, 650.0, 500.0)),
        m60 = attr(rot(60.0, 700.0, 700.0)),
    );
    let spread = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 1200 1200" ItemTransform="1 0 0 1 0 0"/>
{body}
</Spread></idPkg:Spread>"#
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
    CanvasModel::load("rotated-scale", &buf, CanvasOptions::default()).expect("load")
}

fn rect(id: &str) -> ElementId {
    ElementId::Rectangle(id.to_string())
}

fn group(id: &str) -> ElementId {
    ElementId::Group(id.to_string())
}

fn map(m: M, p: Pt) -> Pt {
    (
        m[0] * p.0 + m[2] * p.1 + m[4],
        m[1] * p.0 + m[3] * p.1 + m[5],
    )
}

/// An item's box as the selection chrome draws it: its bounds through its
/// transform, page space (the page sits at the spread origin here).
#[derive(Clone, Copy, Debug)]
struct HandleBox {
    bounds: [f32; 4],
    m: M,
}

impl HandleBox {
    fn of(model: &CanvasModel, id: ElementId) -> Self {
        let g = model
            .element_geometry(std::slice::from_ref(&id))
            .pop()
            .unwrap_or_else(|| panic!("{id:?} answers geometry"));
        HandleBox {
            bounds: g.bounds,
            m: g.item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        }
    }

    /// The axis-aligned box around several items — what a multi-selection
    /// draws (InDesign: probe cases 1c–1e).
    fn union(boxes: &[HandleBox]) -> Self {
        let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for bx in boxes {
            for h in ["nw", "ne", "se", "sw"] {
                let (x, y) = bx.handle(h);
                l = l.min(x);
                t = t.min(y);
                r = r.max(x);
                b = b.max(y);
            }
        }
        HandleBox {
            bounds: [t, l, b, r],
            m: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }

    fn handle(&self, name: &str) -> Pt {
        let [t, l, b, r] = self.bounds;
        let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
        let local = match name {
            "nw" => (l, t),
            "n" => (cx, t),
            "ne" => (r, t),
            "e" => (r, cy),
            "se" => (r, b),
            "s" => (cx, b),
            "sw" => (l, b),
            "w" => (l, cy),
            other => panic!("handle {other}"),
        };
        map(self.m, local)
    }

    /// Unit vectors along the box's own x and y axes, page space.
    fn axes(&self) -> (Pt, Pt) {
        let unit = |(x, y): Pt| {
            let n = (x * x + y * y).sqrt();
            (x / n, y / n)
        };
        (unit((self.m[0], self.m[1])), unit((self.m[2], self.m[3])))
    }

    /// The probe's readback: edge lengths along the box's own axes, the
    /// angle its top edge makes (degrees, page space, y down) and the
    /// angle between its top and left edges (90 = a rectangle).
    fn measure(&self) -> Measure {
        let tl = self.handle("nw");
        let tr = self.handle("ne");
        let bl = self.handle("sw");
        let u = (tr.0 - tl.0, tr.1 - tl.1);
        let v = (bl.0 - tl.0, bl.1 - tl.1);
        let w = (u.0 * u.0 + u.1 * u.1).sqrt();
        let h = (v.0 * v.0 + v.1 * v.1).sqrt();
        Measure {
            w,
            h,
            top_edge: u.1.atan2(u.0).to_degrees(),
            corner: ((u.0 * v.0 + u.1 * v.1) / (w * h)).acos().to_degrees(),
        }
    }
}

#[derive(Debug)]
struct Measure {
    w: f32,
    h: f32,
    top_edge: f32,
    corner: f32,
}

fn close(a: f32, b: f32, tol: f32, what: &str) {
    assert!((a - b).abs() < tol, "{what}: {a} vs {b}");
}

fn close_pt(a: Pt, b: Pt, what: &str) {
    assert!(
        (a.0 - b.0).abs() < 1e-2 && (a.1 - b.1).abs() < 1e-2,
        "{what}: {a:?} vs {b:?}"
    );
}

fn add(p: Pt, q: Pt) -> Pt {
    (p.0 + q.0, p.1 + q.1)
}

fn along(axes: (Pt, Pt), x: f32, y: f32) -> Pt {
    let ((ux, uy), (vx, vy)) = axes;
    (ux * x + vx * y, uy * x + vy * y)
}

/// Drag `grabbed` by `delta` with `pivot` fixed, as the canvas does.
fn scale_drag(
    model: &mut CanvasModel,
    targets: Vec<ElementId>,
    grabbed: Pt,
    pivot: Pt,
    delta: Pt,
    modifiers: GestureModifiers,
) {
    let anchor = GestureAnchor {
        page_id: PageId("p1".into()),
        point_in_page: grabbed,
        pivot_in_page: Some(pivot),
    };
    let h = model
        .begin_gesture(targets, GestureType::Scale, Some(anchor))
        .expect("begin scale");
    model
        .update_gesture(h, delta, modifiers)
        .expect("update scale");
    model.commit_gesture(h).expect("commit scale");
}

const PLAIN: GestureModifiers = GestureModifiers {
    shift: false,
    alt: false,
    disable_snap: false,
};

// ---- one rotated frame ----------------------------------------------

#[test]
fn a_side_handle_on_a_rotated_frame_lengthens_it_along_its_own_axis() {
    let mut m = model();
    let before = HandleBox::of(&m, rect("r30"));
    let axes = before.axes();
    let (east, west) = (before.handle("e"), before.handle("w"));
    // 40 pt out along the frame's own x axis, with 7 pt of wobble across it.
    scale_drag(
        &mut m,
        vec![rect("r30")],
        east,
        west,
        along(axes, 40.0, 7.0),
        PLAIN,
    );
    let after = HandleBox::of(&m, rect("r30"));
    let got = after.measure();
    close(got.w, 200.0, 1e-2, "width along its own axis");
    close(got.h, 60.0, 1e-2, "height untouched by a side handle");
    close(got.top_edge, -30.0, 1e-2, "rotation unchanged");
    close(got.corner, 90.0, 1e-2, "still a rectangle");
    close_pt(after.handle("w"), west, "the opposite handle stays put");
    close_pt(
        after.handle("e"),
        add(east, along(axes, 40.0, 0.0)),
        "the handle follows the pointer along the axis",
    );
}

#[test]
fn a_corner_handle_on_a_rotated_frame_scales_both_of_its_own_axes() {
    let mut m = model();
    let before = HandleBox::of(&m, rect("r30"));
    let axes = before.axes();
    let (se, nw) = (before.handle("se"), before.handle("nw"));
    let delta = along(axes, 40.0, 30.0);
    scale_drag(&mut m, vec![rect("r30")], se, nw, delta, PLAIN);
    let after = HandleBox::of(&m, rect("r30"));
    let got = after.measure();
    close(got.w, 200.0, 1e-2, "width");
    close(got.h, 90.0, 1e-2, "height");
    close(got.top_edge, -30.0, 1e-2, "rotation unchanged");
    close(got.corner, 90.0, 1e-2, "still a rectangle");
    close_pt(after.handle("nw"), nw, "the opposite corner stays put");
    close_pt(
        after.handle("se"),
        add(se, delta),
        "the corner follows the pointer",
    );
}

#[test]
fn shift_on_a_rotated_frame_keeps_its_proportions_in_its_own_axes() {
    let mut m = model();
    let before = HandleBox::of(&m, rect("r30"));
    let axes = before.axes();
    let (se, nw) = (before.handle("se"), before.handle("nw"));
    // 160 → 200 is ×1.25, 60 → 70 is ×1.17: the larger change drives both.
    let shift = GestureModifiers {
        shift: true,
        ..PLAIN
    };
    scale_drag(
        &mut m,
        vec![rect("r30")],
        se,
        nw,
        along(axes, 40.0, 10.0),
        shift,
    );
    let after = HandleBox::of(&m, rect("r30"));
    let got = after.measure();
    close(got.w, 200.0, 1e-2, "width");
    close(got.h, 75.0, 1e-2, "height keeps the proportion");
    close(got.top_edge, -30.0, 1e-2, "rotation unchanged");
    close(got.corner, 90.0, 1e-2, "still a rectangle");
    close_pt(after.handle("nw"), nw, "the opposite corner stays put");
}

#[test]
fn a_rotated_frame_scaled_and_undone_comes_back_exactly() {
    let mut m = model();
    let before = HandleBox::of(&m, rect("r30"));
    let axes = before.axes();
    scale_drag(
        &mut m,
        vec![rect("r30")],
        before.handle("se"),
        before.handle("nw"),
        along(axes, 40.0, 30.0),
        PLAIN,
    );
    m.undo().expect("undo");
    let back = HandleBox::of(&m, rect("r30"));
    for (a, b) in back.m.iter().zip(before.m) {
        assert!((a - b).abs() < 1e-4, "{:?} vs {:?}", back.m, before.m);
    }
}

// ---- one rotated group ------------------------------------------------

/// InDesign probe case 4c: a group turned 30° holding a 160 × 60 and an
/// 80 × 40 rectangle, scaled 150 % along its own x axis about its left
/// side — the members come back 240 × 60 and 120 × 40, still rectangles
/// at 30°.
#[test]
fn a_side_handle_on_a_rotated_group_scales_its_members_along_the_groups_axis() {
    let mut m = model();
    let before = HandleBox::of(&m, group("g30"));
    let axes = before.axes();
    let width = before.measure().w;
    close(width, 160.0, 1e-2, "the group's box is its members' union");
    let (east, west) = (before.handle("e"), before.handle("w"));
    scale_drag(
        &mut m,
        vec![group("g30")],
        east,
        west,
        along(axes, width * 0.5, 5.0),
        PLAIN,
    );
    let after = HandleBox::of(&m, group("g30"));
    let got = after.measure();
    close(got.w, 240.0, 1e-2, "group width");
    close(got.h, 140.0, 1e-2, "group height");
    close(got.top_edge, -30.0, 1e-2, "the group's rotation unchanged");
    close(
        got.corner,
        90.0,
        1e-2,
        "the group's box is still a rectangle",
    );
    close_pt(after.handle("w"), west, "the opposite handle stays put");
    for (id, w, h) in [("ga", 240.0, 60.0), ("gb", 120.0, 40.0)] {
        let leaf = HandleBox::of(&m, rect(id)).measure();
        close(leaf.w, w, 1e-2, &format!("{id} width"));
        close(leaf.h, h, 1e-2, &format!("{id} height"));
        close(leaf.top_edge, -30.0, 1e-2, &format!("{id} rotation"));
        close(
            leaf.corner,
            90.0,
            1e-2,
            &format!("{id} is still a rectangle"),
        );
    }
}

#[test]
fn a_corner_handle_on_a_rotated_group_keeps_the_opposite_corner() {
    let mut m = model();
    let before = HandleBox::of(&m, group("g30"));
    let axes = before.axes();
    let (se, nw) = (before.handle("se"), before.handle("nw"));
    // 160 × 140 → 200 × 210: ×1.25 along the group's x, ×1.5 along its
    // y. (A uniform scale is the same in every frame, so it would not
    // tell the group's axes from the page's.)
    let delta = along(axes, 40.0, 70.0);
    scale_drag(&mut m, vec![group("g30")], se, nw, delta, PLAIN);
    let after = HandleBox::of(&m, group("g30"));
    let got = after.measure();
    close(got.w, 200.0, 1e-2, "group width");
    close(got.h, 210.0, 1e-2, "group height");
    close(got.top_edge, -30.0, 1e-2, "rotation unchanged");
    close(got.corner, 90.0, 1e-2, "still a rectangle");
    close_pt(after.handle("nw"), nw, "the opposite corner stays put");
    close_pt(
        after.handle("se"),
        add(se, delta),
        "the corner follows the pointer",
    );
    for (id, w, h) in [("ga", 200.0, 90.0), ("gb", 100.0, 60.0)] {
        let leaf = HandleBox::of(&m, rect(id)).measure();
        close(leaf.w, w, 1e-2, &format!("{id} width"));
        close(leaf.h, h, 1e-2, &format!("{id} height"));
        close(leaf.top_edge, -30.0, 1e-2, &format!("{id} rotation"));
        close(
            leaf.corner,
            90.0,
            1e-2,
            &format!("{id} is still a rectangle"),
        );
    }
}

// ---- several items: InDesign scales them as one unit on the page axes ---

/// Scale the multi-selection's axis-aligned box 150 % across from its west
/// side (with some vertical wobble, which a side handle ignores), and
/// return each item's box after.
fn scale_union_150_percent_x(
    m: &mut CanvasModel,
    ids: &[&str],
) -> (HandleBox, HandleBox, Vec<HandleBox>) {
    let boxes: Vec<HandleBox> = ids.iter().map(|id| HandleBox::of(m, rect(id))).collect();
    let before = HandleBox::union(&boxes);
    let width = before.bounds[3] - before.bounds[1];
    scale_drag(
        m,
        ids.iter().map(|id| rect(id)).collect(),
        before.handle("e"),
        before.handle("w"),
        (width * 0.5, 9.0),
        PLAIN,
    );
    let after: Vec<HandleBox> = ids.iter().map(|id| HandleBox::of(m, rect(id))).collect();
    (before, HandleBox::union(&after), after)
}

/// InDesign probe cases 1d + 3b: two rectangles at 30° and 60° draw an
/// axis-aligned box and scale as one unit; 150 % across turns both into
/// parallelograms with InDesign's exact edges and angles.
#[test]
fn a_mixed_rotation_multi_selection_scales_as_one_unit_on_the_page_axes() {
    let mut m = model();
    let (before, after, items) = scale_union_150_percent_x(&mut m, &["m30", "m60"]);
    let [t0, l0, b0, r0] = before.bounds;
    let [t1, l1, b1, r1] = after.bounds;
    close(l1, l0, 1e-2, "the west side stays put");
    close(r1 - l1, (r0 - l0) * 1.5, 1e-2, "the union is 150 % as wide");
    close(t1, t0, 1e-2, "top untouched by a side handle");
    close(b1, b0, 1e-2, "bottom untouched by a side handle");
    // InDesign's readback (scale-frame-probe.json, 3b): the 160 × 60 at
    // 30° → 222.71 × 68.74, top edge −21.05°, corner 70.16°; the 80 × 40
    // at 60° → 91.65 × 55.68, top edge −49.11°, corner 70.16°.
    let m30 = items[0].measure();
    close(m30.w, 222.71, 1e-2, "m30 width");
    close(m30.h, 68.74, 1e-2, "m30 height");
    close(m30.top_edge, -21.05, 1e-2, "m30 top edge");
    close(m30.corner, 70.16, 1e-2, "m30 corner");
    let m60 = items[1].measure();
    close(m60.w, 91.65, 1e-2, "m60 width");
    close(m60.h, 55.68, 1e-2, "m60 height");
    close(m60.top_edge, -49.11, 1e-2, "m60 top edge");
    close(m60.corner, 70.16, 1e-2, "m60 corner");
}

/// InDesign probe cases 1c + 3a: sharing a rotation does not turn a
/// multi-selection's box — it stays axis-aligned and the unit scale
/// shears both rectangles alike.
#[test]
fn a_multi_selection_sharing_one_rotation_still_scales_on_the_page_axes() {
    let mut m = model();
    let (before, after, items) = scale_union_150_percent_x(&mut m, &["m30", "n30"]);
    close(
        after.bounds[1],
        before.bounds[1],
        1e-2,
        "the west side stays put",
    );
    close(
        after.bounds[3] - after.bounds[1],
        (before.bounds[3] - before.bounds[1]) * 1.5,
        1e-2,
        "the union is 150 % as wide",
    );
    for (id, b) in ["m30", "n30"].iter().zip(&items) {
        let got = b.measure();
        close(got.w, 222.71, 1e-2, &format!("{id} width"));
        close(got.h, 68.74, 1e-2, &format!("{id} height"));
        close(got.top_edge, -21.05, 1e-2, &format!("{id} top edge"));
        close(got.corner, 70.16, 1e-2, &format!("{id} corner"));
    }
}
