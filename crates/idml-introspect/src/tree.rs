//! Scene-graph tree as the inspector's left-pane renders it.
//!
//! Hierarchy: [`InspectorTree`] → [`SpreadEntry`] → [`PageEntry`] →
//! [`FrameEntry`]. Each level carries enough info for the React tree
//! widget to show + select.
//!
//! Today only TextFrames + Rectangles surface as frames; Ovals,
//! Polygons, GraphicLines, Groups land as the inspector's property
//! coverage extends.

use idml_mutate::NodeId;
use idml_scene::Document;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct InspectorTree {
    pub spreads: Vec<SpreadEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpreadEntry {
    pub index: usize,
    pub label: String,
    pub pages: Vec<PageEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageEntry {
    pub index: usize,
    pub label: String,
    pub frames: Vec<FrameEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameEntry {
    pub id: NodeIdJson,
    pub label: String,
}

/// JSON-friendly mirror of `idml_mutate::NodeId` — wasm-bindgen
/// serialises this directly rather than reaching into idml-mutate's
/// enum, so the wire format stays stable as new node kinds land.
///
/// `Spread` and `Page` are addressable as parents in `InsertNode`/
/// `MoveNode` Ops, even though the inspector tree's left pane today
/// only surfaces the page-item variants as selectable rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id")]
pub enum NodeIdJson {
    TextFrame(String),
    Rectangle(String),
    Oval(String),
    Polygon(String),
    GraphicLine(String),
    Group(String),
    Spread(String),
    Page(String),
}

impl From<NodeId> for NodeIdJson {
    fn from(value: NodeId) -> Self {
        match value {
            NodeId::TextFrame(s) => NodeIdJson::TextFrame(s),
            NodeId::Rectangle(s) => NodeIdJson::Rectangle(s),
            NodeId::Oval(s) => NodeIdJson::Oval(s),
            NodeId::Polygon(s) => NodeIdJson::Polygon(s),
            NodeId::GraphicLine(s) => NodeIdJson::GraphicLine(s),
            NodeId::Group(s) => NodeIdJson::Group(s),
            NodeId::Spread(s) => NodeIdJson::Spread(s),
            NodeId::Page(s) => NodeIdJson::Page(s),
        }
    }
}

impl From<&NodeIdJson> for NodeId {
    fn from(value: &NodeIdJson) -> Self {
        match value {
            NodeIdJson::TextFrame(s) => NodeId::TextFrame(s.clone()),
            NodeIdJson::Rectangle(s) => NodeId::Rectangle(s.clone()),
            NodeIdJson::Oval(s) => NodeId::Oval(s.clone()),
            NodeIdJson::Polygon(s) => NodeId::Polygon(s.clone()),
            NodeIdJson::GraphicLine(s) => NodeId::GraphicLine(s.clone()),
            NodeIdJson::Group(s) => NodeId::Group(s.clone()),
            NodeIdJson::Spread(s) => NodeId::Spread(s.clone()),
            NodeIdJson::Page(s) => NodeId::Page(s.clone()),
        }
    }
}

pub fn build_tree(document: &Document) -> InspectorTree {
    let spreads = document
        .spreads
        .iter()
        .enumerate()
        .map(|(spread_index, parsed)| {
            let pages = parsed
                .spread
                .pages
                .iter()
                .enumerate()
                .map(|(page_index, page)| {
                    let label = page
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Page {}", page_index + 1));
                    // For the M0 inspector we don't yet do per-page
                    // containment routing on the *tree* (the renderer
                    // does its own routing pass). Every frame in the
                    // spread shows up under page 0; once the inspector
                    // grows multi-page coverage we'll route by centroid
                    // here too. This keeps the tree faithful to the
                    // IDML serialisation order for now.
                    let frames = if page_index == 0 {
                        frames_for_spread(parsed)
                    } else {
                        Vec::new()
                    };
                    PageEntry {
                        index: page_index,
                        label,
                        frames,
                    }
                })
                .collect();
            SpreadEntry {
                index: spread_index,
                label: format!("Spread {}", spread_index + 1),
                pages,
            }
        })
        .collect();

    InspectorTree { spreads }
}

fn frames_for_spread(parsed: &idml_scene::ParsedSpread) -> Vec<FrameEntry> {
    let mut frames = Vec::new();
    for frame in &parsed.spread.text_frames {
        if let Some(self_id) = frame.self_id.clone() {
            frames.push(FrameEntry {
                label: format!("TextFrame {}", short_id(&self_id)),
                id: NodeIdJson::TextFrame(self_id),
            });
        }
    }
    for rect in &parsed.spread.rectangles {
        if let Some(self_id) = rect.self_id.clone() {
            frames.push(FrameEntry {
                label: format!("Rectangle {}", short_id(&self_id)),
                id: NodeIdJson::Rectangle(self_id),
            });
        }
    }
    frames
}

/// `TextFrame/u123` → `u123` for compact tree labels.
fn short_id(self_id: &str) -> &str {
    self_id.rsplit_once('/').map_or(self_id, |(_, tail)| tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::document_with_one_textframe;

    #[test]
    fn build_tree_lists_one_frame_under_one_page() {
        let doc = document_with_one_textframe("TextFrame/u1");
        let tree = build_tree(&doc);
        assert_eq!(tree.spreads.len(), 1);
        assert_eq!(tree.spreads[0].pages.len(), 1);
        assert_eq!(tree.spreads[0].pages[0].frames.len(), 1);
        let frame = &tree.spreads[0].pages[0].frames[0];
        assert_eq!(frame.label, "TextFrame u1");
        match &frame.id {
            NodeIdJson::TextFrame(s) => assert_eq!(s, "TextFrame/u1"),
            _ => panic!("expected TextFrame variant"),
        }
    }
}
