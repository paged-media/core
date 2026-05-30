//! Test fixtures shared across the introspect tests.

use std::collections::{BTreeMap, HashMap};

use bytes::Bytes;
use idml_parse::{Bounds, Container, DesignMap, Graphic, Spread, StyleSheet, TextFrame};
use idml_scene::{Document, ParsedSpread};

pub fn empty_text_frame(self_id: &str, bounds: Bounds) -> TextFrame {
    TextFrame {
        self_id: Some(self_id.to_string()),
        parent_story: None,
        bounds,
        item_transform: None,
        fill_color: None,
        fill_tint: None,
        stroke_color: None,
        stroke_weight: None,
        stroke_type: None,
        drop_shadow: None,
        stroke_drop_shadow: None,
        next_text_frame: None,
        vertical_justification: None,
        first_baseline_offset: None,
        minimum_first_baseline_offset: None,
        inset_spacing: None,
        auto_sizing: None,
        auto_sizing_reference_point: None,
        minimum_width_for_auto_sizing: None,
        minimum_height_for_auto_sizing: None,
        use_minimum_height_for_auto_sizing: None,
        applied_object_style: None,
        text_wrap: None,
        item_layer: None,
        is_anchored: false,
        opacity: None,
        blend_mode: None,
        anchors: Vec::new(),
        subpath_starts: Vec::new(),
        subpath_open: Vec::new(),
        effects: None,
        gradient_fill_angle: None,
        gradient_fill_length: None,
        gradient_stroke_angle: None,
        gradient_stroke_length: None,
        applied_toc_style: None,
        overprint_fill: false,
        overprint_stroke: false,
        nonprinting: false,
    }
}

pub fn document_with_one_textframe(self_id: &str) -> Document {
    let mut spread = Spread::default();
    spread.text_frames.push(empty_text_frame(
        self_id,
        Bounds {
            top: 0.0,
            left: 0.0,
            bottom: 100.0,
            right: 200.0,
        },
    ));
    // Pages need to exist so build_tree's "frames live under page 0"
    // assignment has a page to attach to.
    spread.pages.push(idml_parse::Page {
        self_id: Some("Page/u1".to_string()),
        name: Some("1".to_string()),
        bounds: Bounds {
            top: 0.0,
            left: 0.0,
            bottom: 200.0,
            right: 300.0,
        },
        item_transform: None,
        applied_master: None,
        master_page_transform: None,
        override_list: Vec::new(),
    });
    Document {
        container: Container {
            mimetype: "application/vnd.adobe.indesign-idml-package".to_string(),
            designmap_raw: Bytes::new(),
            designmap: DesignMap::default(),
            entries: BTreeMap::new(),
        },
        palette: Graphic::default(),
        spreads: vec![ParsedSpread {
            src: "Spreads/syn.xml".to_string(),
            spread,
        }],
        stories: Vec::new(),
        master_spreads: HashMap::new(),
        frame_for_story: HashMap::new(),
        text_frame_index: HashMap::new(),
        styles: StyleSheet::default(),
        anchors: Vec::new(),
    }
}
