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
    /// Number of text frames skipped because they were nested inside a
    /// Group. Exposed so callers can flag lossy parses without reading
    /// logs.
    pub skipped_nested_frames: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub self_id: Option<String>,
    pub bounds: Bounds,
    /// `AppliedMaster` reference — `MasterSpread/<id>` typically.
    /// Resolved to a `MasterSpread` by `idml_scene::Document`.
    pub applied_master: Option<String>,
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
    /// `VerticalJustification` from `<TextFramePreference>`. IDML
    /// values: `TopAlign` (default), `CenterAlign`, `BottomAlign`,
    /// `JustifyAlign`.
    pub vertical_justification: Option<String>,
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
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Bounds {
    pub top: f32,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
}

impl Bounds {
    pub fn width(&self) -> f32 {
        self.right - self.left
    }
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
}

/// Tracks which frame the current `<DropShadowSetting>` should
/// attach to — the most recently opened TextFrame / Rectangle / Oval.
#[derive(Debug, Clone, Copy)]
enum CurrentFrame {
    Text(usize),
    Rect(usize),
    Oval(usize),
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

        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                    b"Spread" => {
                        if out.self_id.is_none() {
                            out.self_id = attr(&e, b"Self");
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
                            });
                        }
                    }
                    b"TextFrame" => {
                        let Some(bounds) =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s))
                        else {
                            continue;
                        };
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
                            bounds,
                            item_transform,
                            fill_color,
                            stroke_color,
                            stroke_weight,
                            drop_shadow: None,
                            next_text_frame: attr(&e, b"NextTextFrame"),
                            vertical_justification: None,
                        });
                        current_frame = Some(CurrentFrame::Text(out.text_frames.len() - 1));
                    }
                    b"Rectangle" => {
                        let Some(bounds) =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s))
                        else {
                            continue;
                        };
                        let item_transform = effective_item_transform(
                            &group_transforms,
                            attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s)),
                        );
                        out.rectangles.push(Rectangle {
                            self_id: attr(&e, b"Self"),
                            bounds,
                            item_transform,
                            fill_color: attr(&e, b"FillColor"),
                            stroke_color: attr(&e, b"StrokeColor"),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                            drop_shadow: None,
                            image_link: None,
                        });
                        current_frame = Some(CurrentFrame::Rect(out.rectangles.len() - 1));
                    }
                    b"Oval" => {
                        let Some(bounds) =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s))
                        else {
                            continue;
                        };
                        let item_transform = effective_item_transform(
                            &group_transforms,
                            attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s)),
                        );
                        out.ovals.push(Oval {
                            self_id: attr(&e, b"Self"),
                            bounds,
                            item_transform,
                            fill_color: attr(&e, b"FillColor"),
                            stroke_color: attr(&e, b"StrokeColor"),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                            drop_shadow: None,
                        });
                        current_frame = Some(CurrentFrame::Oval(out.ovals.len() - 1));
                    }
                    b"DropShadowSetting" => {
                        if let (Some(cf), Some(setting)) = (current_frame, parse_drop_shadow(&e)) {
                            // Only "Drop"/"Default" mode results in a
                            // visible shadow. "None" means the shadow
                            // is disabled even though the setting is
                            // serialised.
                            if setting.mode != "None" {
                                match cf {
                                    CurrentFrame::Text(i) => {
                                        out.text_frames[i].drop_shadow = Some(setting);
                                    }
                                    CurrentFrame::Rect(i) => {
                                        out.rectangles[i].drop_shadow = Some(setting);
                                    }
                                    CurrentFrame::Oval(i) => {
                                        out.ovals[i].drop_shadow = Some(setting);
                                    }
                                }
                            }
                        }
                    }
                    b"TextFramePreference" => {
                        // Attach VerticalJustification to the
                        // current TextFrame. Other knobs on
                        // TextFramePreference (insets, columns,
                        // FirstBaselineOffset) land here too once
                        // the renderer consumes them.
                        if let (Some(CurrentFrame::Text(i)), Some(vj)) =
                            (current_frame, attr(&e, b"VerticalJustification"))
                        {
                            out.text_frames[i].vertical_justification = Some(vj);
                        }
                    }
                    b"Image" | b"Link" => {
                        // IDML's image-bearing rectangle nests an
                        // <Image> with a LinkResourceURI on the
                        // element itself or on its <Link> child.
                        // Either source attaches to the current
                        // Rectangle (the only frame type that hosts
                        // images in this slice).
                        if let (Some(CurrentFrame::Rect(i)), Some(uri)) = (
                            current_frame,
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
                        let Some(bounds) =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s))
                        else {
                            continue;
                        };
                        let item_transform = effective_item_transform(
                            &group_transforms,
                            attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s)),
                        );
                        out.graphic_lines.push(GraphicLine {
                            self_id: attr(&e, b"Self"),
                            bounds,
                            item_transform,
                            stroke_color: attr(&e, b"StrokeColor"),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                        });
                    }
                    _ => {}
                },
                Event::End(e) => match e.name().as_ref() {
                    b"Group" if !group_transforms.is_empty() => {
                        group_transforms.pop();
                    }
                    b"TextFrame" | b"Rectangle" | b"Oval" => {
                        // Frame is fully parsed; clear the
                        // drop-shadow attachment context.
                        current_frame = None;
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
}
