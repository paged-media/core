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

//! Q-14 — master-page item inheritance + per-item override suppression.
//!
//! Renders the generated `masters` sample (three body pages, each with
//! its own master carrying a shared rect + an overridable rect) and
//! asserts:
//!
//!   * page 0 (inherit-both)  — both master rects stamped (2 black fills
//!     at the master positions).
//!   * page 1 (override-one)  — the overridable master rect is NOT
//!     double-painted: the position carries the body's paper replacement
//!     fill, no black master copy underneath. The shared master rect is
//!     still inherited.
//!   * page 2 (hide-all, ShowMasterItems="false") — no master rects at
//!     all.

use paged_compose::{Color, DisplayCommand, Paint};

/// Master shared rect — top-left. Mirrors the constants in the sample.
const SHARED_X: f32 = 48.0;
const SHARED_Y: f32 = 48.0;
/// Master overridable rect — bottom-right.
const OVR_X: f32 = 360.0;
const OVR_Y: f32 = 700.0;

/// Is `c` (very close to) black?
fn is_black(c: Color) -> bool {
    c.r < 0.05 && c.g < 0.05 && c.b < 0.05 && c.a > 0.95
}

/// Is `c` (very close to) white / paper?
fn is_white(c: Color) -> bool {
    c.r > 0.95 && c.g > 0.95 && c.b > 0.95 && c.a > 0.95
}

/// Count `FillPath` commands whose unit-rect top-left lands within 2pt
/// of `(x, y)` in page coords and whose solid paint matches `color_ok`.
fn count_fills_at(
    page: &paged_renderer::pipeline::BuiltPage,
    x: f32,
    y: f32,
    color_ok: impl Fn(Color) -> bool,
) -> usize {
    page.list
        .commands
        .iter()
        .filter(|cmd| match cmd {
            DisplayCommand::FillPath {
                paint: Paint::Solid(c),
                transform,
                ..
            } => {
                let (px, py) = transform.apply(0.0, 0.0);
                (px - x).abs() < 2.0 && (py - y).abs() < 2.0 && color_ok(*c)
            }
            _ => false,
        })
        .count()
}

fn build_masters() -> paged_renderer::pipeline::BuiltDocument {
    let sample = paged_gen::samples::masters::build();
    let bytes = paged_gen::write_idml(&sample).expect("write masters idml");
    let doc = paged_scene::Document::open(&bytes).expect("open masters idml");
    let options = paged_renderer::pipeline::PipelineOptions::default();
    paged_renderer::pipeline::build_document(&doc, &options).expect("build masters")
}

#[test]
fn inherit_both_stamps_both_master_rects() {
    let built = build_masters();
    let page = &built.pages[0]; // "masters · inherit-both"
    assert_eq!(
        count_fills_at(page, SHARED_X, SHARED_Y, is_black),
        1,
        "shared master rect should be stamped once"
    );
    assert_eq!(
        count_fills_at(page, OVR_X, OVR_Y, is_black),
        1,
        "overridable master rect should be stamped once (not overridden)"
    );
}

#[test]
fn override_suppresses_master_copy_no_double_paint() {
    let built = build_masters();
    let page = &built.pages[1]; // "masters · override-one"

    // The shared master rect is NOT overridden — still inherited.
    assert_eq!(
        count_fills_at(page, SHARED_X, SHARED_Y, is_black),
        1,
        "shared master rect must still be inherited on the override page"
    );

    // The overridable master rect's BLACK master copy must be suppressed
    // — this is the Q-14 invariant. If it leaked through, the body's
    // paper override would be double-painted over the black placeholder.
    assert_eq!(
        count_fills_at(page, OVR_X, OVR_Y, is_black),
        0,
        "overridden master copy must NOT be stamped (no double-paint)"
    );

    // The body's own replacement (paper / white) takes its place.
    assert_eq!(
        count_fills_at(page, OVR_X, OVR_Y, is_white),
        1,
        "body replacement frame should paint at the overridable position"
    );
}

#[test]
fn show_master_items_false_hides_all_master_rects() {
    let built = build_masters();
    let page = &built.pages[2]; // "masters · hide-all"
    assert_eq!(
        count_fills_at(page, SHARED_X, SHARED_Y, is_black),
        0,
        "ShowMasterItems=false must hide the shared master rect"
    );
    assert_eq!(
        count_fills_at(page, OVR_X, OVR_Y, is_black),
        0,
        "ShowMasterItems=false must hide the overridable master rect"
    );
}
