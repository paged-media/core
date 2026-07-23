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

use crate::{idml_parts_hash, write_idml, write_paged};

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
        "footnotes" => paged_gen::samples::footnotes::build(),
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
        let doc = idml_import::import_idml_doc(&original)
            .unwrap_or_else(|e| panic!("{name}: open: {e:?}"));
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
        let doc = idml_import::import_idml_doc(&original).unwrap();
        let out = write_idml(&doc, &original).unwrap();
        let re =
            idml_import::import_idml_doc(&out).unwrap_or_else(|e| panic!("{name}: reparse: {e:?}"));

        assert_eq!(doc.spreads.len(), re.spreads.len(), "{name}: spread count");
        assert_eq!(doc.stories.len(), re.stories.len(), "{name}: story count");

        let frames =
            |d: &Document| -> usize { d.spreads.iter().map(|s| s.spread.text_frames.len()).sum() };
        assert_eq!(frames(&doc), frames(&re), "{name}: text-frame count");

        // Story text content is preserved verbatim.
        for (a, b) in doc.stories.iter().zip(re.stories.iter()) {
            let text = |s: &idml_import::Story| -> String {
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
        let doc = idml_import::import_idml_doc(&original).unwrap();
        let out = write_idml(&doc, &original).unwrap();
        assert_eq!(
            original, out,
            "{name}: whole-package bytes diverged on unmutated round-trip"
        );
    }
}

// ---------------------------------------------------------------------
// 1b. `.paged` namespace: plugin-owned parts round-trip through a write.
// ---------------------------------------------------------------------

/// Re-zip `idml` with one extra entry appended (the `.paged` container's
/// plugin-namespaced parts ride alongside the IDML as ordinary ZIP entries
/// unreferenced by `designmap.xml`). Existing entries copy through raw so
/// `mimetype` stays first + stored and the package re-opens.
fn inject_entry(idml: &[u8], name: &str, body: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut src = zip::ZipArchive::new(Cursor::new(idml)).expect("zip");
    let out = Cursor::new(Vec::<u8>::new());
    let mut zip = zip::write::ZipWriter::new(out);
    for i in 0..src.len() {
        let entry = src.by_index_raw(i).expect("raw entry");
        zip.raw_copy_file(entry).expect("copy");
    }
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file(name, deflated).expect("start");
    zip.write_all(body).expect("write");
    zip.finish().expect("finish").into_inner()
}

/// The `.paged` guarantee: a plugin-owned part (`paged/<plugin>/<id>/…`),
/// unreferenced by `designmap.xml`, survives a MUTATED write byte-identically
/// and the package stays valid IDML. This is the foundation the container
/// format builds on — the carry-through writer preserves foreign entries.
#[test]
fn paged_namespace_part_survives_a_mutated_write() {
    let name = "geometry";
    let original = build_sample(name);
    let part_path = "paged/media.paged.sheet/obj1/spec.json";
    let part_body = br#"{"v":1,"data":{"formulas":["=A1+B1"]}}"#.to_vec();
    let injected = inject_entry(&original, part_path, &part_body);

    // The injected package still parses (the paged/ part is ignored by the
    // IDML reader — not referenced from designmap.xml).
    let doc = idml_import::import_idml_doc(&injected).expect("open .paged-shaped package");

    // Apply a REAL mutation so the writer takes the patch path for a spread
    // (not the trivial byte-identical short-circuit).
    let (rect_id, orig_fill) = doc
        .spreads
        .iter()
        .find_map(|s| {
            s.spread
                .rectangles
                .iter()
                .find(|r| r.self_id.is_some())
                .map(|r| (r.self_id.clone().unwrap(), r.fill_color.clone()))
        })
        .expect("a rectangle with a Self id");
    let new_fill = doc
        .palette
        .colors
        .keys()
        .find(|id| Some(id.as_str()) != orig_fill.as_deref())
        .cloned()
        .expect("a second swatch");
    let mut project = Project::new(doc);
    project
        .apply(Operation::SetProperty {
            node: NodeId::Rectangle(rect_id),
            path: PropertyPath::FrameFillColor,
            value: Value::ColorRef(Some(new_fill)),
        })
        .expect("apply fill");
    let out = write_idml(project.document(), &injected).expect("write");

    // The plugin part round-tripped untouched, and the package re-opens.
    let dst = entries(&out);
    let survived = dst
        .get(part_path)
        .expect("paged/ part dropped by the writer");
    assert_eq!(survived, &part_body, "paged/ part bytes diverged on write");
    idml_import::import_idml_doc(&out).expect("written .paged re-opens");
}

/// `write_paged` appends the model-held `paged/` parts + a `manifest.json`
/// carrying the Paged identity, the wire protocol, and the IDML-parts hash.
#[test]
fn write_paged_appends_new_parts_and_manifest() {
    let original = build_sample("geometry");
    let doc = idml_import::import_idml_doc(&original).unwrap();

    let mut parts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    parts.insert(
        "paged/media.paged.sheet/obj1/spec.json".to_string(),
        br#"{"v":1,"data":{}}"#.to_vec(),
    );
    parts.insert(
        "paged/media.paged.sheet/obj1/values.parquet".to_string(),
        vec![1u8, 2, 3, 4],
    );
    let out = write_paged(&doc, &original, &parts, 50).expect("write_paged");

    let dst = entries(&out);
    let m: serde_json::Value =
        serde_json::from_slice(dst.get("manifest.json").expect("manifest present"))
            .expect("manifest is valid json");
    assert_eq!(m["format"], serde_json::json!("paged-container"));
    assert_eq!(m["pagedProtocol"], serde_json::json!(50));
    assert!(m["idmlPartsHash"]
        .as_str()
        .expect("hash field")
        .starts_with("fnv1a64:"));
    assert_eq!(
        dst.get("paged/media.paged.sheet/obj1/spec.json").unwrap(),
        &br#"{"v":1,"data":{}}"#.to_vec()
    );
    assert_eq!(
        dst.get("paged/media.paged.sheet/obj1/values.parquet")
            .unwrap(),
        &vec![1u8, 2, 3, 4]
    );
    // mimetype is still first + the package re-opens as valid IDML.
    idml_import::import_idml_doc(&out).expect("written .paged re-opens");
}

/// The IDML-parts hash IGNORES `paged/` parts (so editing plugin data does
/// not trip the data-loss guard) but DOES change when an IDML part changes
/// (so an InDesign round-trip that rewrote the IDML is detectable).
#[test]
fn write_paged_idml_hash_excludes_paged_parts_but_tracks_idml_changes() {
    let original = build_sample("geometry");
    let doc = idml_import::import_idml_doc(&original).unwrap();

    let mut a: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    a.insert("paged/x/1/spec.json".to_string(), b"AAA".to_vec());
    let mut b: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    b.insert("paged/x/1/spec.json".to_string(), b"BBBBBBBBB".to_vec());
    let out_a = write_paged(&doc, &original, &a, 50).unwrap();
    let out_b = write_paged(
        &idml_import::import_idml_doc(&original).unwrap(),
        &original,
        &b,
        50,
    )
    .unwrap();
    assert_eq!(
        idml_parts_hash(&out_a).unwrap(),
        idml_parts_hash(&out_b).unwrap(),
        "IDML-parts hash must ignore differing paged/ parts"
    );

    // A real IDML mutation changes the hash.
    let (rect_id, orig_fill) = doc
        .spreads
        .iter()
        .find_map(|s| {
            s.spread
                .rectangles
                .iter()
                .find(|r| r.self_id.is_some())
                .map(|r| (r.self_id.clone().unwrap(), r.fill_color.clone()))
        })
        .expect("a rectangle");
    let new_fill = doc
        .palette
        .colors
        .keys()
        .find(|id| Some(id.as_str()) != orig_fill.as_deref())
        .cloned()
        .expect("a second swatch");
    let mut project = Project::new(idml_import::import_idml_doc(&original).unwrap());
    project
        .apply(Operation::SetProperty {
            node: NodeId::Rectangle(rect_id),
            path: PropertyPath::FrameFillColor,
            value: Value::ColorRef(Some(new_fill)),
        })
        .expect("apply");
    let mutated = write_idml(project.document(), &original).unwrap();
    assert_ne!(
        idml_parts_hash(&original).unwrap(),
        idml_parts_hash(&mutated).unwrap(),
        "IDML-parts hash must change when an IDML part changes"
    );
}

/// FORWARD COMPATIBILITY: a build that knows only its OWN plugin must, on
/// save, preserve every OTHER plugin's data parts (including third-party
/// plugins it has never heard of) AND the unknown fields + other-plugin
/// metadata inside `manifest.json`. Nothing third-party is dropped because
/// the current build cannot interpret it.
#[test]
fn write_paged_preserves_unknown_third_party_data_and_manifest_fields() {
    let original = build_sample("geometry");
    // A `.paged` as authored by OTHER builds: a manifest carrying a foreign
    // plugin's metadata + an unknown future field, plus two third-party
    // plugins' data parts (multi-tenant namespace).
    let foreign_manifest = br#"{"v":1,"format":"paged-container","pagedProtocol":48,"plugins":{"com.acme.widget":{"parts":["paged/com.acme.widget/9/data.bin"]}},"x-future-field":42}"#;
    let mut seeded = inject_entry(&original, "manifest.json", foreign_manifest);
    seeded = inject_entry(
        &seeded,
        "paged/com.acme.widget/9/data.bin",
        &[9u8, 8, 7, 6, 5],
    );
    seeded = inject_entry(
        &seeded,
        "paged/io.other.tool/3/notes.json",
        br#"{"hello":"world"}"#,
    );

    let doc = idml_import::import_idml_doc(&seeded).expect("open multi-plugin .paged");

    // THIS build knows only its own plugin: it writes one part, protocol 50.
    let mut mine: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    mine.insert(
        "paged/media.paged.sheet/1/spec.json".to_string(),
        br#"{"v":1,"data":{}}"#.to_vec(),
    );
    let out = write_paged(&doc, &seeded, &mine, 50).expect("write_paged");

    let dst = entries(&out);
    // (a) Both third-party plugins' data parts survived byte-identically.
    assert_eq!(
        dst.get("paged/com.acme.widget/9/data.bin").unwrap(),
        &vec![9u8, 8, 7, 6, 5]
    );
    assert_eq!(
        dst.get("paged/io.other.tool/3/notes.json").unwrap(),
        &br#"{"hello":"world"}"#.to_vec()
    );
    assert!(dst.contains_key("paged/media.paged.sheet/1/spec.json"));

    // (b) The manifest MERGED: this build's fields updated, the foreign
    //     plugin's metadata + the unknown future field preserved.
    let m: serde_json::Value =
        serde_json::from_slice(dst.get("manifest.json").unwrap()).expect("manifest json");
    assert_eq!(
        m["pagedProtocol"],
        serde_json::json!(50),
        "core field updated"
    );
    assert!(m["idmlPartsHash"].as_str().unwrap().starts_with("fnv1a64:"));
    assert_eq!(
        m["plugins"]["com.acme.widget"]["parts"][0],
        serde_json::json!("paged/com.acme.widget/9/data.bin"),
        "foreign plugin metadata preserved"
    );
    assert_eq!(
        m["x-future-field"],
        serde_json::json!(42),
        "unknown future field preserved"
    );
    idml_import::import_idml_doc(&out).expect("written multi-plugin .paged re-opens");
}

/// The manifest carries a `parts` INDEX (§7/§8): one entry per `paged/`
/// part — path, owning plugin (the namespace segment), byte length, and a
/// content hash. It mirrors the ACTUAL container contents (existing
/// carried-through parts + new ones), is sorted by path, and the hash
/// changes with the bytes (the staleness/integrity substrate).
#[test]
fn write_paged_records_a_parts_index_with_plugin_and_hash() {
    let original = build_sample("geometry");
    // Seed an existing third-party part so the index covers BOTH the
    // carried-through part and this build's new one.
    let seeded = inject_entry(&original, "paged/com.acme.widget/9/data.bin", &[1u8, 2, 3]);
    let doc = idml_import::import_idml_doc(&seeded).unwrap();

    let mut parts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    parts.insert(
        "paged/media.paged.sheet/obj1/spec.json".to_string(),
        br#"{"v":1,"data":{}}"#.to_vec(),
    );
    let out = write_paged(&doc, &seeded, &parts, 51).expect("write_paged");

    let dst = entries(&out);
    let m: serde_json::Value =
        serde_json::from_slice(dst.get("manifest.json").unwrap()).expect("manifest json");
    let index = m["parts"].as_array().expect("parts is an array");

    // Both parts are indexed, sorted by path (acme < media...).
    let paths: Vec<&str> = index.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        vec![
            "paged/com.acme.widget/9/data.bin",
            "paged/media.paged.sheet/obj1/spec.json",
        ],
        "index covers carried-through + new parts, sorted by path"
    );

    // Plugin attribution comes from the namespace segment; bytes + hash match.
    let acme = &index[0];
    assert_eq!(acme["plugin"], serde_json::json!("com.acme.widget"));
    assert_eq!(acme["bytes"], serde_json::json!(3));
    assert!(acme["hash"].as_str().unwrap().starts_with("fnv1a64:"));
    let sheet = &index[1];
    assert_eq!(sheet["plugin"], serde_json::json!("media.paged.sheet"));
    assert_eq!(
        sheet["bytes"],
        serde_json::json!(br#"{"v":1,"data":{}}"#.len())
    );

    // The hash is content-sensitive: re-writing the same part with different
    // bytes changes its index hash (the staleness signal).
    let mut parts2 = parts.clone();
    parts2.insert(
        "paged/media.paged.sheet/obj1/spec.json".to_string(),
        br#"{"v":1,"data":{"changed":true}}"#.to_vec(),
    );
    let out2 = write_paged(
        &idml_import::import_idml_doc(&seeded).unwrap(),
        &seeded,
        &parts2,
        51,
    )
    .expect("write_paged 2");
    let m2: serde_json::Value =
        serde_json::from_slice(entries(&out2).get("manifest.json").unwrap()).unwrap();
    let sheet2 = m2["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == serde_json::json!("paged/media.paged.sheet/obj1/spec.json"))
        .unwrap();
    assert_ne!(
        sheet["hash"], sheet2["hash"],
        "a part's index hash tracks its bytes"
    );
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
    let doc = idml_import::import_idml_doc(&original).unwrap();

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
    let re = idml_import::import_idml_doc(&out).expect("reparse");

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
    let doc = idml_import::import_idml_doc(&original).unwrap();

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
    let re = idml_import::import_idml_doc(&out).expect("reparse");

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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
    let re = idml_import::import_idml_doc(&out).expect("reparse");
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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
                caller: None,
                prev: None,
            },
        })
        .expect("set metadata");

    // Write → reparse → the label is there, value byte-equal (incl.
    // the XML-escaped quotes/ampersands inside the JSON envelope).
    let out = write_idml(project.document(), &original).expect("write");
    let re = idml_import::import_idml_doc(&out).expect("reparse");
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
                caller: None,
                prev: None,
            },
        })
        .expect("delete metadata");
    let out2 = write_idml(project2.document(), &out).expect("write 2");
    let re2 = idml_import::import_idml_doc(&out2).expect("reparse 2");
    assert!(
        re2.spreads
            .iter()
            .all(|s| !s.spread.labels.contains_key(&rect_id)),
        "label removed"
    );

    // Undo (exact restoration) → byte-identical write.
    let doc3 = idml_import::import_idml_doc(&original).unwrap();
    let mut project3 = Project::new(doc3);
    project3
        .apply(Operation::SetProperty {
            node: NodeId::Rectangle(rect_id.clone()),
            path: PropertyPath::PluginMetadata,
            value: Value::PluginMetadata {
                key: "x-paged:web".to_string(),
                value: Some(envelope.to_string()),
                caller: None,
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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
            caller: None,
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
    let mut doc = idml_import::import_idml_doc(&original).unwrap();
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
    let re = idml_import::import_idml_doc(&out).expect("reparse");

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
    let mut doc = idml_import::import_idml_doc(&original).unwrap();
    let (si, pi, ri) = first_multi_content_run(&doc).expect("a multi-Content run");

    doc.stories[si].story.paragraphs[pi].runs[ri].text = "line one\nline two".to_string();
    let out = write_idml(&doc, &original).expect("write");
    let re = idml_import::import_idml_doc(&out).expect("reparse");

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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
    let doc = idml_import::import_idml_doc(&original).unwrap();

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
    let re = idml_import::import_idml_doc(&out).expect("reparse");

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
    let doc = idml_import::import_idml_doc(&original).unwrap();

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
    let re = idml_import::import_idml_doc(&out).expect("reparse");

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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
    let re = idml_import::import_idml_doc(&out).expect("reparse");

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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
                parent_story: None,
            },
            z_slot: None,
        })
        .expect("insert text frame");

    let out = write_idml(project.document(), &original).expect("write");
    let re = idml_import::import_idml_doc(&out).expect("reparse");
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
    let doc = idml_import::import_idml_doc(&original).unwrap();

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
    let re = idml_import::import_idml_doc(&out).expect("reparse");
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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
    let doc2 = idml_import::import_idml_doc(&original).unwrap();
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
    let doc = idml_import::import_idml_doc(&original).unwrap();

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
    let re = idml_import::import_idml_doc(&out).expect("reparse");

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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
    let doc = idml_import::import_idml_doc(&original).unwrap();

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
    let re = idml_import::import_idml_doc(&out).expect("reparse");

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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
    let re = idml_import::import_idml_doc(&out).expect("reparse");
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
    let doc = idml_import::import_idml_doc(&original).unwrap();
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
    let re = idml_import::import_idml_doc(&out).expect("reparse");

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

// ---------------------------------------------------------------------
// 7. W1.15 — table-cell text write-back.
// ---------------------------------------------------------------------

/// Locate `(story_idx, para_idx, cell_idx, run path)` of the first table
/// cell carrying a run with text, so a cell-text edit exercises the
/// cell-content rewrite. Returns the cell's `Self` id + first run text.
fn first_table_cell_with_text(doc: &Document) -> Option<(usize, usize, usize, String, String)> {
    for (si, s) in doc.stories.iter().enumerate() {
        for (pi, p) in s.story.paragraphs.iter().enumerate() {
            if let Some(table) = &p.table {
                for (ci, cell) in table.cells.iter().enumerate() {
                    if let Some(id) = cell.self_id.clone() {
                        if let Some(run) = cell
                            .paragraphs
                            .iter()
                            .flat_map(|cp| cp.runs.iter())
                            .find(|r| !r.text.is_empty())
                        {
                            return Some((si, pi, ci, id, run.text.clone()));
                        }
                    }
                }
            }
        }
    }
    None
}

/// A table-cell text change (whatever the model holds for the cell
/// paragraph) writes back: save → re-parse shows the new cell text, and
/// untouched cells survive. Closes loss (a) — table-cell content
/// previously passed through verbatim. The edit is applied directly to
/// the model cell paragraph (the cell-text editing op is a parallel
/// lane; the writer serialises whatever the model already carries).
#[test]
fn table_cell_text_change_writes_back() {
    let original = build_sample("tables");
    let mut doc = idml_import::import_idml_doc(&original).unwrap();
    let (si, pi, ci, cell_id, old_text) =
        first_table_cell_with_text(&doc).expect("a table cell with text");

    // Edit the first run of the cell's first paragraph in the model.
    let new_text = "WroteBack".to_string();
    {
        let cell = doc.stories[si].story.paragraphs[pi]
            .table
            .as_mut()
            .unwrap()
            .cells
            .get_mut(ci)
            .unwrap();
        let run = cell
            .paragraphs
            .iter_mut()
            .flat_map(|cp| cp.runs.iter_mut())
            .find(|r| !r.text.is_empty())
            .unwrap();
        assert_eq!(run.text, old_text, "found the run we measured");
        run.text = new_text.clone();
    }

    let out = write_idml(&doc, &original).expect("write");
    assert_ne!(original, out, "a cell-text edit must change bytes");
    let re = idml_import::import_idml_doc(&out).expect("reparse");

    // The edited cell re-parses with the new text.
    let cell = re.stories[si].story.paragraphs[pi]
        .table
        .as_ref()
        .unwrap()
        .cells
        .iter()
        .find(|c| c.self_id.as_deref() == Some(cell_id.as_str()))
        .expect("edited cell present");
    let got: String = cell
        .paragraphs
        .iter()
        .flat_map(|cp| cp.runs.iter())
        .map(|r| r.text.clone())
        .collect();
    assert!(
        got.contains(&new_text),
        "cell text saved + re-parsed (got {got:?})"
    );

    // A sibling cell is untouched.
    let table = re.stories[si].story.paragraphs[pi].table.as_ref().unwrap();
    let other = table
        .cells
        .iter()
        .find(|c| c.self_id.as_deref() != Some(cell_id.as_str()))
        .expect("a sibling cell");
    let other_model = doc.stories[si].story.paragraphs[pi]
        .table
        .as_ref()
        .unwrap()
        .cells
        .iter()
        .find(|c| c.self_id == other.self_id)
        .unwrap();
    let other_text = |c: &idml_import::TableCell| -> String {
        c.paragraphs
            .iter()
            .flat_map(|cp| cp.runs.iter())
            .map(|r| r.text.clone())
            .collect()
    };
    assert_eq!(
        other_text(other),
        other_text(other_model),
        "sibling cell survived"
    );
}

/// A cell-text mutate-then-restore writes byte-identically — the cell
/// rewrite is value-driven (it only diverges when the model text differs
/// from the on-disk cell content).
#[test]
fn table_cell_unchanged_round_trips_byte_identical() {
    // The tables sample's stories all carry cell content; an unmutated
    // write must leave every Stories/* entry byte-identical (the cell
    // pass-through is now active but value-driven).
    let original = build_sample("tables");
    let doc = idml_import::import_idml_doc(&original).unwrap();
    let out = write_idml(&doc, &original).expect("write");
    let src = entries(&original);
    let dst = entries(&out);
    for (path, sb) in &src {
        if path.starts_with("Stories/") {
            assert_eq!(
                sb,
                dst.get(path).expect("entry present"),
                "{path}: table story not byte-identical unmutated"
            );
        }
    }
}

// ---------------------------------------------------------------------
// 8. W1.15 — group-member transforms.
// ---------------------------------------------------------------------

/// Find a group member rectangle: returns `(spread_idx, rect_id,
/// composed_item_transform)` for a rectangle that is a member of a group
/// (the `geometry-groups` sample's spread 0 wraps two rects in a group).
fn first_group_member_rect(doc: &Document) -> Option<(usize, String, [f32; 6])> {
    use idml_import::FrameRef;
    for (si, s) in doc.spreads.iter().enumerate() {
        for g in &s.spread.groups {
            for m in &g.members {
                if let FrameRef::Rectangle(ri) = *m {
                    if let Some(r) = s.spread.rectangles.get(ri) {
                        if let (Some(id), Some(tx)) = (r.self_id.clone(), r.item_transform) {
                            return Some((si, id, tx));
                        }
                    }
                }
            }
        }
    }
    None
}

/// A transform change on an item INSIDE a group writes back to the right
/// nested element: the writer recovers the on-disk member transform by
/// inverting the group accumulation, and a re-parse re-composes it to the
/// mutated (composed) model value. Closes loss (b).
#[test]
fn group_member_transform_writes_back() {
    let original = build_sample("geometry-groups");
    let mut doc = idml_import::import_idml_doc(&original).unwrap();
    let (spread_idx, rect_id, base) =
        first_group_member_rect(&doc).expect("a group-member rectangle");

    // The model `item_transform` is the COMPOSED group∘member matrix.
    // Translate the member by (50, -30) in composed (spread) space —
    // what a FrameTransform edit on a grouped item produces.
    let mut new_composed = base;
    new_composed[4] += 50.0;
    new_composed[5] -= 30.0;
    {
        let rect = doc.spreads[spread_idx]
            .spread
            .rectangles
            .iter_mut()
            .find(|r| r.self_id.as_deref() == Some(rect_id.as_str()))
            .unwrap();
        rect.item_transform = Some(new_composed);
    }

    let out = write_idml(&doc, &original).expect("write");
    assert_ne!(original, out, "a group-member transform edit must save");
    let re = idml_import::import_idml_doc(&out).expect("reparse");

    let rect = re.spreads[spread_idx]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some(rect_id.as_str()))
        .expect("group member present");
    let got = rect.item_transform.expect("transform present");
    // The re-parsed composed transform matches the mutated model value
    // (the writer's recovery + the parser's re-composition cancel).
    for (a, b) in got.iter().zip(new_composed.iter()) {
        assert!(
            (a - b).abs() < 1e-2,
            "re-composed member transform {got:?} != mutated {new_composed:?}"
        );
    }

    // A sibling group member is unchanged.
    let model_other = doc.spreads[spread_idx]
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() != Some(rect_id.as_str()) && r.item_transform.is_some());
    if let Some(other) = model_other {
        let re_other = re.spreads[spread_idx]
            .spread
            .rectangles
            .iter()
            .find(|r| r.self_id == other.self_id)
            .unwrap();
        let a = other.item_transform.unwrap();
        let b = re_other.item_transform.unwrap();
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x - y).abs() < 1e-2, "sibling member transform survived");
        }
    }
}

/// A group-member transform mutate-then-restore writes byte-identically:
/// the recovery is exact (recovered on-disk transform re-formats to the
/// source bytes), so an unchanged grouped item round-trips bit-for-bit.
#[test]
fn group_member_transform_unchanged_round_trips_byte_identical() {
    // The geometry-groups sample's spreads carry grouped items; an
    // unmutated write must leave every Spreads/* entry byte-identical
    // even though the group-member transform recovery is now active.
    let original = build_sample("geometry-groups");
    let doc = idml_import::import_idml_doc(&original).unwrap();
    let out = write_idml(&doc, &original).expect("write");
    let src = entries(&original);
    let dst = entries(&out);
    for (path, sb) in &src {
        if path.starts_with("Spreads/") {
            assert_eq!(
                sb,
                dst.get(path).expect("entry present"),
                "{path}: grouped spread not byte-identical unmutated"
            );
        }
    }
}

// ---------------------------------------------------------------------
// 9. W1.15 lane 5 / C-8 — a page inserted MID-DOCUMENT. The minted
//    spread is serialised as a new entry and its designmap ref lands
//    right after its host's, so the page order survives the round-trip
//    while every existing entry stays byte-identical.
// ---------------------------------------------------------------------

#[test]
fn inserted_page_mid_document_keeps_order_and_existing_entries() {
    let original = build_sample("geometry");
    let doc = idml_import::import_idml_doc(&original).unwrap();
    let before_spreads = doc.spreads.len();
    // Address the first page so InsertPage has a valid `after_page_id`
    // (the minted spread then lands at index 1 — between existing ones).
    let page_id = doc
        .spreads
        .iter()
        .find_map(|s| s.spread.pages.iter().find_map(|p| p.self_id.clone()))
        .expect("a page");

    let mut project = Project::new(doc);
    project
        .apply(Operation::InsertPage {
            after_page_id: Some(page_id),
            master_id: None,
            spread_self_id: None,
            page_self_id: None,
            restore_spread_json: None,
        })
        .expect("insert page");
    assert_eq!(
        project.document().spreads.len(),
        before_spreads + 1,
        "model gained a spread"
    );
    let minted_sid = project.document().spreads[1]
        .spread
        .self_id
        .clone()
        .expect("minted spread id");

    let out = write_idml(project.document(), &original).expect("write");
    let src = entries(&original);
    let dst = entries(&out);
    // Every existing entry except designmap.xml is byte-identical; the
    // minted spread is the only addition.
    for (path, bytes) in &src {
        if path == "designmap.xml" {
            continue;
        }
        assert_eq!(
            dst.get(path),
            Some(bytes),
            "existing entry {path} unchanged"
        );
    }
    let added: Vec<&String> = dst.keys().filter(|k| !src.contains_key(*k)).collect();
    assert_eq!(added.len(), 1, "one added entry: {added:?}");

    // Re-open: the inserted page survives AT ITS POSITION (index 1).
    let re = idml_import::import_idml_doc(&out).expect("output re-parses");
    assert_eq!(re.spreads.len(), before_spreads + 1, "spread count");
    assert_eq!(
        re.spreads[1].spread.self_id.as_deref(),
        Some(minted_sid.as_str()),
        "minted spread kept its mid-document position"
    );
}

// ---------------------------------------------------------------------
// W4.14 — footnote round-trip. A `<Footnote>` is a self-contained
// paragraph stream anchored mid-run; the parser keeps its body on
// `paragraph.footnotes[]`, NOT on the host story's `paragraphs`. The
// story rewriter must skip the footnote subtree so its inner ranges
// don't misalign the host story's positional cursors. Before the fix
// the rewrite dropped the host run's `<Content>` + the `<Footnote>`
// open tag and left a mismatched `</Footnote>`, so the written package
// re-parsed to ZERO pages.
// ---------------------------------------------------------------------

/// An unmutated `footnotes.idml` round-trips: the written package
/// re-parses with its page (and spread / story / frame) counts intact,
/// and — since nothing diverged from the model — every entry is
/// byte-identical to the source (the footnote subtree replays verbatim).
#[test]
fn footnote_story_round_trips_without_losing_pages() {
    let original = build_sample("footnotes");
    let doc = idml_import::import_idml_doc(&original).expect("open footnotes");

    // Sanity: the fixture has a page and a footnote-bearing host story.
    let pages_before: usize = doc.spreads.iter().map(|s| s.spread.pages.len()).sum();
    assert_eq!(pages_before, 1, "fixture has one page");
    let footnotes_before: usize = doc
        .stories
        .iter()
        .flat_map(|s| s.story.paragraphs.iter())
        .map(|p| p.footnotes.len())
        .sum();
    assert_eq!(
        footnotes_before, 3,
        "fixture host paragraph anchors 3 footnotes"
    );

    let out = write_idml(&doc, &original).expect("write must not crash");

    // The pages survive the round-trip (the regression: page_count → 0).
    let re = idml_import::import_idml_doc(&out).expect("written package re-parses");
    let pages_after: usize = re.spreads.iter().map(|s| s.spread.pages.len()).sum();
    assert_eq!(
        pages_after, pages_before,
        "pages preserved across round-trip"
    );
    assert_eq!(
        re.spreads.len(),
        doc.spreads.len(),
        "spread count preserved"
    );
    assert_eq!(re.stories.len(), doc.stories.len(), "story count preserved");
    let frames =
        |d: &Document| -> usize { d.spreads.iter().map(|s| s.spread.text_frames.len()).sum() };
    assert_eq!(frames(&re), frames(&doc), "frame count preserved");

    // The footnote bodies survive too — same count, same body text.
    let footnotes_after: usize = re
        .stories
        .iter()
        .flat_map(|s| s.story.paragraphs.iter())
        .map(|p| p.footnotes.len())
        .sum();
    assert_eq!(
        footnotes_after, footnotes_before,
        "footnote count preserved"
    );

    // Byte-identity on every untouched entry — an unmutated round-trip
    // is a pure pass-through (the footnote subtree replays verbatim).
    let src = entries(&original);
    let dst = entries(&out);
    assert_eq!(
        src.keys().collect::<Vec<_>>(),
        dst.keys().collect::<Vec<_>>(),
        "entry set unchanged"
    );
    for (path, src_bytes) in &src {
        let dst_bytes = dst.get(path).expect("entry present");
        assert_eq!(
            src_bytes, dst_bytes,
            "entry {path} not byte-identical on unmutated footnotes round-trip"
        );
    }
}

// ---------------------------------------------------------------------
// C-8 — new-entry emission: minted stories + InsertPage spreads.
// ---------------------------------------------------------------------

/// The K-1 live-validation gap: a text frame inserted via the
/// wire-shaped Operation MINTS its story (`Story/u<n>`, `src: ""`).
/// Text poured into that story must survive an IDML export: the writer
/// emits a full `Stories/Story_*.xml` part, references it from
/// designmap.xml, and the frame's `ParentStory` resolves on reopen.
#[test]
fn inserted_text_frame_with_minted_story_survives_export() {
    let original = build_sample("text");
    let doc = idml_import::import_idml_doc(&original).unwrap();
    let spread_id = doc.spreads[0].spread.self_id.clone().expect("spread id");
    let position = doc.spreads[0].spread.text_frames.len();
    let n_stories = doc.stories.len();

    let mut project = Project::new(doc);
    project
        .apply(Operation::InsertNode {
            parent: NodeId::Spread(spread_id),
            position,
            node: paged_mutate::NodeSpec::TextFrame {
                self_id: "u9001".to_string(),
                bounds: [100.0, 100.0, 200.0, 300.0],
                fill_color: None,
                stroke_color: None,
                stroke_weight: None,
                item_transform: None,
                parent_story: Some("Story/unew".to_string()),
            },
            z_slot: None,
        })
        .expect("insert text frame");

    // Pour text into the minted story. The canvas TextOp lane lives in
    // `paged-canvas` (not a dev-dep here), so mutate the parsed story
    // model directly — the same state the ops produce.
    {
        let doc = project.document_mut();
        let story = doc
            .stories
            .iter_mut()
            .find(|s| s.self_id == "Story/unew")
            .expect("minted story attached");
        assert_eq!(story.src, "", "minted story has no source entry");
        story.story.paragraphs[0].runs[0].text = "Poured into a fresh frame.".to_string();
    }

    let out = write_idml(project.document(), &original).expect("write");
    let re = idml_import::import_idml_doc(&out).expect("reparse");

    // The story survived — under the sanitized id the entry stem
    // re-derives (`Story/unew` → `Story_unew`).
    assert_eq!(re.stories.len(), n_stories + 1, "story count");
    let story = re
        .stories
        .iter()
        .find(|s| s.self_id == "Story_unew")
        .expect("minted story present after reopen");
    assert_eq!(story.src, "Stories/Story_Story_unew.xml");
    let text: String = story
        .story
        .paragraphs
        .iter()
        .flat_map(|p| p.runs.iter())
        .map(|r| r.text.clone())
        .collect();
    assert_eq!(text, "Poured into a fresh frame.");

    // The frame survived and its ParentStory resolves.
    let frame = re
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some("u9001"))
        .expect("inserted frame survived");
    assert_eq!(frame.parent_story.as_deref(), Some("Story_unew"));
    assert!(
        re.frame_for_story.contains_key("Story_unew"),
        "ParentStory resolves through frame_for_story"
    );

    // SourceArchive deltas: exactly one ADDED entry (the story part), and
    // only the host spread + designmap changed.
    let src_e = entries(&original);
    let dst_e = entries(&out);
    let added: Vec<&String> = dst_e.keys().filter(|k| !src_e.contains_key(*k)).collect();
    assert_eq!(added, ["Stories/Story_Story_unew.xml"], "added entries");
    let mut changed: Vec<&String> = src_e
        .iter()
        .filter(|(k, v)| dst_e.get(*k) != Some(v))
        .map(|(k, _)| k)
        .collect();
    changed.sort();
    assert_eq!(changed.len(), 2, "changed entries: {changed:?}");
    assert!(changed.iter().any(|k| *k == "designmap.xml"));
    assert!(changed.iter().any(|k| k.starts_with("Spreads/")));

    // The designmap gained exactly one idPkg:Story element.
    let dm_src = String::from_utf8(src_e["designmap.xml"].clone()).unwrap();
    let dm_dst = String::from_utf8(dst_e["designmap.xml"].clone()).unwrap();
    let count = |s: &str| s.matches("<idPkg:Story ").count();
    assert_eq!(count(&dm_dst), count(&dm_src) + 1, "one new idPkg:Story");
    assert!(
        dm_dst.contains(r#"<idPkg:Story src="Stories/Story_Story_unew.xml"/>"#),
        "new ref present: {dm_dst}"
    );

    // Idempotence: re-saving the reopened package (the story is now a
    // real, referenced entry) is a byte-identical round-trip.
    let out2 = write_idml(&re, &out).expect("write 2");
    assert_eq!(out, out2, "second save is byte-identical");
}

/// `InsertPage` mints a `ParsedSpread` whose `Spreads/Spread_*.xml` src
/// has no source entry — the same hole, spread-shaped. The writer emits
/// a full spread part (page + any items inserted onto it) and adds the
/// `<idPkg:Spread>` ref after its host so page order survives.
#[test]
fn inserted_page_spread_survives_export() {
    let original = build_sample("geometry");
    let doc = idml_import::import_idml_doc(&original).unwrap();
    let n_spreads = doc.spreads.len();
    let ref_bounds = doc.spreads.last().unwrap().spread.pages[0].bounds;

    let mut project = Project::new(doc);
    project
        .apply(Operation::InsertPage {
            after_page_id: None,
            master_id: None,
            spread_self_id: None,
            page_self_id: None,
            restore_spread_json: None,
        })
        .expect("insert page");
    let (sid, pid, minted_src) = {
        let parsed = project.document().spreads.last().expect("minted spread");
        (
            parsed.spread.self_id.clone().expect("spread id"),
            parsed.spread.pages[0].self_id.clone().expect("page id"),
            parsed.src.clone(),
        )
    };
    // Put a page item on the fresh page — the realistic insert-then-draw
    // flow; it must ride along inside the emitted part.
    project
        .apply(Operation::InsertNode {
            parent: NodeId::Spread(sid.clone()),
            position: 0,
            node: paged_mutate::NodeSpec::Rectangle {
                self_id: "u9100".to_string(),
                bounds: [10.0, 10.0, 60.0, 90.0],
                fill_color: Some("Color/Black".to_string()),
                stroke_color: None,
                stroke_weight: None,
                item_transform: None,
            },
            z_slot: None,
        })
        .expect("insert rectangle on new page");

    let out = write_idml(project.document(), &original).expect("write");
    let re = idml_import::import_idml_doc(&out).expect("reparse");

    assert_eq!(re.spreads.len(), n_spreads + 1, "spread count");
    // The minted spread is still LAST (designmap order = page order).
    let minted = re.spreads.last().unwrap();
    assert_eq!(minted.spread.self_id.as_deref(), Some(sid.as_str()));
    assert_eq!(minted.src, minted_src);
    let page = &minted.spread.pages[0];
    assert_eq!(page.self_id.as_deref(), Some(pid.as_str()));
    assert_eq!(
        (
            page.bounds.top,
            page.bounds.left,
            page.bounds.bottom,
            page.bounds.right
        ),
        (
            ref_bounds.top,
            ref_bounds.left,
            ref_bounds.bottom,
            ref_bounds.right
        ),
        "page size cloned from the host"
    );
    let rect = minted
        .spread
        .rectangles
        .iter()
        .find(|r| r.self_id.as_deref() == Some("u9100"))
        .expect("rectangle on the minted page survived");
    assert_eq!(rect.fill_color.as_deref(), Some("Color/Black"));

    // SourceArchive deltas: one added entry, only designmap changed.
    let src_e = entries(&original);
    let dst_e = entries(&out);
    let added: Vec<&String> = dst_e.keys().filter(|k| !src_e.contains_key(*k)).collect();
    assert_eq!(added, [&minted_src], "added entries");
    let changed: Vec<&String> = src_e
        .iter()
        .filter(|(k, v)| dst_e.get(*k) != Some(v))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(changed, ["designmap.xml"], "changed entries");
    let dm_src = String::from_utf8(src_e["designmap.xml"].clone()).unwrap();
    let dm_dst = String::from_utf8(dst_e["designmap.xml"].clone()).unwrap();
    let count = |s: &str| s.matches("<idPkg:Spread ").count();
    assert_eq!(count(&dm_dst), count(&dm_src) + 1, "one new idPkg:Spread");

    // Idempotence: a re-save of the reopened package round-trips
    // byte-identically (the minted spread is now patched in place).
    let out2 = write_idml(&re, &out).expect("write 2");
    assert_eq!(out, out2, "second save is byte-identical");
}

/// A page inserted and then REMOVED before saving must leave no trace:
/// the minted spread had no source entry, so the package round-trips
/// byte-identically (the insert-then-undo invariant, spread-shaped).
#[test]
fn inserted_then_removed_page_round_trips_byte_identical() {
    let original = build_sample("geometry");
    let doc = idml_import::import_idml_doc(&original).unwrap();

    let mut project = Project::new(doc);
    let applied = project
        .apply(Operation::InsertPage {
            after_page_id: None,
            master_id: None,
            spread_self_id: None,
            page_self_id: None,
            restore_spread_json: None,
        })
        .expect("insert page");
    project.apply(applied.inverse).expect("remove page");

    let out = write_idml(project.document(), &original).expect("write");
    assert_eq!(original, out, "insert+remove page is a byte-identical save");
}

// v43 — stroke line ends (LeftLineEnd / RightLineEnd) write-back.
// ---------------------------------------------------------------------

/// Minimal hand-rolled package with one `<GraphicLine>` that carries a
/// `LeftLineEnd` in the source XML (no generator sample emits lines).
fn line_end_idml() -> Vec<u8> {
    use std::io::Write;
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#,
        )
        .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<idPkg:Spread src="Spreads/Spread_s1.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<GraphicLine Self="gl1" GeometricBounds="100 100 300 300" ItemTransform="1 0 0 1 0 0" StrokeColor="Color/Black" StrokeWeight="2" LeftLineEnd="CircleSolidArrowHead"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

/// Set + clear line ends on an existing line (the patch lane: Set on a
/// key the source lacks rides the extras append; clearing removes the
/// attribute) and an inserted line keeps the arrowheads it was given
/// before save (the inserted-item lane). Unmutated, the package
/// round-trips byte-identically — the line-end patch is value-driven.
#[test]
fn graphic_line_line_ends_write_back() {
    let original = line_end_idml();

    // Unmutated: byte-identical (the CircleSolidArrowHead Set patch
    // re-emits the exact source token).
    let doc = idml_import::import_idml_doc(&original).expect("open");
    assert_eq!(
        doc.spreads[0].spread.graphic_lines[0].start_arrow,
        idml_import::ArrowheadType::CircleSolid
    );
    let out = write_idml(&doc, &original).expect("write unmutated");
    assert_eq!(entries(&original), entries(&out), "unmutated round-trip");

    // Mutate: add an end arrowhead (attr absent in source → extras
    // lane), clear the start arrowhead (→ Patch::Remove), and insert a
    // fresh line that gets arrowheads before save.
    let mut project = Project::new(idml_import::import_idml_doc(&original).unwrap());
    project
        .apply(Operation::SetProperty {
            node: NodeId::GraphicLine("gl1".to_string()),
            path: PropertyPath::FrameStrokeEndArrowhead,
            value: Value::Text("TriangleArrowHead".to_string()),
        })
        .expect("set end arrow");
    project
        .apply(Operation::SetProperty {
            node: NodeId::GraphicLine("gl1".to_string()),
            path: PropertyPath::FrameStrokeStartArrowhead,
            value: Value::Text(String::new()),
        })
        .expect("clear start arrow");
    project
        .apply(Operation::InsertNode {
            z_slot: None,
            parent: NodeId::Spread("s1".to_string()),
            position: 1,
            node: paged_mutate::NodeSpec::GraphicLine {
                self_id: "gl2".to_string(),
                bounds: [400.0, 100.0, 500.0, 300.0],
                anchors: Vec::new(),
                subpath_starts: Vec::new(),
                subpath_open: Vec::new(),
                stroke_color: Some("Color/Black".to_string()),
                stroke_weight: Some(1.0),
                item_transform: None,
            },
        })
        .expect("insert line");
    project
        .apply(Operation::SetProperty {
            node: NodeId::GraphicLine("gl2".to_string()),
            path: PropertyPath::FrameStrokeStartArrowhead,
            value: Value::Text("BarbedArrowHead".to_string()),
        })
        .expect("set inserted arrow");

    let out = write_idml(project.document(), &original).expect("write mutated");
    let re = idml_import::import_idml_doc(&out).expect("reparse");
    let lines = &re.spreads[0].spread.graphic_lines;
    let gl1 = lines
        .iter()
        .find(|l| l.self_id.as_deref() == Some("gl1"))
        .expect("gl1 present");
    assert_eq!(gl1.start_arrow, idml_import::ArrowheadType::None);
    assert_eq!(gl1.end_arrow, idml_import::ArrowheadType::Triangle);
    assert_eq!(
        gl1.stroke_weight,
        Some(2.0),
        "unrelated stroke attrs survive"
    );
    let gl2 = lines
        .iter()
        .find(|l| l.self_id.as_deref() == Some("gl2"))
        .expect("inserted gl2 present");
    assert_eq!(gl2.start_arrow, idml_import::ArrowheadType::Barbed);
    assert_eq!(gl2.end_arrow, idml_import::ArrowheadType::None);
}
