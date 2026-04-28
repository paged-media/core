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

use crate::util::attr;
use crate::ParseError;

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
    /// Resolved to a `MasterSpread` by `idml_scene::Document`.
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
    /// `Graphic` in `idml-parse::graphic`.
    pub fill_color: Option<String>,
    /// `StrokeColor` attribute.
    pub stroke_color: Option<String>,
    /// `StrokeWeight` attribute, in points. `None` → document default
    /// (typically 1 pt in InDesign).
    pub stroke_weight: Option<f32>,
    /// `<DropShadowSetting>` parsed from `<Properties><TransparencySetting>`.
    /// `None` when absent or `Mode="None"`.
    pub drop_shadow: Option<DropShadowSetting>,
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
    /// `AppliedObjectStyle` reference — `ObjectStyle/<id>`. Real-
    /// world IDMLs almost always rely on this for fill/stroke; the
    /// per-element FillColor attribute is rare. Resolved by
    /// `idml_scene::Document` against the document's StyleSheet.
    pub applied_object_style: Option<String>,
    /// `<TextWrapPreference>` parsed off the frame.
    pub text_wrap: Option<TextWrap>,
}

/// IDML `<TextFramePreference VerticalJustification="...">` values.
/// `Top` is the IDML default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum VerticalJustification {
    Top,
    Center,
    Bottom,
    /// "JustifyAlign" — distributes paragraph spacing to fill the
    /// frame vertically. Renderer falls through to Top until the
    /// per-paragraph distribution pass lands.
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
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
    pub drop_shadow: Option<DropShadowSetting>,
    /// `LinkResourceURI` from a nested `<Image>` (or its `<Link>`
    /// child). The pipeline routes this through
    /// `AssetResolver::resolve_image`. `None` means the rectangle
    /// is a plain colour swatch.
    pub image_link: Option<String>,
    /// `AppliedObjectStyle` reference; see `TextFrame`.
    pub applied_object_style: Option<String>,
    /// `<TextWrapPreference>` parsed off the rectangle.
    pub text_wrap: Option<TextWrap>,
}

/// Axis-aligned ellipse — `<Oval>` in IDML. Same fill/stroke story as
/// Rectangle; geometry is the ellipse inscribed in `GeometricBounds`.
#[derive(Debug, Clone, Serialize)]
pub struct Oval {
    pub self_id: Option<String>,
    pub bounds: Bounds,
    pub item_transform: Option<[f32; 6]>,
    pub fill_color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
    pub drop_shadow: Option<DropShadowSetting>,
    /// `AppliedObjectStyle` reference; see `TextFrame`.
    pub applied_object_style: Option<String>,
    /// `<TextWrapPreference>` parsed off the oval.
    pub text_wrap: Option<TextWrap>,
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
    /// `AppliedObjectStyle` reference; see `TextFrame`.
    pub applied_object_style: Option<String>,
    /// `<TextWrapPreference>` parsed off the line.
    pub text_wrap: Option<TextWrap>,
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
    pub fn from_idml(s: &str) -> Self {
        match s {
            "None" => Self::None,
            "BoundingBoxTextWrap" => Self::BoundingBoxTextWrap,
            "ContourTextWrap" => Self::ContourTextWrap,
            "JumpObjectTextWrap" => Self::JumpObjectTextWrap,
            "NextColumnTextWrap" => Self::NextColumnTextWrap,
            _ => Self::Other,
        }
    }
    pub fn is_active(self) -> bool {
        !matches!(self, Self::None)
    }
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
    pub stroke_color: Option<String>,
    pub stroke_weight: Option<f32>,
    pub applied_object_style: Option<String>,
    /// Path-point anchors with their Bezier control points, in the
    /// polygon's inner coords. Empty for synthetic IDMLs that
    /// declared the polygon via `GeometricBounds` only.
    pub anchors: Vec<PathAnchor>,
    /// `<TextWrapPreference>` parsed off the polygon, if any.
    /// `None` ⇒ the polygon does not exclude text.
    pub text_wrap: Option<TextWrap>,
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
    /// True for Polygons even when `needs_bounds` is false, so the
    /// emitter still gets the curved-path data.
    keep_anchors: bool,
    /// True while a `<TextWrapPreference>` block is open, so the
    /// child `<TextWrapOffset>` knows to write back to the current
    /// shape.
    in_text_wrap: bool,
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
        let mut current_frame: Option<CurrentFrame> = None;
        let mut buf = Vec::new();

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
                                    .map(|s| {
                                        s.split_whitespace().map(str::to_string).collect()
                                    })
                                    .unwrap_or_default(),
                                name: attr(&e, b"Name"),
                            });
                        }
                    }
                    b"TextFrame" => {
                        let bounds_attr =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                        let parent_story = attr(&e, b"ParentStory");
                        let item_transform = effective_item_transform(
                            &group_transforms,
                            attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s)),
                        );
                        let fill_color = attr(&e, b"FillColor");
                        let stroke_color = attr(&e, b"StrokeColor");
                        let stroke_weight =
                            attr(&e, b"StrokeWeight").and_then(|s| s.parse::<f32>().ok());
                        out.text_frames.push(TextFrame {
                            self_id: attr(&e, b"Self"),
                            parent_story,
                            bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                            item_transform,
                            fill_color,
                            stroke_color,
                            stroke_weight,
                            drop_shadow: None,
                            next_text_frame: attr(&e, b"NextTextFrame"),
                            vertical_justification: None,
                            first_baseline_offset: None,
                            minimum_first_baseline_offset: None,
                            inset_spacing: None,
                            applied_object_style: attr(&e, b"AppliedObjectStyle"),
                            text_wrap: None,
                        });
                        current_frame = Some(CurrentFrame {
                            kind: CurrentFrameKind::Text(out.text_frames.len() - 1),
                            needs_bounds: bounds_attr.is_none(),
                            anchors: Vec::new(),
                            keep_anchors: false,
                            in_text_wrap: false,
                        });
                    }
                    b"Rectangle" => {
                        let bounds_attr =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                        let item_transform = effective_item_transform(
                            &group_transforms,
                            attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s)),
                        );
                        out.rectangles.push(Rectangle {
                            self_id: attr(&e, b"Self"),
                            bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                            item_transform,
                            fill_color: attr(&e, b"FillColor"),
                            stroke_color: attr(&e, b"StrokeColor"),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                            drop_shadow: None,
                            image_link: None,
                            applied_object_style: attr(&e, b"AppliedObjectStyle"),
                            text_wrap: None,
                        });
                        current_frame = Some(CurrentFrame {
                            kind: CurrentFrameKind::Rect(out.rectangles.len() - 1),
                            needs_bounds: bounds_attr.is_none(),
                            anchors: Vec::new(),
                            keep_anchors: false,
                            in_text_wrap: false,
                        });
                    }
                    b"Oval" => {
                        let bounds_attr =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                        let item_transform = effective_item_transform(
                            &group_transforms,
                            attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s)),
                        );
                        out.ovals.push(Oval {
                            self_id: attr(&e, b"Self"),
                            bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                            item_transform,
                            fill_color: attr(&e, b"FillColor"),
                            stroke_color: attr(&e, b"StrokeColor"),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                            drop_shadow: None,
                            applied_object_style: attr(&e, b"AppliedObjectStyle"),
                            text_wrap: None,
                        });
                        current_frame = Some(CurrentFrame {
                            kind: CurrentFrameKind::Oval(out.ovals.len() - 1),
                            needs_bounds: bounds_attr.is_none(),
                            anchors: Vec::new(),
                            keep_anchors: false,
                            in_text_wrap: false,
                        });
                    }
                    b"DropShadowSetting" => {
                        if let (Some(cf), Some(setting)) =
                            (current_frame.as_ref(), parse_drop_shadow(&e))
                        {
                            // Only "Drop"/"Default" mode results in a
                            // visible shadow. "None" means the shadow
                            // is disabled even though the setting is
                            // serialised.
                            if setting.mode != "None" {
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
                                    CurrentFrameKind::Line(_) | CurrentFrameKind::Polygon(_) => {
                                        // GraphicLine + Polygon have
                                        // no drop_shadow field today;
                                        // ignore.
                                    }
                                }
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
                        }
                    }
                    b"Image" | b"Link" => {
                        // IDML's image-bearing rectangle nests an
                        // <Image> with a LinkResourceURI on the
                        // element itself or on its <Link> child.
                        // Either source attaches to the current
                        // Rectangle (the only frame type that hosts
                        // images in this slice).
                        if let (Some(CurrentFrameKind::Rect(i)), Some(uri)) = (
                            current_frame.as_ref().map(|cf| cf.kind),
                            attr(&e, b"LinkResourceURI").or_else(|| attr(&e, b"href")),
                        ) {
                            // First-write-wins so the outer <Image>
                            // attribute beats the inner <Link>'s.
                            if out.rectangles[i].image_link.is_none() {
                                out.rectangles[i].image_link = Some(uri);
                            }
                        }
                    }
                    b"GraphicLine" => {
                        let bounds_attr =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                        let item_transform = effective_item_transform(
                            &group_transforms,
                            attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s)),
                        );
                        out.graphic_lines.push(GraphicLine {
                            self_id: attr(&e, b"Self"),
                            bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                            item_transform,
                            stroke_color: attr(&e, b"StrokeColor"),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                            applied_object_style: attr(&e, b"AppliedObjectStyle"),
                            text_wrap: None,
                        });
                        current_frame = Some(CurrentFrame {
                            kind: CurrentFrameKind::Line(out.graphic_lines.len() - 1),
                            needs_bounds: bounds_attr.is_none(),
                            anchors: Vec::new(),
                            keep_anchors: false,
                            in_text_wrap: false,
                        });
                    }
                    b"Polygon" => {
                        let bounds_attr =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s));
                        let item_transform = effective_item_transform(
                            &group_transforms,
                            attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s)),
                        );
                        out.polygons.push(Polygon {
                            self_id: attr(&e, b"Self"),
                            bounds: bounds_attr.unwrap_or(Bounds::ZERO),
                            item_transform,
                            fill_color: attr(&e, b"FillColor"),
                            stroke_color: attr(&e, b"StrokeColor"),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                            applied_object_style: attr(&e, b"AppliedObjectStyle"),
                            text_wrap: None,
                            anchors: Vec::new(),
                        });
                        current_frame = Some(CurrentFrame {
                            kind: CurrentFrameKind::Polygon(out.polygons.len() - 1),
                            needs_bounds: bounds_attr.is_none(),
                            anchors: Vec::new(),
                            // Always retain Bezier path anchors for
                            // polygons so the renderer can emit a
                            // FillPath instead of a bbox FillRect.
                            keep_anchors: true,
                            in_text_wrap: false,
                        });
                    }
                    _ => {}
                },
                Event::End(e) => match e.name().as_ref() {
                    b"Group" if !group_transforms.is_empty() => {
                        group_transforms.pop();
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
                            // outline.
                            if cf.keep_anchors && !cf.anchors.is_empty() {
                                if let CurrentFrameKind::Polygon(i) = cf.kind {
                                    if i < out.polygons.len() {
                                        out.polygons[i].anchors = cf.anchors;
                                    }
                                }
                            }
                        }
                    }
                    b"TextWrapPreference" => {
                        if let Some(cf) = current_frame.as_mut() {
                            cf.in_text_wrap = false;
                        }
                    }
                    _ => {}
                },
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
    Some(DropShadowSetting {
        mode: attr(e, b"Mode").unwrap_or_else(|| "Drop".to_string()),
        x_offset: attr(e, b"XOffset")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        y_offset: attr(e, b"YOffset")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        size: attr(e, b"Size").and_then(|s| s.parse().ok()).unwrap_or(0.0),
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
/// `idml_compose::Transform::compose` so the parser and the
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
}
