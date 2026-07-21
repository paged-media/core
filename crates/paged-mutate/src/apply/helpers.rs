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

use paged_parse::{GraphicLine, Polygon, Rectangle, TextFrame};
use paged_scene::Document;

use crate::error::OperationError;
use crate::operation::{GradientFeatherSpec, InvalidationHint, NodeId, PropertyPath, Value};

// ---------------------------------------------------------------------------
// Helpers — finders + converters + constructors
// ---------------------------------------------------------------------------

pub(super) fn find_text_frame_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut TextFrame> {
    for parsed in &mut doc.spreads {
        for frame in &mut parsed.spread.text_frames {
            if frame.self_id.as_deref() == Some(self_id) {
                return Some(frame);
            }
        }
    }
    None
}

pub(super) fn find_polygon_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut Polygon> {
    for parsed in &mut doc.spreads {
        if let Some(p) = parsed
            .spread
            .polygons
            .iter_mut()
            .find(|p| p.self_id.as_deref() == Some(self_id))
        {
            return Some(p);
        }
    }
    None
}

pub(super) fn find_graphic_line_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut GraphicLine> {
    for parsed in &mut doc.spreads {
        if let Some(l) = parsed
            .spread
            .graphic_lines
            .iter_mut()
            .find(|l| l.self_id.as_deref() == Some(self_id))
        {
            return Some(l);
        }
    }
    None
}

pub(super) fn find_oval_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut paged_parse::Oval> {
    for parsed in &mut doc.spreads {
        if let Some(o) = parsed
            .spread
            .ovals
            .iter_mut()
            .find(|o| o.self_id.as_deref() == Some(self_id))
        {
            return Some(o);
        }
    }
    None
}

/// Editor-ops — resolve the gradient angle/length field a
/// `FrameGradient*` path addresses on whichever kind hosts `node`.
pub(super) fn find_gradient_field_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
    path: PropertyPath,
) -> Option<&'a mut Option<f32>> {
    macro_rules! pick {
        ($item:expr) => {
            match path {
                PropertyPath::FrameGradientFillAngle => Some(&mut $item.gradient_fill_angle),
                PropertyPath::FrameGradientFillLength => Some(&mut $item.gradient_fill_length),
                PropertyPath::FrameGradientStrokeAngle => Some(&mut $item.gradient_stroke_angle),
                PropertyPath::FrameGradientStrokeLength => Some(&mut $item.gradient_stroke_length),
                _ => None,
            }
        };
    }
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).and_then(|f| pick!(f)),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).and_then(|r| pick!(r)),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).and_then(|p| pick!(p)),
        NodeId::Oval(id) => find_oval_mut(doc, id).and_then(|o| pick!(o)),
        _ => None,
    }
}

pub(super) fn find_rectangle_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut Rectangle> {
    for parsed in &mut doc.spreads {
        for rect in &mut parsed.spread.rectangles {
            if rect.self_id.as_deref() == Some(self_id) {
                return Some(rect);
            }
        }
    }
    None
}

/// W1.16 — locate an anchored frame's `AnchoredObjectSetting` by its
/// `Self` id, materialising a default block when the frame carried
/// none (writing a single anchored attribute on a frame with no
/// `<AnchoredObjectSetting>` yet creates one — matching the
/// drop-shadow / text-wrap "materialise on first write" precedent).
/// Anchored frames live on a story's `Paragraph.anchored_frames`
/// (parsed from frames nested under a `<CharacterStyleRange>`), not in
/// the spread page-item vecs, so this scans the stories — recursing
/// into anchored Group children, which can themselves nest frames.
pub(super) fn find_anchored_setting_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut paged_parse::AnchoredObjectSetting> {
    fn in_frames<'b>(
        frames: &'b mut [paged_parse::AnchoredFrame],
        self_id: &str,
    ) -> Option<&'b mut paged_parse::AnchoredObjectSetting> {
        for frame in frames.iter_mut() {
            if frame.self_id.as_deref() == Some(self_id) {
                return Some(frame.setting.get_or_insert_with(Default::default));
            }
            // Anchored Groups can nest further anchored frames.
            if let Some(found) = in_frames(&mut frame.children, self_id) {
                return Some(found);
            }
        }
        None
    }
    for parsed in &mut doc.stories {
        for para in &mut parsed.story.paragraphs {
            if let Some(found) = in_frames(&mut para.anchored_frames, self_id) {
                return Some(found);
            }
        }
    }
    None
}

// ---- W0.3 — enum string round-trippers (parse `from_idml`s are
// non-injective for some variants, so we name the canonical string
// explicitly rather than reusing a parse helper). -----------------

pub(super) fn vj_as_idml(v: paged_parse::VerticalJustification) -> &'static str {
    use paged_parse::VerticalJustification as V;
    match v {
        V::Top => "TopAlign",
        V::Center => "CenterAlign",
        V::Bottom => "BottomAlign",
        V::Justify => "JustifyAlign",
    }
}

pub(super) fn auto_sizing_as_idml(v: paged_parse::AutoSizingType) -> &'static str {
    use paged_parse::AutoSizingType as A;
    match v {
        A::Off => "Off",
        A::HeightOnly => "HeightOnly",
        A::WidthOnly => "WidthOnly",
        A::HeightAndWidth => "HeightAndWidth",
        A::HeightAndWidthProportionally => "HeightAndWidthProportionally",
    }
}

pub(super) fn first_baseline_as_idml(v: paged_parse::FirstBaselineOffset) -> &'static str {
    use paged_parse::FirstBaselineOffset as F;
    match v {
        F::AscentOffset => "AscentOffset",
        F::CapHeight => "CapHeight",
        F::XHeight => "XHeight",
        F::EmBoxHeight => "EmBoxHeight",
        F::LeadingOffset => "LeadingOffset",
        F::FixedHeight => "FixedHeight",
    }
}

pub(super) fn corner_option_as_idml(v: paged_parse::CornerOption) -> &'static str {
    use paged_parse::CornerOption as C;
    match v {
        C::None => "None",
        C::Rounded => "RoundedCorner",
        C::Inverse => "InverseRoundedCorner",
        C::Inset => "InsetCorner",
        C::Bevel => "BeveledCorner",
        C::Fancy => "FancyCorner",
    }
}

/// W0.3 — map a per-corner `PropertyPath` to its index in
/// `Rectangle::corners` (IDML order `[top_left, top_right,
/// bottom_right, bottom_left]`).
pub(super) fn corner_index(path: PropertyPath) -> usize {
    match path {
        PropertyPath::FrameCornerOptionTopLeft | PropertyPath::FrameCornerRadiusTopLeft => 0,
        PropertyPath::FrameCornerOptionTopRight | PropertyPath::FrameCornerRadiusTopRight => 1,
        PropertyPath::FrameCornerOptionBottomRight | PropertyPath::FrameCornerRadiusBottomRight => {
            2
        }
        PropertyPath::FrameCornerOptionBottomLeft | PropertyPath::FrameCornerRadiusBottomLeft => 3,
        _ => unreachable!("corner_index called with a non-corner path"),
    }
}

/// W0.3 — locate the `stroke_type: Option<String>` field on any
/// stroked page-item kind.
pub(super) fn find_stroke_type_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<String>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.stroke_type),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.stroke_type),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.stroke_type),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.stroke_type),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.stroke_type),
        _ => None,
    }
}

/// W0.3 — locate the `stroke_gap_color: Option<String>` field.
pub(super) fn find_stroke_gap_color_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<String>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.stroke_gap_color),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.stroke_gap_color),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.stroke_gap_color),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.stroke_gap_color),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.stroke_gap_color),
        _ => None,
    }
}

/// W0.3 — locate the `stroke_gap_tint: Option<f32>` field.
pub(super) fn find_stroke_gap_tint_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<f32>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.stroke_gap_tint),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.stroke_gap_tint),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.stroke_gap_tint),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.stroke_gap_tint),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.stroke_gap_tint),
        _ => None,
    }
}

/// Punch-list (rides v35) — locate the `miter_limit: Option<f32>` field.
/// Carried by every closed-path kind that can show a mitered corner:
/// Rectangle (its four corners), Polygon (its vertices), and GraphicLine
/// (multi-segment / curved joins). Oval has no corners and TextFrame's
/// stroke is the rectangular frame, which keeps the legacy Rectangle-only
/// mutation surface.
pub(super) fn find_miter_limit_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<f32>> {
    match node {
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.miter_limit),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.miter_limit),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.miter_limit),
        _ => None,
    }
}

/// Punch-list (rides v35) — locate the `end_join: Option<String>` field.
/// Same kinds as [`find_miter_limit_mut`] (the two are an IDML pair).
pub(super) fn find_end_join_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<String>> {
    match node {
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.end_join),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.end_join),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.end_join),
        _ => None,
    }
}

/// W1.1 — locate the `stroke_dash: Vec<f32>` field (per-frame
/// `StrokeDashAndGap` override) on any stroked page-item kind.
pub(super) fn find_stroke_dash_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Vec<f32>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.stroke_dash),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.stroke_dash),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.stroke_dash),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.stroke_dash),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.stroke_dash),
        _ => None,
    }
}

/// W0.3 — locate the `item_transform: Option<[f32; 6]>` field on any
/// page-item kind (including Group, whose own transform decomposes).
pub(super) fn find_item_transform_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<[f32; 6]>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.item_transform),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.item_transform),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.item_transform),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.item_transform),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.item_transform),
        NodeId::Group(id) => find_group_mut(doc, id).map(|g| &mut g.item_transform),
        _ => None,
    }
}

/// W0.3 — locate the `overprint_fill: bool` field (fill-bearing kinds;
/// GraphicLine has no fill, so it's excluded).
pub(super) fn find_overprint_fill_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut bool> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.overprint_fill),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.overprint_fill),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.overprint_fill),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.overprint_fill),
        _ => None,
    }
}

/// W0.3 — locate the `overprint_stroke: bool` field (every stroked
/// kind, including GraphicLine).
pub(super) fn find_overprint_stroke_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut bool> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.overprint_stroke),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.overprint_stroke),
        NodeId::Oval(id) => find_oval_mut(doc, id).map(|o| &mut o.overprint_stroke),
        NodeId::Polygon(id) => find_polygon_mut(doc, id).map(|p| &mut p.overprint_stroke),
        NodeId::GraphicLine(id) => find_graphic_line_mut(doc, id).map(|l| &mut l.overprint_stroke),
        _ => None,
    }
}

pub(super) fn find_group_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut paged_parse::Group> {
    for parsed in &mut doc.spreads {
        for group in &mut parsed.spread.groups {
            if group.self_id.as_deref() == Some(self_id) {
                return Some(group);
            }
        }
    }
    None
}

/// SDK Phase 5 (v1 sweep) — synthesise a default DropShadowSetting
/// for the toggle-on case + per-field editors that write into a
/// prior-None state. Values mirror InDesign's "Drop Shadow"
/// preset (multiply blend, ~3pt offset, ~30% opacity).
pub(super) fn default_drop_shadow() -> paged_parse::DropShadowSetting {
    paged_parse::DropShadowSetting {
        mode: "Drop".to_string(),
        x_offset: 3.0,
        y_offset: 3.0,
        size: 4.0,
        opacity_pct: 30.0,
        effect_color: None,
    }
}

// W0.4 — paint-only single-node invalidation, the shared shape every
// transparency-effect arm returns (the rasterizer re-reads the effect
// fields on the next rebuild; none of them reflow text).
pub(super) fn frame_style_hint(node: &NodeId) -> InvalidationHint {
    InvalidationHint {
        frame_style: vec![node.clone()],
        ..Default::default()
    }
}

// W0.4 — InDesign-preset defaults for the non-DropShadow effect
// blocks. Materialised when a per-field editor (or the `*Enabled`
// toggle) writes into a prior-`None` block, exactly like
// `default_drop_shadow`. Values mirror InDesign's "Effects" dialog
// presets for each effect (Multiply/Screen blend, 75% opacity, the
// 120°/19° light angles, 5 pt sizes, …).

pub(super) fn default_inner_shadow() -> paged_parse::InnerShadowParams {
    paged_parse::InnerShadowParams {
        x_offset: None,
        y_offset: None,
        size: Some(5.0),
        opacity_pct: Some(75.0),
        effect_color: None,
        angle_deg: Some(120.0),
        distance: Some(5.0),
        choke_pct: Some(0.0),
        blend_mode: Some("Multiply".to_string()),
        noise_pct: Some(0.0),
    }
}

pub(super) fn default_outer_glow() -> paged_parse::OuterGlowParams {
    paged_parse::OuterGlowParams {
        size: Some(5.0),
        opacity_pct: Some(75.0),
        effect_color: None,
        spread_pct: Some(0.0),
        blend_mode: Some("Screen".to_string()),
        noise_pct: Some(0.0),
    }
}

pub(super) fn default_inner_glow() -> paged_parse::InnerGlowParams {
    paged_parse::InnerGlowParams {
        size: Some(5.0),
        opacity_pct: Some(75.0),
        effect_color: None,
        choke_pct: Some(0.0),
        blend_mode: Some("Screen".to_string()),
        source: Some("EdgeGlow".to_string()),
        noise_pct: Some(0.0),
    }
}

pub(super) fn default_bevel() -> paged_parse::BevelEmbossParams {
    paged_parse::BevelEmbossParams {
        depth_pct: Some(100.0),
        size: Some(5.0),
        angle_deg: Some(120.0),
        altitude_deg: Some(30.0),
        highlight_color: None,
        shadow_color: None,
        highlight_opacity_pct: Some(75.0),
        shadow_opacity_pct: Some(75.0),
        style: Some("InnerBevel".to_string()),
        direction: Some("Up".to_string()),
        technique: Some("Smooth".to_string()),
        soften: Some(0.0),
    }
}

pub(super) fn default_satin() -> paged_parse::SatinParams {
    paged_parse::SatinParams {
        size: Some(14.0),
        angle_deg: Some(19.0),
        distance: Some(11.0),
        effect_color: None,
        opacity_pct: Some(50.0),
        blend_mode: Some("Multiply".to_string()),
        invert: Some(true),
    }
}

pub(super) fn default_feather() -> paged_parse::FeatherParams {
    paged_parse::FeatherParams {
        width: Some(5.0),
        corner_type: Some("Diffusion".to_string()),
        noise_pct: Some(0.0),
        choke_pct: Some(0.0),
    }
}

pub(super) fn default_directional_feather() -> paged_parse::DirectionalFeatherParams {
    paged_parse::DirectionalFeatherParams {
        left_width: Some(5.0),
        right_width: Some(5.0),
        top_width: Some(5.0),
        bottom_width: Some(5.0),
        angle_deg: Some(0.0),
        noise_pct: Some(0.0),
        choke_pct: Some(0.0),
        corner_type: None,
    }
}

/// SDK Phase 5 (v1 sweep) — locate a mutable DropShadowSetting on
/// the named page item, materialising a default on `None` so
/// per-field editors always have a target to mutate. Supports
/// TextFrame + Rectangle (the two kinds with apply arms today);
/// returns `None` for other kinds.
pub(super) fn find_drop_shadow_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::DropShadowSetting> {
    let raw = node.self_id();
    for parsed in &mut doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if let Some(f) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    if f.drop_shadow.is_none() {
                        f.drop_shadow = Some(default_drop_shadow());
                    }
                    return f.drop_shadow.as_mut();
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(f) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    if f.drop_shadow.is_none() {
                        f.drop_shadow = Some(default_drop_shadow());
                    }
                    return f.drop_shadow.as_mut();
                }
            }
            _ => {}
        }
    }
    None
}

/// SDK Phase 5 (v1 sweep) — locate the `text_wrap: Option<TextWrap>`
/// field on any page-item kind that carries it. TextFrame /
/// Rectangle / Oval / Polygon / GraphicLine all do (Group doesn't —
/// the wrap rect is a leaf-item concept). Returns a mutable
/// reference so the apply arm can swap `mode` / `offsets`
/// independently while preserving the other.
pub(super) fn find_text_wrap_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<paged_parse::TextWrap>> {
    let raw = node.self_id();
    for parsed in &mut doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if let Some(p) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.text_wrap);
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(p) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.text_wrap);
                }
            }
            NodeId::Oval(_) => {
                if let Some(p) = parsed
                    .spread
                    .ovals
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.text_wrap);
                }
            }
            NodeId::GraphicLine(_) => {
                if let Some(p) = parsed
                    .spread
                    .graphic_lines
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.text_wrap);
                }
            }
            NodeId::Polygon(_) => {
                if let Some(p) = parsed
                    .spread
                    .polygons
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.text_wrap);
                }
            }
            _ => {}
        }
    }
    None
}

/// W2.5 — which element-level `bool` field a lookup targets.
#[derive(Clone, Copy)]
pub(super) enum ElementBoolField {
    Visible,
    Locked,
}

/// W2.5 — locate an element-level `bool` field (`visible` / `locked`)
/// on any of the five page-item kinds that carry `CommonAttrs`. The
/// `field` selector keeps the `ElementVisible` / `ElementLocked` apply
/// arms one line each without a closure capturing the borrow.
pub(super) fn find_element_bool_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
    field: ElementBoolField,
) -> Option<&'a mut bool> {
    let raw = node.self_id();
    let pick = |visible: &'a mut bool, locked: &'a mut bool| match field {
        ElementBoolField::Visible => visible,
        ElementBoolField::Locked => locked,
    };
    for parsed in &mut doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if let Some(p) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(pick(&mut p.visible, &mut p.locked));
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(p) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(pick(&mut p.visible, &mut p.locked));
                }
            }
            NodeId::Oval(_) => {
                if let Some(p) = parsed
                    .spread
                    .ovals
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(pick(&mut p.visible, &mut p.locked));
                }
            }
            NodeId::GraphicLine(_) => {
                if let Some(p) = parsed
                    .spread
                    .graphic_lines
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(pick(&mut p.visible, &mut p.locked));
                }
            }
            NodeId::Polygon(_) => {
                if let Some(p) = parsed
                    .spread
                    .polygons
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(pick(&mut p.visible, &mut p.locked));
                }
            }
            _ => {}
        }
    }
    None
}

/// SDK Phase 5 (D3 completion) — locate the `applied_object_style:
/// Option<String>` field on any page-item kind. All six page-item
/// variants carry the same field with identical semantics; this
/// helper makes the AppliedObjectStyle apply arm kind-agnostic.
pub(super) fn find_applied_object_style_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<String>> {
    let raw = node.self_id();
    for parsed in &mut doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if let Some(p) = parsed
                    .spread
                    .text_frames
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.applied_object_style);
                }
            }
            NodeId::Rectangle(_) => {
                if let Some(p) = parsed
                    .spread
                    .rectangles
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.applied_object_style);
                }
            }
            NodeId::Oval(_) => {
                if let Some(p) = parsed
                    .spread
                    .ovals
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.applied_object_style);
                }
            }
            NodeId::Polygon(_) => {
                if let Some(p) = parsed
                    .spread
                    .polygons
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.applied_object_style);
                }
            }
            NodeId::GraphicLine(_) => {
                if let Some(p) = parsed
                    .spread
                    .graphic_lines
                    .iter_mut()
                    .find(|p| p.self_id.as_deref() == Some(raw))
                {
                    return Some(&mut p.applied_object_style);
                }
            }
            // Group does not carry an `applied_object_style` field on
            // the parse-layer struct — object styles are applied to
            // leaf items, not structural containers. Falls through.
            _ => {}
        }
    }
    None
}

pub(super) fn find_spread<'a>(
    doc: &'a Document,
    self_id: &str,
) -> Option<&'a paged_scene::ParsedSpread> {
    doc.spreads
        .iter()
        .find(|p| p.spread.self_id.as_deref() == Some(self_id))
}

pub(super) fn find_spread_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut paged_scene::ParsedSpread> {
    doc.spreads
        .iter_mut()
        .find(|p| p.spread.self_id.as_deref() == Some(self_id))
}

pub(super) fn spread_parent_id(parsed: &paged_scene::ParsedSpread) -> NodeId {
    // Spreads always have a `self_id` in well-formed IDMLs; synthetic
    // test docs that omit it fall back to the manifest src path so the
    // inverse op still names the same container.
    let id = parsed
        .spread
        .self_id
        .clone()
        .unwrap_or_else(|| parsed.src.clone());
    NodeId::Spread(id)
}

/// Cheap document-wide existence check — used for duplicate-ID
/// detection on InsertNode.
pub(super) fn node_exists(doc: &Document, node: &NodeId) -> bool {
    let target = node.self_id();
    for parsed in &doc.spreads {
        match node {
            NodeId::TextFrame(_) => {
                if parsed
                    .spread
                    .text_frames
                    .iter()
                    .any(|f| f.self_id.as_deref() == Some(target))
                {
                    return true;
                }
            }
            NodeId::Rectangle(_) => {
                if parsed
                    .spread
                    .rectangles
                    .iter()
                    .any(|r| r.self_id.as_deref() == Some(target))
                {
                    return true;
                }
            }
            NodeId::GraphicLine(_) => {
                if parsed
                    .spread
                    .graphic_lines
                    .iter()
                    .any(|l| l.self_id.as_deref() == Some(target))
                {
                    return true;
                }
            }
            NodeId::Polygon(_) => {
                if parsed
                    .spread
                    .polygons
                    .iter()
                    .any(|p| p.self_id.as_deref() == Some(target))
                {
                    return true;
                }
            }
            NodeId::Oval(_) => {
                if parsed
                    .spread
                    .ovals
                    .iter()
                    .any(|o| o.self_id.as_deref() == Some(target))
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Track M — locate a `<Layer>` by its `Self` id in the document's
/// designmap. The designmap is the only place layers live; spread /
/// page items only carry an `ItemLayer` reference back into it.
pub(super) fn find_layer_mut<'a>(
    doc: &'a mut Document,
    self_id: &str,
) -> Option<&'a mut paged_parse::Layer> {
    doc.designmap
        .layers
        .iter_mut()
        .find(|l| l.self_id == self_id)
}

pub(super) fn expect_bool(path: PropertyPath, value: &Value) -> Result<bool, OperationError> {
    match value {
        Value::Bool(b) => Ok(*b),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "Bool".to_string(),
        }),
    }
}

pub(super) fn expect_text(path: PropertyPath, value: &Value) -> Result<String, OperationError> {
    match value {
        Value::Text(s) => Ok(s.clone()),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "Text".to_string(),
        }),
    }
}

pub(super) fn expect_gradient_feather(
    path: PropertyPath,
    value: &Value,
) -> Result<Option<GradientFeatherSpec>, OperationError> {
    match value {
        Value::GradientFeather(spec) => Ok(spec.clone()),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "GradientFeather".to_string(),
        }),
    }
}

/// Editor-ops — the `FrameEffects` block of an effect-bearing item,
/// materialising the default block when the item had none yet.
pub(super) fn find_frame_effects_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::FrameEffects> {
    match node {
        NodeId::TextFrame(id) => {
            find_text_frame_mut(doc, id).map(|f| f.effects.get_or_insert_with(Default::default))
        }
        NodeId::Rectangle(id) => {
            find_rectangle_mut(doc, id).map(|r| r.effects.get_or_insert_with(Default::default))
        }
        NodeId::Oval(id) => {
            find_oval_mut(doc, id).map(|o| o.effects.get_or_insert_with(Default::default))
        }
        _ => None,
    }
}

// W0.4 — per-effect mutable accessors. Each locates the
// `FrameEffects` bag (materialising it + the named effect block with
// its InDesign-preset default when the prior was `None`) so the
// per-field apply arms always have a target. Mirrors
// `find_drop_shadow_mut`. Returns `None` only when the node isn't an
// effect-bearing kind (TextFrame / Rectangle / Oval).
pub(super) fn find_inner_shadow_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::InnerShadowParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(
        effects
            .inner_shadow
            .get_or_insert_with(default_inner_shadow),
    )
}

pub(super) fn find_outer_glow_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::OuterGlowParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(effects.outer_glow.get_or_insert_with(default_outer_glow))
}

pub(super) fn find_inner_glow_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::InnerGlowParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(effects.inner_glow.get_or_insert_with(default_inner_glow))
}

pub(super) fn find_bevel_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::BevelEmbossParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(effects.bevel.get_or_insert_with(default_bevel))
}

pub(super) fn find_satin_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::SatinParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(effects.satin.get_or_insert_with(default_satin))
}

pub(super) fn find_feather_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::FeatherParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(effects.feather.get_or_insert_with(default_feather))
}

pub(super) fn find_directional_feather_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut paged_parse::DirectionalFeatherParams> {
    let effects = find_frame_effects_mut(doc, node)?;
    Some(
        effects
            .directional_feather
            .get_or_insert_with(default_directional_feather),
    )
}

// W0.4 — object-level transparency blend mode. Locates the
// `blend_mode: Option<String>` slot on the kinds that parse it
// (TextFrame / Rectangle). The `<BlendingSetting Opacity>` half is
// already wired as `FrameOpacity`.
pub(super) fn find_blend_mode_mut<'a>(
    doc: &'a mut Document,
    node: &NodeId,
) -> Option<&'a mut Option<String>> {
    match node {
        NodeId::TextFrame(id) => find_text_frame_mut(doc, id).map(|f| &mut f.blend_mode),
        NodeId::Rectangle(id) => find_rectangle_mut(doc, id).map(|r| &mut r.blend_mode),
        _ => None,
    }
}

pub(super) fn expect_bounds(path: PropertyPath, value: &Value) -> Result<[f32; 4], OperationError> {
    match value {
        Value::Bounds(b) => Ok(*b),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "Bounds".to_string(),
        }),
    }
}

pub(super) fn expect_color_ref(
    path: PropertyPath,
    value: &Value,
) -> Result<Option<String>, OperationError> {
    match value {
        Value::ColorRef(c) => Ok(c.clone()),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "ColorRef".to_string(),
        }),
    }
}

pub(super) fn expect_length(
    path: PropertyPath,
    value: &Value,
) -> Result<Option<f32>, OperationError> {
    match value {
        Value::Length(v) => Ok(*v),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "Length".to_string(),
        }),
    }
}

pub(super) fn expect_transform(
    path: PropertyPath,
    value: &Value,
) -> Result<Option<[f32; 6]>, OperationError> {
    match value {
        Value::Transform(m) => Ok(*m),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "Transform".to_string(),
        }),
    }
}

pub(super) fn expect_path_point(
    path: PropertyPath,
    value: &Value,
) -> Result<(crate::operation::PathPointAddress, [f32; 2]), OperationError> {
    match value {
        Value::PathPoint { address, position } => Ok((*address, *position)),
        _ => Err(OperationError::TypeMismatch {
            path,
            expected: "PathPoint".to_string(),
        }),
    }
}
