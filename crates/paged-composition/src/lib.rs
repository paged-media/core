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

//! The Paged-native **composition model** (`document.pgd`) — the core-owned
//! *arrangement* of a `.paged` document (ADR-021; `thoughts/docs/paged/core/
//! composition-format.md`).
//!
//! **Not to be confused with `paged-compose`** (the display-list / `SceneLayer`
//! IR one layer down). This crate is the *document model*: how content is
//! placed, ordered, flowed, and projected onto surfaces. It holds **no
//! content** — no text runs, no cell values, no pixels — only arrangement:
//! surfaces, pages, **regions** that bind (a selector into) a content part into
//! a positioned geometry, region-chains (flows), and a positioning-constraint
//! system. Content lives in parts, referenced by handle.
//!
//! It is deliberately dependency-light — `paged-flow` + `serde`, **no IDML**
//! (`paged-parse`/`paged-scene`) — so a composition can be hand-built and
//! serialized with no IDML involved (the `renderer.md` §C1 discipline at the
//! model layer). The flow protocol ([`paged_flow`]) is *part of* the
//! composition: [`Composition::flow_chain`] projects a flow's regions into a
//! [`paged_flow::RegionChain`] the driver consumes — the native analogue of
//! `paged_scene::Document::flow_chain`.
//!
//! Slice 1 (this file) is the **arrangement kernel**: surfaces, flows, pages,
//! the region/group/layer node tree, positioning, and JSON serialization.
//! Templates/instances, the IDML↔composition adapter, and driving the
//! compositor are later slices.

use paged_flow::{FlowId, RegionChain, RegionGeometry, RegionId};
use serde::{Deserialize, Serialize};

/// The canonical container path of the composition part inside a `.paged`
/// document (container-format-v2 §3.1). A [`Composition`] serialized as JSON
/// lives here; `paged/core/` is a core-owned namespace. See S7 in
/// `thoughts/docs/paged/core/flow-implementation-plan.md`.
pub const DOCUMENT_PGD_PATH: &str = "paged/core/composition/document.pgd";

fn default_format() -> String {
    "paged-composition".to_string()
}

/// A reference to a content **part** in the `.paged` container (e.g.
/// `"publishing"`, `"media.paged.web/o3"`, `"media.paged.sheet/o1"`). Opaque
/// to the composition — the content engine owns the part's bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PartRef(pub String);

impl PartRef {
    pub fn new(id: impl Into<String>) -> Self {
        PartRef(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The composition document — `document.pgd`. A small tree of **arrangement**
/// nodes with stable ids; holds no content (composition-format §0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Composition {
    /// Format tag; always `"paged-composition"`.
    #[serde(default = "default_format")]
    pub format: String,
    /// Producer version (container-v2 §5.1).
    pub version: u32,
    /// Capabilities this composition uses (`flow.regionChain@1`, …).
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// The surface set; exactly one should be the print projection (§6).
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    /// Named content flows a region-chain fragments (§5).
    #[serde(default)]
    pub flows: Vec<Flow>,
    /// Pages/spreads.
    #[serde(default)]
    pub pages: Vec<Page>,
    /// The region/group/layer tree (roots).
    #[serde(default)]
    pub nodes: Vec<Node>,
}

impl Composition {
    /// An empty composition at the given version.
    pub fn new(version: u32) -> Self {
        Composition {
            format: default_format(),
            version,
            capabilities: Vec::new(),
            surfaces: Vec::new(),
            flows: Vec::new(),
            pages: Vec::new(),
            nodes: Vec::new(),
        }
    }

    /// Project a flow into a content-agnostic [`RegionChain`]: the regions in
    /// the node tree tagged with `flow`, in document order. This is what the
    /// [`paged_flow::run_flow`] driver consumes — the native analogue of
    /// `paged_scene::Document::flow_chain`, with no IDML. Regions past a group
    /// or layer are included (the walk descends the tree).
    pub fn flow_chain(&self, flow: &FlowId) -> RegionChain {
        let mut regions = Vec::new();
        collect_flow_regions(&self.nodes, flow, &mut regions);
        RegionChain::new(flow.clone(), regions)
    }

    /// Every `Region` in the tree, in document order.
    pub fn regions(&self) -> Vec<&Region> {
        let mut out = Vec::new();
        collect_regions(&self.nodes, &mut out);
        out
    }
}

fn collect_flow_regions(nodes: &[Node], flow: &FlowId, out: &mut Vec<paged_flow::Region>) {
    for node in nodes {
        match node {
            Node::Region(r) => {
                if r.flow.as_ref() == Some(flow) {
                    out.push(paged_flow::Region {
                        id: r.id.clone(),
                        geometry: r.geometry,
                    });
                }
            }
            Node::Group(g) => collect_flow_regions(&g.children, flow, out),
            Node::Layer(l) => collect_flow_regions(&l.children, flow, out),
        }
    }
}

fn collect_regions<'a>(nodes: &'a [Node], out: &mut Vec<&'a Region>) {
    for node in nodes {
        match node {
            Node::Region(r) => out.push(r),
            Node::Group(g) => collect_regions(&g.children, out),
            Node::Layer(l) => collect_regions(&l.children, out),
        }
    }
}

/// A surface the composition projects onto (§6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Surface {
    pub id: String,
    pub kind: SurfaceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SurfaceKind {
    /// The deterministic, CMYK-exact projection the fidelity gate + PDF/IDML
    /// export target. Exactly one surface should be `Print`.
    Print,
    /// An interactive screen surface.
    Screen,
}

/// A named content sequence in a part that a region-chain fragments (§5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flow {
    pub id: FlowId,
    /// The part whose content flows.
    pub part: PartRef,
    /// An engine-specific selector into the part (`"story/s7"`, `"flow:main"`).
    pub selector: String,
}

/// A page/spread. (Master/template instances are a later slice.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub id: String,
    /// Page size in pt `[width, height]`.
    pub size: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<String>,
}

/// A node in the arrangement tree. `kind` is the serde tag, so the JSON is
/// `{ "kind": "region", … }` (composition-format §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Node {
    Region(Region),
    Group(Group),
    Layer(Layer),
}

/// The **placement unit**: places (a selector into) a content part into a
/// geometry on a surface (§3). Holds no content — it *windows* a part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub id: RegionId,
    /// What content this region places (or `none` for a decoration region).
    #[serde(default)]
    pub bind: Bind,
    /// The positioning constraint (§4) — not raw bounds.
    pub position: Position,
    /// The region's content-box geometry (width/height/columns), from
    /// [`paged_flow`].
    pub geometry: RegionGeometry,
    /// The `Layer` (z-band) this region draws in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// When set, this region is a link in the named flow's region-chain (§5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<FlowId>,
    /// Surfaces/conditions this region is visible on; empty = all (§6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visible_on: Vec<String>,
}

/// What a region binds to (§3). A region never contains content.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Bind {
    /// A decoration-only region (a ruled line, a background swatch).
    #[default]
    None,
    /// A selector into a content part.
    Part { part: PartRef, selector: String },
}

/// One positioning constraint (§4, LESSONS #2). One layout pass resolves every
/// kind into concrete geometry per surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Position {
    PageRelative {
        page: String,
        at: [f32; 2],
    },
    SpreadRelative {
        spread: String,
        at: [f32; 2],
    },
    FrameRelative {
        region: RegionId,
        edge: Edge,
        offset: [f32; 2],
    },
    GridCell {
        grid: String,
        row: u32,
        col: u32,
        span: [u32; 2],
    },
    /// Anchored-to-content: the region's origin is a position inside another
    /// part's content (a figure anchored mid-story).
    Anchor {
        part: PartRef,
        at: String,
    },
    ViewportRelative {
        edge: Edge,
        offset: [f32; 2],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

/// A grouping + shared transform/clip (§2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    #[serde(default)]
    pub children: Vec<Node>,
}

/// A stacking + visibility/lock/print band (replaces IDML's `<Layer>`, §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub id: String,
    #[serde(default)]
    pub z: i32,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default = "yes")]
    pub print: bool,
    #[serde(default)]
    pub children: Vec<Node>,
}

fn yes() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_flow::{run_flow, Overset, Placement, Region as FlowRegion};

    fn region(id: &str, flow: Option<&str>, h: f32) -> Node {
        Node::Region(Region {
            id: RegionId::new(id),
            bind: Bind::Part {
                part: PartRef::new("publishing"),
                selector: "story/article".to_string(),
            },
            position: Position::PageRelative {
                page: "p1".to_string(),
                at: [0.0, 0.0],
            },
            geometry: RegionGeometry::new(200.0, h),
            layer: None,
            flow: flow.map(FlowId::new),
            visible_on: Vec::new(),
        })
    }

    /// E1 (composition-format §exemplars): a print story flowing across three
    /// frames, expressed as a composition — no IDML.
    fn exemplar_e1() -> Composition {
        let mut comp = Composition::new(1);
        comp.capabilities = vec![
            "flow.regionChain@1".to_string(),
            "surface.print@1".to_string(),
        ];
        comp.surfaces = vec![Surface {
            id: "print".to_string(),
            kind: SurfaceKind::Print,
        }];
        comp.flows = vec![Flow {
            id: FlowId::new("article"),
            part: PartRef::new("publishing"),
            selector: "story/article".to_string(),
        }];
        comp.pages = vec![
            Page {
                id: "p1".to_string(),
                size: [612.0, 792.0],
                spread: Some("sp1".to_string()),
            },
            Page {
                id: "p2".to_string(),
                size: [612.0, 792.0],
                spread: Some("sp1".to_string()),
            },
        ];
        comp.nodes = vec![
            region("r0", Some("article"), 300.0),
            region("r1", Some("article"), 300.0),
            region("r2", Some("article"), 300.0),
            // A pull-quote anchored mid-story (LESSONS #2 — an Anchor position).
            Node::Region(Region {
                id: RegionId::new("pullquote"),
                bind: Bind::Part {
                    part: PartRef::new("publishing"),
                    selector: "story/pullquote".to_string(),
                },
                position: Position::Anchor {
                    part: PartRef::new("publishing"),
                    at: "story/article#320".to_string(),
                },
                geometry: RegionGeometry::new(180.0, 90.0),
                layer: None,
                flow: None,
                visible_on: Vec::new(),
            }),
        ];
        comp
    }

    #[test]
    fn json_roundtrips() {
        let comp = exemplar_e1();
        let json = serde_json::to_string_pretty(&comp).unwrap();
        let back: Composition = serde_json::from_str(&json).unwrap();
        assert_eq!(comp, back);
        // The format tag is stable and the shape is `document.pgd`.
        assert!(json.contains("\"format\": \"paged-composition\""));
        assert!(json.contains("\"kind\": \"region\""));
        assert!(json.contains("\"kind\": \"anchor\""));
    }

    #[test]
    fn e1_expresses_the_threaded_feature() {
        let comp = exemplar_e1();
        // One print surface, one flow, four regions (3 threaded + 1 anchored).
        assert_eq!(comp.surfaces.len(), 1);
        assert_eq!(comp.flows.len(), 1);
        assert_eq!(comp.regions().len(), 4);
        // The flow projects to a 3-region chain, in order.
        let chain = comp.flow_chain(&FlowId::new("article"));
        assert_eq!(chain.flow, FlowId::new("article"));
        let ids: Vec<&str> = chain.regions.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["r0", "r1", "r2"]);
        // The pull-quote is NOT in the flow (it's anchored, not threaded).
        assert!(!ids.contains(&"pullquote"));
    }

    /// A synthetic content engine (no IDML): fixed-height lines, to prove the
    /// composition's flow_chain drives the `paged_flow` protocol.
    struct Lines {
        heights: Vec<f32>,
    }
    impl paged_flow::FlowContent for Lines {
        type Fragment = usize; // count placed
        type Cursor = usize;
        fn start(&self) -> usize {
            0
        }
        fn place(&self, region: &FlowRegion, cursor: usize) -> Placement<usize, usize> {
            let mut used = 0.0;
            let mut i = cursor;
            while i < self.heights.len()
                && used + self.heights[i] <= region.geometry.height_pt + 0.01
            {
                used += self.heights[i];
                i += 1;
            }
            let next = if i < self.heights.len() {
                Some(i)
            } else {
                None
            };
            Placement {
                fragment: i - cursor,
                next,
            }
        }
    }

    #[test]
    fn composition_flow_chain_drives_the_protocol() {
        // A composition whose flow spans two regions (heights 30 and 30),
        // grouped + layered to prove the tree walk collects in order.
        let mut comp = Composition::new(1);
        comp.flows = vec![Flow {
            id: FlowId::new("f"),
            part: PartRef::new("publishing"),
            selector: "story/s".to_string(),
        }];
        comp.nodes = vec![
            region("a", Some("f"), 30.0),
            Node::Layer(Layer {
                id: "L".to_string(),
                z: 0,
                visible: true,
                print: true,
                children: vec![region("b", Some("f"), 30.0)],
            }),
        ];
        let chain = comp.flow_chain(&FlowId::new("f"));
        assert_eq!(
            chain.regions.len(),
            2,
            "region under a layer is still collected"
        );

        // Five 10pt lines across two 30pt regions → 3 + 2, fits.
        let content = Lines {
            heights: vec![10.0; 5],
        };
        let run = run_flow(&content, &chain);
        let placed: Vec<usize> = run.placements.iter().map(|(_, n)| *n).collect();
        assert_eq!(placed, vec![3, 2]);
        assert_eq!(run.overset, Overset::Fits);
    }
}
