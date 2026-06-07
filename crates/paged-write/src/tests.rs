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
    "text-letterspacing",
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
        "text-letterspacing" => paged_gen::samples::text_letterspacing::build(),
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

/// F1 (plugin-metadata facility §5) — the carrier round-trips: set
/// metadata → write → reparse → metadata-equal; delete → write →
/// reparse → gone; mutate-then-undo writes byte-identically.
#[test]
fn plugin_metadata_round_trips_through_write() {
    let envelope =
        r#"{"v":1,"engine":{"blitz":"0.3.0-alpha.4"},"data":{"source":"<b>hi & \"bye\"</b>"}}"#;
    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();
    let rect_id = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find_map(|r| r.self_id.clone())
        .expect("a rectangle");

    let mut project = Project::new(doc);
    project
        .apply(Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::PluginMetadata,
            value: Value::PluginMetadata {
                key: "x-paged:web".to_string(),
                value: Some(envelope.to_string()),
                prev: None,
            },
        })
        .expect("set metadata");

    // Write → reparse → the label is there, value byte-equal (incl.
    // the XML-escaped quotes/ampersands inside the JSON envelope).
    let out = write_idml(project.document(), &original).expect("write");
    let re = Document::open(&out).expect("reparse");
    let labels = re
        .spreads
        .iter()
        .find_map(|s| s.spread.labels.get(&rect_id))
        .expect("label written");
    assert_eq!(
        labels,
        &vec![("x-paged:web".to_string(), envelope.to_string())]
    );

    // Delete → write → gone again; and the output matches a write of
    // the never-labelled document (carrier leaves no residue).
    let mut project2 = Project::new(re);
    project2
        .apply(Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::PluginMetadata,
            value: Value::PluginMetadata {
                key: "x-paged:web".to_string(),
                value: None,
                prev: None,
            },
        })
        .expect("delete metadata");
    let out2 = write_idml(project2.document(), &out).expect("write 2");
    let re2 = Document::open(&out2).expect("reparse 2");
    assert!(
        re2.spreads
            .iter()
            .all(|s| !s.spread.labels.contains_key(&rect_id)),
        "label removed"
    );

    // Undo (exact restoration) → byte-identical write.
    let doc3 = Document::open(&original).unwrap();
    let mut project3 = Project::new(doc3);
    project3
        .apply(Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::PluginMetadata,
            value: Value::PluginMetadata {
                key: "x-paged:web".to_string(),
                value: Some(envelope.to_string()),
                prev: None,
            },
        })
        .unwrap();
    project3.undo().unwrap().expect("undo");
    let out3 = write_idml(project3.document(), &original).expect("write 3");
    assert_eq!(original, out3, "metadata set→undo is a no-op write");
}

/// The write gates (facility §2/§3): namespace prefix, size cap, and
/// the JSON envelope — all reject BEFORE mutation.
#[test]
fn plugin_metadata_write_gates_reject_cleanly() {
    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();
    let rect_id = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .find_map(|r| r.self_id.clone())
        .expect("a rectangle");
    let mut project = Project::new(doc);

    let set = |key: &str, value: Option<String>| Operation::SetProperty {
        node: NodeId::Rectangle(rect_id.clone()),
        path: PropertyPath::PluginMetadata,
        value: Value::PluginMetadata {
            key: key.to_string(),
            value,
            prev: None,
        },
    };

    // Wrong namespace.
    assert!(project
        .apply(set("vendor:web", Some(r#"{"v":1,"data":{}}"#.into())))
        .is_err());
    // Bare prefix (no plugin name).
    assert!(project
        .apply(set("x-paged:", Some(r#"{"v":1,"data":{}}"#.into())))
        .is_err());
    // Over the 64 KiB cap.
    let big = format!(r#"{{"v":1,"data":{{"blob":"{}"}}}}"#, "x".repeat(64 * 1024));
    assert!(project.apply(set("x-paged:web", Some(big))).is_err());
    // Not the envelope.
    assert!(project
        .apply(set("x-paged:web", Some("not json".into())))
        .is_err());
    assert!(project
        .apply(set("x-paged:web", Some(r#"{"data":{}}"#.into())))
        .is_err());
    assert!(project
        .apply(set("x-paged:web", Some(r#"{"v":1}"#.into())))
        .is_err());

    // Nothing mutated: the write is byte-identical.
    let out = write_idml(project.document(), &original).expect("write");
    assert_eq!(original, out, "rejected ops must not dirty the document");
}

// ---------------------------------------------------------------------
// 3. W3.B2a — multi-`<Content>` / `<Br>` / `<Tab>` text edits.
// ---------------------------------------------------------------------

/// Locate `(story_idx, para_idx, run_idx)` of the first run whose text
/// spans multiple `<Content>` segments (carries a `\t` / `\n`), so a
/// text edit on it exercises the Content/Br/Tab split rewrite. The
/// `text-advanced` tables sample carries tabbed columnar runs
/// (`"Apples\t1.20\t10\t12.00"`).
fn first_multi_content_run(doc: &Document) -> Option<(usize, usize, usize)> {
    for (si, s) in doc.stories.iter().enumerate() {
        for (pi, p) in s.story.paragraphs.iter().enumerate() {
            for (ri, r) in p.runs.iter().enumerate() {
                if r.text.contains('\t') || r.text.contains('\n') {
                    return Some((si, pi, ri));
                }
            }
        }
    }
    None
}

/// A text edit on a multi-`<Content>` run (tab-separated columns) saves
/// and re-parses with the Content/Tab structure intact — closing the
/// "text edits only save for single-Content runs" loss. The new text
/// keeps tabs so the re-emitted run is still multi-Content.
#[test]
fn mutated_multi_content_text_saves_with_tab_structure() {
    let original = build_sample("text-advanced");
    let mut doc = Document::open(&original).unwrap();
    let (si, pi, ri) = first_multi_content_run(&doc).expect("a multi-Content run");

    // Sanity: the source run really is tab-split.
    let old = doc.stories[si].story.paragraphs[pi].runs[ri].text.clone();
    assert!(old.contains('\t'), "fixture run is tab-separated");

    // Edit the model text directly (the run-text edit is what a higher
    // story-editing op produces); keep tabs so the structure must split.
    let new_text = "Pears\t9.99\t3\t29.97".to_string();
    doc.stories[si].story.paragraphs[pi].runs[ri].text = new_text.clone();

    let out = write_idml(&doc, &original).expect("write");
    assert_ne!(original, out, "a multi-Content text edit must change bytes");
    let re = Document::open(&out).expect("reparse");

    // The edited run re-parses to the new text WITH the tabs preserved
    // (proves the `<Content>…</Content><Tab/>…` structure was rebuilt,
    // not flattened into one Content).
    let got = &re.stories[si].story.paragraphs[pi].runs[ri].text;
    assert_eq!(got, &new_text, "edited run text saved + re-parsed");
    assert_eq!(got.matches('\t').count(), 3, "tab structure intact");

    // Neighbours (the sibling paragraphs' runs) are untouched.
    assert_eq!(
        re.stories[si].story.paragraphs[1].runs[0].text,
        doc.stories[si].story.paragraphs[1].runs[0].text,
        "sibling run survived"
    );
}

/// A `<Br/>`-bearing run (newline in the model) saves + re-parses with
/// the `<Br/>` structure intact. Built by editing a tabbed run to carry
/// a newline, proving `\n` → `<Br/>` on the rewrite side.
#[test]
fn mutated_run_with_newline_saves_br_structure() {
    let original = build_sample("text-advanced");
    let mut doc = Document::open(&original).unwrap();
    let (si, pi, ri) = first_multi_content_run(&doc).expect("a multi-Content run");

    doc.stories[si].story.paragraphs[pi].runs[ri].text = "line one\nline two".to_string();
    let out = write_idml(&doc, &original).expect("write");
    let re = Document::open(&out).expect("reparse");

    let got = &re.stories[si].story.paragraphs[pi].runs[ri].text;
    assert_eq!(
        got, "line one\nline two",
        "newline run round-trips as <Br/>"
    );
}

/// Content + Br/Tab byte-identity when unchanged: a multi-Content story
/// that isn't mutated must round-trip byte-for-byte (the structured
/// pass-through, the analogue of the entity-fix buffered span). This is
/// the per-entry guard for the tabbed `text-advanced` story.
#[test]
fn unmutated_multi_content_story_is_byte_identical() {
    let original = build_sample("text-advanced");
    let doc = Document::open(&original).unwrap();
    let out = write_idml(&doc, &original).expect("write");

    let src = entries(&original);
    let dst = entries(&out);
    // Every Stories/* entry — including the tab-columnar one — is
    // byte-identical on the unmutated round-trip.
    for (path, sb) in &src {
        if path.starts_with("Stories/") {
            assert_eq!(
                sb,
                dst.get(path).expect("entry present"),
                "{path}: multi-Content story not byte-identical unmutated"
            );
        }
    }
}

// ---------------------------------------------------------------------
// 4. W3.B2a — PathGeometry frame bounds / path-point edits.
// ---------------------------------------------------------------------

/// A `FrameBounds` mutation on a frame whose geometry lives in a
/// `<PathPointArray>` (a plain `<Rectangle>` from a real-shaped export —
/// no `GeometricBounds` attribute) now saves: the writer regenerates the
/// path corners from the model bounds. Re-parse shows the new bounds.
#[test]
fn mutated_frame_bounds_on_path_geometry_rect_saves() {
    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();

    // A geometry-sample rectangle carries its outline as a
    // `<PathPointArray>` (anchors empty in the model = the 4-corner AABB
    // case) and no `GeometricBounds` attribute — the exact loss case.
    let (spread_idx, rect_id, old_bounds) = doc
        .spreads
        .iter()
        .enumerate()
        .find_map(|(si, s)| {
            s.spread
                .rectangles
                .iter()
                .find(|r| r.self_id.is_some() && r.anchors.is_empty())
                .map(|r| (si, r.self_id.clone().unwrap(), r.bounds))
        })
        .expect("a path-geometry rectangle");

    // New bounds: grow the box. FrameBounds value is [top, left, bottom,
    // right].
    let new = [
        old_bounds.top,
        old_bounds.left,
        old_bounds.bottom + 40.0,
        old_bounds.right + 25.0,
    ];
    let mut project = Project::new(doc);
    project
        .apply(Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::FrameBounds,
            value: Value::Bounds(new),
        })
        .expect("apply bounds");

    let out = write_idml(project.document(), &original).expect("write");
    assert_ne!(original, out, "a path-geometry bounds edit must save");
    let re = Document::open(&out).expect("reparse");

    let rect = re.spreads[spread_idx]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some(rect_id.as_str()))
        .expect("rectangle present");

    // Re-parse derives the bounds from the rewritten `<PathPointArray>`
    // anchors — they reflect the new box.
    assert!((rect.bounds.top - new[0]).abs() < 1e-3, "top");
    assert!((rect.bounds.left - new[1]).abs() < 1e-3, "left");
    assert!((rect.bounds.bottom - new[2]).abs() < 1e-3, "bottom");
    assert!((rect.bounds.right - new[3]).abs() < 1e-3, "right");

    // Render-equivalence: the geometry the renderer consumes off the
    // re-parsed package (bounds + any anchors) is identical to the
    // directly-mutated in-memory model — saving then re-loading draws
    // the same frame. (The renderer derives a path-geometry rect from
    // these bounds; matching them ⇒ identical rasterisation.)
    let model_rect = project.document().spreads[spread_idx]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some(rect_id.as_str()))
        .expect("model rect");
    assert!((rect.bounds.top - model_rect.bounds.top).abs() < 1e-3);
    assert!((rect.bounds.left - model_rect.bounds.left).abs() < 1e-3);
    assert!((rect.bounds.bottom - model_rect.bounds.bottom).abs() < 1e-3);
    assert!((rect.bounds.right - model_rect.bounds.right).abs() < 1e-3);
    assert_eq!(
        rect.item_transform, model_rect.item_transform,
        "frame placement transform unchanged by a bounds edit"
    );

    // Only one spread entry changed.
    let src = entries(&original);
    let dst = entries(&out);
    let changed: Vec<&String> = src
        .iter()
        .filter(|(k, v)| dst.get(*k).map(|d| d != *v).unwrap_or(true))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(changed.len(), 1, "only one entry changed: {changed:?}");
    assert!(changed[0].starts_with("Spreads/"));
}

/// A `FramePathPoint` mutation (move one anchor of a path-geometry
/// frame) round-trips through save: the writer rewrites the
/// `<PathPointArray>`, and a re-parse shows the moved anchor.
#[test]
fn mutated_frame_path_point_round_trips_through_save() {
    use paged_mutate::{PathPointAddress, PathPointRole};

    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();

    // A geometry text frame keeps its 4 corner anchors in the model.
    let (spread_idx, frame_id, base) = doc
        .spreads
        .iter()
        .enumerate()
        .find_map(|(si, s)| {
            s.spread
                .text_frames
                .iter()
                .find(|f| f.self_id.is_some() && f.anchors.len() == 4)
                .map(|f| (si, f.self_id.clone().unwrap(), f.anchors[2].anchor))
        })
        .expect("a 4-anchor text frame");

    // Move anchor #2 by a clear delta.
    let target = [base.0 + 17.0, base.1 - 9.0];
    let mut project = Project::new(doc);
    project
        .apply(Operation::SetProperty {
            node: NodeId::TextFrame(frame_id.clone()),
            path: PropertyPath::FramePathPoint,
            value: Value::PathPoint {
                address: PathPointAddress {
                    index: 2,
                    role: PathPointRole::Anchor,
                },
                position: target,
            },
        })
        .expect("apply path point");

    let out = write_idml(project.document(), &original).expect("write");
    assert_ne!(original, out, "a path-point edit must save");
    let re = Document::open(&out).expect("reparse");

    let frame = re.spreads[spread_idx]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(frame_id.as_str()))
        .expect("frame present");
    assert_eq!(frame.anchors.len(), 4, "anchor count preserved");
    let moved = frame.anchors[2].anchor;
    assert!(
        (moved.0 - target[0]).abs() < 1e-3 && (moved.1 - target[1]).abs() < 1e-3,
        "anchor moved to {target:?}, got {moved:?}"
    );
    // An untouched anchor survived.
    let other = frame.anchors[0].anchor;
    assert!(
        (other.0 - 0.0).abs() < 1e-3 && (other.1 - 0.0).abs() < 1e-3,
        "neighbour anchor unchanged: {other:?}"
    );
}

/// A path-point mutate-then-undo writes byte-identically — proves the
/// `<PathPointArray>` rewrite is value-driven (compares formatted
/// anchors), not touch-driven.
#[test]
fn path_point_mutate_then_undo_round_trips_byte_identical() {
    use paged_mutate::{PathPointAddress, PathPointRole};

    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();
    let frame_id = doc
        .spreads
        .iter()
        .find_map(|s| {
            s.spread
                .text_frames
                .iter()
                .find(|f| f.self_id.is_some() && f.anchors.len() == 4)
                .and_then(|f| f.self_id.clone())
        })
        .expect("a 4-anchor text frame");

    let mut project = Project::new(doc);
    project
        .apply(Operation::SetProperty {
            node: NodeId::TextFrame(frame_id),
            path: PropertyPath::FramePathPoint,
            value: Value::PathPoint {
                address: PathPointAddress {
                    index: 1,
                    role: PathPointRole::Anchor,
                },
                position: [12.0, 34.0],
            },
        })
        .unwrap();
    project.undo().unwrap().expect("undo");

    let out = write_idml(project.document(), &original).expect("write");
    assert_eq!(original, out, "path-point set→undo is a no-op write");
}

// ---------------------------------------------------------------------
// 5. W1.15 — structural inserts / removes of page items.
// ---------------------------------------------------------------------

use paged_mutate::NodeSpec;

/// The `Self` id of the first spread carrying page items, for use as an
/// `InsertNode` parent.
fn first_spread_id(doc: &Document) -> String {
    doc.spreads
        .iter()
        .find_map(|s| s.spread.self_id.clone())
        .expect("a spread with a Self id")
}

/// An inserted `<Rectangle>` (created by an op since load) serialises as
/// a new XML element with its model geometry / fill, re-parses to the
/// same bounds + fill, and leaves every untouched entry byte-identical.
#[test]
fn inserted_rectangle_saves_and_reparses() {
    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();
    let spread_id = first_spread_id(&doc);
    let spread_idx = doc
        .spreads
        .iter()
        .position(|s| s.spread.self_id.as_deref() == Some(spread_id.as_str()))
        .unwrap();
    // Pick a real fill swatch so the round-trip resolves to a colour.
    let fill = doc.palette.colors.keys().next().cloned().expect("a swatch");
    let new_id = "Rectangle/w1insert".to_string();
    let bounds = [40.0_f32, 50.0, 140.0, 210.0]; // top, left, bottom, right
    let rect_pos = doc.spreads[spread_idx].spread.rectangles.len();

    let mut project = Project::new(doc);
    project
        .apply(Operation::InsertNode {
            parent: NodeId::Spread(spread_id.clone()),
            position: rect_pos,
            node: NodeSpec::Rectangle {
                self_id: new_id.clone(),
                bounds,
                fill_color: Some(fill.clone()),
                stroke_color: None,
                stroke_weight: None,
                item_transform: None,
            },
            z_slot: None,
        })
        .expect("insert rectangle");

    let out = write_idml(project.document(), &original).expect("write");
    assert_ne!(original, out, "an insert must change bytes");
    let re = Document::open(&out).expect("reparse");

    let rect = re.spreads[spread_idx]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some(new_id.as_str()))
        .expect("inserted rectangle re-parsed");
    assert_eq!(
        rect.fill_color.as_deref(),
        Some(fill.as_str()),
        "fill saved"
    );
    // Geometry derives from the rewritten `<PathGeometry>` corners.
    assert!((rect.bounds.top - bounds[0]).abs() < 1e-3, "top");
    assert!((rect.bounds.left - bounds[1]).abs() < 1e-3, "left");
    assert!((rect.bounds.bottom - bounds[2]).abs() < 1e-3, "bottom");
    assert!((rect.bounds.right - bounds[3]).abs() < 1e-3, "right");

    // Re-parsed model matches the in-memory mutated model.
    let model_rect = project.document().spreads[spread_idx]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some(new_id.as_str()))
        .expect("model rect");
    assert!((rect.bounds.top - model_rect.bounds.top).abs() < 1e-3);
    assert!((rect.bounds.right - model_rect.bounds.right).abs() < 1e-3);

    // Only one Spread entry changed.
    let src = entries(&original);
    let dst = entries(&out);
    let changed: Vec<&String> = src
        .iter()
        .filter(|(k, v)| dst.get(*k).map(|d| d != *v).unwrap_or(true))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(changed.len(), 1, "only the spread changed: {changed:?}");
    assert!(changed[0].starts_with("Spreads/"));
}

/// An inserted `<TextFrame>` (with a parent story) serialises with the
/// `ParentStory` / `ContentType` attributes so a re-parse recognises it
/// as a text frame, not a rectangle.
#[test]
fn inserted_text_frame_saves_as_text_frame() {
    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();
    let spread_id = first_spread_id(&doc);
    let spread_idx = doc
        .spreads
        .iter()
        .position(|s| s.spread.self_id.as_deref() == Some(spread_id.as_str()))
        .unwrap();
    let new_id = "TextFrame/w1insert".to_string();
    let before = doc.spreads[spread_idx].spread.text_frames.len();

    let mut project = Project::new(doc);
    project
        .apply(Operation::InsertNode {
            parent: NodeId::Spread(spread_id.clone()),
            position: before,
            node: NodeSpec::TextFrame {
                self_id: new_id.clone(),
                bounds: [10.0, 20.0, 90.0, 180.0],
                fill_color: None,
                stroke_color: None,
                stroke_weight: None,
                item_transform: None,
            },
            z_slot: None,
        })
        .expect("insert text frame");

    let out = write_idml(project.document(), &original).expect("write");
    let re = Document::open(&out).expect("reparse");
    assert_eq!(
        re.spreads[spread_idx].spread.text_frames.len(),
        before + 1,
        "text frame count grew by one"
    );
    let f = re.spreads[spread_idx]
        .spread
        .text_frames
        .iter()
        .find(|f| f.self_id.as_deref() == Some(new_id.as_str()))
        .expect("inserted text frame re-parsed as a TextFrame");
    assert!((f.bounds.right - 180.0).abs() < 1e-3, "frame bounds saved");
}

/// A `RemoveNode` (delete a frame created-or-loaded) drops the element
/// from the XML: the re-parse no longer carries it, and surviving
/// siblings still parse.
#[test]
fn removed_rectangle_drops_from_xml() {
    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();

    let (spread_idx, rect_id) = doc
        .spreads
        .iter()
        .enumerate()
        .find_map(|(si, s)| {
            s.spread
                .rectangles
                .iter()
                .find_map(|r| r.self_id.clone())
                .map(|id| (si, id))
        })
        .expect("a rectangle to remove");
    let before = doc.spreads[spread_idx].spread.rectangles.len();

    let mut project = Project::new(doc);
    project
        .apply(Operation::RemoveNode {
            node: NodeId::Rectangle(rect_id.clone()),
        })
        .expect("remove rectangle");

    let out = write_idml(project.document(), &original).expect("write");
    assert_ne!(original, out, "a remove must change bytes");
    let re = Document::open(&out).expect("reparse");
    assert!(
        re.spreads[spread_idx]
            .spread
            .rectangles
            .iter()
            .all(|r| r.self_id.as_deref() != Some(rect_id.as_str())),
        "removed rectangle is gone from the re-parsed model"
    );
    assert_eq!(
        re.spreads[spread_idx].spread.rectangles.len(),
        before - 1,
        "exactly one rectangle removed"
    );
}

/// Insert-then-undo (and remove-then-undo) write byte-identically:
/// proves the structural rewrite is value-driven (no element appears /
/// disappears when the net model is unchanged).
#[test]
fn structural_edit_then_undo_round_trips_byte_identical() {
    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();
    let spread_id = first_spread_id(&doc);
    let rect_id = doc
        .spreads
        .iter()
        .find_map(|s| s.spread.rectangles.iter().find_map(|r| r.self_id.clone()))
        .expect("a rectangle");

    // Insert → undo.
    let mut p1 = Project::new(doc);
    p1.apply(Operation::InsertNode {
        parent: NodeId::Spread(spread_id),
        position: 0,
        node: NodeSpec::Rectangle {
            self_id: "Rectangle/w1undo".to_string(),
            bounds: [0.0, 0.0, 10.0, 10.0],
            fill_color: None,
            stroke_color: None,
            stroke_weight: None,
            item_transform: None,
        },
        z_slot: None,
    })
    .unwrap();
    p1.undo().unwrap().expect("undo insert");
    let out1 = write_idml(p1.document(), &original).expect("write");
    assert_eq!(original, out1, "insert→undo is a no-op write");

    // Remove → undo.
    let doc2 = Document::open(&original).unwrap();
    let mut p2 = Project::new(doc2);
    p2.apply(Operation::RemoveNode {
        node: NodeId::Rectangle(rect_id),
    })
    .unwrap();
    p2.undo().unwrap().expect("undo remove");
    let out2 = write_idml(p2.document(), &original).expect("write");
    assert_eq!(original, out2, "remove→undo is a no-op write");
}

// ---------------------------------------------------------------------
// 6. W1.15 — new resources (swatches / gradients → Graphic.xml;
//    paragraph / character styles → Styles.xml).
// ---------------------------------------------------------------------

use paged_mutate::SwatchSpec;

/// A swatch created by `CreateSwatch` serialises into `Resources/Graphic.xml`
/// and re-parses with the same colour values — closing the
/// "referenced-but-undefined resource" loss.
#[test]
fn created_swatch_saves_to_graphic_and_reparses() {
    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();

    let mut project = Project::new(doc);
    project
        .apply(Operation::CreateSwatch {
            spec: SwatchSpec {
                self_id: Some("Color/w1new".to_string()),
                name: Some("W1 New".to_string()),
                space: "RGB".to_string(),
                value: vec![10.0, 120.0, 240.0],
                model: Some("Process".to_string()),
                alternate_space: None,
                alternate_value: Vec::new(),
                tint: None,
                alpha: None,
            },
        })
        .expect("create swatch");

    let out = write_idml(project.document(), &original).expect("write");
    assert_ne!(original, out, "a new swatch must change bytes");
    let re = Document::open(&out).expect("reparse");

    let color = re
        .palette
        .colors
        .get("Color/w1new")
        .expect("swatch re-parsed into the palette");
    assert_eq!(color.name.as_deref(), Some("W1 New"));
    assert_eq!(
        color.value,
        vec![10.0, 120.0, 240.0],
        "channel values saved"
    );

    // Only Graphic.xml changed.
    let src = entries(&original);
    let dst = entries(&out);
    let changed: Vec<&String> = src
        .iter()
        .filter(|(k, v)| dst.get(*k).map(|d| d != *v).unwrap_or(true))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(changed, vec!["Resources/Graphic.xml"], "only Graphic.xml");
}

/// A swatch create-then-undo writes byte-identically (value-driven, not
/// touch-driven).
#[test]
fn created_swatch_then_undo_round_trips_byte_identical() {
    let original = build_sample("geometry");
    let doc = Document::open(&original).unwrap();
    let mut project = Project::new(doc);
    project
        .apply(Operation::CreateSwatch {
            spec: SwatchSpec {
                self_id: Some("Color/w1undo".to_string()),
                name: Some("U".to_string()),
                space: "RGB".to_string(),
                value: vec![1.0, 2.0, 3.0],
                model: None,
                alternate_space: None,
                alternate_value: Vec::new(),
                tint: None,
                alpha: None,
            },
        })
        .unwrap();
    project.undo().unwrap().expect("undo");
    let out = write_idml(project.document(), &original).expect("write");
    assert_eq!(original, out, "swatch create→undo is a no-op write");
}

/// A paragraph style created by `CreateParagraphStyle` serialises into
/// `Resources/Styles.xml` (inside `RootParagraphStyleGroup`) and
/// re-parses with its name + based-on intact.
#[test]
fn created_paragraph_style_saves_to_styles_and_reparses() {
    let original = build_sample("text");
    let doc = Document::open(&original).unwrap();

    let mut project = Project::new(doc);
    project
        .apply(Operation::CreateParagraphStyle {
            self_id: Some("ParagraphStyle/w1head".to_string()),
            name: Some("W1 Heading".to_string()),
            based_on: Some("ParagraphStyle/$ID/[No paragraph style]".to_string()),
            restore_json: None,
        })
        .expect("create paragraph style");

    let out = write_idml(project.document(), &original).expect("write");
    assert_ne!(original, out, "a new style must change bytes");
    let re = Document::open(&out).expect("reparse");

    let style = re
        .styles
        .paragraph_styles
        .get("ParagraphStyle/w1head")
        .expect("style re-parsed into the stylesheet");
    assert_eq!(style.name.as_deref(), Some("W1 Heading"));
    assert_eq!(
        style.based_on.as_deref(),
        Some("ParagraphStyle/$ID/[No paragraph style]")
    );

    let src = entries(&original);
    let dst = entries(&out);
    let changed: Vec<&String> = src
        .iter()
        .filter(|(k, v)| dst.get(*k).map(|d| d != *v).unwrap_or(true))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(changed, vec!["Resources/Styles.xml"], "only Styles.xml");
}

/// A character style created via `CreateCharacterStyle` round-trips
/// (lands in `RootCharacterStyleGroup`).
#[test]
fn created_character_style_saves_to_styles() {
    let original = build_sample("text");
    let doc = Document::open(&original).unwrap();
    let mut project = Project::new(doc);
    project
        .apply(Operation::CreateCharacterStyle {
            self_id: Some("CharacterStyle/w1emph".to_string()),
            name: Some("W1 Emph".to_string()),
            based_on: None,
            restore_json: None,
        })
        .expect("create character style");
    let out = write_idml(project.document(), &original).expect("write");
    let re = Document::open(&out).expect("reparse");
    let style = re
        .styles
        .character_styles
        .get("CharacterStyle/w1emph")
        .expect("character style re-parsed");
    assert_eq!(style.name.as_deref(), Some("W1 Emph"));
}

/// The full W1.15 round-trip the task asks for: a created frame whose
/// fill references a NEW swatch, plus a NEW paragraph style — open
/// fixture, apply ops, save, re-open, and assert every piece re-parses
/// with its resolved appearance (frame present + fill resolves to the
/// new swatch; style present).
#[test]
fn created_frame_with_new_swatch_and_style_round_trips() {
    let original = build_sample("text");
    let doc = Document::open(&original).unwrap();
    let spread_id = first_spread_id(&doc);
    let spread_idx = doc
        .spreads
        .iter()
        .position(|s| s.spread.self_id.as_deref() == Some(spread_id.as_str()))
        .unwrap();
    let rect_pos = doc.spreads[spread_idx].spread.rectangles.len();

    let mut project = Project::new(doc);
    // New swatch.
    project
        .apply(Operation::CreateSwatch {
            spec: SwatchSpec {
                self_id: Some("Color/w1brand".to_string()),
                name: Some("Brand".to_string()),
                space: "RGB".to_string(),
                value: vec![200.0, 30.0, 90.0],
                model: Some("Process".to_string()),
                alternate_space: None,
                alternate_value: Vec::new(),
                tint: None,
                alpha: None,
            },
        })
        .expect("swatch");
    // New paragraph style.
    project
        .apply(Operation::CreateParagraphStyle {
            self_id: Some("ParagraphStyle/w1body".to_string()),
            name: Some("W1 Body".to_string()),
            based_on: None,
            restore_json: None,
        })
        .expect("style");
    // New rectangle filled with the new swatch.
    project
        .apply(Operation::InsertNode {
            parent: NodeId::Spread(spread_id.clone()),
            position: rect_pos,
            node: NodeSpec::Rectangle {
                self_id: "Rectangle/w1frame".to_string(),
                bounds: [12.0, 24.0, 96.0, 168.0],
                fill_color: Some("Color/w1brand".to_string()),
                stroke_color: None,
                stroke_weight: None,
                item_transform: None,
            },
            z_slot: None,
        })
        .expect("frame");

    let out = write_idml(project.document(), &original).expect("write");
    let re = Document::open(&out).expect("reparse");

    // The new swatch resolves.
    let swatch = re
        .palette
        .colors
        .get("Color/w1brand")
        .expect("new swatch present after round-trip");
    assert_eq!(swatch.value, vec![200.0, 30.0, 90.0]);
    // The new style is present.
    assert!(
        re.styles
            .paragraph_styles
            .contains_key("ParagraphStyle/w1body"),
        "new style present"
    );
    // The new frame is present AND its fill references the new swatch,
    // which now resolves (no dangling reference).
    let rect = re.spreads[spread_idx]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some("Rectangle/w1frame"))
        .expect("new frame present");
    assert_eq!(rect.fill_color.as_deref(), Some("Color/w1brand"));
    assert!(
        re.palette.resolve("Color/w1brand").is_some(),
        "frame fill resolves to a real swatch (appearance preserved)"
    );
}
