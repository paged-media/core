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

//! C-63 — effects, opacity and blend reach the kinds that RENDER them.
//!
//! A path drawn with the pen is a `Polygon`. `paged_model::Polygon` has
//! carried `effects: Option<FrameEffects>` since Q-04, the importer
//! fills it and `emit_polygon_into` paints it — and no apply arm reached
//! it, because `find_frame_effects_mut` matched three kinds and fell
//! through on the fourth. So a glow on a pen path could arrive in a file
//! and never be made in the editor.
//!
//! What this file pins:
//!
//! * every effect path (the seven per-effect families plus the
//!   whole-struct gradient feather) is ACCEPTED on a Polygon, changes
//!   the bag, is restored by its own inverse and reproduced by redo;
//! * a Polygon effect actually changes what is drawn — the display list
//!   gains the effect command, against the polygon's own path;
//! * `Group` takes `FrameOpacity` / `FrameBlendMode` on its
//!   `transparency` block, the renderer brackets the members with it,
//!   and an ungroup-then-undo brings the block back;
//! * `GraphicLine` still REJECTS every effect path. It carries the bag
//!   too, but `emit_line_into` never reads it, so an arm there would
//!   store a value nothing draws.

use std::path::PathBuf;

use paged_compose::DisplayCommand;
use paged_model::FrameEffects;
use paged_mutate::operation::{
    GradientFeatherSpec, GradientFeatherStopSpec, NodeSpec, PathAnchorSpec,
};
use paged_mutate::{apply, GroupSpec, NodeId, Operation, OperationError, PropertyPath, Value};
use paged_renderer::pipeline::{build_document, PipelineOptions};
use paged_scene::Document;

use PropertyPath as P;

fn fixture() -> Document {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
        .join("generated")
        .join("geometry-groups.idml");
    let bytes = std::fs::read(path).expect("read geometry-groups fixture");
    idml_import::import_idml_doc(&bytes).expect("open")
}

/// First polygon that carries real anchors — the shape `insertPath`
/// mints, and the only shape the effects have a path to paint against
/// (an anchorless polygon collapses to its bounding box).
fn first_polygon(doc: &Document) -> String {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.polygons.iter())
        .filter_map(|p| {
            let id = p.self_id.clone()?;
            (!p.anchors.is_empty()).then_some(id)
        })
        .next()
        .expect("fixture has a polygon with anchors")
}

fn polygon<'a>(doc: &'a Document, id: &str) -> &'a paged_model::Polygon {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.polygons.iter())
        .find(|p| p.self_id.as_deref() == Some(id))
        .expect("polygon present")
}

/// The effects bag as text. An absent bag and an empty one are the same
/// value here on purpose: both render nothing, and a toggle that is
/// switched on and undone leaves the empty one behind (the bag itself
/// has no "absent" spelling in a `SetProperty` inverse).
fn bag(doc: &Document, id: &str) -> String {
    let effects: FrameEffects = polygon(doc, id).effects.clone().unwrap_or_default();
    format!("{effects:?}")
}

fn set(node: &NodeId, path: PropertyPath, value: Value) -> Operation {
    Operation::SetProperty {
        node: node.clone(),
        path,
        value,
    }
}

fn len(v: f32) -> Value {
    Value::Length(Some(v))
}
fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}
fn color() -> Value {
    Value::ColorRef(Some("Color/C63Probe".to_string()))
}

fn feather_spec() -> Value {
    Value::GradientFeather(Some(GradientFeatherSpec {
        gradient_type: Some("Linear".into()),
        start_point: Some([0.0, 0.0]),
        end_point: Some([100.0, 0.0]),
        angle_deg: Some(0.0),
        stops: vec![
            GradientFeatherStopSpec {
                stop_color: None,
                location_pct: 0.0,
                alpha_pct: 100.0,
                midpoint_pct: 50.0,
            },
            GradientFeatherStopSpec {
                stop_color: None,
                location_pct: 100.0,
                alpha_pct: 0.0,
                midpoint_pct: 50.0,
            },
        ],
    }))
}

/// Every path that goes through `find_frame_effects_mut` or one of the
/// per-effect accessors below it: `(family toggle, path, a value that
/// differs from the InDesign preset the block is materialised with)`.
/// A row whose path IS its toggle switches the effect on from nothing.
fn effect_rows() -> Vec<(PropertyPath, PropertyPath, Value)> {
    let on = Value::Bool(true);
    vec![
        // Inner shadow — preset Multiply / 75 / 120° / 5 / 5 / 0 / 0.
        (
            P::FrameInnerShadowEnabled,
            P::FrameInnerShadowEnabled,
            on.clone(),
        ),
        (
            P::FrameInnerShadowEnabled,
            P::FrameInnerShadowBlendMode,
            text("Overlay"),
        ),
        (
            P::FrameInnerShadowEnabled,
            P::FrameInnerShadowColor,
            color(),
        ),
        (
            P::FrameInnerShadowEnabled,
            P::FrameInnerShadowOpacity,
            len(13.5),
        ),
        (
            P::FrameInnerShadowEnabled,
            P::FrameInnerShadowAngle,
            len(13.5),
        ),
        (
            P::FrameInnerShadowEnabled,
            P::FrameInnerShadowDistance,
            len(13.5),
        ),
        (
            P::FrameInnerShadowEnabled,
            P::FrameInnerShadowSize,
            len(13.5),
        ),
        (
            P::FrameInnerShadowEnabled,
            P::FrameInnerShadowChoke,
            len(13.5),
        ),
        (
            P::FrameInnerShadowEnabled,
            P::FrameInnerShadowNoise,
            len(13.5),
        ),
        // Outer glow — preset Screen / 75 / 0 / 5 / 0.
        (
            P::FrameOuterGlowEnabled,
            P::FrameOuterGlowEnabled,
            on.clone(),
        ),
        (
            P::FrameOuterGlowEnabled,
            P::FrameOuterGlowBlendMode,
            text("Overlay"),
        ),
        (P::FrameOuterGlowEnabled, P::FrameOuterGlowColor, color()),
        (
            P::FrameOuterGlowEnabled,
            P::FrameOuterGlowOpacity,
            len(13.5),
        ),
        (P::FrameOuterGlowEnabled, P::FrameOuterGlowSpread, len(13.5)),
        (P::FrameOuterGlowEnabled, P::FrameOuterGlowSize, len(13.5)),
        (P::FrameOuterGlowEnabled, P::FrameOuterGlowNoise, len(13.5)),
        // Inner glow — preset Screen / 75 / 0 / 5 / EdgeGlow / 0.
        (
            P::FrameInnerGlowEnabled,
            P::FrameInnerGlowEnabled,
            on.clone(),
        ),
        (
            P::FrameInnerGlowEnabled,
            P::FrameInnerGlowBlendMode,
            text("Overlay"),
        ),
        (P::FrameInnerGlowEnabled, P::FrameInnerGlowColor, color()),
        (
            P::FrameInnerGlowEnabled,
            P::FrameInnerGlowOpacity,
            len(13.5),
        ),
        (P::FrameInnerGlowEnabled, P::FrameInnerGlowChoke, len(13.5)),
        (P::FrameInnerGlowEnabled, P::FrameInnerGlowSize, len(13.5)),
        (
            P::FrameInnerGlowEnabled,
            P::FrameInnerGlowSource,
            text("CenterSourced"),
        ),
        (P::FrameInnerGlowEnabled, P::FrameInnerGlowNoise, len(13.5)),
        // Bevel — preset InnerBevel / Smooth / Up / 100 / 5 / 0 / 120° / 30°.
        (P::FrameBevelEnabled, P::FrameBevelEnabled, on.clone()),
        (P::FrameBevelEnabled, P::FrameBevelStyle, text("Emboss")),
        (
            P::FrameBevelEnabled,
            P::FrameBevelTechnique,
            text("ChiselHard"),
        ),
        (P::FrameBevelEnabled, P::FrameBevelDirection, text("Down")),
        (P::FrameBevelEnabled, P::FrameBevelDepth, len(13.5)),
        (P::FrameBevelEnabled, P::FrameBevelSize, len(13.5)),
        (P::FrameBevelEnabled, P::FrameBevelSoften, len(13.5)),
        (P::FrameBevelEnabled, P::FrameBevelAngle, len(13.5)),
        (P::FrameBevelEnabled, P::FrameBevelAltitude, len(13.5)),
        (P::FrameBevelEnabled, P::FrameBevelHighlightColor, color()),
        (P::FrameBevelEnabled, P::FrameBevelShadowColor, color()),
        (
            P::FrameBevelEnabled,
            P::FrameBevelHighlightOpacity,
            len(13.5),
        ),
        (P::FrameBevelEnabled, P::FrameBevelShadowOpacity, len(13.5)),
        // Satin — preset Multiply / 50 / 19° / 11 / 14 / inverted.
        (P::FrameSatinEnabled, P::FrameSatinEnabled, on.clone()),
        (
            P::FrameSatinEnabled,
            P::FrameSatinBlendMode,
            text("Overlay"),
        ),
        (P::FrameSatinEnabled, P::FrameSatinColor, color()),
        (P::FrameSatinEnabled, P::FrameSatinOpacity, len(13.5)),
        (P::FrameSatinEnabled, P::FrameSatinAngle, len(13.5)),
        (P::FrameSatinEnabled, P::FrameSatinDistance, len(13.5)),
        (P::FrameSatinEnabled, P::FrameSatinSize, len(13.5)),
        (
            P::FrameSatinEnabled,
            P::FrameSatinInvert,
            Value::Bool(false),
        ),
        // Feather — preset 5 / Diffusion / 0 / 0.
        (P::FrameFeatherEnabled, P::FrameFeatherEnabled, on.clone()),
        (P::FrameFeatherEnabled, P::FrameFeatherWidth, len(13.5)),
        (
            P::FrameFeatherEnabled,
            P::FrameFeatherCornerType,
            text("Sharp"),
        ),
        (P::FrameFeatherEnabled, P::FrameFeatherNoise, len(13.5)),
        (P::FrameFeatherEnabled, P::FrameFeatherChoke, len(13.5)),
        // Directional feather — preset 5 / 5 / 5 / 5 / 0° / 0 / 0.
        (
            P::FrameDirectionalFeatherEnabled,
            P::FrameDirectionalFeatherEnabled,
            on,
        ),
        (
            P::FrameDirectionalFeatherEnabled,
            P::FrameDirectionalFeatherLeftWidth,
            len(13.5),
        ),
        (
            P::FrameDirectionalFeatherEnabled,
            P::FrameDirectionalFeatherRightWidth,
            len(13.5),
        ),
        (
            P::FrameDirectionalFeatherEnabled,
            P::FrameDirectionalFeatherTopWidth,
            len(13.5),
        ),
        (
            P::FrameDirectionalFeatherEnabled,
            P::FrameDirectionalFeatherBottomWidth,
            len(13.5),
        ),
        (
            P::FrameDirectionalFeatherEnabled,
            P::FrameDirectionalFeatherAngle,
            len(13.5),
        ),
        (
            P::FrameDirectionalFeatherEnabled,
            P::FrameDirectionalFeatherNoise,
            len(13.5),
        ),
        (
            P::FrameDirectionalFeatherEnabled,
            P::FrameDirectionalFeatherChoke,
            len(13.5),
        ),
        // Whole-struct gradient feather: absent → a two-stop fade.
        (
            P::FrameGradientFeather,
            P::FrameGradientFeather,
            feather_spec(),
        ),
    ]
}

/// Apply, undo, redo — for every effect path, on a Polygon.
#[test]
fn polygon_takes_every_effect_path_and_undo_restores_it() {
    let rows = effect_rows();
    // 58 per-effect paths + the gradient feather. A count, so a path
    // added to the kernel without a row here is a decision somebody has
    // to make rather than an omission nobody sees.
    assert_eq!(rows.len(), 59, "the effect-path roster this file covers");

    for (toggle, path, value) in rows {
        let mut doc = fixture();
        let id = first_polygon(&doc);
        let node = NodeId::Polygon(id.clone());
        assert!(
            polygon(&doc, &id).effects.is_none(),
            "the fixture polygon starts with no effects"
        );
        // A field edit is measured inside an effect that is already on,
        // so its inverse can be compared exactly: writing a field into
        // an ABSENT effect materialises the whole preset block, and the
        // inverse restores the field, not the absence (the documented
        // W0.4 behaviour, shared with every other kind).
        if toggle != path {
            apply(&mut doc, &set(&node, toggle, Value::Bool(true)))
                .unwrap_or_else(|e| panic!("Polygon rejected {toggle:?}: {e:?}"));
        }
        let before = bag(&doc, &id);

        let applied = apply(&mut doc, &set(&node, path, value))
            .unwrap_or_else(|e| panic!("Polygon must accept {path:?} (C-63): {e:?}"));
        let after = bag(&doc, &id);
        assert_ne!(before, after, "{path:?} did not change the effects bag");
        assert_eq!(
            applied.invalidation.frame_style,
            vec![node.clone()],
            "{path:?} is paint-only on the polygon itself"
        );

        let undone =
            apply(&mut doc, &applied.inverse).unwrap_or_else(|e| panic!("undo of {path:?}: {e:?}"));
        assert_eq!(bag(&doc, &id), before, "undo of {path:?} did not restore");

        apply(&mut doc, &undone.inverse).unwrap_or_else(|e| panic!("redo of {path:?}: {e:?}"));
        assert_eq!(bag(&doc, &id), after, "redo of {path:?} did not reproduce");
    }
}

/// The effect is DRAWN, not just stored: the page's display list gains
/// the effect command, and it rides the polygon's own interned path —
/// the same path id the polygon's fill uses.
#[test]
fn a_polygon_effect_changes_what_is_drawn() {
    fn commands(doc: &Document) -> Vec<String> {
        build_document(doc, &PipelineOptions::default())
            .expect("build")
            .pages
            .iter()
            .flat_map(|p| p.list.commands.iter().map(|c| format!("{c:?}")))
            .collect()
    }
    fn count(doc: &Document, pick: fn(&DisplayCommand) -> bool) -> usize {
        build_document(doc, &PipelineOptions::default())
            .expect("build")
            .pages
            .iter()
            .flat_map(|p| p.list.commands.iter())
            .filter(|c| pick(c))
            .count()
    }
    let is_inner_shadow = |c: &DisplayCommand| matches!(c, DisplayCommand::InnerShadow { .. });
    let is_outer_glow = |c: &DisplayCommand| matches!(c, DisplayCommand::OuterGlow { .. });

    let mut doc = fixture();
    let id = first_polygon(&doc);
    let node = NodeId::Polygon(id);
    let plain = commands(&doc);
    let shadows_before = count(&doc, is_inner_shadow);
    let glows_before = count(&doc, is_outer_glow);

    let shadow = apply(
        &mut doc,
        &set(&node, P::FrameInnerShadowEnabled, Value::Bool(true)),
    )
    .expect("inner shadow on a polygon");
    assert_eq!(
        count(&doc, is_inner_shadow),
        shadows_before + 1,
        "the polygon's inner shadow is emitted"
    );
    apply(
        &mut doc,
        &set(&node, P::FrameOuterGlowEnabled, Value::Bool(true)),
    )
    .expect("outer glow on a polygon");
    assert_eq!(
        count(&doc, is_outer_glow),
        glows_before + 1,
        "the polygon's outer glow is emitted"
    );
    assert_ne!(commands(&doc), plain, "the page paints differently");

    // Undo both: the page is back to exactly what it drew before.
    apply(
        &mut doc,
        &set(&node, P::FrameOuterGlowEnabled, Value::Bool(false)),
    )
    .expect("glow off");
    apply(&mut doc, &shadow.inverse).expect("undo the shadow");
    assert_eq!(
        commands(&doc),
        plain,
        "undo restores the exact command list"
    );
}

/// Append a leaf page item to the first spread, on top — the position
/// the wire's own insert ops use (the end of the item's kind vec).
fn insert(doc: &mut Document, node: NodeSpec) {
    let spread = &doc.spreads[0].spread;
    let position = match &node {
        NodeSpec::Rectangle { .. } => spread.rectangles.len(),
        NodeSpec::GraphicLine { .. } => spread.graphic_lines.len(),
        other => panic!("this helper appends rectangles and lines, not {other:?}"),
    };
    let parent = NodeId::Spread(spread.self_id.clone().expect("the first spread has an id"));
    apply(
        doc,
        &Operation::InsertNode {
            parent,
            position,
            z_slot: None,
            node,
        },
    )
    .expect("insert node");
}

/// A `GraphicLine` carries the `effects` bag and nothing draws it, so
/// every effect path keeps rejecting — one honest "unsupported" instead
/// of 59 writes into a field the renderer never opens.
#[test]
fn a_graphic_line_rejects_every_effect_path() {
    let mut doc = fixture();
    let corner = |x: f32, y: f32| PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    };
    insert(
        &mut doc,
        NodeSpec::GraphicLine {
            self_id: "c63line".to_string(),
            bounds: [40.0, 40.0, 140.0, 140.0],
            anchors: vec![corner(40.0, 40.0), corner(140.0, 140.0)],
            subpath_starts: vec![],
            subpath_open: vec![],
            stroke_color: Some("Color/Black".to_string()),
            stroke_weight: Some(1.0),
            item_transform: None,
        },
    );
    let node = NodeId::GraphicLine("c63line".to_string());
    for (_, path, value) in effect_rows() {
        let err = apply(&mut doc, &set(&node, path, value))
            .expect_err("a line has no fill path for an effect to composite against");
        assert!(
            matches!(err, OperationError::UnsupportedProperty { .. }),
            "{path:?} on a GraphicLine must be UnsupportedProperty, got {err:?}"
        );
    }
}

// ── Group ───────────────────────────────────────────────────────────

/// Two overlapping filled rectangles in one group; returns the group id.
fn make_group(doc: &mut Document) -> String {
    for (id, bounds) in [
        ("c63a", [100.0, 100.0, 300.0, 300.0]),
        ("c63b", [150.0, 150.0, 350.0, 350.0]),
    ] {
        insert(
            doc,
            NodeSpec::Rectangle {
                self_id: id.to_string(),
                bounds,
                fill_color: Some("Color/Black".to_string()),
                stroke_color: None,
                stroke_weight: None,
                item_transform: None,
            },
        );
    }
    let applied = apply(
        doc,
        &Operation::CreateGroup {
            spec: GroupSpec {
                self_id: None,
                members: vec![
                    NodeId::Rectangle("c63a".to_string()),
                    NodeId::Rectangle("c63b".to_string()),
                ],
                parent: None,
                item_transform: None,
                opacity: None,
                blend_mode: None,
            },
        },
    )
    .expect("create group");
    match applied.op {
        Operation::CreateGroup { spec } => spec.self_id.expect("minted id echoed"),
        other => panic!("unexpected echoed op: {other:?}"),
    }
}

fn group<'a>(doc: &'a Document, id: &str) -> &'a paged_model::Group {
    doc.spreads
        .iter()
        .flat_map(|s| s.spread.groups.iter())
        .find(|g| g.self_id.as_deref() == Some(id))
        .expect("group present")
}

/// `(blend mode, opacity)` of every blend group the build opens.
fn blend_groups(doc: &Document) -> Vec<String> {
    build_document(doc, &PipelineOptions::default())
        .expect("build")
        .pages
        .iter()
        .flat_map(|p| p.list.commands.iter())
        .filter_map(|c| match c {
            DisplayCommand::BeginBlendGroup {
                blend_mode,
                opacity,
                ..
            } => Some(format!("{blend_mode:?} @ {opacity}")),
            _ => None,
        })
        .collect()
}

/// Opacity and blend mode on a group: stored on its `transparency`
/// block, composited by the renderer as ONE bracket around the members,
/// and undone exactly.
#[test]
fn group_takes_opacity_and_blend_and_the_renderer_brackets_its_members() {
    let mut doc = fixture();
    let id = make_group(&mut doc);
    let node = NodeId::Group(id.clone());
    let brackets_before = blend_groups(&doc);
    assert_eq!(group(&doc, &id).transparency.opacity, None);
    assert_eq!(group(&doc, &id).transparency.blend_mode, None);

    // Opacity.
    let faded = apply(&mut doc, &set(&node, P::FrameOpacity, len(40.0)))
        .expect("Group must accept FrameOpacity (C-63)");
    assert_eq!(group(&doc, &id).transparency.opacity, Some(40.0));
    assert_eq!(faded.invalidation.frame_style, vec![node.clone()]);
    let brackets = blend_groups(&doc);
    assert_eq!(
        brackets.len(),
        brackets_before.len() + 1,
        "a group below 100% opens exactly one blend group around its members"
    );
    assert!(
        brackets.iter().any(|b| b.ends_with("@ 0.4")),
        "the bracket carries the group's opacity: {brackets:?}"
    );
    let undone = apply(&mut doc, &faded.inverse).expect("undo opacity");
    assert_eq!(group(&doc, &id).transparency.opacity, None);
    assert_eq!(
        blend_groups(&doc),
        brackets_before,
        "undo closes the bracket"
    );
    apply(&mut doc, &undone.inverse).expect("redo opacity");
    assert_eq!(group(&doc, &id).transparency.opacity, Some(40.0));
    apply(&mut doc, &faded.inverse).expect("undo opacity again");

    // Blend mode.
    let blended = apply(&mut doc, &set(&node, P::FrameBlendMode, text("Multiply")))
        .expect("Group must accept FrameBlendMode (C-63)");
    assert_eq!(
        group(&doc, &id).transparency.blend_mode.as_deref(),
        Some("Multiply")
    );
    let brackets = blend_groups(&doc);
    assert_eq!(brackets.len(), brackets_before.len() + 1);
    assert!(
        brackets.iter().any(|b| b.starts_with("Multiply")),
        "the bracket carries the group's blend mode: {brackets:?}"
    );
    let undone = apply(&mut doc, &blended.inverse).expect("undo blend");
    assert_eq!(group(&doc, &id).transparency.blend_mode, None);
    assert_eq!(blend_groups(&doc), brackets_before);
    apply(&mut doc, &undone.inverse).expect("redo blend");
    assert_eq!(
        group(&doc, &id).transparency.blend_mode.as_deref(),
        Some("Multiply")
    );
    // The empty string clears, as on every other kind.
    apply(&mut doc, &set(&node, P::FrameBlendMode, text(""))).expect("clear blend");
    assert_eq!(group(&doc, &id).transparency.blend_mode, None);
}

/// Ungrouping drops the wrapper that carried the opacity; undoing the
/// ungroup must bring the wrapper back AS IT WAS, or making the value
/// settable would have made undo lossy.
#[test]
fn undoing_an_ungroup_restores_the_groups_opacity_and_blend() {
    let mut doc = fixture();
    let id = make_group(&mut doc);
    let node = NodeId::Group(id.clone());
    apply(&mut doc, &set(&node, P::FrameOpacity, len(40.0))).expect("opacity");
    apply(&mut doc, &set(&node, P::FrameBlendMode, text("Screen"))).expect("blend");
    let bracketed = blend_groups(&doc);

    let dissolved = apply(
        &mut doc,
        &Operation::DissolveGroup {
            group_id: id.clone(),
            restore_slots: None,
        },
    )
    .expect("ungroup");
    assert_ne!(
        blend_groups(&doc),
        bracketed,
        "the bracket went with the group"
    );

    apply(&mut doc, &dissolved.inverse).expect("undo the ungroup");
    let back = group(&doc, &id);
    assert_eq!(back.transparency.opacity, Some(40.0));
    assert_eq!(back.transparency.blend_mode.as_deref(), Some("Screen"));
    assert_eq!(blend_groups(&doc), bracketed, "and it paints as it did");
}
