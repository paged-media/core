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

impl Spread {
    pub fn parse(xml: &[u8]) -> Result<Self, ParseError> {
        let mut reader = quick_xml::Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut out = Spread::default();
        let mut group_depth: usize = 0;
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
                        group_depth += 1;
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
                        if group_depth > 0 {
                            out.skipped_nested_frames += 1;
                            continue;
                        }
                        let parent_story = attr(&e, b"ParentStory");
                        let item_transform =
                            attr(&e, b"ItemTransform").and_then(|s| parse_matrix(&s));
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
                        });
                    }
                    b"Rectangle" => {
                        let Some(bounds) =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s))
                        else {
                            continue;
                        };
                        if group_depth > 0 {
                            out.skipped_nested_frames += 1;
                            continue;
                        }
                        out.rectangles.push(Rectangle {
                            self_id: attr(&e, b"Self"),
                            bounds,
                            item_transform: attr(&e, b"ItemTransform")
                                .and_then(|s| parse_matrix(&s)),
                            fill_color: attr(&e, b"FillColor"),
                            stroke_color: attr(&e, b"StrokeColor"),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                        });
                    }
                    b"Oval" => {
                        let Some(bounds) =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s))
                        else {
                            continue;
                        };
                        if group_depth > 0 {
                            out.skipped_nested_frames += 1;
                            continue;
                        }
                        out.ovals.push(Oval {
                            self_id: attr(&e, b"Self"),
                            bounds,
                            item_transform: attr(&e, b"ItemTransform")
                                .and_then(|s| parse_matrix(&s)),
                            fill_color: attr(&e, b"FillColor"),
                            stroke_color: attr(&e, b"StrokeColor"),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                        });
                    }
                    b"GraphicLine" => {
                        let Some(bounds) =
                            attr(&e, b"GeometricBounds").and_then(|s| parse_bounds(&s))
                        else {
                            continue;
                        };
                        if group_depth > 0 {
                            out.skipped_nested_frames += 1;
                            continue;
                        }
                        out.graphic_lines.push(GraphicLine {
                            self_id: attr(&e, b"Self"),
                            bounds,
                            item_transform: attr(&e, b"ItemTransform")
                                .and_then(|s| parse_matrix(&s)),
                            stroke_color: attr(&e, b"StrokeColor"),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                        });
                    }
                    _ => {}
                },
                Event::End(e) => {
                    if e.name().as_ref() == b"Group" && group_depth > 0 {
                        group_depth -= 1;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }
}

fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| std::str::from_utf8(&a.value).ok().map(str::to_string))
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
    fn skips_frames_inside_groups() {
        let xml =
            br#"<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Spread Self="s">
            <TextFrame Self="top" ParentStory="u1" GeometricBounds="0 0 100 200"/>
            <Group>
              <TextFrame Self="inner" ParentStory="u2" GeometricBounds="0 0 50 50"/>
            </Group>
            <TextFrame Self="after" ParentStory="u3" GeometricBounds="0 0 100 200"/>
          </Spread>
        </idPkg:Spread>"#;
        let s = Spread::parse(xml).unwrap();
        assert_eq!(s.text_frames.len(), 2);
        assert_eq!(s.skipped_nested_frames, 1);
        assert_eq!(s.text_frames[0].self_id.as_deref(), Some("top"));
        assert_eq!(s.text_frames[1].self_id.as_deref(), Some("after"));
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
