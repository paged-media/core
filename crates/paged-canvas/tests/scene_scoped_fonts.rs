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

//! Faces scoped to scene layers.
//!
//! One registry used to serve everything: document layout, scene-layer
//! text (ADR 126), the Fonts panel's `isMissing` and the substitution
//! report. So a plugin that registered the face its own content draws
//! in — say "Lora", for a web frame — also made "Lora" present for the
//! document: a document asking for Lora stopped being flagged missing
//! and started composing in the plugin's bytes, without the user ever
//! supplying the font.
//!
//! `RegisterFont { scope: "sceneLayer" }` puts a face in a second table
//! that ONLY scene-layer text consults (before the document registry).
//! The document's view — its layout, `isMissing`, substitution — never
//! sees it. `ClearFontRegistry { scope }` drops one table and leaves the
//! other.

use paged_canvas::channel::{FontScope, MainToWorkerKind, Mutation};
use paged_canvas::{CanvasModel, CanvasOptions, ElementId, FontEntry};
use paged_compose::{SceneItem, SceneLayer, ScenePaint, SceneTextItem};

fn corpus_font(name: &str) -> Vec<u8> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/fonts")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn text_layer(family: &str) -> SceneLayer {
    SceneLayer {
        items: vec![SceneItem::Text(SceneTextItem {
            x: 4.0,
            y: 20.0,
            text: "Faces".to_string(),
            size: 18.0,
            paint: ScenePaint {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            family: Some(family.to_string()),
            style: None,
            weight: None,
            italic: None,
        })],
    }
}

fn lora() -> FontEntry {
    FontEntry {
        family: "Lora".into(),
        style: None,
        bytes: corpus_font("Lora.ttf"),
    }
}

/// The text sample with its first story set in Lora, which nobody has
/// registered: the document is missing a font.
fn load_missing_lora() -> (CanvasModel, String) {
    let bytes = paged_gen::write_idml(&paged_gen::samples::text::build()).unwrap();
    let mut m = CanvasModel::load(
        "d",
        &bytes,
        CanvasOptions {
            fonts: vec![corpus_font("Inter.ttf")],
            ..CanvasOptions::default()
        },
    )
    .unwrap();
    let story = m.scene().stories[0].self_id.clone();
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::StoryRange {
            story_id: story,
            start: 0,
            end: 5,
        },
        path: paged_mutate::PropertyPath::CharacterFontFamily,
        value: paged_mutate::Value::Text("Lora".into()),
    })
    .expect("set Lora");
    let frame = m
        .scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find_map(|f| f.self_id.clone())
        .expect("a text frame");
    (m, frame)
}

fn lora_missing(m: &CanvasModel) -> bool {
    m.fonts()
        .into_iter()
        .find(|f| f.family == "Lora")
        .expect("the document uses Lora")
        .is_missing
}

fn page_digests(m: &CanvasModel) -> Vec<u64> {
    m.built().pages.iter().map(|p| p.list.digest()).collect()
}

#[test]
fn a_scene_face_draws_scene_text_and_leaves_the_document_alone() {
    let (mut m, frame) = load_missing_lora();
    assert!(lora_missing(&m), "nobody registered Lora");
    let document_render = page_digests(&m);

    m.set_scene_layer(frame.clone(), text_layer("Lora"))
        .unwrap();
    assert_eq!(
        m.scene_layer_font_fallbacks(&frame),
        vec!["Lora".to_string()]
    );

    // Scene-scoped: the scene text resolves it, with no fallback …
    let touched = m
        .register_font_scoped(lora(), FontScope::SceneLayer)
        .unwrap();
    assert!(
        m.scene_layer_font_fallbacks(&frame).is_empty(),
        "scene text draws in the scene face"
    );
    assert_eq!(touched, vec![frame.clone()], "only the scene frame rebuilt");
    // … and the document still lacks Lora: flagged missing, laid out as
    // before (with the layer gone the pages are what they were).
    assert!(lora_missing(&m), "a scene face is not a document font");
    m.clear_scene_layer(&frame).unwrap();
    assert_eq!(
        page_digests(&m),
        document_render,
        "document layout unchanged"
    );

    // A document registration is what the document sees.
    m.register_font(lora()).unwrap();
    assert!(!lora_missing(&m));
}

#[test]
fn clearing_one_scope_keeps_the_other() {
    let (mut m, frame) = load_missing_lora();
    m.set_scene_layer(frame.clone(), text_layer("Lora"))
        .unwrap();
    m.register_font_scoped(lora(), FontScope::SceneLayer)
        .unwrap();
    m.register_font(FontEntry {
        family: "Fraunces".into(),
        style: None,
        bytes: corpus_font("Fraunces-VF.ttf"),
    })
    .unwrap();

    // The document clear leaves the scene face …
    m.clear_font_registry_scoped(FontScope::Document).unwrap();
    assert!(m.scene_layer_font_fallbacks(&frame).is_empty());

    // … and the scene clear takes it, reporting the frame it rebuilt.
    let touched = m.clear_font_registry_scoped(FontScope::SceneLayer).unwrap();
    assert_eq!(touched, vec![frame.clone()]);
    assert_eq!(
        m.scene_layer_font_fallbacks(&frame),
        vec!["Lora".to_string()]
    );
}

#[test]
fn the_scope_is_optional_on_the_wire() {
    let msg = |json: &str| -> MainToWorkerKind {
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"))
    };
    match msg(r#"{"kind":"registerFont","payload":{"family":"A","bytes":[0]}}"#) {
        MainToWorkerKind::RegisterFont { scope, .. } => assert_eq!(scope, FontScope::Document),
        other => panic!("{other:?}"),
    }
    match msg(
        r#"{"kind":"registerFont","payload":{"family":"A","bytes":[0],"scope":"sceneLayer"}}"#,
    ) {
        MainToWorkerKind::RegisterFont { scope, .. } => assert_eq!(scope, FontScope::SceneLayer),
        other => panic!("{other:?}"),
    }
    // A clear with no payload is today's message: the document registry.
    let scope_of = |m: MainToWorkerKind| match m {
        MainToWorkerKind::ClearFontRegistry(p) => p.unwrap_or_default().scope,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        scope_of(msg(r#"{"kind":"clearFontRegistry"}"#)),
        FontScope::Document
    );
    assert_eq!(
        scope_of(msg(r#"{"kind":"clearFontRegistry","payload":null}"#)),
        FontScope::Document
    );
    assert_eq!(
        scope_of(msg(r#"{"kind":"clearFontRegistry","payload":{}}"#)),
        FontScope::Document
    );
    assert_eq!(
        scope_of(msg(
            r#"{"kind":"clearFontRegistry","payload":{"scope":"sceneLayer"}}"#
        )),
        FontScope::SceneLayer
    );
    // And it still serialises payload-less when the scope is the default
    // a host left out.
    let none = serde_json::to_value(MainToWorkerKind::ClearFontRegistry(None)).unwrap();
    assert_eq!(none["kind"], "clearFontRegistry");
}
