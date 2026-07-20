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
