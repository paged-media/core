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

//! B-23 — corner options on a POLYGON, end to end.
//!
//! The `corners` sample now carries, on every option page, a right
//! triangle (legs 200 pt from a 90° corner, hypotenuse closing two 45°
//! corners) with `TopLeftCornerOption` + a 20 pt radius. That shape is
//! the whole point of the generalisation: a rectangle only ever presents
//! 90° corners, where the tangent points sit exactly `r` back from the
//! vertex and a quarter-circle KAPPA is exact. At 45° neither holds, so
//! these tests pin the true inscribed-circle construction.
//!
//! Mirrors the B-18 `paste_into.rs` idiom: display-list structure
//! assertions plus 72 dpi pixel probes (1 pt = 1 px).

use paged_compose::{Color, DisplayCommand, PathSegment};

/// Sample geometry, in spread/pixel space (the polygon's item transform
/// is a pure translate, and at 72 dpi 1 pt = 1 px).
const OX: f32 = 150.0;
const OY: f32 = 60.0;
const LEG: f32 = 200.0;
const R: f32 = 20.0;

fn document() -> paged_scene::Document {
    let bytes = paged_gen::write_idml(&paged_gen::samples::corners::build()).expect("write_idml");
    idml_import::import_idml_doc(&bytes).expect("open corners idml")
}

fn built() -> paged_renderer::pipeline::BuiltDocument {
    let options = paged_renderer::pipeline::PipelineOptions::default();
    paged_renderer::pipeline::build_document(&document(), &options).expect("build corners")
}

/// Every `FillPath`'s segments on a page, in emit order.
fn fill_paths(page: &paged_renderer::pipeline::BuiltPage) -> Vec<Vec<PathSegment>> {
    page.list
        .commands
        .iter()
        .filter_map(|cmd| match cmd {
            DisplayCommand::FillPath { path_id, .. } => {
                page.list.paths.get(*path_id).map(|p| p.segments.clone())
            }
            _ => None,
        })
        .collect()
}

fn segment_points(segs: &[PathSegment]) -> Vec<(f32, f32)> {
    segs.iter()
        .filter_map(|s| match *s {
            PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => Some((x, y)),
            PathSegment::CubicTo { x, y, .. } => Some((x, y)),
            _ => None,
        })
        .collect()
}

/// The triangle's fill path: the one whose on-path extent fits inside
/// the 200 pt legs rather than the demo rectangle's 320×220. (The
/// corner effect pulls the extent in below `LEG`, so this is a bound,
/// not an equality.)
fn triangle_path(page: &paged_renderer::pipeline::BuiltPage) -> Vec<PathSegment> {
    let mut hits: Vec<Vec<PathSegment>> = fill_paths(page)
        .into_iter()
        .filter(|segs| {
            let pts = segment_points(segs);
            let w = pts.iter().fold(0.0f32, |a, p| a.max(p.0));
            w > 0.0 && w <= LEG + 0.01
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly one 200×200 fill path (the corner-effected triangle)"
    );
    hits.pop().unwrap()
}

fn count_cubics(segs: &[PathSegment]) -> usize {
    segs.iter()
        .filter(|s| matches!(s, PathSegment::CubicTo { .. }))
        .count()
}

fn count_lines(segs: &[PathSegment]) -> usize {
    segs.iter()
        .filter(|s| matches!(s, PathSegment::LineTo { .. }))
        .count()
}

/// Rounded: one convex arc per corner, and the raw vertices are gone —
/// three cubics (the corners) and three LineTos (the edges between the
/// tangent points).
#[test]
fn rounded_polygon_cuts_all_three_corners() {
    let built = built();
    let segs = triangle_path(&built.pages[0]);
    assert_eq!(count_cubics(&segs), 3, "one arc per triangle corner");
    assert_eq!(count_lines(&segs), 3, "one straight edge per corner pair");
    let pts = segment_points(&segs);
    for v in [(0.0f32, 0.0f32), (LEG, 0.0), (0.0, LEG)] {
        assert!(
            !pts.iter()
                .any(|p| (p.0 - v.0).abs() < 0.5 && (p.1 - v.1).abs() < 0.5),
            "sharp vertex {v:?} must be cut away, path points: {pts:?}"
        );
    }
}

/// The load-bearing claim of the generalisation: the tangent points sit
/// `r / tan(theta/2)` back from the vertex, NOT a flat `r`. At the
/// triangle's 90° corner that is `r` (identical to the rect builder); at
/// its 45° corners it is ≈ 2.414·r. A rect-shaped implementation would
/// put both at `r` and produce a visibly non-circular corner.
#[test]
fn tangent_distance_follows_the_corner_angle() {
    let built = built();
    let pts = segment_points(&triangle_path(&built.pages[0]));
    let near = |p: &(f32, f32), q: (f32, f32)| (p.0 - q.0).abs() < 0.05 && (p.1 - q.1).abs() < 0.05;

    // 90° corner at the origin: tangent points at exactly r.
    assert!(pts.iter().any(|p| near(p, (R, 0.0))), "{pts:?}");
    assert!(pts.iter().any(|p| near(p, (0.0, R))), "{pts:?}");

    // 45° corner at (LEG, 0): d = r / tan(22.5°).
    let d = R / (std::f32::consts::FRAC_PI_8).tan();
    assert!((d - 48.284_27).abs() < 0.01, "d = {d}");
    assert!(pts.iter().any(|p| near(p, (LEG - d, 0.0))), "{pts:?}");
    let diag = d * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        pts.iter().any(|p| near(p, (LEG - diag, diag))),
        "outgoing tangent point on the hypotenuse: {pts:?}"
    );
}

/// Inverse Rounded and Bevel — the two extra options the generalised
/// emitter must produce on a polygon, distinct from Rounded.
///
/// Page order mirrors the sample's `variants()`:
///   0 Rounded, 1 Inverse, 2 Bevel, 3 Inset, 4 Fancy.
#[test]
fn inverse_and_bevel_polygons_emit_their_own_geometry() {
    let built = built();

    // Inverse: same endpoint count as Rounded, but the arcs bulge the
    // other way — the control points sit OUTSIDE the convex hull of the
    // rounded arc, so the two paths are not equal.
    let rounded = triangle_path(&built.pages[0]);
    let inverse = triangle_path(&built.pages[1]);
    assert_eq!(count_cubics(&inverse), 3);
    assert_ne!(
        format!("{rounded:?}"),
        format!("{inverse:?}"),
        "inverse-rounded must not collapse onto rounded"
    );
    // The 90° corner's concave arc is a quarter circle about the VERTEX,
    // so its first control point lies on the incoming edge's offset —
    // `(KAPPA·r, r)` — whereas the convex arc's lies at `(0, r-KAPPA·r)`.
    let inv_ctl = inverse.iter().find_map(|s| match *s {
        PathSegment::CubicTo { cx1, cy1, .. } if cy1 > 19.0 && cy1 < 21.0 => Some((cx1, cy1)),
        _ => None,
    });
    assert!(
        matches!(inv_ctl, Some((cx, _)) if cx > 10.0),
        "concave control pulled toward the inner centre, got {inv_ctl:?}"
    );

    // Bevel: straight chamfers only — no cubics, six LineTos (one edge
    // plus one chamfer per corner).
    let bevel = triangle_path(&built.pages[2]);
    assert_eq!(count_cubics(&bevel), 0, "bevel is line-only");
    assert_eq!(count_lines(&bevel), 6, "3 edges + 3 chamfers");
}

/// Pixel probes at 72 dpi (1 pt = 1 px) on the Rounded page. The
/// triangle is black on white paper; every probe is chosen so that a
/// NON-corner-effected polygon would give the opposite answer at the
/// two corner-cut sites.
#[test]
fn rounded_polygon_pixels_cut_the_corners() {
    let (_built, images) = paged_renderer::pipeline::render_document(
        &document(),
        &paged_renderer::pipeline::PipelineOptions::default(),
        72.0,
        Color::WHITE,
    )
    .expect("render");
    let img = &images[0];
    let px = |x: f32, y: f32| img.get_pixel(x as u32, y as u32).0;
    let dark = |p: [u8; 4]| p[0] < 100 && p[1] < 100 && p[2] < 100;
    let light = |p: [u8; 4]| p[0] > 200 && p[1] > 200 && p[2] > 200;

    // Deep inside the triangle — unaffected by any corner.
    assert!(
        dark(px(OX + 50.0, OY + 50.0)),
        "{:?}",
        px(OX + 50.0, OY + 50.0)
    );
    assert!(
        dark(px(OX + 100.0, OY + 20.0)),
        "{:?}",
        px(OX + 100.0, OY + 20.0)
    );

    // 90° corner: 2 pt in from the vertex is inside the RAW triangle but
    // outside the 20 pt rounding.
    assert!(
        light(px(OX + 2.0, OY + 2.0)),
        "90° corner must be cut: {:?}",
        px(OX + 2.0, OY + 2.0)
    );
    // Just inside the arc, on the corner bisector at radius > r·√2.
    assert!(
        dark(px(OX + 22.0, OY + 22.0)),
        "inside the arc the polygon paints: {:?}",
        px(OX + 22.0, OY + 22.0)
    );

    // 45° corner at (LEG, 0): the cut reaches 48.28 pt back along the
    // leg, so a point 10 pt short of the vertex is gone…
    assert!(
        light(px(OX + LEG - 10.0, OY + 2.0)),
        "acute corner must be cut back further than r: {:?}",
        px(OX + LEG - 10.0, OY + 2.0)
    );
    // …while a point beyond the tangent distance still paints.
    assert!(
        dark(px(OX + LEG - 60.0, OY + 2.0)),
        "past the tangent point the leg is intact: {:?}",
        px(OX + LEG - 60.0, OY + 2.0)
    );

    // 45° corner at (0, LEG): same on the other acute vertex.
    assert!(
        light(px(OX + 5.0, OY + LEG - 10.0)),
        "second acute corner cut: {:?}",
        px(OX + 5.0, OY + LEG - 10.0)
    );
}
