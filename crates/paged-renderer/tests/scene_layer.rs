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

//! C-1 — the plugin scene-layer splice, end-to-end through the real
//! pipeline (CPU lane; no GPU, no corpus). Proves the `PipelineOptions
//! ::scene_layers` registry threads through `build_document`, matches a
//! frame by its `Self` id, and lowers the layer's commands into that
//! frame's display list.

use std::collections::HashMap;

use paged_compose::{DisplayCommand, SceneItem, SceneLayer, ScenePaint, ScenePathSeg};
use paged_renderer::{pipeline, Document, PipelineOptions};

fn total_commands(built: &pipeline::BuiltDocument) -> usize {
    built.pages.iter().map(|p| p.list.commands.len()).sum()
}

fn one_fill_layer() -> SceneLayer {
    SceneLayer {
        items: vec![SceneItem::FillPath {
            path: vec![
                ScenePathSeg::MoveTo { x: 0.0, y: 0.0 },
                ScenePathSeg::LineTo { x: 10.0, y: 0.0 },
                ScenePathSeg::LineTo { x: 10.0, y: 10.0 },
                ScenePathSeg::Close,
            ],
            paint: ScenePaint {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        }],
    }
}

fn sample_doc() -> Document {
    let idml = paged_gen::write_idml(&paged_gen::samples::text::build()).unwrap();
    Document::open(&idml).unwrap()
}

fn first_text_frame_id(doc: &Document) -> String {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find_map(|f| f.self_id.clone())
        .expect("the text sample has a text frame with a Self id")
}

#[test]
fn scene_layer_splices_into_a_frame_by_self_id() {
    let doc = sample_doc();
    let id = first_text_frame_id(&doc);

    // Baseline: no registry.
    let base = pipeline::build_document(&doc, &PipelineOptions::default()).unwrap();
    let base_n = total_commands(&base);

    // With a one-fill layer bound to the frame's id.
    let mut reg = HashMap::new();
    reg.insert(id.clone(), one_fill_layer());
    let opts = PipelineOptions {
        scene_layers: Some(&reg),
        ..PipelineOptions::default()
    };
    let withed = pipeline::build_document(&doc, &opts).unwrap();

    // PushClip + FillPath + PopClip = exactly +3 commands.
    assert_eq!(
        total_commands(&withed),
        base_n + 3,
        "a one-fill scene layer splices PushClip+FillPath+PopClip into the frame"
    );

    // The spliced commands are present somewhere in the document.
    let has_clip = withed
        .pages
        .iter()
        .any(|p| p.list.commands.iter().any(|c| matches!(c, DisplayCommand::PushClip { .. })));
    assert!(has_clip, "the layer brackets its content in a content-box clip");
}

#[test]
fn unmatched_id_splices_nothing() {
    let doc = sample_doc();
    let base_n = total_commands(&pipeline::build_document(&doc, &PipelineOptions::default()).unwrap());

    let mut reg = HashMap::new();
    reg.insert("media.paged.sheet.no-such-frame".to_string(), one_fill_layer());
    let opts = PipelineOptions {
        scene_layers: Some(&reg),
        ..PipelineOptions::default()
    };
    let built = pipeline::build_document(&doc, &opts).unwrap();
    assert_eq!(
        total_commands(&built),
        base_n,
        "a registry whose ids match no frame leaves the render untouched"
    );
}

#[test]
fn empty_registry_is_identical_to_no_registry() {
    let doc = sample_doc();
    let base = pipeline::build_document(&doc, &PipelineOptions::default()).unwrap();
    let empty: HashMap<String, SceneLayer> = HashMap::new();
    let opts = PipelineOptions {
        scene_layers: Some(&empty),
        ..PipelineOptions::default()
    };
    let with_empty = pipeline::build_document(&doc, &opts).unwrap();
    assert_eq!(
        total_commands(&base),
        total_commands(&with_empty),
        "an empty registry is the no-plugin path"
    );
}
