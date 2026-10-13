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

//! The InDesign round-trip lane. The engine's own round trip proves nothing
//! about InDesign: a spelling the writer and the reader share can be one
//! InDesign ignores. So each scenario here is exported by the engine,
//! opened in InDesign by `tools/indesign-export/lane.sh`, and InDesign's
//! view of it (`paged.indesign-view/1`, `tools/indesign-export/lib/
//! oracle.jsx`) is recorded under `tests/fixtures/indesign-lane/<scenario>/`
//! together with InDesign's own re-save of the document.
//!
//! [`recordings_agree_with_indesign`] needs no InDesign. For every
//! scenario it checks, on the fields the scenario declares:
//!
//! 1. that InDesign read the engine's export as the engine reads it — the
//!    engine's view of its export against InDesign's `opened` view. When
//!    the writer's output has changed since the recording (its digest no
//!    longer matches) the comparison is skipped and the scenario reported
//!    STALE; `PAGED_LANE_REQUIRE_FRESH=1` (set for a release) fails on it;
//! 2. that the engine reads InDesign's re-save as InDesign does — the
//!    engine's view of `resaved.idml` against InDesign's `reopened` view.
//!    Whatever InDesign dropped or rewrote on its way through shows here.

use std::path::{Path, PathBuf};

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};
use paged_mutate::Value;
use serde_json::{json, Value as J};

/// One lane scenario: a gen sample, edited through the canvas model, and
/// the view fields compared with InDesign's.
struct Scenario {
    name: &'static str,
    compare: &'static [&'static str],
    build: fn() -> Vec<u8>,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        // A-01: styles based on InDesign's `$ID/` roots, written back in
        // its spelling; the cascade InDesign resolves is the engine's.
        Scenario {
            name: "styles-cascade",
            compare: &["paragraphStyles", "stories", "pages"],
            build: || export(paged_gen::samples::styles_cascade::build(), |_, _| {}),
        },
        // A-02: a bleed edit reaches InDesign through Preferences.xml.
        Scenario {
            name: "bleed",
            compare: &["document"],
            build: || {
                export(paged_gen::samples::text::build(), |m, _| {
                    for (path, pt) in [
                        ("documentBleedTop", 9.0),
                        ("documentBleedBottom", 12.0),
                        ("documentBleedInsideOrLeft", 3.0),
                        ("documentBleedOutsideOrRight", 6.0),
                    ] {
                        set(m, "doc", path, Value::Length(Some(pt)));
                    }
                })
            },
        },
        // A-05: footnotes keep their text and their place in the story.
        Scenario {
            name: "footnotes",
            compare: &["stories", "pages"],
            build: || export(paged_gen::samples::footnotes::build(), |_, _| {}),
        },
        // ADR 131/132: a page item's name, visibility and lock.
        Scenario {
            name: "item-state",
            compare: &["pages"],
            build: || {
                export(paged_gen::samples::geometry::build(), |m, doc| {
                    let spread = &doc.spreads[0].spread;
                    let rect = spread.rectangles[0].self_id.clone().expect("a rectangle");
                    let text = spread.text_frames[0].self_id.clone().expect("a text frame");
                    let at = |kind: &str, id: &str| format!("{kind}:{id}");
                    set(
                        m,
                        &at("rectangle", &rect),
                        "itemName",
                        Value::Text("lane-box".into()),
                    );
                    set(
                        m,
                        &at("rectangle", &rect),
                        "elementVisible",
                        Value::Bool(false),
                    );
                    set(
                        m,
                        &at("frame", &text),
                        "itemName",
                        Value::Text("lane-text".into()),
                    );
                    set(m, &at("frame", &text), "elementLocked", Value::Bool(true));
                })
            },
        },
        // The general text sample: stories, styles and frames as written.
        Scenario {
            name: "text",
            compare: &["stories", "pages", "paragraphStyles"],
            build: || export(paged_gen::samples::text::build(), |_, _| {}),
        },
    ]
}

/// A sample written as IDML, opened in the canvas model, edited, and
/// exported by the engine's writer.
fn export(
    sample: paged_gen::Sample,
    edit: impl FnOnce(&mut CanvasModel, &paged_scene::Document),
) -> Vec<u8> {
    let bytes = paged_gen::write_idml(&sample).expect("write sample");
    let doc = idml_import::import_idml_doc(&bytes).expect("import sample");
    let mut model = CanvasModel::load("lane", &bytes, CanvasOptions::default()).expect("load");
    edit(&mut model, &doc);
    model.export_idml().expect("export")
}

fn set(model: &mut CanvasModel, address: &str, path: &str, value: Value) {
    let mutation = Mutation::Set {
        address: paged_wire::Address::parse(address).expect("address"),
        path: path.to_string(),
        value,
    };
    model
        .apply_mutation(&mutation)
        .unwrap_or_else(|e| panic!("{address} {path}: {e:?}"));
}

/// FNV-1a, 64 bit: the digest a recording keeps of the export it was made
/// from.
fn digest(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn recordings() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/indesign-lane")
}

// ---- the engine's view: the same schema as oracle.jsx --------------------

/// Text as the view keeps it: no control or private-use characters (the
/// engine's markers, InDesign's special characters), whitespace collapsed.
fn plain(s: &str) -> String {
    let kept: String = s
        .chars()
        .filter_map(|c| match c {
            '\r' | '\n' | '\t' | '\u{2028}' | '\u{2029}' | '\u{a0}' => Some(' '),
            '\u{feff}' | '\u{fffc}' | '\u{e000}'..='\u{f8ff}' => None,
            c if (c as u32) < 0x20 => None,
            c => Some(c),
        })
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A style's name as InDesign's DOM reports it.
fn style_name(id_or_name: &str) -> String {
    let tail = id_or_name
        .split_once('/')
        .filter(|(kind, _)| kind.ends_with("Style"))
        .map_or(id_or_name, |(_, t)| t);
    match tail {
        "$ID/NormalParagraphStyle" => "[Basic Paragraph]".into(),
        "$ID/[No paragraph style]" => "[No Paragraph Style]".into(),
        "$ID/[No character style]" => "[None]".into(),
        t => t.strip_prefix("$ID/").unwrap_or(t).to_string(),
    }
}

/// A family's styles. A style with no `BasedOn` is based on the family's
/// root, which InDesign names.
fn styles<'a, T: 'a>(
    defs: impl Iterator<Item = (&'a String, &'a T)>,
    root: &str,
    name_of: impl Fn(&T) -> (Option<&String>, Option<&String>),
) -> J {
    J::Array(
        defs.map(|(id, def)| {
            let (name, based_on) = name_of(def);
            json!({
                "name": style_name(name.unwrap_or(id)),
                "basedOn": based_on.map_or_else(|| root.to_string(), |b| style_name(b)),
            })
        })
        .collect(),
    )
}

fn engine_view(bytes: &[u8]) -> J {
    let doc = idml_import::import_idml_doc(bytes).expect("import");
    let dp = &doc.designmap.document_preference;
    let pages: Vec<&paged_model::Page> = doc.spreads.iter().flat_map(|s| &s.spread.pages).collect();
    let first = pages.first().map(|p| p.bounds);
    let mut page_items: Vec<Vec<J>> = vec![Vec::new(); pages.len()];
    let mut base = 0;
    for parsed in &doc.spreads {
        let spread = &parsed.spread;
        for frame in &spread.frames_in_order {
            if let Some((page, item)) = item_view(spread, frame) {
                if let Some(slot) = page_items.get_mut(base + page) {
                    slot.push(item);
                }
            }
        }
        base += spread.pages.len();
    }
    let mut swatches: Vec<String> = doc
        .palette
        .colors
        .values()
        .filter_map(|c| c.name.clone())
        .chain(doc.palette.swatches.values().filter_map(|s| s.name.clone()))
        .chain(
            doc.palette
                .gradients
                .values()
                .filter_map(|g| g.name.clone()),
        )
        .collect();
    swatches.sort();
    swatches.dedup();
    let stories: Vec<J> = doc
        .stories
        .iter()
        .map(|st| {
            // A model paragraph holds InDesign's paragraphs between `\n`
            // marks (one `<ParagraphStyleRange>` with `<Br/>`s, as
            // InDesign writes paragraphs that share their formatting).
            let paragraphs: Vec<J> = st
                .story
                .paragraphs
                .iter()
                .flat_map(|p| {
                    let style = p.paragraph_style.as_deref().map(|s| {
                        doc.styles
                            .paragraph_styles
                            .get(s)
                            .and_then(|d| d.name.clone())
                            .map_or_else(|| style_name(s), |n| style_name(&n))
                    });
                    let text: String = p.runs.iter().map(|r| r.text.as_str()).collect();
                    text.split('\n')
                        .map(|t| json!({ "style": style, "text": plain(t) }))
                        .collect::<Vec<_>>()
                })
                .collect();
            let footnotes: Vec<J> = st
                .story
                .paragraphs
                .iter()
                .flat_map(|p| &p.footnotes)
                .map(|f| {
                    let text: Vec<String> = f
                        .paragraphs
                        .iter()
                        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect())
                        .collect();
                    J::String(plain(&text.join(" ")))
                })
                .collect();
            let text: Vec<String> = paragraphs
                .iter()
                .map(|p| p["text"].as_str().unwrap_or("").to_string())
                .collect();
            json!({
                "text": plain(&text.join(" ")),
                "paragraphs": paragraphs,
                "footnotes": footnotes,
            })
        })
        .collect();
    json!({
        "schema": "paged.indesign-view/1",
        "document": {
            "pageWidth": first.map(|b| b.right - b.left),
            "pageHeight": first.map(|b| b.bottom - b.top),
            "bleed": [dp.bleed_top, dp.bleed_bottom, dp.bleed_inside_or_left, dp.bleed_outside_or_right],
            "pages": pages.len(),
        },
        "layers": doc.designmap.layers.iter().map(|l| json!({
            "name": l.name.clone().unwrap_or_default(),
            "visible": l.visible,
            "locked": l.locked,
        })).collect::<Vec<_>>(),
        "swatches": swatches,
        "paragraphStyles": styles(doc.styles.paragraph_styles.iter(), "[No Paragraph Style]", |d| (d.name.as_ref(), d.based_on.as_ref())),
        "characterStyles": styles(doc.styles.character_styles.iter(), "[None]", |d| (d.name.as_ref(), d.based_on.as_ref())),
        "pages": page_items.into_iter().map(|items| json!({ "items": items })).collect::<Vec<_>>(),
        "stories": stories,
    })
}

/// One top-level page item: the page it sits on (by its centre, as
/// `paged_scene::page_index_for_bounds` assigns frames) and its view, its
/// bounds measured from that page's top-left corner. A group is reported
/// by kind only: its members nest their transforms.
fn item_view(spread: &paged_model::Spread, frame: &paged_model::FrameRef) -> Option<(usize, J)> {
    use paged_model::FrameRef as F;
    let (kind, id, bounds, xf, visible, locked) = match *frame {
        F::TextFrame(i) => {
            let f = spread.text_frames.get(i)?;
            (
                "TextFrame",
                &f.self_id,
                f.bounds,
                f.item_transform,
                f.visible,
                f.locked,
            )
        }
        F::Rectangle(i) => {
            let f = spread.rectangles.get(i)?;
            (
                "Rectangle",
                &f.self_id,
                f.bounds,
                f.item_transform,
                f.visible,
                f.locked,
            )
        }
        F::Oval(i) => {
            let f = spread.ovals.get(i)?;
            (
                "Oval",
                &f.self_id,
                f.bounds,
                f.item_transform,
                f.visible,
                f.locked,
            )
        }
        F::Polygon(i) => {
            let f = spread.polygons.get(i)?;
            (
                "Polygon",
                &f.self_id,
                f.bounds,
                f.item_transform,
                f.visible,
                f.locked,
            )
        }
        F::GraphicLine(i) => {
            let f = spread.graphic_lines.get(i)?;
            (
                "GraphicLine",
                &f.self_id,
                f.bounds,
                f.item_transform,
                f.visible,
                f.locked,
            )
        }
        F::Group(i) => {
            let g = spread.groups.get(i)?;
            let page = g
                .members
                .first()
                .and_then(|m| item_view(spread, m))
                .map_or(0, |(p, _)| p);
            return Some((page, json!({ "kind": "Group" })));
        }
    };
    let page = paged_scene::page_index_for_bounds(&spread.pages, bounds, xf).unwrap_or(0);
    let (ox, oy) = spread.pages.get(page).map_or((0.0, 0.0), |p| {
        let (tx, ty) = p.item_transform.map_or((0.0, 0.0), |t| (t[4], t[5]));
        (p.bounds.left + tx, p.bounds.top + ty)
    });
    let corners = [
        (bounds.left, bounds.top),
        (bounds.right, bounds.top),
        (bounds.right, bounds.bottom),
        (bounds.left, bounds.bottom),
    ]
    .map(|(x, y)| match xf {
        Some([a, b, c, d, tx, ty]) => (a * x + c * y + tx, b * x + d * y + ty),
        None => (x, y),
    });
    let fold = |f: fn(f32, f32) -> f32, init: f32, axis: fn(&(f32, f32)) -> f32| {
        corners.iter().map(axis).fold(init, f)
    };
    let left = fold(f32::min, f32::MAX, |c| c.0) - ox;
    let right = fold(f32::max, f32::MIN, |c| c.0) - ox;
    let top = fold(f32::min, f32::MAX, |c| c.1) - oy;
    let bottom = fold(f32::max, f32::MIN, |c| c.1) - oy;
    let name = id
        .as_deref()
        .and_then(|id| spread.item_names.get(id))
        .map(|n| n.strip_prefix("$ID/").unwrap_or(n).to_string())
        .unwrap_or_default();
    Some((
        page,
        json!({
            "kind": kind,
            "name": name,
            "visible": visible,
            "locked": locked,
            "bounds": [top, left, bottom, right],
        }),
    ))
}

// ---- the comparison -------------------------------------------------------

fn num(v: &J) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

fn close(a: &J, b: &J, tol: f64) -> bool {
    (num(a) - num(b)).abs() <= tol
}

/// Where the engine's view of a package and InDesign's disagree on one
/// field group; empty when they agree.
fn differences(field: &str, ours: &J, theirs: &J) -> Vec<String> {
    let mut out = Vec::new();
    match field {
        "document" => {
            let (o, t) = (&ours["document"], &theirs["document"]);
            if o["pages"] != t["pages"] {
                out.push(format!(
                    "pages: ours {} InDesign {}",
                    o["pages"], t["pages"]
                ));
            }
            for k in ["pageWidth", "pageHeight"] {
                if !close(&o[k], &t[k], 0.01) {
                    out.push(format!("{k}: ours {} InDesign {}", o[k], t[k]));
                }
            }
            for i in 0..4 {
                if !close(&o["bleed"][i], &t["bleed"][i], 0.01) {
                    out.push(format!(
                        "bleed: ours {} InDesign {}",
                        o["bleed"], t["bleed"]
                    ));
                    break;
                }
            }
        }
        "paragraphStyles" | "characterStyles" => {
            let ours_by_name = |n: &str| {
                ours[field]
                    .as_array()
                    .and_then(|a| a.iter().find(|s| s["name"] == n))
                    .cloned()
            };
            for s in theirs[field].as_array().into_iter().flatten() {
                let n = s["name"].as_str().unwrap_or("");
                if n.starts_with('[') {
                    continue;
                }
                match ours_by_name(n) {
                    None => out.push(format!("{field}: {n:?} is InDesign's only")),
                    Some(o) if o["basedOn"] != s["basedOn"] => out.push(format!(
                        "{field}: {n:?} based on {} (ours) / {} (InDesign)",
                        o["basedOn"], s["basedOn"]
                    )),
                    _ => {}
                }
            }
        }
        "pages" => {
            let sorted = |v: &J| -> Vec<J> {
                let mut items: Vec<J> = v["items"].as_array().cloned().unwrap_or_default();
                items.sort_by(|a, b| {
                    let key = |i: &J| (num(&i["bounds"][0]).round(), num(&i["bounds"][1]).round());
                    key(a)
                        .partial_cmp(&key(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                items
            };
            let (op, tp) = (ours["pages"].as_array(), theirs["pages"].as_array());
            for (p, (o, t)) in op
                .into_iter()
                .flatten()
                .zip(tp.into_iter().flatten())
                .enumerate()
            {
                let (oi, ti) = (sorted(o), sorted(t));
                if oi.len() != ti.len() {
                    out.push(format!(
                        "page {p}: {} items (ours) / {} (InDesign)",
                        oi.len(),
                        ti.len()
                    ));
                    continue;
                }
                for (a, b) in oi.iter().zip(&ti) {
                    if a["kind"] != b["kind"] {
                        out.push(format!(
                            "page {p}: {} (ours) / {} (InDesign)",
                            a["kind"], b["kind"]
                        ));
                        continue;
                    }
                    if a["kind"] == "Group" {
                        continue;
                    }
                    for k in ["name", "visible", "locked"] {
                        if a[k] != b[k] {
                            out.push(format!(
                                "page {p} {} {k}: ours {} InDesign {}",
                                a["kind"], a[k], b[k]
                            ));
                        }
                    }
                    if (0..4).any(|i| !close(&a["bounds"][i], &b["bounds"][i], 0.05)) {
                        out.push(format!(
                            "page {p} {} bounds: ours {} InDesign {}",
                            a["kind"], a["bounds"], b["bounds"]
                        ));
                    }
                }
            }
        }
        "stories" => {
            let texts = |v: &J| -> Vec<J> {
                let mut s: Vec<J> = v["stories"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|s| !s["text"].as_str().unwrap_or("").is_empty())
                    .cloned()
                    .collect();
                s.sort_by(|a, b| a["text"].as_str().cmp(&b["text"].as_str()));
                s
            };
            let (os, ts) = (texts(ours), texts(theirs));
            if os.len() != ts.len() {
                out.push(format!(
                    "{} stories with text (ours) / {} (InDesign)",
                    os.len(),
                    ts.len()
                ));
            }
            for (o, t) in os.iter().zip(&ts) {
                if o["text"] != t["text"] {
                    out.push(format!(
                        "story text: ours {} InDesign {}",
                        o["text"], t["text"]
                    ));
                    continue;
                }
                let styles = |s: &J| -> Vec<J> {
                    s["paragraphs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|p| !p["text"].as_str().unwrap_or("").is_empty())
                        .map(|p| p["style"].clone())
                        .collect()
                };
                if styles(o) != styles(t) {
                    out.push(format!(
                        "paragraph styles of {}: ours {:?} InDesign {:?}",
                        o["text"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(40)
                            .collect::<String>(),
                        styles(o),
                        styles(t)
                    ));
                }
                if o["footnotes"] != t["footnotes"] {
                    out.push(format!(
                        "footnotes: ours {} InDesign {}",
                        o["footnotes"], t["footnotes"]
                    ));
                }
            }
        }
        other => out.push(format!("unknown field group {other}")),
    }
    out
}

// ---- the tests --------------------------------------------------------------

/// Writes each scenario's export for `lane.sh` (needs `PAGED_INDESIGN_LANE`;
/// `PAGED_LANE_SCENARIOS` picks scenarios by name, comma-separated).
#[test]
#[ignore = "writes the lane's inputs for tools/indesign-export/lane.sh"]
fn write_lane_inputs() {
    let dir = PathBuf::from(std::env::var("PAGED_INDESIGN_LANE").expect("PAGED_INDESIGN_LANE"));
    let wanted: Vec<String> = std::env::var("PAGED_LANE_SCENARIOS")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for s in scenarios() {
        if !wanted.is_empty() && !wanted.iter().any(|w| w == s.name) {
            continue;
        }
        let out = dir.join(s.name);
        std::fs::create_dir_all(&out).unwrap();
        let bytes = (s.build)();
        std::fs::write(out.join("out.idml"), &bytes).unwrap();
        std::fs::write(out.join("input.digest"), digest(&bytes)).unwrap();
    }
}

#[test]
fn recordings_agree_with_indesign() {
    let require_fresh = std::env::var_os("PAGED_LANE_REQUIRE_FRESH").is_some();
    let mut failures = Vec::new();
    for s in scenarios() {
        let dir = recordings().join(s.name);
        let Ok(view) = std::fs::read_to_string(dir.join("indesign-view.json")) else {
            failures.push(format!(
                "{}: no recording (bash tools/indesign-export/lane.sh {})",
                s.name, s.name
            ));
            continue;
        };
        let view: J = serde_json::from_str(&view).expect("indesign-view.json");
        let bytes = (s.build)();
        let recorded = std::fs::read_to_string(dir.join("input.digest")).unwrap_or_default();
        if recorded.trim() == digest(&bytes) {
            let ours = engine_view(&bytes);
            for f in s.compare {
                for d in differences(f, &ours, &view["opened"]) {
                    failures.push(format!("{} (InDesign opened our export): {d}", s.name));
                }
            }
        } else {
            eprintln!(
                "indesign-lane: {} is STALE (the writer's output changed since the recording; \
                 bash tools/indesign-export/lane.sh {})",
                s.name, s.name
            );
            if require_fresh {
                failures.push(format!("{}: stale recording", s.name));
            }
        }
        let resaved = std::fs::read(dir.join("resaved.idml")).expect("resaved.idml");
        let ours = engine_view(&resaved);
        for f in s.compare {
            for d in differences(f, &ours, &view["reopened"]) {
                failures.push(format!(
                    "{} (our import of InDesign's re-save): {d}",
                    s.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn the_view_keeps_text_without_markers() {
    assert_eq!(plain("a\u{e018}b\tc\u{2029}\u{fffc} d "), "ab c d");
    assert_eq!(
        style_name("ParagraphStyle/$ID/NormalParagraphStyle"),
        "[Basic Paragraph]"
    );
    assert_eq!(style_name("ParagraphStyle/Body Copy"), "Body Copy");
}
