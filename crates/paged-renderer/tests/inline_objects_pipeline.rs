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

//! Inline and above-line anchored objects take room in their line, as
//! InDesign sets them — pinned against InDesign 20.0.1's own geometry
//! for `inline-objects.idml` (every line's baseline and every object's
//! visible box, read through its DOM; tolerance 0.25 pt).
//!
//! The IDML importer does not record where in its paragraph an anchored
//! object sits yet, so the test supplies the anchors the way the
//! importer will: `paged-gen` writes each object at the start of the run
//! that carries it, and the run after an object starts with the space
//! that separates it from the next word.

use std::path::PathBuf;

use paged_compose::{DisplayCommand, PathSegment};
use paged_renderer::{pipeline, BytesResolver, PipelineOptions};

const TOL: f32 = 0.25;

fn read_font(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read font fixture {}: {e}", p.display()))
}

/// Anchor each paragraph's objects where `paged-gen` wrote them (see the
/// module docs): at the start of every run after the first that opens
/// with a space or is empty, and at the start of a paragraph whose first
/// run opens with a space.
fn anchor_as_written(document: &mut paged_scene::Document) {
    for story in &mut document.stories {
        for para in &mut story.story.paragraphs {
            if para.anchored_frames.is_empty() {
                continue;
            }
            let mut offsets = Vec::new();
            let mut at = 0u32;
            for run in &para.runs {
                if run.text.is_empty() || run.text.starts_with(' ') {
                    offsets.push(at);
                }
                at += run.text.chars().count() as u32;
            }
            // An object closing the paragraph rides an empty run, which the
            // reader may drop.
            while offsets.len() < para.anchored_frames.len() {
                offsets.push(at);
            }
            offsets.truncate(para.anchored_frames.len());
            para.anchored_frame_offsets = offsets;
        }
    }
}

fn build() -> pipeline::BuiltDocument {
    let sample = paged_gen::samples::inline_objects::build();
    let bytes = paged_gen::write_idml(&sample).expect("write_idml");
    let mut document = idml_import::import_idml_doc(&bytes).expect("open");
    anchor_as_written(&mut document);
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    pipeline::build_document(&document, &opts).expect("build_document")
}

/// The body lines' baselines on `page`, top to bottom, page-local pt
/// (the label frame above y = 50 left out).
fn baselines(built: &pipeline::BuiltDocument, page: usize) -> Vec<f32> {
    let mut v: Vec<f32> = built.pages[page]
        .story_layout
        .iter()
        .map(|l| l.baseline_y_pt)
        .filter(|&b| b > 50.0)
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v
}

/// The visible box `[top, left, bottom, right]` of every stroked path on
/// `page`: the path's extent grown by half its stroke.
fn stroked_boxes(built: &pipeline::BuiltDocument, page: usize) -> Vec<[f32; 4]> {
    let list = &built.pages[page].list;
    let mut boxes = Vec::new();
    for c in &list.commands {
        let DisplayCommand::StrokePath {
            path_id,
            transform,
            stroke,
            ..
        } = c
        else {
            continue;
        };
        let Some(path) = list.paths.get(*path_id) else {
            continue;
        };
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for s in &path.segments {
            let pts: &[(f32, f32)] = &match *s {
                PathSegment::MoveTo { x, y } | PathSegment::LineTo { x, y } => vec![(x, y)],
                PathSegment::CubicTo { x, y, .. } => vec![(x, y)],
                _ => vec![],
            };
            for &(px, py) in pts {
                let (tx, ty) = transform.apply(px, py);
                x0 = x0.min(tx);
                y0 = y0.min(ty);
                x1 = x1.max(tx);
                y1 = y1.max(ty);
            }
        }
        let h = stroke.width * 0.5;
        boxes.push([y0 - h, x0 - h, y1 + h, x1 + h]);
    }
    boxes.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
    boxes
}

/// InDesign 20.0.1's answer for one page: every body line's baseline,
/// and every object's visible box `[top, left, bottom, right]`, read
/// through its DOM (`Line.baseline`, `PageItem.visibleBounds`) with the
/// ruler at the page origin.
struct Expected {
    page: usize,
    baselines: &'static [f32],
    boxes: &'static [[f32; 4]],
}

const EXPECTED: &[Expected] = &[
    // Inline, auto leading: a 24.5 pt box grows its line to 26.5 pt
    // (rise + auto leading − size), its top 2 pt under the previous
    // baseline; the 6.5 pt box changes nothing; the line after a grown
    // line steps back to 12 pt.
    Expected {
        page: 0,
        baselines: &[
            69.688, 81.688, 108.188, 120.188, 132.188, 182.688, 194.688, 221.188, 233.188, 245.188,
        ],
        boxes: &[
            [63.188, 36.0, 69.688, 66.5],
            [83.688, 162.401, 108.188, 192.901],
            [134.188, 280.844, 182.688, 311.344],
            [196.688, 36.0, 221.188, 66.5],
        ],
    },
    // Fixed 12 pt leading: the lines keep 12 pt and the boxes overlap
    // the lines above, still standing on their baselines.
    Expected {
        page: 1,
        baselines: &[
            69.688, 81.688, 93.688, 105.688, 117.688, 129.688, 141.688, 153.688, 165.688, 177.688,
        ],
        boxes: &[
            [63.188, 36.0, 69.688, 66.5],
            [69.188, 162.401, 93.688, 192.901],
            [81.188, 280.844, 129.688, 311.344],
            [129.188, 36.0, 153.688, 66.5],
        ],
    },
    // AnchorYoffset raises (+6) or lowers (−6) the box, and the line
    // grows by its rise only; fixed leading ignores it.
    Expected {
        page: 2,
        baselines: &[
            69.688, 102.188, 114.188, 134.688, 146.688, 158.688, 170.688, 249.688, 261.688,
            273.688, 285.688, 297.688,
        ],
        boxes: &[
            [71.688, 36.0, 96.188, 66.5],
            [116.188, 159.94, 140.688, 190.44],
            [231.188, 36.0, 255.688, 66.5],
        ],
    },
    // A frame's first line: the baseline sits the box's height below the
    // frame's top (it is the line's ascent), in both leading modes.
    Expected {
        page: 3,
        baselines: &[96.5, 108.5, 120.5, 276.5, 288.5, 300.5],
        boxes: &[[60.0, 36.0, 96.5, 66.5], [240.0, 36.0, 276.5, 66.5]],
    },
    // A box taller than its frame: its line goes overset (even as the
    // frame's first line), and the box with it — nothing is drawn.
    Expected {
        page: 4,
        baselines: &[69.688],
        boxes: &[],
    },
    // Above line: the line moves down by the box (24.5) and the space
    // after it (6 on the last one); the box aligns left / centre / right
    // to the column.
    Expected {
        page: 5,
        baselines: &[96.5, 108.5, 145.0, 157.0, 193.5, 205.5, 248.0, 260.0, 272.0],
        boxes: &[
            [63.362, 36.0, 87.862, 116.5],
            [111.862, 175.75, 136.362, 256.25],
            [160.362, 315.5, 184.862, 396.0],
            [208.862, 36.0, 233.362, 116.5],
        ],
    },
    // Above line as a frame's first line: a full leading under the box.
    Expected {
        page: 6,
        baselines: &[96.5, 108.5, 120.5, 282.5, 294.5, 306.5],
        boxes: &[
            [63.362, 36.0, 87.862, 116.5],
            [243.362, 175.75, 267.862, 256.25],
        ],
    },
];

#[test]
fn inline_and_above_line_objects_take_room_as_indesign_sets_them() {
    let built = build();
    let mut failures = Vec::new();
    for e in EXPECTED {
        let got = baselines(&built, e.page);
        if got.len() != e.baselines.len()
            || got
                .iter()
                .zip(e.baselines)
                .any(|(g, w)| (g - w).abs() > TOL)
        {
            failures.push(format!(
                "page {}: baselines {got:?}, InDesign {:?}",
                e.page + 1,
                e.baselines
            ));
        }
        let boxes = stroked_boxes(&built, e.page);
        let off = |g: &[f32; 4], w: &[f32; 4]| g.iter().zip(w).any(|(a, b)| (a - b).abs() > TOL);
        if boxes.len() != e.boxes.len() || boxes.iter().zip(e.boxes).any(|(g, w)| off(g, w)) {
            failures.push(format!(
                "page {}: boxes {boxes:?}, InDesign {:?}",
                e.page + 1,
                e.boxes
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Without a recorded anchor (what the IDML reader gives today) an
/// object anchors at its paragraph's start — and still takes its room.
#[test]
fn an_unrecorded_anchor_sits_at_the_paragraph_start() {
    let sample = paged_gen::samples::inline_objects::build();
    let bytes = paged_gen::write_idml(&sample).expect("write_idml");
    let document = idml_import::import_idml_doc(&bytes).expect("open");
    let mut resolver = BytesResolver::new();
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).expect("build_document");
    // Page 1, paragraph b: its 24.5 pt box moves from mid-line to the
    // line's start, and the line still grows to 26.5 pt.
    let boxes = stroked_boxes(&built, 0);
    let b = boxes[1];
    assert!((b[1] - 36.0).abs() < TOL, "box at the line start: {b:?}");
    assert!((b[2] - 108.188).abs() < TOL, "box on its baseline: {b:?}");
    let lines = baselines(&built, 0);
    assert!((lines[2] - 108.188).abs() < TOL, "line grown: {lines:?}");
}

/// A paragraph holding nothing but a picture (a Word inline picture on a
/// line of its own, as plugin-doc pours it) is a line as tall as the
/// picture: the picture stands on its baseline and the next paragraph
/// follows a line's pitch below.
#[test]
fn a_picture_alone_in_its_paragraph_is_a_line_of_its_height() {
    let sample = paged_gen::samples::inline_objects::build();
    let bytes = paged_gen::write_idml(&sample).expect("write_idml");
    let mut document = idml_import::import_idml_doc(&bytes).expect("open");
    // Page 1's body story: give its last paragraph's place to a picture
    // paragraph, then the last paragraph again.
    let story = document
        .stories
        .iter_mut()
        .find(|s| {
            s.story
                .paragraphs
                .first()
                .is_some_and(|p| p.runs.first().is_some_and(|r| r.text.contains("a01")))
        })
        .expect("page 1 body story");
    let paras = &mut story.story.paragraphs;
    let last = paras.pop().expect("paragraph e");
    let picture = paged_model::AnchoredFrame {
        frame_kind: paged_model::AnchoredFrameKind::Rectangle,
        self_id: Some("picture".into()),
        bounds: Some(paged_model::Bounds {
            top: 0.0,
            left: 0.0,
            bottom: 100.0,
            right: 80.0,
        }),
        item_transform: None,
        parent_story: None,
        setting: None,
        fill_color: None,
        stroke_color: Some("Color/Black".into()),
        stroke_weight: Some(1.0),
        fill_tint: None,
        gradient_fill_angle: None,
        applied_object_style: None,
        image_link: None,
        image_item_transform: None,
        children: Vec::new(),
    };
    paras.push(paged_model::Paragraph {
        paragraph_style: last.paragraph_style.clone(),
        runs: Vec::new(),
        anchored_frames: vec![picture],
        ..Default::default()
    });
    paras.push(last);
    anchor_as_written(&mut document);

    // The picture's character has no run to take a face from: the
    // document's default face sets it.
    let mut resolver = BytesResolver::new().with_default_font(read_font("Inter.ttf"));
    resolver.add_font("Inter", None, read_font("Inter.ttf"));
    let opts = PipelineOptions {
        assets: Some(&resolver),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&document, &opts).expect("build_document");
    let lines = baselines(&built, 0);
    let boxes = stroked_boxes(&built, 0);
    // d's last line at 221.188; the picture line rises 101 pt (100 + the
    // 1 pt stroke) and keeps auto leading's 2.4 pt over its 12 pt
    // default-size character; e follows 12 pt below at the body size.
    let picture_line = lines[8];
    let pic = boxes.last().copied().expect("picture box");
    assert!(
        (pic[2] - picture_line).abs() < TOL,
        "the picture stands on its line: {pic:?} / {lines:?}"
    );
    assert!(
        (pic[0] - (221.188 + 2.4)).abs() < TOL,
        "its top clears the previous baseline by auto leading's gap: {pic:?}"
    );
    assert!((pic[1] - 36.0).abs() < TOL, "at the line start: {pic:?}");
    assert!(
        (lines[9] - picture_line - 12.0).abs() < TOL,
        "the next paragraph a line's pitch below: {lines:?}"
    );
}
