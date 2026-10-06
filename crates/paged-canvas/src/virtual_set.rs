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

//! ADR 132 — `Mutation::Set { address, path, value }` and its read twin.
//!
//! `Set` is not a second apply path. [`CanvasModel::resolve_set`] turns
//! `(address, path, value)` into the ONE thing that already writes that
//! property — a `SetProperty` operation for a real `PropertyPath`, or the
//! dedicated mutation (`EditSwatch` with its spec read and the one field
//! merged, `LayerSetName`, `SetFlowGrowRule`, …) — and `apply_mutation`
//! applies that. Undo, inversion and batch handles are therefore exactly
//! what the resolved op already had.
//!
//! The value is checked against the path's schema first
//! ([`paged_introspect::schema::validate`]): closed enums refuse
//! strangers, ranges hold, and the ADR 132 typed forms (`Number`, `Enum`,
//! `Ref`) normalise to the wire forms the ops take.
//!
//! Lives as a child of `model` so it reads the model's private state (the
//! document defaults, colour settings, ink settings) without widening it.

use paged_introspect::catalog::{addressed_on, lookup_path, schema, wire_name, ADDRESSED_PATHS};
use paged_introspect::schema::{validate, PropertySchema};
use paged_introspect::virtual_paths::{lookup_virtual, segment, style_paths};
use paged_mutate::{NodeId, Operation, PropertyPath, StyleCollection, Value};
use paged_wire::{ElementId, PageId, StyleFamily, VirtualTarget};

use super::CanvasModel;
use crate::channel::Mutation;

/// What a `Set` becomes.
#[derive(Debug, Clone)]
pub enum Resolved {
    /// An existing wire mutation (the dedicated op).
    Mutation(Mutation),
    /// A `SetProperty` on a node the wire's `ElementId` cannot name (a
    /// layer, a page, the document, a text variable).
    Operation(Operation),
}

fn text_of(v: &Value) -> Option<Option<String>> {
    match v {
        Value::Text(s) | Value::Enum(s) | Value::Ref(s) => Some((!s.is_empty()).then(|| s.clone())),
        Value::ColorRef(c) => Some(c.clone()),
        Value::Length(None) => Some(None),
        _ => None,
    }
}

fn number_of(v: &Value) -> Option<Option<f32>> {
    match v {
        Value::Length(n) => Some(*n),
        Value::Number(n) => Some(Some(*n as f32)),
        _ => None,
    }
}

fn bool_of(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn family_collection(f: StyleFamily) -> StyleCollection {
    match f {
        StyleFamily::Paragraph => StyleCollection::Paragraph,
        StyleFamily::Character => StyleCollection::Character,
        StyleFamily::Object => StyleCollection::Object,
        StyleFamily::Cell => StyleCollection::Cell,
        StyleFamily::Table => StyleCollection::Table,
    }
}

/// The `PropertyPath` an addressed row names on `kind`, if any.
fn addressed_path(kind: &str, path: &str) -> Option<PropertyPath> {
    ADDRESSED_PATHS
        .iter()
        .find(|(name, _, k)| *name == path && *k == kind)
        .map(|(_, p, _)| *p)
}

/// The schema-checked, normalised value for a virtual row.
fn checked(schema: &PropertySchema, value: &Value, what: &str) -> Result<Value, String> {
    validate(schema, value).map_err(|e| format!("{what}: {e}"))
}

impl CanvasModel {
    /// Resolve one `Set` to the operation that already writes it. `Err`
    /// names why (unknown path for the kind, a value outside the schema,
    /// a target that does not exist).
    pub fn resolve_set(
        &self,
        address: &VirtualTarget,
        path: &str,
        value: &Value,
    ) -> Result<Resolved, String> {
        let kind = address.address_kind();
        let what = format!("{kind}.{path}");
        let unknown = || format!("{what}: no such settable path on a {kind}");
        // Op-backed rows first: they are named by the table.
        let row = lookup_virtual(kind, path).or_else(|| match kind {
            // a page item's row also answers on its specific kind
            "group" => lookup_virtual("frame", path),
            _ => None,
        });
        if let Some(row) = row {
            let v = checked(&row.schema, value, &what)?;
            return self.resolve_virtual(address, row.op, row.field, path, &v, &what);
        }
        match address {
            VirtualTarget::Element { element_id } => {
                let p = lookup_path(path).ok_or_else(unknown)?;
                let v = checked(&schema(p), value, &what)?;
                Ok(Resolved::Mutation(Mutation::SetElementProperty {
                    element_id: element_id.clone(),
                    path: p,
                    value: v,
                }))
            }
            VirtualTarget::Table { story_id, table_id } => {
                let p = lookup_path(path)
                    .filter(|p| matches!(p, PropertyPath::AppliedTableStyle))
                    .ok_or_else(unknown)?;
                let v = checked(&schema(p), value, &what)?;
                Ok(Resolved::Mutation(Mutation::SetElementProperty {
                    element_id: ElementId::Table {
                        story_id: story_id.clone(),
                        table_id: table_id.clone(),
                    },
                    path: p,
                    value: v,
                }))
            }
            VirtualTarget::Cell {
                story_id,
                table_id,
                row,
                col,
            } => {
                let p = lookup_path(path).ok_or_else(unknown)?;
                let v = checked(&schema(p), value, &what)?;
                Ok(Resolved::Mutation(Mutation::SetElementProperty {
                    element_id: ElementId::TableCell {
                        story_id: story_id.clone(),
                        table_id: table_id.clone(),
                        row: *row,
                        col: *col,
                    },
                    path: p,
                    value: v,
                }))
            }
            VirtualTarget::Style { family, style_id } => {
                let p = lookup_path(path)
                    .or_else(|| addressed_path(&format!("style:{}", family.as_str()), path))
                    .filter(|p| style_paths(family.as_str()).contains(p))
                    .ok_or_else(unknown)?;
                let v = checked(&schema(p), value, &what)?;
                Ok(Resolved::Mutation(Mutation::SetStyleProperty {
                    collection: family_collection(*family),
                    style_id: style_id.clone(),
                    path: p,
                    value: v,
                }))
            }
            VirtualTarget::Layer { layer_id } => {
                let p = addressed_path("layer", path).ok_or_else(unknown)?;
                let v = checked(&schema(p), value, &what)?;
                Ok(Resolved::Operation(Operation::SetProperty {
                    node: NodeId::Layer(layer_id.clone()),
                    path: p,
                    value: v,
                }))
            }
            VirtualTarget::Page { page_id } => {
                let p = addressed_path("page", path).ok_or_else(unknown)?;
                let v = checked(&schema(p), value, &what)?;
                Ok(Resolved::Operation(Operation::SetProperty {
                    node: NodeId::Page(page_id.clone()),
                    path: p,
                    value: v,
                }))
            }
            VirtualTarget::Doc => {
                let p = addressed_path("doc", path).ok_or_else(unknown)?;
                let v = checked(&schema(p), value, &what)?;
                Ok(Resolved::Operation(Operation::SetProperty {
                    node: NodeId::Document(String::new()),
                    path: p,
                    value: v,
                }))
            }
            VirtualTarget::TextVariable { variable_id } => {
                let p = addressed_path("textVariable", path).ok_or_else(unknown)?;
                let v = checked(&schema(p), value, &what)?;
                Ok(Resolved::Operation(Operation::SetProperty {
                    node: NodeId::TextVariable(variable_id.clone()),
                    path: p,
                    value: v,
                }))
            }
            _ => Err(unknown()),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn resolve_virtual(
        &self,
        address: &VirtualTarget,
        op: &str,
        field: &str,
        path: &str,
        v: &Value,
        what: &str,
    ) -> Result<Resolved, String> {
        let bad = || format!("{what}: value does not fit");
        let missing = |id: &str| format!("{what}: no {} {id:?}", address.address_kind());
        let m = |m: Mutation| Ok(Resolved::Mutation(m));
        let scene = &self.scene;
        match (address, op) {
            // ---- doc
            (VirtualTarget::Doc, "setDocumentDefaults") => {
                let mut d = self.document_defaults.clone();
                match field {
                    "fillColor" => d.fill_color = text_of(v).ok_or_else(bad)?,
                    "strokeColor" => d.stroke_color = text_of(v).ok_or_else(bad)?,
                    _ => d.stroke_weight = number_of(v).ok_or_else(bad)?,
                }
                m(Mutation::SetDocumentDefaults {
                    fill_color: d.fill_color,
                    stroke_color: d.stroke_color,
                    stroke_weight: d.stroke_weight,
                })
            }
            (VirtualTarget::Doc, "setColorSettings") => {
                let c = &self.color_settings;
                let mut name = c.cmyk_profile_name.clone();
                let mut rgb = c.rgb_policy.clone();
                let mut intent = Some(c.intent.name().to_string());
                let mut bpc = Some(c.bpc);
                match field {
                    "cmykProfileName" => name = text_of(v).ok_or_else(bad)?,
                    "rgbPolicy" => rgb = text_of(v).ok_or_else(bad)?,
                    "intent" => intent = text_of(v).ok_or_else(bad)?,
                    _ => bpc = Some(bool_of(v).ok_or_else(bad)?),
                }
                m(Mutation::SetColorSettings {
                    cmyk_profile_name: name,
                    rgb_policy: rgb,
                    intent,
                    bpc,
                })
            }
            (VirtualTarget::Doc, "setProofSetup") => {
                let p = self.proof_state.as_ref();
                let mut name = p.map(|p| p.name.clone());
                let mut paper = p.is_some_and(|p| p.simulate_paper_white);
                let mut intent = p.map(|p| p.intent.name().to_string());
                match field {
                    "profileName" => name = text_of(v).ok_or_else(bad)?,
                    "simulatePaperWhite" => paper = bool_of(v).ok_or_else(bad)?,
                    _ => intent = text_of(v).ok_or_else(bad)?,
                }
                m(Mutation::SetProofSetup {
                    profile_name: name,
                    simulate_paper_white: paper,
                    intent,
                })
            }
            (VirtualTarget::Doc, "setUseStandardLabForSpots") => {
                m(Mutation::SetUseStandardLabForSpots {
                    enabled: bool_of(v).ok_or_else(bad)?,
                })
            }
            (VirtualTarget::Doc, "activateConditionSet") => m(Mutation::ActivateConditionSet {
                set: text_of(v).ok_or_else(bad)?.ok_or_else(bad)?,
            }),
            (VirtualTarget::Doc, "setDocumentMetadata") => m(Mutation::SetDocumentMetadata {
                key: segment(path).ok_or_else(bad)?.to_string(),
                value: text_of(v).ok_or_else(bad)?,
                caller: None,
            }),
            // ---- swatch
            (VirtualTarget::Swatch { swatch_id }, "editSwatch") => {
                let entry = scene
                    .palette
                    .colors
                    .get(swatch_id)
                    .ok_or_else(|| missing(swatch_id))?;
                let mut spec = paged_mutate::apply::swatch_spec_from_entry(entry);
                match field {
                    "name" => spec.name = text_of(v).ok_or_else(bad)?,
                    "space" => spec.space = text_of(v).ok_or_else(bad)?.ok_or_else(bad)?,
                    "model" => spec.model = text_of(v).ok_or_else(bad)?,
                    "alternateSpace" => spec.alternate_space = text_of(v).ok_or_else(bad)?,
                    "tint" => spec.tint = number_of(v).ok_or_else(bad)?,
                    "alpha" => spec.alpha = number_of(v).ok_or_else(bad)?,
                    "value" | "alternateValue" => {
                        let Value::Lengths(list) = v else {
                            return Err(bad());
                        };
                        if field == "value" {
                            spec.value = list.clone();
                        } else {
                            spec.alternate_value = list.clone();
                        }
                    }
                    _ => return Err(bad()),
                }
                m(Mutation::EditSwatch {
                    swatch_id: swatch_id.clone(),
                    spec,
                })
            }
            (VirtualTarget::Swatch { swatch_id }, "setInkSetting") => {
                let cur = self
                    .ink_settings
                    .get(swatch_id)
                    .cloned()
                    .unwrap_or_default();
                let (mut convert, mut alias) = (cur.convert_to_process, cur.alias_to);
                match field {
                    "convertToProcess" => convert = bool_of(v).ok_or_else(bad)?,
                    _ => alias = text_of(v).ok_or_else(bad)?,
                }
                m(Mutation::SetInkSetting {
                    spot_id: swatch_id.clone(),
                    convert_to_process: convert,
                    alias_to: alias,
                })
            }
            // ---- gradient / colour group / numbering list
            (VirtualTarget::Gradient { gradient_id }, "editGradient") => {
                let entry = scene
                    .palette
                    .gradients
                    .get(gradient_id)
                    .ok_or_else(|| missing(gradient_id))?;
                let mut spec = paged_mutate::apply::gradient_spec_from_entry(entry);
                match (field, v) {
                    ("name", _) => spec.name = text_of(v).ok_or_else(bad)?,
                    ("kind", _) => spec.kind = text_of(v).ok_or_else(bad)?.ok_or_else(bad)?,
                    ("stops", Value::GradientStops(stops)) => spec.stops = stops.clone(),
                    _ => return Err(bad()),
                }
                m(Mutation::EditGradient {
                    gradient_id: gradient_id.clone(),
                    spec,
                })
            }
            (VirtualTarget::ColorGroup { group_id }, "editColorGroup") => {
                let g = scene
                    .palette
                    .color_groups
                    .get(group_id)
                    .ok_or_else(|| missing(group_id))?;
                let mut spec = paged_mutate::ColorGroupSpec {
                    self_id: Some(g.self_id.clone()),
                    name: g.name.clone(),
                    members: g.members.clone(),
                };
                match field {
                    "name" => spec.name = text_of(v).ok_or_else(bad)?,
                    _ => {
                        spec.members = text_of(v)
                            .ok_or_else(bad)?
                            .unwrap_or_default()
                            .split_whitespace()
                            .map(str::to_string)
                            .collect();
                    }
                }
                m(Mutation::EditColorGroup {
                    group_id: group_id.clone(),
                    spec,
                })
            }
            (VirtualTarget::NumberingList { list_id }, "editNumberingList") => {
                let def = scene
                    .styles
                    .numbering_lists
                    .get(list_id)
                    .ok_or_else(|| missing(list_id))?;
                let mut spec = paged_mutate::apply::numbering_list_spec_from_def(def);
                match field {
                    "name" => spec.name = text_of(v).ok_or_else(bad)?,
                    "continueAcrossStories" => {
                        spec.continue_across_stories = Some(bool_of(v).ok_or_else(bad)?)
                    }
                    _ => spec.continue_across_documents = Some(bool_of(v).ok_or_else(bad)?),
                }
                m(Mutation::EditNumberingList {
                    list_id: list_id.clone(),
                    spec,
                })
            }
            // ---- style name
            (VirtualTarget::Style { family, style_id }, _) if field == "name" => {
                let name = text_of(v).ok_or_else(bad)?.ok_or_else(bad)?;
                let style_id = style_id.clone();
                m(match family {
                    StyleFamily::Paragraph => Mutation::RenameParagraphStyle { style_id, name },
                    StyleFamily::Character => Mutation::RenameCharacterStyle { style_id, name },
                    StyleFamily::Object => Mutation::RenameObjectStyle { style_id, name },
                    StyleFamily::Cell => Mutation::RenameCellStyle { style_id, name },
                    StyleFamily::Table => Mutation::RenameTableStyle { style_id, name },
                })
            }
            // ---- guide / condition
            (VirtualTarget::Guide { guide_id }, "moveGuide") => m(Mutation::MoveGuide {
                guide_id: guide_id.clone(),
                position: number_of(v).flatten().ok_or_else(bad)?,
            }),
            (VirtualTarget::Condition { condition_id }, "setConditionVisible") => {
                m(Mutation::SetConditionVisible {
                    condition: condition_id.clone(),
                    visible: bool_of(v).ok_or_else(bad)?,
                })
            }
            // ---- story
            (VirtualTarget::Story { story_id }, "setFlowGrowRule") => {
                let story = scene
                    .stories
                    .iter()
                    .find(|s| s.self_id == *story_id)
                    .ok_or_else(|| missing(story_id))?;
                let cur = story.story.grow.clone();
                let mut max_pages = cur.as_ref().and_then(|g| g.max_pages);
                let mut copy = cur.as_ref().is_some_and(|g| g.copy_frame_options);
                // Setting either option turns the rule on; `flowGrow`
                // false clears it.
                let grow = match field {
                    "grow" => bool_of(v).ok_or_else(bad)?,
                    "maxPages" => {
                        max_pages = number_of(v).ok_or_else(bad)?.map(|n| n as u32);
                        true
                    }
                    _ => {
                        copy = bool_of(v).ok_or_else(bad)?;
                        true
                    }
                };
                m(Mutation::SetFlowGrowRule {
                    story_id: story_id.clone(),
                    grow,
                    max_pages,
                    copy_frame_options: Some(copy),
                })
            }
            // ---- page / master / section
            (VirtualTarget::Page { page_id }, "applyMasterToPage") => {
                m(Mutation::ApplyMasterToPage {
                    page: PageId(page_id.clone()),
                    master: text_of(v).ok_or_else(bad)?,
                })
            }
            (VirtualTarget::Page { page_id }, "setPageMetadata") => m(Mutation::SetPageMetadata {
                page: PageId(page_id.clone()),
                key: segment(path).ok_or_else(bad)?.to_string(),
                value: text_of(v).ok_or_else(bad)?,
                caller: None,
            }),
            (VirtualTarget::Master { master_id }, "renameMaster") => m(Mutation::RenameMaster {
                master: master_id.clone(),
                name: text_of(v).ok_or_else(bad)?,
            }),
            (VirtualTarget::Section { section_id }, "editSection") => {
                let (mut prefix, mut style, mut start) = (None, None, None);
                match field {
                    "prefix" => prefix = Some(text_of(v).ok_or_else(bad)?),
                    "numberingStyle" => style = text_of(v).ok_or_else(bad)?,
                    _ => start = Some(number_of(v).ok_or_else(bad)?.map(|n| n as u32)),
                }
                m(Mutation::EditSection {
                    section_id: section_id.clone(),
                    prefix,
                    numbering_style: style,
                    start_at: start,
                })
            }
            // ---- table / cell
            (VirtualTarget::Table { story_id, table_id }, "setRowHeight" | "setColumnWidth") => {
                let index: u32 = segment(path)
                    .and_then(|s| s.parse::<u32>().ok())
                    .ok_or_else(bad)?;
                let n = number_of(v).ok_or_else(bad)?;
                m(if op == "setRowHeight" {
                    Mutation::SetRowHeight {
                        story_id: story_id.clone(),
                        table_id: table_id.clone(),
                        row: index,
                        height: n,
                    }
                } else {
                    Mutation::SetColumnWidth {
                        story_id: story_id.clone(),
                        table_id: table_id.clone(),
                        col: index,
                        width: n,
                    }
                })
            }
            (
                VirtualTarget::Cell {
                    story_id,
                    table_id,
                    row,
                    col,
                },
                "setCellSpan",
            ) => {
                let cell = self
                    .find_table(story_id, table_id)
                    .and_then(|t| table_cell(t, *row, *col))
                    .ok_or_else(|| missing(&format!("{table_id}/{row},{col}")))?;
                let n = number_of(v).flatten().ok_or_else(bad)? as u32;
                let (mut rs, mut cs) = (cell.row_span.max(1), cell.column_span.max(1));
                if field == "rowSpan" {
                    rs = n;
                } else {
                    cs = n;
                }
                m(Mutation::SetCellSpan {
                    story_id: story_id.clone(),
                    table_id: table_id.clone(),
                    row: *row,
                    col: *col,
                    row_span: rs,
                    column_span: cs,
                })
            }
            // ---- field
            (VirtualTarget::Field { story_id, offset }, "setFieldValue") => {
                m(Mutation::SetFieldValue {
                    story_id: story_id.clone(),
                    offset: *offset,
                    value: text_of(v).ok_or_else(bad)?,
                })
            }
            // ---- frame / group
            (VirtualTarget::Element { element_id }, "placeImage") => m(Mutation::PlaceImage {
                element_id: element_id.raw_id().to_string(),
                uri: text_of(v).ok_or_else(bad)?.ok_or_else(bad)?,
                fit: None,
            }),
            (VirtualTarget::Element { element_id }, "setPluginMetadata") => {
                m(Mutation::SetPluginMetadata {
                    element_id: element_id.clone(),
                    key: segment(path).ok_or_else(bad)?.to_string(),
                    value: text_of(v).ok_or_else(bad)?,
                    caller: None,
                })
            }
            (VirtualTarget::Element { element_id }, "setElementProperty") => {
                m(Mutation::SetElementProperty {
                    element_id: element_id.clone(),
                    path: PropertyPath::ItemName,
                    value: Value::Text(text_of(v).ok_or_else(bad)?.unwrap_or_default()),
                })
            }
            (VirtualTarget::Element { element_id }, "setGroupTransform") => {
                let ElementId::Group(group_id) = element_id else {
                    return Err(format!("{what}: only a group takes groupTransform"));
                };
                let Value::Transform(t) = v else {
                    return Err(bad());
                };
                m(Mutation::SetGroupTransform {
                    group_id: group_id.clone(),
                    transform: *t,
                })
            }
            _ => Err(format!("{what}: no resolution for op {op}")),
        }
    }

    fn find_table(&self, story_id: &str, table_id: &str) -> Option<&paged_model::Table> {
        self.scene
            .stories
            .iter()
            .find(|s| s.self_id == story_id)?
            .story
            .paragraphs
            .iter()
            .filter_map(|p| p.table.as_ref())
            .find(|t| t.self_id.as_deref() == Some(table_id))
    }

    /// Read one property of any object, in its wire `Value` form — the
    /// read twin of [`Self::resolve_set`], over the same table.
    #[allow(clippy::too_many_lines)]
    pub fn get_property(&self, address: &VirtualTarget, path: &str) -> Result<Value, String> {
        let kind = address.address_kind();
        let what = format!("{kind}.{path}");
        let missing = || format!("{what}: the target does not exist");
        let unknown = || format!("{what}: no such readable path on a {kind}");
        let text = |s: &Option<String>| Value::Text(s.clone().unwrap_or_default());
        let scene = &self.scene;
        // Script labels: every key, not only `x-paged:` ones — a native
        // InDesign `Label` is a readable binding target.
        if path.starts_with("labels[") {
            let key = segment(path).ok_or_else(unknown)?;
            let labels: &[(String, String)] = match address {
                VirtualTarget::Doc => &scene.designmap.labels,
                VirtualTarget::Page { page_id } => self.label_list(page_id).unwrap_or(&[]),
                VirtualTarget::Spread { spread_id } => self.label_list(spread_id).unwrap_or(&[]),
                VirtualTarget::Element { element_id } => {
                    self.label_list(element_id.raw_id()).unwrap_or(&[])
                }
                _ => return Err(unknown()),
            };
            return Ok(match labels.iter().find(|(k, _)| k == key) {
                Some((_, v)) => Value::Text(v.clone()),
                None => Value::Length(None),
            });
        }
        match address {
            VirtualTarget::Element { element_id } if path == "imageUri" => {
                let id = element_id.raw_id();
                let spread = scene
                    .spreads
                    .iter()
                    .map(|s| &s.spread)
                    .find(|s| {
                        s.rectangles
                            .iter()
                            .any(|r| r.self_id.as_deref() == Some(id))
                            || s.ovals.iter().any(|r| r.self_id.as_deref() == Some(id))
                            || s.polygons.iter().any(|r| r.self_id.as_deref() == Some(id))
                    })
                    .ok_or_else(missing)?;
                let link = match element_id {
                    ElementId::Rectangle(_) => spread
                        .rectangles
                        .iter()
                        .find(|r| r.self_id.as_deref() == Some(id))
                        .and_then(|r| r.image_link.clone()),
                    ElementId::Oval(_) => spread
                        .ovals
                        .iter()
                        .find(|r| r.self_id.as_deref() == Some(id))
                        .and_then(|r| r.image_link.clone()),
                    ElementId::Polygon(_) => spread
                        .polygons
                        .iter()
                        .find(|r| r.self_id.as_deref() == Some(id))
                        .and_then(|r| r.image_link.clone()),
                    _ => return Err(unknown()),
                };
                Ok(text(&link))
            }
            VirtualTarget::Element { element_id } if path == "name" || path == "itemName" => {
                let id = element_id.raw_id();
                Ok(text(
                    &scene
                        .spreads
                        .iter()
                        .find_map(|s| s.spread.item_names.get(id))
                        .cloned(),
                ))
            }
            VirtualTarget::Element {
                element_id: ElementId::Group(id),
            } if path == "groupTransform" => {
                let g = scene
                    .spreads
                    .iter()
                    .flat_map(|s| s.spread.groups.iter())
                    .find(|g| g.self_id.as_deref() == Some(id.as_str()))
                    .ok_or_else(missing)?;
                Ok(Value::Transform(g.item_transform))
            }
            VirtualTarget::Element { element_id } => {
                let p = lookup_path(path).ok_or_else(unknown)?;
                let props = self.element_properties(element_id).ok_or_else(missing)?;
                props
                    .entries
                    .into_iter()
                    .find(|e| e.path == p)
                    .and_then(|e| e.value)
                    .ok_or_else(unknown)
            }
            VirtualTarget::Doc => {
                if let Some(p) = addressed_path("doc", path) {
                    let d = &scene.designmap.document_preference;
                    let n = match p {
                        PropertyPath::DocumentBleedTop => d.bleed_top,
                        PropertyPath::DocumentBleedBottom => d.bleed_bottom,
                        PropertyPath::DocumentBleedInsideOrLeft => d.bleed_inside_or_left,
                        PropertyPath::DocumentBleedOutsideOrRight => d.bleed_outside_or_right,
                        PropertyPath::DocumentSlugTop => d.slug_top,
                        PropertyPath::DocumentSlugBottom => d.slug_bottom,
                        PropertyPath::DocumentSlugInsideOrLeft => d.slug_inside_or_left,
                        _ => d.slug_right_or_outside,
                    };
                    return Ok(Value::Length(Some(n)));
                }
                let d = &self.document_defaults;
                let c = &self.color_settings;
                let pr = self.proof_state.as_ref();
                Ok(match path {
                    "defaultFillColor" => text(&d.fill_color),
                    "defaultStrokeColor" => text(&d.stroke_color),
                    "defaultStrokeWeight" => Value::Length(d.stroke_weight),
                    "cmykProfile" => text(&c.cmyk_profile_name),
                    "rgbPolicy" => text(&c.rgb_policy),
                    "renderingIntent" => Value::Text(c.intent.name().to_string()),
                    "blackPointCompensation" => Value::Bool(c.bpc),
                    "proofProfile" => text(&pr.map(|p| p.name.clone())),
                    "proofSimulatePaperWhite" => {
                        Value::Bool(pr.is_some_and(|p| p.simulate_paper_white))
                    }
                    "proofIntent" => text(&pr.map(|p| p.intent.name().to_string())),
                    "useStandardLabForSpots" => Value::Bool(self.use_standard_lab_for_spots),
                    "activeConditionSet" => text(&self.active_condition_set()),
                    _ => return Err(unknown()),
                })
            }
            VirtualTarget::Page { page_id } => {
                let (spread, page) = scene
                    .spreads
                    .iter()
                    .map(|s| &s.spread)
                    .find_map(|s| {
                        s.pages
                            .iter()
                            .find(|p| p.self_id.as_deref() == Some(page_id))
                            .map(|p| (s, p))
                    })
                    .ok_or_else(missing)?;
                if path == "appliedMaster" {
                    return Ok(text(&page.applied_master));
                }
                let p = addressed_path("page", path).ok_or_else(unknown)?;
                if p == PropertyPath::PageBounds {
                    let b = &page.bounds;
                    return Ok(Value::Bounds([b.top, b.left, b.bottom, b.right]));
                }
                let Some(m) = spread.page_margins.get(page_id) else {
                    return Ok(Value::Length(None));
                };
                Ok(Value::Length(Some(match p {
                    PropertyPath::PageMarginTop => m.top,
                    PropertyPath::PageMarginBottom => m.bottom,
                    PropertyPath::PageMarginLeft => m.left,
                    PropertyPath::PageMarginRight => m.right,
                    PropertyPath::PageColumnCount => m.column_count as f32,
                    _ => m.column_gutter,
                })))
            }
            VirtualTarget::Layer { layer_id } => {
                let l = scene
                    .designmap
                    .layers
                    .iter()
                    .find(|l| l.self_id == *layer_id)
                    .ok_or_else(missing)?;
                Ok(match addressed_path("layer", path).ok_or_else(unknown)? {
                    PropertyPath::LayerVisible => Value::Bool(l.visible),
                    PropertyPath::LayerLocked => Value::Bool(l.locked),
                    PropertyPath::LayerPrintable => Value::Bool(l.printable),
                    _ => text(&l.name),
                })
            }
            VirtualTarget::Master { master_id } => match path {
                "name" => Ok(text(
                    &scene
                        .master_spreads
                        .get(master_id)
                        .ok_or_else(missing)?
                        .name,
                )),
                _ => Err(unknown()),
            },
            VirtualTarget::Story { story_id } => {
                let s = scene
                    .stories
                    .iter()
                    .find(|s| s.self_id == *story_id)
                    .ok_or_else(missing)?;
                let g = s.story.grow.as_ref();
                Ok(match path {
                    "flowGrow" => Value::Bool(g.is_some()),
                    "flowGrowMaxPages" => {
                        Value::Length(g.and_then(|g| g.max_pages).map(|n| n as f32))
                    }
                    "flowGrowCopyFrameOptions" => {
                        Value::Bool(g.is_some_and(|g| g.copy_frame_options))
                    }
                    _ => return Err(unknown()),
                })
            }
            VirtualTarget::Field { story_id, offset } => match path {
                "value" => {
                    let field = self.placeholder_at(story_id, *offset).ok_or_else(missing)?;
                    Ok(text(&field.value))
                }
                _ => Err(unknown()),
            },
            VirtualTarget::Swatch { swatch_id } => {
                if let Some(ink) = path
                    .strip_prefix("convertToProcess")
                    .map(|_| true)
                    .or_else(|| (path == "inkAliasTo").then_some(false))
                {
                    let s = self
                        .ink_settings
                        .get(swatch_id)
                        .cloned()
                        .unwrap_or_default();
                    return Ok(if ink {
                        Value::Bool(s.convert_to_process)
                    } else {
                        text(&s.alias_to)
                    });
                }
                let e = scene.palette.colors.get(swatch_id).ok_or_else(missing)?;
                let spec = paged_mutate::apply::swatch_spec_from_entry(e);
                Ok(match path {
                    "name" => text(&spec.name),
                    "space" => Value::Text(spec.space),
                    "value" => Value::Lengths(spec.value),
                    "model" => text(&spec.model),
                    "alternateSpace" => text(&spec.alternate_space),
                    "alternateValue" => Value::Lengths(spec.alternate_value),
                    "tint" => Value::Length(spec.tint),
                    "alpha" => Value::Length(spec.alpha),
                    _ => return Err(unknown()),
                })
            }
            VirtualTarget::Gradient { gradient_id } => {
                let e = scene
                    .palette
                    .gradients
                    .get(gradient_id)
                    .ok_or_else(missing)?;
                let spec = paged_mutate::apply::gradient_spec_from_entry(e);
                Ok(match path {
                    "name" => text(&spec.name),
                    "kind" => Value::Text(spec.kind),
                    "stops" => Value::GradientStops(spec.stops),
                    _ => return Err(unknown()),
                })
            }
            VirtualTarget::ColorGroup { group_id } => {
                let g = scene
                    .palette
                    .color_groups
                    .get(group_id)
                    .ok_or_else(missing)?;
                Ok(match path {
                    "name" => text(&g.name),
                    "members" => Value::Text(g.members.join(" ")),
                    _ => return Err(unknown()),
                })
            }
            VirtualTarget::NumberingList { list_id } => {
                let d = scene
                    .styles
                    .numbering_lists
                    .get(list_id)
                    .ok_or_else(missing)?;
                let spec = paged_mutate::apply::numbering_list_spec_from_def(d);
                Ok(match path {
                    "name" => text(&spec.name),
                    "continueAcrossStories" => {
                        Value::Bool(spec.continue_across_stories.unwrap_or(false))
                    }
                    "continueAcrossDocuments" => {
                        Value::Bool(spec.continue_across_documents.unwrap_or(false))
                    }
                    _ => return Err(unknown()),
                })
            }
            VirtualTarget::Style { family, style_id } => {
                self.style_property(*family, style_id, path)
            }
            VirtualTarget::Section { section_id } => {
                let s = scene
                    .designmap
                    .sections
                    .iter()
                    .find(|s| s.self_id == *section_id)
                    .ok_or_else(missing)?;
                Ok(match path {
                    "prefix" => text(&s.section_prefix),
                    "numberingStyle" => {
                        Value::Text(numbering_style_idml(s.numbering_style).to_string())
                    }
                    "startAt" => Value::Length(s.start_at.map(|n| n as f32)),
                    _ => return Err(unknown()),
                })
            }
            VirtualTarget::Guide { guide_id } => match path {
                "position" => {
                    let rest = guide_id.strip_prefix("Guide/").ok_or_else(missing)?;
                    let (spread_id, idx) = rest.rsplit_once('/').ok_or_else(missing)?;
                    let idx: usize = idx.parse().map_err(|_| missing())?;
                    let g = scene
                        .spreads
                        .iter()
                        .find(|s| s.spread.self_id.as_deref() == Some(spread_id))
                        .and_then(|s| s.spread.guides.get(idx))
                        .ok_or_else(missing)?;
                    Ok(Value::Length(Some(g.location)))
                }
                _ => Err(unknown()),
            },
            VirtualTarget::Condition { condition_id } => match path {
                "visible" => Ok(Value::Bool(
                    scene
                        .styles
                        .conditions
                        .get(condition_id)
                        .ok_or_else(missing)?
                        .visible
                        .unwrap_or(true),
                )),
                _ => Err(unknown()),
            },
            VirtualTarget::TextVariable { variable_id } => {
                let t = scene
                    .designmap
                    .text_variables
                    .iter()
                    .find(|t| t.self_id == *variable_id)
                    .ok_or_else(missing)?;
                Ok(text(
                    match addressed_path("textVariable", path).ok_or_else(unknown)? {
                        PropertyPath::TextVariableName => &t.name,
                        PropertyPath::TextVariableType => &t.variable_type,
                        PropertyPath::TextVariableContents => &t.contents,
                        PropertyPath::TextVariableTextBefore => &t.text_before,
                        PropertyPath::TextVariableTextAfter => &t.text_after,
                        _ => &t.date_format,
                    },
                ))
            }
            // Addressable (ADR 131) with no property rows yet: only their
            // labels (spreads, above) read.
            VirtualTarget::Spread { .. }
            | VirtualTarget::ConditionSet { .. }
            | VirtualTarget::Hyperlink { .. } => Err(unknown()),
            VirtualTarget::Table { story_id, table_id } => {
                let t = self.find_table(story_id, table_id).ok_or_else(missing)?;
                if let Some(seg) = segment(path) {
                    let i: usize = seg.parse::<usize>().map_err(|_| unknown())?;
                    return if path.starts_with("rows[") {
                        Ok(Value::Length(
                            t.rows.get(i).ok_or_else(missing)?.single_row_height,
                        ))
                    } else {
                        Ok(Value::Length(
                            t.columns.get(i).ok_or_else(missing)?.single_column_width,
                        ))
                    };
                }
                Ok(match lookup_path(path) {
                    Some(PropertyPath::AppliedTableStyle) => text(&t.applied_table_style),
                    Some(PropertyPath::TableRowCount) => Value::Length(Some(
                        (t.header_row_count + t.body_row_count + t.footer_row_count) as f32,
                    )),
                    Some(PropertyPath::TableColumnCount) => {
                        Value::Length(Some(t.column_count as f32))
                    }
                    _ => return Err(unknown()),
                })
            }
            VirtualTarget::Cell {
                story_id,
                table_id,
                row,
                col,
            } => {
                let t = self.find_table(story_id, table_id).ok_or_else(missing)?;
                let c = table_cell(t, *row, *col).ok_or_else(missing)?;
                match path {
                    "rowSpan" => return Ok(Value::Length(Some(c.row_span.max(1) as f32))),
                    "columnSpan" => return Ok(Value::Length(Some(c.column_span.max(1) as f32))),
                    _ => {}
                }
                let id = ElementId::TableCell {
                    story_id: story_id.clone(),
                    table_id: table_id.clone(),
                    row: *row,
                    col: *col,
                };
                self.get_property(&VirtualTarget::Element { element_id: id }, path)
            }
        }
    }

    fn label_list(&self, self_id: &str) -> Option<&[(String, String)]> {
        self.scene
            .spreads
            .iter()
            .find_map(|s| s.spread.labels.get(self_id))
            .map(Vec::as_slice)
    }

    fn placeholder_at(
        &self,
        story_id: &str,
        offset: u32,
    ) -> Option<&paged_model::PlaceholderField> {
        let story = self.scene.stories.iter().find(|s| s.self_id == story_id)?;
        let mut cursor = 0u32;
        for para in &story.story.paragraphs {
            for run in &para.runs {
                let len = run.text.chars().count() as u32;
                if let Some(tag) = run.placeholder.as_ref() {
                    if offset == cursor || (offset > cursor && offset < cursor + len) {
                        return Some(tag);
                    }
                }
                cursor += len;
            }
            // the paragraph break
            cursor += 1;
        }
        None
    }

    /// The set whose conditions are exactly the visible ones, if any —
    /// what `activateConditionSet` leaves behind.
    fn active_condition_set(&self) -> Option<String> {
        let styles = &self.scene.styles;
        let visible = |id: &str| {
            let id = id.strip_prefix("Condition/").map_or(id, |_| id);
            styles
                .conditions
                .get(id)
                .and_then(|c| c.visible)
                .unwrap_or(true)
        };
        styles
            .condition_sets
            .values()
            .find(|set| {
                styles.conditions.keys().all(|c| {
                    let member = set.conditions.iter().any(|m| m == c);
                    member == visible(c)
                })
            })
            .map(|s| s.self_id.clone())
    }

    /// The settable fields of an object, cell or table style definition as
    /// inspector rows — what `style_properties` publishes for those
    /// families, read by the same arms `get_property` uses.
    pub(super) fn definition_entries(
        &self,
        family: StyleFamily,
        style_id: &str,
    ) -> Vec<crate::channel::PropertyEntry> {
        style_paths(family.as_str())
            .iter()
            .map(|p| crate::channel::PropertyEntry {
                path: *p,
                value: self.style_property(family, style_id, wire_name(*p)).ok(),
            })
            .collect()
    }

    fn style_property(
        &self,
        family: StyleFamily,
        style_id: &str,
        path: &str,
    ) -> Result<Value, String> {
        let what = format!("style.{path}");
        let missing = || format!("{what}: no {} style {style_id:?}", family.as_str());
        let unknown = || format!("{what}: not a {} style path", family.as_str());
        let st = &self.scene.styles;
        let text = |s: &Option<String>| Value::Text(s.clone().unwrap_or_default());
        let color = |s: &Option<String>| Value::ColorRef(s.clone());
        let len = |n: Option<f32>| Value::Length(n);
        if path == "name" {
            let name = match family {
                StyleFamily::Paragraph => st.paragraph_styles.get(style_id).map(|s| s.name.clone()),
                StyleFamily::Character => st.character_styles.get(style_id).map(|s| s.name.clone()),
                StyleFamily::Object => st.object_styles.get(style_id).map(|s| s.name.clone()),
                StyleFamily::Cell => st.cell_styles.get(style_id).map(|s| s.name.clone()),
                StyleFamily::Table => st.table_styles.get(style_id).map(|s| s.name.clone()),
            };
            return Ok(text(&name.ok_or_else(missing)?));
        }
        let p = lookup_path(path)
            .or_else(|| addressed_path(&format!("style:{}", family.as_str()), path))
            .filter(|p| style_paths(family.as_str()).contains(p))
            .ok_or_else(unknown)?;
        use PropertyPath as P;
        match family {
            StyleFamily::Object => {
                let d = st.object_styles.get(style_id).ok_or_else(missing)?;
                Ok(match p {
                    P::FrameFillColor => color(&d.fill_color),
                    P::FrameFillTint => len(d.fill_tint),
                    P::FrameStrokeColor => color(&d.stroke_color),
                    P::FrameStrokeWeight => len(d.stroke_weight),
                    P::ObjectStyleStrokeTint => len(d.stroke_tint),
                    P::ObjectStyleCornerRadius => len(d.corner_radius),
                    _ => text(&d.corner_option),
                })
            }
            StyleFamily::Cell => {
                let d = st.cell_styles.get(style_id).ok_or_else(missing)?;
                Ok(match p {
                    P::CellFillColor => color(&d.fill_color),
                    P::CellVerticalJustification => text(&d.vertical_justification),
                    P::CellStyleRotationAngle => len(d.rotation_angle),
                    P::CellTopEdgeStrokeColor => color(&d.top_edge_stroke_color),
                    P::CellTopEdgeStrokeWeight => len(d.top_edge_stroke_weight),
                    P::CellBottomEdgeStrokeColor => color(&d.bottom_edge_stroke_color),
                    P::CellBottomEdgeStrokeWeight => len(d.bottom_edge_stroke_weight),
                    P::CellLeftEdgeStrokeColor => color(&d.left_edge_stroke_color),
                    P::CellLeftEdgeStrokeWeight => len(d.left_edge_stroke_weight),
                    P::CellRightEdgeStrokeColor => color(&d.right_edge_stroke_color),
                    _ => len(d.right_edge_stroke_weight),
                })
            }
            StyleFamily::Table => {
                let d = st.table_styles.get(style_id).ok_or_else(missing)?;
                let cnt = |n: Option<u32>| Value::Length(n.map(|n| n as f32));
                Ok(match p {
                    P::TableStyleHeaderRegionCellStyle => text(&d.header_region_cell_style),
                    P::TableStyleBodyRegionCellStyle => text(&d.body_region_cell_style),
                    P::TableStyleFooterRegionCellStyle => text(&d.footer_region_cell_style),
                    P::TableStyleLeftColumnRegionCellStyle => {
                        text(&d.left_column_region_cell_style)
                    }
                    P::TableStyleRightColumnRegionCellStyle => {
                        text(&d.right_column_region_cell_style)
                    }
                    P::TableStyleTopBorderStrokeColor => color(&d.top_border_stroke_color),
                    P::TableStyleTopBorderStrokeWeight => len(d.top_border_stroke_weight),
                    P::TableStyleBottomBorderStrokeColor => color(&d.bottom_border_stroke_color),
                    P::TableStyleBottomBorderStrokeWeight => len(d.bottom_border_stroke_weight),
                    P::TableStyleLeftBorderStrokeColor => color(&d.left_border_stroke_color),
                    P::TableStyleLeftBorderStrokeWeight => len(d.left_border_stroke_weight),
                    P::TableStyleRightBorderStrokeColor => color(&d.right_border_stroke_color),
                    P::TableStyleRightBorderStrokeWeight => len(d.right_border_stroke_weight),
                    P::TableStyleAlternatingFills => text(&d.alternating_fills),
                    P::TableStyleStartRowFillColor => color(&d.start_row_fill_color),
                    P::TableStyleStartRowFillCount => cnt(d.start_row_fill_count),
                    P::TableStyleStartRowFillTint => len(d.start_row_fill_tint),
                    P::TableStyleEndRowFillColor => color(&d.end_row_fill_color),
                    P::TableStyleEndRowFillCount => cnt(d.end_row_fill_count),
                    _ => len(d.end_row_fill_tint),
                })
            }
            // Paragraph and character definitions: the style-properties
            // read the editor's style panels already use.
            StyleFamily::Paragraph | StyleFamily::Character => {
                let collection = family_collection(family);
                let props = self
                    .style_properties(collection, style_id)
                    .ok_or_else(missing)?;
                props
                    .entries
                    .into_iter()
                    .find(|e| e.path == p)
                    .and_then(|e| e.value)
                    .ok_or_else(|| format!("{what}: {} is not read back", wire_name(p)))
            }
        }
    }
}

fn table_cell(t: &paged_model::Table, row: u32, col: u32) -> Option<&paged_model::TableCell> {
    t.cells.iter().find(|c| c.coords() == Some((col, row)))
}

/// The address kind an addressed `PropertyPath` lives on — re-exported
/// for surfaces that build a `VirtualTarget` from a path alone.
#[must_use]
pub fn path_address_kind(path: PropertyPath) -> Option<&'static str> {
    addressed_on(path)
}

fn numbering_style_idml(s: paged_model::NumberingStyle) -> &'static str {
    match s {
        paged_model::NumberingStyle::Arabic => "Arabic",
        paged_model::NumberingStyle::UpperRoman => "UpperRoman",
        paged_model::NumberingStyle::LowerRoman => "LowerRoman",
        paged_model::NumberingStyle::UpperAlpha => "UpperLetters",
        paged_model::NumberingStyle::LowerAlpha => "LowerLetters",
    }
}
