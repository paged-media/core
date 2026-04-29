//! Page-item shapes. Phase 0 only emits Rectangle (with optional
//! `parent_story` so a TextFrame-equivalent rectangle can host body
//! text). Other variants land in subsequent phases.

use crate::geometry::{format_matrix, Matrix, IDENTITY};
use crate::xml::{format_f32, XmlBuilder};

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
        if let Some(w) = self.stroke_weight_pt {
            attrs.push(("StrokeWeight", format_f32(w)));
        }
        for (k, v) in &self.extra_attrs {
            attrs.push((k.as_str(), v.clone()));
        }
        let attr_refs: Vec<(&str, &str)> = attrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        b.start(kind, &attr_refs);
        b.start("Properties", &[]);
        write_path_geometry(b, self.width_pt, self.height_pt);
        b.end("Properties");
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
