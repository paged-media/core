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

//! PathData → PDF path-construction operators.

use paged_compose::{DashPattern, LineCap, LineJoin, PathData, PathSegment, Stroke};
use pdf_writer::Content;

/// Emit the path-construction ops for `path`. Quadratics are
/// degree-elevated to exact cubics (`c1 = p0 + 2/3(ctrl − p0)`,
/// `c2 = p + 2/3(ctrl − p)`) — no flattening, vectors stay vectors.
pub fn emit_path(content: &mut Content, path: &PathData) {
    let mut current: (f32, f32) = (0.0, 0.0);
    let mut start: (f32, f32) = (0.0, 0.0);
    for seg in &path.segments {
        match *seg {
            PathSegment::MoveTo { x, y } => {
                content.move_to(x, y);
                current = (x, y);
                start = (x, y);
            }
            PathSegment::LineTo { x, y } => {
                content.line_to(x, y);
                current = (x, y);
            }
            PathSegment::QuadTo { cx, cy, x, y } => {
                let c1x = current.0 + 2.0 / 3.0 * (cx - current.0);
                let c1y = current.1 + 2.0 / 3.0 * (cy - current.1);
                let c2x = x + 2.0 / 3.0 * (cx - x);
                let c2y = y + 2.0 / 3.0 * (cy - y);
                content.cubic_to(c1x, c1y, c2x, c2y, x, y);
                current = (x, y);
            }
            PathSegment::CubicTo {
                cx1,
                cy1,
                cx2,
                cy2,
                x,
                y,
            } => {
                content.cubic_to(cx1, cy1, cx2, cy2, x, y);
                current = (x, y);
            }
            PathSegment::Close => {
                content.close_path();
                current = start;
            }
        }
    }
}

/// Emit the stroke graphics-state ops (`w`/`J`/`j`/`M`/`d`).
pub fn emit_stroke_params(content: &mut Content, stroke: &Stroke) {
    content.set_line_width(stroke.width);
    content.set_line_cap(match stroke.cap {
        LineCap::Butt => pdf_writer::types::LineCapStyle::ButtCap,
        LineCap::Round => pdf_writer::types::LineCapStyle::RoundCap,
        LineCap::Square => pdf_writer::types::LineCapStyle::ProjectingSquareCap,
    });
    content.set_line_join(match stroke.join {
        LineJoin::Miter => pdf_writer::types::LineJoinStyle::MiterJoin,
        LineJoin::Round => pdf_writer::types::LineJoinStyle::RoundJoin,
        LineJoin::Bevel => pdf_writer::types::LineJoinStyle::BevelJoin,
    });
    content.set_miter_limit(stroke.miter_limit.max(1.0));
    emit_dash(content, &stroke.dash);
}

fn emit_dash(content: &mut Content, dash: &DashPattern) {
    let pattern = &dash.pattern[..dash.len as usize];
    if pattern.is_empty() {
        return; // solid — PDF default
    }
    content.set_dash_pattern(pattern.iter().copied(), 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_degree_elevation_is_exact_at_endpoints_and_midpoint() {
        // Quadratic from (0,0) ctrl (3,6) to (6,0). The elevated
        // cubic must pass through the same midpoint: Q(0.5) =
        // 0.25*p0 + 0.5*ctrl + 0.25*p2 = (3, 3).
        let mut content = Content::new();
        let path = PathData {
            segments: vec![
                PathSegment::MoveTo { x: 0.0, y: 0.0 },
                PathSegment::QuadTo {
                    cx: 3.0,
                    cy: 6.0,
                    x: 6.0,
                    y: 0.0,
                },
            ],
        };
        emit_path(&mut content, &path);
        let ops = String::from_utf8(content.finish().to_vec()).unwrap();
        // c1 = (2, 4), c2 = (4, 4) — exact 2/3 lerp.
        assert!(ops.contains("2 4 4 4 6 0 c"), "{ops}");
    }

    #[test]
    fn close_resets_current_point_for_following_quad() {
        let mut content = Content::new();
        let path = PathData {
            segments: vec![
                PathSegment::MoveTo { x: 10.0, y: 10.0 },
                PathSegment::LineTo { x: 20.0, y: 10.0 },
                PathSegment::Close,
                PathSegment::QuadTo {
                    cx: 13.0,
                    cy: 16.0,
                    x: 16.0,
                    y: 10.0,
                },
            ],
        };
        emit_path(&mut content, &path);
        let ops = String::from_utf8(content.finish().to_vec()).unwrap();
        // After close, current = (10,10): c1 = 10 + 2/3*3 = 12.
        assert!(ops.contains("12 14"), "{ops}");
    }
}
