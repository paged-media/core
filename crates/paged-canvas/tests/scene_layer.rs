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

//! C-1 — the model side of the plugin scene-layer channel: `set_scene_layer`
//! / `clear_scene_layer` mutate the registry and rebuild the document so the
//! next snapshot reflects the change. The registry is ephemeral (not part of
//! the document). The compose-time splice itself is tested in paged-compose +
//! paged-renderer; this proves the model wiring + rebuild integration.

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_compose::{SceneItem, SceneLayer, ScenePaint, ScenePathSeg};

fn doc_bytes() -> Vec<u8> {
    paged_gen::write_idml(&paged_gen::samples::text::build()).unwrap()
}

fn a_layer() -> SceneLayer {
    SceneLayer {
        items: vec![SceneItem::FillPath {
            path: vec![
                ScenePathSeg::MoveTo { x: 0.0, y: 0.0 },
                ScenePathSeg::LineTo { x: 5.0, y: 0.0 },
                ScenePathSeg::LineTo { x: 5.0, y: 5.0 },
                ScenePathSeg::Close,
            ],
            paint: ScenePaint {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        }],
    }
}

#[test]
fn set_then_clear_scene_layer_round_trips_through_a_rebuild() {
    let mut m = CanvasModel::load("d", &doc_bytes(), CanvasOptions::default()).unwrap();
    assert!(m.scene_layer_ids().is_empty(), "no layers at load");

    // Submit a layer — registry updates and the document rebuilds (the new
    // PipelineOptions.scene_layers path runs without error).
    m.set_scene_layer("media.paged.sheet.grid.f1".to_string(), a_layer())
        .expect("set + rebuild");
    assert_eq!(m.scene_layer_ids(), vec!["media.paged.sheet.grid.f1"]);

    // Replace is idempotent on the id set.
    m.set_scene_layer("media.paged.sheet.grid.f1".to_string(), a_layer())
        .expect("replace + rebuild");
    assert_eq!(m.scene_layer_ids().len(), 1);

    // Clear removes it and rebuilds back to native content.
    m.clear_scene_layer("media.paged.sheet.grid.f1")
        .expect("clear + rebuild");
    assert!(m.scene_layer_ids().is_empty(), "layer cleared");

    // Clearing an absent id is a no-op (no rebuild, no error).
    m.clear_scene_layer("nope")
        .expect("clear absent is a no-op");
}
