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
