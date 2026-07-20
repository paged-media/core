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
