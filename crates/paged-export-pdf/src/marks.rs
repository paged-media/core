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

//! Printer's marks — drawn by the exporter only (they never touch
//! the document scene), outside the trim into the bleed/slug/marks
//! area. Registration content paints in the `/Separation /All`
//! colorant so it hits every plate.

use pdf_writer::{Content, Name};

use crate::writer::{DocState, PageResources};
use crate::MarkOptions;

/// Geometry context (PDF y-up MEDIA space) for one page's marks.
pub struct MarkGeometry {
    pub media_w: f32,
    pub media_h: f32,
    /// Trim rectangle in media coords (x0, y0, x1, y1).
    pub trim: [f32; 4],
    /// Bleed rectangle in media coords.
    pub bleed: [f32; 4],
}

const MARK_LEN: f32 = 18.0;

/// Emit crop + registration marks, colour bars and page info.
pub fn emit_marks(
    content: &mut Content,
    state: &mut DocState,
    resources: &mut PageResources,
    geo: &MarkGeometry,
    opts: &MarkOptions,
) {
    if !(opts.crop_marks || opts.registration_marks || opts.color_bars || opts.page_info) {
        return;
    }
    let weight = if opts.weight_pt > 0.0 { opts.weight_pt } else { 0.25 };
    let offset = if opts.offset_pt > 0.0 { opts.offset_pt } else { 6.0 };

    content.save_state();
    // Registration colour: full coverage on every plate.
    let all = crate::color::registration_all_space(state, resources);
    let operand = pdf_writer::types::ColorSpaceOperand::Named(Name(all.as_bytes()));
    content.set_stroke_color_space(operand);
    content.set_stroke_color([1.0]);
    let fill_operand = pdf_writer::types::ColorSpaceOperand::Named(Name(all.as_bytes()));
    content.set_fill_color_space(fill_operand);
    content.set_fill_color([1.0]);
    content.set_line_width(weight);

    let [tx0, ty0, tx1, ty1] = geo.trim;
    let [bx0, by0, bx1, by1] = geo.bleed;

    if opts.crop_marks {
        // Eight crop marks: two per corner, starting `offset` past
        // the BLEED edge, length MARK_LEN, aligned with the trim.
        let mut line = |x0: f32, y0: f32, x1: f32, y1: f32| {
            content.move_to(x0, y0);
            content.line_to(x1, y1);
        };
        // Bottom-left corner.
        line(tx0, by0 - offset, tx0, by0 - offset - MARK_LEN);
        line(bx0 - offset, ty0, bx0 - offset - MARK_LEN, ty0);
        // Bottom-right.
        line(tx1, by0 - offset, tx1, by0 - offset - MARK_LEN);
        line(bx1 + offset, ty0, bx1 + offset + MARK_LEN, ty0);
        // Top-left.
        line(tx0, by1 + offset, tx0, by1 + offset + MARK_LEN);
        line(bx0 - offset, ty1, bx0 - offset - MARK_LEN, ty1);
        // Top-right.
        line(tx1, by1 + offset, tx1, by1 + offset + MARK_LEN);
        line(bx1 + offset, ty1, bx1 + offset + MARK_LEN, ty1);
        content.stroke();
    }

    if opts.registration_marks {
        // Circle-and-cross targets centred on each media edge.
        let centers = [
            ((tx0 + tx1) * 0.5, by0 - offset - MARK_LEN * 0.5),
            ((tx0 + tx1) * 0.5, by1 + offset + MARK_LEN * 0.5),
            (bx0 - offset - MARK_LEN * 0.5, (ty0 + ty1) * 0.5),
            (bx1 + offset + MARK_LEN * 0.5, (ty0 + ty1) * 0.5),
        ];
        let r = MARK_LEN * 0.30;
        for (cx, cy) in centers {
            if cx < 0.0 || cy < 0.0 || cx > geo.media_w || cy > geo.media_h {
                continue;
            }
            // Cross.
            content.move_to(cx - r * 1.4, cy);
            content.line_to(cx + r * 1.4, cy);
            content.move_to(cx, cy - r * 1.4);
            content.line_to(cx, cy + r * 1.4);
            // Circle via 4 bezier arcs (kappa).
            let k = 0.5523 * r;
            content.move_to(cx + r, cy);
            content.cubic_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r);
            content.cubic_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy);
            content.cubic_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r);
            content.cubic_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy);
            content.stroke();
        }
    }

    if opts.color_bars {
        // Process colour bar along the bottom slug: C/M/Y/K patches
        // at 100% + 50%.
        let patch = 12.0_f32;
        let y = (by0 - offset - patch).max(2.0);
        let mut x = tx0 + 2.0 * patch;
        let channels: [[f32; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        for tint in [1.0_f32, 0.5] {
            for ch in channels {
                content.set_fill_cmyk(ch[0] * tint, ch[1] * tint, ch[2] * tint, ch[3] * tint);
                content.rect(x, y, patch, patch);
                content.fill_nonzero();
                x += patch;
            }
        }
    }

    content.restore_state();
}
