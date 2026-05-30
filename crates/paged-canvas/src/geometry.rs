//! Caret + selection geometry computations.
//!
//! Phase 3 correctness layer (Items 3 + 4).
//!
//! Given a `ContentSelection` and a `BuiltDocument` (with the Item-A
//! StoryLayout captured), these functions return:
//!
//! - `CaretGeometry` — single (page_id, x_pt, top_pt, height_pt)
//!   for the caret position. Honours the `affinity` bit at line
//!   breaks per the rule in `selection.rs`.
//! - `Vec<SelectionRect>` — one rect per visible line for the
//!   selection range. Splits on frame boundaries when the story is
//!   threaded across pages.

use paged_renderer::{BuiltDocument, LineLayout, PageId};
use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

use crate::hit::paragraph_byte_offset;
use crate::selection::ContentSelection;
use crate::SelectionRect;

#[derive(Debug, Clone, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct CaretGeometry {
    pub page_id: PageId,
    pub frame_id: Option<String>,
    /// Page-local x of the caret leading edge.
    pub x_pt: f32,
    /// Page-local y of the caret top (baseline - ascent).
    pub top_pt: f32,
    /// Total caret height (ascent + descent).
    pub height_pt: f32,
}

/// Caret position for `sel.start` (caret selections) or for the
/// active end of a range selection (caller picks; we use `start`
/// as the conservative default — drag-extend code should pass the
/// drag head as a synthetic caret selection).
///
/// Returns `None` when the story has no captured layout (e.g. the
/// document carries no fonts so no glyphs were emitted) — the caller
/// can render a no-caret placeholder.
pub fn caret_geometry(
    built: &BuiltDocument,
    sel: &ContentSelection,
) -> Option<CaretGeometry> {
    let lines: Vec<&LineLayout> = built.story_layout(&sel.story_id);
    if lines.is_empty() {
        return None;
    }
    let target_offset = sel.start;

    // Find the line whose [start, end] character span contains the
    // caret offset, accounting for affinity at line breaks.
    // Walk in document order; the *first* line whose end ≥ offset
    // is the one we're at... unless affinity is downstream and the
    // caret sits on a line break, in which case we use the *next*
    // line's start.
    let mut chosen: Option<(&&LineLayout, u32)> = None; // (line, line_start_in_story)
    let mut prev: Option<(&&LineLayout, u32)> = None;
    for line in &lines {
        let para_start = paragraph_byte_offset(built, &sel.story_id, line.paragraph_idx);
        let line_start = para_start + line.byte_range.start;
        let line_end = para_start + line.byte_range.end;
        if target_offset < line_start {
            // Caret is before this line. If we had a prev line and
            // the offset equals prev's end (line-break boundary),
            // pick *this* line when affinity is downstream.
            if let Some((p, _)) = prev {
                let p_para_start =
                    paragraph_byte_offset(built, &sel.story_id, p.paragraph_idx);
                let p_end = p_para_start + p.byte_range.end;
                if sel.affinity && target_offset == p_end {
                    chosen = Some((line, line_start));
                }
            }
            break;
        }
        if target_offset >= line_start && target_offset <= line_end {
            chosen = Some((line, line_start));
            // If the caret is exactly at line end AND downstream
            // affinity is set, continue to pick the NEXT line on
            // the next iteration.
            if !(sel.affinity && target_offset == line_end) {
                break;
            }
        }
        prev = Some((line, line_start));
    }
    let (line, line_start) = chosen?;
    let relative = target_offset - line_start;

    // Bisect cluster x by the relative byte offset. The caret sits
    // at the leading edge of the cluster whose `byte` == relative;
    // if no exact match, snap to the cluster that contains it.
    let x_pt = caret_x_in_line(line, relative);
    Some(CaretGeometry {
        page_id: line.page_id.clone(),
        frame_id: line.frame_id.clone(),
        x_pt,
        top_pt: line.baseline_y_pt - line.ascent_pt,
        height_pt: line.ascent_pt + line.descent_pt,
    })
}

/// One rect per visible line covered by `[sel.start, sel.end)`.
/// Empty when `sel` is a caret or when the story has no layout.
pub fn selection_geometry(
    built: &BuiltDocument,
    sel: &ContentSelection,
) -> Vec<SelectionRect> {
    if sel.is_caret() {
        return Vec::new();
    }
    let lines: Vec<&LineLayout> = built.story_layout(&sel.story_id);
    let mut out: Vec<SelectionRect> = Vec::new();
    for line in &lines {
        let para_start = paragraph_byte_offset(built, &sel.story_id, line.paragraph_idx);
        let line_start = para_start + line.byte_range.start;
        let line_end = para_start + line.byte_range.end;
        // Line is in the selection if its range intersects [sel.start, sel.end).
        if line_end < sel.start {
            continue;
        }
        if line_start >= sel.end {
            break;
        }
        let local_start = sel.start.saturating_sub(line_start);
        let local_end = (sel.end - line_start).min(line.byte_range.end);
        let left = caret_x_in_line(line, local_start);
        let right = caret_x_in_line(line, local_end);
        // Empty-line case: emit a thin ~1-em rect (1pt advance) so
        // the user sees a visible cue on blank lines.
        let width = if right > left {
            right - left
        } else {
            line.ascent_pt.max(8.0) * 0.5
        };
        out.push(SelectionRect {
            page_id: line.page_id.clone(),
            frame_id: line.frame_id.clone(),
            left_pt: left,
            top_pt: line.baseline_y_pt - line.ascent_pt,
            width_pt: width,
            height_pt: line.ascent_pt + line.descent_pt,
        });
    }
    out
}

/// x-coordinate of the caret at line-local byte offset `rel`.
///
/// - `rel == 0` → leading edge of first cluster (or line origin
///   when no clusters captured).
/// - `rel == byte_range.end - byte_range.start` (end-of-line) →
///   trailing edge of last cluster.
/// - Otherwise → leading edge of the cluster whose `byte == rel`,
///   or the closest cluster's leading edge if no exact match.
fn caret_x_in_line(line: &LineLayout, rel: u32) -> f32 {
    if line.clusters.is_empty() {
        // No glyphs captured — fall back to ascent-relative left
        // edge approximation (callers shouldn't hit this in normal
        // usage but we guard against panics).
        return 0.0;
    }
    let target = line.byte_range.start + rel;
    if target <= line.clusters[0].byte {
        return line.clusters[0].x_pt;
    }
    for c in &line.clusters {
        if c.byte >= target {
            return c.x_pt;
        }
    }
    // Past all clusters: trailing edge of the last one.
    let last = *line.clusters.last().unwrap();
    last.x_pt + last.advance_pt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CanvasModel, CanvasOptions};
    use paged_renderer::BytesResolver;
    use std::path::PathBuf;

    fn font_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/fonts")
    }

    fn read_font(name: &str) -> Vec<u8> {
        std::fs::read(font_dir().join(name))
            .unwrap_or_else(|e| panic!("read font fixture {name}: {e}"))
    }

    fn build_idml(text: &str) -> Vec<u8> {
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();

        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Resources/Graphic.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 380 572" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        let story = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="36">
        <Content>{text}</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
        );
        zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
        zip.write_all(story.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    fn load_model(text: &str) -> CanvasModel {
        let bytes = build_idml(text);
        let _resolver: BytesResolver = BytesResolver::new();
        let opts = CanvasOptions {
            fonts: vec![read_font("Inter.ttf")],
            ..Default::default()
        };
        CanvasModel::load("d", &bytes, opts).unwrap()
    }

    #[test]
    fn caret_at_start_of_story_is_leftmost_cluster() {
        let model = load_model("Hello world.");
        let sel = ContentSelection::caret("u10", 0);
        let g = caret_geometry(model.built(), &sel).expect("caret geometry");
        // First cluster's x_pt should be near text_origin (40 + insets ≈ 40).
        assert!(
            g.x_pt > 35.0 && g.x_pt < 60.0,
            "caret x out of expected range: {}",
            g.x_pt
        );
        assert!(g.height_pt > 0.0);
        assert_eq!(g.frame_id.as_deref(), Some("frameA"));
    }

    #[test]
    fn caret_at_end_of_line_is_trailing_edge() {
        let model = load_model("Hi.");
        let sel = ContentSelection::caret("u10", 3);
        let g = caret_geometry(model.built(), &sel).expect("caret at end");
        // x must exceed the leftmost x (we have at least one glyph
        // before the end).
        let leftmost = caret_geometry(model.built(), &ContentSelection::caret("u10", 0))
            .unwrap()
            .x_pt;
        assert!(g.x_pt > leftmost);
    }

    #[test]
    fn selection_geometry_for_range_returns_one_rect_per_line() {
        let model = load_model("Hello world.");
        let sel = ContentSelection::range("u10", 0, 5);
        let rects = selection_geometry(model.built(), &sel);
        assert_eq!(rects.len(), 1, "single-line selection → 1 rect");
        let r = &rects[0];
        assert!(r.width_pt > 0.0);
        assert_eq!(r.frame_id.as_deref(), Some("frameA"));
    }

    #[test]
    fn selection_geometry_for_caret_is_empty() {
        let model = load_model("Hello");
        let rects = selection_geometry(model.built(), &ContentSelection::caret("u10", 2));
        assert!(rects.is_empty());
    }
}
