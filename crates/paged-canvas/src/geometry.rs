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

/// Vertical caret navigation: given a story offset and a direction,
/// return the story offset the caret lands on one visible line above
/// (`Up`) or below (`Down`), targeting the line at the column nearest
/// the source caret's x. Mirrors the standard text-editor "remember
/// the goal column" behaviour for a single step (the caller threads a
/// sticky goal-x across consecutive presses if it wants iOS/desktop
/// fidelity; one step from the live x is the honest engine primitive).
///
/// Returns `None` when the story has no captured layout, or when
/// there's no line in the requested direction (caret already on the
/// first/last visible line) — the caller leaves the caret put.
///
/// Lines are gathered in document order (`story_layout` sorts by
/// `(paragraph_idx, line_idx)`), so "above"/"below" means the
/// previous/next entry in that flattened sequence — correct across
/// paragraph and frame/page boundaries in a threaded story.
pub fn caret_nav(
    built: &BuiltDocument,
    story_id: &str,
    offset: u32,
    direction: CaretDirection,
) -> Option<u32> {
    let lines: Vec<&LineLayout> = built.story_layout(story_id);
    if lines.is_empty() {
        return None;
    }
    // Per-line absolute [start, end] story offsets (paragraph offset
    // + line byte range), parallel to `lines`.
    let spans: Vec<(u32, u32)> = lines
        .iter()
        .map(|line| {
            let para_start =
                paragraph_byte_offset(built, story_id, line.paragraph_idx);
            (
                para_start + line.byte_range.start,
                para_start + line.byte_range.end,
            )
        })
        .collect();

    // Locate the current line: the first whose [start, end] contains
    // the offset. At a line-break boundary (offset == end of line i ==
    // start of line i+1) we prefer the EARLIER line for Up and the
    // LATER line for Down, so a round-trip up-then-down is stable.
    let mut current = None;
    for (i, &(s, e)) in spans.iter().enumerate() {
        if offset >= s && offset <= e {
            current = Some(i);
            // For a downward move at a boundary, prefer the later line.
            if !(offset == e
                && direction == CaretDirection::Down
                && i + 1 < spans.len()
                && spans[i + 1].0 == e)
            {
                break;
            }
        }
    }
    let current = current?;

    let target = match direction {
        CaretDirection::Up => {
            if current == 0 {
                return None;
            }
            current - 1
        }
        CaretDirection::Down => {
            if current + 1 >= lines.len() {
                return None;
            }
            current + 1
        }
    };

    // Source caret x in the current line, then the nearest cluster x
    // on the target line → its byte offset.
    let cur_line = lines[current];
    let cur_rel = offset.saturating_sub(spans[current].0);
    let goal_x = caret_x_in_line(cur_line, cur_rel);
    let tgt_line = lines[target];
    let (tgt_start, tgt_end) = spans[target];
    let local = nearest_byte_for_x(tgt_line, goal_x);
    Some((tgt_start + local).min(tgt_end))
}

/// Story `[line_start, line_end]` offsets for the visible line
/// containing `offset` (the line the caret sits on). Powers Home/End
/// and shift-Home/shift-End without the editor re-deriving line
/// breaks. `None` when the story has no layout or the offset doesn't
/// fall on any visible line.
pub fn line_bounds(
    built: &BuiltDocument,
    story_id: &str,
    offset: u32,
) -> Option<LineBounds> {
    let lines: Vec<&LineLayout> = built.story_layout(story_id);
    for line in &lines {
        let para_start =
            paragraph_byte_offset(built, story_id, line.paragraph_idx);
        let line_start = para_start + line.byte_range.start;
        let line_end = para_start + line.byte_range.end;
        if offset >= line_start && offset <= line_end {
            return Some(LineBounds {
                line_start,
                line_end,
            });
        }
    }
    None
}

/// `RequestLineBounds` reply payload.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi, missing_as_null)]
#[serde(rename_all = "camelCase")]
pub struct LineBounds {
    /// Story offset of the line's first character.
    pub line_start: u32,
    /// Story offset just past the line's last character.
    pub line_end: u32,
}

/// Direction for [`caret_nav`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub enum CaretDirection {
    Up,
    Down,
}

/// Line-local byte offset (relative to `byte_range.start`) whose
/// caret x sits nearest `goal_x`. Used by vertical caret nav to
/// preserve the column. Candidates are each cluster's leading edge
/// plus the line's trailing edge (end-of-line). Falls back to 0 when
/// no clusters were captured.
fn nearest_byte_for_x(line: &LineLayout, goal_x: f32) -> u32 {
    if line.clusters.is_empty() {
        return 0;
    }
    // Each cluster contributes its leading-edge (byte, x). The final
    // candidate is the line end (trailing edge of the last cluster).
    let mut best_byte = line.byte_range.start;
    let mut best_dist = f32::INFINITY;
    for c in &line.clusters {
        let d = (c.x_pt - goal_x).abs();
        if d < best_dist {
            best_dist = d;
            best_byte = c.byte;
        }
    }
    let last = *line.clusters.last().unwrap();
    let trail_x = last.x_pt + last.advance_pt;
    if (trail_x - goal_x).abs() < best_dist {
        best_byte = line.byte_range.end;
    }
    best_byte.saturating_sub(line.byte_range.start)
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

    /// A frame narrow enough to wrap "Hello world." onto two lines.
    /// PointSize 36 in a 160pt-wide frame breaks after "Hello".
    fn load_two_line_model() -> CanvasModel {
        use std::io::Write;
        use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored =
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
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
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"><Graphic/></idPkg:Graphic>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 400 612"/>
    <TextFrame Self="frameA" ParentStory="u10" GeometricBounds="40 40 380 200" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();
        zip.start_file("Stories/Story_u10.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange>
      <CharacterStyleRange AppliedFont="Inter" PointSize="36">
        <Content>Hello world.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#,
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let opts = CanvasOptions {
            fonts: vec![read_font("Inter.ttf")],
            ..Default::default()
        };
        CanvasModel::load("d", &bytes, opts).unwrap()
    }

    #[test]
    fn caret_nav_down_then_up_round_trips_to_the_first_line() {
        let model = load_two_line_model();
        let built = model.built();
        // Confirm the fixture actually wrapped to ≥2 lines.
        let lines = built.story_layout("u10");
        assert!(lines.len() >= 2, "fixture should wrap; got {} line(s)", lines.len());

        // From a caret on line 0 (offset 2, inside "Hello"), Down lands
        // on line 1, and Up from there returns to line 0.
        let down = caret_nav(built, "u10", 2, CaretDirection::Down)
            .expect("a line below line 0");
        // The destination must be past the first line's end (i.e. on
        // the second visible line).
        let l0_end = {
            let l = lines[0];
            let ps = crate::hit::paragraph_byte_offset(built, "u10", l.paragraph_idx);
            ps + l.byte_range.end
        };
        assert!(down >= l0_end, "down ({down}) should be on the next line (≥ {l0_end})");

        let back_up = caret_nav(built, "u10", down, CaretDirection::Up)
            .expect("a line above line 1");
        assert!(back_up <= l0_end, "up ({back_up}) should land back on line 0 (≤ {l0_end})");
    }

    #[test]
    fn caret_nav_up_from_first_line_is_none() {
        let model = load_two_line_model();
        assert!(
            caret_nav(model.built(), "u10", 1, CaretDirection::Up).is_none(),
            "no line above the first"
        );
    }

    #[test]
    fn caret_nav_down_from_last_line_is_none() {
        let model = load_two_line_model();
        let built = model.built();
        let lines = built.story_layout("u10");
        // Offset on the last visible line.
        let last = lines.last().unwrap();
        let ps = crate::hit::paragraph_byte_offset(built, "u10", last.paragraph_idx);
        let last_offset = ps + last.byte_range.start;
        assert!(
            caret_nav(built, "u10", last_offset, CaretDirection::Down).is_none(),
            "no line below the last"
        );
    }

    #[test]
    fn line_bounds_returns_the_containing_line_span() {
        let model = load_two_line_model();
        let built = model.built();
        let b = line_bounds(built, "u10", 2).expect("line bounds for offset 2");
        // The first line starts at 0 and ends before the second line.
        assert_eq!(b.line_start, 0);
        assert!(b.line_end >= 2, "line_end {} must cover offset 2", b.line_end);
        // An offset on a later line yields a different, non-overlapping span.
        let lines = built.story_layout("u10");
        if lines.len() >= 2 {
            let l1 = lines[1];
            let ps = crate::hit::paragraph_byte_offset(built, "u10", l1.paragraph_idx);
            let mid = ps + l1.byte_range.start;
            let b2 = line_bounds(built, "u10", mid).expect("line bounds line 1");
            assert!(b2.line_start >= b.line_end, "line 1 starts after line 0 ends");
        }
    }

    #[test]
    fn line_bounds_for_unknown_story_is_none() {
        let model = load_model("Hello");
        assert!(line_bounds(model.built(), "nope", 0).is_none());
    }
}
