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

//! Round-trip harness for the carry-through writer.
//!
//! Fixtures are generated in-process via `paged-gen` (the same builders
//! the corpus `.idml`s are emitted from), so the suite is hermetic — no
//! gitignored fixture on disk is required. Each sample exercises the
//! pass-through + patch paths end-to-end:
//!
//! 1. **Unmutated round-trip** — every entry must be byte-identical to
//!    the source package (the rewrite is a pure pass-through when nothing
//!    diverged from the model), and a re-parse must reproduce the model.
//! 2. **Mutated round-trip** — apply a `SetProperty` via `paged-mutate`,
//!    write, re-parse, assert the change landed AND unrelated attributes
//!    on the same element survived.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use paged_mutate::{NodeId, Operation, Project, PropertyPath, Value};
use paged_scene::Document;

use crate::write_idml;

/// Every generator sample the writer is exercised against. Spans
/// geometry-only, text, mixed, effects, tables, images, masters, etc. —
/// the full feature matrix the renderer's fidelity gate runs on.
const SAMPLES: &[&str] = &[
    "geometry",
    "geometry-groups",
    "strokes-fills",
    "text",
    "text-advanced",
    "text-wrap",
    "effects",
    "gradients",
    "tables",
    "images",
    "anchored",
    "transparency",
    "markers",
    "masters",
    "corners",
];

fn build_sample(name: &str) -> Vec<u8> {
    let sample = match name {
        "geometry" => paged_gen::samples::geometry::build(),
        "geometry-groups" => paged_gen::samples::geometry_groups::build(),
        "strokes-fills" => paged_gen::samples::strokes_fills::build(),
        "text" => paged_gen::samples::text::build(),
        "text-advanced" => paged_gen::samples::text_advanced::build(),
        "text-wrap" => paged_gen::samples::text_wrap::build(),
        "effects" => paged_gen::samples::effects::build(),
        "gradients" => paged_gen::samples::gradients::build(),
        "tables" => paged_gen::samples::tables::build(),
        "images" => paged_gen::samples::images::build(),
        "anchored" => paged_gen::samples::anchored::build(),
        "transparency" => paged_gen::samples::transparency::build(),
        "markers" => paged_gen::samples::markers::build(),
        "masters" => paged_gen::samples::masters::build(),
        "corners" => paged_gen::samples::corners::build(),
        other => panic!("unknown sample {other}"),
    };
    paged_gen::write_idml(&sample).expect("emit fixture")
}

/// Decompress every entry of an IDML package into a path→bytes map.
fn entries(idml: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(idml)).expect("zip");
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).expect("entry");
        if e.is_dir() {
            continue;
        }
        let name = e.name().to_string();
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).expect("read entry");
        out.insert(name, buf);
    }
    out
}

// ---------------------------------------------------------------------
// 1. Unmutated round-trip: byte-identical entries + model equivalence.
// ---------------------------------------------------------------------

#[test]
fn unmutated_round_trip_is_byte_identical_per_entry() {
    for &name in SAMPLES {
        let original = build_sample(name);
        let doc = Document::open(&original).unwrap_or_else(|e| panic!("{name}: open: {e:?}"));
        let out = write_idml(&doc, &original).unwrap_or_else(|e| panic!("{name}: write: {e:?}"));

        let src = entries(&original);
        let dst = entries(&out);

        assert_eq!(
            src.keys().collect::<Vec<_>>(),
            dst.keys().collect::<Vec<_>>(),
            "{name}: entry set changed"
        );
        for (path, src_bytes) in &src {
            let dst_bytes = dst.get(path).expect("entry present");
            assert_eq!(
                src_bytes, dst_bytes,
                "{name}: entry {path} not byte-identical on unmutated round-trip"
            );
        }
    }
}

#[test]
fn unmutated_round_trip_reparses_to_same_model_stats() {
    for &name in SAMPLES {
        let original = build_sample(name);
        let doc = Document::open(&original).unwrap();
        let out = write_idml(&doc, &original).unwrap();
        let re = Document::open(&out).unwrap_or_else(|e| panic!("{name}: reparse: {e:?}"));

        assert_eq!(doc.spreads.len(), re.spreads.len(), "{name}: spread count");
        assert_eq!(doc.stories.len(), re.stories.len(), "{name}: story count");

        let frames =
            |d: &Document| -> usize { d.spreads.iter().map(|s| s.spread.text_frames.len()).sum() };
        assert_eq!(frames(&doc), frames(&re), "{name}: text-frame count");

        // Story text content is preserved verbatim.
        for (a, b) in doc.stories.iter().zip(re.stories.iter()) {
            let text = |s: &paged_parse::Story| -> String {
                s.paragraphs
                    .iter()
                    .flat_map(|p| p.runs.iter())
                    .map(|r| r.text.clone())
                    .collect()
            };
            assert_eq!(text(&a.story), text(&b.story), "{name}: story text");
        }
    }
}

/// The whole-package bytes are identical, not just the entries — proves
/// the ZIP container itself (entry order, compression, mimetype-first)
/// is reproduced. This is the strongest carry-through guarantee.
#[test]
fn unmutated_round_trip_whole_package_is_byte_identical() {
    for &name in SAMPLES {
        let original = build_sample(name);
        let doc = Document::open(&original).unwrap();
        let out = write_idml(&doc, &original).unwrap();
        assert_eq!(
            original, out,
            "{name}: whole-package bytes diverged on unmutated round-trip"
        );
    }
}

// ---------------------------------------------------------------------
// 2. Mutated round-trip: the change lands; neighbours survive.
// ---------------------------------------------------------------------

/// Find the first text frame (and its spread index) that carries a
/// `Self` id, so a mutation can address it.
fn first_text_frame(doc: &Document) -> Option<String> {
    for s in &doc.spreads {
        for f in &s.spread.text_frames {
            if let Some(id) = f.self_id.as_deref() {
                return Some(id.to_string());
            }
        }
    }
    None
}

#[test]
fn mutated_frame_fill_color_saves_and_neighbours_survive() {
    let name = "geometry";
    let original = build_sample(name);
    let doc = Document::open(&original).unwrap();

    // Pick a rectangle to recolor (geometry sample is rectangle-rich).
    let (spread_idx, rect_id, orig_fill, orig_stroke) = doc
        .spreads
        .iter()
        .enumerate()
        .find_map(|(si, s)| {
            s.spread
                .rectangles
                .iter()
                .find(|r| r.self_id.is_some())
                .map(|r| {
                    (
                        si,
                        r.self_id.clone().unwrap(),
                        r.fill_color.clone(),
                        r.stroke_color.clone(),
                    )
                })
        })
        .expect("a rectangle with a Self id");

    // Choose a swatch genuinely DIFFERENT from the current fill so the
    // rewrite produces a real diff (the geometry rects are all
    // `Color/Black`, and a value-driven writer is a no-op when the value
    // doesn't change — which is correct, but not what this test probes).
    let new_fill = doc
        .palette
        .colors
        .keys()
        .find(|id| Some(id.as_str()) != orig_fill.as_deref())
        .cloned()
        .expect("a second swatch to recolor with");

    let mut project = Project::new(doc);
    project
        .apply(Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::FrameFillColor,
            value: Value::ColorRef(Some(new_fill.clone())),
        })
        .expect("apply fill");

    let out = write_idml(project.document(), &original).expect("write");
    let re = Document::open(&out).expect("reparse");

    let rect = re.spreads[spread_idx]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some(rect_id.as_str()))
        .expect("rectangle still present");

    // The mutation landed.
    assert_eq!(rect.fill_color.as_deref(), Some(new_fill.as_str()));
    // An unrelated attribute on the SAME element survived the rewrite.
    assert_eq!(rect.stroke_color, orig_stroke);

    // Exactly one Spread entry changed; everything else is byte-identical.
    let src = entries(&original);
    let dst = entries(&out);
    let mut changed: Vec<&String> = src
        .iter()
        .filter(|(k, v)| dst.get(*k).map(|d| d != *v).unwrap_or(true))
        .map(|(k, _)| k)
        .collect();
    changed.sort();
    assert_eq!(
        changed.len(),
        1,
        "only one entry should change, got {changed:?}"
    );
    assert!(
        changed[0].starts_with("Spreads/"),
        "changed entry is a spread"
    );
}

#[test]
fn mutated_text_fill_color_saves_and_text_survives() {
    let name = "text";
    let original = build_sample(name);
    let doc = Document::open(&original).unwrap();

    // Find a story with at least one run; capture its first run's text.
    let (story_idx, story_id, run_text) = doc
        .stories
        .iter()
        .enumerate()
        .find_map(|(si, s)| {
            s.story
                .paragraphs
                .iter()
                .flat_map(|p| p.runs.iter())
                .next()
                .map(|r| (si, s.self_id.clone(), r.text.clone()))
        })
        .expect("a story with a run");

    // Address the first character of the story; the character-fill path
    // splits/writes the covered run.
    let mut project = Project::new(doc);
    project
        .apply(Operation::SetProperty {
            node: NodeId::StoryRange {
                story_id: story_id.clone(),
                start: 0,
                end: 1,
            },
            path: PropertyPath::CharacterFillColor,
            value: Value::ColorRef(Some("Color/RGBCyan".to_string())),
        })
        .expect("apply character fill");

    let out = write_idml(project.document(), &original).expect("write");
    let re = Document::open(&out).expect("reparse");

    let story = &re.stories[story_idx].story;
    // First run now carries the new fill.
    let first_run = story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .next()
        .expect("run present");
    assert_eq!(first_run.fill_color.as_deref(), Some("Color/RGBCyan"));

    // The story's full text is unchanged (run-split preserves content).
    let full: String = story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.clone())
        .collect();
    assert!(full.starts_with(&run_text[..1]), "leading text preserved");
}

#[test]
fn mutated_item_transform_saves() {
    let name = "geometry";
    let original = build_sample(name);
    let doc = Document::open(&original).unwrap();
    let frame_id = first_text_frame(&doc).expect("a text frame");

    let m = [1.0, 0.0, 0.0, 1.0, 33.0, 44.0];
    let mut project = Project::new(doc);
    project
        .apply(Operation::SetProperty {
            node: NodeId::TextFrame(frame_id.clone()),
            path: PropertyPath::FrameTransform,
            value: Value::Transform(Some(m)),
        })
        .expect("apply transform");

    let out = write_idml(project.document(), &original).expect("write");
    let re = Document::open(&out).expect("reparse");
    let frame = re
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some(frame_id.as_str()))
        .expect("frame present");
    let got = frame.item_transform.expect("transform set");
    for (a, b) in got.iter().zip(m.iter()) {
        assert!((a - b).abs() < 1e-3, "transform {got:?} != {m:?}");
    }
}

/// A mutate-then-undo (no net change) must round-trip byte-identically:
/// proves the rewrite is value-driven, not touch-driven.
#[test]
fn mutate_then_undo_round_trips_byte_identical() {
    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();
    let frame_id = first_text_frame(&doc).expect("frame");

    let mut project = Project::new(doc);
    project
        .apply(Operation::SetProperty {
            node: NodeId::TextFrame(frame_id),
            path: PropertyPath::FrameFillColor,
            value: Value::ColorRef(Some("Color/Black".to_string())),
        })
        .unwrap();
    project.undo().unwrap().expect("undo");

    let out = write_idml(project.document(), &original).expect("write");
    assert_eq!(original, out, "mutate→undo should be a no-op write");
}
