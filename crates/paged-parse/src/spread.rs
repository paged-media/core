//! Spread_*.xml parser.
//!
//! Extracts page bounds and text-frame geometry from a Spread. This is
//! the minimal schema slice needed to know *where* text goes on the
//! page — a TextFrame's bounding rectangle becomes a column width for
//! the composer.
//!
//! Coverage:
//! - `<Page GeometricBounds="...">` — one entry per page.
//! - `<TextFrame ParentStory="..." GeometricBounds="..." ItemTransform="...">`
//!   at spread level. Text frames nested inside `<Group>` are
//!   intentionally out of scope for now; a warning surfaces via the
//!   parse result counters so higher layers can detect loss.
//!
//! GeometricBounds is `y1 x1 y2 x2` in points (IDML convention:
//! y-axis grows downward from page origin).

use quick_xml::events::Event;
use serde::Serialize;

use crate::util::{attr, parse_f, parse_tint_attr};
use crate::ParseError;

/// Q-16: per-corner override for `Rectangle::corners`. IDML lists
/// these on `<Rectangle>` as `TopLeftCornerOption` / `TopLeftCornerRadius`
/// and the other three corners. When both fields are `None` the
/// renderer falls back to the legacy single `corner_option` /
/// `corner_radius` pair (which itself defaults to "no rounding").
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize)]
pub struct CornerSpec {
    pub option: Option<CornerOption>,
    pub radius: Option<f32>,
}

/// IDML `CornerOption` enum (per-corner or document-default). The
/// renderer treats every non-`None`, non-`Square` variant as
/// `Rounded` for the time being — the decorative shapes (Fancy,
/// Bevel, Inset, etc.) need bespoke path generators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

    /// Whether this corner option actually rounds (i.e. produces a
    /// non-square corner). The decorative variants all fall back to
    /// `Rounded` shape in the renderer for now, so they all return true.
    pub fn rounds(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Spread {
    pub self_id: Option<String>,
    /// `ItemTransform` on the `<Spread>` (or `<MasterSpread>`)
    /// element. Per the IDML spec §10.3.3, this maps the spread's
    /// inner coords into the document's pasteboard. InDesign limits
    /// this to translation + 0/90/180/270 rotation. Per-page
    /// rendering doesn't need the value (each page already lives in
    /// the spread's coord system), but pasteboard-faithful output
    /// across the whole document does. `None` ⇒ identity.
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
}

/// Ruler guide on a spread. See [`Spread::guides`].
#[derive(Debug, Clone, Copy, Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum GuideOrientation {
    Vertical,
    Horizontal,
}

/// One `<Group>` page-item record. The renderer walks `members` to
/// know which frames sit inside this group, and `transparency` to
/// decide whether to bracket the range with a transparency-group
/// composite.
#[derive(Debug, Clone, Default, Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
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
}

#[derive(Debug, Clone, Serialize)]
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
}

/// IDML `<TextFramePreference VerticalJustification="...">` values.
/// `Top` is the IDML default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
pub struct DropShadowSetting {
    pub mode: String,
    pub x_offset: f32,
    pub y_offset: f32,
    pub size: f32,
    pub opacity_pct: f32,
    pub effect_color: Option<String>,
}

/// Vector-only frame (no story). Mirrors `TextFrame` minus the
/// `parent_story` field; shares the same paint / stroke handling
/// downstream.
#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
pub struct InnerGlowParams {
    pub size: Option<f32>,
    pub opacity_pct: Option<f32>,
    pub effect_color: Option<String>,
    pub choke_pct: Option<f32>,
    pub blend_mode: Option<String>,
    pub source: Option<String>,
    pub noise_pct: Option<f32>,
}

/// `<BevelAndEmbossSetting>` parameters. `style` and `direction` steer
/// between bevel / emboss / pillow variants; the rasterizer uses the
/// Lambertian light at `(angle_deg, altitude_deg)` regardless.
#[derive(Debug, Default, Clone, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
pub struct FeatherParams {
    pub width: Option<f32>,
    pub corner_type: Option<String>,
    pub noise_pct: Option<f32>,
    pub choke_pct: Option<f32>,
}

/// `<DirectionalFeatherSetting>` parameters. Each edge carries an
/// independent feather width in pt; `angle_deg` rotates the per-edge
/// directions. The renderer currently approximates this with a
/// uniform feather using the max of the four widths — angle is
/// captured but unused.
#[derive(Debug, Default, Clone, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
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
}

/// Axis-aligned ellipse — `<Oval>` in IDML. Same fill/stroke story as
/// Rectangle; geometry is the ellipse inscribed in `GeometricBounds`.
#[derive(Debug, Clone, Serialize)]
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
    /// `OverprintFill="true"`. See [`TextFrame::overprint_fill`].
    pub overprint_fill: bool,
    /// `OverprintStroke="true"`. See [`TextFrame::overprint_stroke`].
    pub overprint_stroke: bool,
    /// SDK Phase 5 (v1 sweep) — `Nonprinting`. See
    /// [`TextFrame::nonprinting`].
    pub nonprinting: bool,
}

/// Straight line — `<GraphicLine>` in IDML. The endpoints are the
/// `GeometricBounds` rect's top-left and bottom-right corners (IDML
/// stores the endpoints implicitly via the bounds).
#[derive(Debug, Clone, Serialize)]
pub struct GraphicLine {
    pub self_id: Option<String>,
    pub bounds: Bounds,
    pub item_transform: Option<[f32; 6]>,
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
    /// `StrokeType` reference; see [`Rectangle::stroke_type`].
    pub stroke_type: Option<String>,
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
}

/// One point on an IDML `<PathGeometry>` path. `anchor` is the
/// on-curve point; `left` / `right` are the incoming / outgoing
/// Bezier control points respectively. Coordinates are in the
/// owning page item's *inner* coordinate system.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct PathAnchor {
    pub anchor: (f32, f32),
    pub left: (f32, f32),
    pub right: (f32, f32),
}

/// `<TextWrapPreference>` settings on a page item. Parsed onto
/// every shape (TextFrame / Rectangle / Oval / Polygon /
/// GraphicLine). The renderer uses the AABB plus offsets as a
/// per-page wrap exclusion when laying out other text frames.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct TextWrap {
    pub mode: TextWrapMode,
    /// IDML order: `[top, left, bottom, right]`, in pt. Inflates the
    /// wrap rectangle outwards so text keeps its distance.
    pub offsets: [f32; 4],
}

impl TextWrap {
    pub const NONE: TextWrap = TextWrap {
        mode: TextWrapMode::None,
        offsets: [0.0; 4],
    };
}

/// `TextWrapMode` enum value. Values not in the IDML spec collapse
/// to `Other` so the cascade still records *something* the renderer
/// can decide to ignore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
pub struct TextPath {
    pub self_id: Option<String>,
    /// Story reference (e.g. `u3ae`). Same shape as
    /// `TextFrame::parent_story`.
    pub parent_story: String,
    /// `PathAlignment` — `CenterPathAlignment` (default), `TopPathAlignment`,
    /// `BottomPathAlignment`. Captured for future fidelity work; the
    /// current renderer assumes the glyphs sit on the host path.
    pub path_alignment: Option<String>,
    /// `PathEffect` — `RainbowPathEffect`, `SkewPathEffect`,
    /// `Path3DRibbonEffect`, `StairStepPathEffect`,
    /// `GravityPathEffect`. Currently informational.
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
#[derive(Debug, Clone, Serialize)]
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
    /// `OverprintFill="true"`. See [`TextFrame::overprint_fill`].
    pub overprint_fill: bool,
    /// `OverprintStroke="true"`. See [`TextFrame::overprint_stroke`].
    pub overprint_stroke: bool,
    /// SDK Phase 5 (v1 sweep) — `Nonprinting`. See
    /// [`TextFrame::nonprinting`].
    pub nonprinting: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
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

/// Identifies the most recently opened shape element so child
/// elements (DropShadowSetting, TextFramePreference, PathPointType,
/// Image, Link) can attach to the right frame.
#[derive(Debug, Clone, Copy)]
enum CurrentFrameKind {
    Text(usize),
    Rect(usize),
    Oval(usize),
    Line(usize),
    Polygon(usize),
}

/// Per-`<Group>` parser state. Accumulates the group's child page
/// items and the transparency block as each fires; finalised into a
/// `Group` record on the closing `</Group>` tag.
struct GroupBuilder {
    self_id: Option<String>,
    item_transform: Option<[f32; 6]>,
    members: Vec<FrameRef>,
    transparency: GroupTransparency,
    /// Depth counter for nested `<StrokeTransparencySetting>` /
    /// `<ContentTransparencySetting>` containers seen *while no inner
    /// page-item is open*. Routes child `<DropShadowSetting>` blocks
    /// to the right place: stroke-only / content-only shadows attached
    /// to a Group don't map onto our model and are skipped.
    stroke_transparency_depth: u32,
    content_transparency_depth: u32,
}

/// Per-frame parser state held while a shape element is open.
/// Tracks whether the bounds came from a `GeometricBounds` attribute
/// (the legacy synthetic-IDML shape) or need to be derived from the
/// frame's `<PathGeometry>` (the InDesign-export shape — the format
/// real-world IDMLs use almost exclusively).
struct CurrentFrame {
    kind: CurrentFrameKind,
    /// True if the open tag had no `GeometricBounds` — bounds must
    /// then come from `<PathPointType Anchor="...">` children.
    needs_bounds: bool,
    /// Path-point anchors accumulated while the frame is open.
    /// Always collected for Polygons (so the renderer can rasterise
    /// the curved path); collected for the other shapes only when
    /// `needs_bounds` is true so we can derive an AABB on close.
    anchors: Vec<PathAnchor>,
    /// Subpath start offsets into `anchors` — one per
    /// `<GeometryPathType>` opening tag while the shape is open.
    /// Allows the renderer to lift compound paths (square-with-hole
    /// etc.) into multiple `MoveTo`/`Close` segments rather than
    /// joining them into one broken polyline.
    subpath_starts: Vec<usize>,
    /// Parallel to `subpath_starts`: the open/closed flag harvested
    /// from each `<GeometryPathType PathOpen="...">` (P-15).
    subpath_open: Vec<bool>,
    /// True for Polygons even when `needs_bounds` is false, so the
    /// emitter still gets the curved-path data.
    keep_anchors: bool,
    /// True while a `<TextWrapPreference>` block is open, so the
    /// child `<TextWrapOffset>` knows to write back to the current
    /// shape.
    in_text_wrap: bool,
    /// Depth counter for nested `<StrokeTransparencySetting>`
    /// containers. When > 0, child `<DropShadowSetting>` blocks
    /// describe stroke-only shadows — captured into
    /// `stroke_drop_shadow` on the shape so the renderer can emit
    /// them only when the stroke is actually visible.
    stroke_transparency_depth: u32,
    /// Depth counter for nested `<ContentTransparencySetting>`
    /// containers. When > 0, child `<DropShadowSetting>` blocks
    /// describe content-only shadows that don't map onto our
    /// single-shadow-per-frame model and are skipped.
    content_transparency_depth: u32,
}

/// Read whatever `text_wrap.offsets` has already been recorded on the
/// current shape, defaulting to all zeros.
fn current_text_wrap_offsets(out: &Spread, kind: CurrentFrameKind) -> [f32; 4] {
    let cur = match kind {
        CurrentFrameKind::Text(i) => out.text_frames.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Rect(i) => out.rectangles.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Oval(i) => out.ovals.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Line(i) => out.graphic_lines.get(i).and_then(|s| s.text_wrap),
        CurrentFrameKind::Polygon(i) => out.polygons.get(i).and_then(|s| s.text_wrap),
    };
    cur.map(|w| w.offsets).unwrap_or([0.0; 4])
}

fn apply_text_wrap(out: &mut Spread, kind: CurrentFrameKind, wrap: Option<TextWrap>) {
    match kind {
        CurrentFrameKind::Text(i) => out.text_frames[i].text_wrap = wrap,
        CurrentFrameKind::Rect(i) => out.rectangles[i].text_wrap = wrap,
        CurrentFrameKind::Oval(i) => out.ovals[i].text_wrap = wrap,
        CurrentFrameKind::Line(i) => out.graphic_lines[i].text_wrap = wrap,
        CurrentFrameKind::Polygon(i) => out.polygons[i].text_wrap = wrap,
    }
}

fn set_text_wrap_offsets(out: &mut Spread, kind: CurrentFrameKind, offsets: [f32; 4]) {
    let take = |w: &mut Option<TextWrap>| {
        if let Some(existing) = w.as_mut() {
            existing.offsets = offsets;
        } else {
            *w = Some(TextWrap {
                mode: TextWrapMode::None,
                offsets,
            });
        }
    };
    match kind {
        CurrentFrameKind::Text(i) => take(&mut out.text_frames[i].text_wrap),
        CurrentFrameKind::Rect(i) => take(&mut out.rectangles[i].text_wrap),
        CurrentFrameKind::Oval(i) => take(&mut out.ovals[i].text_wrap),
        CurrentFrameKind::Line(i) => take(&mut out.graphic_lines[i].text_wrap),
        CurrentFrameKind::Polygon(i) => take(&mut out.polygons[i].text_wrap),
    }
}

/// Cross-cutting attributes shared by every shape element
/// (`<TextFrame>`, `<Rectangle>`, `<Oval>`, `<Polygon>`,
/// `<GraphicLine>`). Read once via [`read_common_attrs`] so each
/// per-shape arm doesn't repeat the same `attr(&e, b"...")` block.
///
/// `item_transform` is the *raw* parsed `[a b c d tx ty]` matrix —
/// callers compose it with the surrounding `group_transforms` via
/// [`effective_item_transform`] exactly like before.
struct CommonAttrs {
    self_id: Option<String>,
    item_transform: Option<[f32; 6]>,
    fill_color: Option<String>,
    fill_tint: Option<f32>,
    gradient_fill_angle: Option<f32>,
    gradient_fill_length: Option<f32>,
    gradient_stroke_angle: Option<f32>,
    gradient_stroke_length: Option<f32>,
    stroke_color: Option<String>,
    stroke_weight: Option<f32>,
    /// `StrokeType` reference. Defined by IDML on every page item; the
    /// renderer consumes it to pick built-in dash names or look up a
    /// custom `<DashedStrokeStyle>` (cycle-3 4a). Lives on `CommonAttrs`
    /// rather than `StrokeStyleAttrs` so Oval / Polygon / GraphicLine /
    /// TextFrame all get it without each shape duplicating the read.
    stroke_type: Option<String>,
    applied_object_style: Option<String>,
    item_layer: Option<String>,
    /// `OverprintFill="true"` on the IDML element. Absent attribute
    /// or unparseable value ⇒ `false` (IDML default).
    overprint_fill: bool,
    /// `OverprintStroke="true"` analogue.
    overprint_stroke: bool,
    /// SDK Phase 5 (v1 sweep) — `Nonprinting="true"` excludes the
    /// item from print/export. Renderer keeps it visible on canvas
    /// but suppresses it from output passes. Default: `false`.
    nonprinting: bool,
}

fn read_common_attrs(e: &quick_xml::events::BytesStart) -> CommonAttrs {
    CommonAttrs {
        self_id: attr(e, b"Self"),
        item_transform: attr(e, b"ItemTransform").and_then(|s| parse_matrix(&s)),
        fill_color: attr(e, b"FillColor"),
        fill_tint: parse_tint_attr(e, b"FillTint"),
        gradient_fill_angle: attr(e, b"GradientFillAngle").and_then(|s| s.parse().ok()),
        gradient_fill_length: attr(e, b"GradientFillLength").and_then(|s| s.parse().ok()),
        gradient_stroke_angle: attr(e, b"GradientStrokeAngle").and_then(|s| s.parse().ok()),
        gradient_stroke_length: attr(e, b"GradientStrokeLength").and_then(|s| s.parse().ok()),
        stroke_color: attr(e, b"StrokeColor"),
        stroke_weight: attr(e, b"StrokeWeight").and_then(|s| s.parse().ok()),
        stroke_type: attr(e, b"StrokeType"),
        applied_object_style: attr(e, b"AppliedObjectStyle"),
        item_layer: attr(e, b"ItemLayer"),
        overprint_fill: attr(e, b"OverprintFill")
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false),
        overprint_stroke: attr(e, b"OverprintStroke")
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false),
        nonprinting: attr(e, b"Nonprinting")
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false),
    }
}

/// Rectangle-only stroke style attributes (`StrokeAlignment`,
/// `EndCap`, `EndJoin`, `MiterLimit`). `StrokeType` moved to
/// [`CommonAttrs`] in cycle 4 so non-rectangle shapes can also
/// honour custom dash patterns.
struct StrokeStyleAttrs {
    stroke_alignment: Option<String>,
    end_cap: Option<String>,
    end_join: Option<String>,
    miter_limit: Option<f32>,
}

fn read_stroke_style_attrs(e: &quick_xml::events::BytesStart) -> StrokeStyleAttrs {
    StrokeStyleAttrs {
        stroke_alignment: attr(e, b"StrokeAlignment"),
        end_cap: attr(e, b"EndCap"),
        end_join: attr(e, b"EndJoin"),
        miter_limit: attr(e, b"MiterLimit").and_then(|s| s.parse().ok()),
    }
}

/// Rectangle-only corner attributes (`CornerRadius`, `CornerOption`,
/// plus the four per-corner overrides Q-16 added). The per-corner
/// values default to `None`; the renderer falls back to the legacy
/// global pair when a corner spec is empty.
struct CornerAttrs {
    corner_radius: Option<f32>,
    corner_option: Option<String>,
    corners: [CornerSpec; 4],
}

fn read_corner_attrs(e: &quick_xml::events::BytesStart) -> CornerAttrs {
    // Order: [top_left, top_right, bottom_right, bottom_left] —
    // matches the clockwise-from-top-left walk Rectangle::corners
    // documents.
    let per = [
        (b"TopLeftCornerOption".as_ref(), b"TopLeftCornerRadius".as_ref()),
        (b"TopRightCornerOption".as_ref(), b"TopRightCornerRadius".as_ref()),
        (b"BottomRightCornerOption".as_ref(), b"BottomRightCornerRadius".as_ref()),
        (b"BottomLeftCornerOption".as_ref(), b"BottomLeftCornerRadius".as_ref()),
    ];
    let mut corners = [CornerSpec::default(); 4];
    for (i, (oname, rname)) in per.iter().enumerate() {
        corners[i].option = attr(e, oname).as_deref().and_then(CornerOption::from_idml);
        corners[i].radius = attr(e, rname).and_then(|s| s.parse().ok());
    }
    CornerAttrs {
        corner_radius: attr(e, b"CornerRadius").and_then(|s| s.parse().ok()),
        corner_option: attr(e, b"CornerOption"),
        corners,
    }
}

/// Compute the axis-aligned bounding box of a non-empty point set,
/// using only the anchors (control points pull beyond the visible
/// curve and would inflate the bbox).
fn bounds_from_anchors(anchors: &[PathAnchor]) -> Bounds {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for a in anchors {
        let (x, y) = a.anchor;
        if x < min_x {
            min_x = x;
        }
        if x > max_x {
            max_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
    }
    Bounds {
        top: min_y,
        left: min_x,
        bottom: max_y,
        right: max_x,
    }
}

impl Spread {
    pub fn parse(xml: &[u8]) -> Result<Self, ParseError> {
        let mut reader = quick_xml::Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut out = Spread::default();
        // Stack of <Group> ItemTransforms encountered, outermost
        // first. When a frame appears inside one or more groups, its
        // effective spread-space transform is the composition of
        // those group transforms with its own ItemTransform.
        let mut group_transforms: Vec<Option<[f32; 6]>> = Vec::new();
        // Stack of `<Group>` builders parallel to `group_transforms`.
        // Each entry accumulates the group's members + transparency
        // block until the closing tag fires, at which point the
        // builder is finalised into `out.groups`. Sub-groups register
        // themselves with the outer builder once they close, so the
        // outer group's `members` can carry a `FrameRef::Group(idx)`.
        let mut group_builders: Vec<GroupBuilder> = Vec::new();
        let mut current_frame: Option<CurrentFrame> = None;
        // Tracks the rectangle index whose `<GradientFeatherSetting>`
        // is currently open, so nested `<GradientStop>` children can
        // be appended to the right effects bag. Cleared on the
        // matching close tag. `<GradientStop>` is also a child of
        // `<Gradient>` swatches in graphic.rs — those live in a
        // different parser entirely, so the state here can stay
        // scoped to spread.rs.
        let mut current_gradient_feather: Option<CurrentFrameKind> = None;
        // Q-03: state for capturing inline `<Image><Properties><Contents>`
        // base64 CDATA. `Some(frame_kind)` between `<Contents>` start and
        // end while a frame is the active nested context; we append
        // text / cdata events into `current_contents_buf` then
        // base64-decode and stash on the parent shape at end-tag time.
        // `<Contents>` only appears under image-bearing elements in
        // spread.xml so we don't need to filter by parent tag.
        let mut current_image_contents_target: Option<CurrentFrameKind> = None;
        let mut current_contents_buf: Vec<u8> = Vec::new();
        let mut buf = Vec::new();

        // Register a freshly-opened frame with the innermost
        // `<Group>` builder, if one is active. The builder records
        // a `FrameRef` keyed by the frame's index in its backing
        // vec — that index is stable for the rest of the parse
        // (frames never get reordered after creation).
        //
        // Top-level frames (no group active) instead get appended to
        // `out.frames_in_order`, which feeds the renderer's
        // cross-shape z-order sort (Q-10).
        //
        // Registration happens at open time so self-closing
        // `<Rectangle/>` etc. (which fire as `Event::Empty` and
        // never visit the `End` arm) still get recorded. The
        // close handler below unregisters frames that ultimately
        // got dropped for missing bounds.
        fn register_with_group(
            out: &mut Spread,
            group_builders: &mut [GroupBuilder],
            frame_ref: FrameRef,
        ) {
            if let Some(b) = group_builders.last_mut() {
                b.members.push(frame_ref);
            } else {
                out.frames_in_order.push(frame_ref);
            }
        }
        fn unregister_last_in_group(
            out: &mut Spread,
            group_builders: &mut [GroupBuilder],
            expected: FrameRef,
        ) {
            if let Some(b) = group_builders.last_mut() {
                if b.members.last() == Some(&expected) {
                    b.members.pop();
                }
            } else if out.frames_in_order.last() == Some(&expected) {
                out.frames_in_order.pop();
            }
        }

        // Pop the just-closed frame from its backing vec when no
        // bounds were ever supplied (neither GeometricBounds attr
        // nor PathGeometry anchors). Preserves the prior "skip
        // bounds-less frames" behaviour while letting the open-tag
        // path stay simple.
        fn drop_pending(out: &mut Spread, kind: CurrentFrameKind) {
            match kind {
                CurrentFrameKind::Text(i) => {
                    debug_assert_eq!(i + 1, out.text_frames.len());
                    out.text_frames.pop();
                }
                CurrentFrameKind::Rect(i) => {
                    debug_assert_eq!(i + 1, out.rectangles.len());
                    out.rectangles.pop();
                }
                CurrentFrameKind::Oval(i) => {
                    debug_assert_eq!(i + 1, out.ovals.len());
                    out.ovals.pop();
                }
                CurrentFrameKind::Line(i) => {
                    debug_assert_eq!(i + 1, out.graphic_lines.len());
                    out.graphic_lines.pop();
                }
                CurrentFrameKind::Polygon(i) => {
                    debug_assert_eq!(i + 1, out.polygons.len());
                    out.polygons.pop();
                }
            }
        }
        // Apply path-derived bounds to the just-closed frame.
        fn set_pending_bounds(out: &mut Spread, kind: CurrentFrameKind, bounds: Bounds) {
            match kind {
                CurrentFrameKind::Text(i) => out.text_frames[i].bounds = bounds,
                CurrentFrameKind::Rect(i) => out.rectangles[i].bounds = bounds,
                CurrentFrameKind::Oval(i) => out.ovals[i].bounds = bounds,
                CurrentFrameKind::Line(i) => out.graphic_lines[i].bounds = bounds,
                CurrentFrameKind::Polygon(i) => out.polygons[i].bounds = bounds,
            }
        }

        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                    b"Spread" | b"MasterSpread" => {
                        if out.self_id.is_none() {
                            out.self_id = attr(&e, b"Self");
                            out.item_transform =
                                attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s));
                        }
                    }
                    b"Group" => {
                        let t = attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s));
                        group_transforms.push(t);
                        group_builders.push(GroupBuilder {
                            self_id: attr(&e, b"Self"),
                            item_transform: t,
                            members: Vec::new(),
                            transparency: GroupTransparency::default(),
                            stroke_transparency_depth: 0,
                            content_transparency_depth: 0,
                        });
                    }
                    b"Guide" => {
                        // Plan-2 §8.3 ruler guides. Both `<Guide>`
                        // and `<Empty Guide />` variants surface here.
                        // The `Orientation` + `Location` attributes
                        // are required for the guide to mean anything;
                        // unparseable entries get dropped.
                        let orientation = attr(&e, b"Orientation");
                        let location = attr(&e, b"Location").and_then(|s| s.parse::<f32>().ok());
                        if let (Some(orient), Some(loc)) = (orientation, location) {
                            let orient = match orient.as_str() {
                                "Vertical" => Some(GuideOrientation::Vertical),
                                "Horizontal" => Some(GuideOrientation::Horizontal),
                                _ => None,
                            };
                            let page_index = attr(&e, b"PageIndex")
                                .and_then(|s| s.parse::<u32>().ok())
                                .unwrap_or(0);
                            if let Some(orient) = orient {
                                out.guides.push(RulerGuide {
                                    orientation: orient,
                                    location: loc,
                                    page_index,
                                });
                            }
                        }
                    }
                    b"Page" => {
                        if let Some(bounds) =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s))
                        {
                            out.pages.push(Page {
                                self_id: attr(&e, b"Self"),
                                bounds,
                                applied_master: attr(&e, b"AppliedMaster"),
                                item_transform: attr(&e, b"ItemTransform")
                                    .and_then(|s| parse_matrix(&s)),
                                master_page_transform: attr(&e, b"MasterPageTransform")
                                    .and_then(|s| parse_matrix(&s)),
                                override_list: attr(&e, b"OverrideList")
                                    .map(|s| s.split_whitespace().map(str::to_string).collect())
                                    .unwrap_or_default(),
                                name: attr(&e, b"Name"),
                            });
                        }
                    }
                    b"TextFrame" => {
                        let bounds_attr =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                        let common = read_common_attrs(&e);
                        let item_transform =
                            effective_item_transform(&group_transforms, common.item_transform);
                        out.text_frames.push(TextFrame {
                            self_id: common.self_id,
                            parent_story: attr(&e, b"ParentStory"),
                            bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                            item_transform,
                            fill_color: common.fill_color,
                            fill_tint: common.fill_tint,
                            stroke_color: common.stroke_color,
                            stroke_weight: common.stroke_weight,
                            stroke_type: common.stroke_type,
                            drop_shadow: None,
                            stroke_drop_shadow: None,
                            next_text_frame: attr(&e, b"NextTextFrame"),
                            vertical_justification: None,
                            first_baseline_offset: None,
                            minimum_first_baseline_offset: None,
                            inset_spacing: None,
                            auto_sizing: None,
                            auto_sizing_reference_point: None,
                            minimum_width_for_auto_sizing: None,
                            minimum_height_for_auto_sizing: None,
                            use_minimum_height_for_auto_sizing: None,
                            applied_object_style: common.applied_object_style,
                            text_wrap: None,
                            item_layer: common.item_layer,
                            is_anchored: false,
                            opacity: None,
                            blend_mode: None,
                            anchors: Vec::new(),
                            subpath_starts: Vec::new(),
                            subpath_open: Vec::new(),
                            effects: None,
                            gradient_fill_angle: common.gradient_fill_angle,
                            gradient_fill_length: common.gradient_fill_length,
                            gradient_stroke_angle: common.gradient_stroke_angle,
                            gradient_stroke_length: common.gradient_stroke_length,
                            applied_toc_style: attr(&e, b"AppliedTOCStyle"),
                            overprint_fill: common.overprint_fill,
                            overprint_stroke: common.overprint_stroke,
                            nonprinting: common.nonprinting,
                        });
                        let idx = out.text_frames.len() - 1;
                        register_with_group(
                            &mut out,
                            &mut group_builders,
                            FrameRef::TextFrame(idx),
                        );
                        current_frame = Some(CurrentFrame {
                            kind: CurrentFrameKind::Text(idx),
                            needs_bounds: bounds_attr.is_none(),
                            anchors: Vec::new(),
                            subpath_starts: Vec::new(),
                            subpath_open: Vec::new(),
                            // Always retain Bezier path anchors so the
                            // renderer can detect non-rectangular text
                            // frame outlines (triangle, pentagon, …)
                            // and clip layout to the actual polygon
                            // interior rather than the AABB.
                            keep_anchors: true,
                            in_text_wrap: false,
                            stroke_transparency_depth: 0,
                            content_transparency_depth: 0,
                        });
                    }
                    b"Rectangle" => {
                        let bounds_attr =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                        let common = read_common_attrs(&e);
                        let stroke = read_stroke_style_attrs(&e);
                        let corner = read_corner_attrs(&e);
                        let item_transform =
                            effective_item_transform(&group_transforms, common.item_transform);
                        out.rectangles.push(Rectangle {
                            self_id: common.self_id,
                            bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                            item_transform,
                            fill_color: common.fill_color,
                            fill_tint: common.fill_tint,
                            stroke_color: common.stroke_color,
                            stroke_weight: common.stroke_weight,
                            drop_shadow: None,
                            stroke_drop_shadow: None,
                            image_link: None,
                            image_bytes: None,
                            has_image_element: false,
                            has_inline_pdf: false,
                            image_item_transform: None,
                            applied_object_style: common.applied_object_style,
                            text_wrap: None,
                            frame_fitting: None,
                            stroke_type: common.stroke_type,
                            stroke_alignment: stroke.stroke_alignment,
                            end_cap: stroke.end_cap,
                            end_join: stroke.end_join,
                            miter_limit: stroke.miter_limit,
                            item_layer: common.item_layer,
                            corner_radius: corner.corner_radius,
                            corner_option: corner.corner_option,
                            corners: corner.corners,
                            is_anchored: false,
                            opacity: None,
                            blend_mode: None,
                            effects: None,
                            gradient_fill_angle: common.gradient_fill_angle,
                            gradient_fill_length: common.gradient_fill_length,
                            gradient_stroke_angle: common.gradient_stroke_angle,
                            gradient_stroke_length: common.gradient_stroke_length,
                            text_paths: Vec::new(),
                            overprint_fill: common.overprint_fill,
                            overprint_stroke: common.overprint_stroke,
                            nonprinting: common.nonprinting,
                            anchors: Vec::new(),
                            subpath_starts: Vec::new(),
                            subpath_open: Vec::new(),
                        });
                        let idx = out.rectangles.len() - 1;
                        register_with_group(
                            &mut out,
                            &mut group_builders,
                            FrameRef::Rectangle(idx),
                        );
                        current_frame = Some(CurrentFrame {
                            kind: CurrentFrameKind::Rect(idx),
                            needs_bounds: bounds_attr.is_none(),
                            anchors: Vec::new(),
                            subpath_starts: Vec::new(),
                            subpath_open: Vec::new(),
                            // Q-11: retain anchors so stylised
                            // non-rectangular outlines (torn-paper,
                            // multi-anchor) can route through
                            // `Geometry::Polygon` instead of collapsing
                            // to the AABB.
                            keep_anchors: true,
                            in_text_wrap: false,
                            stroke_transparency_depth: 0,
                            content_transparency_depth: 0,
                        });
                    }
                    b"Oval" => {
                        let bounds_attr =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                        let common = read_common_attrs(&e);
                        let item_transform =
                            effective_item_transform(&group_transforms, common.item_transform);
                        out.ovals.push(Oval {
                            self_id: common.self_id,
                            bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                            item_transform,
                            fill_color: common.fill_color,
                            fill_tint: common.fill_tint,
                            stroke_color: common.stroke_color,
                            stroke_weight: common.stroke_weight,
                            stroke_type: common.stroke_type,
                            drop_shadow: None,
                            stroke_drop_shadow: None,
                            applied_object_style: common.applied_object_style,
                            text_wrap: None,
                            item_layer: common.item_layer,
                            gradient_fill_angle: common.gradient_fill_angle,
                            gradient_fill_length: common.gradient_fill_length,
                            gradient_stroke_angle: common.gradient_stroke_angle,
                            gradient_stroke_length: common.gradient_stroke_length,
                            opacity: None,
                            blend_mode: None,
                            image_link: None,
                            image_bytes: None,
                            has_image_element: false,
                            has_inline_pdf: false,
                            image_item_transform: None,
                            effects: None,
                            overprint_fill: common.overprint_fill,
                            overprint_stroke: common.overprint_stroke,
                            nonprinting: common.nonprinting,
                        });
                        let idx = out.ovals.len() - 1;
                        register_with_group(&mut out, &mut group_builders, FrameRef::Oval(idx));
                        current_frame = Some(CurrentFrame {
                            kind: CurrentFrameKind::Oval(idx),
                            needs_bounds: bounds_attr.is_none(),
                            anchors: Vec::new(),
                            subpath_starts: Vec::new(),
                            subpath_open: Vec::new(),
                            keep_anchors: false,
                            in_text_wrap: false,
                            stroke_transparency_depth: 0,
                            content_transparency_depth: 0,
                        });
                    }
                    b"StrokeTransparencySetting" => {
                        // Drop shadows under this wrapper describe a
                        // shadow cast by the frame's stroke — captured
                        // separately so the renderer can gate emission
                        // on stroke visibility.
                        if let Some(cf) = current_frame.as_mut() {
                            cf.stroke_transparency_depth += 1;
                        } else if let Some(b) = group_builders.last_mut() {
                            b.stroke_transparency_depth += 1;
                        }
                    }
                    b"ContentTransparencySetting" => {
                        // Drop shadows under this wrapper describe
                        // content-only shadows that don't map onto our
                        // single-shadow-per-frame model; skipped.
                        if let Some(cf) = current_frame.as_mut() {
                            cf.content_transparency_depth += 1;
                        } else if let Some(b) = group_builders.last_mut() {
                            b.content_transparency_depth += 1;
                        }
                    }
                    b"DropShadowSetting" => {
                        if let Some(setting) = parse_drop_shadow(&e) {
                            // Only "Drop"/"Default" mode results in a
                            // visible shadow. "None" means the shadow
                            // is disabled even though the setting is
                            // serialised.
                            if setting.mode != "None" {
                                if let Some(cf) = current_frame.as_ref() {
                                    if cf.content_transparency_depth > 0 {
                                        // Content-only shadow — skip.
                                    } else if cf.stroke_transparency_depth > 0 {
                                        // Stroke-only shadow — captured for
                                        // conditional emission by the
                                        // renderer.
                                        match cf.kind {
                                            CurrentFrameKind::Text(i) => {
                                                out.text_frames[i].stroke_drop_shadow =
                                                    Some(setting);
                                            }
                                            CurrentFrameKind::Rect(i) => {
                                                out.rectangles[i].stroke_drop_shadow =
                                                    Some(setting);
                                            }
                                            CurrentFrameKind::Oval(i) => {
                                                out.ovals[i].stroke_drop_shadow = Some(setting);
                                            }
                                            CurrentFrameKind::Line(_)
                                            | CurrentFrameKind::Polygon(_) => {
                                                // GraphicLine + Polygon have
                                                // no shadow fields today;
                                                // ignore.
                                            }
                                        }
                                    } else {
                                        match cf.kind {
                                            CurrentFrameKind::Text(i) => {
                                                out.text_frames[i].drop_shadow = Some(setting);
                                            }
                                            CurrentFrameKind::Rect(i) => {
                                                out.rectangles[i].drop_shadow = Some(setting);
                                            }
                                            CurrentFrameKind::Oval(i) => {
                                                out.ovals[i].drop_shadow = Some(setting);
                                            }
                                            CurrentFrameKind::Line(_)
                                            | CurrentFrameKind::Polygon(_) => {
                                                // GraphicLine + Polygon have
                                                // no drop_shadow field today;
                                                // ignore.
                                            }
                                        }
                                    }
                                } else if let Some(b) = group_builders.last_mut() {
                                    // No frame is open but a `<Group>`
                                    // is — route the shadow to the
                                    // innermost group's transparency
                                    // block. Stroke-/content-only
                                    // wrappers around a group don't
                                    // map onto our model and are
                                    // skipped.
                                    if b.content_transparency_depth == 0
                                        && b.stroke_transparency_depth == 0
                                    {
                                        b.transparency.drop_shadow = Some(setting);
                                    }
                                }
                            }
                        }
                    }
                    b"AnchoredObjectSetting" => {
                        // Mark the current frame as an anchored object.
                        // Renderer-side text-flow integration is
                        // queued; today the flag is informational.
                        if let Some(cf) = current_frame.as_ref() {
                            match cf.kind {
                                CurrentFrameKind::Text(i) => {
                                    out.text_frames[i].is_anchored = true;
                                }
                                CurrentFrameKind::Rect(i) => {
                                    out.rectangles[i].is_anchored = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    b"InnerShadowSetting"
                    | b"OuterGlowSetting"
                    | b"InnerGlowSetting"
                    | b"BevelAndEmbossSetting"
                    | b"SatinSetting"
                    | b"FeatherSetting"
                    | b"DirectionalFeatherSetting"
                    | b"GradientFeatherSetting" => {
                        // Surface each effect's parameters onto the
                        // current shape's effects bag, gated on the
                        // `Applied="true"` flag — `Applied="false"` (or
                        // absent) means the user disabled the effect
                        // even though IDML still serialises the settings
                        // for round-trip preservation. Q-04: extended
                        // from Rectangle-only to all five shape kinds.
                        if let Some(kind) = current_frame.as_ref().map(|cf| cf.kind) {
                            let applied = attr(&e, b"Applied")
                                .and_then(|s| s.parse::<bool>().ok())
                                .unwrap_or(false);
                            if !applied {
                                // Effect is present but disabled; skip
                                // the parameter capture entirely so the
                                // renderer doesn't accidentally emit it.
                                continue;
                            }
                            let Some(bag_slot) = effects_slot_mut(&mut out, kind) else {
                                continue;
                            };
                            let bag = bag_slot.get_or_insert_with(Default::default);
                            match e.name().as_ref() {
                                b"InnerShadowSetting" => {
                                    bag.inner_shadow = Some(InnerShadowParams {
                                        x_offset: parse_f(&e, b"XOffset"),
                                        y_offset: parse_f(&e, b"YOffset"),
                                        size: parse_f(&e, b"Size"),
                                        opacity_pct: parse_f(&e, b"Opacity"),
                                        effect_color: attr(&e, b"EffectColor"),
                                        angle_deg: parse_f(&e, b"Angle"),
                                        distance: parse_f(&e, b"Distance"),
                                        choke_pct: parse_f(&e, b"ChokeAmount"),
                                        blend_mode: attr(&e, b"BlendMode"),
                                        noise_pct: parse_f(&e, b"Noise"),
                                    });
                                }
                                b"OuterGlowSetting" => {
                                    bag.outer_glow = Some(OuterGlowParams {
                                        size: parse_f(&e, b"Size"),
                                        opacity_pct: parse_f(&e, b"Opacity"),
                                        effect_color: attr(&e, b"EffectColor"),
                                        spread_pct: parse_f(&e, b"Spread"),
                                        blend_mode: attr(&e, b"BlendMode"),
                                        noise_pct: parse_f(&e, b"Noise"),
                                    });
                                }
                                b"InnerGlowSetting" => {
                                    bag.inner_glow = Some(InnerGlowParams {
                                        size: parse_f(&e, b"Size"),
                                        opacity_pct: parse_f(&e, b"Opacity"),
                                        effect_color: attr(&e, b"EffectColor"),
                                        choke_pct: parse_f(&e, b"ChokeAmount"),
                                        blend_mode: attr(&e, b"BlendMode"),
                                        source: attr(&e, b"Source"),
                                        noise_pct: parse_f(&e, b"Noise"),
                                    });
                                }
                                b"BevelAndEmbossSetting" => {
                                    bag.bevel = Some(BevelEmbossParams {
                                        depth_pct: parse_f(&e, b"Depth"),
                                        size: parse_f(&e, b"Size"),
                                        angle_deg: parse_f(&e, b"Angle"),
                                        altitude_deg: parse_f(&e, b"Altitude"),
                                        highlight_color: attr(&e, b"HighlightColor"),
                                        shadow_color: attr(&e, b"ShadowColor"),
                                        highlight_opacity_pct: parse_f(&e, b"HighlightOpacity"),
                                        shadow_opacity_pct: parse_f(&e, b"ShadowOpacity"),
                                        style: attr(&e, b"Style"),
                                        direction: attr(&e, b"Direction"),
                                        technique: attr(&e, b"Technique"),
                                        soften: parse_f(&e, b"Soften"),
                                    });
                                }
                                b"SatinSetting" => {
                                    bag.satin = Some(SatinParams {
                                        size: parse_f(&e, b"Size"),
                                        angle_deg: parse_f(&e, b"Angle"),
                                        distance: parse_f(&e, b"Distance"),
                                        effect_color: attr(&e, b"EffectColor"),
                                        opacity_pct: parse_f(&e, b"Opacity"),
                                        blend_mode: attr(&e, b"BlendMode"),
                                        invert: attr(&e, b"Invert")
                                            .and_then(|s| s.parse::<bool>().ok()),
                                    });
                                }
                                b"FeatherSetting" => {
                                    bag.feather = Some(FeatherParams {
                                        width: parse_f(&e, b"Width"),
                                        corner_type: attr(&e, b"CornerType"),
                                        noise_pct: parse_f(&e, b"Noise"),
                                        choke_pct: parse_f(&e, b"ChokeAmount"),
                                    });
                                }
                                b"DirectionalFeatherSetting" => {
                                    bag.directional_feather = Some(DirectionalFeatherParams {
                                        left_width: parse_f(&e, b"LeftWidth"),
                                        right_width: parse_f(&e, b"RightWidth"),
                                        top_width: parse_f(&e, b"TopWidth"),
                                        bottom_width: parse_f(&e, b"BottomWidth"),
                                        angle_deg: parse_f(&e, b"Angle"),
                                        noise_pct: parse_f(&e, b"NoiseAmount"),
                                        choke_pct: parse_f(&e, b"ChokeAmount"),
                                        corner_type: attr(&e, b"CornerType"),
                                    });
                                }
                                b"GradientFeatherSetting" => {
                                    // InDesign uses `GradientStart`
                                    // (an "x y" pair) + `Length` +
                                    // `HiliteAngle` to describe the
                                    // gradient direction; the IDML
                                    // spec also accepts an explicit
                                    // `GradientEnd` pair. We accept
                                    // both shapes — the parser
                                    // computes the end point from
                                    // start + (Length × Angle) when
                                    // GradientEnd is missing so the
                                    // renderer sees one canonical
                                    // pair regardless of the source.
                                    let start_point = attr(&e, b"GradientStart")
                                        .as_deref()
                                        .and_then(parse_xy_pair);
                                    let end_point = attr(&e, b"GradientEnd")
                                        .as_deref()
                                        .and_then(parse_xy_pair)
                                        .or_else(|| {
                                            // `HiliteAngle` is the *highlight*
                                            // ramp orientation, not the
                                            // gradient axis direction —
                                            // InDesign uses it for the
                                            // radial-feather hilite preview
                                            // and leaves the gradient axis
                                            // horizontal (0°) when no
                                            // dedicated angle attribute is
                                            // serialised. Tied to the visible
                                            // page-5 yellow→white feather in
                                            // `manual-sample.idml`, where
                                            // `HiliteAngle="-62.2"` paints a
                                            // diagonal smudge instead of the
                                            // expected left→right fade.
                                            let s = start_point?;
                                            let length = parse_f(&e, b"Length")?;
                                            let angle = parse_f(&e, b"GradientAngle")
                                                .or_else(|| parse_f(&e, b"Angle"))
                                                .unwrap_or(0.0);
                                            let rad = angle.to_radians();
                                            let (sin, cos) = rad.sin_cos();
                                            Some((s.0 + length * cos, s.1 - length * sin))
                                        });
                                    bag.gradient_feather = Some(GradientFeatherParams {
                                        gradient_type: attr(&e, b"Type"),
                                        start_point,
                                        end_point,
                                        angle_deg: parse_f(&e, b"GradientAngle")
                                            .or_else(|| parse_f(&e, b"Angle")),
                                        stops: Vec::new(),
                                    });
                                    // Mark the current frame's gradient
                                    // feather as the open target so
                                    // nested `<GradientStop>` /
                                    // `<OpacityGradientStop>` children
                                    // can append to it. Cleared on the
                                    // close tag below. Q-04: tracks
                                    // CurrentFrameKind (not just rect
                                    // index) so non-Rectangle shapes
                                    // can host gradient feathers too.
                                    current_gradient_feather = Some(kind);
                                }
                                _ => {}
                            }
                        }
                    }
                    b"GradientStop" | b"OpacityGradientStop" => {
                        // Children of an open `<GradientFeatherSetting>`
                        // define the alpha falloff. InDesign serialises
                        // them as `<OpacityGradientStop Opacity="..."
                        // Location="..." Midpoint="...">`; the IDML
                        // spec also documents a `<GradientStop StopColor
                        // ="..." Alpha="..." Location="..."
                        // GradientStopMidpoint="...">` form. Both are
                        // accepted — the alpha lands in `alpha_pct`
                        // regardless of which attribute the IDML
                        // actually used.
                        //
                        // `<GradientStop>` is also a child of
                        // `<Gradient>` swatches in graphic.rs; that's
                        // a separate parser file, so the routing here
                        // only fires when a gradient-feather block is
                        // actually open in the spread parser.
                        if let Some(kind) = current_gradient_feather {
                            if let Some(bag) = effects_slot_mut(&mut out, kind)
                                .and_then(|s| s.as_mut())
                            {
                                if let Some(gf) = bag.gradient_feather.as_mut() {
                                    let location_pct = parse_f(&e, b"Location").unwrap_or(0.0);
                                    // `Opacity` (OpacityGradientStop)
                                    // takes precedence; `Alpha`
                                    // (GradientStop spec form) falls
                                    // back; default 100 (fully opaque)
                                    // when neither is set.
                                    let alpha_pct = parse_f(&e, b"Opacity")
                                        .or_else(|| parse_f(&e, b"Alpha"))
                                        .unwrap_or(100.0);
                                    let midpoint_pct =
                                        parse_f(&e, b"GradientStopMidpoint")
                                            .or_else(|| parse_f(&e, b"Midpoint"))
                                            .unwrap_or(50.0);
                                    gf.stops.push(GradientFeatherStop {
                                        stop_color: attr(&e, b"StopColor"),
                                        location_pct,
                                        alpha_pct,
                                        midpoint_pct,
                                    });
                                }
                            }
                        }
                    }
                    b"BlendingSetting" => {
                        // Nested under <TransparencySetting>; we don't
                        // track the wrapper because no other element
                        // shares this name. Opacity is 0..=100;
                        // BlendMode is a string (Normal / Multiply /
                        // Screen / etc).
                        let opacity = attr(&e, b"Opacity").and_then(|s| s.parse::<f32>().ok());
                        let mode = attr(&e, b"BlendMode");
                        if let Some(cf) = current_frame.as_ref() {
                            match cf.kind {
                                CurrentFrameKind::Rect(i) => {
                                    if opacity.is_some() {
                                        out.rectangles[i].opacity = opacity;
                                    }
                                    if mode.is_some() {
                                        out.rectangles[i].blend_mode = mode;
                                    }
                                }
                                CurrentFrameKind::Text(i) => {
                                    if opacity.is_some() {
                                        out.text_frames[i].opacity = opacity;
                                    }
                                    if mode.is_some() {
                                        out.text_frames[i].blend_mode = mode;
                                    }
                                }
                                CurrentFrameKind::Oval(i) => {
                                    if opacity.is_some() {
                                        out.ovals[i].opacity = opacity;
                                    }
                                    if mode.is_some() {
                                        out.ovals[i].blend_mode = mode;
                                    }
                                }
                                CurrentFrameKind::Polygon(i) => {
                                    if opacity.is_some() {
                                        out.polygons[i].opacity = opacity;
                                    }
                                    if mode.is_some() {
                                        out.polygons[i].blend_mode = mode;
                                    }
                                }
                                _ => {
                                    // GraphicLines don't yet surface
                                    // opacity / blend_mode;
                                    // ignore until they do.
                                }
                            }
                        } else if let Some(b) = group_builders.last_mut() {
                            // No frame is open but a `<Group>` is —
                            // route the BlendingSetting to the
                            // innermost group's transparency block so
                            // the renderer can bracket the group's
                            // member range with a single opacity /
                            // blend mode.
                            if opacity.is_some() {
                                b.transparency.opacity = opacity;
                            }
                            if mode.is_some() {
                                b.transparency.blend_mode = mode;
                            }
                        }
                    }
                    b"GeometryPathType" => {
                        // Record the start of a new subpath. IDML's
                        // `<PathGeometry>` may host multiple
                        // `<GeometryPathType>` children to form a
                        // compound path (e.g. a square with a hole);
                        // capturing the boundary lets the renderer
                        // emit one MoveTo/Close per contour rather
                        // than joining them with a straight segment.
                        // We only track this for shapes that retain
                        // anchors (text frames / graphic lines /
                        // polygons); for the others the field is
                        // unused. The companion `PathOpen` flag lifts
                        // here too so the renderer can skip auto-close
                        // on open paths (P-15).
                        if let Some(cf) = current_frame.as_mut() {
                            if cf.keep_anchors {
                                cf.subpath_starts.push(cf.anchors.len());
                                let open = attr(&e, b"PathOpen")
                                    .and_then(|s| s.parse::<bool>().ok())
                                    .unwrap_or(false);
                                cf.subpath_open.push(open);
                            }
                        }
                    }
                    b"PathPointType" => {
                        // Accumulate path-anchor points so the close
                        // tag can derive bounds when no
                        // GeometricBounds attribute was present, and
                        // so polygon rasterisation has the actual
                        // Bezier control points to work with. Real-
                        // world InDesign exports always serialise
                        // geometry this way.
                        if let Some(cf) = current_frame.as_mut() {
                            if cf.needs_bounds || cf.keep_anchors {
                                let anchor = attr(&e, b"Anchor").and_then(|s| parse_xy_pair(&s));
                                if let Some(a) = anchor {
                                    let left = attr(&e, b"LeftDirection")
                                        .and_then(|s| parse_xy_pair(&s))
                                        .unwrap_or(a);
                                    let right = attr(&e, b"RightDirection")
                                        .and_then(|s| parse_xy_pair(&s))
                                        .unwrap_or(a);
                                    cf.anchors.push(PathAnchor {
                                        anchor: a,
                                        left,
                                        right,
                                    });
                                }
                            }
                        }
                    }
                    b"TextWrapPreference" => {
                        // The wrap rect itself comes from the
                        // enclosing shape's geometry; we just record
                        // mode + offsets here. Offsets serialise as
                        // a `TextWrapOffset` child element rather
                        // than attributes, so the actual numbers
                        // arrive a few events later (handled below).
                        if let Some(cf) = current_frame.as_mut() {
                            let mode = attr(&e, b"TextWrapMode")
                                .as_deref()
                                .map(TextWrapMode::from_idml)
                                .unwrap_or(TextWrapMode::None);
                            let kind = cf.kind;
                            let prior_offsets = current_text_wrap_offsets(&out, kind);
                            apply_text_wrap(
                                &mut out,
                                kind,
                                Some(TextWrap {
                                    mode,
                                    offsets: prior_offsets,
                                }),
                            );
                            cf.in_text_wrap = true;
                        }
                    }
                    b"TextWrapOffset" => {
                        if let Some(cf) = current_frame.as_ref() {
                            if cf.in_text_wrap {
                                let offsets = [
                                    attr(&e, b"Top").and_then(|s| s.parse().ok()).unwrap_or(0.0),
                                    attr(&e, b"Left")
                                        .and_then(|s| s.parse().ok())
                                        .unwrap_or(0.0),
                                    attr(&e, b"Bottom")
                                        .and_then(|s| s.parse().ok())
                                        .unwrap_or(0.0),
                                    attr(&e, b"Right")
                                        .and_then(|s| s.parse().ok())
                                        .unwrap_or(0.0),
                                ];
                                set_text_wrap_offsets(&mut out, cf.kind, offsets);
                            }
                        }
                    }
                    b"TextFramePreference" => {
                        if let Some(CurrentFrameKind::Text(i)) =
                            current_frame.as_ref().map(|cf| cf.kind)
                        {
                            let f = &mut out.text_frames[i];
                            if let Some(vj) = attr(&e, b"VerticalJustification")
                                .as_deref()
                                .and_then(VerticalJustification::from_idml)
                            {
                                f.vertical_justification = Some(vj);
                            }
                            if let Some(fbo) = attr(&e, b"FirstBaselineOffset")
                                .as_deref()
                                .and_then(FirstBaselineOffset::from_idml)
                            {
                                f.first_baseline_offset = Some(fbo);
                            }
                            if let Some(min_fbo) = attr(&e, b"MinimumFirstBaselineOffset")
                                .and_then(|s| s.parse::<f32>().ok())
                            {
                                f.minimum_first_baseline_offset = Some(min_fbo);
                            }
                            if let Some(insets) =
                                attr(&e, b"InsetSpacing").and_then(|s| parse_insets(&s))
                            {
                                f.inset_spacing = Some(insets);
                            }
                            if let Some(at) = attr(&e, b"AutoSizingType")
                                .as_deref()
                                .and_then(AutoSizingType::from_idml)
                            {
                                f.auto_sizing = Some(at);
                            }
                            if let Some(rp) = attr(&e, b"AutoSizingReferencePoint")
                                .as_deref()
                                .and_then(AutoSizingReferencePoint::from_idml)
                            {
                                f.auto_sizing_reference_point = Some(rp);
                            }
                            if let Some(min_w) = attr(&e, b"MinimumWidthForAutoSizing")
                                .and_then(|s| s.parse::<f32>().ok())
                            {
                                f.minimum_width_for_auto_sizing = Some(min_w);
                            }
                            if let Some(min_h) = attr(&e, b"MinimumHeightForAutoSizing")
                                .and_then(|s| s.parse::<f32>().ok())
                            {
                                f.minimum_height_for_auto_sizing = Some(min_h);
                            }
                            if let Some(use_min_h) = attr(&e, b"UseMinimumHeightForAutoSizing")
                                .and_then(|s| s.parse::<bool>().ok())
                            {
                                f.use_minimum_height_for_auto_sizing = Some(use_min_h);
                            }
                        }
                    }
                    b"Image" | b"EPSImage" | b"PDF" | b"ImportedPage" | b"Link" => {
                        // IDML's image-bearing frame nests an
                        // <Image> with a LinkResourceURI on the
                        // element itself or on its <Link> child.
                        // Both Rectangle and Polygon may host placed
                        // images; routing here dispatches on the
                        // open frame's kind.
                        //
                        // The image-element tags (Image / EPSImage /
                        // PDF / ImportedPage) also flip
                        // `has_image_element` so the renderer can
                        // distinguish a plain colour swatch from an
                        // image frame whose link failed to resolve
                        // (Envato template placeholders) and stamp
                        // InDesign's missing-image placeholder
                        // instead of falling back to raw fill.
                        let is_image_element =
                            !matches!(e.name().as_ref(), b"Link");
                        let is_pdf_element = matches!(e.name().as_ref(), b"PDF");
                        let element_uri =
                            attr(&e, b"LinkResourceURI").or_else(|| attr(&e, b"href"));
                        // Q-06: a `<PDF>` element with no link URI carries
                        // its content as inline `<Contents>` CDATA we can't
                        // decode. Flag it so the renderer renders the
                        // frame's intrinsic FillColor instead of the
                        // missing-image grey-X placeholder.
                        let inline_pdf = is_pdf_element && element_uri.is_none();
                        match current_frame.as_ref().map(|cf| cf.kind) {
                            Some(CurrentFrameKind::Rect(i)) => {
                                if is_image_element {
                                    out.rectangles[i].has_image_element = true;
                                }
                                if inline_pdf {
                                    out.rectangles[i].has_inline_pdf = true;
                                }
                                if let Some(uri) = element_uri {
                                    // First-write-wins so the outer <Image>
                                    // attribute beats the inner <Link>'s.
                                    if out.rectangles[i].image_link.is_none() {
                                        out.rectangles[i].image_link = Some(uri);
                                    }
                                }
                                if e.name().as_ref() == b"Image" {
                                    if let Some(m) = attr(&e, b"ItemTransform")
                                        .and_then(|s| parse_matrix(&s))
                                    {
                                        if out.rectangles[i].image_item_transform.is_none() {
                                            out.rectangles[i].image_item_transform = Some(m);
                                        }
                                    }
                                }
                            }
                            Some(CurrentFrameKind::Polygon(i)) => {
                                if is_image_element {
                                    out.polygons[i].has_image_element = true;
                                }
                                if inline_pdf {
                                    out.polygons[i].has_inline_pdf = true;
                                }
                                if let Some(uri) = element_uri {
                                    if out.polygons[i].image_link.is_none() {
                                        out.polygons[i].image_link = Some(uri);
                                    }
                                }
                                if e.name().as_ref() == b"Image" {
                                    if let Some(m) = attr(&e, b"ItemTransform")
                                        .and_then(|s| parse_matrix(&s))
                                    {
                                        if out.polygons[i].image_item_transform.is_none() {
                                            out.polygons[i].image_item_transform = Some(m);
                                        }
                                    }
                                }
                            }
                            Some(CurrentFrameKind::Oval(i)) => {
                                if is_image_element {
                                    out.ovals[i].has_image_element = true;
                                }
                                if inline_pdf {
                                    out.ovals[i].has_inline_pdf = true;
                                }
                                if let Some(uri) = element_uri {
                                    if out.ovals[i].image_link.is_none() {
                                        out.ovals[i].image_link = Some(uri);
                                    }
                                }
                                if e.name().as_ref() == b"Image" {
                                    if let Some(m) = attr(&e, b"ItemTransform")
                                        .and_then(|s| parse_matrix(&s))
                                    {
                                        if out.ovals[i].image_item_transform.is_none() {
                                            out.ovals[i].image_item_transform = Some(m);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    b"Contents" => {
                        // Q-03: enter the inline-image base64 capture
                        // path when we're nested inside a frame.
                        // `<Contents>` only appears under image-bearing
                        // tags in spread.xml so this branch is safe
                        // without a parent-tag filter.
                        if let Some(kind) = current_frame.as_ref().map(|cf| cf.kind) {
                            current_image_contents_target = Some(kind);
                            current_contents_buf.clear();
                        }
                    }
                    b"FrameFittingOption" => {
                        // Attaches to the current Rectangle. Crops are
                        // signed pt offsets — negative values grow the
                        // image past the frame edge for FillProportionally
                        // fits.
                        if let Some(CurrentFrameKind::Rect(i)) =
                            current_frame.as_ref().map(|cf| cf.kind)
                        {
                            out.rectangles[i].frame_fitting = Some(FrameFittingOption {
                                left_crop: attr(&e, b"LeftCrop").and_then(|s| s.parse().ok()),
                                top_crop: attr(&e, b"TopCrop").and_then(|s| s.parse().ok()),
                                right_crop: attr(&e, b"RightCrop").and_then(|s| s.parse().ok()),
                                bottom_crop: attr(&e, b"BottomCrop").and_then(|s| s.parse().ok()),
                                fitting_on_empty_frame: attr(&e, b"FittingOnEmptyFrame"),
                            });
                        }
                    }
                    b"TextPath" => {
                        // `<TextPath>` attaches a story to the current
                        // shape's path (Polygon / Rectangle /
                        // GraphicLine). The shape's own
                        // `<PathGeometry>` provides the curve geometry;
                        // we only record the story reference plus a
                        // few alignment knobs here.
                        if let (Some(cf), Some(parent_story)) =
                            (current_frame.as_ref(), attr(&e, b"ParentStory"))
                        {
                            let tp = TextPath {
                                self_id: attr(&e, b"Self"),
                                parent_story,
                                path_alignment: attr(&e, b"PathAlignment"),
                                path_effect: attr(&e, b"PathEffect"),
                                flip_path_effect: attr(&e, b"FlipPathEffect"),
                                start_bracket: attr(&e, b"StartBracket")
                                    .and_then(|s| s.parse().ok()),
                                end_bracket: attr(&e, b"EndBracket")
                                    .and_then(|s| s.parse().ok()),
                            };
                            match cf.kind {
                                CurrentFrameKind::Polygon(i) => {
                                    out.polygons[i].text_paths.push(tp);
                                }
                                CurrentFrameKind::Rect(i) => {
                                    out.rectangles[i].text_paths.push(tp);
                                }
                                CurrentFrameKind::Line(i) => {
                                    out.graphic_lines[i].text_paths.push(tp);
                                }
                                // Oval / TextFrame don't host TextPath
                                // in the IDML schema; ignore if seen.
                                _ => {}
                            }
                        }
                    }
                    b"GraphicLine" => {
                        let bounds_attr =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                        let common = read_common_attrs(&e);
                        let item_transform =
                            effective_item_transform(&group_transforms, common.item_transform);
                        out.graphic_lines.push(GraphicLine {
                            self_id: common.self_id,
                            bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                            item_transform,
                            stroke_color: common.stroke_color,
                            stroke_weight: common.stroke_weight,
                            stroke_type: common.stroke_type,
                            applied_object_style: common.applied_object_style,
                            text_wrap: None,
                            item_layer: common.item_layer,
                            anchors: Vec::new(),
                            subpath_starts: Vec::new(),
                            subpath_open: Vec::new(),
                            text_paths: Vec::new(),
                            effects: None,
                            overprint_stroke: common.overprint_stroke,
                            nonprinting: common.nonprinting,
                        });
                        let idx = out.graphic_lines.len() - 1;
                        register_with_group(
                            &mut out,
                            &mut group_builders,
                            FrameRef::GraphicLine(idx),
                        );
                        current_frame = Some(CurrentFrame {
                            kind: CurrentFrameKind::Line(idx),
                            needs_bounds: bounds_attr.is_none(),
                            anchors: Vec::new(),
                            subpath_starts: Vec::new(),
                            subpath_open: Vec::new(),
                            // Always retain Bezier path anchors for
                            // graphic lines so a child <TextPath> can
                            // flow text along the actual stroke.
                            keep_anchors: true,
                            in_text_wrap: false,
                            stroke_transparency_depth: 0,
                            content_transparency_depth: 0,
                        });
                    }
                    b"Polygon" => {
                        let bounds_attr =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                        let common = read_common_attrs(&e);
                        let item_transform =
                            effective_item_transform(&group_transforms, common.item_transform);
                        out.polygons.push(Polygon {
                            self_id: common.self_id,
                            bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                            item_transform,
                            fill_color: common.fill_color,
                            fill_tint: common.fill_tint,
                            stroke_color: common.stroke_color,
                            stroke_weight: common.stroke_weight,
                            stroke_type: common.stroke_type,
                            applied_object_style: common.applied_object_style,
                            text_wrap: None,
                            anchors: Vec::new(),
                            subpath_starts: Vec::new(),
                            subpath_open: Vec::new(),
                            item_layer: common.item_layer,
                            gradient_fill_angle: common.gradient_fill_angle,
                            gradient_fill_length: common.gradient_fill_length,
                            gradient_stroke_angle: common.gradient_stroke_angle,
                            gradient_stroke_length: common.gradient_stroke_length,
                            opacity: None,
                            blend_mode: None,
                            text_paths: Vec::new(),
                            image_link: None,
                            image_bytes: None,
                            has_image_element: false,
                            has_inline_pdf: false,
                            image_item_transform: None,
                            effects: None,
                            overprint_fill: common.overprint_fill,
                            overprint_stroke: common.overprint_stroke,
                            nonprinting: common.nonprinting,
                        });
                        let idx = out.polygons.len() - 1;
                        register_with_group(&mut out, &mut group_builders, FrameRef::Polygon(idx));
                        current_frame = Some(CurrentFrame {
                            kind: CurrentFrameKind::Polygon(idx),
                            needs_bounds: bounds_attr.is_none(),
                            anchors: Vec::new(),
                            subpath_starts: Vec::new(),
                            subpath_open: Vec::new(),
                            // Always retain Bezier path anchors for
                            // polygons so the renderer can emit a
                            // FillPath instead of a bbox FillRect.
                            keep_anchors: true,
                            in_text_wrap: false,
                            stroke_transparency_depth: 0,
                            content_transparency_depth: 0,
                        });
                    }
                    _ => {}
                },
                Event::End(e) => match e.name().as_ref() {
                    b"Group" if !group_transforms.is_empty() => {
                        group_transforms.pop();
                        if let Some(builder) = group_builders.pop() {
                            let group = Group {
                                self_id: builder.self_id,
                                item_transform: builder.item_transform,
                                members: builder.members,
                                transparency: builder.transparency,
                            };
                            let group_idx = out.groups.len();
                            out.groups.push(group);
                            // Register this sub-group with the
                            // enclosing group, if any, so the
                            // outer's `members` list captures
                            // sub-groups in document order. Top-level
                            // groups (no outer) surface in
                            // `frames_in_order` so the renderer's
                            // cross-shape z-sort sees them once at
                            // their XML position.
                            if let Some(outer) = group_builders.last_mut() {
                                outer.members.push(FrameRef::Group(group_idx));
                            } else {
                                out.frames_in_order.push(FrameRef::Group(group_idx));
                            }
                        }
                    }
                    b"TextFrame" | b"Rectangle" | b"Oval" | b"GraphicLine" | b"Polygon" => {
                        // Finalize bounds from accumulated path
                        // anchors when no GeometricBounds attribute
                        // was present. If neither source produced
                        // geometry, drop the placeholder frame so
                        // downstream code never sees a zero-rect
                        // ghost (matches the previous behaviour of
                        // skipping bounds-less shapes).
                        if let Some(cf) = current_frame.take() {
                            if cf.needs_bounds {
                                if cf.anchors.is_empty() {
                                    drop_pending(&mut out, cf.kind);
                                    // The frame was registered with
                                    // the open group at open time;
                                    // unregister now that it has been
                                    // discarded so the group's member
                                    // list never points to a stale
                                    // frame index.
                                    let frame_ref = match cf.kind {
                                        CurrentFrameKind::Text(i) => FrameRef::TextFrame(i),
                                        CurrentFrameKind::Rect(i) => FrameRef::Rectangle(i),
                                        CurrentFrameKind::Oval(i) => FrameRef::Oval(i),
                                        CurrentFrameKind::Line(i) => FrameRef::GraphicLine(i),
                                        CurrentFrameKind::Polygon(i) => FrameRef::Polygon(i),
                                    };
                                    unregister_last_in_group(&mut out, &mut group_builders, frame_ref);
                                } else {
                                    set_pending_bounds(
                                        &mut out,
                                        cf.kind,
                                        bounds_from_anchors(&cf.anchors),
                                    );
                                }
                            }
                            // Polygons keep the curved-path data
                            // even when GeometricBounds was set, so
                            // the renderer can rasterise the actual
                            // outline. GraphicLines keep them too so a
                            // child <TextPath> can flow text along the
                            // actual stroke (curved or multi-segment).
                            if cf.keep_anchors && !cf.anchors.is_empty() {
                                // Drop spurious subpath markers — a
                                // subpath start at the very end of
                                // the anchor list points to nothing,
                                // and the canonical single-contour
                                // case is encoded as `[]` (so callers
                                // can keep using the slice as-is).
                                // `subpath_open` stays parallel to
                                // `subpath_starts`, so when we either
                                // empty or shorten the latter we mirror
                                // the truncation here (P-15).
                                let (subpath_starts, subpath_open) = {
                                    let mut starts = cf.subpath_starts.clone();
                                    let mut opens = cf.subpath_open.clone();
                                    // Keep the indices that point at a
                                    // real anchor; trim the parallel
                                    // open flags by index so the two
                                    // arrays stay in step.
                                    let mut keep = vec![true; starts.len()];
                                    for (k, &s) in starts.iter().enumerate() {
                                        if s >= cf.anchors.len() {
                                            keep[k] = false;
                                        }
                                    }
                                    let mut filtered_starts = Vec::with_capacity(starts.len());
                                    let mut filtered_open = Vec::with_capacity(opens.len());
                                    for k in 0..starts.len() {
                                        if keep[k] {
                                            filtered_starts.push(starts[k]);
                                            filtered_open.push(opens.get(k).copied().unwrap_or(false));
                                        }
                                    }
                                    starts = filtered_starts;
                                    opens = filtered_open;
                                    if starts.len() <= 1 {
                                        // The legacy canonical form for
                                        // a single contour. Surface the
                                        // open flag onto a 1-element vec
                                        // so the renderer can still see
                                        // an open single contour.
                                        let lone_open = opens.first().copied().unwrap_or(false);
                                        if lone_open {
                                            (Vec::new(), vec![true])
                                        } else {
                                            (Vec::new(), Vec::new())
                                        }
                                    } else {
                                        (starts, opens)
                                    }
                                };
                                match cf.kind {
                                    CurrentFrameKind::Polygon(i)
                                        if i < out.polygons.len() =>
                                    {
                                        out.polygons[i].anchors = cf.anchors;
                                        out.polygons[i].subpath_starts = subpath_starts;
                                        out.polygons[i].subpath_open = subpath_open;
                                    }
                                    CurrentFrameKind::Line(i)
                                        if i < out.graphic_lines.len() =>
                                    {
                                        out.graphic_lines[i].anchors = cf.anchors;
                                        out.graphic_lines[i].subpath_starts = subpath_starts;
                                        out.graphic_lines[i].subpath_open = subpath_open;
                                    }
                                    CurrentFrameKind::Text(i)
                                        if i < out.text_frames.len() =>
                                    {
                                        out.text_frames[i].anchors = cf.anchors;
                                        out.text_frames[i].subpath_starts = subpath_starts;
                                        out.text_frames[i].subpath_open = subpath_open;
                                    }
                                    CurrentFrameKind::Rect(i)
                                        if i < out.rectangles.len() =>
                                    {
                                        // Q-11: only stash when the
                                        // outline is non-rectangular
                                        // (>4 anchors). A plain 4-corner
                                        // AABB is the existing default
                                        // and skipping the stash here
                                        // keeps `from_rectangle`'s
                                        // legacy `Geometry::Rect` path.
                                        if cf.anchors.len() > 4 {
                                            out.rectangles[i].anchors = cf.anchors;
                                            out.rectangles[i].subpath_starts = subpath_starts;
                                            out.rectangles[i].subpath_open = subpath_open;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    b"TextWrapPreference" => {
                        if let Some(cf) = current_frame.as_mut() {
                            cf.in_text_wrap = false;
                        }
                    }
                    b"StrokeTransparencySetting" => {
                        if let Some(cf) = current_frame.as_mut() {
                            if cf.stroke_transparency_depth > 0 {
                                cf.stroke_transparency_depth -= 1;
                            }
                        } else if let Some(b) = group_builders.last_mut() {
                            if b.stroke_transparency_depth > 0 {
                                b.stroke_transparency_depth -= 1;
                            }
                        }
                    }
                    b"ContentTransparencySetting" => {
                        if let Some(cf) = current_frame.as_mut() {
                            if cf.content_transparency_depth > 0 {
                                cf.content_transparency_depth -= 1;
                            }
                        } else if let Some(b) = group_builders.last_mut() {
                            if b.content_transparency_depth > 0 {
                                b.content_transparency_depth -= 1;
                            }
                        }
                    }
                    b"GradientFeatherSetting" => {
                        // Close the gradient-feather scope so any
                        // later `<GradientStop>` (e.g. inside a
                        // `<Gradient>` swatch parsed in graphic.rs
                        // — different file, but defensive here)
                        // doesn't accidentally route to this rect.
                        current_gradient_feather = None;
                    }
                    b"Contents" => {
                        // Q-03: close the inline-image base64 capture.
                        // Decode and stash on the parent shape; clear
                        // state so a later sibling can't accidentally
                        // route into the same buffer.
                        if let Some(kind) = current_image_contents_target.take() {
                            let decoded = decode_image_contents_base64(&current_contents_buf);
                            current_contents_buf.clear();
                            if let Some(bytes) = decoded {
                                set_image_bytes(&mut out, kind, bytes);
                            }
                        }
                    }
                    _ => {}
                },
                Event::Text(t) if current_image_contents_target.is_some() => {
                    // base64 CDATA can also arrive as Text events
                    // (whitespace-padded between tags). Trim during
                    // decode rather than at capture time.
                    current_contents_buf.extend_from_slice(t.as_ref());
                }
                Event::CData(t) if current_image_contents_target.is_some() => {
                    current_contents_buf.extend_from_slice(t.as_ref());
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }
}

fn parse_bounds(s: &str) -> Option<Bounds> {
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() != 4 {
        return None;
    }
    Some(Bounds {
        top: parts[0],
        left: parts[1],
        bottom: parts[2],
        right: parts[3],
    })
}

/// Parse an "x y" pair from an IDML attribute (Anchor /
/// LeftDirection / RightDirection / etc.). IDML serialises 2D
/// coordinates as two whitespace-separated f32s.
fn parse_xy_pair(s: &str) -> Option<(f32, f32)> {
    let mut it = s.split_whitespace();
    let x: f32 = it.next()?.parse().ok()?;
    let y: f32 = it.next()?.parse().ok()?;
    Some((x, y))
}

fn parse_drop_shadow(e: &quick_xml::events::BytesStart) -> Option<DropShadowSetting> {
    // IDML defaults — see §IDML Defaults Table 84 in the spec:
    // Mode=None, BlendMode=Multiply, Opacity=75, XOffset=7, YOffset=7,
    // Size=5, EffectColor="n" (Black). When `Mode="Drop"` is the only
    // attribute on the element, these are the values InDesign uses for
    // the unspecified ones. Earlier behaviour treated missing offsets
    // / size as zero, which produced a solid black stamp behind the
    // frame instead of a real drop shadow.
    Some(DropShadowSetting {
        mode: attr(e, b"Mode").unwrap_or_else(|| "Drop".to_string()),
        x_offset: attr(e, b"XOffset")
            .and_then(|s| s.parse().ok())
            .unwrap_or(7.0),
        y_offset: attr(e, b"YOffset")
            .and_then(|s| s.parse().ok())
            .unwrap_or(7.0),
        size: attr(e, b"Size").and_then(|s| s.parse().ok()).unwrap_or(5.0),
        opacity_pct: attr(e, b"Opacity")
            .and_then(|s| s.parse().ok())
            .unwrap_or(75.0),
        effect_color: attr(e, b"EffectColor"),
    })
}

/// IDML's `InsetSpacing` is four whitespace-separated numbers in
/// pt — top, left, bottom, right. Returns `None` if the count is
/// off; the renderer falls back to zero insets.
fn parse_insets(s: &str) -> Option<[f32; 4]> {
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    (parts.len() == 4).then(|| [parts[0], parts[1], parts[2], parts[3]])
}

/// Q-03: decode the base64 CDATA payload of `<Image><Properties>
/// <Contents>` into the original image bytes. The CDATA is standard
/// RFC 4648 base64 with arbitrary whitespace (newlines, spaces) so
/// strip those before decoding. Returns `None` on malformed input
/// rather than panicking — the caller falls back to "no inline
/// bytes" and the renderer's missing-image path takes over.
fn decode_image_contents_base64(raw: &[u8]) -> Option<Vec<u8>> {
    use base64::Engine;
    // Strip whitespace in place into a scratch buffer. The XML
    // serializer pretty-prints the base64 payload across many lines
    // (typically 76-char wraps); base64's STANDARD engine rejects
    // any whitespace, so we have to clean first.
    let mut cleaned: Vec<u8> = Vec::with_capacity(raw.len());
    for &b in raw {
        if !matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
            cleaned.push(b);
        }
    }
    base64::engine::general_purpose::STANDARD.decode(&cleaned).ok()
}

/// Q-04: borrow the effects bag slot for any frame kind. Returns
/// `None` only when the kind's index is out of bounds (defensive —
/// the parser shouldn't reach this state). Centralises the per-shape
/// dispatch so the effect-routing block doesn't fan into five copies.
fn effects_slot_mut(out: &mut Spread, kind: CurrentFrameKind) -> Option<&mut Option<FrameEffects>> {
    match kind {
        CurrentFrameKind::Text(i) => out.text_frames.get_mut(i).map(|f| &mut f.effects),
        CurrentFrameKind::Rect(i) => out.rectangles.get_mut(i).map(|f| &mut f.effects),
        CurrentFrameKind::Oval(i) => out.ovals.get_mut(i).map(|f| &mut f.effects),
        CurrentFrameKind::Line(i) => out.graphic_lines.get_mut(i).map(|f| &mut f.effects),
        CurrentFrameKind::Polygon(i) => out.polygons.get_mut(i).map(|f| &mut f.effects),
    }
}

/// Q-03: stash decoded image bytes on the frame the just-closed
/// `<Contents>` element was nested under. Centralised here so the
/// per-shape match doesn't clutter the parser's main loop.
fn set_image_bytes(out: &mut Spread, kind: CurrentFrameKind, bytes: Vec<u8>) {
    match kind {
        CurrentFrameKind::Rect(i) if i < out.rectangles.len() => {
            out.rectangles[i].image_bytes = Some(bytes);
        }
        CurrentFrameKind::Oval(i) if i < out.ovals.len() => {
            out.ovals[i].image_bytes = Some(bytes);
        }
        CurrentFrameKind::Polygon(i) if i < out.polygons.len() => {
            out.polygons[i].image_bytes = Some(bytes);
        }
        // TextFrame / GraphicLine don't carry image_bytes — IDML
        // doesn't put `<Image>` children under them.
        _ => {}
    }
}

fn parse_matrix(s: &str) -> Option<[f32; 6]> {
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() != 6 {
        return None;
    }
    Some([parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]])
}

/// Compose two affine matrices `a ∘ b`: applying the result to a
/// point is equivalent to applying `b` first then `a`. Matches
/// `paged_compose::Transform::compose` so the parser and the
/// renderer agree on composition order.
fn compose_matrix(a: &[f32; 6], b: &[f32; 6]) -> [f32; 6] {
    let [a1, b1, c1, d1, tx1, ty1] = *a;
    let [a2, b2, c2, d2, tx2, ty2] = *b;
    [
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * tx2 + c1 * ty2 + tx1,
        b1 * tx2 + d1 * ty2 + ty1,
    ]
}

/// Resolve the effective `ItemTransform` for a frame nested inside
/// zero or more groups: outer groups apply first, then inner groups,
/// then the frame's own ItemTransform. `None` for every input
/// short-circuits to `None` so axis-aligned frames keep an empty
/// transform field.
fn effective_item_transform(
    group_stack: &[Option<[f32; 6]>],
    own: Option<[f32; 6]>,
) -> Option<[f32; 6]> {
    let mut acc: Option<[f32; 6]> = None;
    for g in group_stack {
        match (acc, g) {
            (None, Some(m)) => acc = Some(*m),
            (Some(a), Some(m)) => acc = Some(compose_matrix(&a, m)),
            (acc_, None) => acc = acc_,
        }
    }
    match (acc, own) {
        (None, x) => x,
        (Some(a), None) => Some(a),
        (Some(a), Some(o)) => Some(compose_matrix(&a, &o)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_PAGE_SPREAD: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="spread1">
    <Page Self="p1" GeometricBounds="0 0 792 612"/>
    <Page Self="p2" GeometricBounds="0 612 792 1224"/>
    <TextFrame Self="frame1" ParentStory="u10"
               GeometricBounds="72 72 720 540"
               ItemTransform="1 0 0 1 0 0"/>
    <TextFrame Self="frame2" ParentStory="u20"
               GeometricBounds="100 700 300 1100"/>
  </Spread>
</idPkg:Spread>"#;

    #[test]
    fn parses_ruler_guides() {
        // Mix of vertical + horizontal guides on a 2-page spread.
        // Both `<Guide>` and self-closing variants accepted.
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="spread1">
    <Page Self="p1" GeometricBounds="0 0 792 612"/>
    <Guide Self="g1" Orientation="Vertical" Location="120.5" PageIndex="0"/>
    <Guide Self="g2" Orientation="Horizontal" Location="240" PageIndex="1"/>
    <Guide Self="g3" Orientation="Bogus" Location="50"/>
  </Spread>
</idPkg:Spread>"#;
        let s = Spread::parse(xml.as_bytes()).unwrap();
        assert_eq!(s.guides.len(), 2, "bogus orientation should be dropped");
        assert!(matches!(
            s.guides[0].orientation,
            GuideOrientation::Vertical
        ));
        assert!((s.guides[0].location - 120.5).abs() < 1e-3);
        assert_eq!(s.guides[0].page_index, 0);
        assert!(matches!(
            s.guides[1].orientation,
            GuideOrientation::Horizontal
        ));
        assert!((s.guides[1].location - 240.0).abs() < 1e-3);
        assert_eq!(s.guides[1].page_index, 1);
    }

    #[test]
    fn parses_pages_and_frames() {
        let s = Spread::parse(TWO_PAGE_SPREAD).unwrap();
        assert_eq!(s.self_id.as_deref(), Some("spread1"));
        assert_eq!(s.pages.len(), 2);
        assert_eq!(s.pages[0].self_id.as_deref(), Some("p1"));
        assert_eq!(s.pages[0].bounds.width(), 612.0);
        assert_eq!(s.pages[0].bounds.height(), 792.0);

        assert_eq!(s.text_frames.len(), 2);
        assert_eq!(s.text_frames[0].self_id.as_deref(), Some("frame1"));
        assert_eq!(s.text_frames[0].parent_story.as_deref(), Some("u10"));
        assert_eq!(s.text_frames[0].bounds.width(), 468.0);
        assert_eq!(s.text_frames[0].bounds.height(), 648.0);
        assert_eq!(
            s.text_frames[0].item_transform,
            Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        );
        assert_eq!(s.text_frames[1].item_transform, None);
    }

    #[test]
    fn lifts_frames_out_of_groups_with_composed_transform() {
        // Two levels of nesting: outer group translates by (10, 20),
        // inner group translates by (3, 4), inner frame has its own
        // ItemTransform translating by (100, 200). Expected effective
        // transform: outer ∘ inner ∘ frame = translate(113, 224).
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="top" ParentStory="u1" GeometricBounds="0 0 100 200"/>
            <Group ItemTransform="1 0 0 1 10 20">
              <Group ItemTransform="1 0 0 1 3 4">
                <TextFrame Self="inner" ParentStory="u2"
                           GeometricBounds="0 0 50 50"
                           ItemTransform="1 0 0 1 100 200"/>
              </Group>
            </Group>
            <TextFrame Self="after" ParentStory="u3" GeometricBounds="0 0 100 200"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.text_frames.len(), 3, "all frames lifted out of groups");
        assert_eq!(s.skipped_nested_frames, 0);
        assert_eq!(s.text_frames[0].self_id.as_deref(), Some("top"));
        assert_eq!(s.text_frames[1].self_id.as_deref(), Some("inner"));
        assert_eq!(s.text_frames[2].self_id.as_deref(), Some("after"));
        // outer translation (10, 20) + inner translation (3, 4) +
        // frame's own (100, 200) = translation (113, 224); the linear
        // part stays identity since every transform is pure
        // translation.
        let m = s.text_frames[1].item_transform.expect("composed");
        assert!((m[0] - 1.0).abs() < 1e-4 && (m[3] - 1.0).abs() < 1e-4);
        assert!(m[1].abs() < 1e-4 && m[2].abs() < 1e-4);
        assert!((m[4] - 113.0).abs() < 1e-4, "tx = {}", m[4]);
        assert!((m[5] - 224.0).abs() < 1e-4, "ty = {}", m[5]);
    }

    #[test]
    fn parses_text_frame_preference_inset_and_first_baseline() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1" GeometricBounds="0 0 200 300">
              <Properties/>
              <TextFramePreference VerticalJustification="CenterAlign"
                                   FirstBaselineOffset="CapHeight"
                                   MinimumFirstBaselineOffset="14"
                                   InsetSpacing="6 8 10 12"/>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        let f = &s.text_frames[0];
        assert_eq!(
            f.vertical_justification,
            Some(VerticalJustification::Center)
        );
        assert_eq!(
            f.first_baseline_offset,
            Some(FirstBaselineOffset::CapHeight)
        );
        assert_eq!(f.minimum_first_baseline_offset, Some(14.0));
        assert_eq!(f.inset_spacing, Some([6.0, 8.0, 10.0, 12.0]));
    }

    #[test]
    fn q16_parses_per_corner_options() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r" GeometricBounds="0 0 100 200"
                       CornerOption="RoundedCorner" CornerRadius="0"
                       TopLeftCornerOption="None" TopLeftCornerRadius="0"
                       TopRightCornerOption="None" TopRightCornerRadius="0"
                       BottomRightCornerOption="None" BottomRightCornerRadius="0"
                       BottomLeftCornerOption="RoundedCorner" BottomLeftCornerRadius="19.84"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        let r = &s.rectangles[0];
        // Top-left + top-right + bottom-right squared off explicitly.
        assert_eq!(r.corners[0].option, Some(CornerOption::None));
        assert_eq!(r.corners[1].option, Some(CornerOption::None));
        assert_eq!(r.corners[2].option, Some(CornerOption::None));
        // Bottom-left rounded with explicit radius.
        assert_eq!(r.corners[3].option, Some(CornerOption::Rounded));
        assert_eq!(r.corners[3].radius, Some(19.84));
        assert!(r.corners[3].option.unwrap().rounds());
    }

    #[test]
    fn q03_parses_inline_image_contents_base64() {
        // "Hello, IDML!" base64-encoded → the bytes round-trip.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 50 50">
              <Image>
                <Properties>
                  <Contents><![CDATA[SGVsbG8sIElETUwh]]></Contents>
                </Properties>
              </Image>
            </Rectangle>
            <Rectangle Self="r2" GeometricBounds="0 0 50 50">
              <Image LinkResourceURI="file:///link.jpg"/>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.rectangles.len(), 2);
        let r1 = &s.rectangles[0];
        assert_eq!(
            r1.image_bytes.as_deref(),
            Some(b"Hello, IDML!" as &[u8]),
            "inline CDATA should base64-decode and stash on the rect",
        );
        assert!(r1.has_image_element, "rect should still flag has_image_element");
        let r2 = &s.rectangles[1];
        assert!(r2.image_bytes.is_none(), "link-only rect carries no inline bytes");
        assert_eq!(r2.image_link.as_deref(), Some("file:///link.jpg"));
    }

    #[test]
    fn q03_decodes_whitespace_padded_base64() {
        // InDesign's serializer wraps base64 at ~76 chars with
        // surrounding whitespace; verify the decoder strips it.
        let xml = br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r" GeometricBounds="0 0 1 1">
              <Image><Properties><Contents><![CDATA[
                SGVsbG8s
                IElETUwh
              ]]></Contents></Properties></Image>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(
            s.rectangles[0].image_bytes.as_deref(),
            Some(b"Hello, IDML!" as &[u8]),
        );
    }

    #[test]
    fn q02_parses_text_frame_preference_auto_sizing() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1" GeometricBounds="0 0 50 30">
              <Properties/>
              <TextFramePreference AutoSizingType="WidthOnly"
                                   AutoSizingReferencePoint="TopLeftPoint"
                                   MinimumWidthForAutoSizing="40"
                                   MinimumHeightForAutoSizing="20"
                                   UseMinimumHeightForAutoSizing="true"/>
            </TextFrame>
            <TextFrame Self="frameB" ParentStory="u2" GeometricBounds="0 0 100 100">
              <Properties/>
              <TextFramePreference AutoSizingType="HeightAndWidth"/>
            </TextFrame>
            <TextFrame Self="frameC" ParentStory="u3" GeometricBounds="0 0 100 100">
              <Properties/>
              <TextFramePreference VerticalJustification="TopAlign"/>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        let a = &s.text_frames[0];
        assert_eq!(a.auto_sizing, Some(AutoSizingType::WidthOnly));
        assert!(a.auto_sizing.unwrap().grows_width());
        assert!(!a.auto_sizing.unwrap().grows_height());
        assert_eq!(
            a.auto_sizing_reference_point,
            Some(AutoSizingReferencePoint::TopLeftPoint)
        );
        assert_eq!(a.minimum_width_for_auto_sizing, Some(40.0));
        assert_eq!(a.minimum_height_for_auto_sizing, Some(20.0));
        assert_eq!(a.use_minimum_height_for_auto_sizing, Some(true));

        let b = &s.text_frames[1];
        assert_eq!(b.auto_sizing, Some(AutoSizingType::HeightAndWidth));
        assert!(b.auto_sizing.unwrap().grows_width());
        assert!(b.auto_sizing.unwrap().grows_height());

        let c = &s.text_frames[2];
        assert!(c.auto_sizing.is_none(), "frameC has no AutoSizingType");
    }

    #[test]
    fn parses_applied_toc_style_on_text_frame() {
        // TOC-host TextFrames carry `AppliedTOCStyle="TOCStyle/<id>"`
        // so the renderer can swap the unresolved story's paragraphs
        // for the resolver's output.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1" GeometricBounds="0 0 100 200"
                       AppliedTOCStyle="TOCStyle/Main"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(
            s.text_frames[0].applied_toc_style.as_deref(),
            Some("TOCStyle/Main")
        );
    }

    #[test]
    fn parses_next_text_frame_link() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1"
                       GeometricBounds="0 0 100 100"
                       NextTextFrame="frameB"/>
            <TextFrame Self="frameB" ParentStory="u1"
                       GeometricBounds="120 0 220 100"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.text_frames.len(), 2);
        assert_eq!(s.text_frames[0].next_text_frame.as_deref(), Some("frameB"));
        assert!(s.text_frames[1].next_text_frame.is_none());
    }

    #[test]
    fn group_without_item_transform_passes_child_through() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Group>
              <TextFrame Self="inner" ParentStory="u1" GeometricBounds="0 0 50 50"/>
            </Group>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.text_frames.len(), 1);
        assert!(
            s.text_frames[0].item_transform.is_none(),
            "no group transform + no own transform → None"
        );
    }

    #[test]
    fn parses_rectangles_alongside_text_frames() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="t1" ParentStory="u1" GeometricBounds="0 0 100 200"/>
            <Rectangle Self="r1" GeometricBounds="10 10 90 190"
                       FillColor="Color/Blue" StrokeColor="Color/Black"
                       StrokeWeight="1.5"/>
            <Rectangle Self="r2" GeometricBounds="200 200 300 300"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.text_frames.len(), 1);
        assert_eq!(s.rectangles.len(), 2);
        assert_eq!(s.rectangles[0].self_id.as_deref(), Some("r1"));
        assert_eq!(s.rectangles[0].fill_color.as_deref(), Some("Color/Blue"));
        assert_eq!(s.rectangles[0].stroke_weight, Some(1.5));
        assert_eq!(s.rectangles[1].fill_color, None);
    }

    #[test]
    fn parses_gradient_fill_and_stroke_angle_length() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 200"
                       FillColor="Gradient/Sky" StrokeColor="Gradient/Sun"
                       GradientFillAngle="45" GradientFillLength="120"
                       GradientStrokeAngle="-30" GradientStrokeLength="80"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        let r = &s.rectangles[0];
        assert_eq!(r.gradient_fill_angle, Some(45.0));
        assert_eq!(r.gradient_fill_length, Some(120.0));
        assert_eq!(r.gradient_stroke_angle, Some(-30.0));
        assert_eq!(r.gradient_stroke_length, Some(80.0));
    }

    #[test]
    fn parses_drop_shadow_inside_text_frame_properties() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frame1" ParentStory="u1" GeometricBounds="0 0 100 200">
              <Properties>
                <TransparencySetting>
                  <DropShadowSetting Mode="Drop" XOffset="3" YOffset="3" Size="6"
                                     Opacity="50" EffectColor="Color/Black"/>
                </TransparencySetting>
              </Properties>
            </TextFrame>
            <Rectangle Self="rect1" GeometricBounds="0 0 50 50"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.text_frames.len(), 1);
        let shadow = s.text_frames[0]
            .drop_shadow
            .as_ref()
            .expect("drop shadow parsed");
        assert_eq!(shadow.mode, "Drop");
        assert_eq!(shadow.x_offset, 3.0);
        assert_eq!(shadow.y_offset, 3.0);
        assert_eq!(shadow.size, 6.0);
        assert_eq!(shadow.opacity_pct, 50.0);
        assert_eq!(shadow.effect_color.as_deref(), Some("Color/Black"));
        // Plain rectangle without shadow stays None.
        assert_eq!(s.rectangles.len(), 1);
        assert!(s.rectangles[0].drop_shadow.is_none());
    }

    #[test]
    fn drop_shadow_under_stroke_transparency_lands_in_stroke_field() {
        // <StrokeTransparencySetting><DropShadowSetting/> → captured
        // as `stroke_drop_shadow`, not `drop_shadow`. Renderer gates
        // emission on stroke visibility downstream.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frame1" ParentStory="u1" GeometricBounds="0 0 100 200">
              <Properties>
                <StrokeTransparencySetting>
                  <DropShadowSetting Mode="Drop" XOffset="3" YOffset="3" Size="6"
                                     Opacity="50" EffectColor="Color/Black"/>
                </StrokeTransparencySetting>
              </Properties>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert!(s.text_frames[0].drop_shadow.is_none());
        let shadow = s.text_frames[0]
            .stroke_drop_shadow
            .as_ref()
            .expect("stroke drop shadow parsed");
        assert_eq!(shadow.mode, "Drop");
        assert_eq!(shadow.x_offset, 3.0);
    }

    #[test]
    fn drop_shadow_under_content_transparency_is_skipped() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frame1" ParentStory="u1" GeometricBounds="0 0 100 200">
              <Properties>
                <ContentTransparencySetting>
                  <DropShadowSetting Mode="Drop" XOffset="3" YOffset="3" Size="6"
                                     Opacity="50" EffectColor="Color/Black"/>
                </ContentTransparencySetting>
              </Properties>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert!(s.text_frames[0].drop_shadow.is_none());
        assert!(s.text_frames[0].stroke_drop_shadow.is_none());
    }

    #[test]
    fn drop_shadow_with_mode_none_is_skipped() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="f1" ParentStory="u1" GeometricBounds="0 0 100 200">
              <Properties>
                <TransparencySetting>
                  <DropShadowSetting Mode="None" XOffset="3" YOffset="3" Size="6"
                                     Opacity="50"/>
                </TransparencySetting>
              </Properties>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert!(s.text_frames[0].drop_shadow.is_none());
    }

    #[test]
    fn ignores_malformed_bounds() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Page Self="bad" GeometricBounds="0 0 bogus"/>
            <Page Self="good" GeometricBounds="0 0 100 200"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.pages.len(), 1);
        assert_eq!(s.pages[0].self_id.as_deref(), Some("good"));
    }

    /// Real-world IDMLs almost never serialise `GeometricBounds` on
    /// shape elements; geometry lives in `<Properties><PathGeometry>`
    /// instead. The parser must derive the bounds from the path
    /// anchors so InDesign exports populate frames at all.
    #[test]
    fn text_frame_bounds_come_from_path_geometry_when_attribute_absent() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frameA" ParentStory="u1"
                       ItemTransform="1 0 0 1 0 0">
              <Properties>
                <PathGeometry>
                  <GeometryPathType PathOpen="false">
                    <PathPointArray>
                      <PathPointType Anchor="-100 -50"
                                     LeftDirection="-100 -50"
                                     RightDirection="-100 -50"/>
                      <PathPointType Anchor="-100  150"
                                     LeftDirection="-100  150"
                                     RightDirection="-100  150"/>
                      <PathPointType Anchor=" 200  150"
                                     LeftDirection=" 200  150"
                                     RightDirection=" 200  150"/>
                      <PathPointType Anchor=" 200 -50"
                                     LeftDirection=" 200 -50"
                                     RightDirection=" 200 -50"/>
                    </PathPointArray>
                  </GeometryPathType>
                </PathGeometry>
              </Properties>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.text_frames.len(), 1, "frame must survive without GB");
        let f = &s.text_frames[0];
        // Bounding box of (-100,-50) and (200,150) → top=-50, left=-100,
        // bottom=150, right=200.
        assert_eq!(f.bounds.top, -50.0);
        assert_eq!(f.bounds.left, -100.0);
        assert_eq!(f.bounds.bottom, 150.0);
        assert_eq!(f.bounds.right, 200.0);
        assert_eq!(f.bounds.width(), 300.0);
        assert_eq!(f.bounds.height(), 200.0);
    }

    #[test]
    fn rectangle_oval_and_graphic_line_also_derive_bounds_from_path() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1">
              <Properties>
                <PathGeometry><GeometryPathType><PathPointArray>
                  <PathPointType Anchor="0 0"/>
                  <PathPointType Anchor="40 60"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
            </Rectangle>
            <Oval Self="o1">
              <Properties>
                <PathGeometry><GeometryPathType><PathPointArray>
                  <PathPointType Anchor="-5 -5"/>
                  <PathPointType Anchor="15 25"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
            </Oval>
            <GraphicLine Self="l1">
              <Properties>
                <PathGeometry><GeometryPathType><PathPointArray>
                  <PathPointType Anchor="0 100"/>
                  <PathPointType Anchor="200 100"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
            </GraphicLine>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.rectangles.len(), 1);
        assert_eq!(s.rectangles[0].bounds.width(), 40.0);
        assert_eq!(s.rectangles[0].bounds.height(), 60.0);
        assert_eq!(s.ovals.len(), 1);
        assert_eq!(s.ovals[0].bounds.width(), 20.0);
        assert_eq!(s.ovals[0].bounds.height(), 30.0);
        assert_eq!(s.graphic_lines.len(), 1);
        assert_eq!(s.graphic_lines[0].bounds.width(), 200.0);
        // Degenerate-height line still produces a frame so downstream
        // can render it as a stroke between the two anchors.
        assert_eq!(s.graphic_lines[0].bounds.height(), 0.0);
    }

    #[test]
    fn frame_with_neither_bounds_attribute_nor_path_is_dropped() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="lost" ParentStory="u1">
              <Properties/>
            </TextFrame>
            <TextFrame Self="kept" ParentStory="u2" GeometricBounds="0 0 50 50"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.text_frames.len(), 1);
        assert_eq!(s.text_frames[0].self_id.as_deref(), Some("kept"));
    }

    /// The CS5+ multi-page-size feature places each `<Page>` in the
    /// spread via its own ItemTransform. Previously we ignored the
    /// attribute, which made every real-world IDML page route frames
    /// to (0, 0) of spread coords and miss every page after the
    /// first. Capture both the attribute extraction and the
    /// translation-only common case here.
    #[test]
    fn page_carries_item_transform_for_multi_page_spreads() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Page Self="left"
                  GeometricBounds="0 0 792 612"
                  ItemTransform="1 0 0 1 -612 -396"/>
            <Page Self="right"
                  GeometricBounds="0 0 792 612"
                  ItemTransform="1 0 0 1 0 -396"/>
            <Page Self="legacy" GeometricBounds="0 0 792 612"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.pages.len(), 3);
        assert_eq!(
            s.pages[0].item_transform,
            Some([1.0, 0.0, 0.0, 1.0, -612.0, -396.0]),
            "left page's ItemTransform translates by (-612, -396)",
        );
        assert_eq!(
            s.pages[1].item_transform,
            Some([1.0, 0.0, 0.0, 1.0, 0.0, -396.0]),
            "right page's ItemTransform translates by (0, -396)",
        );
        assert_eq!(
            s.pages[2].item_transform, None,
            "legacy page without the attribute reads as identity",
        );
    }

    #[test]
    fn geometric_bounds_attribute_wins_over_path_geometry_when_both_present() {
        // Defensive: when both shapes carry geometry, the attribute
        // is the authoritative source (it's what InDesign writes when
        // emitting a synthetic element). PathGeometry should not
        // overwrite it.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="frame" ParentStory="u1"
                       GeometricBounds="0 0 100 200">
              <Properties>
                <PathGeometry><GeometryPathType><PathPointArray>
                  <PathPointType Anchor="999 999"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
            </TextFrame>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.text_frames[0].bounds.right, 200.0);
        assert_eq!(s.text_frames[0].bounds.bottom, 100.0);
    }

    #[test]
    fn polygon_text_path_attaches_to_parent_polygon() {
        // Real-world IDML serialises text-on-path as a `<TextPath>`
        // child of the host shape, referencing a story via
        // `ParentStory`. The host's own `<PathGeometry>` provides the
        // curve geometry — we just need the story link plus a few
        // alignment knobs.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Polygon Self="poly1">
              <Properties>
                <PathGeometry><GeometryPathType><PathPointArray>
                  <PathPointType Anchor="0 0"/>
                  <PathPointType Anchor="100 0"/>
                </PathPointArray></GeometryPathType></PathGeometry>
              </Properties>
              <TextPath Self="tp1" ParentStory="story_u1"
                        PathAlignment="CenterPathAlignment"
                        PathEffect="RainbowPathEffect"
                        FlipPathEffect="NotFlipped"
                        StartBracket="0" EndBracket="100"/>
            </Polygon>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.polygons.len(), 1);
        assert_eq!(s.polygons[0].text_paths.len(), 1);
        let tp = &s.polygons[0].text_paths[0];
        assert_eq!(tp.parent_story, "story_u1");
        assert_eq!(tp.self_id.as_deref(), Some("tp1"));
        assert_eq!(tp.path_alignment.as_deref(), Some("CenterPathAlignment"));
        assert_eq!(tp.path_effect.as_deref(), Some("RainbowPathEffect"));
        assert_eq!(tp.start_bracket, Some(0.0));
        assert_eq!(tp.end_bracket, Some(100.0));
    }

    #[test]
    fn polygon_hosts_image_link_and_item_transform() {
        // A `<Polygon>` may host a placed image just like a Rectangle.
        // The nested `<Image>`'s `LinkResourceURI` (or its `<Link>`
        // child's `LinkResourceURI`) populates `image_link`; the
        // `<Image>`'s `ItemTransform` populates `image_item_transform`.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Polygon Self="poly1" GeometricBounds="0 0 100 100">
              <Properties/>
              <Image Self="img1" ItemTransform="0.5 0 0 0.5 10 20">
                <Link Self="link1" LinkResourceURI="file:///tmp/photo.jpg"/>
              </Image>
            </Polygon>
            <Polygon Self="poly2" GeometricBounds="0 0 50 50"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.polygons.len(), 2);
        let p = &s.polygons[0];
        assert_eq!(p.image_link.as_deref(), Some("file:///tmp/photo.jpg"));
        assert_eq!(p.image_item_transform, Some([0.5, 0.0, 0.0, 0.5, 10.0, 20.0]));
        // Plain polygon without image stays None.
        assert!(s.polygons[1].image_link.is_none());
        assert!(s.polygons[1].image_item_transform.is_none());
        // Rectangles in the same spread keep working.
        assert_eq!(s.rectangles.len(), 0);
    }

    #[test]
    fn group_records_members_and_transparency_block() {
        // A `<Group>` wrapping two rectangles with its own
        // `<TransparencySetting>` / `<BlendingSetting>` /
        // `<DropShadowSetting>` block. The group entry should carry
        // the blend mode + opacity + drop shadow; member FrameRefs
        // should match the rectangles' indices in document order.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Group Self="grp1" ItemTransform="1 0 0 1 5 7">
              <Properties>
                <TransparencySetting>
                  <BlendingSetting Opacity="60" BlendMode="Multiply"/>
                  <DropShadowSetting Mode="Drop" XOffset="2" YOffset="3" Size="5"
                                     Opacity="80" EffectColor="Color/Black"/>
                </TransparencySetting>
              </Properties>
              <Rectangle Self="r1" GeometricBounds="0 0 50 50"/>
              <Rectangle Self="r2" GeometricBounds="0 60 50 110"/>
            </Group>
            <Rectangle Self="r3" GeometricBounds="100 0 150 50"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.rectangles.len(), 3);
        assert_eq!(s.groups.len(), 1);
        let g = &s.groups[0];
        assert_eq!(g.self_id.as_deref(), Some("grp1"));
        assert_eq!(g.item_transform, Some([1.0, 0.0, 0.0, 1.0, 5.0, 7.0]));
        assert_eq!(g.transparency.blend_mode.as_deref(), Some("Multiply"));
        assert_eq!(g.transparency.opacity, Some(60.0));
        let shadow = g
            .transparency
            .drop_shadow
            .as_ref()
            .expect("drop shadow on group");
        assert_eq!(shadow.mode, "Drop");
        assert_eq!(shadow.x_offset, 2.0);
        assert_eq!(shadow.opacity_pct, 80.0);
        // Members are the two grouped rectangles in document order;
        // r3 sits outside and is NOT a member.
        assert_eq!(
            g.members,
            vec![FrameRef::Rectangle(0), FrameRef::Rectangle(1)]
        );
        // Top-level surface: the group as a single entry + the
        // ungrouped r3. Grouped rectangles do NOT appear here.
        assert_eq!(
            s.frames_in_order,
            vec![FrameRef::Group(0), FrameRef::Rectangle(2)]
        );
    }

    #[test]
    fn nested_groups_register_subgroup_members() {
        // Outer group contains a sub-group + a TextFrame. The
        // sub-group contains two Polygons. Outer's members should
        // list TextFrame(0), Group(0). Inner's members should list
        // both polygons.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Group Self="outer">
              <TextFrame Self="t1" ParentStory="u1" GeometricBounds="0 0 10 10"/>
              <Group Self="inner">
                <Polygon Self="p1" GeometricBounds="0 0 5 5"/>
                <Polygon Self="p2" GeometricBounds="0 0 6 6"/>
              </Group>
            </Group>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.groups.len(), 2);
        // Inner group closes first → at index 0.
        let inner = &s.groups[0];
        assert_eq!(inner.self_id.as_deref(), Some("inner"));
        assert_eq!(
            inner.members,
            vec![FrameRef::Polygon(0), FrameRef::Polygon(1)]
        );
        let outer = &s.groups[1];
        assert_eq!(outer.self_id.as_deref(), Some("outer"));
        assert_eq!(
            outer.members,
            vec![FrameRef::TextFrame(0), FrameRef::Group(0)]
        );
        // Group transparency defaults to all-None when absent.
        assert!(outer.transparency.blend_mode.is_none());
        assert!(outer.transparency.opacity.is_none());
        assert!(outer.transparency.drop_shadow.is_none());
        // Outer is the only top-level item; inner stays buried in
        // outer.members and does NOT surface in frames_in_order.
        assert_eq!(s.frames_in_order, vec![FrameRef::Group(1)]);
    }

    #[test]
    fn group_blending_setting_does_not_leak_into_inner_frame() {
        // BlendingSetting attached to the Group must update the
        // group's transparency, not the inner frames' opacity. The
        // current_frame check in the BlendingSetting arm already
        // disambiguates; this test pins the contract.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Group Self="grp">
              <Properties>
                <TransparencySetting>
                  <BlendingSetting Opacity="40" BlendMode="Screen"/>
                </TransparencySetting>
              </Properties>
              <Rectangle Self="r1" GeometricBounds="0 0 50 50"/>
            </Group>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert!(s.rectangles[0].opacity.is_none());
        assert!(s.rectangles[0].blend_mode.is_none());
        assert_eq!(s.groups.len(), 1);
        assert_eq!(s.groups[0].transparency.opacity, Some(40.0));
        assert_eq!(s.groups[0].transparency.blend_mode.as_deref(), Some("Screen"));
    }

    #[test]
    fn polygon_image_link_falls_through_to_outer_image_attribute() {
        // When the `<Image>` element itself carries a
        // `LinkResourceURI` (no nested `<Link>`), the polygon still
        // picks it up. Mirrors the Rectangle behaviour.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Polygon Self="poly1" GeometricBounds="0 0 100 100">
              <Image Self="img1" LinkResourceURI="file:///tmp/cat.png"
                     ItemTransform="1 0 0 1 0 0"/>
            </Polygon>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.polygons[0].image_link.as_deref(), Some("file:///tmp/cat.png"));
        assert_eq!(
            s.polygons[0].image_item_transform,
            Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        );
    }

    #[test]
    fn parses_directional_feather_setting() {
        // Per-edge widths land in `directional_feather`; the bool
        // sentinel from the previous parser is gone.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Properties>
                <TransparencySetting>
                  <DirectionalFeatherSetting Applied="true"
                    LeftWidth="2" RightWidth="3" TopWidth="4" BottomWidth="5"
                    Angle="90" NoiseAmount="10" ChokeAmount="20"
                    CornerType="Rounded"/>
                </TransparencySetting>
              </Properties>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        let bag = s.rectangles[0].effects.as_ref().expect("effects bag");
        let dir = bag
            .directional_feather
            .as_ref()
            .expect("directional feather parsed");
        assert_eq!(dir.left_width, Some(2.0));
        assert_eq!(dir.right_width, Some(3.0));
        assert_eq!(dir.top_width, Some(4.0));
        assert_eq!(dir.bottom_width, Some(5.0));
        assert_eq!(dir.angle_deg, Some(90.0));
        assert_eq!(dir.noise_pct, Some(10.0));
        assert_eq!(dir.choke_pct, Some(20.0));
        assert_eq!(dir.corner_type.as_deref(), Some("Rounded"));
    }

    #[test]
    fn directional_feather_disabled_when_applied_false() {
        // `Applied="false"` short-circuits the whole block —
        // directional_feather stays None.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Properties>
                <TransparencySetting>
                  <DirectionalFeatherSetting Applied="false"
                    LeftWidth="2" RightWidth="3" TopWidth="4" BottomWidth="5"/>
                </TransparencySetting>
              </Properties>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        // The effects bag may be absent entirely or have no
        // directional_feather; both are acceptable.
        let dir_present = s.rectangles[0]
            .effects
            .as_ref()
            .and_then(|e| e.directional_feather.as_ref())
            .is_some();
        assert!(!dir_present, "Applied=false should leave directional_feather=None");
    }

    #[test]
    fn parses_gradient_feather_setting_with_stops() {
        // Linear gradient feather with two stops; `<GradientStop>`
        // children are nested inside `<GradientFeatherSetting>` and
        // get appended to `gradient_feather.stops`.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <Rectangle Self="r1" GeometricBounds="0 0 100 100">
              <Properties>
                <TransparencySetting>
                  <GradientFeatherSetting Applied="true" Type="Linear"
                                          GradientAngle="45">
                    <GradientStop StopColor="Color/Black" Location="0"
                                  Alpha="100" GradientStopMidpoint="50"/>
                    <GradientStop StopColor="Color/Black" Location="100"
                                  Alpha="0" GradientStopMidpoint="50"/>
                  </GradientFeatherSetting>
                </TransparencySetting>
              </Properties>
            </Rectangle>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        let bag = s.rectangles[0].effects.as_ref().expect("effects bag");
        let gf = bag
            .gradient_feather
            .as_ref()
            .expect("gradient feather parsed");
        assert_eq!(gf.gradient_type.as_deref(), Some("Linear"));
        assert_eq!(gf.angle_deg, Some(45.0));
        assert_eq!(gf.stops.len(), 2);
        assert_eq!(gf.stops[0].location_pct, 0.0);
        assert_eq!(gf.stops[0].alpha_pct, 100.0);
        assert_eq!(gf.stops[0].stop_color.as_deref(), Some("Color/Black"));
        assert_eq!(gf.stops[1].location_pct, 100.0);
        assert_eq!(gf.stops[1].alpha_pct, 0.0);
    }

    /// IDML compound paths (e.g. `<Polygon>` with two
    /// `<GeometryPathType>` children — square + hole) must surface
    /// the contour boundaries via `subpath_starts` so the renderer
    /// can lift them into separate MoveTo/Close subpaths. Without
    /// this, the renderer silently joins the two contours into one
    /// broken polyline (the geometry-groups page-6 visual regression).
    #[test]
    fn polygon_compound_path_records_subpath_starts() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
              <Spread Self="s">
                <Polygon Self="p1" FillColor="Color/Black">
                  <Properties>
                    <PathGeometry>
                      <GeometryPathType PathOpen="false">
                        <PathPointArray>
                          <PathPointType Anchor="0 0"/>
                          <PathPointType Anchor="200 0"/>
                          <PathPointType Anchor="200 200"/>
                          <PathPointType Anchor="0 200"/>
                        </PathPointArray>
                      </GeometryPathType>
                      <GeometryPathType PathOpen="false">
                        <PathPointArray>
                          <PathPointType Anchor="60 60"/>
                          <PathPointType Anchor="60 140"/>
                          <PathPointType Anchor="140 140"/>
                          <PathPointType Anchor="140 60"/>
                        </PathPointArray>
                      </GeometryPathType>
                    </PathGeometry>
                  </Properties>
                </Polygon>
              </Spread>
            </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.polygons.len(), 1);
        let p = &s.polygons[0];
        assert_eq!(p.anchors.len(), 8, "both contours' anchors are stored");
        assert_eq!(
            p.subpath_starts,
            vec![0, 4],
            "compound path → two contour starts at indices 0 and 4"
        );
    }

    /// Single-contour polygons (the InDesign-export shape every plain
    /// rectangle / polygon uses) leave `subpath_starts` empty so the
    /// renderer's legacy single-MoveTo path keeps firing.
    #[test]
    fn polygon_path_open_lifts_to_subpath_open_flag() {
        // P-15: `<GeometryPathType PathOpen="true">` should lift onto
        // the polygon's `subpath_open` slice so the renderer can skip
        // the auto-close. Single open contour: `subpath_starts` stays
        // empty (legacy canonical form for one contour) but
        // `subpath_open` carries `[true]` so the renderer can branch.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
              <Spread Self="s">
                <Polygon Self="p1" FillColor="Color/Black">
                  <Properties>
                    <PathGeometry>
                      <GeometryPathType PathOpen="true">
                        <PathPointArray>
                          <PathPointType Anchor="0 0"/>
                          <PathPointType Anchor="100 0"/>
                          <PathPointType Anchor="50 50"/>
                        </PathPointArray>
                      </GeometryPathType>
                    </PathGeometry>
                  </Properties>
                </Polygon>
              </Spread>
            </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.polygons.len(), 1);
        assert_eq!(s.polygons[0].subpath_open, vec![true]);
    }

    #[test]
    fn polygon_compound_path_open_records_per_contour_flags() {
        // P-15: two contours, one open and one closed; the flags need
        // to come out in declaration order parallel to `subpath_starts`.
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
              <Spread Self="s">
                <Polygon Self="p1" FillColor="Color/Black">
                  <Properties>
                    <PathGeometry>
                      <GeometryPathType PathOpen="true">
                        <PathPointArray>
                          <PathPointType Anchor="0 0"/>
                          <PathPointType Anchor="40 40"/>
                        </PathPointArray>
                      </GeometryPathType>
                      <GeometryPathType PathOpen="false">
                        <PathPointArray>
                          <PathPointType Anchor="100 0"/>
                          <PathPointType Anchor="200 0"/>
                          <PathPointType Anchor="200 100"/>
                        </PathPointArray>
                      </GeometryPathType>
                    </PathGeometry>
                  </Properties>
                </Polygon>
              </Spread>
            </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.polygons.len(), 1);
        assert_eq!(s.polygons[0].subpath_starts, vec![0, 2]);
        assert_eq!(s.polygons[0].subpath_open, vec![true, false]);
    }

    #[test]
    fn polygon_single_contour_leaves_subpath_starts_empty() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
              <Spread Self="s">
                <Polygon Self="p1" FillColor="Color/Black">
                  <Properties>
                    <PathGeometry>
                      <GeometryPathType PathOpen="false">
                        <PathPointArray>
                          <PathPointType Anchor="0 0"/>
                          <PathPointType Anchor="100 0"/>
                          <PathPointType Anchor="100 100"/>
                          <PathPointType Anchor="0 100"/>
                        </PathPointArray>
                      </GeometryPathType>
                    </PathGeometry>
                  </Properties>
                </Polygon>
              </Spread>
            </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.polygons.len(), 1);
        assert!(
            s.polygons[0].subpath_starts.is_empty(),
            "single contour → no markers (legacy path stays hot)"
        );
    }

    #[test]
    fn overprint_attributes_round_trip_through_every_shape() {
        // Pin that `OverprintFill` / `OverprintStroke` lift off the
        // outer tag for every page-item kind (Rectangle / Oval /
        // TextFrame / Polygon / GraphicLine). Absent attributes
        // default to `false` (the IDML default).
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
            <idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
              <Spread Self="s1">
                <Rectangle Self="r1"
                           GeometricBounds="0 0 10 10"
                           FillColor="Color/Black"
                           StrokeColor="Color/None"
                           OverprintFill="true"
                           OverprintStroke="true"/>
                <Rectangle Self="r2"
                           GeometricBounds="0 0 10 10"
                           FillColor="Color/Cyan"/>
                <Oval Self="o1"
                      GeometricBounds="0 0 10 10"
                      FillColor="Color/Black"
                      OverprintFill="true"/>
                <TextFrame Self="t1"
                           ParentStory="u10"
                           GeometricBounds="0 0 10 10"
                           OverprintFill="true"/>
                <Polygon Self="p1"
                         GeometricBounds="0 0 10 10"
                         FillColor="Color/Black"
                         OverprintFill="true"
                         OverprintStroke="false">
                  <Properties>
                    <PathGeometry>
                      <GeometryPathType>
                        <PathPointArray>
                          <PathPointType Anchor="0 0"/>
                          <PathPointType Anchor="10 0"/>
                          <PathPointType Anchor="10 10"/>
                          <PathPointType Anchor="0 10"/>
                        </PathPointArray>
                      </GeometryPathType>
                    </PathGeometry>
                  </Properties>
                </Polygon>
                <GraphicLine Self="l1"
                             GeometricBounds="0 0 10 10"
                             StrokeColor="Color/Black"
                             OverprintStroke="true"/>
              </Spread>
            </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        // Rect r1: both flags true; r2: both false (defaults).
        assert!(s.rectangles[0].overprint_fill);
        assert!(s.rectangles[0].overprint_stroke);
        assert!(!s.rectangles[1].overprint_fill);
        assert!(!s.rectangles[1].overprint_stroke);
        // Oval: fill flag picked up.
        assert!(s.ovals[0].overprint_fill);
        assert!(!s.ovals[0].overprint_stroke);
        // TextFrame: fill flag picked up.
        assert!(s.text_frames[0].overprint_fill);
        // Polygon: fill true, stroke explicitly false.
        assert!(s.polygons[0].overprint_fill);
        assert!(!s.polygons[0].overprint_stroke);
        // GraphicLine: only stroke is meaningful.
        assert!(s.graphic_lines[0].overprint_stroke);
    }
}
