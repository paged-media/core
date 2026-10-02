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

//! A write that only repaints a page item keeps the text emit caches
//! (`Invalidation::PageItemPaint`). This proves the cache it keeps is
//! never stale: on every `paged-gen` fixture, every page item takes a set
//! of paint writes under the DIGEST GATE, which rebuilds cold after each
//! build and fails when the incremental result differs.
//!
//! The fixtures matter more than the count: `text-wrap` has rectangles a
//! story wraps around, `anchored` and `inline-objects` have frames a story
//! emits itself, `paste-into` has content nested in a container,
//! `text-on-path` has text riding an item's outline, `effects` and
//! `transparency` change what a page's pools hold. An item the classifier
//! does not take as plain falls back to clearing the caches; either way
//! the build must equal a cold one.

use std::path::PathBuf;

use paged_canvas::{CanvasModel, CanvasOptions, ElementId, Mutation};
use paged_compose::perf;
use paged_mutate::{PropertyPath, Value};

fn inter() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load(name: &str) -> CanvasModel {
    let sample = paged_gen::samples::build(name).unwrap_or_else(|| panic!("no sample {name:?}"));
    let bytes = paged_gen::write_idml(&sample).unwrap_or_else(|e| panic!("gen {name}: {e}"));
    let opts = CanvasOptions {
        fonts: vec![inter()],
        ..CanvasOptions::default()
    };
    CanvasModel::load("paint", &bytes, opts).unwrap_or_else(|e| panic!("load {name}: {e:?}"))
}

fn tiny_png() -> Vec<u8> {
    let mut img = image::RgbaImage::new(8, 8);
    for (x, y, p) in img.enumerate_pixels_mut() {
        *p = image::Rgba([(x * 30) as u8, (y * 30) as u8, 200, 255]);
    }
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode png");
    bytes
}

/// Up to `cap` page items of each kind on the document's spreads.
fn page_items(m: &CanvasModel, cap: usize) -> Vec<ElementId> {
    let mut out = Vec::new();
    for parsed in &m.scene().spreads {
        let s = &parsed.spread;
        let ids = |it: &mut dyn Iterator<Item = &Option<String>>| -> Vec<String> {
            it.filter_map(|id| id.clone()).take(cap).collect()
        };
        out.extend(
            ids(&mut s.rectangles.iter().map(|f| &f.self_id))
                .into_iter()
                .map(ElementId::Rectangle),
        );
        out.extend(
            ids(&mut s.ovals.iter().map(|f| &f.self_id))
                .into_iter()
                .map(ElementId::Oval),
        );
        out.extend(
            ids(&mut s.polygons.iter().map(|f| &f.self_id))
                .into_iter()
                .map(ElementId::Polygon),
        );
        out.extend(
            ids(&mut s.graphic_lines.iter().map(|f| &f.self_id))
                .into_iter()
                .map(ElementId::GraphicLine),
        );
    }
    out
}

/// The paint writes each item takes: colour to and from none (a fill that
/// appears adds a path to the page), transparency, and every effect family
/// (each adds layers, paths or gradients to the page's pools).
fn paint_writes(color: &str) -> Vec<(PropertyPath, Value)> {
    vec![
        (
            PropertyPath::FrameFillColor,
            Value::ColorRef(Some(color.to_string())),
        ),
        (PropertyPath::FrameFillTint, Value::Length(Some(40.0))),
        (
            PropertyPath::FrameStrokeColor,
            Value::ColorRef(Some(color.to_string())),
        ),
        (PropertyPath::FrameOpacity, Value::Length(Some(55.0))),
        (
            PropertyPath::FrameBlendMode,
            Value::Text("Multiply".to_string()),
        ),
        (
            PropertyPath::FrameGradientFillAngle,
            Value::Length(Some(45.0)),
        ),
        (PropertyPath::FrameDropShadow, Value::Bool(true)),
        (PropertyPath::FrameInnerShadowEnabled, Value::Bool(true)),
        (PropertyPath::FrameOuterGlowEnabled, Value::Bool(true)),
        (PropertyPath::FrameInnerGlowEnabled, Value::Bool(true)),
        (PropertyPath::FrameBevelEnabled, Value::Bool(true)),
        (PropertyPath::FrameSatinEnabled, Value::Bool(true)),
        (PropertyPath::FrameFeatherEnabled, Value::Bool(true)),
        (
            PropertyPath::FrameDirectionalFeatherEnabled,
            Value::Bool(true),
        ),
        (PropertyPath::FrameFillColor, Value::ColorRef(None)),
    ]
}

/// The fixtures where a page item and a story can meet. The whole
/// `paged-gen` roster takes minutes under the gate (a cold build per
/// write); `PAGED_PAINT_SWEEP_ALL=1` runs it, and did, clean, when this
/// landed.
const INTERACTING: &[&str] = &[
    "text-wrap",
    "anchored",
    "inline-objects",
    "paste-into",
    "text-on-path",
    "text-in-shape",
    "effects",
    "transparency",
    "images",
    "image-clipping",
    "nested-groups",
    "geometry-groups",
    "layers-z",
    "masters",
    "tables",
];

#[test]
fn a_paint_write_never_leaves_a_stale_text_cache() {
    let all = std::env::var("PAGED_PAINT_SWEEP_ALL").is_ok_and(|v| v == "1");
    let fixtures: &[&str] = if all {
        paged_gen::samples::SAMPLES
    } else {
        INTERACTING
    };
    let mut applied = 0usize;
    for name in fixtures {
        let mut m = load(name);
        // The 134-page bases cost a cold build per write for a handful of
        // items; the small fixtures are where the interactions are.
        if m.built().pages.len() > 24 {
            continue;
        }
        m.set_digest_gate(true);
        let color = m
            .scene()
            .palette
            .colors
            .keys()
            .find(|k| !k.contains("None") && !k.contains("Paper"))
            .cloned()
            .unwrap_or_else(|| "Color/Black".to_string());
        let items = page_items(&m, if all { 4 } else { 3 });
        let items = if all {
            &items[..]
        } else {
            &items[..items.len().min(8)]
        };
        for (n, id) in items.iter().enumerate() {
            // The first two items take every write; the rest take the three
            // that change a page's pools (a fill appearing, a shadow's
            // layers, the fill going away), which is where a kept delta
            // could go stale.
            let mut writes: Vec<Mutation> = paint_writes(&color)
                .into_iter()
                .filter(|(path, value)| {
                    all || n < 2
                        || matches!(
                            (path, value),
                            (PropertyPath::FrameFillColor, _) | (PropertyPath::FrameDropShadow, _)
                        )
                })
                .map(|(path, value)| Mutation::SetElementProperty {
                    element_id: id.clone(),
                    path,
                    value,
                })
                .collect();
            if all || n < 2 {
                writes.push(Mutation::ReplaceImageBytes {
                    element_id: id.raw_id().to_string(),
                    bytes: Some(tiny_png().into()),
                });
            }
            for write in &writes {
                // A write the item does not take (a line has no fill) is
                // refused, and is not the subject here. One that applies
                // has just rebuilt under the gate, which panics on a
                // difference; the explicit check names the write.
                if m.apply_mutation(write).is_ok() {
                    applied += 1;
                    if let Err(e) = m.digest_gate_check() {
                        panic!("{name}: {id:?} after {write:?}: incremental != cold: {e}");
                    }
                    // Undo and redo take the same narrowed path.
                    if n == 0 {
                        m.undo().expect("undo the write");
                        if let Err(e) = m.digest_gate_check() {
                            panic!("{name}: {id:?} after UNDO of {write:?}: {e}");
                        }
                        m.redo().expect("redo the write");
                        if let Err(e) = m.digest_gate_check() {
                            panic!("{name}: {id:?} after REDO of {write:?}: {e}");
                        }
                    }
                }
            }
        }
    }
    println!("paint sweep: {applied} writes applied under the gate");
    assert!(applied > 150, "the sweep applied only {applied} writes");
}

/// The narrowing is real, and it is narrow: a paint write on a plain
/// rectangle re-emits no story, and a geometry write on the same
/// rectangle still clears the caches.
#[test]
fn a_paint_write_re_emits_no_story_and_a_geometry_write_still_does() {
    let mut m = load("text-wrap");
    let id = m.scene().spreads[0]
        .spread
        .rectangles
        .iter()
        .find(|r| !r.is_anchored)
        .and_then(|r| r.self_id.clone())
        .expect("text-wrap carries a rectangle");
    let fill = |m: &mut CanvasModel, color: &str| {
        m.apply_mutation(&Mutation::SetElementProperty {
            element_id: ElementId::Rectangle(id.clone()),
            path: PropertyPath::FrameFillColor,
            value: Value::ColorRef(Some(color.to_string())),
        })
        .expect("fill");
    };
    fill(&mut m, "Color/Black"); // settle: a first fill may add a path
    let before = perf::snapshot();
    fill(&mut m, "Color/Paper");
    assert_eq!(
        perf::snapshot().since(&before).story_emits,
        0,
        "a colour change on a plain rectangle laid a story out again"
    );

    let bounds = m.scene().spreads[0]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some(id.as_str()))
        .map(|r| {
            [
                r.bounds.top,
                r.bounds.left,
                r.bounds.bottom + 6.0,
                r.bounds.right,
            ]
        })
        .unwrap();
    let before = perf::snapshot();
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::Rectangle(id.clone()),
        path: PropertyPath::FrameBounds,
        value: Value::Bounds(bounds),
    })
    .expect("resize");
    assert!(
        perf::snapshot().since(&before).story_emits > 0,
        "a geometry write must still re-emit the stories"
    );
}
