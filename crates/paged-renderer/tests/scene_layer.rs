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
use std::path::PathBuf;

use paged_compose::{
    DisplayCommand, SceneItem, SceneLayer, ScenePaint, ScenePathSeg, SceneTextItem,
};
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
    idml_import::import_idml_doc(&idml).unwrap()
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
    let has_clip = withed.pages.iter().any(|p| {
        p.list
            .commands
            .iter()
            .any(|c| matches!(c, DisplayCommand::PushClip { .. }))
    });
    assert!(
        has_clip,
        "the layer brackets its content in a content-box clip"
    );
}

#[test]
fn unmatched_id_splices_nothing() {
    let doc = sample_doc();
    let base_n =
        total_commands(&pipeline::build_document(&doc, &PipelineOptions::default()).unwrap());

    let mut reg = HashMap::new();
    reg.insert(
        "media.paged.sheet.no-such-frame".to_string(),
        one_fill_layer(),
    );
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

fn fill_count(built: &pipeline::BuiltDocument) -> usize {
    built
        .pages
        .iter()
        .map(|p| {
            p.list
                .commands
                .iter()
                .filter(|c| matches!(c, DisplayCommand::FillPath { .. }))
                .count()
        })
        .sum()
}

#[test]
fn text_item_emits_glyph_fills_with_a_font() {
    // Corpus-optional (mirrors the other render tests): skip if the test
    // font isn't in this checkout.
    let font_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts/Inter.ttf");
    let Ok(font) = std::fs::read(&font_path) else {
        eprintln!("skipping: {} not present", font_path.display());
        return;
    };
    let doc = sample_doc();
    let id = first_text_frame_id(&doc);

    // Baseline: same default font, no scene layer.
    let base = pipeline::build_document(
        &doc,
        &PipelineOptions {
            font: Some(&font),
            ..PipelineOptions::default()
        },
    )
    .unwrap();

    // A text run "42" → two glyph FillPaths, inside the content-box clip.
    let mut reg = HashMap::new();
    reg.insert(
        id.clone(),
        SceneLayer {
            items: vec![SceneItem::Text(SceneTextItem {
                x: 5.0,
                y: 12.0,
                text: "42".to_string(),
                size: 12.0,
                paint: ScenePaint {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                },
                family: None,
                style: None,
            })],
        },
    );
    let withed = pipeline::build_document(
        &doc,
        &PipelineOptions {
            font: Some(&font),
            scene_layers: Some(&reg),
            ..PipelineOptions::default()
        },
    )
    .unwrap();

    // The two glyphs of "42" added two FillPaths over the baseline.
    assert_eq!(
        fill_count(&withed),
        fill_count(&base) + 2,
        "a two-glyph text run emits two glyph fills"
    );
    // ...bracketed by a content-box clip.
    assert!(withed.pages.iter().any(|p| p
        .list
        .commands
        .iter()
        .any(|c| matches!(c, DisplayCommand::PushClip { .. }))));
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

/// A one-tile provider: the whole level-0 image as a single opaque tile.
struct OneTile {
    revision: u64,
}
impl paged_renderer::resource_provider::ImageResourceProvider for OneTile {
    fn tile(
        &self,
        _image_id: &str,
        level: u8,
        x: u32,
        y: u32,
    ) -> Option<paged_renderer::resource_provider::ProviderTile> {
        if level != 0 || x != 0 || y != 0 {
            return None;
        }
        Some(paged_renderer::resource_provider::ProviderTile {
            rgba: vec![255u8; 8 * 8 * 4].into(),
            width: 8,
            height: 8,
            dest: [0, 0],
        })
    }
    fn revision(&self, _image_id: &str) -> u64 {
        self.revision
    }
}

/// C-1 over C-6 — a frame carrying BOTH a plugin scene layer and a claimed
/// tile provider paints the tiles FIRST and the plugin's layer OVER them.
///
/// The two channels are not peers. C-6 tiles are the frame's OWN image
/// content — the claim's own doc comment says releasing one "restores the
/// native whole-image fallback lane" — while a C-1 layer is the plugin's
/// render of that frame, spliced in "right after the frame's own content".
/// Emitting the tiles last inverted that, and because a display list is
/// painted in order the tiles covered the layer completely.
///
/// It was not theoretical: paged.image claims tiles for every frame it
/// ingests and composites its adjustments as a Stage-A scene layer, so
/// Apply changed the page by exactly 0 px — the whole "adjust an image,
/// see the adjustment" loop was invisible. Measured in the editor on the
/// showcase's raster page before this fix; 25,760 px of a red cover layer
/// on a bare frame, 318 px of the same layer on a tiled one.
#[test]
fn a_scene_layer_paints_over_the_frames_claimed_tiles() {
    let doc = sample_doc();
    let id = first_text_frame_id(&doc);

    let mut layers = HashMap::new();
    layers.insert(id.clone(), one_fill_layer());

    let provider = OneTile { revision: 1 };
    let mut providers = HashMap::new();
    providers.insert(
        id.clone(),
        pipeline::ResourceProviderEntry {
            image_id: "x-paged-image:probe",
            pyramid: paged_renderer::resource_provider::ResourcePyramid {
                base_width: 8,
                base_height: 8,
                levels: 1,
                tile_size: 8,
            },
            provider: &provider,
        },
    );

    let built = pipeline::build_document(
        &doc,
        &PipelineOptions {
            scene_layers: Some(&layers),
            resource_providers: Some(&providers),
            ..PipelineOptions::default()
        },
    )
    .unwrap();

    // Find the page carrying both, and the index of each contribution.
    let mut checked = false;
    for page in &built.pages {
        let tile_at = page
            .list
            .commands
            .iter()
            .position(|c| matches!(c, DisplayCommand::Image { .. }));
        // The scene layer's fill is the LAST FillPath on the page: the
        // frame's own fills are emitted before either plugin channel.
        let layer_at = page
            .list
            .commands
            .iter()
            .rposition(|c| matches!(c, DisplayCommand::FillPath { .. }));
        let (Some(tile_at), Some(layer_at)) = (tile_at, layer_at) else {
            continue;
        };
        assert!(
            tile_at < layer_at,
            "the C-6 tiles must paint BEFORE the C-1 scene layer \
             (tiles at {tile_at}, layer at {layer_at}) — emitted after, \
             they cover the plugin's in-frame render completely"
        );
        checked = true;
    }
    assert!(
        checked,
        "no page carried both a claimed tile and a scene-layer fill — the \
         test proved nothing"
    );
}
