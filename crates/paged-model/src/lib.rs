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

//! The Paged document **model** — the Paged-owned data types and their pure
//! value logic (geometry, IDML token maps), with **no XML/ZIP parsing**.
//!
//! This is the foundation of the fork: the model is Paged's, not IDML's. The
//! IDML parser (`paged-parse` today, destined for the import/export adapter)
//! *depends on* this crate and imports into it; the render/mutate stack speaks
//! these types. Serde-serializable (it backs the native `.paged` codec).
//!
//! N5: the model is being lifted out of the parser crate incrementally — the
//! split axis is "touches quick-xml/zip" (stays in the parser) vs "pure value
//! logic" (moves here). This is the first slice: the foundational geometry
//! primitive. `paged-parse` re-exports everything moved here, so its dependents
//! compile unchanged.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// An axis-aligned bounding box in points: `top`, `left`, `bottom`, `right`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Bounds {
    pub top: f32,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
}

impl Bounds {
    pub const ZERO: Bounds = Bounds {
        top: 0.0,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    };
    pub fn width(&self) -> f32 {
        self.right - self.left
    }
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
}

/// IDML `<TextFramePreference VerticalJustification="...">` values.
/// `Top` is the IDML default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerticalJustification {
    Top,
    Center,
    Bottom,
    /// "JustifyAlign" — distributes the per-frame slack as extra
    /// space between paragraphs so the last paragraph's baseline
    /// reaches the frame's effective bottom. Line spacing within
    /// each paragraph is preserved; only inter-paragraph gaps grow.
    Justify,
}

impl VerticalJustification {
    /// Parse an IDML attribute value. Unknown values return `None`.
    pub fn from_idml(s: &str) -> Option<Self> {
        match s {
            "TopAlign" => Some(Self::Top),
            "CenterAlign" => Some(Self::Center),
            "BottomAlign" => Some(Self::Bottom),
            "JustifyAlign" => Some(Self::Justify),
            _ => None,
        }
    }
}

/// IDML `<TextFramePreference FirstBaselineOffset="...">` values.
/// Drives where the first line's baseline sits inside the frame's
/// inset box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FirstBaselineOffset {
    /// IDML default. Baseline at one ascender below the top inset.
    AscentOffset,
    /// Baseline at one cap-height below the top inset.
    CapHeight,
    /// Baseline at one x-height below the top inset.
    XHeight,
    /// Baseline at one em-box-height below the top inset.
    EmBoxHeight,
    /// Distance is `MinimumFirstBaselineOffset` pt below the inset.
    LeadingOffset,
    /// Same as LeadingOffset; both consult the
    /// `MinimumFirstBaselineOffset` pt value.
    FixedHeight,
}

impl FirstBaselineOffset {
    pub fn from_idml(s: &str) -> Option<Self> {
        match s {
            "AscentOffset" => Some(Self::AscentOffset),
            "CapHeight" => Some(Self::CapHeight),
            "XHeight" => Some(Self::XHeight),
            "EmBoxHeight" => Some(Self::EmBoxHeight),
            "LeadingOffset" => Some(Self::LeadingOffset),
            "FixedHeight" => Some(Self::FixedHeight),
            _ => None,
        }
    }
}

/// IDML `<TextFramePreference AutoSizingType="...">` values. Drives
/// whether the frame's bounds grow at composition time so display
/// headlines / dynamic copy don't clip against their authored bounds.
/// `Off` is the IDML default (static frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoSizingType {
    /// Static frame — bounds are authoritative.
    Off,
    /// Frame may grow vertically to fit text.
    HeightOnly,
    /// Frame may grow horizontally to fit the longest line.
    WidthOnly,
    /// Frame may grow in both directions.
    HeightAndWidth,
    /// Same as `HeightAndWidth` but maintains the original aspect
    /// ratio while growing.
    HeightAndWidthProportionally,
}

impl AutoSizingType {
    pub fn from_idml(s: &str) -> Option<Self> {
        match s {
            "Off" => Some(Self::Off),
            "HeightOnly" => Some(Self::HeightOnly),
            "WidthOnly" => Some(Self::WidthOnly),
            "HeightAndWidth" => Some(Self::HeightAndWidth),
            "HeightAndWidthProportionally" => Some(Self::HeightAndWidthProportionally),
            _ => None,
        }
    }

    /// True when the frame is allowed to grow in width.
    pub fn grows_width(self) -> bool {
        matches!(
            self,
            Self::WidthOnly | Self::HeightAndWidth | Self::HeightAndWidthProportionally
        )
    }

    /// True when the frame is allowed to grow in height.
    pub fn grows_height(self) -> bool {
        matches!(
            self,
            Self::HeightOnly | Self::HeightAndWidth | Self::HeightAndWidthProportionally
        )
    }
}

/// IDML `<TextFramePreference AutoSizingReferencePoint="...">` values.
/// Pins which corner / midpoint of the frame stays fixed when the
/// frame grows. `TopLeftPoint` is the IDML default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoSizingReferencePoint {
    TopLeftPoint,
    TopCenterPoint,
    TopRightPoint,
    CenterLeftPoint,
    CenterPoint,
    CenterRightPoint,
    BottomLeftPoint,
    BottomCenterPoint,
    BottomRightPoint,
}

impl AutoSizingReferencePoint {
    pub fn from_idml(s: &str) -> Option<Self> {
        match s {
            "TopLeftPoint" => Some(Self::TopLeftPoint),
            "TopCenterPoint" => Some(Self::TopCenterPoint),
            "TopRightPoint" => Some(Self::TopRightPoint),
            "CenterLeftPoint" => Some(Self::CenterLeftPoint),
            "CenterPoint" => Some(Self::CenterPoint),
            "CenterRightPoint" => Some(Self::CenterRightPoint),
            "BottomLeftPoint" => Some(Self::BottomLeftPoint),
            "BottomCenterPoint" => Some(Self::BottomCenterPoint),
            "BottomRightPoint" => Some(Self::BottomRightPoint),
            _ => None,
        }
    }
}

/// Drop shadow as carried in the IDML XML. Distances are in pt;
/// `opacity_pct` is 0..=100; `effect_color` is a Color id reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropShadowSetting {
    pub mode: String,
    pub x_offset: f32,
    pub y_offset: f32,
    pub size: f32,
    pub opacity_pct: f32,
    pub effect_color: Option<String>,
}

/// Authoritative placed-image metadata InDesign bakes onto the
/// `<Image>` element at export. Unlike a fresh decode, these reflect
/// the *placed* state (the link's resolution, the colour space the
/// asset was in, and the effective ppi after the frame's scale), so
/// the Links panel can surface colour-space + resolution warnings
/// (panels.md gaps 2-3) without resolving and decoding the asset
/// bytes — which the canvas worker may not even have.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageMetadata {
    /// `Space` attribute — the image's colour space as InDesign sees
    /// it: `"$ID/CMYK"`, `"$ID/RGB"`, `"$ID/Gray"`, `"$ID/LAB"`, etc.
    /// Stored stripped of the `$ID/` namespace prefix (`"CMYK"`).
    /// `None` when the element omits it (synthetic IDMLs, vector
    /// placements).
    pub space: Option<String>,
    /// `ActualPpi` x-resolution — the source asset's native ppi
    /// before the placement scale. IDML writes `"(x y)"`; we keep the
    /// x component (square pixels are near-universal). `None` when
    /// absent.
    pub actual_ppi: Option<f32>,
    /// `EffectivePpi` x-resolution — the native ppi divided by the
    /// placement scale, i.e. the resolution at print size. This is
    /// the number a preflight check compares against a 300-ppi
    /// threshold. `None` when absent (then the canvas may derive it
    /// from pixel-dims ÷ placed-size if it has the decode).
    pub effective_ppi: Option<f32>,
}

/// IDML `<ClippingPathSettings ClippingType="...">` value. InDesign
/// clips a placed image to a path *in addition to* the frame outline.
/// The type drives where the path comes from:
///
/// * `None` — no clip; the frame outline is the only crop.
/// * `UserModifiedPath` — a hand-edited path whose *resolved geometry
///   is serialised in the IDML* (a `<PathGeometry>` child of
///   `<ClippingPathSettings>`). This is the variant the renderer can
///   honour from the XML alone.
/// * `PhotoshopPath` — a named path stored in the linked image's 8BIM
///   resources. The IDML records only `AppliedPathName`; the geometry
///   lives in the image binary, so without 8BIM extraction the renderer
///   defers (renders unclipped + a diagnostic).
/// * `AlphaChannel` — clip from a named alpha channel; needs raster
///   analysis of the image, deferred.
/// * `DetectEdges` — clip auto-traced from luminance; needs raster
///   analysis, deferred.
///
/// Unknown strings collapse to `Other` (deferred, like the raster
/// types) so a future InDesign value never silently mis-clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClippingType {
    None,
    PhotoshopPath,
    AlphaChannel,
    DetectEdges,
    UserModifiedPath,
    Other,
}

impl ClippingType {
    pub fn from_idml(s: &str) -> Self {
        // InDesign serialises these bare (no `$ID/` prefix) on
        // `ClippingPathSettings/@ClippingType`.
        match s {
            "None" => Self::None,
            "PhotoshopPath" => Self::PhotoshopPath,
            "AlphaChannel" => Self::AlphaChannel,
            "DetectEdges" => Self::DetectEdges,
            "UserModifiedPath" => Self::UserModifiedPath,
            _ => Self::Other,
        }
    }

    /// Whether this type can ever carry resolvable geometry inside the
    /// IDML XML. Only `UserModifiedPath` does in practice; the others
    /// reference a resource in the image binary (8BIM path / alpha
    /// channel) or need raster edge-detection, all out of XML scope.
    pub fn geometry_may_be_inline(self) -> bool {
        matches!(self, Self::UserModifiedPath)
    }
}

/// Q-16: per-corner override for `Rectangle::corners`. IDML lists
/// these on `<Rectangle>` as `TopLeftCornerOption` / `TopLeftCornerRadius`
/// and the other three corners. When both fields are `None` the
/// renderer falls back to the legacy single `corner_option` /
/// `corner_radius` pair (which itself defaults to "no rounding").
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CornerSpec {
    pub option: Option<CornerOption>,
    pub radius: Option<f32>,
}

/// IDML `CornerOption` enum (per-corner or document-default). The
/// renderer emits bespoke geometry per variant (Rounded / Inverse /
/// Bevel / Inset / Fancy); `Inset` and `Fancy` are approximations
/// pending reference-PDF calibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CornerOption {
    /// IDML defaults: square corners.
    None,
    Rounded,
    Inverse,
    Inset,
    Bevel,
    Fancy,
}

impl CornerOption {
    pub fn from_idml(s: &str) -> Option<Self> {
        match s {
            "None" => Some(Self::None),
            "RoundedCorner" | "Rounded" => Some(Self::Rounded),
            "InverseRoundedCorner" | "InverseRounded" => Some(Self::Inverse),
            "InsetCorner" | "Inset" => Some(Self::Inset),
            "BeveledCorner" | "Beveled" | "Bevel" => Some(Self::Bevel),
            "FancyCorner" | "Fancy" => Some(Self::Fancy),
            _ => None,
        }
    }

    /// Whether this corner option produces a non-square corner (i.e.
    /// consumes the corner radius). Every variant except `None` does.
    pub fn rounds(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// IDML `<GraphicLine>` arrowhead style (`LeftLineEnd` / `RightLineEnd`).
/// One variant per token of InDesign's `ArrowHead` enumeration (the 11
/// stroke-panel line ends + `None` — the XML attribute carries the
/// enumeration's CamelCase name, `"TriangleArrowHead"` etc.);
/// unrecognised-but-present names become `Other` (drawn as a triangle
/// and counted as approximated; [`Self::as_idml`] can't reproduce the
/// source token for it, so writers leave `Other` untouched).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArrowheadType {
    None,
    /// Open / simple arrow — drawn as a filled triangle.
    Simple,
    /// Wide open arrow — drawn as a wide filled triangle.
    SimpleWide,
    Triangle,
    TriangleWide,
    /// Swallow-tail arrow (notched back edge).
    Barbed,
    /// Curved swept arrow — approximated as a filled triangle.
    Curved,
    /// Open (outlined) circle — drawn as a ring.
    Circle,
    CircleSolid,
    /// Open (outlined) square — drawn as a hollow square.
    Square,
    SquareSolid,
    /// Thin perpendicular bar across the line end.
    Bar,
    /// A recognised-but-unmapped end style; drawn as a triangle.
    Other,
}

impl ArrowheadType {
    /// The short aliases double as the mutate/wire-friendly spellings;
    /// `*Head` forms are kept for fixtures written before the
    /// vocabulary was verified against InDesign's enumeration.
    pub fn from_idml(s: &str) -> Self {
        match s {
            "None" | "" => Self::None,
            "SimpleArrowHead" | "Simple" => Self::Simple,
            "SimpleWideArrowHead" | "SimpleWide" => Self::SimpleWide,
            "TriangleArrowHead" | "TriangleHead" | "Triangle" => Self::Triangle,
            "TriangleWideArrowHead" | "TriangleWideHead" | "TriangleWide" => Self::TriangleWide,
            "BarbedArrowHead" | "Barbed" => Self::Barbed,
            "CurvedArrowHead" | "Curved" => Self::Curved,
            "CircleArrowHead" | "CircleHead" | "Circle" => Self::Circle,
            "CircleSolidArrowHead" | "CircleSolidHead" | "CircleSolid" => Self::CircleSolid,
            "SquareArrowHead" | "Square" => Self::Square,
            "SquareSolidArrowHead" | "SquareSolidHead" | "SquareSolid" => Self::SquareSolid,
            "BarArrowHead" | "BarHead" | "Bar" => Self::Bar,
            _ => Self::Other,
        }
    }

    /// The canonical IDML attribute token. `Other` yields `""` — the
    /// original source spelling was discarded at parse time, so callers
    /// that serialise (paged-write, mutate inverses) must treat `Other`
    /// as not-representable rather than write the empty string.
    pub fn as_idml(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Simple => "SimpleArrowHead",
            Self::SimpleWide => "SimpleWideArrowHead",
            Self::Triangle => "TriangleArrowHead",
            Self::TriangleWide => "TriangleWideArrowHead",
            Self::Barbed => "BarbedArrowHead",
            Self::Curved => "CurvedArrowHead",
            Self::Circle => "CircleArrowHead",
            Self::CircleSolid => "CircleSolidArrowHead",
            Self::Square => "SquareArrowHead",
            Self::SquareSolid => "SquareSolidArrowHead",
            Self::Bar => "BarArrowHead",
            Self::Other => "",
        }
    }

    /// Whether this end actually draws an arrowhead.
    pub fn draws(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Ruler guide on a spread. See [`Spread::guides`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RulerGuide {
    pub orientation: GuideOrientation,
    /// Coordinate (pt) along the perpendicular axis. For
    /// `Vertical`, this is the page-local x; for `Horizontal`,
    /// the page-local y.
    pub location: f32,
    /// Zero-based index into the spread's pages. IDML's
    /// `PageIndex` attribute is 1-based on per-spread guides but
    /// 0-based in real-world exports inspected — we read it
    /// verbatim and let downstream consumers clamp.
    pub page_index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuideOrientation {
    Vertical,
    Horizontal,
}

/// One `<Group>` page-item record. The renderer walks `members` to
/// know which frames sit inside this group, and `transparency` to
/// decide whether to bracket the range with a transparency-group
/// composite.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Group {
    pub self_id: Option<String>,
    /// Page items wrapped by this group, in document order.
    pub members: Vec<FrameRef>,
    pub transparency: GroupTransparency,
    /// `ItemTransform` attribute on the `<Group>` element. The
    /// per-frame `item_transform` already composes this in (see
    /// [`effective_item_transform`]); this field exists so renderers
    /// that need the un-composed group transform on its own can
    /// recover it without re-walking the spread.
    pub item_transform: Option<[f32; 6]>,
}

/// Reference to one of a `Spread`'s page-item vecs. Carries the
/// integer index back into the matching `Vec<...>` so the renderer
/// can look up the frame's data without a name search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameRef {
    TextFrame(usize),
    Rectangle(usize),
    Oval(usize),
    GraphicLine(usize),
    Polygon(usize),
    /// Index into `Spread::groups` — sub-groups are first-class
    /// members so the renderer can bracket each one independently.
    Group(usize),
}

/// Group-level transparency block parsed from `<TransparencySetting>` /
/// `<BlendingSetting>` / `<DropShadowSetting>` attached directly to a
/// `<Group>` element. Mirrors the per-frame fields of [`Rectangle`] /
/// [`TextFrame`] but applies to every member of the group at once.
/// Empty (`Default::default()`) when the group carried no
/// transparency block.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GroupTransparency {
    /// `<BlendingSetting BlendMode="…" />`. Same value space as
    /// `Rectangle::blend_mode` — `Normal | Multiply | Screen | …`.
    pub blend_mode: Option<String>,
    /// `<BlendingSetting Opacity="…" />`. Range `0.0..=100.0`.
    pub opacity: Option<f32>,
    /// `<DropShadowSetting>` attached to the group. The renderer
    /// emits the shadow against the group's flattened raster (so
    /// child fills don't double-stamp under it).
    pub drop_shadow: Option<DropShadowSetting>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub self_id: Option<String>,
    /// Page bounds in the page's *inner* coordinate system. Per the
    /// IDML spec, `GeometricBounds` describes the page rectangle in
    /// the page's own coords; the page's `ItemTransform` maps those
    /// coords into the parent spread. For older single-page-size
    /// layouts ItemTransform is identity, so the bounds are also
    /// the spread-space bounds — that's why our synthetic fixtures
    /// have always "just worked".
    pub bounds: Bounds,
    /// `AppliedMaster` reference — `MasterSpread/<id>` typically.
    /// Resolved to a `MasterSpread` by `paged_scene::Document`.
    pub applied_master: Option<String>,
    /// `ItemTransform` attribute on the `<Page>` element (CS5+).
    /// Maps the page's inner coordinate system into the spread's
    /// inner coordinate system. `None` ⇒ identity, in which case the
    /// page sits at the spread's origin.
    pub item_transform: Option<[f32; 6]>,
    /// `MasterPageTransform` attribute on the `<Page>` element
    /// (CS5+). Per spec §10.3.3, applied *after* the spread's
    /// ItemTransform but *before* each master page item's own
    /// ItemTransform — it positions the master overlay on this
    /// specific page (the "Master Page Overlay" feature). `None` ⇒
    /// identity.
    pub master_page_transform: Option<[f32; 6]>,
    /// `OverrideList` attribute on the `<Page>` element — space-
    /// separated list of master-spread item Self ids that this body
    /// page has overridden. The body page typically holds replacement
    /// frames for these items, so the original master items must NOT
    /// be stamped onto the page (the renderer would otherwise paint
    /// the placeholder under the body content).
    pub override_list: Vec<String>,
    /// `Name` attribute on the `<Page>` element — the user-visible
    /// page number/label as InDesign rendered it (already accounting
    /// for `<Section>` numbering style + start). Typically "1" / "2"
    /// for plain Arabic, but can be "iii", "A-3", etc. when sections
    /// override the style. The renderer substitutes this for ACE 18
    /// auto-page-number markers; if absent, it falls back to the
    /// 1-based body page index.
    pub name: Option<String>,
    /// `ShowMasterItems` attribute on the `<Page>` element. When
    /// `Some(false)` the page hides **all** master-spread overlay
    /// items (InDesign's "Hide Master Items" per-page toggle); the
    /// renderer skips stamping master frames/lines/text onto it.
    /// `None`/`Some(true)` ⇒ stamp as usual.
    pub show_master_items: Option<bool>,
}

/// `<MarginPreference>` — per-page margin box + column grid. All
/// distances are in points (IDML's native unit on the spread). The
/// margins inset the page rectangle; the column grid divides the
/// resulting content area. See [`Page::margins`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarginPreference {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
    /// `ColumnCount` — number of text columns the margin box is
    /// divided into. Defaults to 1.
    pub column_count: u32,
    /// `ColumnGutter` — gutter width (pt) between adjacent columns.
    pub column_gutter: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextFrame {
    pub self_id: Option<String>,
    /// Story reference (e.g. `u10`). Maps to a `Stories/Story_<id>.xml`
    /// entry via `DesignMap.stories`.
    pub parent_story: Option<String>,
    pub bounds: Bounds,
    /// 6-element affine transform `[a b c d tx ty]`. `None` if absent.
    pub item_transform: Option<[f32; 6]>,
    /// `FillColor` attribute, e.g. `Color/Red`. Resolved against
    /// `Graphic` in `paged-parse::graphic`.
    pub fill_color: Option<String>,
    /// See [`Rectangle::fill_tint`].
    pub fill_tint: Option<f32>,
    /// `StrokeColor` attribute.
    pub stroke_color: Option<String>,
    /// `StrokeWeight` attribute, in points. `None` → document default
    /// (typically 1 pt in InDesign).
    pub stroke_weight: Option<f32>,
    /// `StrokeType` reference; see [`Rectangle::stroke_type`].
    pub stroke_type: Option<String>,
    /// `GapColor` / `GapTint` for dashed-stroke gaps; see
    /// [`Rectangle::stroke_gap_color`].
    pub stroke_gap_color: Option<String>,
    pub stroke_gap_tint: Option<f32>,
    /// W1.1 — per-frame dash override; see [`Rectangle::stroke_dash`].
    pub stroke_dash: Vec<f32>,
    /// `<DropShadowSetting>` parsed from `<Properties><TransparencySetting>`.
    /// `None` when absent or `Mode="None"`.
    pub drop_shadow: Option<DropShadowSetting>,
    /// `<DropShadowSetting>` nested under `<StrokeTransparencySetting>`.
    /// Renderer emits this only when the frame's stroke is actually
    /// visible (non-`Swatch/None` colour AND `StrokeWeight > 0`).
    /// Splitting the storage from `drop_shadow` is required because a
    /// frame can carry both a fill-shadow (under `<TransparencySetting>`)
    /// and a stroke-shadow (under `<StrokeTransparencySetting>`).
    pub stroke_drop_shadow: Option<DropShadowSetting>,
    /// `NextTextFrame` attribute — the `Self` id of the frame that
    /// continues this story when its content overflows the current
    /// frame. `None` for end-of-chain or unthreaded frames.
    pub next_text_frame: Option<String>,
    /// `VerticalJustification` from `<TextFramePreference>`.
    pub vertical_justification: Option<VerticalJustification>,
    /// `FirstBaselineOffset` from `<TextFramePreference>`. Controls
    /// how the first line's baseline is placed inside the frame's
    /// inset box. `None` falls back to the renderer's heuristic
    /// (point_size × 0.8).
    pub first_baseline_offset: Option<FirstBaselineOffset>,
    /// `MinimumFirstBaselineOffset` from `<TextFramePreference>`,
    /// in pt. Used with `FirstBaselineOffset="LeadingOffset"` and
    /// `"FixedHeight"`.
    pub minimum_first_baseline_offset: Option<f32>,
    /// Frame insets in pt: (top, left, bottom, right). Comes from
    /// `<TextFramePreference>` `InsetSpacing` attribute (a
    /// space-separated list of four numbers, IDML order
    /// `top left bottom right`). `None` when absent.
    pub inset_spacing: Option<[f32; 4]>,
    /// `<TextFramePreference AutoSizingType="...">`. Drives whether
    /// the frame's bounds should grow at composition time to fit the
    /// story's content. `None`/`Off` ⇒ static frame (default).
    pub auto_sizing: Option<AutoSizingType>,
    /// `<TextFramePreference AutoSizingReferencePoint="...">`. Pins
    /// which corner / midpoint stays fixed when the frame grows.
    /// `None` ⇒ `TopLeftPoint` (the IDML default).
    pub auto_sizing_reference_point: Option<AutoSizingReferencePoint>,
    /// `<TextFramePreference MinimumWidthForAutoSizing="...">` in pt.
    /// Floor for width-growth. `None` ⇒ no floor.
    pub minimum_width_for_auto_sizing: Option<f32>,
    /// `<TextFramePreference MinimumHeightForAutoSizing="...">` in pt.
    /// Floor for height-growth (only consulted when
    /// `use_minimum_height_for_auto_sizing == Some(true)`).
    pub minimum_height_for_auto_sizing: Option<f32>,
    /// `<TextFramePreference UseMinimumHeightForAutoSizing="...">`.
    /// `Some(true)` ⇒ apply `minimum_height_for_auto_sizing`.
    pub use_minimum_height_for_auto_sizing: Option<bool>,
    /// `<TextFramePreference TextColumnCount="...">` — number of text
    /// columns the frame splits its inset box into. `None` ⇒ inherit
    /// the IDML default (1). W0.3 wires this as a mutable text-frame
    /// pref; the composer's per-column layout follows in a later wave.
    pub column_count: Option<u32>,
    /// `<TextFramePreference TextColumnGutter="...">` in pt — the gap
    /// between adjacent text columns. `None` ⇒ inherit (12pt default).
    pub column_gutter: Option<f32>,
    /// `<TextFramePreference TextColumnFixedWidth="...">` is the sibling
    /// "fixed-width" knob; `TextColumnCount` + balance cover the common
    /// authoring case. `<TextFramePreference VerticalBalanceColumns="...">`
    /// — `Some(true)` balances the last line across columns. `None` ⇒
    /// inherit (false).
    pub column_balance: Option<bool>,
    /// `AppliedObjectStyle` reference — `ObjectStyle/<id>`. Real-
    /// world IDMLs almost always rely on this for fill/stroke; the
    /// per-element FillColor attribute is rare. Resolved by
    /// `paged_scene::Document` against the document's StyleSheet.
    pub applied_object_style: Option<String>,
    /// `<TextWrapPreference>` parsed off the frame.
    pub text_wrap: Option<TextWrap>,
    /// `ItemLayer` reference. Renderer skips items whose layer is
    /// hidden or non-printable.
    pub item_layer: Option<String>,
    /// True when the frame is an *anchored object* — defined inside a
    /// CharacterStyleRange or carrying an `<AnchoredObjectSetting>`.
    /// The renderer's current pass treats anchored frames the same as
    /// page-level frames (free-floating). Real text-flow integration
    /// (inline reservation, custom-position offset from the anchor)
    /// is a queued follow-up; the flag exists so callers can skip
    /// these frames pending that work.
    pub is_anchored: bool,
    /// Item-level opacity from `<TransparencySetting>` /
    /// `<BlendingSetting Opacity="..." />`. Range `0.0..=100.0`. The
    /// renderer scales every paint's alpha by `opacity / 100` at
    /// emission time. `None` ⇒ fully opaque.
    pub opacity: Option<f32>,
    /// `<BlendingSetting BlendMode="..." />` (Normal | Multiply |
    /// Screen | Overlay | …). The renderer composites both the frame
    /// fill and the text glyphs using this mode.
    pub blend_mode: Option<String>,
    /// Path-point anchors with their Bezier control points, in the
    /// frame's inner coords. Real-world InDesign exports always
    /// serialise the path here even for plain rectangles (4 corner
    /// anchors). Empty when no `<PathGeometry>` was parsed (synthetic
    /// IDMLs that only carry `GeometricBounds`). The renderer treats
    /// non-rectangular paths as a clip mask for text layout so glyphs
    /// stay inside the actual triangle / pentagon / Bezier outline
    /// rather than the AABB.
    pub anchors: Vec<PathAnchor>,
    /// Subpath start offsets into `anchors`. Each entry is the index
    /// of the first anchor of one `<GeometryPathType>` contour. IDML
    /// `<PathGeometry>` may contain multiple `<GeometryPathType>`
    /// children (compound paths — e.g. a square with a hole); without
    /// these boundaries the renderer would join the contours into a
    /// single broken polyline. Empty for the common single-contour
    /// case (so existing callers can keep using the slice as-is).
    pub subpath_starts: Vec<usize>,
    /// Parallel to `subpath_starts`: `true` ⇒ the contour is open
    /// (omit the closing curve + Close). When `subpath_starts` is
    /// empty (single-contour shape) and this is also empty, the
    /// renderer treats the contour as closed (legacy behaviour).
    /// IDML's `<GeometryPathType PathOpen="true">` lifts to a `true`
    /// here so the renderer doesn't auto-close lassoed paths (P-15).
    pub subpath_open: Vec<bool>,
    /// See [`Rectangle::effects`] (Q-04).
    pub effects: Option<FrameEffects>,
    /// See [`Rectangle::gradient_fill_angle`].
    pub gradient_fill_angle: Option<f32>,
    /// See [`Rectangle::gradient_fill_length`].
    pub gradient_fill_length: Option<f32>,
    /// See [`Rectangle::gradient_stroke_angle`].
    pub gradient_stroke_angle: Option<f32>,
    /// See [`Rectangle::gradient_stroke_length`].
    pub gradient_stroke_length: Option<f32>,
    /// `AppliedTOCStyle` attribute — `TOCStyle/<id>` reference. Frames
    /// authored as Table-of-Contents hosts carry this; the renderer
    /// detects it at story emission and swaps the story's paragraphs
    /// for the resolver's output (see `Document::resolve_toc`). `None`
    /// for ordinary body frames.
    pub applied_toc_style: Option<String>,
    /// `OverprintFill="true"` on the IDML element. When true, the
    /// frame's fill physically mixes with whatever ink already sits
    /// behind it instead of knocking out the underlying separations
    /// (Adobe's print-preview overprint behaviour). The renderer
    /// approximates this on RGB with a per-pixel darken composite;
    /// per-channel CMYK overprint is deferred (see Phase 3 plan).
    pub overprint_fill: bool,
    /// `OverprintStroke="true"` analogue for the frame stroke.
    pub overprint_stroke: bool,
    /// SDK Phase 5 (v1 sweep) — `Nonprinting="true"`. Excludes
    /// this item from print/export passes; canvas still shows it.
    pub nonprinting: bool,
    /// W2.5 — element-level `Visible` (default `true`). The renderer
    /// skips items whose `Visible="false"`. See `CommonAttrs::visible`.
    pub visible: bool,
    /// W2.5 — element-level `Locked` (default `false`). The renderer
    /// paints locked items; the canvas hit-tester blocks their
    /// selection. See `CommonAttrs::locked`.
    pub locked: bool,
}

/// Parsed `<ClippingPathSettings>` for a placed image. Carries the
/// clip type, the boolean knobs the renderer honours (`InvertPath`,
/// `IncludeInsideEdges`), the `AppliedPathName` (for diagnostics when
/// the geometry lives in the image binary), and — when the IDML
/// serialised it (UserModifiedPath) — the resolved clip-path geometry
/// in the *image's pixel coordinate space* (the same space the
/// `<Image>`'s own `<PathGeometry>` uses, mapped to spread coords by
/// the image's `ItemTransform`).
///
/// `clip_anchors` / `clip_subpath_starts` / `clip_subpath_open` mirror
/// the frame-geometry convention exactly (one subpath-start per
/// `<GeometryPathType>`, `subpath_open` parallel) so the renderer can
/// reuse `polygon_path_from_anchors_with_open`. Holes (a compound clip
/// path — e.g. a star with a punched centre) survive via the same
/// `subpath_starts` boundaries as compound frame paths.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClippingPathSettings {
    /// The `ClippingType`. `None` when the attribute was absent (the
    /// IDML default — `ClippingType="None"`).
    pub clipping_type: Option<ClippingType>,
    /// `InvertPath` — `true` keeps the area *outside* the path (the
    /// path becomes a hole punched out of the frame).
    pub invert_path: bool,
    /// `IncludeInsideEdges` — `true` keeps interior contours as holes
    /// (a doughnut clip). The renderer already honours compound
    /// subpaths, so this flag selects the even-odd-style hole fill.
    pub include_inside_edges: bool,
    /// `AppliedPathName` — the named 8BIM path / alpha channel the clip
    /// references. Surfaced in the defer diagnostic so a user can see
    /// *which* embedded path we couldn't reach.
    pub applied_path_name: Option<String>,
    /// `Threshold` (DetectEdges luminance cutoff). Parsed for
    /// completeness; the renderer defers DetectEdges so it is unused
    /// today.
    pub threshold: Option<f32>,
    /// `Tolerance` (path-simplification tolerance). Parsed for
    /// completeness; unused until DetectEdges/AlphaChannel land.
    pub tolerance: Option<f32>,
    /// Resolved clip-path anchors in image-pixel space, captured from a
    /// `<PathGeometry>` nested under `<ClippingPathSettings>`. Empty
    /// when the geometry lives in the image binary (the defer case).
    pub clip_anchors: Vec<PathAnchor>,
    /// Subpath start offsets into `clip_anchors`; see
    /// [`Rectangle::subpath_starts`]. One entry per `<GeometryPathType>`.
    pub clip_subpath_starts: Vec<usize>,
    /// Parallel to `clip_subpath_starts`; see [`Rectangle::subpath_open`].
    pub clip_subpath_open: Vec<bool>,
}

impl ClippingPathSettings {
    /// True when this clip should actually crop the image: a known
    /// non-`None` type AND resolvable geometry present in the XML. The
    /// renderer uses this to decide between clipping and deferring.
    pub fn has_renderable_geometry(&self) -> bool {
        !self.clip_anchors.is_empty()
            && matches!(
                self.clipping_type,
                Some(ClippingType::UserModifiedPath) | None
            )
    }

    /// True when the IDML asks for a clip we can't satisfy from the XML
    /// (a Photoshop-path / alpha-channel / detect-edges type, or a
    /// named path with no inline geometry). Drives the defer
    /// diagnostic + render-unclipped fallback.
    pub fn is_deferred_clip(&self) -> bool {
        match self.clipping_type {
            None | Some(ClippingType::None) => false,
            Some(ClippingType::UserModifiedPath) => self.clip_anchors.is_empty(),
            Some(
                ClippingType::PhotoshopPath
                | ClippingType::AlphaChannel
                | ClippingType::DetectEdges
                | ClippingType::Other,
            ) => true,
        }
    }
}

/// Vector-only frame (no story). Mirrors `TextFrame` minus the
/// `parent_story` field; shares the same paint / stroke handling
/// downstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rectangle {
    pub self_id: Option<String>,
    pub bounds: Bounds,
    pub item_transform: Option<[f32; 6]>,
    pub fill_color: Option<String>,
    /// `FillTint` percentage (0..=100). `None` ⇒ use the swatch at
    /// full strength. The renderer scales the resolved RGB toward
    /// paper white by `(1 - tint/100)` when `Some`.
    pub fill_tint: Option<f32>,
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
    pub drop_shadow: Option<DropShadowSetting>,
    /// See [`TextFrame::stroke_drop_shadow`].
    pub stroke_drop_shadow: Option<DropShadowSetting>,
    /// `LinkResourceURI` from a nested `<Image>` (or its `<Link>`
    /// child). The pipeline routes this through
    /// `AssetResolver::resolve_image`. `None` means the rectangle
    /// is a plain colour swatch.
    pub image_link: Option<String>,
    /// True when the rectangle nests an `<Image>` / `<EPSImage>` /
    /// `<PDF>` / `<ImportedPage>` child regardless of whether a
    /// `LinkResourceURI` resolved. Distinguishes "plain colour
    /// swatch" (false) from "image frame whose link is unresolvable"
    /// (true) so the renderer can stamp a missing-image placeholder
    /// instead of falling back to the frame's raw fill.
    pub has_image_element: bool,
    /// True when the rectangle nests a `<PDF>` element carrying inline
    /// `<Contents>` CDATA but no `LinkResourceURI` — Envato templates
    /// embed placed PDFs this way. Until we have a PDF decoder, the
    /// frame should fall back to its intrinsic FillColor rather than
    /// the grey-X missing-image placeholder (Q-06).
    pub has_inline_pdf: bool,
    /// `ItemTransform` attribute on the nested `<Image>` element.
    /// Maps the image's natural-pixel coordinate space (origin at the
    /// top-left of the source pixmap, with 1px ≈ 1pt at 72 ppi) into
    /// the *frame's inner* coordinate system — the same space the
    /// rectangle's `<PathGeometry>` lives in. When present, the
    /// renderer composes
    ///    `frame_outer ∘ image_item_transform ∘ pixel_to_pt`
    /// to position the image; the rectangle's path then clips it.
    /// `None` falls back to the legacy "stretch image to frame
    /// bounds" behaviour for synthetic IDMLs that omit the inner
    /// transform.
    pub image_item_transform: Option<[f32; 6]>,
    /// Q-03: raw bytes from `<Image><Properties><Contents><![CDATA[...]]>`
    /// after base64 decode. Set when the IDML inlines the JPEG (or
    /// other) payload instead of (or in addition to) a `LinkResourceURI`.
    /// Real-world Envato newspaper / magazine packs do this for the
    /// majority of placed images. `None` for swatch-only Rectangles or
    /// link-only Image elements.
    pub image_bytes: Option<Vec<u8>>,
    /// W1.21: `<ClippingPathSettings>` parsed from the nested `<Image>`.
    /// `None` ⇒ no clip (the frame outline is the only crop). When
    /// present with renderable geometry the image is additionally
    /// clipped to the path (frame ∩ clip); otherwise a per-image
    /// diagnostic records the defer and the image renders unclipped.
    pub image_clip: Option<ClippingPathSettings>,
    /// `AppliedObjectStyle` reference; see `TextFrame`.
    pub applied_object_style: Option<String>,
    /// `<TextWrapPreference>` parsed off the rectangle.
    pub text_wrap: Option<TextWrap>,
    /// `<FrameFittingOption>` child element (or its inherited
    /// equivalent on the applied object style). Drives where the
    /// placed image's pixel grid lands inside the frame and how far
    /// it can overflow / underflow via `LeftCrop` / `TopCrop` etc.
    pub frame_fitting: Option<FrameFittingOption>,
    /// `StrokeType` reference (e.g. `StrokeStyle/$ID/Solid`,
    /// `StrokeStyle/$ID/Dashed`, `StrokeStyle/$ID/Dotted`). The
    /// renderer maps the standard built-in names to a dash pattern;
    /// user-defined custom `<StrokeStyle>` definitions fall back to
    /// solid until full parser support lands.
    pub stroke_type: Option<String>,
    /// `StrokeAlignment` — `CenterAlignment` (default, stroke
    /// straddles the path), `InsideAlignment` (stroke lies inside
    /// the geometry), or `OutsideAlignment` (outside). The renderer
    /// inset/outsets the rectangle by half the stroke weight to
    /// approximate Inside/Outside without clipping.
    pub stroke_alignment: Option<String>,
    /// `EndCap` — `ButtEndCap` (default), `RoundEndCap`, or
    /// `ProjectingEndCap`. Maps to tiny-skia's `LineCap`. Only
    /// visible on open paths or dashed/dotted strokes.
    pub end_cap: Option<String>,
    /// `EndJoin` — `MiterEndJoin` (default), `RoundEndJoin`, or
    /// `BevelEndJoin`. Controls how stroke segments meet at
    /// corners (e.g. the four corners of a rectangle).
    pub end_join: Option<String>,
    /// `MiterLimit` — when joins are mitered, the maximum miter
    /// length expressed as a multiple of the stroke width before
    /// the join falls back to bevel. InDesign defaults to 4.0.
    pub miter_limit: Option<f32>,
    /// `GapColor` reference — the colour painted between dashes of a
    /// dashed / striped stroke. `Swatch/None` (the default) leaves the
    /// gaps transparent. The renderer doesn't paint gap colour yet;
    /// the field is wired for authoring + round-trip.
    pub stroke_gap_color: Option<String>,
    /// `GapTint` percentage (0..=100) for `stroke_gap_color`.
    pub stroke_gap_tint: Option<f32>,
    /// W1.1 — per-frame `StrokeDashAndGap` override: the alternating
    /// on/off dash lengths in pt parsed off this page item's
    /// attribute. Empty ⇒ no per-frame override (the stroke uses its
    /// `StrokeType`'s `StrokeStyleDef` pattern / built-in name). When
    /// present, the renderer honours it in PREFERENCE to the
    /// stroke-style pattern (the per-instance-wins precedent the
    /// `stroke_gap_color` field already follows). Lives on every
    /// stroked page-item kind via `CommonAttrs`.
    pub stroke_dash: Vec<f32>,
    /// `ItemLayer` reference (`<self_id>` of a `<Layer>` in
    /// designmap.xml). The renderer skips this rectangle when its
    /// layer is hidden or non-printable.
    pub item_layer: Option<String>,
    /// `CornerRadius` in pt; pairs with `corner_option`. `None`
    /// inherits from the applied object style. Legacy single-corner
    /// fallback when none of the per-corner attrs below are set.
    pub corner_radius: Option<f32>,
    /// `CornerOption` (`None`, `Rounded`, etc). The renderer emits a
    /// rounded-rect path for `Rounded` (and the decorative variants
    /// fall back to `Rounded` for now). Legacy single-corner fallback.
    pub corner_option: Option<String>,
    /// Q-16: per-corner `(option, radius)` overrides. When *any* corner
    /// carries an explicit value, the renderer builds a 4-corner path
    /// with each corner using its own radius (defaulting to the legacy
    /// `corner_radius` for corners left unspecified). Order:
    /// `[top_left, top_right, bottom_right, bottom_left]` — matches the
    /// stroke walk in `rounded_rect_path`. Each entry is `(option,
    /// radius)`; both default to None.
    pub corners: [CornerSpec; 4],
    /// Anchored object marker; see `TextFrame::is_anchored`.
    pub is_anchored: bool,
    /// Item-level opacity from `<TransparencySetting>` /
    /// `<BlendingSetting Opacity="..." />`. Range `0.0..=100.0`. The
    /// renderer scales every paint's alpha channel by `opacity / 100`
    /// at emission time. `None` ⇒ fully opaque.
    pub opacity: Option<f32>,
    /// `<BlendingSetting BlendMode="..." />` (Normal | Multiply |
    /// Screen | Overlay | …). Currently parsed for completeness;
    /// non-Normal modes are not yet honoured by the rasterizer.
    pub blend_mode: Option<String>,
    /// Visual effects beyond `DropShadow`. All currently parser-only:
    /// the rasterizer-side blur / composite pipeline they need is a
    /// future batch (T2.7 in the roadmap). When unset (`None`) the
    /// renderer treats the field as "no effect"; when set it's
    /// surfaced for downstream tooling but visibly emits nothing.
    pub effects: Option<FrameEffects>,
    /// `GradientFillAngle` in degrees. IDML serialises a gradient
    /// fill direction as an angle around the frame's centre — 0°
    /// is horizontal (left → right), 90° is vertical (top → bottom).
    /// `None` ⇒ keep the renderer's default top-to-bottom unit-rect
    /// endpoints. Combined with `gradient_fill_length` the renderer
    /// places the gradient line through the frame's center.
    pub gradient_fill_angle: Option<f32>,
    /// `GradientFillLength` in points — the page-space length of the
    /// gradient line through the frame's centre. `None` falls back to
    /// the bbox diagonal (covers the rect end-to-end). Values smaller
    /// than the diagonal compress the gradient (extreme stops paint
    /// flat regions outside the line); values larger expand it.
    pub gradient_fill_length: Option<f32>,
    /// `GradientStrokeAngle` in degrees — same convention as
    /// `gradient_fill_angle` but applied to the stroke gradient.
    pub gradient_stroke_angle: Option<f32>,
    /// `GradientStrokeLength` in points — same role as
    /// `gradient_fill_length` for the stroke gradient.
    pub gradient_stroke_length: Option<f32>,
    /// `<TextPath>` children attached to this rectangle. See
    /// [`Polygon::text_paths`].
    pub text_paths: Vec<TextPath>,
    /// `OverprintFill="true"`. See [`TextFrame::overprint_fill`].
    pub overprint_fill: bool,
    /// `OverprintStroke="true"`. See [`TextFrame::overprint_stroke`].
    pub overprint_stroke: bool,
    /// SDK Phase 5 (v1 sweep) — `Nonprinting`. See
    /// [`TextFrame::nonprinting`].
    pub nonprinting: bool,
    /// W2.5 — element-level `Visible` (default `true`). See
    /// [`TextFrame::visible`].
    pub visible: bool,
    /// W2.5 — element-level `Locked` (default `false`). See
    /// [`TextFrame::locked`].
    pub locked: bool,
    /// Q-11: Bezier path anchors captured from `<PathGeometry>` when
    /// the rectangle's outline is non-rectangular (torn-paper /
    /// asymmetric / multi-anchor stylised shapes that Envato saves as
    /// `<Rectangle>` rather than `<Polygon>`). When this exceeds the
    /// 4-anchor AABB case, the renderer routes through `Geometry::Polygon`.
    pub anchors: Vec<PathAnchor>,
    /// Subpath start offsets into `anchors`; see [`Polygon::subpath_starts`].
    pub subpath_starts: Vec<usize>,
    /// Per-contour open/closed flags; see [`Polygon::subpath_open`].
    pub subpath_open: Vec<bool>,
}

/// Mirror of IDML's optional `InnerShadow`, `OuterGlow`, `InnerGlow`,
/// `Bevel`, `Satin`, and `FeatherSetting` blocks on a page item. Each
/// inner field is `Some(EffectParams)` when the IDML's `Applied="true"`
/// attribute is present; `None` (the default) when the effect is
/// absent or explicitly disabled. The renderer's compose-layer
/// equivalents (`paged_compose::InnerShadow`, etc.) consume these
/// parameters directly.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FrameEffects {
    pub inner_shadow: Option<InnerShadowParams>,
    pub outer_glow: Option<OuterGlowParams>,
    pub inner_glow: Option<InnerGlowParams>,
    pub bevel: Option<BevelEmbossParams>,
    pub satin: Option<SatinParams>,
    pub feather: Option<FeatherParams>,
    /// Directional feather — per-edge widths + rotation. Carries
    /// the four IDML edge attributes (`LeftWidth`, `RightWidth`,
    /// `TopWidth`, `BottomWidth`) plus optional `Angle` /
    /// `NoiseAmount` / `ChokeAmount` / `CornerType`.
    pub directional_feather: Option<DirectionalFeatherParams>,
    /// Gradient feather — linear/radial alpha falloff defined by a
    /// list of `<GradientStop>` children.
    pub gradient_feather: Option<GradientFeatherParams>,
}

/// `<InnerShadowSetting>` parameters. Either `(XOffset, YOffset)` or
/// `(Angle, Distance)` may be set; the renderer uses XOffset/YOffset
/// directly when present, otherwise computes them from the polar
/// pair: `XOffset = Distance * cos(Angle)`, `YOffset = -Distance * sin(Angle)`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct InnerShadowParams {
    pub x_offset: Option<f32>,
    pub y_offset: Option<f32>,
    pub size: Option<f32>,
    pub opacity_pct: Option<f32>,
    pub effect_color: Option<String>,
    pub angle_deg: Option<f32>,
    pub distance: Option<f32>,
    pub choke_pct: Option<f32>,
    pub blend_mode: Option<String>,
    pub noise_pct: Option<f32>,
}

/// `<OuterGlowSetting>` parameters.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct OuterGlowParams {
    pub size: Option<f32>,
    pub opacity_pct: Option<f32>,
    pub effect_color: Option<String>,
    pub spread_pct: Option<f32>,
    pub blend_mode: Option<String>,
    pub noise_pct: Option<f32>,
}

/// `<InnerGlowSetting>` parameters. `source` selects center-out vs
/// edge-in glow (IDML's `Source="EdgeGlow"`/`"CenterGlow"`); the
/// renderer assumes edge-in when absent.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct InnerGlowParams {
    pub size: Option<f32>,
    pub opacity_pct: Option<f32>,
    pub effect_color: Option<String>,
    pub choke_pct: Option<f32>,
    pub blend_mode: Option<String>,
    pub source: Option<String>,
    pub noise_pct: Option<f32>,
}

/// `<BevelAndEmbossSetting>` parameters. `style` (Inner/Outer/Emboss/
/// Pillow), `direction` (Up/Down), `technique` (Smooth/Chisel*) and
/// `soften` steer the rasterizer's height-field shading (W1.4): the
/// CPU path reshapes the height field per style, flips the light's
/// elevation sign for `Down`, narrows the smoothing band for the
/// chisel techniques, and blurs the shaded layer by `soften`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct BevelEmbossParams {
    pub depth_pct: Option<f32>,
    pub size: Option<f32>,
    pub angle_deg: Option<f32>,
    pub altitude_deg: Option<f32>,
    pub highlight_color: Option<String>,
    pub shadow_color: Option<String>,
    pub highlight_opacity_pct: Option<f32>,
    pub shadow_opacity_pct: Option<f32>,
    pub style: Option<String>,
    pub direction: Option<String>,
    pub technique: Option<String>,
    pub soften: Option<f32>,
}

/// `<SatinSetting>` parameters. `Distance` + `Angle` set the wave
/// direction and spacing.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SatinParams {
    pub size: Option<f32>,
    pub angle_deg: Option<f32>,
    pub distance: Option<f32>,
    pub effect_color: Option<String>,
    pub opacity_pct: Option<f32>,
    pub blend_mode: Option<String>,
    pub invert: Option<bool>,
}

/// `<FeatherSetting>` parameters. `corner_type` is `Sharp`/`Rounded`/
/// `Diffusion`; the rasterizer only branches on Diffusion (which adds
/// noise) at the moment.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FeatherParams {
    pub width: Option<f32>,
    pub corner_type: Option<String>,
    pub noise_pct: Option<f32>,
    pub choke_pct: Option<f32>,
}

/// `<DirectionalFeatherSetting>` parameters. Each edge carries an
/// independent feather width in pt; `angle_deg` rotates the per-edge
/// directions. The CPU rasterizer honours all four widths
/// independently and rotates each pixel into the rect's intrinsic
/// frame by `angle_deg` before computing per-side fades (W1.4); the
/// Vello preview still approximates with a uniform max-width feather.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DirectionalFeatherParams {
    pub left_width: Option<f32>,
    pub right_width: Option<f32>,
    pub top_width: Option<f32>,
    pub bottom_width: Option<f32>,
    pub angle_deg: Option<f32>,
    pub noise_pct: Option<f32>,
    pub choke_pct: Option<f32>,
    pub corner_type: Option<String>,
}

/// `<GradientFeatherSetting>` parameters. The gradient direction is
/// either an explicit `(start_point, end_point)` pair or
/// `(angle_deg, …)` polar form (start/end are derived). `stops`
/// captures the `<GradientStop>` children; each stop's alpha
/// (extracted from the `<Color>` referenced by `StopColor`) is the
/// feather opacity at that location.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GradientFeatherParams {
    /// `Type` attribute: `"Linear"` or `"Radial"`. Defaults to
    /// `"Linear"` when absent or unrecognised at the renderer.
    pub gradient_type: Option<String>,
    pub start_point: Option<(f32, f32)>,
    pub end_point: Option<(f32, f32)>,
    pub angle_deg: Option<f32>,
    pub stops: Vec<GradientFeatherStop>,
}

/// One `<GradientStop>` child of a `<GradientFeatherSetting>`. Each
/// stop's `StopColor` references a `<Color>` swatch; the renderer
/// resolves the swatch's alpha later. Until then `alpha_pct` is
/// initialised to 100 and the stop color id is preserved for
/// downstream resolution.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GradientFeatherStop {
    /// Color id referenced by `StopColor`. Resolved by the renderer
    /// to extract the alpha component. Black + alpha = "opaque
    /// feather" in InDesign's UI.
    pub stop_color: Option<String>,
    pub location_pct: f32,
    /// 0..100 — opacity at this stop. Defaults to 100 (fully
    /// opaque) when not surfaced by the parser.
    pub alpha_pct: f32,
    /// Transition midpoint (0..100) between this stop and the
    /// next; mirrors IDML's `GradientStopMidpoint` if present.
    pub midpoint_pct: f32,
}

/// Mirrors IDML's `<FrameFittingOption>` — an optional element nested
/// inside a Rectangle (or referenced via `AppliedObjectStyle`) that
/// describes how a placed image fits the frame.
///
/// Crop values are signed point offsets *from the corresponding frame
/// edge inward*. Negative crops grow the image outside the frame
/// (typical for `Proportionally` / `FillProportionally` fits where the
/// image is scaled to cover the frame and the overflow is meant to be
/// clipped). InDesign bakes these out when the user picks a fit, so
/// the renderer just trusts whatever number lands here.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FrameFittingOption {
    pub left_crop: Option<f32>,
    pub top_crop: Option<f32>,
    pub right_crop: Option<f32>,
    pub bottom_crop: Option<f32>,
    /// `FittingOnEmptyFrame` value: `None | Proportionally |
    /// FillProportionally | FitContent | FitContentToFrame |
    /// ContentAwareFit`. We don't currently branch on this — the
    /// crops alone reproduce InDesign's resolved placement for the
    /// common cases. The string is kept for future fidelity work.
    pub fitting_on_empty_frame: Option<String>,
    /// `FittingAlignment` — the reference point the content is pinned
    /// to when the fit is reapplied (`TopLeftPoint`, `CenterPoint`,
    /// …). `None` ⇒ inherit (`CenterPoint`). W0.3 surfaces it as
    /// `FrameFittingReferencePoint`; the crops drive placement today.
    pub reference_point: Option<String>,
    /// `AutoFit="true"` — when set, InDesign re-runs the fit whenever
    /// the frame is resized. `None` ⇒ false. Surfaced as
    /// `FrameAutoFit`; informational until the live-fit pass lands.
    pub auto_fit: Option<bool>,
}

/// Axis-aligned ellipse — `<Oval>` in IDML. Same fill/stroke story as
/// Rectangle; geometry is the ellipse inscribed in `GeometricBounds`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Oval {
    pub self_id: Option<String>,
    pub bounds: Bounds,
    pub item_transform: Option<[f32; 6]>,
    pub fill_color: Option<String>,
    /// See [`Rectangle::fill_tint`].
    pub fill_tint: Option<f32>,
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
    /// `StrokeType` reference; see [`Rectangle::stroke_type`].
    pub stroke_type: Option<String>,
    /// `StrokeAlignment` (`Center`/`Inside`/`Outside`); see
    /// [`Rectangle::stroke_alignment`]. W1.5 honours inside/outside on
    /// the elliptical outline by offsetting it ±weight/2.
    pub stroke_alignment: Option<String>,
    /// `GapColor` / `GapTint`; see [`Rectangle::stroke_gap_color`].
    pub stroke_gap_color: Option<String>,
    pub stroke_gap_tint: Option<f32>,
    /// W1.1 — per-frame dash override; see [`Rectangle::stroke_dash`].
    pub stroke_dash: Vec<f32>,
    pub drop_shadow: Option<DropShadowSetting>,
    /// See [`TextFrame::stroke_drop_shadow`].
    pub stroke_drop_shadow: Option<DropShadowSetting>,
    /// `AppliedObjectStyle` reference; see `TextFrame`.
    pub applied_object_style: Option<String>,
    /// `<TextWrapPreference>` parsed off the oval.
    pub text_wrap: Option<TextWrap>,
    pub item_layer: Option<String>,
    /// See [`Rectangle::effects`] (Q-04).
    pub effects: Option<FrameEffects>,
    /// See [`Rectangle::gradient_fill_angle`].
    pub gradient_fill_angle: Option<f32>,
    /// See [`Rectangle::gradient_fill_length`].
    pub gradient_fill_length: Option<f32>,
    /// See [`Rectangle::gradient_stroke_angle`].
    pub gradient_stroke_angle: Option<f32>,
    /// See [`Rectangle::gradient_stroke_length`].
    pub gradient_stroke_length: Option<f32>,
    /// See [`Rectangle::opacity`].
    pub opacity: Option<f32>,
    /// See [`Rectangle::blend_mode`].
    pub blend_mode: Option<String>,
    /// `LinkResourceURI` of the placed image (or `<Image href>`), if
    /// the oval nests one. Mirrors [`Rectangle::image_link`] (P-16).
    pub image_link: Option<String>,
    /// True when the oval nests an `<Image>` / `<EPSImage>` / `<PDF>` /
    /// `<ImportedPage>` element, regardless of resolvability. Mirrors
    /// [`Rectangle::has_image_element`] (P-16).
    pub has_image_element: bool,
    /// Inline-`<PDF>`-without-link marker (Q-06). Mirrors
    /// [`Rectangle::has_inline_pdf`].
    pub has_inline_pdf: bool,
    /// `ItemTransform` from the nested `<Image>`. Mirrors
    /// [`Rectangle::image_item_transform`] (P-16).
    pub image_item_transform: Option<[f32; 6]>,
    /// Q-03: see [`Rectangle::image_bytes`].
    pub image_bytes: Option<Vec<u8>>,
    /// W1.21: see [`Rectangle::image_clip`].
    pub image_clip: Option<ClippingPathSettings>,
    /// `OverprintFill="true"`. See [`TextFrame::overprint_fill`].
    pub overprint_fill: bool,
    /// `OverprintStroke="true"`. See [`TextFrame::overprint_stroke`].
    pub overprint_stroke: bool,
    /// SDK Phase 5 (v1 sweep) — `Nonprinting`. See
    /// [`TextFrame::nonprinting`].
    pub nonprinting: bool,
    /// W2.5 — element-level `Visible` (default `true`). See
    /// [`TextFrame::visible`].
    pub visible: bool,
    /// W2.5 — element-level `Locked` (default `false`). See
    /// [`TextFrame::locked`].
    pub locked: bool,
}

/// Straight line — `<GraphicLine>` in IDML. The endpoints are the
/// `GeometricBounds` rect's top-left and bottom-right corners (IDML
/// stores the endpoints implicitly via the bounds).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphicLine {
    pub self_id: Option<String>,
    pub bounds: Bounds,
    pub item_transform: Option<[f32; 6]>,
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
    /// `StrokeType` reference; see [`Rectangle::stroke_type`].
    pub stroke_type: Option<String>,
    /// `EndJoin`; see [`Rectangle::end_join`]. Multi-segment / curved
    /// lines miter their interior joins.
    pub end_join: Option<String>,
    /// `MiterLimit`; see [`Rectangle::miter_limit`]. Punch-list: wired
    /// for all path kinds, not just rectangles.
    pub miter_limit: Option<f32>,
    /// `GapColor` / `GapTint`; see [`Rectangle::stroke_gap_color`].
    pub stroke_gap_color: Option<String>,
    pub stroke_gap_tint: Option<f32>,
    /// W1.1 — per-frame dash override; see [`Rectangle::stroke_dash`].
    pub stroke_dash: Vec<f32>,
    /// `AppliedObjectStyle` reference; see `TextFrame`.
    pub applied_object_style: Option<String>,
    /// `<TextWrapPreference>` parsed off the line.
    pub text_wrap: Option<TextWrap>,
    pub item_layer: Option<String>,
    /// Path-point anchors for lines that carry a curved or
    /// multi-segment `<PathGeometry>` (so a `<TextPath>` child can
    /// flow text along the actual stroke). Empty for synthetic
    /// `GeometricBounds`-only lines, which the renderer continues to
    /// rasterise as the corner-to-corner diagonal.
    pub anchors: Vec<PathAnchor>,
    /// Subpath start offsets into `anchors`. See
    /// [`TextFrame::subpath_starts`]. Lines almost always carry a
    /// single contour, but the field is parsed uniformly for symmetry
    /// with the other path-bearing shapes.
    pub subpath_starts: Vec<usize>,
    /// Parallel to `subpath_starts`. See [`TextFrame::subpath_open`].
    pub subpath_open: Vec<bool>,
    /// `<TextPath>` children attached to this line. See
    /// [`Polygon::text_paths`].
    pub text_paths: Vec<TextPath>,
    /// See [`Rectangle::effects`] (Q-04). Lines carry no fill so
    /// fill-only effects (GradientFeather, InnerShadow, etc.) just
    /// log; stroke-side effects map naturally.
    pub effects: Option<FrameEffects>,
    /// `OverprintStroke="true"`. See [`TextFrame::overprint_stroke`].
    /// Lines carry no fill, so only the stroke flag is meaningful.
    pub overprint_stroke: bool,
    /// SDK Phase 5 (v1 sweep) — `Nonprinting`. See
    /// [`TextFrame::nonprinting`].
    pub nonprinting: bool,
    /// W2.5 — element-level `Visible` (default `true`). See
    /// [`TextFrame::visible`].
    pub visible: bool,
    /// W2.5 — element-level `Locked` (default `false`). See
    /// [`TextFrame::locked`].
    pub locked: bool,
    /// `LeftLineEnd` — arrowhead at the line's start anchor. Defaults
    /// to `None`.
    pub start_arrow: ArrowheadType,
    /// `RightLineEnd` — arrowhead at the line's end anchor.
    pub end_arrow: ArrowheadType,
    /// `LeftArrowHeadScale` / `RightArrowHeadScale` (percent, default
    /// 100). Scales the arrowhead size, which is otherwise derived from
    /// the stroke weight.
    pub start_arrow_scale: f32,
    pub end_arrow_scale: f32,
}

/// One point on an IDML `<PathGeometry>` path. `anchor` is the
/// on-curve point; `left` / `right` are the incoming / outgoing
/// Bezier control points respectively. Coordinates are in the
/// owning page item's *inner* coordinate system.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PathAnchor {
    pub anchor: (f32, f32),
    pub left: (f32, f32),
    pub right: (f32, f32),
}

/// `<TextWrapPreference>` settings on a page item. Parsed onto
/// every shape (TextFrame / Rectangle / Oval / Polygon /
/// GraphicLine). The renderer uses the AABB plus offsets as a
/// per-page wrap exclusion when laying out other text frames.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TextWrap {
    pub mode: TextWrapMode,
    /// IDML order: `[top, left, bottom, right]`, in pt. Inflates the
    /// wrap rectangle outwards so text keeps its distance.
    pub offsets: [f32; 4],
    /// `Inverse`/`Inverted` flag — `true` flows text *inside* the wrap
    /// shape rather than around it. `None` ⇒ false (the IDML default).
    /// Kept `Copy`-compatible (a plain `Option<bool>`) so `TextWrap`
    /// stays the small value type the renderer copies by value.
    pub invert: Option<bool>,
    /// W2.5 — `<ContourOption ContourType="...">`, the contour source
    /// for `ContourTextWrap` mode. Modelled as a small `Copy` enum (not
    /// a `String`) so `TextWrap` stays `Copy` — the W0.3 note's reason
    /// to defer string-valued contour knobs is sidestepped. `None` ⇒ no
    /// `<ContourOption>` (InDesign defaults to the graphic's frame /
    /// clip path). Only meaningful when `mode == ContourTextWrap`.
    pub contour_type: Option<ContourOptionType>,
    /// W2.5 — `<ContourOption IncludeInsideEdges="true|false">`. `true`
    /// lets text flow into interior holes of a contour. `None` ⇒ the
    /// IDML default (`false`). `Option<bool>` keeps `TextWrap` `Copy`.
    pub include_inside_edges: Option<bool>,
}

impl TextWrap {
    pub const NONE: TextWrap = TextWrap {
        mode: TextWrapMode::None,
        offsets: [0.0; 4],
        invert: None,
        contour_type: None,
        include_inside_edges: None,
    };
}

/// W2.5 — `<ContourOption ContourType="...">` source for a contour
/// text-wrap. A small `Copy` enum so `TextWrap` stays a by-value type.
/// Unknown / unsupported types map to `Other` (the renderer treats the
/// whole wrap as a bounding-box exclusion regardless, so the exact
/// contour source is authoring metadata for v1 — see the renderer's
/// `wrap_exclusion` note).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContourOptionType {
    /// `SameAsClipping` — use the placed graphic's clip path.
    SameAsClipping,
    /// `GraphicFrame` — use the holding frame's geometry.
    GraphicFrame,
    /// `DetectEdges` — auto-detect the graphic's edges.
    DetectEdges,
    /// `AlphaChannel` — use the image's alpha channel.
    AlphaChannel,
    /// `PhotoshopPath` — use a named Photoshop path.
    PhotoshopPath,
    /// Any other / unrecognised value.
    Other,
}

impl ContourOptionType {
    /// Map an IDML `ContourType` attribute string.
    pub fn from_idml(s: &str) -> Self {
        match s {
            "SameAsClipping" => Self::SameAsClipping,
            "GraphicFrame" => Self::GraphicFrame,
            "DetectEdges" => Self::DetectEdges,
            "AlphaChannel" => Self::AlphaChannel,
            "PhotoshopPath" => Self::PhotoshopPath,
            _ => Self::Other,
        }
    }
    /// Render back to the canonical IDML attribute string. `Other`
    /// falls back to `"SameAsClipping"` (the InDesign default contour
    /// source) since the original string was lost on `from_idml`.
    pub fn as_idml(self) -> &'static str {
        match self {
            Self::SameAsClipping => "SameAsClipping",
            Self::GraphicFrame => "GraphicFrame",
            Self::DetectEdges => "DetectEdges",
            Self::AlphaChannel => "AlphaChannel",
            Self::PhotoshopPath => "PhotoshopPath",
            Self::Other => "SameAsClipping",
        }
    }
}

/// `TextWrapMode` enum value. Values not in the IDML spec collapse
/// to `Other` so the cascade still records *something* the renderer
/// can decide to ignore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextWrapMode {
    None,
    BoundingBoxTextWrap,
    ContourTextWrap,
    JumpObjectTextWrap,
    NextColumnTextWrap,
    Other,
}

impl TextWrapMode {
    /// Map IDML attribute string. InDesign's exporter uses both the
    /// `<Mode>TextWrap` long form and the bare-stem short form
    /// depending on the property; we match either so wrap rectangles
    /// from real-world exports route correctly.
    pub fn from_idml(s: &str) -> Self {
        match s {
            "None" => Self::None,
            "BoundingBoxTextWrap" | "BoundingBox" => Self::BoundingBoxTextWrap,
            "ContourTextWrap" | "Contour" => Self::ContourTextWrap,
            "JumpObjectTextWrap" | "JumpObject" => Self::JumpObjectTextWrap,
            "NextColumnTextWrap" | "NextColumn" => Self::NextColumnTextWrap,
            _ => Self::Other,
        }
    }
    /// Render back to the canonical IDML attribute string. Used by
    /// the editor's `paged.inspect` round-trip + the mutate-layer
    /// inverse, which need a stable string form. Falls back to
    /// `"None"` for `Other` since the underlying string was lost on
    /// `from_idml` (a future fidelity polish carries the raw string).
    pub fn as_idml(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::BoundingBoxTextWrap => "BoundingBoxTextWrap",
            Self::ContourTextWrap => "ContourTextWrap",
            Self::JumpObjectTextWrap => "JumpObjectTextWrap",
            Self::NextColumnTextWrap => "NextColumnTextWrap",
            Self::Other => "None",
        }
    }
    pub fn is_active(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// `<TextPath>` child element. Attaches a story to a host shape's
/// path so the text flows along the curve rather than filling a
/// rectangular column. Attribute coverage is intentionally minimal —
/// the renderer needs only the story reference today; richer
/// alignment / spacing / effect attributes can land later without
/// a struct breaking change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextPath {
    pub self_id: Option<String>,
    /// Story reference (e.g. `u3ae`). Same shape as
    /// `TextFrame::parent_story`.
    pub parent_story: String,
    /// `PathAlignment` — a non-standard knob the early foundation read;
    /// kept for back-compat. Real InDesign IDML uses `PathTypeAlignment`
    /// (below) for the glyph's vertical seat on the path. The current
    /// renderer ignores this field; prefer `path_type_alignment`.
    pub path_alignment: Option<String>,
    /// `PathTypeAlignment` — IDML's vertical alignment of each glyph to
    /// the path: `BaselinePathType` (default — glyph baseline rides the
    /// path), `CenterPathType` (glyph em-box centre on the path),
    /// `AscenderPathType` (glyph top on the path), `DescenderPathType`
    /// (glyph bottom on the path). The renderer honours Baseline /
    /// Center; Ascender / Descender land too but are exercised less.
    pub path_type_alignment: Option<String>,
    /// `PathEffect` — `RainbowPathEffect` (default; glyph rotated to the
    /// local tangent), `SkewPathEffect`, `Path3DRibbonEffect`,
    /// `StairStepPathEffect`, `GravityPathEffect`. The renderer ships
    /// Rainbow; the other four parse but render as Rainbow today (see
    /// `emit_text_path_into`).
    pub path_effect: Option<String>,
    /// `FlipPathEffect` — `Flipped` flips the text to the path's
    /// other side. `NotFlipped` is the IDML default.
    pub flip_path_effect: Option<String>,
    /// `StartBracket` / `EndBracket` — IDML's per-path range over
    /// which the text flows, in path-local arc-length units. Captured
    /// for future fidelity; the current renderer flows from t=0.
    pub start_bracket: Option<f32>,
    pub end_bracket: Option<f32>,
}

/// `<Polygon>`. Same paint/stroke story as `Rectangle`. `anchors`
/// retains the parsed `<PathPointType>` data so the renderer can
/// rasterise the actual curved path; `bounds` is still the AABB so
/// page-routing and emit paths that haven't been switched over keep
/// working.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Polygon {
    pub self_id: Option<String>,
    pub bounds: Bounds,
    pub item_transform: Option<[f32; 6]>,
    pub fill_color: Option<String>,
    /// See [`Rectangle::fill_tint`].
    pub fill_tint: Option<f32>,
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
    /// `StrokeType` reference; see [`Rectangle::stroke_type`].
    pub stroke_type: Option<String>,
    /// `StrokeAlignment` (`Center`/`Inside`/`Outside`); see
    /// [`Rectangle::stroke_alignment`]. W1.5 honours inside/outside on
    /// the closed polygon outline by offsetting it ±weight/2.
    pub stroke_alignment: Option<String>,
    /// `EndJoin` — `MiterEndJoin` (default) / `RoundEndJoin` /
    /// `BevelEndJoin`; see [`Rectangle::end_join`]. Closed polygons miter
    /// their corners like a rect's four corners.
    pub end_join: Option<String>,
    /// `MiterLimit`; see [`Rectangle::miter_limit`]. A polygon corner
    /// sharper than this limit bevels rather than spiking. Punch-list:
    /// wired for all closed path kinds, not just rectangles.
    pub miter_limit: Option<f32>,
    /// `GapColor` / `GapTint`; see [`Rectangle::stroke_gap_color`].
    pub stroke_gap_color: Option<String>,
    pub stroke_gap_tint: Option<f32>,
    /// W1.1 — per-frame dash override; see [`Rectangle::stroke_dash`].
    pub stroke_dash: Vec<f32>,
    pub applied_object_style: Option<String>,
    /// Path-point anchors with their Bezier control points, in the
    /// polygon's inner coords. Empty for synthetic IDMLs that
    /// declared the polygon via `GeometricBounds` only.
    pub anchors: Vec<PathAnchor>,
    /// Subpath start offsets into `anchors` — one per
    /// `<GeometryPathType>` contour. See
    /// [`TextFrame::subpath_starts`]. Compound polygons (e.g. a
    /// square with a hole) ship multiple contours; rendering them as
    /// a single connected polyline would silently merge the inner
    /// loop into the outer outline.
    pub subpath_starts: Vec<usize>,
    /// Parallel to `subpath_starts`. See
    /// [`TextFrame::subpath_open`] — open contours skip the auto-
    /// close so polygons used as lassoed strokes / clip paths don't
    /// silently fill into rectangles (P-15).
    pub subpath_open: Vec<bool>,
    /// `<TextWrapPreference>` parsed off the polygon, if any.
    /// `None` ⇒ the polygon does not exclude text.
    pub text_wrap: Option<TextWrap>,
    pub item_layer: Option<String>,
    /// See [`Rectangle::effects`] (Q-04).
    pub effects: Option<FrameEffects>,
    /// See [`Rectangle::gradient_fill_angle`].
    pub gradient_fill_angle: Option<f32>,
    /// See [`Rectangle::gradient_fill_length`].
    pub gradient_fill_length: Option<f32>,
    /// See [`Rectangle::gradient_stroke_angle`].
    pub gradient_stroke_angle: Option<f32>,
    /// See [`Rectangle::gradient_stroke_length`].
    pub gradient_stroke_length: Option<f32>,
    /// See [`Rectangle::opacity`].
    pub opacity: Option<f32>,
    /// See [`Rectangle::blend_mode`].
    pub blend_mode: Option<String>,
    /// `<TextPath>` children attached to this polygon. IDML allows a
    /// page item to host more than one text-on-path entry (one per
    /// "slot" in the path effect); typical files carry exactly one.
    pub text_paths: Vec<TextPath>,
    /// `LinkResourceURI` from a nested `<Image>` (or its `<Link>`
    /// child). Mirrors [`Rectangle::image_link`]: a polygon may host
    /// a placed image just like a rectangle, in which case the
    /// polygon's path becomes the image's clip mask. `None` means the
    /// polygon is a plain colour swatch.
    pub image_link: Option<String>,
    /// See [`Rectangle::has_image_element`].
    pub has_image_element: bool,
    /// Inline-`<PDF>`-without-link marker (Q-06). Mirrors
    /// [`Rectangle::has_inline_pdf`].
    pub has_inline_pdf: bool,
    /// `ItemTransform` attribute on the nested `<Image>` element.
    /// See [`Rectangle::image_item_transform`].
    pub image_item_transform: Option<[f32; 6]>,
    /// Q-03: see [`Rectangle::image_bytes`].
    pub image_bytes: Option<Vec<u8>>,
    /// W1.21: see [`Rectangle::image_clip`].
    pub image_clip: Option<ClippingPathSettings>,
    /// `OverprintFill="true"`. See [`TextFrame::overprint_fill`].
    pub overprint_fill: bool,
    /// `OverprintStroke="true"`. See [`TextFrame::overprint_stroke`].
    pub overprint_stroke: bool,
    /// SDK Phase 5 (v1 sweep) — `Nonprinting`. See
    /// [`TextFrame::nonprinting`].
    pub nonprinting: bool,
    /// W2.5 — element-level `Visible` (default `true`). See
    /// [`TextFrame::visible`].
    pub visible: bool,
    /// W2.5 — element-level `Locked` (default `false`). See
    /// [`TextFrame::locked`].
    pub locked: bool,
}

// ---------------------------------------------------------------------------
// Graphic / colour model — the swatch-palette value types (moved out of
// `paged-parse::graphic`; the `<Color>`/`<Gradient>`/`<Swatch>` XML parsing +
// the `Graphic` container itself stay in the parser). Colour math
// (`effective_cmyk`, `to_linear_rgb`) and the `from_attr`/`as_attr` token maps
// are runtime render vocabulary, so they live with the model.
// ---------------------------------------------------------------------------

/// SDK Phase 5 (v1 sweep) — `<ColorGroup>`. Named grouping of
/// `Color` self_ids the document organises its swatch palette
/// into. Kept for round-trip + the editor's Color Groups panel.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ColorGroupEntry {
    pub self_id: String,
    pub name: Option<String>,
    /// IDML `ColorGroupSwatches` attribute — space-separated
    /// list of `Color/<self_id>` (or `Swatch/<self_id>`) refs.
    /// Stored as-parsed; the editor resolves them against the
    /// `colors` table for display.
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorEntry {
    pub self_id: String,
    pub name: Option<String>,
    pub space: ColorSpace,
    pub value: Vec<f32>,
    /// IDML `Model` attribute. `Process` is the default; `Spot` marks
    /// a named-ink swatch (e.g. PANTONE 286) which the renderer
    /// previews via the alternate-CMYK fallback.
    pub model: ColorModel,
    /// `AlternateSpace` — colour space of the CMYK / RGB fallback the
    /// IDML carries for a spot swatch. `None` when not a spot or when
    /// the producer omitted it.
    pub alternate_space: Option<ColorSpace>,
    /// `AlternateColorValue` — whitespace-separated channel values
    /// in the `alternate_space`. For a CMYK alternate this is four
    /// percentages.
    pub alternate_value: Vec<f32>,
    /// `TintValue` (0..=100) stored on a spot `<Color>` swatch that
    /// represents "base spot ink at N% tint". `None` means the swatch
    /// has no swatch-level tint (the most common case — per-use tints
    /// arrive via `FillTint` on the *user*, handled separately).
    pub tint: Option<f32>,
    /// Optional alpha channel (0..=1, 1 = fully opaque) sourced from
    /// the IDML `Alpha` / `AlphaPercentage` attribute on `<Color>`.
    /// `None` means the swatch carries no alpha; the consumer should
    /// treat the swatch as opaque. Used by the gradient-feather
    /// renderer when a `<GradientStop>` in spec form references a
    /// `<Color>` whose alpha defines the stop's opacity.
    pub alpha: Option<f32>,
}

impl ColorEntry {
    /// Resolve a swatch to the effective CMYK percentages a renderer
    /// should send to ICC. Returns `Some([c, m, y, k])` when:
    ///
    /// * the swatch is a process CMYK colour (just returns `value`), or
    /// * the swatch is a spot colour with a CMYK alternate — in which
    ///   case the swatch-level `TintValue` (if any) is multiplied into
    ///   each channel here (`tinted = base * tint / 100`), matching
    ///   InDesign's preview interpolation between the spot ink and
    ///   paper white in CMYK before the ICC transform.
    ///
    /// Returns `None` for RGB / LAB / Gray swatches and for spot
    /// colours whose alternate isn't CMYK (rare; caller falls back to
    /// the swatch's primary `value` via [`to_linear_rgb`]).
    pub fn effective_cmyk(&self) -> Option<[f32; 4]> {
        let (base_space, base_value) = match self.model {
            ColorModel::Spot => {
                // Spot inks are previewed via the CMYK alternate; we
                // don't try to interpret the spot's primary Lab/RGB
                // value because spot rendering requires a spectral
                // model we don't ship.
                match self.alternate_space {
                    Some(ColorSpace::Cmyk) if self.alternate_value.len() == 4 => {
                        (ColorSpace::Cmyk, self.alternate_value.as_slice())
                    }
                    _ => return None,
                }
            }
            _ => (self.space, self.value.as_slice()),
        };
        if base_space != ColorSpace::Cmyk || base_value.len() != 4 {
            return None;
        }
        let t = self
            .tint
            .map(|v| (v / 100.0).clamp(0.0, 1.0))
            .unwrap_or(1.0);
        Some([
            base_value[0] * t,
            base_value[1] * t,
            base_value[2] * t,
            base_value[3] * t,
        ])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwatchEntry {
    pub self_id: String,
    pub name: Option<String>,
    /// `Self` reference to the Color this swatch wraps, if any.
    pub color_ref: Option<String>,
}

/// IDML gradient swatch. Stops reference Color entries by `Self` id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradientEntry {
    pub self_id: String,
    pub name: Option<String>,
    pub kind: GradientKind,
    pub stops: Vec<GradientStopRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GradientKind {
    Linear,
    Radial,
    Unknown,
}

/// One stop in a gradient: a Color reference + a normalised location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradientStopRef {
    pub stop_color: String,
    /// `Location` attribute, 0..=100 in IDML.
    pub location_pct: f32,
    /// `Midpoint` attribute, 0..=100 (default 50): where, *within the
    /// segment to the next stop*, the colour reaches the halfway blend.
    /// `None` ⇒ the file omitted it ⇒ treat as the linear 50.
    pub midpoint_pct: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ColorSpace {
    Cmyk,
    Rgb,
    Lab,
    Gray,
    /// Anything we didn't recognise — callers should treat it as
    /// unresolved and fall back to a sensible default.
    Unknown,
}

/// IDML `<Color Model="…">`. `Process` is the default (CMYK / RGB /
/// Lab inks blended on press); `Spot` marks a named ink that ships
/// with a CMYK fallback for preview / un-spotted output. `MixedInk`
/// is recognised but treated as `Unknown` — we don't ship the
/// per-ink decomposition.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ColorModel {
    Process,
    Spot,
    MixedInk,
    Unknown,
}

impl ColorModel {
    pub fn from_attr(s: &str) -> Self {
        match s {
            "Process" => ColorModel::Process,
            "Spot" => ColorModel::Spot,
            "MixedInk" | "MixedInkGroup" => ColorModel::MixedInk,
            _ => ColorModel::Unknown,
        }
    }

    /// IDML `Model` attribute string, round-trippable with `from_attr`.
    pub fn as_attr(self) -> &'static str {
        match self {
            ColorModel::Process => "Process",
            ColorModel::Spot => "Spot",
            ColorModel::MixedInk => "MixedInk",
            ColorModel::Unknown => "Unknown",
        }
    }
}

impl ColorSpace {
    pub fn from_attr(s: &str) -> Self {
        match s {
            "CMYK" => ColorSpace::Cmyk,
            "RGB" => ColorSpace::Rgb,
            "LAB" | "Lab" => ColorSpace::Lab,
            "Gray" => ColorSpace::Gray,
            _ => ColorSpace::Unknown,
        }
    }

    /// IDML `Space` attribute string, round-trippable with `from_attr`.
    pub fn as_attr(self) -> &'static str {
        match self {
            ColorSpace::Cmyk => "CMYK",
            ColorSpace::Rgb => "RGB",
            ColorSpace::Lab => "LAB",
            ColorSpace::Gray => "Gray",
            ColorSpace::Unknown => "Unknown",
        }
    }
}

/// Convert a [`ColorEntry`] to non-color-managed linear RGB (0..=1).
///
/// This is a stopgap — the proper path goes through `paged-color` with
/// ICC profiles. Fine for exploratory tooling and the fidelity
/// harness's first seed documents.
///
/// Spot swatches route through their CMYK alternate (with any
/// swatch-level `TintValue` already folded in by
/// [`ColorEntry::effective_cmyk`]). Spot swatches without a CMYK
/// alternate fall back to whatever the primary `Space` claims —
/// usually `Lab`, which we render as `None` (unresolved).
pub fn to_linear_rgb(c: &ColorEntry) -> Option<[f32; 3]> {
    if let Some([cv, mv, yv, kv]) = c.effective_cmyk() {
        let cv = cv / 100.0;
        let mv = mv / 100.0;
        let yv = yv / 100.0;
        let kv = kv / 100.0;
        let r = (1.0 - cv) * (1.0 - kv);
        let g = (1.0 - mv) * (1.0 - kv);
        let b = (1.0 - yv) * (1.0 - kv);
        return Some([srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)]);
    }
    let v = c.value.as_slice();
    match c.space {
        ColorSpace::Rgb if v.len() == 3 => Some([
            srgb_to_linear(v[0] / 255.0),
            srgb_to_linear(v[1] / 255.0),
            srgb_to_linear(v[2] / 255.0),
        ]),
        ColorSpace::Gray if v.len() == 1 => {
            let g = srgb_to_linear(1.0 - v[0] / 100.0);
            Some([g, g, g])
        }
        _ => None,
    }
}

/// Concept 2 — the reserved swatches (`[None]`, `[Paper]`,
/// `[Black]`, `[Registration]`). Never editable or deletable;
/// semantics: None = no paint, Paper = the substrate (knockout, not
/// white ink), Black = 100% K process, Registration = prints on ALL
/// plates. This is THE classifier — display sites and the none-fill
/// fast paths route through it instead of scattering id matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservedSwatch {
    None,
    Paper,
    Black,
    Registration,
}

impl ReservedSwatch {
    /// Classify a swatch/colour reference. Handles both the
    /// `Color/<name>` and `Swatch/<name>` spellings plus the legacy
    /// `"n"` / empty forms IDML uses for "no paint".
    pub fn classify(id: &str) -> Option<Self> {
        match id {
            "Color/None" | "Swatch/None" | "n" | "" => Some(Self::None),
            "Color/Paper" | "Swatch/Paper" => Some(Self::Paper),
            "Color/Black" | "Swatch/Black" => Some(Self::Black),
            "Color/Registration" | "Swatch/Registration" => Some(Self::Registration),
            _ => None,
        }
    }

    /// True for any spelling of the no-paint swatch.
    pub fn is_none(id: &str) -> bool {
        matches!(Self::classify(id), Some(Self::None))
    }

    /// The wire/display label ("none" / "paper" / "black" /
    /// "registration").
    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Paper => "paper",
            Self::Black => "black",
            Self::Registration => "registration",
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

// ---------------------------------------------------------------------------
// DesignMap value types — sections, layers, hyperlinks, preferences, footnote
// options, page-numbering (moved out of `paged-parse::designmap`; the
// `designmap.xml` XML parsing + the `DesignMap` container itself stay in the
// parser). Page-number formatting + the `from_idml`/`as_str` token maps are
// runtime render vocabulary, so they live with the model.
// ---------------------------------------------------------------------------
/// Page-numbering style for a `<Section>` (`PageNumberStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumberingStyle {
    Arabic,
    UpperRoman,
    LowerRoman,
    UpperAlpha,
    LowerAlpha,
}

impl NumberingStyle {
    /// Map an IDML `PageNumberStyle` value. Unknown / unsupported
    /// styles (Kanji, Katakana, …) fall back to Arabic.
    pub fn from_idml(s: &str) -> Self {
        match s {
            "UpperRoman" => NumberingStyle::UpperRoman,
            "LowerRoman" => NumberingStyle::LowerRoman,
            "UpperLetters" => NumberingStyle::UpperAlpha,
            "LowerLetters" => NumberingStyle::LowerAlpha,
            _ => NumberingStyle::Arabic,
        }
    }

    /// Stable lower-camel wire name for the editor's section panel
    /// (panels.md gaps 9/10/19). Distinct from `format`, which
    /// renders a number; this names the *style* itself.
    pub fn as_str(self) -> &'static str {
        match self {
            NumberingStyle::Arabic => "arabic",
            NumberingStyle::UpperRoman => "upperRoman",
            NumberingStyle::LowerRoman => "lowerRoman",
            NumberingStyle::UpperAlpha => "upperAlpha",
            NumberingStyle::LowerAlpha => "lowerAlpha",
        }
    }

    /// Format a 1-based page number in this style. `0` (or anything
    /// the roman/alpha encoders can't represent) renders as the bare
    /// Arabic digits so the label is never empty.
    pub fn format(self, n: u32) -> String {
        match self {
            NumberingStyle::Arabic => n.to_string(),
            NumberingStyle::UpperRoman => to_roman(n).unwrap_or_else(|| n.to_string()),
            NumberingStyle::LowerRoman => to_roman(n)
                .map(|r| r.to_lowercase())
                .unwrap_or_else(|| n.to_string()),
            NumberingStyle::UpperAlpha => to_alpha(n).unwrap_or_else(|| n.to_string()),
            NumberingStyle::LowerAlpha => to_alpha(n)
                .map(|a| a.to_lowercase())
                .unwrap_or_else(|| n.to_string()),
        }
    }
}

/// Classic additive Roman numerals (1..=3999). Returns `None` outside
/// that range so callers fall back to Arabic.
fn to_roman(mut n: u32) -> Option<String> {
    if n == 0 || n > 3999 {
        return None;
    }
    const TABLE: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, sym) in TABLE {
        while n >= value {
            out.push_str(sym);
            n -= value;
        }
    }
    Some(out)
}

/// Spreadsheet-style alphabetic numbering: 1→A, 26→Z, 27→AA, 28→AB.
/// `None` for 0.
fn to_alpha(mut n: u32) -> Option<String> {
    if n == 0 {
        return None;
    }
    let mut out = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        out.push(b'A' + rem);
        n = (n - 1) / 26;
    }
    out.reverse();
    Some(String::from_utf8(out).expect("ascii"))
}

/// IDML `<Section>` definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub self_id: String,
    /// `PageStart` — the `Self` of the `<Page>` this section begins at.
    pub page_start: Option<String>,
    /// `ContinueNumbering="true"` — the section continues the running
    /// page number from the previous section rather than restarting.
    pub continue_numbering: bool,
    /// `PageNumberStart` — the number the section's first page takes
    /// when `continue_numbering` is false. Defaults to 1.
    pub start_at: Option<u32>,
    /// `PageNumberStyle`, defaulting to Arabic.
    pub numbering_style: NumberingStyle,
    /// `SectionPrefix` — prepended to the formatted number when
    /// `include_prefix` is set (e.g. `"A-"` → "A-1").
    pub section_prefix: Option<String>,
    /// `Marker` — the section marker text (chapter marker). Captured
    /// for round-trip / tooling; not used in the page label today.
    pub marker: Option<String>,
    /// `IncludeSectionPrefix="true"` — whether the prefix shows in the
    /// page label.
    pub include_prefix: bool,
}

/// IDML `<Article>` definition. Members reference stories via
/// `ArticleMember/ItemRef`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub self_id: String,
    pub name: Option<String>,
    /// Member self_ids the article wraps (typically Story refs).
    pub members: Vec<String>,
}

/// IDML `<Hyperlink>` definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hyperlink {
    pub self_id: String,
    pub name: Option<String>,
    /// Source ref (typically `HyperlinkTextSource/<id>`).
    pub source: Option<String>,
    /// Destination ref (URL / page / anchor). May be a
    /// `HyperlinkURLDestination`, `HyperlinkPageDestination`,
    /// or `HyperlinkTextDestination` self_id depending on the
    /// kind of hyperlink.
    pub destination: Option<String>,
}

/// IDML `<Bookmark>` definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub self_id: String,
    pub name: Option<String>,
    /// Destination ref (`HyperlinkTextDestination/<id>` or
    /// `HyperlinkPageDestination/<id>`).
    pub destination: Option<String>,
}

/// W1.4 — a hyperlink destination resource. IDML declares three
/// flavours at the document level, each referenced from a
/// `<Hyperlink Destination="...">`:
///
/// - `HyperlinkURLDestination` carries an external `DestinationURL`.
/// - `HyperlinkPageDestination` points at a `<Page>` by `Self`
///   (`DestinationPage`), optionally with a zoom setting.
/// - `HyperlinkTextDestination` is an in-story text anchor whose
///   `DestinationText` references the hosting story; the renderer
///   resolves it to the page the anchor lands on (best-effort: the
///   first page hosting that story).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperlinkDestination {
    pub self_id: String,
    pub kind: HyperlinkDestinationKind,
}

/// The destination flavour + its payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HyperlinkDestinationKind {
    /// External URL (e.g. `https://paged.media`).
    Url(String),
    /// `DestinationPage` — the `Self` id of the target `<Page>`.
    Page(String),
    /// `DestinationText` — the `Self` id of the target text anchor /
    /// story. Resolved to a page index downstream.
    TextAnchor(String),
}

/// IDML `<CrossReferenceSource>` marker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossReference {
    pub self_id: String,
    pub name: Option<String>,
    /// `AppliedFormat` — ref to a `<CrossReferenceFormat>`.
    pub format: Option<String>,
    /// `Destination` — anchor / text-destination ref.
    pub destination: Option<String>,
}

/// IDML `<Topic>` definition for an index entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexTopic {
    pub self_id: String,
    pub name: Option<String>,
    /// Sort key (`SortOrder` attribute). Some IDMLs use this to
    /// override the alphabetical order.
    pub sort_order: Option<String>,
}

/// IDML `<TextVariable>` declaration. W1.4: the renderer resolves the
/// value per `variable_type` at emit time (falling back to each
/// instance's baked `ResultText` when the type's inputs aren't
/// modelled). The `<TextVariablePreference>` child carries the
/// type-specific payload — the literal contents of a custom variable,
/// the date `Format` string, and the surrounding `TextBefore` /
/// `TextAfter` decoration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextVariable {
    pub self_id: String,
    pub name: Option<String>,
    /// `VariableType` — e.g. `CustomTextType`, `FileNameType`,
    /// `PageCountType`, `CreationDateType`, `ModificationDateType`,
    /// `OutputDateType`, `ChapterNumberType`, `RunningHeaderType`.
    pub variable_type: Option<String>,
    /// `<TextVariablePreference Contents="...">` — the literal value of
    /// a `CustomTextType` variable (verbatim). `None` for other types.
    pub contents: Option<String>,
    /// `<TextVariablePreference Format="...">` — the date/time format
    /// pattern for the date variable types (InDesign/ICU-style tokens).
    /// `None` when absent.
    pub date_format: Option<String>,
    /// `<TextVariablePreference TextBefore="...">` decoration prepended
    /// to the resolved value.
    pub text_before: Option<String>,
    /// `<TextVariablePreference TextAfter="...">` decoration appended to
    /// the resolved value.
    pub text_after: Option<String>,
    /// W1.18c — `<TextVariablePreference AppliedParagraphStyle="...">`
    /// (or `AppliedCharacterStyle`) for `RunningHeaderType` variables:
    /// the style whose nearest on-page occurrence supplies the header
    /// text. `None` for non-header variables.
    pub running_header_style: Option<String>,
    /// W1.18c — `<TextVariablePreference Use="FirstOnPage|LastOnPage">`
    /// — which on-page match a running header picks up. `None` ⇒
    /// FirstOnPage (InDesign's default).
    pub running_header_use: Option<String>,
}

/// IDML `<Layer>` definition. Only the fields the renderer needs
/// today; visibility / printability decide whether items on that
/// layer are emitted at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub self_id: String,
    pub name: Option<String>,
    /// `Visible="true|false"` — when false the layer is hidden in
    /// InDesign's view and PDF export skips it.
    pub visible: bool,
    /// `Locked="true|false"` — purely an editor concern; the renderer
    /// ignores it but we surface the field so future tooling can
    /// honour it.
    pub locked: bool,
    /// `Printable="true|false"` — InDesign's "Print Layer" checkbox.
    /// Non-printable layers are skipped during rendering.
    pub printable: bool,
    /// `Self` of the enclosing `<Layer>` when this layer is nested
    /// inside a layer group (folder) in InDesign's Layers panel.
    /// `None` for a top-level layer — the overwhelmingly common case,
    /// where every `<Layer>` is a self-closing peer. The render-time
    /// visibility / lock resolution ANDs/ORs a layer with its ancestors
    /// so an item on a visible child layer inside a hidden parent group
    /// is still hidden.
    pub parent_id: Option<String>,
}

/// Document-level color management config. Mirrors the attributes that
/// real InDesign exports carry on the `<Document>` element (CS6 / IDML
/// 8.0). Empty defaults match "no opinion" and let the renderer pick
/// a global fallback.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ColorSettings {
    /// `CMYKProfile` attribute, e.g. `"Coated FOGRA39 (ISO 12647-2:2004)"`.
    pub cmyk_profile: Option<String>,
    /// `RGBProfile` attribute, e.g. `"sRGB IEC61966-2.1"`.
    pub rgb_profile: Option<String>,
    /// `SolidColorIntent` — typically `"UseColorSettings"` (use the
    /// document's working spaces) or one of `Perceptual`,
    /// `Saturation`, `RelativeColorimetric`, `AbsoluteColorimetric`.
    pub solid_color_intent: Option<String>,
    /// `AfterBlendingIntent` — same value space as `solid_color_intent`.
    pub after_blending_intent: Option<String>,
    /// `DefaultImageIntent` — same value space.
    pub default_image_intent: Option<String>,
}

/// `<DocumentPreference>` page-setup values the renderer ignores but
/// print export needs. All offsets are points. NOTE the IDML quirk:
/// bleed spells "…InsideOrLeft/…OutsideOrRight" while slug flips the
/// word order to "SlugRightOrOutsideOffset" — that's faithful to the
/// spec, not a typo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct DocumentPreference {
    pub bleed_top: f32,
    pub bleed_bottom: f32,
    pub bleed_inside_or_left: f32,
    pub bleed_outside_or_right: f32,
    pub slug_top: f32,
    pub slug_bottom: f32,
    pub slug_inside_or_left: f32,
    pub slug_right_or_outside: f32,
}

/// `<GridPreference>` — the document's baseline-grid + document-grid
/// settings. InDesign serialises this once under `<Document>`. Only the
/// baseline-grid subset (the part the editor's baseline-grid panel +
/// overlay need) is modelled; the document-grid (horizontal/vertical
/// gridline divisions for the layout grid) is carried too since it
/// shares the element. All offsets / divisions are in points.
///
/// The renderer ignores this entirely (the baseline grid is a
/// non-printing authoring aid). `present` distinguishes "no
/// `<GridPreference>`" (InDesign defaults apply) from "explicitly
/// configured", mirroring [`FootnoteOptions::present`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GridPreference {
    /// True when a `<GridPreference>` element was parsed.
    pub present: bool,
    /// `BaselineStart` — the offset (pt) of the first baseline-grid line
    /// from the top of the page (or from the relative-to point below).
    pub baseline_start: Option<f32>,
    /// `BaselineDivision` — the spacing (pt) between baseline-grid lines.
    pub baseline_division: Option<f32>,
    /// `BaselineGridShown` — whether the grid is shown by default in the
    /// authoring view. The editor's overlay toggle seeds from this.
    pub baseline_grid_shown: Option<bool>,
    /// `BaselineGridRelativeOption` — `"TopOfPage"` / `"TopMargin"`. The
    /// reference the `baseline_start` offset is measured from.
    pub baseline_grid_relative_option: Option<String>,
    /// `BaselineColor` — the grid line colour. Usually a `"Color/…"`
    /// swatch ref or a named UI colour (e.g. `"LightBlue"`). The overlay
    /// resolves it to a stroke colour; an unknown name falls back to the
    /// editor's default guide colour.
    pub baseline_color: Option<String>,
    /// `HorizontalGridlineDivision` — document-grid horizontal spacing
    /// (pt). Carried for completeness; the baseline panel ignores it.
    pub horizontal_gridline_division: Option<f32>,
    /// `VerticalGridlineDivision` — document-grid vertical spacing (pt).
    pub vertical_gridline_division: Option<f32>,
}

/// `<FootnoteOption>` — document-level footnote separator + spacing
/// settings. In IDML this element is serialised inside the document's
/// `<RootFootnoteStory>` (or directly under `<Document>`); its attribute
/// names mirror the InDesign DOM `FootnoteOption` object exactly. Only
/// the subset the renderer consumes is modelled here.
///
/// W1.8 — the renderer draws a separator rule above each frame's
/// footnote pool when `rule_on` is true, using `rule_*`. The `spacer`
/// (minimum gap between body and first footnote) and `space_between`
/// (gap between footnotes) feed the pool layout's vertical metrics.
///
/// `None` everywhere is the absent-element default; the renderer then
/// falls back to InDesign's own defaults (`rule_on = true`, a 0.5pt
/// black rule 50% of the column wide). See [`FootnoteOptions::is_default`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FootnoteOptions {
    /// True when the element was present in the designmap. When false
    /// the struct is all-`None` and the renderer applies its built-in
    /// defaults; we keep the flag so "rule explicitly off" (`rule_on =
    /// Some(false)`) is distinguishable from "no FootnoteOption at all".
    pub present: bool,
    /// `RuleOn` — draw the separator rule above the first footnote.
    pub rule_on: Option<bool>,
    /// `RuleColor` — swatch id (`Color/...`) for the rule stroke.
    pub rule_color: Option<String>,
    /// `RuleTint` — tint percent (0–100) of the rule colour.
    pub rule_tint: Option<f32>,
    /// `RuleLineWeight` — stroke weight of the rule, in points.
    pub rule_line_weight: Option<f32>,
    /// `RuleWidth` — length of the rule, in points (the drawn segment;
    /// InDesign measures it from `rule_left_indent`).
    pub rule_width: Option<f32>,
    /// `RuleLeftIndent` — left inset of the rule from the column edge,
    /// in points.
    pub rule_left_indent: Option<f32>,
    /// `RuleOffset` — vertical offset of the rule above the first
    /// footnote's baseline-anchored top, in points.
    pub rule_offset: Option<f32>,
    /// `SeparatorText` — string between the footnote marker number and
    /// its text (e.g. `"\t"`). The renderer expands `^t`/`^m` markers.
    pub separator_text: Option<String>,
    /// `Spacer` — minimum vertical space between the text-column bottom
    /// and the first footnote, in points.
    pub spacer: Option<f32>,
    /// `SpaceBetween` — vertical space between consecutive footnotes,
    /// in points.
    pub space_between: Option<f32>,
}

impl FootnoteOptions {
    /// True when no `<FootnoteOption>` was parsed (or it carried no
    /// recognised attributes). Lets the renderer cheaply skip the
    /// separator/spacing machinery for the overwhelmingly common case
    /// of a document with no customised footnote settings.
    pub fn is_default(&self) -> bool {
        !self.present
    }

    /// Effective `rule_on`, applying InDesign's default (rule ON) when
    /// the document didn't say.
    pub fn rule_on_effective(&self) -> bool {
        self.rule_on.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpreadRef {
    pub src: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryRef {
    pub src: String,
}

// ---------------------------------------------------------------------------
// Story shared leaf types (moved out of `paged-parse::story`): the `Justification`
// alignment enum (+ its manual IDML-string serde), the `TabStop` stop, and the
// `OtfFeatures` OpenType flag bag (+ `merge_below` cascade). The XML parsing
// (`parse_otf_features`, tab-stop parsing) stays in the parser. Moved first so
// the styles value types that embed them can follow (N5).
// ---------------------------------------------------------------------------
/// IDML `Justification` attribute values, as carried on
/// `<ParagraphStyleRange>` and `<ParagraphStyle>`. The IDML default
/// is `LeftAlign`. Parsed once at XML-read time; the renderer maps
/// these down to `paged_text::Alignment` (Left / Right / Center /
/// Justify).
///
/// `ToBindingSide` / `AwayFromBindingSide` are binding-aware values
/// (left page vs. right page in a spread). The renderer currently
/// treats them as `LeftAlign` / `RightAlign` respectively — binding
/// side is a document-level setting that's not yet plumbed through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Justification {
    LeftAlign,
    CenterAlign,
    RightAlign,
    /// Justify with last line left-aligned.
    LeftJustified,
    /// Justify with last line centered.
    CenterJustified,
    /// Justify with last line right-aligned.
    RightJustified,
    /// "Fully justified" — every line including the last is stretched
    /// to fill the column. The composer currently treats this the
    /// same as `LeftJustified`; kept as a distinct variant so
    /// round-tripping the parsed attribute is lossless.
    FullyJustified,
    /// Binding-aware: aligns toward the spine. Falls back to
    /// `LeftAlign` until binding side is plumbed through.
    ToBindingSide,
    /// Binding-aware: aligns away from the spine. Falls back to
    /// `RightAlign`.
    AwayFromBindingSide,
}
impl Justification {
    /// Parse an IDML attribute value. Unknown values return `None`,
    /// which mirrors the pre-enum stringly-typed behaviour (the
    /// renderer's `map_justification` fell through to Left for any
    /// value it didn't recognise).
    pub fn from_idml(s: &str) -> Option<Self> {
        match s {
            "LeftAlign" => Some(Self::LeftAlign),
            "CenterAlign" => Some(Self::CenterAlign),
            "RightAlign" => Some(Self::RightAlign),
            "LeftJustified" => Some(Self::LeftJustified),
            "CenterJustified" => Some(Self::CenterJustified),
            "RightJustified" => Some(Self::RightJustified),
            "FullyJustified" => Some(Self::FullyJustified),
            "ToBindingSide" => Some(Self::ToBindingSide),
            "AwayFromBindingSide" => Some(Self::AwayFromBindingSide),
            _ => None,
        }
    }

    /// Render back to the IDML attribute string. Used by JSON
    /// surfaces (the editor wasm bridge) and any path that needs to
    /// round-trip the value through a string format.
    pub fn as_idml(self) -> &'static str {
        match self {
            Self::LeftAlign => "LeftAlign",
            Self::CenterAlign => "CenterAlign",
            Self::RightAlign => "RightAlign",
            Self::LeftJustified => "LeftJustified",
            Self::CenterJustified => "CenterJustified",
            Self::RightJustified => "RightJustified",
            Self::FullyJustified => "FullyJustified",
            Self::ToBindingSide => "ToBindingSide",
            Self::AwayFromBindingSide => "AwayFromBindingSide",
        }
    }
}
// Serialise as the IDML attribute string ("LeftAlign", etc.) so the
// JSON wire format used by the editor bridge stays stable across
// the enum promotion. Deserialise rejects unknown strings via a
// serde error (matches `from_idml`'s strictness).
impl Serialize for Justification {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_idml())
    }
}
impl<'de> Deserialize<'de> for Justification {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = <&str>::deserialize(de)?;
        Self::from_idml(s)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown Justification value: {s:?}")))
    }
}
/// One stop in a paragraph's `<TabList>`. Position is in pt from
/// the column's left edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabStop {
    pub position: f32,
    /// IDML alignment string: `LeftAlign`, `RightAlign`,
    /// `CenterAlign`, `CharacterAlign`.
    pub alignment: Option<String>,
    /// `AlignmentCharacter` for `CharacterAlign` stops (rare).
    pub alignment_character: Option<String>,
    /// `Leader` string rendered in the tab gap.
    pub leader: Option<String>,
}
/// Phase 4 typography — the discrete OpenType feature toggles IDML
/// records as individual attributes on a `<CharacterStyleRange>` /
/// `<CharacterStyle>` (each `OTF*` attribute is its own flag, not a
/// packed tag list).
///
/// Every field is `Option` so the style cascade can distinguish
/// "unset at this level — inherit" from "explicitly off". A bottom-of-
/// cascade `None` means the feature is off (its OpenType default).
/// `merge_below` fills each unset field from the level below.
///
/// The renderer maps these to rustybuzz feature tags in
/// `paged_text::ShapingFeatures` — see that type for the tag table.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct OtfFeatures {
    /// `OTFFraction="true"` — `frac` (diagonal fractions like ½).
    pub fraction: Option<bool>,
    /// `OTFOrdinal="true"` — `ordn` (superscripted ordinals: 1st, 2nd).
    pub ordinal: Option<bool>,
    /// `OTFSwash="true"` — `swsh` (swash alternates).
    pub swash: Option<bool>,
    /// `OTFDiscretionaryLigature="true"` — `dlig` (discretionary
    /// ligatures, distinct from the standard `liga`/`clig` driven by
    /// [`CharacterRun::ligatures_on`]).
    pub discretionary_ligatures: Option<bool>,
    /// `OTFSlashedZero="true"` — `zero` (slashed zero).
    pub slashed_zero: Option<bool>,
    /// `OTFTitling="true"` — `titl` (titling alternates).
    pub titling: Option<bool>,
    /// `OTFContextualAlternate` — `calt` (contextual alternates). IDML
    /// defaults this on; we treat `None` as "inherit", and only the
    /// explicit `false` disables `calt`.
    pub contextual_alternates: Option<bool>,
    /// `OTFFigureStyle` raw string — one of `Default`, `Lining`,
    /// `OldStyle`, `TabularLining`, `ProportionalLining`,
    /// `TabularOldstyle`, `ProportionalOldstyle`. Drives the figure
    /// (digit) features `lnum`/`onum` (lining vs oldstyle) and
    /// `pnum`/`tnum` (proportional vs tabular). `None`/`Default` ⇒ the
    /// font's own default digits (no figure feature forced).
    pub figure_style: Option<String>,
    /// `OTFStylisticSets` integer bitfield. InDesign packs the enabled
    /// stylistic sets into one integer where bit `i` (0-based) enables
    /// `ss{i+1}` (`ss01`..`ss20`). `0`/`None` ⇒ no stylistic set.
    pub stylistic_sets: Option<i32>,
}
impl OtfFeatures {
    /// Fill any unset field from `below` (a lower cascade level: this
    /// run's character style, then paragraph style). Nothing is
    /// overwritten once set.
    pub fn merge_below(&mut self, below: &OtfFeatures) {
        self.fraction = self.fraction.or(below.fraction);
        self.ordinal = self.ordinal.or(below.ordinal);
        self.swash = self.swash.or(below.swash);
        self.discretionary_ligatures = self
            .discretionary_ligatures
            .or(below.discretionary_ligatures);
        self.slashed_zero = self.slashed_zero.or(below.slashed_zero);
        self.titling = self.titling.or(below.titling);
        self.contextual_alternates = self.contextual_alternates.or(below.contextual_alternates);
        if self.figure_style.is_none() {
            self.figure_style = below.figure_style.clone();
        }
        self.stylistic_sets = self.stylistic_sets.or(below.stylistic_sets);
    }
}

// ---------------------------------------------------------------------------
// Style-definition value types — condition/stroke/TOC/object/cell/table/character/
// paragraph style defs, nested styles, and the Resolved* cascade accumulators
// (moved out of `paged-parse::styles`; `StyleSheet` + `StyleSheet::parse` + all the
// `parse_*` free fns stay in the parser). The merge_below cascade + merge_rule/
// merge_border are pure style resolution, so they live with the model (N5).
// ---------------------------------------------------------------------------
/// IDML `<Condition>` — a named visibility toggle that can be applied
/// to a `<CharacterStyleRange>` (and other text-marker elements). The
/// document carries the current `Visible` setting per condition. A
/// run whose `AppliedConditions` reference one or more conditions is
/// rendered only when every referenced condition resolves to `Visible="true"`.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionDef {
    pub self_id: String,
    pub name: Option<String>,
    /// `Visible="true|false"`. Default: true (`None` ⇒ visible).
    pub visible: Option<bool>,
    /// `IndicatorMethod` — `Underline` / `Highlight` / `None`. The
    /// renderer ignores indicators today; captured for round-trip.
    pub indicator_method: Option<String>,
}
/// SDK Phase 5 (v1 sweep) — IDML `<ConditionSet>`. Each entry is a
/// named grouping of `Condition` self_ids that the editor's
/// Conditions panel can toggle as a unit. The renderer doesn't
/// branch on this today (visibility resolution walks individual
/// conditions); kept for round-trip + a future "show only this
/// set" affordance.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionSetDef {
    pub self_id: String,
    pub name: Option<String>,
    /// IDML `Conditions` attribute — space-separated list of
    /// `Condition/<self_id>` refs (or `Condition/$ID/...` for
    /// IDs in the special namespace). Stored as-parsed; the
    /// editor de-dupes for display.
    pub conditions: Vec<String>,
}
/// W1.22 (engine gap 22) — IDML `<NumberingList>` resource. A named
/// list definition. Paragraphs reference one via
/// `AppliedNumberingList="NumberingList/<self_id>"`; the numbering
/// counter for that list is scoped per the continuity flags below.
///
/// `ContinueNumbersAcrossStories` is the field that matters to the
/// renderer: when `true`, paragraphs sharing this list keep counting
/// across story boundaries (in document story order) instead of
/// restarting at 1 in each story. `ContinueNumbersAcrossDocuments`
/// is captured for round-trip only — a single rendered document has
/// no neighbouring document to continue from, so the renderer treats
/// it as a no-op (documented in `numbering.rs`).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct NumberingListDef {
    pub self_id: String,
    pub name: Option<String>,
    /// `ContinueNumbersAcrossStories="true|false"`. Default: false
    /// (`None` ⇒ each story restarts — InDesign's default for a new
    /// list). When true, the renderer carries the counter forward
    /// across stories that share this list.
    pub continue_across_stories: Option<bool>,
    /// `ContinueNumbersAcrossDocuments="true|false"`. Round-trip only;
    /// see the struct doc. Default: false.
    pub continue_across_documents: Option<bool>,
}
/// Custom stroke-style definition. The renderer consumes the
/// `Dashed`/`Dotted` patterns directly, the `Striped` stripe table as
/// N parallel rules, and the `Wavy` width/wavelength as a sampled sine
/// (W1.2). Anything still unused is captured so we don't lose it during
/// round-trips.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrokeStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub kind: StrokeStyleKind,
    /// On/off pattern in pt for `Dashed` (the `Pattern` attribute
    /// parsed as space-separated floats). Empty for the other kinds.
    pub pattern: Vec<f32>,
    /// `<Stripe>` children of a `<StripedStrokeStyle>`. Each entry is
    /// `(left, width)` as fractions in `0.0..=1.0` of the *total*
    /// stroke weight — InDesign serialises them as 0..1 ratios on the
    /// `StartWidth` / `Width` attributes. Empty for non-striped kinds.
    pub stripes: Vec<StripeDef>,
    /// `<WavyStrokeStyle Width=… Wavelength=…>` — the wave amplitude
    /// and period as fractions of the stroke weight (InDesign's 0..1
    /// ratios). `None` when this isn't a wavy style or the attribute
    /// was absent (the renderer then substitutes IDML defaults).
    pub wave_width: Option<f32>,
    pub wave_length: Option<f32>,
    /// `GapColor` swatch ref painted in the gaps of a dashed / dotted /
    /// striped stroke (W1.2). IDML carries this on the *stroke-style
    /// definition*, not the page item. `Swatch/None` normalises to
    /// `None` (no gap fill — the default).
    pub gap_color: Option<String>,
    /// `GapTint` — 0..100 dilution of the gap colour toward paper.
    /// `None` ⇒ full strength.
    pub gap_tint: Option<f32>,
}
/// One stripe of a `<StripedStrokeStyle>`. `left` and `width` are
/// fractions of the total stroke weight (`0.0..=1.0`). The stripe's
/// centreline sits at `left + width/2` measured from the stroke's
/// upper edge, and its sub-weight is `width * total_weight`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StripeDef {
    pub left: f32,
    pub width: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokeStyleKind {
    Dashed,
    Dotted,
    Striped,
    Wavy,
}
/// `<TOCStyle>` — Table of Contents style. Carries the heading text,
/// the paragraph style for the title, and an ordered list of
/// `<TOCStyleEntry>` children declaring which paragraph styles
/// should be picked up as TOC entries.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TOCStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    /// `Title` attribute — the heading text printed at the top of
    /// the resolved TOC story (e.g. `"Contents"` / `"Inhalt"`).
    /// `None` when omitted; some IDMLs use an empty string.
    pub title: Option<String>,
    /// `TitleStyle` — `ParagraphStyle/<id>` reference applied to
    /// the title paragraph. May resolve to the `[No paragraph
    /// style]` sentinel for the default TOCStyle.
    pub title_style: Option<String>,
    /// `IncludeBookDocuments` — true when entries should be pulled
    /// from sibling book documents in addition to this one. Single-
    /// document renders ignore this; captured for round-trip.
    pub include_book_documents: Option<bool>,
    /// `IncludeHidden` — when true the resolver should also pick up
    /// paragraphs on hidden layers. The renderer currently honours
    /// layer visibility at emission time and matches this default.
    pub include_hidden: Option<bool>,
    /// `RunIn` — when true, sibling entries at the same level
    /// concatenate on the same line separated by a soft separator
    /// rather than each landing on its own line. The current
    /// resolver leaves run-in handling to the renderer; captured
    /// here for round-trip.
    pub run_in: Option<bool>,
    /// Ordered list of `<TOCStyleEntry>` children in document order.
    pub entries: Vec<TOCStyleEntryDef>,
}
/// `<TOCStyleEntry>` — one row in the TOC style table. IDML serialises
/// these in document order under the `<TOCStyle>`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TOCStyleEntryDef {
    /// `Name` — human-readable label (usually mirrors the paragraph
    /// style name picked up by `IncludeStyle`).
    pub name: Option<String>,
    /// `IncludeStyle` — `ParagraphStyle/<id>` reference. Paragraphs
    /// with this applied paragraph style feed the TOC entry.
    pub include_style: Option<String>,
    /// `FormatStyle` — `ParagraphStyle/<id>` reference applied to
    /// the rendered TOC entry paragraph.
    pub format_style: Option<String>,
    /// `Level` — outline depth (1 is the top level). `None` falls
    /// back to 1 at resolve time.
    pub level: Option<u32>,
    /// `PageNumber` — IDML enum (`On` / `Off` / `NoPageNumber`).
    /// `On` is the default when absent.
    pub page_number: Option<String>,
    /// `Separator` — string placed between the entry text and the
    /// page number. IDML serialises tabs as `^t`; the resolver
    /// expands them at use time. Default `^t` when absent.
    pub separator: Option<String>,
}
/// `<ObjectStyle>` — the page-item analogue of paragraph/character
/// styles. Carries fill + stroke defaults that flow into a frame
/// when it carries no per-element override.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ObjectStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub fill_color: Option<String>,
    /// `FillTint` percentage [0..100] from `<ObjectStyle FillTint="…">`.
    /// `None` ⇒ inherit from BasedOn (and ultimately default to 100%
    /// at the renderer). Cascades into a frame whose own inline
    /// `FillTint` is absent — needed for placeholder rects whose
    /// 15% grey paint comes entirely from the style.
    pub fill_tint: Option<f32>,
    pub stroke_color: Option<String>,
    pub stroke_tint: Option<f32>,
    pub stroke_weight: Option<f32>,
    /// `CornerRadius` in pt. Only honoured when `CornerOption` is one
    /// of the rounding variants (`Rounded`, `InverseRounded`, `Inset`,
    /// `Bevel`, `Fancy`). `None` ⇒ inherit from BasedOn.
    pub corner_radius: Option<f32>,
    /// `CornerOption` value (`None | Rounded | InverseRounded | Inset
    /// | Bevel | Fancy`). The renderer maps `Rounded` to a rounded-
    /// rect path; the decorative variants currently fall back to
    /// `Rounded` until per-shape parsers land.
    pub corner_option: Option<String>,
}
/// Effective object-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedObject {
    pub fill_color: Option<String>,
    pub fill_tint: Option<f32>,
    pub stroke_color: Option<String>,
    pub stroke_tint: Option<f32>,
    pub stroke_weight: Option<f32>,
    pub corner_radius: Option<f32>,
    pub corner_option: Option<String>,
}
/// `<CellStyle>` — per-cell defaults for fill, edge strokes, and
/// vertical justification. Cells can override individual fields
/// inline; missing fields cascade through `BasedOn` and finally
/// fall through to renderer defaults.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CellStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub fill_color: Option<String>,
    pub vertical_justification: Option<String>,
    /// `RotationAngle` (degrees) for the cell's content.
    pub rotation_angle: Option<f32>,
    pub top_edge_stroke_color: Option<String>,
    pub top_edge_stroke_weight: Option<f32>,
    pub bottom_edge_stroke_color: Option<String>,
    pub bottom_edge_stroke_weight: Option<f32>,
    pub left_edge_stroke_color: Option<String>,
    pub left_edge_stroke_weight: Option<f32>,
    pub right_edge_stroke_color: Option<String>,
    pub right_edge_stroke_weight: Option<f32>,
}
/// `<TableStyle>` — table-level defaults that flow through to
/// cells. Carries the region → CellStyle map (Header / Body /
/// Footer / Left / Right column regions) plus the table border
/// strokes. BasedOn cascade applies the same way as the other
/// resolvers.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TableStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub header_region_cell_style: Option<String>,
    pub body_region_cell_style: Option<String>,
    pub footer_region_cell_style: Option<String>,
    pub left_column_region_cell_style: Option<String>,
    pub right_column_region_cell_style: Option<String>,
    pub top_border_stroke_color: Option<String>,
    pub top_border_stroke_weight: Option<f32>,
    pub bottom_border_stroke_color: Option<String>,
    pub bottom_border_stroke_weight: Option<f32>,
    pub left_border_stroke_color: Option<String>,
    pub left_border_stroke_weight: Option<f32>,
    pub right_border_stroke_color: Option<String>,
    pub right_border_stroke_weight: Option<f32>,
    /// `AlternatingFills` discriminator: `"None"` (default),
    /// `"AlternatingRows"`, or `"AlternatingColumns"`. Selects which
    /// axis the Start/End fill pattern paints along — InDesign reuses
    /// the same Start/End fill attributes for both axes and this
    /// attribute disambiguates. The renderer treats an absent /
    /// `"None"` value as "no alternating fill" even if a Start fill
    /// colour is present.
    pub alternating_fills: Option<String>,
    /// Alternating-row fill: every Nth body row from the top gets
    /// `start_row_fill_color`. `start_row_fill_count` is the
    /// number of consecutive rows that participate in the
    /// "starting" fill before alternating to the end-row fill.
    pub start_row_fill_color: Option<String>,
    pub start_row_fill_count: Option<u32>,
    pub start_row_fill_tint: Option<f32>,
    pub end_row_fill_color: Option<String>,
    pub end_row_fill_count: Option<u32>,
    pub end_row_fill_tint: Option<f32>,
    /// `SkipFirstAlternatingFillRows` / `SkipLastAlternatingFillRows`:
    /// body rows at the start / end of the table that the alternating
    /// pattern leaves unfilled. `None` ⇒ 0.
    pub skip_first_alternating_fill_rows: Option<u32>,
    pub skip_last_alternating_fill_rows: Option<u32>,
    /// Alternating-column fill: the column analogue of the row fields
    /// above. Paints column-by-column from the first body column when
    /// `alternating_fills == "AlternatingColumns"`.
    pub start_column_fill_color: Option<String>,
    pub start_column_fill_count: Option<u32>,
    pub start_column_fill_tint: Option<f32>,
    pub end_column_fill_color: Option<String>,
    pub end_column_fill_count: Option<u32>,
    pub end_column_fill_tint: Option<f32>,
    pub skip_first_alternating_fill_columns: Option<u32>,
    pub skip_last_alternating_fill_columns: Option<u32>,
}
/// Effective table-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedTable {
    pub header_region_cell_style: Option<String>,
    pub body_region_cell_style: Option<String>,
    pub footer_region_cell_style: Option<String>,
    pub left_column_region_cell_style: Option<String>,
    pub right_column_region_cell_style: Option<String>,
    pub top_border_stroke_color: Option<String>,
    pub top_border_stroke_weight: Option<f32>,
    pub bottom_border_stroke_color: Option<String>,
    pub bottom_border_stroke_weight: Option<f32>,
    pub left_border_stroke_color: Option<String>,
    pub left_border_stroke_weight: Option<f32>,
    pub right_border_stroke_color: Option<String>,
    pub right_border_stroke_weight: Option<f32>,
    pub alternating_fills: Option<String>,
    pub start_row_fill_color: Option<String>,
    pub start_row_fill_count: Option<u32>,
    pub start_row_fill_tint: Option<f32>,
    pub end_row_fill_color: Option<String>,
    pub end_row_fill_count: Option<u32>,
    pub end_row_fill_tint: Option<f32>,
    pub skip_first_alternating_fill_rows: Option<u32>,
    pub skip_last_alternating_fill_rows: Option<u32>,
    pub start_column_fill_color: Option<String>,
    pub start_column_fill_count: Option<u32>,
    pub start_column_fill_tint: Option<f32>,
    pub end_column_fill_color: Option<String>,
    pub end_column_fill_count: Option<u32>,
    pub end_column_fill_tint: Option<f32>,
    pub skip_first_alternating_fill_columns: Option<u32>,
    pub skip_last_alternating_fill_columns: Option<u32>,
}
/// Effective cell-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedCell {
    pub fill_color: Option<String>,
    pub vertical_justification: Option<String>,
    pub rotation_angle: Option<f32>,
    pub top_edge_stroke_color: Option<String>,
    pub top_edge_stroke_weight: Option<f32>,
    pub bottom_edge_stroke_color: Option<String>,
    pub bottom_edge_stroke_weight: Option<f32>,
    pub left_edge_stroke_color: Option<String>,
    pub left_edge_stroke_weight: Option<f32>,
    pub right_edge_stroke_color: Option<String>,
    pub right_edge_stroke_weight: Option<f32>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CharacterStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    /// `FillTint` — see `CharacterRun::fill_tint` for semantics.
    pub fill_tint: Option<f32>,
    /// `StrokeColor` declared on the `<CharacterStyle>`. Cascades
    /// through `BasedOn` like every other field. `Swatch/None` is
    /// normalised to `None` at parse time so a cascade can fall
    /// through to a real colour from the base style.
    pub stroke_color: Option<String>,
    /// `StrokeWeight` declared on the `<CharacterStyle>` in pt.
    pub stroke_weight: Option<f32>,
    pub capitalization: Option<String>,
    pub baseline_shift: Option<f32>,
    pub horizontal_scale: Option<f32>,
    pub vertical_scale: Option<f32>,
    pub skew: Option<f32>,
    pub position: Option<String>,
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// `OverprintFill="true"` declared on the `<CharacterStyle>`.
    /// Cascades through `BasedOn` like every other field. None ⇒
    /// inherit; bottom of cascade = false (IDML's default).
    pub overprint_fill: Option<bool>,
    /// `OverprintStroke="true"` analogue. Currently rare on text
    /// runs (only outlined text carries a stroke) but parsed for
    /// completeness.
    pub overprint_stroke: Option<bool>,
    /// `RubyFlag` — when `true`, this character style carries ruby
    /// annotation. See [`CharacterRun::ruby_flag`]. Parser-only;
    /// renderer integration is queued under Tier 4 — CJK Stage 4.
    pub ruby_flag: Option<bool>,
    /// `RubyType` — `PerCharacter` / `GroupRuby`. See
    /// [`CharacterRun::ruby_type`].
    pub ruby_type: Option<String>,
    /// `RubyString` — the ruby annotation text. See
    /// [`CharacterRun::ruby_string`].
    pub ruby_string: Option<String>,
    /// `KentenKind` — emphasis-mark glyph. See
    /// [`CharacterRun::kenten_kind`].
    pub kenten_kind: Option<String>,
    /// `KentenCharacter` — custom emphasis-mark codepoint when
    /// `kenten_kind == "Custom"`.
    pub kenten_character: Option<String>,
    /// `KentenFontSize` — emphasis-mark size as a % of base size.
    pub kenten_font_size: Option<f32>,
    /// Phase 4 typography — IDML `Ligatures="true|false"`. Standard +
    /// contextual OpenType ligatures (`liga`, `clig`). Default (when
    /// None and bottom of cascade) is `true`, matching InDesign's
    /// CharacterStyle default.
    pub ligatures_on: Option<bool>,
    /// IDML `KerningMethod="Metrics|Optical|None"`. Default
    /// (when None and bottom of cascade) is `Metrics`. `Optical`
    /// falls back to `Metrics` at the renderer until the outline-
    /// driven pass lands.
    pub kerning_method: Option<String>,
    /// Discrete OpenType feature toggles (`OTFFraction`, `OTFOrdinal`,
    /// `OTFSwash`, `OTFDiscretionaryLigature`, `OTFFigureStyle`,
    /// `OTFStylisticSets`, …) declared on the `<CharacterStyle>`.
    /// Cascades through `BasedOn` per-field. See
    /// [`OtfFeatures`].
    pub otf: OtfFeatures,
}
/// Q-09: `ParagraphShading*` attributes parsed off a
/// `<ParagraphStyle>` or `<ParagraphStyleRange>`. The renderer emits
/// a coloured rectangle behind each line of the paragraph when `on`
/// is true. `None` for any field means "not set at this level" so the
/// cascade can inherit from `BasedOn`. The decorative per-corner
/// options + radii live alongside the bag in case a future cycle
/// renders rounded shading bands.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParagraphShading {
    pub on: Option<bool>,
    pub color: Option<String>,
    pub tint: Option<f32>,
    /// `ColumnWidth` | `TextWidth`. None ⇒ ColumnWidth default.
    pub width: Option<String>,
    /// Inset offsets in pt, order `[top, left, bottom, right]`.
    pub offset_top: Option<f32>,
    pub offset_left: Option<f32>,
    pub offset_bottom: Option<f32>,
    pub offset_right: Option<f32>,
    /// `AscentTopOrigin` | `BaselineTopOrigin` | etc. Drives the
    /// shading band's top edge: `None` ⇒ AscentTopOrigin default.
    pub top_origin: Option<String>,
    /// `DescentBottomOrigin` | `BaselineBottomOrigin` | etc.
    pub bottom_origin: Option<String>,
    pub clip_to_frame: Option<bool>,
    pub overprint: Option<bool>,
    pub suppress_printing: Option<bool>,
}
/// Q-09: `RuleAbove*` / `RuleBelow*` rule-line parameters parsed
/// off a `<ParagraphStyle>` or `<ParagraphStyleRange>`. The renderer
/// strokes a horizontal line above the first line (RuleAbove) or
/// below the last line (RuleBelow) of the paragraph when `on` is
/// true. Only the fields actually consumed by the renderer are
/// listed; gap / stroke-style / overprint variants are queued.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParagraphRule {
    pub on: Option<bool>,
    pub color: Option<String>,
    pub tint: Option<f32>,
    /// Stroke weight in pt.
    pub weight: Option<f32>,
    /// Distance from the paragraph's baseline (RuleAbove) or
    /// descent (RuleBelow) to the rule.
    pub offset: Option<f32>,
    pub left_indent: Option<f32>,
    pub right_indent: Option<f32>,
    /// `ColumnWidth` | `TextWidth`. None ⇒ ColumnWidth default.
    pub width: Option<String>,
}
/// Q-09: `ParagraphBorder*` attributes parsed off a `<ParagraphStyle>`
/// or `<ParagraphStyleRange>`. The renderer strokes a rectangular
/// border around the paragraph's content box when `on` is true.
/// Per-corner `CornerOption` / `CornerRadius` attrs are honoured via
/// `corners` (Track 4d) — order matches `Rectangle::corners`:
/// `[top_left, top_right, bottom_right, bottom_left]`.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParagraphBorder {
    pub on: Option<bool>,
    pub color: Option<String>,
    pub tint: Option<f32>,
    /// Stroke weight in pt.
    pub weight: Option<f32>,
    /// Inset offsets in pt.
    pub offset_top: Option<f32>,
    pub offset_left: Option<f32>,
    pub offset_bottom: Option<f32>,
    pub offset_right: Option<f32>,
    /// `ColumnWidth` | `TextWidth`. None ⇒ ColumnWidth default.
    pub width: Option<String>,
    /// Per-corner option/radius overrides. `[tl, tr, br, bl]`.
    pub corners: [CornerSpec; 4],
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ParagraphStyleDef {
    pub self_id: String,
    pub name: Option<String>,
    pub based_on: Option<String>,
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    /// `FillTint` — see `CharacterRun::fill_tint` for semantics.
    pub fill_tint: Option<f32>,
    /// `StrokeColor` declared on the `<ParagraphStyle>` — the paint
    /// used to outline glyphs whose run / character style don't
    /// override it. `Swatch/None` normalises to `None`.
    pub stroke_color: Option<String>,
    /// `StrokeWeight` declared on the `<ParagraphStyle>` in pt.
    pub stroke_weight: Option<f32>,
    pub capitalization: Option<String>,
    pub baseline_shift: Option<f32>,
    pub horizontal_scale: Option<f32>,
    pub vertical_scale: Option<f32>,
    pub skew: Option<f32>,
    pub position: Option<String>,
    pub tracking: Option<f32>,
    /// `Justification` from the style. Parsed into the typed
    /// `Justification` enum at XML-read time.
    pub justification: Option<Justification>,
    pub first_line_indent: Option<f32>,
    /// `LeftIndent` / `RightIndent` in pt — the paragraph's left/right
    /// margin offsets. Narrow the composed column and shift the body
    /// (FINDING #7.2). `None` ⇒ inherit through the cascade.
    pub left_indent: Option<f32>,
    pub right_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// `<TabList>` parsed from the style. Empty means "no
    /// declaration" — the cascade may inherit from `BasedOn`.
    pub tab_list: Vec<TabStop>,
    /// `BulletsAndNumberingListType`: `BulletList` /
    /// `NumberedList` / `NoList`. `None` when absent.
    pub bullets_list_type: Option<String>,
    /// `<BulletChar BulletCharacterValue="...">` — Unicode code
    /// point of the bullet glyph. None when no bullet declared.
    pub bullet_character: Option<u32>,
    /// `BulletsTextAfter` — string rendered between the bullet
    /// and the paragraph text (typically a tab `^t` or a space).
    /// IDML serialises tabs as the literal `^t` sequence.
    pub bullets_text_after: Option<String>,
    /// `NumberingFormat` for `NumberedList` paragraphs. IDML
    /// serialises these as the literal sample string, e.g.
    /// `"1, 2, 3, 4..."`, `"I, II, III, IV..."`,
    /// `"01, 02, 03, 04..."`, `"A, B, C, D..."`. The renderer
    /// reads only the prefix before the first comma to decide
    /// the format. `None` falls back to Arabic.
    pub numbering_format: Option<String>,
    /// `BulletsCharacterStyle` — a `CharacterStyle/<id>` reference
    /// that styles the bullet marker (font, size, colour) independently
    /// of the paragraph text. IDML applies this only to `BulletList`
    /// paragraphs. `None` ⇒ the bullet inherits the first run's
    /// formatting (the historical fallback).
    pub bullets_character_style: Option<String>,
    /// `BulletsAndNumberingDigitsCharacterStyle` — a `CharacterStyle/<id>`
    /// reference that styles the digits of a `NumberedList` paragraph's
    /// marker. IDML overloads this same field as the bullet-style
    /// reference for `BulletList` paragraphs when
    /// `bullets_character_style` is absent (the InDesign UI presents
    /// one "Character Style" picker regardless of list kind), so the
    /// renderer falls back to it when shaping bullets.
    pub bullets_and_numbering_digits_character_style: Option<String>,
    /// `NumberingExpression` — the formatting template for the
    /// numbered-list marker. Tokens:
    /// - `^#` substitutes the formatted counter (per
    ///   `numbering_format`),
    /// - `^.` is a literal period,
    /// - `^t` is a literal tab.
    ///
    /// Anything else passes through unchanged. `None` falls back
    /// to the IDML default `^#.^t` (e.g. `"1.\t"`).
    pub numbering_expression: Option<String>,
    /// `NumberingStartAt` — explicit integer the paragraph's
    /// counter starts at. Overrides any continued count from a
    /// previous paragraph. `None` means "no explicit start"; the
    /// counter increments off whatever the story carries.
    pub numbering_start_at: Option<i32>,
    /// `NumberingContinue` — when `true`, the counter persists
    /// across the previous paragraph (even if that paragraph
    /// applied a different style or wasn't a numbered list at all,
    /// up to whatever the previous numbered-list state was). When
    /// `false`, the counter resets at the start of this paragraph.
    /// `None` ⇒ inherit; the renderer's default is "continue".
    pub numbering_continue: Option<bool>,
    /// W1.22 — `AppliedNumberingList="NumberingList/<id>"`. Binds the
    /// paragraph (via the style cascade) to a named `<NumberingList>`
    /// resource. The renderer reads the list's
    /// `ContinueNumbersAcrossStories` flag off this reference to
    /// decide cross-story numbering continuity. `None` ⇒ no named
    /// list (the paragraph still numbers, but the counter is scoped
    /// per story as before). IDML's literal "no list" sentinel
    /// `n` / `NumberingList/$ID/[No numbering list]` normalises to
    /// `None`.
    pub applied_numbering_list: Option<String>,
    /// styles.next-style — `NextStyle="ParagraphStyle/<id>"`. The
    /// style InDesign applies to the FOLLOWING paragraph when the
    /// user presses Enter at this paragraph's end (the "Next Style"
    /// field in the paragraph-style options dialog). The renderer
    /// does not act on this — it is a typing-time editor behaviour —
    /// but the data is surfaced so the editor can implement the flow.
    /// `None` ⇒ no chaining (InDesign defaults this to "[Same style]"
    /// which serialises as the style's own self id; that self-loop is
    /// preserved verbatim, the editor reads it as "stay").
    pub next_style: Option<String>,
    /// `Hyphenation` boolean. IDML default is true; the resolver
    /// only flips a paragraph off when an explicit `Hyphenation="false"`
    /// lands on the cascade. Drives whether the composer registers a
    /// language-specific hyphenator with the breaker.
    pub hyphenation: Option<bool>,
    /// `HyphenationZone` in pt. InDesign's "hyphenation zone" is the
    /// width of whitespace allowed at the end of a line before a word
    /// is broken: a word becomes hyphenation-eligible only when it
    /// would otherwise start within `zone` of the right margin (i.e.
    /// the gap before it exceeds the zone). Larger zones ⇒ fewer
    /// hyphens (more raggedness tolerated); `0` ⇒ no zone restriction
    /// (the breaker may hyphenate anywhere). Only consulted for
    /// left-aligned / ragged paragraphs in InDesign; `None` ⇒ inherit.
    pub hyphenation_zone: Option<f32>,
    /// `AppliedLanguage` reference (e.g. `$ID/English: USA`). Used to
    /// pick the hyphenation dictionary; unrecognised values fall back
    /// to English-US so we always have *some* dictionary loaded.
    pub applied_language: Option<String>,
    /// `MinimumWordSpacing` percentage (`80` = 80% of normal). Drives
    /// the composer's shrink ratio.
    pub minimum_word_spacing: Option<f32>,
    /// `DesiredWordSpacing` percentage (`100` = 100% of normal). The
    /// renderer scales `Min`/`Max` against this so the composer's
    /// ratios stay relative to the desired baseline.
    pub desired_word_spacing: Option<f32>,
    /// `MaximumWordSpacing` percentage (`133` = 133% of normal).
    /// Drives the composer's stretch ratio.
    pub maximum_word_spacing: Option<f32>,
    /// Q-20: `MinimumLetterSpacing` pt (additive, signed). Allows
    /// the composer to tighten inter-glyph advance up to this much
    /// when justifying lines.
    pub minimum_letter_spacing: Option<f32>,
    /// Q-20: `DesiredLetterSpacing` pt (default 0 = none).
    pub desired_letter_spacing: Option<f32>,
    /// Q-20: `MaximumLetterSpacing` pt (additive, signed).
    pub maximum_letter_spacing: Option<f32>,
    /// Q-20: `MinimumGlyphScaling` percent (default 100 = identity).
    /// Allows per-glyph x-advance scaling for justification.
    pub minimum_glyph_scaling: Option<f32>,
    /// Q-20: `DesiredGlyphScaling` percent.
    pub desired_glyph_scaling: Option<f32>,
    /// Q-20: `MaximumGlyphScaling` percent.
    pub maximum_glyph_scaling: Option<f32>,
    /// `DropCapCharacters` count. 0 / `None` ⇒ no drop cap.
    pub drop_cap_characters: Option<u32>,
    /// `DropCapLines` — vertical extent of the drop cap.
    pub drop_cap_lines: Option<u32>,
    /// `DropCapDetail` — InDesign's scaling-factor integer.
    pub drop_cap_detail: Option<i32>,
    /// `OverprintFill="true"` declared on the `<ParagraphStyle>`. See
    /// [`CharacterStyleDef::overprint_fill`]. Cascades like every other
    /// paragraph attribute via `merge_below`.
    pub overprint_fill: Option<bool>,
    /// `OverprintStroke="true"` analogue.
    pub overprint_stroke: Option<bool>,
    /// `KinsokuSet="KinsokuTable/$ID/PhotoshopKinsokuHard"` ref on the
    /// `<ParagraphStyle>`. Cascades like every other paragraph attribute.
    /// See [`Paragraph::kinsoku_set`].
    pub kinsoku_set: Option<String>,
    /// `KinsokuType` flavour. See [`Paragraph::kinsoku_type`].
    pub kinsoku_type: Option<String>,
    /// `MojikumiTable` ref. See [`Paragraph::mojikumi_table`].
    pub mojikumi_table: Option<String>,
    /// `MojikumiSet` (older IDML attribute name; see
    /// [`Paragraph::mojikumi_set`]).
    pub mojikumi_set: Option<String>,
    /// Q-09: paragraph-level shading band parameters. `on` defaulting
    /// to `None` means "not declared at this style level" so the
    /// `BasedOn` cascade can inherit. Renderer emit module is a
    /// separate follow-up.
    pub shading: ParagraphShading,
    /// Q-09: horizontal rule above the first line of the paragraph.
    pub rule_above: ParagraphRule,
    /// Q-09: horizontal rule below the last line of the paragraph.
    pub rule_below: ParagraphRule,
    /// Q-09: rectangular border around the paragraph's content box.
    pub border: ParagraphBorder,
    /// Phase 4 typography — nested character styles applied to the
    /// paragraph's leading characters. Each entry restyles a prefix
    /// range; successive entries chain (the previous entry's end is
    /// the next entry's start). Empty when the IDML declares no
    /// `<NestedStyle>` children. Always replaces (no cascade merge)
    /// because the IDML serialiser writes the full list per style.
    pub nested_styles: Vec<NestedStyle>,
}
/// IDML `<NestedStyle>` — a CharacterStyle applied to a leading
/// portion of a paragraph, bounded by a delimiter (count of
/// words / sentences / characters, a literal char, or a special
/// "any digit / letter / quote" matcher).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct NestedStyle {
    /// `AppliedCharacterStyle="CharacterStyle/<id>"`. The named style
    /// applies to the entry's range. Resolved by the renderer
    /// against `Styles::character_styles`.
    pub applied_character_style: String,
    /// `Delimiter` — what marks the boundary. See [`NestedDelimiter`].
    pub delimiter: NestedDelimiter,
    /// `Repetition` — how many of the delimiter unit this range
    /// covers. Default 1. Negative / zero ⇒ no application.
    pub repetition: i32,
    /// `Inclusive` — when true the delimiter character itself sits
    /// inside the styled range; when false the range ends just
    /// before it. InDesign default: true.
    pub inclusive: bool,
}
/// What delimits the end of a `<NestedStyle>` range.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub enum NestedDelimiter {
    /// `Words` — N whitespace-delimited words.
    Words,
    /// `Sentences` — N sentences (terminated by `.!?`).
    Sentences,
    /// `Characters` — N source characters.
    Characters,
    /// `AnyDigit` — N digit characters.
    AnyDigit,
    /// `AnyLetter` — N letter characters (Unicode `is_alphabetic`).
    AnyLetter,
    /// `AnyDoubleQuotes` — N occurrences of `"`, U+201C, U+201D.
    AnyDoubleQuotes,
    /// `AnySingleQuotes` — N occurrences of `'`, U+2018, U+2019.
    AnySingleQuotes,
    /// `Tab` — N tab characters (`\t`).
    Tab,
    /// `ForcedLineBreak` — N forced line breaks (rare in paragraph
    /// styles; mirrors IDML's enumerated value).
    ForcedLineBreak,
    /// `EndNestedStyle` — InDesign's "End Nested Style Here" marker
    /// (U+0003). Often inserted manually in the source text.
    EndNestedStyle,
    /// Literal character delimiter, e.g. `:` or `;` from an
    /// `Delimiter="ANY_CHARACTER"` + explicit char on the style.
    Char(char),
    /// Fallback for unsupported / unparseable delimiter values —
    /// the nested style entry is effectively a no-op (matches
    /// nothing).
    #[default]
    Unknown,
}
/// Effective character-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedCharacter {
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub fill_tint: Option<f32>,
    /// Cascaded text-stroke colour. See
    /// [`CharacterStyleDef::stroke_color`].
    pub stroke_color: Option<String>,
    /// Cascaded text-stroke weight in pt.
    pub stroke_weight: Option<f32>,
    pub capitalization: Option<String>,
    pub baseline_shift: Option<f32>,
    pub horizontal_scale: Option<f32>,
    pub vertical_scale: Option<f32>,
    pub skew: Option<f32>,
    pub position: Option<String>,
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// Cascaded `OverprintFill` flag. See
    /// [`CharacterStyleDef::overprint_fill`]. None at the bottom of
    /// the cascade ⇒ false (the IDML default).
    pub overprint_fill: Option<bool>,
    /// Cascaded `OverprintStroke` flag.
    pub overprint_stroke: Option<bool>,
    /// Cascaded `RubyFlag`. See [`CharacterStyleDef::ruby_flag`].
    pub ruby_flag: Option<bool>,
    /// Cascaded `RubyType`.
    pub ruby_type: Option<String>,
    /// Cascaded `RubyString`.
    pub ruby_string: Option<String>,
    /// Cascaded `KentenKind`.
    pub kenten_kind: Option<String>,
    /// Cascaded `KentenCharacter`.
    pub kenten_character: Option<String>,
    /// Cascaded `KentenFontSize`.
    pub kenten_font_size: Option<f32>,
    /// Phase 4 typography — cascaded `Ligatures` flag. See
    /// [`CharacterStyleDef::ligatures_on`].
    pub ligatures_on: Option<bool>,
    /// Cascaded `KerningMethod` string. See
    /// [`CharacterStyleDef::kerning_method`].
    pub kerning_method: Option<String>,
    /// Cascaded discrete OpenType feature toggles. See
    /// [`CharacterStyleDef::otf`].
    pub otf: OtfFeatures,
}
/// Effective paragraph-level attributes after walking BasedOn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedParagraph {
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub fill_tint: Option<f32>,
    /// Cascaded text-stroke colour. See
    /// [`ParagraphStyleDef::stroke_color`].
    pub stroke_color: Option<String>,
    /// Cascaded text-stroke weight in pt.
    pub stroke_weight: Option<f32>,
    pub capitalization: Option<String>,
    pub baseline_shift: Option<f32>,
    pub horizontal_scale: Option<f32>,
    pub vertical_scale: Option<f32>,
    pub skew: Option<f32>,
    pub position: Option<String>,
    pub tracking: Option<f32>,
    pub justification: Option<Justification>,
    pub first_line_indent: Option<f32>,
    /// `LeftIndent` / `RightIndent` in pt (FINDING #7.2) — the
    /// paragraph's left/right margin offsets resolved through the
    /// cascade. `None` ⇒ no indent.
    pub left_indent: Option<f32>,
    pub right_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// `<TabList>` from the cascade. Empty means inherited / none.
    pub tab_list: Vec<TabStop>,
    pub bullets_list_type: Option<String>,
    pub bullet_character: Option<u32>,
    pub bullets_text_after: Option<String>,
    pub numbering_format: Option<String>,
    /// Cascaded `BulletsCharacterStyle` ref. See
    /// [`ParagraphStyleDef::bullets_character_style`].
    pub bullets_character_style: Option<String>,
    /// Cascaded `BulletsAndNumberingDigitsCharacterStyle` ref. See
    /// [`ParagraphStyleDef::bullets_and_numbering_digits_character_style`].
    pub bullets_and_numbering_digits_character_style: Option<String>,
    /// `NumberingExpression` template (`^#`, `^.`, `^t` tokens
    /// plus literal characters). `None` ⇒ renderer default `^#.^t`.
    pub numbering_expression: Option<String>,
    /// `NumberingStartAt` explicit start integer. See
    /// `ParagraphStyleDef::numbering_start_at`.
    pub numbering_start_at: Option<i32>,
    /// `NumberingContinue` flag. See
    /// `ParagraphStyleDef::numbering_continue`.
    pub numbering_continue: Option<bool>,
    /// W1.22 — cascaded `AppliedNumberingList` ref. See
    /// [`ParagraphStyleDef::applied_numbering_list`].
    pub applied_numbering_list: Option<String>,
    /// styles.next-style — cascaded `NextStyle` ref. See
    /// [`ParagraphStyleDef::next_style`].
    pub next_style: Option<String>,
    pub hyphenation: Option<bool>,
    /// Cascaded `HyphenationZone` in pt. See
    /// [`ParagraphStyleDef::hyphenation_zone`].
    pub hyphenation_zone: Option<f32>,
    pub applied_language: Option<String>,
    pub minimum_word_spacing: Option<f32>,
    pub desired_word_spacing: Option<f32>,
    pub maximum_word_spacing: Option<f32>,
    /// Q-20: cascaded letter / glyph spacing knobs.
    pub minimum_letter_spacing: Option<f32>,
    pub desired_letter_spacing: Option<f32>,
    pub maximum_letter_spacing: Option<f32>,
    pub minimum_glyph_scaling: Option<f32>,
    pub desired_glyph_scaling: Option<f32>,
    pub maximum_glyph_scaling: Option<f32>,
    /// `DropCapCharacters` count (number of leading characters that
    /// drop down across `drop_cap_lines` lines). 0 / `None` ⇒ no
    /// drop cap.
    pub drop_cap_characters: Option<u32>,
    /// `DropCapLines` count (lines the drop cap spans). 0 / `None` ⇒
    /// no drop cap.
    pub drop_cap_lines: Option<u32>,
    /// `DropCapDetail` (the IDML scaling factor InDesign records on
    /// the drop cap's character formatting; an arbitrary integer).
    pub drop_cap_detail: Option<i32>,
    /// Cascaded `OverprintFill` flag from the paragraph style chain.
    /// See [`CharacterStyleDef::overprint_fill`].
    pub overprint_fill: Option<bool>,
    /// Cascaded `OverprintStroke` flag.
    pub overprint_stroke: Option<bool>,
    /// Cascaded `KinsokuSet` ref. See [`Paragraph::kinsoku_set`].
    pub kinsoku_set: Option<String>,
    /// Cascaded `KinsokuType` flavour.
    pub kinsoku_type: Option<String>,
    /// Cascaded `MojikumiTable` ref.
    pub mojikumi_table: Option<String>,
    /// Cascaded `MojikumiSet` ref.
    pub mojikumi_set: Option<String>,
    /// Q-09: cascaded paragraph shading. Each field falls through
    /// `BasedOn` only when unset at higher levels.
    pub shading: ParagraphShading,
    pub rule_above: ParagraphRule,
    pub rule_below: ParagraphRule,
    pub border: ParagraphBorder,
    /// Phase 4 typography — cascaded `<NestedStyle>` entries.
    /// Replaces rather than merges (the IDML serialiser writes the
    /// full list per ParagraphStyle).
    pub nested_styles: Vec<NestedStyle>,
}
impl ResolvedObject {
    pub fn merge_below(&mut self, def: &ObjectStyleDef) {
        if self.fill_color.is_none() {
            self.fill_color = def.fill_color.clone();
        }
        self.fill_tint = self.fill_tint.or(def.fill_tint);
        if self.stroke_color.is_none() {
            self.stroke_color = def.stroke_color.clone();
        }
        self.stroke_tint = self.stroke_tint.or(def.stroke_tint);
        self.stroke_weight = self.stroke_weight.or(def.stroke_weight);
        self.corner_radius = self.corner_radius.or(def.corner_radius);
        if self.corner_option.is_none() {
            self.corner_option = def.corner_option.clone();
        }
    }
}
impl ResolvedTable {
    pub fn merge_below(&mut self, def: &TableStyleDef) {
        macro_rules! merge_str {
            ($field:ident) => {
                if self.$field.is_none() {
                    self.$field = def.$field.clone();
                }
            };
        }
        merge_str!(header_region_cell_style);
        merge_str!(body_region_cell_style);
        merge_str!(footer_region_cell_style);
        merge_str!(left_column_region_cell_style);
        merge_str!(right_column_region_cell_style);
        merge_str!(top_border_stroke_color);
        merge_str!(bottom_border_stroke_color);
        merge_str!(left_border_stroke_color);
        merge_str!(right_border_stroke_color);
        merge_str!(alternating_fills);
        merge_str!(start_row_fill_color);
        merge_str!(end_row_fill_color);
        merge_str!(start_column_fill_color);
        merge_str!(end_column_fill_color);
        self.top_border_stroke_weight = self
            .top_border_stroke_weight
            .or(def.top_border_stroke_weight);
        self.bottom_border_stroke_weight = self
            .bottom_border_stroke_weight
            .or(def.bottom_border_stroke_weight);
        self.left_border_stroke_weight = self
            .left_border_stroke_weight
            .or(def.left_border_stroke_weight);
        self.right_border_stroke_weight = self
            .right_border_stroke_weight
            .or(def.right_border_stroke_weight);
        self.start_row_fill_count = self.start_row_fill_count.or(def.start_row_fill_count);
        self.start_row_fill_tint = self.start_row_fill_tint.or(def.start_row_fill_tint);
        self.end_row_fill_count = self.end_row_fill_count.or(def.end_row_fill_count);
        self.end_row_fill_tint = self.end_row_fill_tint.or(def.end_row_fill_tint);
        self.skip_first_alternating_fill_rows = self
            .skip_first_alternating_fill_rows
            .or(def.skip_first_alternating_fill_rows);
        self.skip_last_alternating_fill_rows = self
            .skip_last_alternating_fill_rows
            .or(def.skip_last_alternating_fill_rows);
        self.start_column_fill_count = self.start_column_fill_count.or(def.start_column_fill_count);
        self.start_column_fill_tint = self.start_column_fill_tint.or(def.start_column_fill_tint);
        self.end_column_fill_count = self.end_column_fill_count.or(def.end_column_fill_count);
        self.end_column_fill_tint = self.end_column_fill_tint.or(def.end_column_fill_tint);
        self.skip_first_alternating_fill_columns = self
            .skip_first_alternating_fill_columns
            .or(def.skip_first_alternating_fill_columns);
        self.skip_last_alternating_fill_columns = self
            .skip_last_alternating_fill_columns
            .or(def.skip_last_alternating_fill_columns);
    }
}
impl ResolvedCell {
    pub fn merge_below(&mut self, def: &CellStyleDef) {
        if self.fill_color.is_none() {
            self.fill_color = def.fill_color.clone();
        }
        if self.vertical_justification.is_none() {
            self.vertical_justification = def.vertical_justification.clone();
        }
        self.rotation_angle = self.rotation_angle.or(def.rotation_angle);
        if self.top_edge_stroke_color.is_none() {
            self.top_edge_stroke_color = def.top_edge_stroke_color.clone();
        }
        self.top_edge_stroke_weight = self.top_edge_stroke_weight.or(def.top_edge_stroke_weight);
        if self.bottom_edge_stroke_color.is_none() {
            self.bottom_edge_stroke_color = def.bottom_edge_stroke_color.clone();
        }
        self.bottom_edge_stroke_weight = self
            .bottom_edge_stroke_weight
            .or(def.bottom_edge_stroke_weight);
        if self.left_edge_stroke_color.is_none() {
            self.left_edge_stroke_color = def.left_edge_stroke_color.clone();
        }
        self.left_edge_stroke_weight = self.left_edge_stroke_weight.or(def.left_edge_stroke_weight);
        if self.right_edge_stroke_color.is_none() {
            self.right_edge_stroke_color = def.right_edge_stroke_color.clone();
        }
        self.right_edge_stroke_weight = self
            .right_edge_stroke_weight
            .or(def.right_edge_stroke_weight);
    }
}
impl ResolvedCharacter {
    /// Fill any unset (`None`) field from `def`. Cascade convention:
    /// already-set fields on `self` win; `def` only patches gaps.
    pub fn merge_below(&mut self, def: &CharacterStyleDef) {
        if self.font.is_none() {
            self.font = def.font.clone();
        }
        if self.font_style.is_none() {
            self.font_style = def.font_style.clone();
        }
        self.point_size = self.point_size.or(def.point_size);
        if self.fill_color.is_none() {
            self.fill_color = def.fill_color.clone();
        }
        self.fill_tint = self.fill_tint.or(def.fill_tint);
        if self.stroke_color.is_none() {
            self.stroke_color = def.stroke_color.clone();
        }
        self.stroke_weight = self.stroke_weight.or(def.stroke_weight);
        if self.capitalization.is_none() {
            self.capitalization = def.capitalization.clone();
        }
        self.baseline_shift = self.baseline_shift.or(def.baseline_shift);
        self.horizontal_scale = self.horizontal_scale.or(def.horizontal_scale);
        self.vertical_scale = self.vertical_scale.or(def.vertical_scale);
        self.skew = self.skew.or(def.skew);
        if self.position.is_none() {
            self.position = def.position.clone();
        }
        self.tracking = self.tracking.or(def.tracking);
        self.underline = self.underline.or(def.underline);
        self.strikethru = self.strikethru.or(def.strikethru);
        self.overprint_fill = self.overprint_fill.or(def.overprint_fill);
        self.overprint_stroke = self.overprint_stroke.or(def.overprint_stroke);
        self.ruby_flag = self.ruby_flag.or(def.ruby_flag);
        if self.ruby_type.is_none() {
            self.ruby_type = def.ruby_type.clone();
        }
        if self.ruby_string.is_none() {
            self.ruby_string = def.ruby_string.clone();
        }
        if self.kenten_kind.is_none() {
            self.kenten_kind = def.kenten_kind.clone();
        }
        if self.kenten_character.is_none() {
            self.kenten_character = def.kenten_character.clone();
        }
        self.kenten_font_size = self.kenten_font_size.or(def.kenten_font_size);
        self.ligatures_on = self.ligatures_on.or(def.ligatures_on);
        if self.kerning_method.is_none() {
            self.kerning_method = def.kerning_method.clone();
        }
        self.otf.merge_below(&def.otf);
    }
}
impl ResolvedParagraph {
    /// Fill any unset field from `def` (BasedOn cascade). For
    /// `tab_list` "unset" means empty — IDML has no
    /// distinction between "no tabs" and "tab list inherited".
    pub fn merge_below(&mut self, def: &ParagraphStyleDef) {
        if self.font.is_none() {
            self.font = def.font.clone();
        }
        if self.font_style.is_none() {
            self.font_style = def.font_style.clone();
        }
        self.point_size = self.point_size.or(def.point_size);
        if self.fill_color.is_none() {
            self.fill_color = def.fill_color.clone();
        }
        self.fill_tint = self.fill_tint.or(def.fill_tint);
        if self.stroke_color.is_none() {
            self.stroke_color = def.stroke_color.clone();
        }
        self.stroke_weight = self.stroke_weight.or(def.stroke_weight);
        if self.capitalization.is_none() {
            self.capitalization = def.capitalization.clone();
        }
        self.baseline_shift = self.baseline_shift.or(def.baseline_shift);
        self.horizontal_scale = self.horizontal_scale.or(def.horizontal_scale);
        self.vertical_scale = self.vertical_scale.or(def.vertical_scale);
        self.skew = self.skew.or(def.skew);
        if self.position.is_none() {
            self.position = def.position.clone();
        }
        self.tracking = self.tracking.or(def.tracking);
        self.justification = self.justification.or(def.justification);
        self.first_line_indent = self.first_line_indent.or(def.first_line_indent);
        self.left_indent = self.left_indent.or(def.left_indent);
        self.right_indent = self.right_indent.or(def.right_indent);
        self.space_before = self.space_before.or(def.space_before);
        self.space_after = self.space_after.or(def.space_after);
        self.underline = self.underline.or(def.underline);
        self.strikethru = self.strikethru.or(def.strikethru);
        if self.tab_list.is_empty() && !def.tab_list.is_empty() {
            self.tab_list = def.tab_list.clone();
        }
        if self.bullets_list_type.is_none() {
            self.bullets_list_type = def.bullets_list_type.clone();
        }
        self.bullet_character = self.bullet_character.or(def.bullet_character);
        if self.bullets_text_after.is_none() {
            self.bullets_text_after = def.bullets_text_after.clone();
        }
        if self.numbering_format.is_none() {
            self.numbering_format = def.numbering_format.clone();
        }
        if self.bullets_character_style.is_none() {
            self.bullets_character_style = def.bullets_character_style.clone();
        }
        if self.bullets_and_numbering_digits_character_style.is_none() {
            self.bullets_and_numbering_digits_character_style =
                def.bullets_and_numbering_digits_character_style.clone();
        }
        if self.numbering_expression.is_none() {
            self.numbering_expression = def.numbering_expression.clone();
        }
        self.numbering_start_at = self.numbering_start_at.or(def.numbering_start_at);
        self.numbering_continue = self.numbering_continue.or(def.numbering_continue);
        if self.applied_numbering_list.is_none() {
            self.applied_numbering_list = def.applied_numbering_list.clone();
        }
        if self.next_style.is_none() {
            self.next_style = def.next_style.clone();
        }
        self.hyphenation = self.hyphenation.or(def.hyphenation);
        self.hyphenation_zone = self.hyphenation_zone.or(def.hyphenation_zone);
        if self.applied_language.is_none() {
            self.applied_language = def.applied_language.clone();
        }
        self.minimum_word_spacing = self.minimum_word_spacing.or(def.minimum_word_spacing);
        self.desired_word_spacing = self.desired_word_spacing.or(def.desired_word_spacing);
        self.maximum_word_spacing = self.maximum_word_spacing.or(def.maximum_word_spacing);
        // Q-20: letter / glyph spacing per-field inheritance.
        self.minimum_letter_spacing = self.minimum_letter_spacing.or(def.minimum_letter_spacing);
        self.desired_letter_spacing = self.desired_letter_spacing.or(def.desired_letter_spacing);
        self.maximum_letter_spacing = self.maximum_letter_spacing.or(def.maximum_letter_spacing);
        self.minimum_glyph_scaling = self.minimum_glyph_scaling.or(def.minimum_glyph_scaling);
        self.desired_glyph_scaling = self.desired_glyph_scaling.or(def.desired_glyph_scaling);
        self.maximum_glyph_scaling = self.maximum_glyph_scaling.or(def.maximum_glyph_scaling);
        self.drop_cap_characters = self.drop_cap_characters.or(def.drop_cap_characters);
        self.drop_cap_lines = self.drop_cap_lines.or(def.drop_cap_lines);
        self.drop_cap_detail = self.drop_cap_detail.or(def.drop_cap_detail);
        self.overprint_fill = self.overprint_fill.or(def.overprint_fill);
        self.overprint_stroke = self.overprint_stroke.or(def.overprint_stroke);
        if self.kinsoku_set.is_none() {
            self.kinsoku_set = def.kinsoku_set.clone();
        }
        if self.kinsoku_type.is_none() {
            self.kinsoku_type = def.kinsoku_type.clone();
        }
        if self.mojikumi_table.is_none() {
            self.mojikumi_table = def.mojikumi_table.clone();
        }
        if self.mojikumi_set.is_none() {
            self.mojikumi_set = def.mojikumi_set.clone();
        }
        // Q-09: per-field shading inheritance. Each Option survives
        // the cascade independently so a child can override `tint`
        // without dragging in the parent's `width`, etc.
        let s = &mut self.shading;
        let p = &def.shading;
        s.on = s.on.or(p.on);
        if s.color.is_none() {
            s.color = p.color.clone();
        }
        s.tint = s.tint.or(p.tint);
        if s.width.is_none() {
            s.width = p.width.clone();
        }
        s.offset_top = s.offset_top.or(p.offset_top);
        s.offset_left = s.offset_left.or(p.offset_left);
        s.offset_bottom = s.offset_bottom.or(p.offset_bottom);
        s.offset_right = s.offset_right.or(p.offset_right);
        if s.top_origin.is_none() {
            s.top_origin = p.top_origin.clone();
        }
        if s.bottom_origin.is_none() {
            s.bottom_origin = p.bottom_origin.clone();
        }
        s.clip_to_frame = s.clip_to_frame.or(p.clip_to_frame);
        s.overprint = s.overprint.or(p.overprint);
        s.suppress_printing = s.suppress_printing.or(p.suppress_printing);
        // Q-09: per-field rule_above / rule_below inheritance.
        merge_rule(&mut self.rule_above, &def.rule_above);
        merge_rule(&mut self.rule_below, &def.rule_below);
        // Q-09: per-field border inheritance.
        merge_border(&mut self.border, &def.border);
        // Phase 4 — nested styles replace as a whole list. The IDML
        // serialiser writes the full list per style; cascade through
        // BasedOn only when the lower style has none of its own.
        if self.nested_styles.is_empty() && !def.nested_styles.is_empty() {
            self.nested_styles = def.nested_styles.clone();
        }
    }
}
fn merge_rule(child: &mut ParagraphRule, parent: &ParagraphRule) {
    child.on = child.on.or(parent.on);
    if child.color.is_none() {
        child.color = parent.color.clone();
    }
    child.tint = child.tint.or(parent.tint);
    child.weight = child.weight.or(parent.weight);
    child.offset = child.offset.or(parent.offset);
    child.left_indent = child.left_indent.or(parent.left_indent);
    child.right_indent = child.right_indent.or(parent.right_indent);
    if child.width.is_none() {
        child.width = parent.width.clone();
    }
}
fn merge_border(child: &mut ParagraphBorder, parent: &ParagraphBorder) {
    child.on = child.on.or(parent.on);
    if child.color.is_none() {
        child.color = parent.color.clone();
    }
    child.tint = child.tint.or(parent.tint);
    child.weight = child.weight.or(parent.weight);
    child.offset_top = child.offset_top.or(parent.offset_top);
    child.offset_left = child.offset_left.or(parent.offset_left);
    child.offset_bottom = child.offset_bottom.or(parent.offset_bottom);
    child.offset_right = child.offset_right.or(parent.offset_right);
    if child.width.is_none() {
        child.width = parent.width.clone();
    }
    for i in 0..4 {
        child.corners[i].option = child.corners[i].option.or(parent.corners[i].option);
        child.corners[i].radius = child.corners[i].radius.or(parent.corners[i].radius);
    }
}

// ---------------------------------------------------------------------------
// Story content value types — paragraphs, runs, tables (rows/cols/cells),
// footnotes, index markers, anchored objects, placeholder fields, writing
// direction (moved out of `paged-parse::story`; `Story` + `Story::parse` + all
// the `parse_*` free fns + the private parser-state structs stay in the parser).
// Paragraph's rule refs now resolve to the model-local ParagraphRule (N5).
// ---------------------------------------------------------------------------
/// IDML `StoryDirection` attribute — the writing-mode flag carried on
/// `<Story>`. The IDML default is `HorizontalWritingDirection` (left-
/// to-right body text). `VerticalWritingDirection` is the CJK vertical
/// mode where lines stack top-to-bottom and columns advance right-to-
/// left. The parser only captures this; the layout / renderer
/// integration is queued (see docs/plan.md Tier 4 — CJK Stage 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoryDirection {
    HorizontalWritingDirection,
    VerticalWritingDirection,
}
impl StoryDirection {
    pub fn from_idml(s: &str) -> Option<Self> {
        match s {
            "HorizontalWritingDirection" => Some(Self::HorizontalWritingDirection),
            "VerticalWritingDirection" => Some(Self::VerticalWritingDirection),
            _ => None,
        }
    }
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Paragraph {
    pub paragraph_style: Option<String>,
    /// `Justification` attribute from IDML, parsed into a typed
    /// `Justification` enum at XML-read time. Unknown attribute
    /// values become `None` (matches the pre-enum fallback in
    /// `map_justification`, which mapped anything it didn't
    /// recognise to `Left`).
    pub justification: Option<Justification>,
    /// `FirstLineIndent` in pt.
    pub first_line_indent: Option<f32>,
    /// `LeftIndent` in pt — the paragraph's left margin offset.
    /// `None` ⇒ inherit from the applied paragraph style cascade.
    /// W0.2: surfaced as a paragraph-scope mutate path; the renderer
    /// resolves indents through the style cascade today, so this
    /// per-paragraph override is parser+mutate only until the
    /// composer reads it off the instance.
    pub left_indent: Option<f32>,
    /// `RightIndent` in pt — the paragraph's right margin offset.
    /// `None` ⇒ inherit. W0.2: see [`Paragraph::left_indent`].
    pub right_indent: Option<f32>,
    /// `SpaceBefore` in pt.
    pub space_before: Option<f32>,
    /// `SpaceAfter` in pt.
    pub space_after: Option<f32>,
    /// `<TabList>` parsed from `<Properties>`. Empty when none is
    /// declared on this paragraph (the cascade fills in from the
    /// applied paragraph style if available).
    pub tab_list: Vec<TabStop>,
    /// `BulletsAndNumberingListType` local override —
    /// `BulletList`, `NumberedList`, or `NoList`. `None` ⇒ inherit
    /// from the applied paragraph style.
    pub bullets_list_type: Option<String>,
    /// Bullet glyph codepoint, parsed from
    /// `<Properties><BulletChar BulletCharacterValue="…"/></Properties>`.
    /// Acts as a local override of the cascaded paragraph style's
    /// bullet character.
    pub bullet_character: Option<u32>,
    /// `NumberingExpression` template for `NumberedList` paragraphs
    /// (e.g. `"^#.^t"`). W0.2: a local override surfaced by the
    /// mutate API. `None` ⇒ inherit from the cascade. The renderer
    /// reads the numbering expression off the resolved style today;
    /// this per-paragraph override is parser+mutate only until the
    /// composer reads it off the instance.
    pub numbering_format: Option<String>,
    /// W1.22 — `AppliedNumberingList="NumberingList/<id>"` local
    /// override on the `<ParagraphStyleRange>`. `None` ⇒ inherit the
    /// named list from the applied paragraph style cascade. The
    /// `ParagraphAppliedNumberingList` mutate path writes this; the
    /// renderer resolves it (instance-over-cascade) to find the
    /// list's cross-story continuity flag.
    pub applied_numbering_list: Option<String>,
    /// `DropCapCharacters` count from `<ParagraphStyleRange>`. 0 ⇒ no
    /// drop cap (the IDML default). Local override of the cascaded
    /// paragraph style's drop-cap settings.
    pub drop_cap_characters: u32,
    /// `DropCapLines` — vertical extent of the drop cap in lines.
    /// 0 ⇒ no drop cap.
    pub drop_cap_lines: u32,
    /// `DropCapDetail` — InDesign's per-paragraph side-bearing tweak
    /// for drop caps. `0` is the default. Stored signed because the
    /// IDML serialisation allows negative values.
    pub drop_cap_detail: i32,
    /// `Hyphenation` boolean override on the `<ParagraphStyleRange>`.
    /// `None` ⇒ inherit (IDML default at the bottom of the cascade is
    /// `true`). The composer keys hyphenation off the resolved style
    /// today; W0.2 surfaces this per-paragraph override via the
    /// mutate API.
    pub hyphenation: Option<bool>,
    /// `KeepLinesTogether` boolean — when `true`, InDesign tries to
    /// keep all lines of the paragraph in the same column / frame.
    /// `None` ⇒ inherit. Parser+mutate only (the frame-breaker does
    /// not yet honour keep options).
    pub keep_lines_together: Option<bool>,
    /// `KeepWithNext` — the number of lines of the *following*
    /// paragraph that InDesign keeps together with the end of this
    /// one (IDML serialises a line count, not a boolean). `None` ⇒
    /// inherit; `Some(0)` is the explicit "off". Parser+mutate only.
    pub keep_with_next: Option<u32>,
    /// `RuleAbove*` — the horizontal rule stroked above the first
    /// line when `on` is true. W0.2: surfaced as a whole-struct
    /// paragraph-scope mutate path. Defaults (all-`None`) mean "not
    /// declared on this paragraph"; the cascade fills it in. Mirrors
    /// the style-level [`ParagraphRule`].
    pub rule_above: ParagraphRule,
    /// `RuleBelow*` — the horizontal rule stroked below the last
    /// line. W0.2: see [`Paragraph::rule_above`].
    pub rule_below: ParagraphRule,
    pub runs: Vec<CharacterRun>,
    /// `KinsokuSet="KinsokuTable/$ID/PhotoshopKinsokuHard"` (or
    /// similar) reference. Identifies the set of CJK line-break
    /// characters InDesign should respect for this paragraph. `None`
    /// when absent. Parser-only today — the composer uses a built-in
    /// "Hard Kinsoku" set when `KinsokuType` triggers enforcement
    /// (see `paged_text::compose`).
    pub kinsoku_set: Option<String>,
    /// `KinsokuType` flavour controlling how the breaker reacts to a
    /// no-start / no-end violation:
    /// - `None` ⇒ no kinsoku enforcement
    /// - `WordbreakWithJustification` ⇒ allow line-end whitespace
    ///   stretch to absorb the violation
    /// - `PushIn` ⇒ pull the offending character back onto the
    ///   previous line (shrinks glue)
    /// - `PushOut` ⇒ push the offending character to the next line
    ///   (forces a break earlier)
    ///
    /// IDML default when absent: `None`. Parser captures the string
    /// verbatim; the composer keys "any value present" → "apply
    /// hard-kinsoku penalty" today, with finer flavour-specific
    /// resolution queued.
    pub kinsoku_type: Option<String>,
    /// `MojikumiTable="MojikumiTable/$ID/PhotoshopMojikumiSet4"` (or
    /// similar) reference. Drives the per-character-class inter-glyph
    /// spacing rules (e.g. shrink the space before an opening
    /// bracket if it follows a Hiragana). Parser-only — the renderer
    /// does not yet implement Mojikumi spacing adjustments. See
    /// docs/plan.md Tier 4 — CJK Stage 4.
    pub mojikumi_table: Option<String>,
    /// `MojikumiSet` — analogous to `MojikumiTable`, but the older
    /// IDML attribute name some exporters still emit.
    pub mojikumi_set: Option<String>,
    /// Anchored frames declared as a child of any
    /// `<CharacterStyleRange>` inside this paragraph (a `<TextFrame>`,
    /// `<Rectangle>`, or `<Group>` nested directly under a
    /// `<CharacterStyleRange>` is an inline-anchored object). The
    /// renderer's text-flow integration is queued; today these
    /// records carry the bounds + setting + a reference back to the
    /// hosted story (for anchored TextFrames) so the renderer can
    /// draw the frame at the anchor's baseline once the placement
    /// pass lands. The frame's full transparency / fill / stroke is
    /// intentionally NOT recursed into here — the parser punts on
    /// nested transparency / image links inside an anchored frame
    /// (trivial follow-up once the renderer needs it).
    pub anchored_frames: Vec<AnchoredFrame>,
    /// `<Table>` nested inside the paragraph's CharacterStyleRange.
    /// When present, the paragraph is rendered as a table at the
    /// current y_cursor; `runs` is typically empty for these.
    /// Tables can't currently nest inside tables — only one per
    /// paragraph.
    pub table: Option<Table>,
    /// `OverprintFill="true"` on the `<ParagraphStyleRange>`. None ⇒
    /// inherit from the applied paragraph style cascade. Stage 3
    /// honours this when a run inside the paragraph leaves its own
    /// overprint unset.
    pub overprint_fill: Option<bool>,
    /// `OverprintStroke="true"` analogue.
    pub overprint_stroke: Option<bool>,
    /// Phase 5 — `<Footnote>` elements anchored on this paragraph.
    /// Each footnote carries its own self-contained paragraph stream
    /// (the footnote body). The renderer's footnote placement pass
    /// reads this to populate the per-page footnote pool. Empty for
    /// the overwhelming majority of paragraphs (only the paragraphs
    /// that host a `<Footnote>` anchor have entries).
    pub footnotes: Vec<Footnote>,
    /// Phase 5 — `<Topic>` references on this paragraph from
    /// `<PageReference>` or `<IndexEntry>` markers. Each entry maps
    /// to one place where this paragraph contributes to the index.
    /// The renderer's index pass collects these across all
    /// paragraphs and emits an alphabetized index story.
    pub index_markers: Vec<IndexMarker>,
}
/// IDML `<Footnote>` — a self-contained paragraph stream anchored at
/// a point inside a host paragraph. The renderer places footnotes in
/// a per-page footnote pool at the bottom of the host frame.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Footnote {
    pub self_id: Option<String>,
    /// The footnote body, parsed identically to top-level story
    /// paragraphs. Inherits the host story's character / paragraph
    /// style cascade just like any other paragraph stream.
    pub paragraphs: Vec<Paragraph>,
}
/// IDML index marker — a `<PageReference>` or `<IndexEntry>` element
/// that records "this paragraph contributes to the index entry for
/// `topic_name`". The renderer's resolution pass collects all
/// markers, groups by topic, alphabetises, and emits an index story.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct IndexMarker {
    /// The indexed term. From the marker's `TopicName` attribute,
    /// or — when only `AppliedTopic="Topic/<id>"` is present — the
    /// resolver looks up the Topic table on the document and pulls
    /// the topic's `Name` from there.
    pub topic_name: String,
    /// `AppliedTopic` reference (`Topic/<id>`) when present. Empty
    /// when the marker carried only the inline `TopicName`.
    pub applied_topic: Option<String>,
    /// Optional sort override. IDML's `SortOrder` attribute.
    pub sort_order: Option<String>,
}
/// One anchored frame declared inside a `<CharacterStyleRange>`. The
/// frame carries its own geometry / transform and an
/// `<AnchoredObjectSetting>` describing where it should land relative
/// to the anchor character.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchoredFrame {
    pub frame_kind: AnchoredFrameKind,
    pub self_id: Option<String>,
    pub bounds: Option<Bounds>,
    pub item_transform: Option<[f32; 6]>,
    /// For anchored TextFrames: the `ParentStory` reference, so the
    /// renderer can chase the story content. `None` for Rectangles
    /// (which would carry an image link instead) and Groups (which
    /// hold sub-items).
    pub parent_story: Option<String>,
    pub setting: Option<AnchoredObjectSetting>,
    /// `FillColor` attribute on the frame's start tag. Mirrors the
    /// spread-level Rectangle / TextFrame parsing in
    /// `spread.rs::read_common_attrs`. `None` means inherit from the
    /// applied object style cascade.
    pub fill_color: Option<String>,
    /// `StrokeColor` attribute on the frame's start tag.
    pub stroke_color: Option<String>,
    /// `StrokeWeight` attribute, in points.
    pub stroke_weight: Option<f32>,
    /// `FillTint` percentage (0..=100).
    pub fill_tint: Option<f32>,
    /// `GradientFillAngle` in degrees.
    pub gradient_fill_angle: Option<f32>,
    /// `AppliedObjectStyle` reference (e.g. `ObjectStyle/$ID/[None]`).
    pub applied_object_style: Option<String>,
    /// `LinkResourceURI` from a nested `<Image>` (or its `<Link>`
    /// child) — non-empty when the anchored Rectangle hosts a placed
    /// image.
    pub image_link: Option<String>,
    /// `ItemTransform` on the nested `<Image>` element.
    pub image_item_transform: Option<[f32; 6]>,
    /// Children of an anchored Group, in z-order. Empty for non-Group
    /// anchored frames.
    pub children: Vec<AnchoredFrame>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchoredFrameKind {
    TextFrame,
    Rectangle,
    Group,
}
/// Mirrors IDML's `<AnchoredObjectSetting>` block. The renderer needs
/// only the position + offset attributes to place the anchored frame;
/// fancier kerning / spine-relative behaviour can land in follow-ups.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AnchoredObjectSetting {
    /// `AnchoredPosition` — `InlinePosition`, `AbovePosition`, or
    /// `Custom`. `None` ⇒ use the cascaded default (`InlinePosition`).
    pub anchored_position: Option<String>,
    /// `SpineRelative="true"` flips the offset direction on facing
    /// pages. False is the IDML default.
    pub spine_relative: bool,
    /// `AnchorXoffset` in pt — horizontal nudge from the anchor
    /// point. 0.0 when absent.
    pub anchor_x_offset: f32,
    /// `AnchorYoffset` in pt.
    pub anchor_y_offset: f32,
    /// `AnchorPoint` — `TopLeftAnchor`, `TopCenterAnchor`,
    /// `TopRightAnchor`, `LeftCenterAnchor`, `CenterAnchor`,
    /// `RightCenterAnchor`, `BottomLeftAnchor`, `BottomCenterAnchor`,
    /// `BottomRightAnchor`. `None` ⇒ inherit from the cascade.
    pub anchor_point: Option<String>,
    /// `LockPosition="true"` pins the anchored frame to its current
    /// page position; the user can't drag it.
    pub lock_position: bool,
    /// Phase 5 — `HorizontalReferencePoint` for Custom positioning:
    /// `AnchorLocation` (default), `ColumnEdge`, `TextFrame`,
    /// `PageMargins`, `PageEdge`. None ⇒ AnchorLocation.
    pub horizontal_reference_point: Option<String>,
    /// `HorizontalAlignment` — `LeftAlign` (default), `CenterAlign`,
    /// `RightAlign`. Describes which side of the chosen reference
    /// rectangle the anchor sits against.
    pub horizontal_alignment: Option<String>,
    /// `VerticalReferencePoint` for Custom positioning:
    /// `LineBaseline` (default), `LineXHeight`, `LineCapHeight`,
    /// `TopOfLeading`, `Column`, `TextFrame`, `PageMargins`,
    /// `PageEdge`.
    pub vertical_reference_point: Option<String>,
    /// `VerticalAlignment` — `TopAlign`, `CenterAlign`, `BottomAlign`.
    pub vertical_alignment: Option<String>,
}
/// `<Table>` element parsed from a Story. Cells reference rows /
/// columns by their `Name` (the IDML index, "0"..n-1). Cells in
/// `cells` are stored in document order — IDML serialises them
/// column-major (all cells in column 0, then column 1, etc.).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Table {
    pub self_id: Option<String>,
    pub header_row_count: u32,
    pub footer_row_count: u32,
    pub body_row_count: u32,
    pub column_count: u32,
    /// `RepeatingHeader` flag — when the table breaks across a chain
    /// of threaded text frames, the first `header_row_count` rows
    /// duplicate at the top of every continuation frame. `None` means
    /// the attribute was absent; IDML treats that as the default
    /// ("Repeat" / true). `Some(false)` is the explicit "Once" / no-
    /// repeat case.
    pub repeating_header: Option<bool>,
    /// `RepeatingFooter` analogue — the last `footer_row_count` rows
    /// duplicate at the bottom of every frame except the last.
    pub repeating_footer: Option<bool>,
    /// `AppliedTableStyle="TableStyle/..."` reference. Currently
    /// recorded; cell rendering uses TextTopInset etc. directly off
    /// the cell rather than resolving styles.
    pub applied_table_style: Option<String>,
    pub rows: Vec<TableRow>,
    pub columns: Vec<TableColumn>,
    pub cells: Vec<TableCell>,
    /// Direct outer-border attributes serialised on the `<Table>`
    /// element itself. InDesign emits these on the Table when the
    /// user customises borders without creating a TableStyle. They
    /// take precedence over the `AppliedTableStyle`'s border
    /// declarations.
    pub border: TableBorder,
    /// `StartRowStroke*` / `EndRowStroke*` describe the alternating
    /// dividers between rows. Captured here for the renderer.
    pub row_strokes: TableLineStrokes,
    /// `StartColumnStroke*` / `EndColumnStroke*` analogue for column
    /// dividers. Currently captured but not rendered.
    pub column_strokes: TableLineStrokes,
}
/// Outer-table border attributes serialised directly on `<Table>`
/// (vs. via an `AppliedTableStyle`). All fields optional — `None`
/// means "fall through to the TableStyle cascade / default".
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TableBorder {
    pub top_color: Option<String>,
    pub top_type: Option<String>,
    pub top_weight: Option<f32>,
    pub top_tint: Option<f32>,
    pub top_gap_color: Option<String>,
    pub top_gap_tint: Option<f32>,
    pub bottom_color: Option<String>,
    pub bottom_type: Option<String>,
    pub bottom_weight: Option<f32>,
    pub bottom_tint: Option<f32>,
    pub bottom_gap_color: Option<String>,
    pub bottom_gap_tint: Option<f32>,
    pub left_color: Option<String>,
    pub left_type: Option<String>,
    pub left_weight: Option<f32>,
    pub left_tint: Option<f32>,
    pub left_gap_color: Option<String>,
    pub left_gap_tint: Option<f32>,
    pub right_color: Option<String>,
    pub right_type: Option<String>,
    pub right_weight: Option<f32>,
    pub right_tint: Option<f32>,
    pub right_gap_color: Option<String>,
    pub right_gap_tint: Option<f32>,
}
/// Bag of `Start*Stroke*` / `End*Stroke*` attributes for either the
/// row or column dimension. The "start" set kicks in for the first
/// `start_count` lines, then "end" for `end_count`, alternating.
/// Used for IDML's row / column dividers. All fields optional.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TableLineStrokes {
    pub start_count: Option<u32>,
    pub start_color: Option<String>,
    pub start_type: Option<String>,
    pub start_weight: Option<f32>,
    pub start_tint: Option<f32>,
    pub start_gap_color: Option<String>,
    pub start_gap_tint: Option<f32>,
    pub end_count: Option<u32>,
    pub end_color: Option<String>,
    pub end_type: Option<String>,
    pub end_weight: Option<f32>,
    pub end_tint: Option<f32>,
    pub end_gap_color: Option<String>,
    pub end_gap_tint: Option<f32>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TableRow {
    pub self_id: Option<String>,
    /// IDML index ("0" .. row_count - 1).
    pub name: Option<String>,
    pub single_row_height: Option<f32>,
    pub minimum_height: Option<f32>,
    /// `MaximumHeight` clamp. `None` means unbounded — the row may grow
    /// to fit its tallest cell content. IDML defaults this to a large
    /// sentinel (`8640pt`) when omitted; we keep it `None` and treat
    /// missing as infinity at the call site.
    pub maximum_height: Option<f32>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TableColumn {
    pub self_id: Option<String>,
    pub name: Option<String>,
    pub single_column_width: Option<f32>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TableCell {
    pub self_id: Option<String>,
    /// `Name="col:row"` (zero-indexed). The `row()` and `column()`
    /// helpers parse this.
    pub name: Option<String>,
    pub row_span: u32,
    pub column_span: u32,
    pub text_top_inset: f32,
    pub text_left_inset: f32,
    pub text_bottom_inset: f32,
    pub text_right_inset: f32,
    pub applied_cell_style: Option<String>,
    /// Per-cell edge-stroke overrides. IDML serialises every cell
    /// boundary explicitly on the `<Cell>` element when a TableStyle
    /// applies a divider style, even though the AppliedCellStyle is
    /// `[None]`. Without honouring these, the row/column dividers
    /// vanish entirely. `None` ⇒ inherit from the cell-style cascade.
    pub top_edge_stroke_color: Option<String>,
    pub top_edge_stroke_weight: Option<f32>,
    pub top_edge_stroke_tint: Option<f32>,
    pub bottom_edge_stroke_color: Option<String>,
    pub bottom_edge_stroke_weight: Option<f32>,
    pub bottom_edge_stroke_tint: Option<f32>,
    pub left_edge_stroke_color: Option<String>,
    pub left_edge_stroke_weight: Option<f32>,
    pub left_edge_stroke_tint: Option<f32>,
    pub right_edge_stroke_color: Option<String>,
    pub right_edge_stroke_weight: Option<f32>,
    pub right_edge_stroke_tint: Option<f32>,
    /// Inline `FillColor="Color/..."` on the `<Cell>` element.
    /// Wins over the cell-style cascade — used by header / body /
    /// alternating-fill rows when the table doesn't carry an
    /// AppliedTableStyle. `None` ⇒ inherit from the resolved cell
    /// style.
    pub fill_color: Option<String>,
    /// `FirstBaselineOffset` enum (Ascent / Cap / Leading / Emboxed /
    /// FixedHeight / etc). Drives where the first line of cell text
    /// drops from the cell's top edge. Parsed for completeness; the
    /// renderer currently uses Ascent semantics by default.
    pub first_baseline_offset: Option<String>,
    /// `MinimumFirstBaselineOffset` in pt — only honoured when
    /// `first_baseline_offset` is `FixedHeight` (then the value
    /// becomes the absolute pt drop). Parsed for cascade
    /// completeness.
    pub minimum_first_baseline_offset: Option<f32>,
    /// IDML's per-cell diagonal stroke. The `Left` diagonal in IDML
    /// goes top-left → bottom-right; the `Right` diagonal goes
    /// top-right → bottom-left. Stored as a small bag because all
    /// fields are optional and most cells have neither.
    pub diagonal: CellDiagonal,
    /// `RotationAngle` (degrees, clockwise) applied to the cell's
    /// content. In practice InDesign quantises this to 0/90/180/270.
    /// `None` ⇒ inherit from the cell-style cascade, then default 0.
    /// Borders/fills are not rotated — only the cell content.
    pub rotation_angle: Option<f32>,
    /// `<Cell VerticalJustification="…">` enum string (`"TopAlign"`,
    /// `"CenterAlign"`, `"BottomAlign"`, `"JustifyAlign"`). `None` ⇒
    /// inherit from the cell-style cascade, then default Top. The
    /// renderer currently lays cell content top-aligned; this field is
    /// parsed + writable (W3.A1) so the value round-trips and the
    /// cell-vertical-justify pass can honour it later.
    pub vertical_justification: Option<String>,
    /// Cell content — paragraphs, parsed identically to top-level
    /// story paragraphs.
    pub paragraphs: Vec<Paragraph>,
}
/// Mirrors IDML's diagonal-stroke attributes on `<Cell>`. `LeftLine*`
/// describes the diagonal that drops from top-left to bottom-right;
/// `RightLine*` describes the opposite diagonal. The renderer emits
/// one `<GraphicLine>`-equivalent stroke per drawn diagonal.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CellDiagonal {
    pub left_line_drawn: Option<bool>,
    pub left_line_color: Option<String>,
    pub left_line_weight: Option<f32>,
    /// `LeftLineStrokeTint` percentage (0..=100). `None` ⇒ paint the
    /// diagonal stroke swatch at full strength.
    pub left_line_tint: Option<f32>,
    pub right_line_drawn: Option<bool>,
    pub right_line_color: Option<String>,
    pub right_line_weight: Option<f32>,
    /// `RightLineStrokeTint` percentage (0..=100).
    pub right_line_tint: Option<f32>,
    /// `DiagonalLineInFront` boolean — true means the diagonal paints
    /// on top of cell content. The renderer emits diagonals after
    /// content when this is true.
    pub diagonal_in_front: Option<bool>,
}
impl TableCell {
    /// Parse `(column, row)` from `Name`. Returns `None` if the
    /// attribute is absent or doesn't match `col:row`.
    pub fn coords(&self) -> Option<(u32, u32)> {
        let name = self.name.as_deref()?;
        let (c, r) = name.split_once(':')?;
        Some((c.parse().ok()?, r.parse().ok()?))
    }
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CharacterRun {
    pub character_style: Option<String>,
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    /// `FillColor="Color/..."` on the CharacterStyleRange; resolved
    /// against `Graphic`.
    pub fill_color: Option<String>,
    /// `FillTint` percentage (0..=100). IDML semantics: 100% = use the
    /// swatch at full strength, lower values blend toward paper white.
    /// `-1` (or absent) means "use the swatch as-is" — translates to
    /// `None`. The renderer applies the tint after CMYK→RGB so the
    /// result matches InDesign's preview, where tints sit on top of
    /// the colour-managed pipeline.
    pub fill_tint: Option<f32>,
    /// `Capitalization` value: `Normal | SmallCaps | AllCaps |
    /// CapToSmallCap`. `None` ⇒ use the cascade. The renderer
    /// uppercases the text before shaping when the resolved value is
    /// `AllCaps` (or `SmallCaps`, until proper OT smcp lookup lands).
    pub capitalization: Option<String>,
    /// `BaselineShift` in pt. Positive lifts glyphs above the
    /// baseline, negative drops them. Applied per-glyph at emit time.
    pub baseline_shift: Option<f32>,
    /// `HorizontalScale` percentage (100 = identity). Folded into the
    /// glyph affine by [`CharacterRun::to_styled_run`] so the
    /// shaper sees the requested glyph x-scale (P-08).
    pub horizontal_scale: Option<f32>,
    /// `VerticalScale` percentage (100 = identity). Parsed for future
    /// per-glyph y-scale; not applied yet.
    pub vertical_scale: Option<f32>,
    /// `Skew` in degrees (positive = right-leaning). Folded into the
    /// glyph affine alongside `HorizontalScale` (P-08).
    pub skew: Option<f32>,
    /// `Position` value (`Normal | Superscript | Subscript |
    /// OTSuperscript | OTSubscript | OTNumerator | OTDenominator`).
    /// Parsed for future scaling/baseline-shift application; not yet
    /// honoured.
    pub position: Option<String>,
    /// `Tracking` in 1/1000 em (InDesign's unit — divide by 1000 to
    /// get the em fraction that should be added to every glyph's
    /// advance).
    pub tracking: Option<f32>,
    /// `Underline="true"` on the CharacterStyleRange.
    pub underline: Option<bool>,
    /// `StrikeThru="true"` on the CharacterStyleRange.
    pub strikethru: Option<bool>,
    /// Explicit `Leading` in pt. `None` ⇒ Auto leading
    /// (`point_size × 1.2`). InDesign serialises `Leading` as a
    /// number on the CharacterStyleRange, with magic `Auto` not
    /// modelled here (we treat absence == Auto).
    pub leading: Option<f32>,
    /// `RubyFlag` — when `true`, this run carries ruby annotation
    /// (small phonetic-guide text) above / beside the base run. The
    /// parser captures the flag; full ruby layout (positioning the
    /// annotation text, sizing it as a fraction of the base, etc.)
    /// is queued. See docs/plan.md Tier 4 — CJK Stage 4.
    pub ruby_flag: Option<bool>,
    /// `RubyType` — `PerCharacter` / `GroupRuby`. Parser-only today.
    pub ruby_type: Option<String>,
    /// `RubyString` — the ruby annotation text itself.
    pub ruby_string: Option<String>,
    /// `KentenKind` — the emphasis-mark glyph for this run.
    /// Parser-only — emphasis-mark rendering is queued (Tier 4 Stage 4).
    pub kenten_kind: Option<String>,
    /// `KentenCharacter` — codepoint to stamp when `kenten_kind == "Custom"`.
    pub kenten_character: Option<String>,
    /// `KentenFontSize` — emphasis-mark glyph size as a percentage.
    pub kenten_font_size: Option<f32>,
    /// `OverprintFill="true"` on the `<CharacterStyleRange>`. None ⇒
    /// inherit from the applied character / paragraph style cascade.
    /// Drives the renderer's Stage 3 darken composite when true.
    pub overprint_fill: Option<bool>,
    /// `OverprintStroke="true"` analogue (rare on text but parsed).
    pub overprint_stroke: Option<bool>,
    /// `StrokeColor` on the `<CharacterStyleRange>` — the paint used
    /// to outline each glyph. None ⇒ inherit from the applied
    /// character / paragraph style cascade; if still absent at the
    /// bottom of the cascade the renderer treats the glyph as
    /// fill-only (no outline). IDML stores "no stroke" as
    /// `Swatch/None`; the parser normalises that to `None` for text
    /// runs the same way object strokes do.
    pub stroke_color: Option<String>,
    /// `StrokeWeight` on the `<CharacterStyleRange>` in pt. Absent on
    /// most runs because InDesign records the run's stroke weight
    /// only when it differs from the document's `<TextDefault>`
    /// value (which is 1pt for new documents). The renderer falls
    /// back to 1pt at emit time when `stroke_color` resolves but
    /// `stroke_weight` doesn't.
    pub stroke_weight: Option<f32>,
    /// Phase 4 typography — `Ligatures="true|false"` on the
    /// `<CharacterStyleRange>`. None ⇒ inherit. Default at the
    /// bottom of the cascade is `true` (InDesign's CharacterStyle
    /// default).
    pub ligatures_on: Option<bool>,
    /// `KerningMethod="Metrics|Optical|None"` on the
    /// `<CharacterStyleRange>`. None ⇒ inherit. Default at the
    /// bottom of the cascade is `Metrics`.
    pub kerning_method: Option<String>,
    /// `AppliedLanguage="$ID/..."` on the `<CharacterStyleRange>` —
    /// the run's language reference (drives hyphenation /
    /// spell-check dictionaries). None ⇒ inherit from the applied
    /// character / paragraph style cascade. Stored as the raw IDML
    /// reference string; no renderer behaviour is keyed off it yet
    /// (parser-only today), but the mutate API surfaces it so the
    /// editor can author the value.
    pub applied_language: Option<String>,
    /// OpenType feature toggles as an opaque, space-separated tag
    /// list (e.g. `"frac ordn ss01"`). The mutate API owns this as a
    /// free-form authoring override string (parser leaves it `None`);
    /// the *parsed* discrete IDML attributes live in [`Self::otf`].
    pub otf_features: Option<String>,
    /// Discrete OpenType feature toggles (`OTFFraction`, `OTFOrdinal`,
    /// `OTFSwash`, `OTFDiscretionaryLigature`, `OTFFigureStyle`,
    /// `OTFStylisticSets`, …) parsed off the `<CharacterStyleRange>`.
    /// Each field `None` ⇒ inherit from the cascade. See [`OtfFeatures`].
    pub otf: OtfFeatures,
    /// Phase 5 — `AppliedConditions="Condition/A Condition/B"`.
    /// Space-separated list of `<Condition>` references. Empty
    /// means "no condition gating" (always visible). A run with
    /// non-empty conditions is rendered iff every referenced
    /// condition resolves to `Visible="true"` in the document's
    /// `<Condition>` table.
    pub applied_conditions: Vec<String>,
    /// W1.4 — the `Self` of the enclosing `<HyperlinkTextSource>` (or
    /// `<CrossReferenceSource>`) when this run sits inside one. IDML
    /// serialises a hyperlink/cross-reference *source* span as a
    /// wrapper element around the character ranges it covers; the
    /// `<Hyperlink>` / `<CrossReference>` in the designmap references
    /// it by `Source`. The renderer resolves the source id back to a
    /// destination (URL / page) and emits a clickable region over the
    /// run's glyph rect. `None` for the overwhelming majority of runs.
    pub hyperlink_source: Option<String>,
    /// W1.4 — the `AssociatedTextVariable` id (`TextVariable/<id>`)
    /// when this run was produced by a `<TextVariableInstance>`. The
    /// run's `text` carries InDesign's baked `ResultText`; the
    /// renderer re-resolves the value per type at emit time (real page
    /// count, document name, custom content, formatted dates) when it
    /// can do better than the baked string, and falls back to
    /// `ResultText` otherwise. `None` for ordinary text runs.
    pub text_variable: Option<String>,
    /// v43 (D-01) — plugin placeholder tag when this run IS a tagged
    /// placeholder field (the paged.data anchor model). The run's
    /// `text` always carries the field's *display* string (the cached
    /// resolved value, or the visible `<key>` token while unresolved),
    /// so layout/shaping treats it as ordinary run text — the same
    /// "baked result text" posture as [`Self::text_variable`], except
    /// nothing ever re-resolves it at emit time (the offline-forever
    /// rule: the engine never calls the owning plugin to render).
    /// `None` for ordinary text runs. The parser never sets this today;
    /// placeholders enter via the mutate API (`InsertField`).
    pub placeholder: Option<PlaceholderField>,
    pub text: String,
}
/// v43 (D-01) — a plugin-owned tagged placeholder: a named,
/// edit-surviving anchor inside a story's run list. `(plugin, key)` is
/// the identity the owning plugin re-finds it by; `value` is the
/// cached resolved display (`None` = not yet resolved → the run shows
/// the `<key>` token).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceholderField {
    /// The owning bundle's manifest id (host-stamped; e.g.
    /// `media.paged.data`).
    pub plugin: String,
    /// Bundle-unique placeholder name (the binding's anchor).
    pub key: String,
    /// Last-resolved display value. `None` ⇒ unresolved.
    pub value: Option<String>,
}
impl PlaceholderField {
    /// The string the run renders: the cached value, or the visible
    /// `<key>` token while unresolved.
    pub fn display_text(&self) -> String {
        match &self.value {
            Some(v) => v.clone(),
            None => format!("<{}>", self.key),
        }
    }
}

// ---------------------------------------------------------------------------
// Graphic container — the document's swatch palette. Owns the parsed
// `<Color>`/`<Swatch>`/`<Gradient>`/`<ColorGroup>` tables + the pure lookup
// helpers. The XML parsing (`parse_graphic` + the per-element parse helpers)
// stays in the parser; this type is de-inherented (N6) so it can live in the
// model with everything it holds.
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Graphic {
    /// All `<Color>` entries, keyed by `Self` (e.g. "Color/Red").
    pub colors: BTreeMap<String, ColorEntry>,
    /// Named `<Swatch>` entries — "None", "Paper", "Black", etc.
    pub swatches: BTreeMap<String, SwatchEntry>,
    /// `<Gradient>` swatches (linear / radial), keyed by `Self`
    /// (e.g. "Gradient/Sky").
    pub gradients: BTreeMap<String, GradientEntry>,
    /// SDK Phase 5 (v1 sweep) — `<ColorGroup>` named groupings of
    /// `Color` self_ids. The Color Groups panel surfaces them as
    /// a way to organise the palette into themed families
    /// ("Brand colours", "UI accents"). Empty when the document
    /// declares no groups (the renderer doesn't branch on them).
    pub color_groups: BTreeMap<String, ColorGroupEntry>,
}

impl Graphic {
    /// Look up a colour by its `Self` id. Follows a `<Swatch>` indirection
    /// one level if the id names a Swatch rather than a Color directly.
    pub fn resolve(&self, id: &str) -> Option<&ColorEntry> {
        if let Some(c) = self.colors.get(id) {
            return Some(c);
        }
        let swatch = self.swatches.get(id)?;
        let color_ref = swatch.color_ref.as_deref()?;
        self.colors.get(color_ref)
    }

    /// Resolve a swatch's alpha channel (0..=1, 1 = fully opaque).
    /// Used by the gradient-feather renderer when a `<GradientStop>`
    /// in IDML spec form (`StopColor="Color/..."`) references a
    /// `<Color>` swatch whose alpha defines the stop's opacity.
    /// Returns `None` when the swatch carries no alpha (CMYK / RGB
    /// without `AlphaPercentage`) — callers should treat that as
    /// "opaque" and fall back to whatever inline alpha attribute the
    /// stop carries (e.g. the IDML `Alpha` / `Opacity`).
    pub fn resolve_alpha(&self, id: &str) -> Option<f32> {
        self.resolve(id).and_then(|c| c.alpha)
    }
}

// ---------------------------------------------------------------------------
// StyleSheet — the parsed style tables + the BasedOn-cascade resolvers (moved
// out of `paged-parse::styles`; `parse_stylesheet` stays in the parser). The
// resolve_* methods are pure cascade resolution, so they live with the model (N6).
// ---------------------------------------------------------------------------

/// Maximum BasedOn chain length. IDML doesn't forbid cycles, so the
/// resolver short-circuits once it hits this depth — typical real-
/// world chains are 1–3 hops.
const MAX_BASED_ON_DEPTH: usize = 16;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StyleSheet {
    pub character_styles: BTreeMap<String, CharacterStyleDef>,
    pub paragraph_styles: BTreeMap<String, ParagraphStyleDef>,
    /// `<ObjectStyle>` definitions from `<RootObjectStyleGroup>`.
    /// Page-item shapes (TextFrame, Rectangle, Oval, GraphicLine,
    /// Polygon) reference these via `AppliedObjectStyle="..."` to
    /// inherit fill / stroke / etc. when their own attributes are
    /// absent. Real-world IDMLs use this almost exclusively for
    /// rectangle fills.
    pub object_styles: BTreeMap<String, ObjectStyleDef>,
    /// `<CellStyle>` definitions from `<RootCellStyleGroup>`. Cells
    /// reference these via `AppliedCellStyle="..."` to inherit
    /// fill / VJ / per-edge strokes when their own attributes are
    /// absent.
    pub cell_styles: BTreeMap<String, CellStyleDef>,
    /// `<TableStyle>` definitions. Tables reference one via
    /// `AppliedTableStyle="..."`; the style nominates a default
    /// CellStyle per region (header, body, footer, left column,
    /// right column) plus the table-level border strokes.
    pub table_styles: BTreeMap<String, TableStyleDef>,
    /// `<TOCStyle>` definitions from `Resources/Styles.xml`. Each
    /// carries an ordered list of `<TOCStyleEntry>` children
    /// declaring which paragraph styles feed the TOC, the format
    /// style applied to each rendered entry, and the page-number /
    /// separator settings. Real-world IDMLs commonly serialise a
    /// single default empty TOCStyle (no entries) alongside any
    /// user-defined ones.
    pub toc_styles: BTreeMap<String, TOCStyleDef>,
    /// Track 4a: custom `<DashedStrokeStyle>` / `<DottedStrokeStyle>` /
    /// `<StripedStrokeStyle>` definitions from `Resources/Styles.xml`.
    /// Page items reference these via `StrokeType="StrokeStyle/<id>"`;
    /// without this table the renderer fell back to `Solid` for every
    /// user-defined stroke (e.g. business-proposal-template's
    /// diagonal-stripe cover, which is a dense custom dash).
    pub stroke_styles: BTreeMap<String, StrokeStyleDef>,
    /// Phase 5 — `<Condition>` definitions from `Resources/Styles.xml`.
    /// A `<CharacterStyleRange AppliedConditions="Condition/A Condition/B">`
    /// is rendered iff every referenced condition has `Visible="true"`
    /// at the document level. Empty when the IDML declares no
    /// conditional text.
    pub conditions: BTreeMap<String, ConditionDef>,
    /// SDK Phase 5 (v1 sweep) — `<ConditionSet>` named groupings of
    /// Conditions. A user-defined collection of `Condition` refs
    /// the document organises into one toggleable set (e.g. "Print
    /// preview", "Online preview"). Empty when the IDML declares
    /// no condition sets.
    pub condition_sets: BTreeMap<String, ConditionSetDef>,
    /// W1.22 (engine gap 22) — `<NumberingList>` resources. A named
    /// list definition paragraphs bind to via `AppliedNumberingList`;
    /// its `continue_across_stories` / `continue_across_documents`
    /// flags control whether the renderer's numbering counter carries
    /// forward when the same list spans multiple stories. Empty when
    /// the IDML declares no numbered lists. Lives in `Resources/
    /// Styles.xml` alongside `<Condition>` (and inside the optional
    /// `<RootNumberingListGroup>` wrapper InDesign sometimes emits) —
    /// mirrors the `conditions` table's home.
    pub numbering_lists: BTreeMap<String, NumberingListDef>,
}

impl StyleSheet {
    /// Walk a CharacterStyle's `BasedOn` chain, folding each hop's
    /// unset attributes from its parent. Missing or cyclic chains
    /// short-circuit at `MAX_BASED_ON_DEPTH`.
    pub fn resolve_character(&self, id: &str) -> ResolvedCharacter {
        let mut acc = ResolvedCharacter::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.character_styles.get(&cur_id) else {
                break;
            };
            acc.merge_below(s);
            cursor = s.based_on.clone();
        }
        acc
    }

    pub fn resolve_paragraph(&self, id: &str) -> ResolvedParagraph {
        let mut acc = ResolvedParagraph::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.paragraph_styles.get(&cur_id) else {
                break;
            };
            acc.merge_below(s);
            cursor = s.based_on.clone();
        }
        acc
    }

    /// Walk an ObjectStyle's `BasedOn` chain. Same depth-bounded
    /// pattern as `resolve_paragraph` / `resolve_character`.
    pub fn resolve_object(&self, id: &str) -> ResolvedObject {
        let mut acc = ResolvedObject::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.object_styles.get(&cur_id) else {
                break;
            };
            acc.merge_below(s);
            cursor = s.based_on.clone();
        }
        acc
    }

    /// Walk a CellStyle's BasedOn chain. Cell strokes / fills /
    /// vertical justification cascade through it.
    pub fn resolve_cell(&self, id: &str) -> ResolvedCell {
        let mut acc = ResolvedCell::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.cell_styles.get(&cur_id) else {
                break;
            };
            acc.merge_below(s);
            cursor = s.based_on.clone();
        }
        acc
    }

    /// Walk a TableStyle's BasedOn chain. Resolves region →
    /// CellStyle assignments + table border strokes + alternating
    /// row fills.
    pub fn resolve_table(&self, id: &str) -> ResolvedTable {
        let mut acc = ResolvedTable::default();
        let mut cursor = Some(id.to_string());
        for _ in 0..MAX_BASED_ON_DEPTH {
            let Some(cur_id) = cursor else { break };
            let Some(s) = self.table_styles.get(&cur_id) else {
                break;
            };
            acc.merge_below(s);
            cursor = s.based_on.clone();
        }
        acc
    }
}

// ---------------------------------------------------------------------------
// Spread — a parsed spread / master-spread (pages + page items + groups). The
// XML parsing (`parse_spread` + the private frame-builder state) stays in the
// parser; the value struct lives in the model (N6).
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Spread {
    pub self_id: Option<String>,
    /// `ItemTransform` on the `<Spread>` (or `<MasterSpread>`)
    /// element. Per the IDML spec §10.3.3, this maps the spread's
    /// inner coords into the document's pasteboard. InDesign limits
    /// this to translation + 0/90/180/270 rotation. W1.9: the
    /// renderer honours its rotation/scale (linear part) per page via
    /// `BuiltPage::spread_transform` — composed into every page-item
    /// emission and inverted by the canvas hit-tester so selection
    /// stays in lockstep. A pure translation cancels against the
    /// spread-inner page origin, so only the linear part has effect.
    /// `None` ⇒ identity.
    pub item_transform: Option<[f32; 6]>,
    pub pages: Vec<Page>,
    pub text_frames: Vec<TextFrame>,
    /// Axis-aligned rectangles used as pure vector frames (no parent
    /// story). A full Rectangle path can have corner radii etc. — we
    /// treat it as a rect; higher-fidelity paths come with §10.1.
    pub rectangles: Vec<Rectangle>,
    /// Ellipses (`<Oval>`). Treated as the inscribed ellipse of the
    /// `GeometricBounds` rect.
    pub ovals: Vec<Oval>,
    /// Straight lines (`<GraphicLine>`). The `GeometricBounds`
    /// describe the line's bounding box; its endpoints are the
    /// rect's top-left and bottom-right corners.
    pub graphic_lines: Vec<GraphicLine>,
    /// `<Polygon>` items. Real-world IDMLs use these for charts,
    /// rosettes, and any non-rectangular flat shape. Today the
    /// renderer treats them as their axis-aligned bounding box —
    /// faithful only for axis-aligned simple polygons; complex
    /// shapes (donut charts etc.) come with full path rasterisation
    /// later in the roadmap.
    pub polygons: Vec<Polygon>,
    /// Number of text frames skipped because they were nested inside a
    /// Group. Exposed so callers can flag lossy parses without reading
    /// logs.
    pub skipped_nested_frames: usize,
    /// `<Group>` records, one per group element seen. Each entry
    /// names the page items it wraps (TextFrame / Rectangle / Oval /
    /// GraphicLine / Polygon / sub-groups) and the group-level
    /// transparency settings (`<BlendingSetting>` / `<DropShadowSetting>`)
    /// the IDML attached. Real-world IDMLs use a Group around several
    /// shapes when the user wants a single Opacity / BlendMode / drop
    /// shadow to apply uniformly to the cluster — the renderer
    /// brackets the frame range with a transparency group and reuses
    /// the per-frame paint pipeline inside.
    ///
    /// Outermost groups appear first; nested groups come later in the
    /// vec. Child shape indices are recorded in the order the parser
    /// encountered them.
    pub groups: Vec<Group>,
    /// Top-level page items in XML order. Group members live on the
    /// corresponding `Group::members` list and are NOT duplicated here
    /// — instead the outermost group surfaces here as a single
    /// `FrameRef::Group(idx)`. The renderer uses this flat list to
    /// drive cross-shape z-ordering (Q-10): items on a back ItemLayer
    /// paint before items on a front layer regardless of the
    /// per-shape XML order their backing `Vec<…>` records.
    pub frames_in_order: Vec<FrameRef>,
    /// `<Guide>` elements parsed off the spread (plan-2 §8.3 "ruler
    /// guides"). Vertical guides have `orientation = Vertical` and a
    /// page-local `location` on the x axis; horizontal guides
    /// flip the axis. `page_index` is the zero-based index into the
    /// spread's pages (matches IDML's `PageIndex` attribute). The
    /// snap pass treats each guide on the moving frame's host page
    /// as an extra target; the overlay renders them as cyan lines.
    #[serde(default)]
    pub guides: Vec<RulerGuide>,
    /// Placed-image colour space + resolution InDesign baked onto each
    /// `<Image>` element, keyed by the HOST frame's `Self` id
    /// (Rectangle / Oval / Polygon). Kept as a side map rather than a
    /// field on the frame structs so the metadata rides along without
    /// expanding every frame literal (panels.md gaps 2-3). Empty when
    /// no placed image carried `Space` / `ActualPpi` / `EffectivePpi`.
    #[serde(default)]
    pub image_metadata: std::collections::HashMap<String, ImageMetadata>,
    /// `<MarginPreference>` per `<Page>`, keyed by the page's `Self`
    /// id (panels.md gap 10). Side map for the same reason as
    /// [`Spread::image_metadata`] — keeps `Page` literals untouched.
    /// Empty when no page declared margins.
    #[serde(default)]
    pub page_margins: std::collections::HashMap<String, MarginPreference>,
    /// Per-object `Properties/Label` `KeyValuePair`s, keyed by the
    /// host item's `Self` id — IDML's native extension point (the
    /// plugin-metadata carrier; InDesign preserves Labels verbatim).
    /// Side map like [`Spread::image_metadata`] so the frame literals
    /// stay untouched. The inner Vec preserves XML order; one entry
    /// per `Key`.
    #[serde(default)]
    pub labels: std::collections::HashMap<String, Vec<(String, String)>>,
}
