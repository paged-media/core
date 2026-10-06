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

//! ADR 132 — every non-element row of the catalog's `propertySchemas`
//! is SET through `Mutation::Set`, READ back through `get_property`,
//! and UNDONE, on a real fixture.
//!
//! The rows are the catalog's own (`property_schemas()`), not a list kept
//! here, so a virtual path added to the table is probed on the next run —
//! or this test names it as unprobeable. The element paths have their own
//! gate (`paged-introspect/tests/catalog_apply_parity.rs`).
//!
//! What the three steps prove: the row resolves to an op that exists and
//! accepts the schema's wire form (set), the read twin reads what the op
//! wrote (get), and the resolution kept the op's inverse (undo) — which
//! is the "no second apply path" claim made checkable.

use paged_canvas::{channel::Mutation, CanvasModel, CanvasOptions};
use paged_introspect::schema::{DefaultValue, ValueType, Wire};
use paged_introspect::virtual_paths::{property_schemas, SchemaRow};
use paged_mutate::{GradientStopSpec, Value};
use paged_wire::{ElementId, PageId, StyleFamily, VirtualTarget};

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/generated")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Rows whose op is application state the engine deliberately keeps out
/// of the undo log (colour management, proofing, inks, document defaults
/// outside a batch). Set and read back still hold for them.
const NOT_UNDOABLE_OPS: &[&str] = &[
    "setDocumentDefaults",
    "setColorSettings",
    "setProofSetup",
    "setInkSetting",
    "setUseStandardLabForSpots",
];

/// Every object kind a row can live on, minted or found in one document.
struct Targets {
    doc: VirtualTarget,
    page: String,
    layer: String,
    master: String,
    story: String,
    field: (String, u32),
    swatch: String,
    spot: String,
    gradient: String,
    color_group: String,
    numbering_list: String,
    styles: Vec<(StyleFamily, String)>,
    section: String,
    guide: String,
    condition: String,
    condition_set: String,
    text_variable: String,
    table: (String, String),
    frame: ElementId,
    group: ElementId,
}

fn apply(m: &mut CanvasModel, mutation: Mutation) {
    m.apply_mutation(&mutation)
        .unwrap_or_else(|e| panic!("setup {}: {e:?}", mutation.discriminant()));
}

fn build() -> (CanvasModel, Targets) {
    let mut m = CanvasModel::load("vp", &fixture("annual-base.idml"), CanvasOptions::default())
        .expect("load annual-base");
    let scene = m.scene();
    let page = scene
        .spreads
        .iter()
        .find_map(|s| s.spread.pages.first().and_then(|p| p.self_id.clone()))
        .expect("a page");
    let spread = scene.spreads[0].spread.self_id.clone().expect("spread id");
    let layer = scene.designmap.layers[0].self_id.clone();
    let master = scene
        .master_spreads
        .keys()
        .next()
        .cloned()
        .expect("a master");
    let swatch = scene
        .palette
        .colors
        .keys()
        .find(|k| k.starts_with("Color/") && !k.contains("Registration"))
        .cloned()
        .expect("a colour");
    let gradient = scene
        .palette
        .gradients
        .keys()
        .next()
        .cloned()
        .expect("a gradient");
    let color_group = scene
        .palette
        .color_groups
        .keys()
        .next()
        .cloned()
        .expect("a colour group");
    let condition = scene
        .styles
        .conditions
        .keys()
        .next()
        .cloned()
        .expect("condition");
    let condition_set = scene
        .styles
        .condition_sets
        .keys()
        .next()
        .cloned()
        .expect("condition set");
    let text_variable = scene
        .designmap
        .text_variables
        .iter()
        .find(|t| t.variable_type.as_deref() == Some("CustomTextType"))
        .or(scene.designmap.text_variables.first())
        .map(|t| t.self_id.clone())
        .expect("a text variable");
    let pick = |ids: Vec<&String>| ids.into_iter().find(|k| !k.contains("$ID")).cloned();
    let styles = vec![
        (
            StyleFamily::Paragraph,
            pick(scene.styles.paragraph_styles.keys().collect()).expect("pstyle"),
        ),
        (
            StyleFamily::Character,
            pick(scene.styles.character_styles.keys().collect()).expect("cstyle"),
        ),
        (
            StyleFamily::Object,
            pick(scene.styles.object_styles.keys().collect()).expect("ostyle"),
        ),
        (
            StyleFamily::Cell,
            pick(scene.styles.cell_styles.keys().collect()).expect("cellstyle"),
        ),
        (
            StyleFamily::Table,
            pick(scene.styles.table_styles.keys().collect()).expect("tstyle"),
        ),
    ];
    let group = scene
        .spreads
        .iter()
        .find_map(|s| s.spread.groups.first().and_then(|g| g.self_id.clone()));

    // Mint what the fixture lacks, through the engine's own doors.
    apply(
        &mut m,
        Mutation::InsertTextFrame {
            page_id: PageId(page.clone()),
            bounds: (10.0, 10.0, 200.0, 200.0),
        },
    );
    let story = m
        .scene()
        .stories
        .last()
        .expect("minted story")
        .self_id
        .clone();
    let outcome = m
        .apply_mutation(&Mutation::InsertFrame {
            page_id: PageId(page.clone()),
            bounds: (220.0, 10.0, 300.0, 120.0),
        })
        .expect("graphic frame");
    let frame = outcome.created_id.expect("the frame's id");
    apply(
        &mut m,
        Mutation::InsertText {
            story_id: story.clone(),
            offset: 0,
            text: "Probe text".into(),
            cell: None,
        },
    );
    apply(
        &mut m,
        Mutation::InsertField {
            story_id: story.clone(),
            offset: 0,
            content_offset: None,
            field: paged_mutate::operation::FieldKind::Placeholder {
                plugin: "media.paged.probe".into(),
                key: "k".into(),
                value: Some("v".into()),
            },
        },
    );
    apply(
        &mut m,
        Mutation::InsertTable {
            story_id: story.clone(),
            rows: 2,
            cols: 2,
            header_rows: 0,
            footer_rows: 0,
            column_widths: vec![],
            row_heights: vec![],
        },
    );
    let table_id = m
        .scene()
        .stories
        .iter()
        .find(|s| s.self_id == story)
        .and_then(|s| s.story.paragraphs.iter().find_map(|p| p.table.as_ref()))
        .and_then(|t| t.self_id.clone())
        .expect("minted table");
    apply(
        &mut m,
        Mutation::CreateNumberingList {
            spec: paged_mutate::NumberingListSpec {
                self_id: Some("NumberingList/probe".into()),
                name: Some("Probe".into()),
                continue_across_stories: None,
                continue_across_documents: None,
            },
        },
    );
    apply(
        &mut m,
        Mutation::InsertSection {
            at_page: PageId(page.clone()),
            prefix: None,
            numbering_style: None,
            start_at: None,
        },
    );
    let section = m
        .scene()
        .designmap
        .sections
        .iter()
        .find(|s| s.page_start.as_deref() == Some(page.as_str()))
        .map(|s| s.self_id.clone())
        .expect("minted section");
    apply(
        &mut m,
        Mutation::InsertGuide {
            spread_id: spread.clone(),
            orientation: paged_mutate::operation::GuideOrientationSpec::Vertical,
            position: 50.0,
            page_index: 0,
        },
    );
    let gi = m.scene().spreads[0].spread.guides.len() - 1;
    let guide = format!("Guide/{spread}/{gi}");
    // A spot ink for the ink-manager rows, a registered profile for the
    // colour-management rows, and a group for `groupTransform`.
    apply(
        &mut m,
        Mutation::CreateSwatch {
            spec: paged_mutate::SwatchSpec {
                self_id: Some("Color/ProbeSpot".into()),
                name: Some("Probe spot".into()),
                space: "CMYK".into(),
                value: vec![0.0, 50.0, 100.0, 0.0],
                model: Some("Spot".into()),
                alternate_space: None,
                alternate_value: vec![],
                tint: None,
                alpha: None,
            },
        },
    );
    let icc = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus/profiles/default_cmyk.icc"),
    )
    .expect("corpus/profiles/default_cmyk.icc (copy it from a full checkout)");
    m.register_color_profile(PROFILE.into(), icc);
    apply(
        &mut m,
        Mutation::SetProofSetup {
            profile_name: Some(PROFILE.into()),
            simulate_paper_white: false,
            intent: None,
        },
    );
    let second = m
        .apply_mutation(&Mutation::InsertFrame {
            page_id: PageId(page.clone()),
            bounds: (320.0, 10.0, 380.0, 120.0),
        })
        .expect("second frame")
        .created_id
        .expect("its id");
    let grouped = m
        .apply_mutation(&Mutation::CreateGroup {
            member_ids: vec![frame.clone(), second],
        })
        .expect("group");
    let group = grouped
        .created_id
        .or(group.map(ElementId::Group))
        .expect("a group");
    let frame = m
        .apply_mutation(&Mutation::InsertFrame {
            page_id: PageId(page.clone()),
            bounds: (400.0, 10.0, 480.0, 120.0),
        })
        .expect("ungrouped frame")
        .created_id
        .expect("its id");
    (
        m,
        Targets {
            doc: VirtualTarget::Doc,
            page,
            layer,
            master,
            story: story.clone(),
            field: (story.clone(), 0),
            swatch,
            spot: "Color/ProbeSpot".into(),
            gradient,
            color_group,
            numbering_list: "NumberingList/probe".into(),
            styles,
            section,
            guide,
            condition,
            condition_set,
            text_variable,
            table: (story, table_id),
            frame,
            group,
        },
    )
}

fn targets_for(row: &SchemaRow, t: &Targets) -> Vec<VirtualTarget> {
    let one = |v: VirtualTarget| vec![v];
    match row.address_kind {
        "doc" => one(t.doc.clone()),
        "page" => one(VirtualTarget::Page {
            page_id: t.page.clone(),
        }),
        "layer" => one(VirtualTarget::Layer {
            layer_id: t.layer.clone(),
        }),
        "master" => one(VirtualTarget::Master {
            master_id: t.master.clone(),
        }),
        "story" => one(VirtualTarget::Story {
            story_id: t.story.clone(),
        }),
        "field" => one(VirtualTarget::Field {
            story_id: t.field.0.clone(),
            offset: t.field.1,
        }),
        "swatch" if matches!(row.path, "convertToProcess" | "inkAliasTo") => {
            one(VirtualTarget::Swatch {
                swatch_id: t.spot.clone(),
            })
        }
        "swatch" => one(VirtualTarget::Swatch {
            swatch_id: t.swatch.clone(),
        }),
        "gradient" => one(VirtualTarget::Gradient {
            gradient_id: t.gradient.clone(),
        }),
        "colorGroup" => one(VirtualTarget::ColorGroup {
            group_id: t.color_group.clone(),
        }),
        "numberingList" => one(VirtualTarget::NumberingList {
            list_id: t.numbering_list.clone(),
        }),
        "section" => one(VirtualTarget::Section {
            section_id: t.section.clone(),
        }),
        "guide" => one(VirtualTarget::Guide {
            guide_id: t.guide.clone(),
        }),
        "condition" => one(VirtualTarget::Condition {
            condition_id: t.condition.clone(),
        }),
        "textVariable" => one(VirtualTarget::TextVariable {
            variable_id: t.text_variable.clone(),
        }),
        "table" => one(VirtualTarget::Table {
            story_id: t.table.0.clone(),
            table_id: t.table.1.clone(),
        }),
        "cell" => one(VirtualTarget::Cell {
            story_id: t.table.0.clone(),
            table_id: t.table.1.clone(),
            row: 0,
            col: 0,
        }),
        "frame" => one(VirtualTarget::Element {
            element_id: t.frame.clone(),
        }),
        "group" => one(VirtualTarget::Element {
            element_id: t.group.clone(),
        }),
        "style" => t
            .styles
            .iter()
            .filter(|(f, _)| row.applies_to.is_empty() || row.applies_to.contains(&f.as_str()))
            .map(|(family, id)| VirtualTarget::Style {
                family: *family,
                style_id: id.clone(),
            })
            .collect(),
        other => panic!("no target for address kind {other:?} — add one to `Targets`"),
    }
}

fn concrete(path: &str) -> String {
    path.replace("[i]", "[0]")
        .replace("[key]", "[x-paged:media.paged.probe]")
}

/// A value of the row's type that differs from `current`.
fn sample(row: &SchemaRow, current: &Value, t: &Targets) -> Value {
    let ty = &row.value_type;
    // Rows whose value is a document object or a vocabulary the schema
    // cannot know.
    match (row.address_kind, row.path) {
        ("doc", "cmykProfile" | "proofProfile") => return Value::Text(PROFILE.into()),
        ("doc", "proofSimulatePaperWhite" | "proofIntent") => {
            // meaningful only with proofing on; the build turns it on.
        }
        ("paragraphBulletCharacter", _) | (_, "paragraphBulletCharacter") => {
            return Value::Text("\u{2022}".into())
        }
        ("doc", "activeConditionSet") => return Value::Ref(t.condition_set.clone()),
        ("page", "appliedMaster") => return Value::Ref(t.master.clone()),
        ("swatch", "inkAliasTo") => return Value::Ref(t.swatch.clone()),
        ("swatch", "value" | "alternateValue") => {
            let n = match current {
                Value::Lengths(v) if !v.is_empty() => v.len(),
                _ => 4,
            };
            return Value::Lengths(vec![12.0; n]);
        }
        ("swatch", "space" | "alternateSpace") => {
            // Keep the channel count valid: a 4-channel space stays CMYK
            // unless it is not CMYK now.
            return Value::Enum(if *current == Value::Text("CMYK".into()) {
                "CMYK".into()
            } else {
                "CMYK".into()
            });
        }
        ("gradient", "stops") => {
            return Value::GradientStops(vec![
                GradientStopSpec {
                    stop_color: "Color/Black".into(),
                    location_pct: 0.0,
                    midpoint_pct: None,
                },
                GradientStopSpec {
                    stop_color: t.swatch.clone(),
                    location_pct: 100.0,
                    midpoint_pct: None,
                },
            ])
        }
        ("colorGroup", "members") => return Value::Text(t.swatch.clone()),
        ("frame", "imageUri") => return Value::Ref("data:,probe".into()),
        ("group", "groupTransform") => {
            return Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 7.0, 5.0]))
        }
        ("style", "name") => return Value::Text("Probe name".into()),
        (_, "labels[key]") => return Value::Text(ENVELOPE.into()),
        _ => {}
    }
    let differs = |v: Value| v != *current;
    match ty.kind {
        ValueType::Bool => Value::Bool(!matches!(current, Value::Bool(true))),
        ValueType::Enum => {
            let pick = ty
                .members
                .iter()
                .find(|m| differs(Value::Text((**m).to_string())))
                .expect("an enum with one member");
            Value::Enum((*pick).to_string())
        }
        ValueType::Color => {
            let c = if differs(Value::ColorRef(Some("Color/Black".into())))
                && differs(Value::Text("Color/Black".into()))
            {
                "Color/Black"
            } else {
                t.swatch.as_str()
            };
            match row.wire {
                Wire::ColorRef => Value::ColorRef(Some(c.into())),
                _ => Value::Text(c.into()),
            }
        }
        ValueType::Ref => {
            let id = match ty.to {
                Some(paged_introspect::schema::RefKind::CellStyle) => t
                    .styles
                    .iter()
                    .find(|(f, _)| *f == StyleFamily::Cell)
                    .map(|(_, id)| id.clone())
                    .unwrap(),
                Some(paged_introspect::schema::RefKind::Swatch) => t.swatch.clone(),
                Some(paged_introspect::schema::RefKind::Master) => t.master.clone(),
                _ => "probe".into(),
            };
            Value::Ref(id)
        }
        ValueType::Text => Value::Text("probe".into()),
        ValueType::Number | ValueType::Length => {
            let lo = row.range.as_ref().and_then(|r| r.min).unwrap_or(0.0);
            let hi = row.range.as_ref().and_then(|r| r.max).unwrap_or(500.0);
            let mut n = match row.default {
                Some(DefaultValue::Number(d)) => d + 1.0,
                _ => lo.max(0.0) + 3.0,
            };
            if n > hi {
                n = lo.max(hi - 1.0);
            }
            if differs(Value::Length(Some(n as f32))) {
                Value::Number(n)
            } else {
                Value::Number((n + 1.0).min(hi))
            }
        }
        ValueType::Bounds => Value::Bounds([0.0, 0.0, 700.0, 500.0]),
        ValueType::List if row.wire == Wire::TabStops => {
            Value::TabStops(vec![paged_mutate::operation::TabStopSpec {
                position: 36.0,
                alignment: Some("LeftAlign".into()),
                alignment_character: None,
                leader: None,
            }])
        }
        ValueType::Transform => Value::Transform(Some([1.0, 0.0, 0.0, 1.0, 3.0, 4.0])),
        _ => panic!(
            "{} {}: no sample for {:?}",
            row.address_kind, row.path, ty.kind
        ),
    }
}

/// A plugin-metadata label value: the engine checks the JSON envelope.
const PROFILE: &str = "Probe CMYK";

const ENVELOPE: &str = r#"{"v":1,"data":{"probe":1}}"#;

/// Compare a read against what was written, through the wire forms the
/// typed values normalise to.
fn same(written: &Value, read: &Value) -> bool {
    let norm = |v: &Value| -> Value {
        match v.normalized().into_owned() {
            Value::ColorRef(Some(s)) => Value::Text(s),
            Value::ColorRef(None) | Value::Length(None) => Value::Text(String::new()),
            other => other,
        }
    };
    let (w, r) = (norm(written), norm(read));
    if w == r {
        return true;
    }
    // A count the engine keeps as text ("4" — the span split count also
    // takes "All") reads back as the string of the number.
    matches!((&w, &r), (Value::Length(Some(n)), Value::Text(s)) if s.parse::<f32>().ok() == Some(*n))
}

#[test]
fn every_virtual_row_sets_reads_back_and_undoes() {
    let rows: Vec<SchemaRow> = property_schemas()
        .into_iter()
        .filter(|r| r.virtual_op.is_some() && r.access == "readWrite")
        .collect();
    assert!(rows.len() >= 120, "only {} virtual rows", rows.len());
    let mut failures = Vec::new();
    let mut probed = 0usize;
    for row in &rows {
        let (mut m, t) = build();
        let path = concrete(row.path);
        for target in targets_for(row, &t) {
            let label = format!("{} {} on {target:?}", row.address_kind, path);
            let before = match m.get_property(&target, &path) {
                Ok(v) => v,
                Err(e) => {
                    failures.push(format!("{label}: read before: {e}"));
                    continue;
                }
            };
            let value = sample(row, &before, &t);
            let set = Mutation::Set {
                address: target.clone(),
                path: path.clone(),
                value: value.clone(),
            };
            if let Err(e) = m.apply_mutation(&set) {
                failures.push(format!("{label}: set {value:?}: {e:?}"));
                continue;
            }
            match m.get_property(&target, &path) {
                Ok(read) if same(&value, &read) => {}
                Ok(read) => {
                    failures.push(format!("{label}: wrote {value:?}, read {read:?}"));
                    continue;
                }
                Err(e) => {
                    failures.push(format!("{label}: read after: {e}"));
                    continue;
                }
            }
            let op = row.virtual_op.as_ref().map(|v| v.op).unwrap_or_default();
            if !NOT_UNDOABLE_OPS.contains(&op) {
                m.undo();
                let after_undo = m.get_property(&target, &path);
                if after_undo.as_ref().ok().is_none_or(|v| !same(&before, v)) {
                    failures.push(format!("{label}: undo left {after_undo:?}, was {before:?}"));
                    continue;
                }
            }
            probed += 1;
        }
    }
    assert!(
        failures.is_empty(),
        "{} of the virtual rows failed set/get/undo:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
    assert!(
        probed >= rows.len(),
        "probed {probed} of {} rows",
        rows.len()
    );
}

#[test]
fn a_set_inside_a_batch_takes_a_handle_and_is_one_undo_step() {
    let (mut m, t) = build();
    let swatch = VirtualTarget::Swatch {
        swatch_id: t.swatch.clone(),
    };
    let before_tint = m.get_property(&swatch, "tint").unwrap();
    let page = t.page.clone();
    let batch = Mutation::Batch {
        ops: vec![
            Mutation::InsertTextFrame {
                page_id: PageId(page),
                bounds: (300.0, 10.0, 400.0, 200.0),
            },
            Mutation::BindCreated { handle: "f".into() },
            Mutation::Set {
                address: VirtualTarget::Element {
                    element_id: ElementId::TextFrame("$h:f".into()),
                },
                path: "frameFillColor".into(),
                value: Value::ColorRef(Some("Color/Black".into())),
            },
            // Two Sets on one swatch: the second must merge against the
            // first, not against the pre-batch spec.
            Mutation::Set {
                address: swatch.clone(),
                path: "tint".into(),
                value: Value::Number(40.0),
            },
            Mutation::Set {
                address: swatch.clone(),
                path: "name".into(),
                value: Value::Text("Batch probe".into()),
            },
        ],
    };
    let frames_before = m
        .scene()
        .spreads
        .iter()
        .map(|s| s.spread.text_frames.len())
        .sum::<usize>();
    m.apply_mutation(&batch).expect("batch");
    assert_eq!(
        m.get_property(&swatch, "tint").unwrap(),
        Value::Length(Some(40.0))
    );
    assert_eq!(
        m.get_property(&swatch, "name").unwrap(),
        Value::Text("Batch probe".into())
    );
    m.undo().expect("one undo");
    assert_eq!(m.get_property(&swatch, "tint").unwrap(), before_tint);
    let frames_after = m
        .scene()
        .spreads
        .iter()
        .map(|s| s.spread.text_frames.len())
        .sum::<usize>();
    assert_eq!(frames_before, frames_after, "the whole batch is one step");
}

#[test]
fn the_schema_refuses_what_it_does_not_describe() {
    let (mut m, t) = build();
    let story_range = VirtualTarget::Element {
        element_id: ElementId::StoryRange {
            story_id: t.story.clone(),
            start: 0,
            end: 3,
        },
    };
    let refuse = |m: &mut CanvasModel, address: &VirtualTarget, path: &str, value: Value| {
        let r = m.apply_mutation(&Mutation::Set {
            address: address.clone(),
            path: path.into(),
            value,
        });
        assert!(r.is_err(), "{path} accepted a value its schema refuses");
    };
    refuse(
        &mut m,
        &story_range,
        "paragraphJustification",
        Value::Enum("Middle".into()),
    );
    refuse(
        &mut m,
        &t.doc.clone(),
        "documentBleedTop",
        Value::Bool(true),
    );
    refuse(
        &mut m,
        &VirtualTarget::Layer {
            layer_id: t.layer.clone(),
        },
        "frameOpacity",
        Value::Number(10.0),
    );
    refuse(
        &mut m,
        &VirtualTarget::Swatch {
            swatch_id: t.swatch.clone(),
        },
        "tint",
        Value::Number(140.0),
    );
    // …and the typed forms are taken where the old ones were.
    m.apply_mutation(&Mutation::Set {
        address: story_range.clone(),
        path: "paragraphJustification".into(),
        value: Value::Enum("CenterAlign".into()),
    })
    .expect("an enum member applies");
    m.apply_mutation(&Mutation::Set {
        address: story_range,
        path: "characterFontSize".into(),
        value: Value::Number(18.0),
    })
    .expect("a Number applies on a length path");
}

#[test]
fn a_native_label_is_readable_whatever_its_namespace() {
    let (mut m, t) = build();
    let frame = VirtualTarget::Element {
        element_id: t.frame.clone(),
    };
    // Written the only way a script may (x-paged:), read back through the
    // same path…
    m.apply_mutation(&Mutation::Set {
        address: frame.clone(),
        path: "labels[x-paged:media.paged.probe]".into(),
        value: Value::Text(ENVELOPE.into()),
    })
    .expect("x-paged label");
    assert_eq!(
        m.get_property(&frame, "labels[x-paged:media.paged.probe]")
            .unwrap(),
        Value::Text(ENVELOPE.into())
    );
    // …and a native key cannot be written, but an absent one reads null.
    assert!(m
        .apply_mutation(&Mutation::Set {
            address: frame.clone(),
            path: "labels[Label]".into(),
            value: Value::Text("x".into()),
        })
        .is_err());
    assert_eq!(
        m.get_property(&frame, "labels[Label]").unwrap(),
        Value::Length(None)
    );
}
