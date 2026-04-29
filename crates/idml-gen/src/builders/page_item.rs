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
    Rect(Rect),
    Group(Group),
    Polygon(Polygon),
}

impl From<Rect> for PageItem {
    fn from(r: Rect) -> Self {
        PageItem::Rect(r)
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
/// `idml-parse/src/spread.rs` recognises just the `ItemTransform`
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
            &[
                ("Self", self.self_id.as_str()),
                ("ItemTransform", &xform),
            ],
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
        let mut attrs: Vec<(&str, String)> = Vec::new();
        attrs.push(("Self", self.self_id.clone()));
        attrs.push(("ItemTransform", xform));
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
    /// Optional placed-image payload. When set, the rectangle becomes
    /// a graphic frame: a nested `<Image>` element carries the
    /// `LinkResourceURI`, and a sibling `<FrameFittingOption>` element
    /// describes how the image is cropped against the frame edges.
    pub placed_image: Option<PlacedImage>,
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
#[derive(Clone)]
pub struct DropShadow {
    /// `Mode` — typically `"Drop"` for an enabled shadow,
    /// `"None"` to serialise but disable.
    pub mode: &'static str,
    pub x_offset: f32,
    pub y_offset: f32,
    pub size: f32,
    pub opacity_pct: f32,
    pub effect_color: String,
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
            extra_attrs: Vec::new(),
            blending: None,
            drop_shadow: None,
            placed_image: None,
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
            attrs.push(("PreviousTextFrame", "n".to_string()));
            attrs.push(("NextTextFrame", "n".to_string()));
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
        attrs.push((
            "AppliedObjectStyle",
            "ObjectStyle/$ID/[None]".to_string(),
        ));
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
        if self.blending.is_some() || self.drop_shadow.is_some() {
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
                let xo = format_f32(ds.x_offset);
                let yo = format_f32(ds.y_offset);
                let sz = format_f32(ds.size);
                let op = format_f32(ds.opacity_pct);
                b.empty(
                    "DropShadowSetting",
                    &[
                        ("Mode", ds.mode),
                        ("XOffset", xo.as_str()),
                        ("YOffset", yo.as_str()),
                        ("Size", sz.as_str()),
                        ("Opacity", op.as_str()),
                        ("EffectColor", ds.effect_color.as_str()),
                    ],
                );
            }
            b.end("TransparencySetting");
        }
        if let Some(img) = &self.placed_image {
            // `<FrameFittingOption>` is a direct child of the
            // Rectangle (not inside Properties) — matches what
            // InDesign emits and what the idml-parse Rectangle
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
            b.start(
                "Image",
                &[
                    ("Self", img.image_self_id.as_str()),
                    ("ItemTransform", "1 0 0 1 0 0"),
                    ("LinkResourceURI", img.link_resource_uri.as_str()),
                ],
            );
            b.start("Properties", &[]);
            write_path_geometry(b, img.image_w_pt, img.image_h_pt);
            b.end("Properties");
            b.empty(
                "Link",
                &[("LinkResourceURI", img.link_resource_uri.as_str())],
            );
            b.end("Image");
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
