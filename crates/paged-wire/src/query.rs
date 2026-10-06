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

//! Selector evaluation against a document (ADR 131): `query(doc, sel)`
//! returns the canonical address of every matching object, in document
//! order, without duplicates.
//!
//! The document is flattened once into a containment tree (see the
//! selector module docs for its shape). Every address handed out names
//! its kind — `frame:<id>` and `page:#<n>` resolve to the kind-specific
//! / id form, a style named by display name to its `Self` id, a master
//! item to `master:<id> > <item>`.
//!
//! What a selector can test, and where it reads it:
//! - `name` — page items: `Spread::item_names`; pages, layers, masters,
//!   styles, swatches, gradients, colour groups, numbering lists,
//!   conditions, hyperlinks: their own `Name`; the document: its name; a
//!   field: its placeholder key.
//! - `label` — the `Properties/Label` `KeyValuePair`s the model carries:
//!   the document (designmap), and everything keyed by `Self` in a
//!   spread's label map (page items, pages, the spread itself, master
//!   spreads). Values are matched after decoding InDesign's
//!   `<?AID xxxx?>` spelling of non-BMP characters ([`decode_aid`]).

use std::collections::{HashMap, HashSet};

use paged_model::{FrameRef, Spread};
use paged_scene::Document;

use crate::address::{Address, AddressKind, GrammarError, StyleFamily};
use crate::selector::{Combinator, Filter, Head, Op, Selector, Step};
use crate::{ElementId, VirtualTarget};

/// Decode InDesign's processing-instruction spelling of a UTF-16 code
/// unit, `<?AID d83d?>`, back into text. InDesign writes every non-BMP
/// character of a label value as a surrogate PAIR of these; a lone or
/// malformed one is kept verbatim.
pub fn decode_aid(s: &str) -> String {
    if !s.contains("<?AID ") {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut units: Vec<u16> = Vec::new();
    let mut raw = String::new();
    let mut rest = s;
    let flush = |out: &mut String, units: &mut Vec<u16>, raw: &mut String| {
        if !units.is_empty() {
            match String::from_utf16(units) {
                Ok(t) => out.push_str(&t),
                Err(_) => out.push_str(raw),
            }
        }
        units.clear();
        raw.clear();
    };
    while let Some(i) = rest.find("<?AID ") {
        let after = &rest[i + 6..];
        let parsed = after.find("?>").and_then(|end| {
            let hex = after[..end].trim();
            u16::from_str_radix(hex, 16).ok().map(|u| (u, end))
        });
        match parsed {
            Some((unit, end)) => {
                if i > 0 {
                    flush(&mut out, &mut units, &mut raw);
                    out.push_str(&rest[..i]);
                }
                units.push(unit);
                raw.push_str(&rest[i..i + 6 + end + 2]);
                rest = &after[end + 2..];
            }
            None => {
                flush(&mut out, &mut units, &mut raw);
                out.push_str(&rest[..i + 6]);
                rest = after;
            }
        }
    }
    flush(&mut out, &mut units, &mut raw);
    out.push_str(rest);
    out
}

struct Node<'a> {
    addr: Address,
    parents: Vec<usize>,
    name: Option<&'a str>,
    labels: &'a [(String, String)],
    /// Page items: the layer's `Self` id.
    layer: Option<&'a str>,
    /// Page items: the applied object style.
    style: Option<&'a str>,
    /// Body pages: 1-based position in the document.
    page_number: Option<u32>,
}

/// The document as a containment tree.
pub struct Index<'a> {
    nodes: Vec<Node<'a>>,
    children: Vec<Vec<usize>>,
    layer_names: HashMap<&'a str, Option<&'a str>>,
    style_names: HashMap<&'a str, Option<&'a str>>,
    stories: HashSet<&'a str>,
}

const NO_LABELS: &[(String, String)] = &[];

fn labels_of<'a>(spread: &'a Spread, id: &str) -> &'a [(String, String)] {
    spread.labels.get(id).map_or(NO_LABELS, Vec::as_slice)
}

/// The bits of a page item a selector needs, by `FrameRef`.
struct ItemInfo<'a> {
    kind: AddressKind,
    id: Option<&'a str>,
    geometry: Option<(paged_model::Bounds, Option<[f32; 6]>)>,
    layer: Option<&'a str>,
    style: Option<&'a str>,
}

fn item_info(spread: &Spread, fr: FrameRef) -> Option<ItemInfo<'_>> {
    macro_rules! leaf {
        ($vec:ident, $i:expr, $kind:ident) => {
            spread.$vec.get($i).map(|f| ItemInfo {
                kind: AddressKind::$kind,
                id: f.self_id.as_deref(),
                geometry: Some((f.bounds, f.item_transform)),
                layer: f.item_layer.as_deref(),
                style: f.applied_object_style.as_deref(),
            })
        };
    }
    match fr {
        FrameRef::TextFrame(i) => leaf!(text_frames, i, TextFrame),
        FrameRef::Rectangle(i) => leaf!(rectangles, i, Rectangle),
        FrameRef::Oval(i) => leaf!(ovals, i, Oval),
        FrameRef::GraphicLine(i) => leaf!(graphic_lines, i, GraphicLine),
        FrameRef::Polygon(i) => leaf!(polygons, i, Polygon),
        FrameRef::Group(i) => spread.groups.get(i).map(|g| ItemInfo {
            kind: AddressKind::Group,
            id: g.self_id.as_deref(),
            geometry: None,
            // A group lives where its first leaf does.
            layer: g
                .members
                .iter()
                .find_map(|m| item_info(spread, *m).and_then(|i| i.layer)),
            style: None,
        }),
    }
}

/// The local page index a page item sits on: its centroid's page, the
/// first page when the centroid is on the pasteboard (the renderer's
/// rule), a group's first leaf's page.
fn page_of(spread: &Spread, fr: FrameRef, depth: usize) -> Option<usize> {
    if spread.pages.is_empty() || depth > 64 {
        return None;
    }
    let info = item_info(spread, fr)?;
    match info.geometry {
        Some((b, t)) => Some(paged_scene::page_index_for_bounds(&spread.pages, b, t).unwrap_or(0)),
        None => match fr {
            FrameRef::Group(i) => spread.groups.get(i).and_then(|g| {
                g.members
                    .iter()
                    .find_map(|m| page_of(spread, *m, depth + 1))
            }),
            _ => None,
        },
    }
}

/// `FrameRef` is not `Hash`; this is its identity.
fn ref_key(fr: FrameRef) -> (u8, usize) {
    match fr {
        FrameRef::TextFrame(i) => (0, i),
        FrameRef::Rectangle(i) => (1, i),
        FrameRef::Oval(i) => (2, i),
        FrameRef::GraphicLine(i) => (3, i),
        FrameRef::Polygon(i) => (4, i),
        FrameRef::Group(i) => (5, i),
    }
}

fn all_refs(spread: &Spread) -> Vec<FrameRef> {
    let mut out = Vec::new();
    out.extend((0..spread.text_frames.len()).map(FrameRef::TextFrame));
    out.extend((0..spread.rectangles.len()).map(FrameRef::Rectangle));
    out.extend((0..spread.ovals.len()).map(FrameRef::Oval));
    out.extend((0..spread.graphic_lines.len()).map(FrameRef::GraphicLine));
    out.extend((0..spread.polygons.len()).map(FrameRef::Polygon));
    out.extend((0..spread.groups.len()).map(FrameRef::Group));
    out
}

impl<'a> Index<'a> {
    pub fn build(doc: &'a Document) -> Self {
        let mut ix = Index {
            nodes: Vec::new(),
            children: Vec::new(),
            layer_names: HashMap::new(),
            style_names: HashMap::new(),
            stories: HashSet::new(),
        };
        let dm = &doc.designmap;
        let root = ix.push(Node {
            addr: Address::Doc,
            parents: vec![],
            name: dm.document_name.as_deref(),
            labels: &dm.labels,
            layer: None,
            style: None,
            page_number: None,
        });

        let mut layer_idx: HashMap<&str, usize> = HashMap::new();
        for layer in &dm.layers {
            ix.layer_names
                .insert(layer.self_id.as_str(), layer.name.as_deref());
            let i = ix.simple(
                Address::Layer {
                    id: layer.self_id.clone(),
                },
                root,
                layer.name.as_deref(),
            );
            layer_idx.insert(layer.self_id.as_str(), i);
        }
        for def in doc.styles.object_styles.values() {
            ix.style_names
                .insert(def.self_id.as_str(), def.name.as_deref());
        }

        let mut page_number = 0u32;
        for parsed in &doc.spreads {
            let spread = &parsed.spread;
            let spread_node = match spread.self_id.as_deref() {
                Some(id) => {
                    let i = ix.simple(Address::Spread { id: id.to_string() }, root, None);
                    ix.nodes[i].labels = labels_of(spread, id);
                    i
                }
                None => root,
            };
            let mut page_nodes = Vec::new();
            for page in &spread.pages {
                page_number += 1;
                let node = page.self_id.as_deref().map(|id| {
                    ix.push(Node {
                        addr: Address::Page { id: id.to_string() },
                        parents: vec![spread_node],
                        name: page.name.as_deref(),
                        labels: labels_of(spread, id),
                        layer: None,
                        style: None,
                        page_number: Some(page_number),
                    })
                });
                page_nodes.push(node.unwrap_or(spread_node));
            }
            ix.walk_spread(spread, spread_node, &page_nodes, None, &layer_idx);
            // Ruler guides: positional ids, `Guide/<spreadSelf>/<index>`,
            // the same the guide ops and `RequestGuides` use.
            if let Some(sid) = spread.self_id.as_deref() {
                for gi in 0..spread.guides.len() {
                    ix.simple(
                        Address::Guide {
                            id: format!("Guide/{sid}/{gi}"),
                        },
                        spread_node,
                        None,
                    );
                }
            }
        }

        let mut masters: Vec<_> = doc.master_spreads.values().collect();
        // Manifest order where the designmap gives one, then by id.
        let order: HashMap<&str, usize> = dm
            .master_spreads
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), i))
            .collect();
        masters.sort_by(|a, b| {
            let ka = order.get(a.src.as_str()).copied().unwrap_or(usize::MAX);
            let kb = order.get(b.src.as_str()).copied().unwrap_or(usize::MAX);
            ka.cmp(&kb).then_with(|| a.self_id.cmp(&b.self_id))
        });
        for m in masters {
            let i = ix.simple(
                Address::Master {
                    id: m.self_id.clone(),
                },
                root,
                m.name.as_deref(),
            );
            ix.nodes[i].labels = labels_of(&m.spread, &m.self_id);
            ix.walk_spread(&m.spread, i, &[], Some(&m.self_id), &layer_idx);
        }

        for parsed in &doc.stories {
            ix.stories.insert(parsed.self_id.as_str());
            let story = ix.simple(
                Address::Story {
                    id: parsed.self_id.clone(),
                },
                root,
                None,
            );
            let mut cursor = 0u32;
            for para in &parsed.story.paragraphs {
                if let Some(table) = &para.table {
                    if let Some(tid) = table.self_id.as_deref() {
                        let t = ix.simple(
                            Address::Table {
                                id: tid.to_string(),
                            },
                            story,
                            None,
                        );
                        for cell in &table.cells {
                            if let Some((col, row)) = cell.coords() {
                                ix.simple(
                                    Address::Cell {
                                        table_id: tid.to_string(),
                                        row,
                                        col,
                                    },
                                    t,
                                    None,
                                );
                            }
                        }
                    }
                }
                for run in &para.runs {
                    if let Some(tag) = &run.placeholder {
                        ix.simple(
                            Address::Field {
                                story_id: parsed.self_id.clone(),
                                offset: cursor,
                            },
                            story,
                            Some(tag.key.as_str()),
                        );
                    }
                    cursor += run.text.chars().count() as u32;
                }
            }
        }

        let st = &doc.styles;
        macro_rules! styles {
            ($map:expr, $family:ident) => {
                for def in $map.values() {
                    ix.simple(
                        Address::Style {
                            family: StyleFamily::$family,
                            id: def.self_id.clone(),
                        },
                        root,
                        def.name.as_deref(),
                    );
                }
            };
        }
        styles!(st.paragraph_styles, Paragraph);
        styles!(st.character_styles, Character);
        styles!(st.object_styles, Object);
        styles!(st.cell_styles, Cell);
        styles!(st.table_styles, Table);

        macro_rules! resources {
            ($iter:expr, $variant:ident) => {
                for e in $iter {
                    ix.simple(
                        Address::$variant {
                            id: e.self_id.clone(),
                        },
                        root,
                        e.name.as_deref(),
                    );
                }
            };
        }
        resources!(doc.palette.colors.values(), Swatch);
        resources!(doc.palette.swatches.values(), Swatch);
        resources!(doc.palette.gradients.values(), Gradient);
        resources!(doc.palette.color_groups.values(), ColorGroup);
        resources!(st.numbering_lists.values(), NumberingList);
        resources!(st.conditions.values(), Condition);
        resources!(st.condition_sets.values(), ConditionSet);
        resources!(dm.hyperlinks.iter(), Hyperlink);
        resources!(dm.text_variables.iter(), TextVariable);
        for s in &dm.sections {
            ix.simple(
                Address::Section {
                    id: s.self_id.clone(),
                },
                root,
                None,
            );
        }
        ix
    }

    fn push(&mut self, node: Node<'a>) -> usize {
        let i = self.nodes.len();
        for &p in &node.parents {
            self.children[p].push(i);
        }
        self.nodes.push(node);
        self.children.push(Vec::new());
        i
    }

    fn simple(&mut self, addr: Address, parent: usize, name: Option<&'a str>) -> usize {
        self.push(Node {
            addr,
            parents: vec![parent],
            name,
            labels: NO_LABELS,
            layer: None,
            style: None,
            page_number: None,
        })
    }

    /// Every page item of a spread, in paint order: `frames_in_order`
    /// with group members and paste-into children under their container,
    /// then anything only the backing vecs hold (opacity-mask items).
    fn walk_spread(
        &mut self,
        spread: &'a Spread,
        spread_node: usize,
        page_nodes: &[usize],
        master: Option<&str>,
        layers: &HashMap<&str, usize>,
    ) {
        let mut seen: HashSet<(u8, usize)> = HashSet::new();
        let top: Vec<FrameRef> = spread.frames_in_order.clone();
        for fr in top {
            self.walk_item(
                spread,
                fr,
                None,
                spread_node,
                page_nodes,
                master,
                layers,
                &mut seen,
                0,
            );
        }
        for fr in all_refs(spread) {
            if !seen.contains(&ref_key(fr)) {
                self.walk_item(
                    spread,
                    fr,
                    None,
                    spread_node,
                    page_nodes,
                    master,
                    layers,
                    &mut seen,
                    0,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_item(
        &mut self,
        spread: &'a Spread,
        fr: FrameRef,
        container: Option<usize>,
        spread_node: usize,
        page_nodes: &[usize],
        master: Option<&str>,
        layers: &HashMap<&str, usize>,
        seen: &mut HashSet<(u8, usize)>,
        depth: usize,
    ) {
        if depth > 64 || !seen.insert(ref_key(fr)) {
            return;
        }
        let Some(info) = item_info(spread, fr) else {
            return;
        };
        let mut this = container;
        if let Some(id) = info.id {
            let mut parents = vec![match container {
                Some(c) => c,
                None => page_of(spread, fr, 0)
                    .and_then(|p| page_nodes.get(p).copied())
                    .unwrap_or(spread_node),
            }];
            if let Some(l) = info.layer.and_then(|l| layers.get(l)) {
                parents.push(*l);
            }
            let item = Address::page_item(info.kind, id).expect("an item kind");
            let addr = match master {
                Some(m) => Address::OnMaster {
                    master: m.to_string(),
                    item: Box::new(item),
                },
                None => item,
            };
            this = Some(self.push(Node {
                addr,
                parents,
                name: spread.item_names.get(id).map(String::as_str),
                labels: labels_of(spread, id),
                layer: info.layer,
                style: info.style,
                page_number: None,
            }));
        }
        let mut kids: Vec<FrameRef> = Vec::new();
        if let FrameRef::Group(g) = fr {
            if let Some(group) = spread.groups.get(g) {
                kids.extend(group.members.iter().copied());
            }
        }
        if let Some(id) = info.id {
            if let Some(nested) = spread.nested_children.get(id) {
                kids.extend(nested.iter().copied());
            }
        }
        for k in kids {
            self.walk_item(
                spread,
                k,
                this,
                spread_node,
                page_nodes,
                master,
                layers,
                seen,
                depth + 1,
            );
        }
    }

    fn descendants(&self, of: usize, out: &mut Vec<usize>, seen: &mut HashSet<usize>) {
        for &c in &self.children[of] {
            if seen.insert(c) {
                out.push(c);
                self.descendants(c, out, seen);
            }
        }
    }

    fn head_matches(&self, n: &Node<'_>, head: &Head) -> bool {
        match head {
            Head::Any => true,
            Head::Kind(k) => kind_matches(n, *k),
            Head::PluginKind { .. } => false,
            Head::Address(a) => self.address_matches(n, a),
        }
    }

    fn address_matches(&self, n: &Node<'_>, a: &Address) -> bool {
        match a {
            Address::Frame { id } => n.addr.page_item_id() == Some(id.as_str()),
            Address::PageNumber { n: want } => n.page_number == Some(*want),
            Address::Style { family, id } => match &n.addr {
                Address::Style {
                    family: f,
                    id: have,
                } => f == family && (have == id || n.name == Some(id.as_str())),
                _ => false,
            },
            Address::OnMaster { master, item } => match &n.addr {
                Address::OnMaster { master: m, .. } => m == master && self.address_matches(n, item),
                _ => false,
            },
            a if a.kind().is_page_item() => {
                n.addr.kind() == a.kind() && n.addr.page_item_id() == a.page_item_id()
            }
            a => &n.addr == a,
        }
    }

    fn filter_matches(&self, n: &Node<'_>, f: &Filter) -> bool {
        match f {
            Filter::Name(op, v) => op.test(n.name, v),
            Filter::Label { key, op, value } => {
                let found = n
                    .labels
                    .iter()
                    .rev()
                    .find(|(k, _)| decode_aid(k) == *key)
                    .map(|(_, v)| decode_aid(v));
                match value {
                    None => found.is_some(),
                    Some(v) => op.test(found.as_deref(), v),
                }
            }
            Filter::Kind(k) => kind_matches(n, *k),
            Filter::Layer(op, v) => id_or_name(
                *op,
                n.layer,
                n.layer
                    .and_then(|l| self.layer_names.get(l).copied().flatten()),
                v,
            ),
            Filter::Style(op, v) => id_or_name(
                *op,
                n.style,
                n.style
                    .and_then(|s| self.style_names.get(s).copied().flatten()),
                v,
            ),
            Filter::Nth(_) => true,
        }
    }

    fn step_matches(&self, i: usize, step: &Step) -> bool {
        let n = &self.nodes[i];
        self.head_matches(n, &step.head) && step.filters.iter().all(|f| self.filter_matches(n, f))
    }

    /// Evaluate a selector: matching addresses, document order within a
    /// chain, chains in written order, no duplicates.
    pub fn query(&self, selector: &Selector) -> Vec<Address> {
        let mut out: Vec<Address> = Vec::new();
        let mut emitted: HashSet<Address> = HashSet::new();
        for chain in &selector.chains {
            // A one-step chain naming an address core does not enumerate
            // is answered by the address itself, when it is valid here.
            if let [(Combinator::Root, step)] = chain.steps.as_slice() {
                if let Head::Address(a) = &step.head {
                    if step.filters.is_empty() {
                        if let Some(a) = self.passthrough(a) {
                            if emitted.insert(a.clone()) {
                                out.push(a);
                            }
                            continue;
                        }
                    }
                }
            }
            let mut ctx: Vec<usize> = Vec::new();
            for (k, (comb, step)) in chain.steps.iter().enumerate() {
                let groups: Vec<Vec<usize>> = match comb {
                    Combinator::Root => vec![(0..self.nodes.len()).collect()],
                    Combinator::Child => ctx.iter().map(|&c| self.children[c].clone()).collect(),
                    Combinator::Descendant => ctx
                        .iter()
                        .map(|&c| {
                            let mut v = Vec::new();
                            self.descendants(c, &mut v, &mut HashSet::new());
                            v
                        })
                        .collect(),
                };
                let nth = step.filters.iter().rev().find_map(|f| match f {
                    Filter::Nth(n) => Some(*n as usize),
                    _ => None,
                });
                let mut next: Vec<usize> = Vec::new();
                for group in groups {
                    let matched = group.into_iter().filter(|&i| self.step_matches(i, step));
                    match nth {
                        Some(n) => next.extend(matched.skip(n - 1).take(1)),
                        None => next.extend(matched),
                    }
                }
                next.sort_unstable();
                next.dedup();
                ctx = next;
                if ctx.is_empty() && k + 1 < chain.steps.len() {
                    break;
                }
            }
            for i in ctx {
                let a = self.nodes[i].addr.clone();
                if emitted.insert(a.clone()) {
                    out.push(a);
                }
            }
        }
        out
    }

    /// Addresses that are answers in their own right rather than nodes:
    /// a story range of a story that exists, a plugin object (opaque,
    /// carried through), a batch handle.
    fn passthrough(&self, a: &Address) -> Option<Address> {
        match a {
            Address::StoryRange { story_id, .. } => {
                self.stories.contains(story_id.as_str()).then(|| a.clone())
            }
            Address::Plugin { .. } | Address::Handle { .. } => Some(a.clone()),
            _ => None,
        }
    }

    /// The canonical address of the object `a` names, if it exists.
    pub fn resolve(&self, a: &Address) -> Option<Address> {
        if let Some(p) = self.passthrough(a) {
            return Some(p);
        }
        self.nodes
            .iter()
            .find(|n| self.address_matches(n, a))
            .map(|n| n.addr.clone())
    }

    /// The `ElementId` an address names in this document, tables and
    /// cells included (their story is found here).
    pub fn resolve_element_id(&self, a: &Address) -> Option<ElementId> {
        let resolved = self.resolve(a)?;
        if let Some(id) = resolved.to_element_id() {
            return Some(id);
        }
        let i = self.nodes.iter().position(|n| n.addr == resolved)?;
        let story_of = |mut i: usize| -> Option<String> {
            for _ in 0..4 {
                match &self.nodes[i].addr {
                    Address::Story { id } => return Some(id.clone()),
                    _ => i = *self.nodes[i].parents.first()?,
                }
            }
            None
        };
        match resolved {
            Address::Table { id } => Some(ElementId::Table {
                story_id: story_of(i)?,
                table_id: id,
            }),
            Address::Cell { table_id, row, col } => Some(ElementId::TableCell {
                story_id: story_of(i)?,
                table_id,
                row,
                col,
            }),
            _ => None,
        }
    }

    /// ADR 131 → ADR 132: the engine target an address names in this
    /// document. Total over the address kinds — every kind either maps
    /// (directly, or through this index for `frame:`, `page:#n`, a style
    /// named by display name, and a table or cell whose story the index
    /// finds) or answers why it cannot (a plugin object, an unbound
    /// handle, an object this document does not have).
    pub fn to_virtual_target(&self, a: &Address) -> Result<VirtualTarget, String> {
        if let Some(t) = VirtualTarget::from_address_direct(a)? {
            return Ok(t);
        }
        let missing = || format!("{a} names nothing in this document");
        let resolved = self.resolve(a).ok_or_else(missing)?;
        match &resolved {
            Address::Page { id } => Ok(VirtualTarget::Page {
                page_id: id.clone(),
            }),
            Address::Style { family, id } => Ok(VirtualTarget::Style {
                family: *family,
                style_id: id.clone(),
            }),
            Address::Table { .. } | Address::Cell { .. } => {
                match self.resolve_element_id(&resolved).ok_or_else(missing)? {
                    ElementId::Table { story_id, table_id } => {
                        Ok(VirtualTarget::Table { story_id, table_id })
                    }
                    ElementId::TableCell {
                        story_id,
                        table_id,
                        row,
                        col,
                    } => Ok(VirtualTarget::Cell {
                        story_id,
                        table_id,
                        row,
                        col,
                    }),
                    _ => Err(missing()),
                }
            }
            other => VirtualTarget::from_address_direct(other)?.ok_or_else(missing),
        }
    }
}

/// [`Index::to_virtual_target`] over a fresh index.
pub fn to_virtual_target(doc: &Document, a: &Address) -> Result<VirtualTarget, String> {
    Index::build(doc).to_virtual_target(a)
}

fn kind_matches(n: &Node<'_>, k: AddressKind) -> bool {
    let have = n.addr.kind();
    if k == AddressKind::Frame {
        have.is_page_item()
    } else {
        have == k
    }
}

fn id_or_name(op: Op, id: Option<&str>, name: Option<&str>, v: &str) -> bool {
    match op {
        Op::Ne => id != Some(v) && name != Some(v),
        Op::Exists => id.is_some(),
        op => op.test(id, v) || op.test(name, v),
    }
}

/// Evaluate `selector` against `doc`.
pub fn query(doc: &Document, selector: &Selector) -> Vec<Address> {
    Index::build(doc).query(selector)
}

/// Parse and evaluate a selector string.
pub fn query_str(doc: &Document, selector: &str) -> Result<Vec<Address>, GrammarError> {
    Ok(query(doc, &Selector::parse(selector)?))
}

/// The canonical address `a` names in `doc` (`frame:` → kind, `page:#n` →
/// id, a style name → its `Self` id), or `None` when nothing matches.
pub fn resolve(doc: &Document, a: &Address) -> Option<Address> {
    Index::build(doc).resolve(a)
}

/// [`Index::resolve_element_id`] over a fresh index.
pub fn resolve_element_id(doc: &Document, a: &Address) -> Option<ElementId> {
    Index::build(doc).resolve_element_id(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_model::{
        Bounds, CharacterRun, Group, Layer, Page, Paragraph, ParagraphStyleDef, PlaceholderField,
        Rectangle, Story, Table, TableCell, TextFrame,
    };
    use paged_scene::{ParsedMasterSpread, ParsedSpread, ParsedStory};

    fn b(top: f32, left: f32, bottom: f32, right: f32) -> Bounds {
        Bounds {
            top,
            left,
            bottom,
            right,
        }
    }

    fn label(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    /// Two pages side by side (0..100 and 100..200), a named text frame
    /// on each, a labelled rectangle and a group (named) of an oval on
    /// page 2, a master with one frame, a story with a table and a
    /// placeholder, two styles and a layer.
    fn doc() -> Document {
        let mut s = Spread {
            self_id: Some("sp1".into()),
            ..Default::default()
        };
        s.pages = vec![
            Page::new("p1", b(0.0, 0.0, 100.0, 100.0)),
            Page::new("p2", b(0.0, 100.0, 100.0, 200.0)),
        ];
        s.pages[1].name = Some("2".into());
        let mut t1 = TextFrame::new("tf1", Some("st1".into()), b(10.0, 10.0, 20.0, 20.0));
        t1.item_layer = Some("ly1".into());
        let t2 = TextFrame::new("tf2", None, b(10.0, 110.0, 20.0, 120.0));
        let mut r = Rectangle::new("r1", b(30.0, 130.0, 40.0, 140.0));
        r.applied_object_style = Some("ObjectStyle/Box".into());
        s.text_frames = vec![t1, t2];
        s.rectangles = vec![r];
        s.ovals = vec![paged_model::Oval::new("o1", b(50.0, 150.0, 60.0, 160.0))];
        s.groups = vec![Group {
            self_id: Some("g1".into()),
            members: vec![FrameRef::Oval(0)],
            ..Default::default()
        }];
        s.frames_in_order = vec![
            FrameRef::TextFrame(0),
            FrameRef::TextFrame(1),
            FrameRef::Rectangle(0),
            FrameRef::Group(0),
        ];
        s.item_names.insert("tf1".into(), "headline".into());
        s.item_names.insert("tf2".into(), "price".into());
        s.item_names.insert("g1".into(), "badge".into());
        s.labels.insert(
            "r1".into(),
            vec![
                label("x-paged:oid", "a1b2"),
                label("vendor", "acme"),
                label("emoji", "<?AID d83d?><?AID de00?>!"),
            ],
        );
        s.labels
            .insert("p2".into(), vec![label("x-paged:slide", "{}")]);

        let mut ms = Spread {
            text_frames: vec![TextFrame::new("mtf", None, b(0.0, 0.0, 5.0, 5.0))],
            frames_in_order: vec![FrameRef::TextFrame(0)],
            ..Default::default()
        };
        ms.item_names.insert("mtf".into(), "folio".into());

        let cell = TableCell {
            name: Some("1:0".into()),
            ..Default::default()
        };
        let table = Table {
            self_id: Some("tb1".into()),
            cells: vec![cell],
            ..Default::default()
        };
        let story = Story {
            paragraphs: vec![
                Paragraph {
                    runs: vec![
                        CharacterRun {
                            text: "Hi ".into(),
                            ..Default::default()
                        },
                        CharacterRun {
                            text: "<price>".into(),
                            placeholder: Some(PlaceholderField {
                                plugin: "paged.data".into(),
                                key: "price".into(),
                                value: None,
                            }),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
                Paragraph {
                    table: Some(table),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let mut d = Document::default();
        d.designmap.document_name = Some("Brochure".into());
        d.designmap.labels = vec![label("x-paged:data", "{}")];
        d.designmap.layers = vec![Layer {
            self_id: "ly1".into(),
            name: Some("Text".into()),
            visible: true,
            locked: false,
            printable: true,
            parent_id: None,
        }];
        d.spreads = vec![ParsedSpread {
            src: "Spreads/Spread_sp1.xml".into(),
            spread: s,
        }];
        d.master_spreads.insert(
            "ma".into(),
            ParsedMasterSpread {
                src: "MasterSpreads/MasterSpread_ma.xml".into(),
                self_id: "ma".into(),
                spread: ms,
                name: Some("A-Master".into()),
            },
        );
        d.stories = vec![ParsedStory {
            src: "Stories/Story_st1.xml".into(),
            self_id: "st1".into(),
            story,
        }];
        for (id, name) in [
            ("ParagraphStyle/Body", "Body"),
            ("ParagraphStyle/Head 1", "Head 1"),
        ] {
            d.styles.paragraph_styles.insert(
                id.into(),
                ParagraphStyleDef {
                    self_id: id.into(),
                    name: Some(name.into()),
                    ..Default::default()
                },
            );
        }
        d.rebuild_indexes();
        d
    }

    fn q(d: &Document, s: &str) -> Vec<String> {
        query_str(d, s)
            .unwrap_or_else(|e| panic!("{s}: {e}"))
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn kinds_enumerate_in_document_order() {
        let d = doc();
        assert_eq!(
            q(&d, "frame"),
            [
                "textFrame:tf1",
                "textFrame:tf2",
                "rectangle:r1",
                "group:g1",
                "oval:o1",
                "master:ma > textFrame:mtf"
            ]
        );
        assert_eq!(q(&d, "page"), ["page:p1", "page:p2"]);
        assert_eq!(q(&d, "doc"), ["doc"]);
        assert_eq!(q(&d, "table, cell"), ["table:tb1", "cell:tb1/0,1"]);
        assert_eq!(q(&d, "field"), ["field:st1@3"]);
        assert_eq!(
            q(&d, "style"),
            [
                "style:paragraph/ParagraphStyle/Body",
                "style:paragraph/ParagraphStyle/Head 1"
            ]
        );
    }

    #[test]
    fn name_selects_every_item_kind_groups_included() {
        let d = doc();
        assert_eq!(q(&d, "frame[name=price]"), ["textFrame:tf2"]);
        assert_eq!(q(&d, "[name=badge]"), ["group:g1"]);
        assert_eq!(q(&d, "frame[name=folio]"), ["master:ma > textFrame:mtf"]);
        assert_eq!(q(&d, "frame[name^=he]"), ["textFrame:tf1"]);
        assert_eq!(
            q(&d, "textFrame[name!=price]"),
            ["textFrame:tf1", "master:ma > textFrame:mtf"]
        );
        assert_eq!(
            q(&d, "style[name='Head 1']"),
            ["style:paragraph/ParagraphStyle/Head 1"]
        );
        assert_eq!(q(&d, "field[name=price]"), ["field:st1@3"]);
        assert_eq!(q(&d, "master[name=A-Master]"), ["master:ma"]);
    }

    #[test]
    fn labels_select_by_key_and_value_and_decode_aid() {
        let d = doc();
        assert_eq!(q(&d, "[label=x-paged:oid]"), ["rectangle:r1"]);
        assert_eq!(q(&d, "*[label=vendor=acme]"), ["rectangle:r1"]);
        assert_eq!(
            q(&d, r#"frame[label.x-paged:oid="a1b2"]"#),
            ["rectangle:r1"]
        );
        assert_eq!(q(&d, "[label.vendor=nope]"), Vec::<String>::new());
        assert_eq!(q(&d, "page[label=x-paged:slide]"), ["page:p2"]);
        assert_eq!(q(&d, "doc[label=x-paged:data]"), ["doc"]);
        assert_eq!(q(&d, "[label.emoji=😀!]"), ["rectangle:r1"]);
    }

    #[test]
    fn containment_child_and_descendant() {
        let d = doc();
        assert_eq!(q(&d, "page:p1 > frame"), ["textFrame:tf1"]);
        assert_eq!(
            q(&d, "page:#2 > frame"),
            ["textFrame:tf2", "rectangle:r1", "group:g1"]
        );
        assert_eq!(
            q(&d, "page:#2 frame"),
            ["textFrame:tf2", "rectangle:r1", "group:g1", "oval:o1"]
        );
        assert_eq!(q(&d, "group > *"), ["oval:o1"]);
        assert_eq!(q(&d, "spread > page"), ["page:p1", "page:p2"]);
        assert_eq!(q(&d, "layer:ly1 > frame"), ["textFrame:tf1"]);
        assert_eq!(q(&d, "frame[layer=Text]"), ["textFrame:tf1"]);
        assert_eq!(q(&d, "story:st1 > table > cell"), ["cell:tb1/0,1"]);
        assert_eq!(q(&d, "master:ma > frame"), ["master:ma > textFrame:mtf"]);
        assert_eq!(q(&d, "page:#2 > frame:nth(2)"), ["rectangle:r1"]);
        assert_eq!(q(&d, "rectangle[style=Box]"), Vec::<String>::new());
        assert_eq!(q(&d, "rectangle[style=ObjectStyle/Box]"), ["rectangle:r1"]);
    }

    #[test]
    fn addresses_resolve() {
        let d = doc();
        assert_eq!(q(&d, "frame:o1"), ["oval:o1"]);
        // A space ends a bare token: `1` is read as a descendant step.
        assert_eq!(
            query_str(&d, "style:paragraph/Head 1")
                .unwrap_err()
                .position,
            21
        );
        assert_eq!(
            q(&d, "\"style:paragraph/Head 1\""),
            ["style:paragraph/ParagraphStyle/Head 1"]
        );
        assert_eq!(q(&d, "storyRange:st1@0..2"), ["storyRange:st1@0..2"]);
        assert_eq!(q(&d, "storyRange:nope@0..2"), Vec::<String>::new());
        assert_eq!(
            q(&d, "plugin:paged.image/layer/7"),
            ["plugin:paged.image/layer/7"]
        );
        assert_eq!(q(&d, "textFrame:gone"), Vec::<String>::new());
        assert_eq!(
            resolve_element_id(&d, &Address::parse("cell:tb1/0,1").unwrap()),
            Some(ElementId::TableCell {
                story_id: "st1".into(),
                table_id: "tb1".into(),
                row: 0,
                col: 1
            })
        );
        assert_eq!(
            resolve(&d, &Address::parse("frame:mtf").unwrap()).map(|a| a.to_string()),
            Some("master:ma > textFrame:mtf".into())
        );
        assert_eq!(
            resolve_element_id(&d, &Address::parse("frame:g1").unwrap()),
            Some(ElementId::Group("g1".into()))
        );
    }

    #[test]
    fn unions_dedupe_and_keep_written_order() {
        let d = doc();
        assert_eq!(q(&d, "oval, frame:o1, group"), ["oval:o1", "group:g1"]);
    }

    #[test]
    fn aid_decoding() {
        assert_eq!(decode_aid("<?AID d83d?><?AID de00?>"), "😀");
        assert_eq!(decode_aid("a<?AID d83d?><?AID de00?>b"), "a😀b");
        assert_eq!(decode_aid("plain"), "plain");
        assert_eq!(decode_aid("<?AID zz?>"), "<?AID zz?>");
        assert_eq!(
            decode_aid("<?AID d83d?>x"),
            "<?AID d83d?>x",
            "a lone surrogate stays verbatim"
        );
    }
}
