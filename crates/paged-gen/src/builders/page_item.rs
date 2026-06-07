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

//! Page-item shapes. Phase 0 only emits Rectangle (with optional
//! `parent_story` so a TextFrame-equivalent rectangle can host body
//! text). Phase-2 adds `<Group>` wrappers (with their own
//! `ItemTransform`) and `<Polygon>` items with custom `PathGeometry`
//! (multi-subpath compound paths).

use crate::geometry::{format_matrix, Matrix, IDENTITY};
use crate::xml::{format_f32, XmlBuilder};

/// One of the spread-level renderable items the spread builder knows
/// how to emit. Wraps the existing `Rect` and the newer `Group` /
/// `Polygon` shapes in a single enum so a `Spread` can carry a
/// heterogeneous tree of children.
pub enum PageItem {
    // `Rect` is ~550 bytes vs ≤128 for the others; box it so the enum
    // isn't sized to its largest variant (clippy::large_enum_variant).
    Rect(Box<Rect>),
    Group(Group),
    Polygon(Polygon),
}

impl From<Rect> for PageItem {
    fn from(r: Rect) -> Self {
        PageItem::Rect(Box::new(r))
    }
}

impl From<Group> for PageItem {
    fn from(g: Group) -> Self {
        PageItem::Group(g)
    }
}

impl From<Polygon> for PageItem {
    fn from(p: Polygon) -> Self {
        PageItem::Polygon(p)
    }
}

impl PageItem {
    pub fn write(&self, b: &mut XmlBuilder) {
        match self {
            PageItem::Rect(r) => r.write(b),
            PageItem::Group(g) => g.write(b),
            PageItem::Polygon(p) => p.write(b),
        }
    }
}

/// IDML `<Group>` — wraps any number of child page items and applies
/// its own `ItemTransform` on top of theirs. Spec §10.3 ("Group") +
/// §10.3.3 ("Nested Objects and IDML Structure"). The parser in
/// `paged-parse/src/spread.rs` recognises just the `ItemTransform`
/// attribute and pushes/pops a transform stack, so emitting `Self`
/// + `ItemTransform` is sufficient for round-trip.
pub struct Group {
    pub self_id: String,
    pub item_transform: Matrix,
    pub children: Vec<PageItem>,
}

impl Group {
    pub fn write(&self, b: &mut XmlBuilder) {
        let xform = format_matrix(&self.item_transform);
        b.start(
            "Group",
            &[("Self", self.self_id.as_str()), ("ItemTransform", &xform)],
        );
        for child in &self.children {
            child.write(b);
        }
        b.end("Group");
    }
}

/// One sub-path inside a `<Polygon>`'s `<PathGeometry>`. Each entry
/// becomes a single `<GeometryPathType>` element with its own
/// `<PathPointArray>`. Multiple sub-paths in one polygon = compound
/// path (visible via the renderer's even-odd fill rule).
pub struct PolygonSubPath {
    /// Anchor points walked in order. For a closed sub-path, the
    /// emitter sets `PathOpen="false"`; the points themselves don't
    /// repeat the first vertex.
    pub anchors: Vec<(f32, f32)>,
    pub closed: bool,
}

/// IDML `<Polygon>` with a fully custom `<PathGeometry>` containing
/// one or more sub-paths. Used for compound paths (e.g. a square
/// with a square hole).
pub struct Polygon {
    pub self_id: String,
    pub item_transform: Matrix,
    pub fill_color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_weight_pt: Option<f32>,
    pub subpaths: Vec<PolygonSubPath>,
}

impl Polygon {
    pub fn write(&self, b: &mut XmlBuilder) {
        let xform = format_matrix(&self.item_transform);
        let mut attrs: Vec<(&str, String)> = vec![
            ("Self", self.self_id.clone()),
            ("ItemTransform", xform),
            (
                "FillColor",
                self.fill_color
                    .clone()
                    .unwrap_or_else(|| "Swatch/None".to_string()),
            ),
            (
                "StrokeColor",
                self.stroke_color
                    .clone()
                    .unwrap_or_else(|| "Swatch/None".to_string()),
            ),
        ];
        if let Some(w) = self.stroke_weight_pt {
            attrs.push(("StrokeWeight", format_f32(w)));
        }
        let attr_refs: Vec<(&str, &str)> = attrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        b.start("Polygon", &attr_refs);
        b.start("Properties", &[]);
        b.start("PathGeometry", &[]);
        for sub in &self.subpaths {
            let open = if sub.closed { "false" } else { "true" };
            b.start("GeometryPathType", &[("PathOpen", open)]);
            b.start("PathPointArray", &[]);
            for (x, y) in &sub.anchors {
                let xy = format!("{} {}", format_f32(*x), format_f32(*y));
                b.empty(
                    "PathPointType",
                    &[
                        ("Anchor", &xy),
                        ("LeftDirection", &xy),
                        ("RightDirection", &xy),
                    ],
                );
            }
            b.end("PathPointArray");
            b.end("GeometryPathType");
        }
        b.end("PathGeometry");
        b.end("Properties");
        b.end("Polygon");
    }
}

/// Spec §10.3.1: a Rectangle (or any spline item) with `<Properties>`
/// holding a `<PathGeometry>` describing its closed bounding box in
/// inner coordinates.
pub struct Rect {
    pub self_id: String,
    pub width_pt: f32,
    pub height_pt: f32,
    pub item_transform: Matrix,
    pub fill_color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_weight_pt: Option<f32>,
    /// Optional `ParentStory` reference — when set, the rectangle
    /// becomes a text frame (kind = `TextFrame` in the XML). Phase-0
    /// labels live in stories on the page they describe.
    pub parent_story: Option<String>,
    /// `NextTextFrame` self-id — the frame that continues this story
    /// when its content overflows the current frame. `None` ⇒ the
    /// builder emits `"n"` (end-of-chain / unthreaded), matching the
    /// historical default. Only meaningful on a text frame (one with a
    /// `parent_story`); the chain is what makes a story overset across
    /// multiple frames instead of a single one.
    pub next_text_frame: Option<String>,
    /// `PreviousTextFrame` self-id — the frame this one continues from.
    /// `None` ⇒ the builder emits `"n"`. The renderer's chain walk
    /// keys off `NextTextFrame` only (it derives the head as the frame
    /// no `NextTextFrame` targets), but real InDesign exports always
    /// write the back-link too, so we mirror it for fidelity.
    pub previous_text_frame: Option<String>,
    /// Sample-specific attribute overrides emitted after the standard
    /// fill/stroke attrs (so they win on duplicate keys per IDML's
    /// "last attribute wins" reader behaviour). Values come straight
    /// from the IDML enum tables — `("StrokeType", "StrokeStyle/$ID/Dashed")`,
    /// `("EndCap", "RoundEndCap")`, `("StrokeAlignment", "InsideAlignment")`,
    /// etc. Avoids ballooning the struct as more samples land.
    pub extra_attrs: Vec<(String, String)>,
    /// `<BlendingSetting>` emitted inside
    /// `<Properties><TransparencySetting>`. None ⇒ no transparency
    /// element is emitted (default: opaque + Normal blend).
    pub blending: Option<Blending>,
    /// `<DropShadowSetting>` emitted alongside `BlendingSetting`.
    pub drop_shadow: Option<DropShadow>,
    /// Frame-effects family (`<InnerShadowSetting>` etc.) emitted inside
    /// the same `<TransparencySetting>` block. Each is written
    /// `Applied="true"`. Empty ⇒ none. Used by the effects sample's
    /// W1.3/W1.4 variant pages.
    pub frame_effects: Vec<EffectSetting>,
    /// Optional placed-image payload. When set, the rectangle becomes
    /// a graphic frame: a nested `<Image>` element carries the
    /// `LinkResourceURI`, and a sibling `<FrameFittingOption>` element
    /// describes how the image is cropped against the frame edges.
    pub placed_image: Option<PlacedImage>,
    /// Optional `<TextWrapPreference>` payload. When set, the page
    /// item carries a wrap-exclusion zone that other text frames on
    /// the same page must flow around.
    pub text_wrap: Option<TextWrap>,
    /// Optional `<AnchoredObjectSetting>` payload. Marks this frame
    /// as an anchored object — only meaningful when the frame is
    /// nested inside a `<CharacterStyleRange>` of a flowing story
    /// (the story emitter handles the nesting). Setting this on a
    /// top-level frame still emits the element so a parser
    /// round-trip records `is_anchored = true`.
    pub anchored_setting: Option<AnchoredObjectSetting>,
}

/// `<TextWrapPreference>` payload emitted as a child of a page item.
/// Mirrors `paged_parse::TextWrap` shape — mode + four offsets.
#[derive(Clone)]
pub struct TextWrap {
    /// `TextWrapMode` enum string: `"None"`, `"BoundingBoxTextWrap"`,
    /// `"ContourTextWrap"`, `"JumpObjectTextWrap"`,
    /// `"NextColumnTextWrap"`.
    pub mode: &'static str,
    /// `[top, left, bottom, right]` offsets in pt — the wrap
    /// rectangle is the page item's AABB inflated outwards by these.
    pub offsets: [f32; 4],
    /// `TextWrapSide` — `"BothSides"` (default), `"LeftSide"`,
    /// `"RightSide"`, `"LargestArea"`, `"SideAwayFromSpine"`,
    /// `"SideTowardsSpine"`. None ⇒ omit the attribute.
    pub side: Option<&'static str>,
}

/// `<AnchoredObjectSetting>` payload — IDML's spec for how an
/// anchored frame positions itself relative to its anchor in the host
/// story. Every attribute is optional so callers can lean on the IDML
/// defaults; the parser only inspects the *presence* of the element
/// today (sets `is_anchored = true`), but the attributes round-trip
/// through real InDesign readers.
#[derive(Clone)]
pub struct AnchoredObjectSetting {
    /// `"InlinePosition"` (in-line with text), `"AboveLine"` (own
    /// line above the host), or `"Anchored"` (custom positioned).
    pub anchored_position: &'static str,
    /// `SpineRelative="true"` mirrors the offsets on facing pages.
    pub spine_relative: bool,
    /// `LockPosition="true"` prevents manual positioning in InDesign.
    pub lock_position: bool,
    /// `PinPosition="true"` keeps the anchor's position unchanged
    /// when the host text reflows. InDesign's default is `true`.
    pub pin_position: bool,
    /// `AnchorPoint` — `"BottomRightAnchor"`, `"TopLeftAnchor"`, etc.
    pub anchor_point: Option<&'static str>,
    /// `HorizontalReferencePoint` — `"TextFrame"`, `"PageEdge"`,
    /// `"PageMargins"`, `"ColumnEdge"`, `"AnchorLocation"`.
    pub horizontal_reference_point: Option<&'static str>,
    pub vertical_reference_point: Option<&'static str>,
    pub horizontal_alignment: Option<&'static str>,
    pub vertical_alignment: Option<&'static str>,
    /// `AnchorXoffset` / `AnchorYoffset` in pt — explicit numerical
    /// offsets used when `anchored_position == "Anchored"`.
    pub anchor_x_offset: Option<f32>,
    pub anchor_y_offset: Option<f32>,
}

impl AnchoredObjectSetting {
    /// Inline-position default — anchored as if the frame were a
    /// glyph in the host text run.
    pub fn inline() -> Self {
        Self {
            anchored_position: "InlinePosition",
            spine_relative: false,
            lock_position: false,
            pin_position: true,
            anchor_point: Some("BottomRightAnchor"),
            horizontal_reference_point: Some("TextFrame"),
            vertical_reference_point: Some("LineBaseline"),
            horizontal_alignment: Some("LeftAlign"),
            vertical_alignment: Some("TopAlign"),
            anchor_x_offset: None,
            anchor_y_offset: None,
        }
    }
}

/// IDML `<Image>` + `<FrameFittingOption>` payload nested inside a
/// `<Rectangle>`. The renderer maps the image to the frame's inner-
/// coordinate rect minus the four crop offsets — so the crops are
/// what actually determine "fit to frame" / "center content" /
/// "fill proportionally" placement (the `FittingOnEmptyFrame` enum
/// is descriptive, not authoritative, as far as the renderer is
/// concerned).
#[derive(Clone)]
pub struct PlacedImage {
    /// Where the link resolves. `"file:<basename>.ext"` keeps the
    /// asset-resolver lookup simple — `--links-dir` joins the
    /// basename onto its search dirs.
    pub link_resource_uri: String,
    /// `FittingOnEmptyFrame` value: `None | Proportionally |
    /// FillProportionally | FitContent | FitContentToFrame |
    /// CenterContent | ContentAwareFit`.
    pub fitting: &'static str,
    /// Per-side crops in pt. Positive shrinks the image inward from
    /// the frame edge; negative grows it outward (used by
    /// `FillProportionally` to overflow on one axis).
    pub left_crop: f32,
    pub top_crop: f32,
    pub right_crop: f32,
    pub bottom_crop: f32,
    /// Self id for the inner `<Image>` element. Stable across runs
    /// so determinism holds.
    pub image_self_id: String,
    /// Native pixel dimensions of the image in pt (1 px = 1 pt at
    /// 72 DPI), used for the inner `<PathGeometry>` describing the
    /// unscaled image bounds. Currently emitted purely for shape;
    /// the renderer's placement uses the frame rect + crops.
    pub image_w_pt: f32,
    pub image_h_pt: f32,
    /// Override for the inner `<Image ItemTransform>`. None ⇒
    /// identity (`"1 0 0 1 0 0"`). Real InDesign exports often store
    /// the actual scale-translate-rotate of the placed image here
    /// while the `<Rectangle>` carries the frame's transform
    /// separately. Tests of "image transform on top of frame
    /// transform" need this knob.
    pub image_item_transform: Option<Matrix>,
    /// `EffectivePpi` attribute on `<Image>` — emitted as a
    /// parenthesised `"(x y)"` pair (e.g. `"(144 144)"`) to match real
    /// InDesign serialisation. When `None`, the attribute is omitted
    /// (parser then leaves `effective_ppi = None`, and the canvas may
    /// derive it from pixel dims ÷ placed size if it has the decode).
    pub effective_ppi: Option<(f32, f32)>,
    /// `ActualPpi` attribute on `<Image>` — the source asset's native
    /// resolution, parenthesised `"(x y)"`. Emitted alongside
    /// `EffectivePpi` so a low-effective-PPI placement reads honestly
    /// (a high `ActualPpi` scaled down to a low `EffectivePpi`). `None`
    /// ⇒ omitted.
    pub actual_ppi: Option<(f32, f32)>,
    /// `Space` attribute on `<Image>` — the colour space InDesign
    /// records (`"$ID/RGB"`, `"$ID/CMYK"`, …). Drives the Links
    /// panel's colour-space column. `None` ⇒ omitted.
    pub color_space: Option<&'static str>,
    /// Raw image bytes to inline as a base64 `<Contents>` CDATA payload
    /// under the `<Image><Properties>`. When `Some`, the placed image
    /// resolves to these bytes at build time (no external file / asset
    /// resolver needed), so its host frame renders "ok" rather than the
    /// missing-image placeholder. `None` ⇒ link-only (resolved via the
    /// URI; "missing" when nothing on disk answers it).
    pub inline_bytes: Option<Vec<u8>>,
}

/// IDML `<BlendingSetting>` — `Opacity` is 0..=100, `BlendMode` is
/// the standard enum (`Normal`, `Multiply`, `Screen`, `Overlay`,
/// `Multiply`, `Darken`, `Lighten`, etc).
#[derive(Clone, Default)]
pub struct Blending {
    pub opacity_pct: Option<f32>,
    pub blend_mode: Option<&'static str>,
}

/// IDML `<DropShadowSetting>` — distances in pt, `opacity_pct` is
/// 0..=100, `effect_color` references a Color self id.
///
/// Every attribute except `mode` is optional: when `None`, it is
/// omitted from the emitted XML so downstream consumers (the parser
/// and InDesign itself) fall back to the IDML defaults documented in
/// §IDML Defaults Table 84 (`XOffset=7`, `YOffset=7`, `Size=5`,
/// `Opacity=75`, `EffectColor=Black`). Use the `default_drop()` helper
/// to construct a "use IDML defaults for everything" shadow.
#[derive(Clone)]
pub struct DropShadow {
    /// `Mode` — typically `"Drop"` for an enabled shadow,
    /// `"None"` to serialise but disable.
    pub mode: &'static str,
    pub x_offset: Option<f32>,
    pub y_offset: Option<f32>,
    pub size: Option<f32>,
    pub opacity_pct: Option<f32>,
    pub effect_color: Option<String>,
}

impl DropShadow {
    /// `<DropShadowSetting Mode="Drop"/>` — a shadow that emits no
    /// attributes other than `Mode`, so every other parameter falls
    /// through to the IDML default. Lets samples exercise the
    /// parser's default-fill-in path (vs. an explicit-value shadow).
    pub fn default_drop() -> Self {
        Self {
            mode: "Drop",
            x_offset: None,
            y_offset: None,
            size: None,
            opacity_pct: None,
            effect_color: None,
        }
    }
}

/// One serialised `<*Setting>` element for the frame-effects family
/// (InnerShadow / OuterGlow / InnerGlow / BevelAndEmboss / Satin /
/// Feather / DirectionalFeather). The sample builder splices these,
/// `Applied="true"`, into the `<TransparencySetting>` so the renderer's
/// parse → compose → rasterize chain is exercised end to end (W1.3
/// reconcile / W1.4 parity). `element` is the bare tag name (e.g.
/// `"BevelAndEmbossSetting"`); `attrs` are the per-effect attributes
/// (`Style`, `Direction`, `LeftWidth`, …) appended verbatim.
#[derive(Clone)]
pub struct EffectSetting {
    pub element: &'static str,
    pub attrs: Vec<(&'static str, String)>,
}

impl EffectSetting {
    /// Construct from a static attribute list (numbers pre-formatted by
    /// the caller). Always emits `Applied="true"`.
    pub fn new(element: &'static str, attrs: Vec<(&'static str, String)>) -> Self {
        Self { element, attrs }
    }

    fn write(&self, b: &mut XmlBuilder) {
        let mut a: Vec<(&str, &str)> = Vec::with_capacity(self.attrs.len() + 1);
        a.push(("Applied", "true"));
        for (k, v) in &self.attrs {
            a.push((k, v.as_str()));
        }
        b.empty(self.element, &a);
    }
}

impl Rect {
    /// Convenience constructor for the common "filled rectangle, no
    /// stroke, no parent story" shape.
    pub fn filled(self_id: impl Into<String>, w: f32, h: f32, item_transform: Matrix) -> Self {
        Self {
            self_id: self_id.into(),
            width_pt: w,
            height_pt: h,
            item_transform,
            fill_color: Some("Color/Black".into()),
            stroke_color: None,
            stroke_weight_pt: None,
            parent_story: None,
            next_text_frame: None,
            previous_text_frame: None,
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
            text_wrap: None,
            anchored_setting: None,
            frame_effects: Vec::new(),
        }
    }

    pub fn with_fill(mut self, color: impl Into<String>) -> Self {
        self.fill_color = Some(color.into());
        self
    }

    pub fn with_stroke(mut self, color: impl Into<String>, weight_pt: f32) -> Self {
        self.stroke_color = Some(color.into());
        self.stroke_weight_pt = Some(weight_pt);
        self
    }

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_attrs.push((key.into(), value.into()));
        self
    }

    pub fn with_parent_story(mut self, story_id: impl Into<String>) -> Self {
        self.parent_story = Some(story_id.into());
        self
    }
}

impl Rect {
    /// Emit either `<Rectangle .../>` or `<TextFrame .../>` depending
    /// on whether a parent story was attached.
    pub fn write(&self, b: &mut XmlBuilder) {
        let kind = if self.parent_story.is_some() {
            "TextFrame"
        } else {
            "Rectangle"
        };
        let mut attrs: Vec<(&str, String)> = Vec::new();
        attrs.push(("Self", self.self_id.clone()));
        if let Some(story) = &self.parent_story {
            attrs.push(("ParentStory", story.clone()));
            attrs.push((
                "PreviousTextFrame",
                self.previous_text_frame
                    .clone()
                    .unwrap_or_else(|| "n".to_string()),
            ));
            attrs.push((
                "NextTextFrame",
                self.next_text_frame
                    .clone()
                    .unwrap_or_else(|| "n".to_string()),
            ));
            attrs.push(("ContentType", "TextType".to_string()));
        }
        // Pin AppliedObjectStyle to the built-in `[None]` style so
        // InDesign doesn't cascade the default Object Style's 1pt
        // black stroke (and other surprises) over our explicit
        // attributes. Real InDesign exports always emit this — even
        // when the object visually has no style applied. Without it
        // the BlendingSetting cascade under the default object style
        // overrides our per-rectangle BlendingSetting back to Normal,
        // and StrokeColor="Swatch/None" gets shadowed by the default
        // 1pt stroke.
        attrs.push(("AppliedObjectStyle", "ObjectStyle/$ID/[None]".to_string()));
        attrs.push(("Visible", "true".to_string()));
        attrs.push(("Name", "$ID/".to_string()));
        attrs.push(("ItemTransform", format_matrix(&self.item_transform)));
        attrs.push((
            "FillColor",
            self.fill_color
                .clone()
                .unwrap_or_else(|| "Swatch/None".to_string()),
        ));
        attrs.push((
            "StrokeColor",
            self.stroke_color
                .clone()
                .unwrap_or_else(|| "Swatch/None".to_string()),
        ));
        // Always emit StrokeWeight — when no stroke is wanted, "0"
        // makes the no-stroke explicit so InDesign's cascade default
        // doesn't fill in 1pt.
        let stroke_weight = self.stroke_weight_pt.unwrap_or(0.0);
        attrs.push(("StrokeWeight", format_f32(stroke_weight)));
        for (k, v) in &self.extra_attrs {
            attrs.push((k.as_str(), v.clone()));
        }
        let attr_refs: Vec<(&str, &str)> = attrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        b.start(kind, &attr_refs);
        b.start("Properties", &[]);
        write_path_geometry(b, self.width_pt, self.height_pt);
        b.end("Properties");
        // TransparencySetting is a SIBLING of Properties under the
        // page item, not a child (spec §IDML File Reference: Spreads
        // and Master Spreads — Rectangle content model). When we
        // earlier nested it inside Properties, InDesign silently
        // dropped the BlendingSetting and the blend reverted to
        // Normal in the exported PDF.
        if self.blending.is_some() || self.drop_shadow.is_some() || !self.frame_effects.is_empty() {
            b.start("TransparencySetting", &[]);
            if let Some(bl) = &self.blending {
                let opacity_str: String;
                let mut a: Vec<(&str, &str)> = Vec::new();
                if let Some(o) = bl.opacity_pct {
                    opacity_str = format_f32(o);
                    a.push(("Opacity", opacity_str.as_str()));
                }
                if let Some(m) = bl.blend_mode {
                    a.push(("BlendMode", m));
                }
                b.empty("BlendingSetting", &a);
            }
            if let Some(ds) = &self.drop_shadow {
                // Optional attributes are emitted only when set so a
                // `<DropShadowSetting Mode="Drop"/>` round-trips
                // through the parser into the IDML defaults (§IDML
                // Defaults Table 84) rather than being pinned to
                // zeroes.
                let xo = ds.x_offset.map(format_f32);
                let yo = ds.y_offset.map(format_f32);
                let sz = ds.size.map(format_f32);
                let op = ds.opacity_pct.map(format_f32);
                let mut a: Vec<(&str, &str)> = Vec::with_capacity(6);
                a.push(("Mode", ds.mode));
                if let Some(xo) = &xo {
                    a.push(("XOffset", xo.as_str()));
                }
                if let Some(yo) = &yo {
                    a.push(("YOffset", yo.as_str()));
                }
                if let Some(sz) = &sz {
                    a.push(("Size", sz.as_str()));
                }
                if let Some(op) = &op {
                    a.push(("Opacity", op.as_str()));
                }
                if let Some(ec) = &ds.effect_color {
                    a.push(("EffectColor", ec.as_str()));
                }
                b.empty("DropShadowSetting", &a);
            }
            // Frame-effects family — InnerShadow / OuterGlow /
            // InnerGlow / BevelAndEmboss / Satin / Feather /
            // DirectionalFeather, each `Applied="true"`.
            for fx in &self.frame_effects {
                fx.write(b);
            }
            b.end("TransparencySetting");
        }
        if let Some(img) = &self.placed_image {
            // `<FrameFittingOption>` is a direct child of the
            // Rectangle (not inside Properties) — matches what
            // InDesign emits and what the paged-parse Rectangle
            // descendant walker expects.
            let lc = format_f32(img.left_crop);
            let tc = format_f32(img.top_crop);
            let rc = format_f32(img.right_crop);
            let bc = format_f32(img.bottom_crop);
            b.empty(
                "FrameFittingOption",
                &[
                    ("LeftCrop", lc.as_str()),
                    ("TopCrop", tc.as_str()),
                    ("RightCrop", rc.as_str()),
                    ("BottomCrop", bc.as_str()),
                    ("FittingOnEmptyFrame", img.fitting),
                ],
            );
            // `<Image>` carries its own Properties / PathGeometry
            // describing the unscaled native image extents (so
            // strict consumers see a complete object), plus a
            // `<Link>` child that mirrors the URI on the Image
            // element itself. The renderer reads either source.
            let img_xform = img
                .image_item_transform
                .as_ref()
                .map(format_matrix)
                .unwrap_or_else(|| "1 0 0 1 0 0".to_string());
            let mut img_attrs: Vec<(&str, &str)> = vec![
                ("Self", img.image_self_id.as_str()),
                ("ItemTransform", img_xform.as_str()),
                ("LinkResourceURI", img.link_resource_uri.as_str()),
            ];
            if let Some(space) = img.color_space {
                img_attrs.push(("Space", space));
            }
            // ActualPpi / EffectivePpi serialise as a parenthesised
            // `"(x y)"` pair — what real InDesign writes and what
            // paged-parse's `parse_ppi_x` expects (it tolerates a bare
            // `"x"` too, but the canonical form round-trips cleanly).
            let actual_ppi_str: String;
            if let Some((px, py)) = img.actual_ppi {
                actual_ppi_str = format!("({} {})", format_f32(px), format_f32(py));
                img_attrs.push(("ActualPpi", actual_ppi_str.as_str()));
            }
            let ppi_str: String;
            if let Some((px, py)) = img.effective_ppi {
                ppi_str = format!("({} {})", format_f32(px), format_f32(py));
                img_attrs.push(("EffectivePpi", ppi_str.as_str()));
            }
            b.start("Image", &img_attrs);
            b.start("Properties", &[]);
            write_path_geometry(b, img.image_w_pt, img.image_h_pt);
            // Inline-embedded image bytes: a base64 `<Contents>` CDATA
            // payload sibling of the PathGeometry. The parser captures
            // this into `image_bytes`, so the renderer resolves it
            // without an external file — the frame renders "ok" rather
            // than the missing-image placeholder.
            if let Some(bytes) = &img.inline_bytes {
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                b.start("Contents", &[]);
                b.cdata(&encoded);
                b.end("Contents");
            }
            b.end("Properties");
            b.empty(
                "Link",
                &[("LinkResourceURI", img.link_resource_uri.as_str())],
            );
            b.end("Image");
        }
        // `<TextWrapPreference>` — sibling of Properties / Image. The
        // parser inspects this on every shape kind (text frame,
        // rectangle, oval, polygon, line); the renderer's wrap-rect
        // collector pulls AABB + offsets from here per page.
        if let Some(tw) = &self.text_wrap {
            let top = format_f32(tw.offsets[0]);
            let left = format_f32(tw.offsets[1]);
            let bottom = format_f32(tw.offsets[2]);
            let right = format_f32(tw.offsets[3]);
            let mut twa: Vec<(&str, &str)> =
                vec![("Inverse", "false"), ("ApplyToMasterPageOnly", "false")];
            if let Some(side) = tw.side {
                twa.push(("TextWrapSide", side));
            }
            twa.push(("TextWrapMode", tw.mode));
            b.start("TextWrapPreference", &twa);
            b.start("Properties", &[]);
            b.empty(
                "TextWrapOffset",
                &[
                    ("Top", top.as_str()),
                    ("Left", left.as_str()),
                    ("Bottom", bottom.as_str()),
                    ("Right", right.as_str()),
                ],
            );
            b.end("Properties");
            b.end("TextWrapPreference");
        }
        // `<AnchoredObjectSetting>` — sibling of Properties. The
        // parser sets `is_anchored = true` on whichever shape is
        // currently open; the host story emitter is responsible for
        // putting that shape inside a CharacterStyleRange so the
        // renderer's text-flow integration sees it.
        if let Some(a) = &self.anchored_setting {
            let xo;
            let yo;
            let mut aa: Vec<(&str, &str)> = Vec::new();
            aa.push(("AnchoredPosition", a.anchored_position));
            aa.push((
                "SpineRelative",
                if a.spine_relative { "true" } else { "false" },
            ));
            aa.push((
                "LockPosition",
                if a.lock_position { "true" } else { "false" },
            ));
            aa.push(("PinPosition", if a.pin_position { "true" } else { "false" }));
            if let Some(p) = a.anchor_point {
                aa.push(("AnchorPoint", p));
            }
            if let Some(p) = a.horizontal_reference_point {
                aa.push(("HorizontalReferencePoint", p));
            }
            if let Some(p) = a.vertical_reference_point {
                aa.push(("VerticalReferencePoint", p));
            }
            if let Some(p) = a.horizontal_alignment {
                aa.push(("HorizontalAlignment", p));
            }
            if let Some(p) = a.vertical_alignment {
                aa.push(("VerticalAlignment", p));
            }
            if let Some(o) = a.anchor_x_offset {
                xo = format_f32(o);
                aa.push(("AnchorXoffset", xo.as_str()));
            } else {
                aa.push(("AnchorXoffset", "0"));
            }
            if let Some(o) = a.anchor_y_offset {
                yo = format_f32(o);
                aa.push(("AnchorYoffset", yo.as_str()));
            } else {
                aa.push(("AnchorYoffset", "0"));
            }
            b.empty("AnchoredObjectSetting", &aa);
        }
        b.end(kind);
    }
}

fn write_path_geometry(b: &mut XmlBuilder, w: f32, h: f32) {
    // Rectangle anchored at (0, 0) with the given inner extents.
    // Spec §10.3.2: PathPointArray walks corners; each anchor stores
    // its on-curve position plus the (degenerate) Bezier handles.
    b.start("PathGeometry", &[]);
    b.start("GeometryPathType", &[("PathOpen", "false")]);
    b.start("PathPointArray", &[]);
    let corners = [(0.0, 0.0), (0.0, h), (w, h), (w, 0.0)];
    for (x, y) in corners {
        let xy = format!("{} {}", format_f32(x), format_f32(y));
        b.empty(
            "PathPointType",
            &[
                ("Anchor", &xy),
                ("LeftDirection", &xy),
                ("RightDirection", &xy),
            ],
        );
    }
    b.end("PathPointArray");
    b.end("GeometryPathType");
    b.end("PathGeometry");
}

/// `IDENTITY` exported for builders that want a concrete `Matrix` to
/// pass through.
pub const fn identity_transform() -> Matrix {
    IDENTITY
}
