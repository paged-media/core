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

//! Conditional text: which runs print.
//!
//! Asked of InDesign 20.0.1 (the `conditions` fixture, probed with
//! `tools/indesign-export/conditions-probe.sh`, toggling each condition so
//! InDesign applies the `Visible` flags it read):
//!
//! * a range under several conditions prints while ANY of them is
//!   visible, and is hidden only when every one it names is hidden;
//! * a range that names a condition the document does not declare
//!   prints: InDesign drops the whole `AppliedConditions` list on import
//!   (its own export writes that range ungated), so `{undeclared}` and
//!   `{Hidden, undeclared}` both print — an undeclared condition counts as
//!   a visible one;
//! * an anchored object's marker, a footnote reference and the text of a
//!   table cell follow the same rule; the object or the note goes with
//!   its marker.
//!
//! InDesign writes hidden text inside `<HiddenText>`, which the importer
//! drops, so a file InDesign saved arrives with nothing left to hide.
//! What this filter acts on is text whose conditions say it is hidden
//! while it still sits in the story: a hand-written file, or a condition
//! hidden after import.

use std::collections::BTreeMap;

use paged_model::{CharacterRun, ConditionDef, Paragraph, Table};

/// Whether a run under `applied` prints (see the module doc).
pub(super) fn run_prints(applied: &[String], declared: &BTreeMap<String, ConditionDef>) -> bool {
    applied.is_empty()
        || applied
            .iter()
            .any(|id| declared.get(id).is_none_or(|c| c.visible.unwrap_or(true)))
}

/// Which side of a range boundary an attachment at that offset belongs
/// to. The model keeps only a character offset, which on a boundary names
/// two ranges; InDesign writes an anchored object BEFORE the text of the
/// range it sits in and a footnote reference AFTER it, so the boundary
/// goes to the range starting there or ending there respectively. A
/// reference whose marker character stands at the offset is that
/// character, and goes with the range holding it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    /// Anchored objects: the range that starts at the offset.
    Starts,
    /// Footnote references recorded without their character: the range
    /// that ends at the offset.
    Ends,
}

/// The run an attachment at `at` sits in. A range with no text standing
/// exactly at `at` holds nothing but attachments, so it wins; past the
/// end of the text the last run hosts it (as `with_object_placeholders`
/// does).
fn host_run(spans: &[(u32, u32)], at: u32, side: Side) -> Option<usize> {
    if let Some(i) = spans.iter().position(|&(s, e)| s == e && s == at) {
        return Some(i);
    }
    let found = match side {
        Side::Starts => spans.iter().position(|&(s, e)| s <= at && at < e),
        Side::Ends => spans.iter().position(|&(s, e)| s < at && at <= e),
    };
    found.or_else(|| match side {
        Side::Starts => spans.len().checked_sub(1),
        Side::Ends => spans.iter().position(|&(_, e)| e >= at),
    })
}

/// `paragraph` without the runs its conditions hide, or `None` when it
/// hides nothing. The anchored objects and footnotes whose markers sat in
/// a dropped run go with it; the rest are re-anchored in the shorter
/// text. A table on the paragraph is filtered cell by cell.
pub(super) fn without_hidden_runs(
    paragraph: &Paragraph,
    declared: &BTreeMap<String, ConditionDef>,
) -> Option<Paragraph> {
    let hides = |r: &CharacterRun| {
        !r.applied_conditions.is_empty() && !run_prints(&r.applied_conditions, declared)
    };
    let table = paragraph
        .table
        .as_ref()
        .and_then(|t| table_without_hidden_runs(t, declared));
    if table.is_none() && !paragraph.runs.iter().any(hides) {
        return None;
    }

    // Each run's character span, and how many hidden characters precede
    // each offset.
    let mut spans = Vec::with_capacity(paragraph.runs.len());
    let mut pos = 0u32;
    for r in &paragraph.runs {
        let n = r.text.chars().count() as u32;
        spans.push((pos, pos + n));
        pos += n;
    }
    let hidden: Vec<bool> = paragraph.runs.iter().map(hides).collect();
    let hidden_before = |at: u32| -> u32 {
        spans
            .iter()
            .zip(&hidden)
            .filter(|(_, &h)| h)
            .map(|(&(s, e), _)| e.min(at).saturating_sub(s))
            .sum()
    };
    let kept = |at: u32, side: Side| host_run(&spans, at, side).is_none_or(|i| !hidden[i]);

    let mut out = Paragraph {
        runs: paragraph
            .runs
            .iter()
            .zip(&hidden)
            .filter(|(_, &h)| !h)
            .map(|(r, _)| r.clone())
            .collect(),
        anchored_frames: Vec::with_capacity(paragraph.anchored_frames.len()),
        anchored_frame_offsets: Vec::with_capacity(paragraph.anchored_frame_offsets.len()),
        footnotes: Vec::with_capacity(paragraph.footnotes.len()),
        table: table.or_else(|| paragraph.table.clone()),
        ..paragraph.clone()
    };
    for (i, frame) in paragraph.anchored_frames.iter().enumerate() {
        let at = paragraph
            .anchored_frame_offsets
            .get(i)
            .copied()
            .unwrap_or(0);
        if kept(at, Side::Starts) {
            out.anchored_frames.push(frame.clone());
            out.anchored_frame_offsets.push(at - hidden_before(at));
        }
    }
    // A note whose reference character stands at its anchor goes with
    // the run holding that character; an anchor with no character (an
    // older import) with the run ending there.
    let chars: Vec<char> = paragraph.runs.iter().flat_map(|r| r.text.chars()).collect();
    for note in &paragraph.footnotes {
        let at = note.anchor.unwrap_or(0);
        let side = if chars.get(at as usize) == Some(&paged_model::FOOTNOTE_REFERENCE_MARKER) {
            Side::Starts
        } else {
            Side::Ends
        };
        if kept(at, side) {
            let mut note = note.clone();
            note.anchor = note.anchor.map(|a| a - hidden_before(a));
            out.footnotes.push(note);
        }
    }
    out.table_anchor = paragraph.table_anchor.map(|a| a - hidden_before(a));
    Some(out)
}

/// `table` with every cell paragraph filtered, or `None` when no cell
/// hides anything.
fn table_without_hidden_runs(
    table: &Table,
    declared: &BTreeMap<String, ConditionDef>,
) -> Option<Table> {
    let mut out: Option<Table> = None;
    for (ci, cell) in table.cells.iter().enumerate() {
        for (pi, p) in cell.paragraphs.iter().enumerate() {
            if let Some(filtered) = without_hidden_runs(p, declared) {
                out.get_or_insert_with(|| table.clone()).cells[ci].paragraphs[pi] = filtered;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_model::{AnchoredFrame, AnchoredFrameKind, Footnote, TableCell};

    fn object() -> AnchoredFrame {
        AnchoredFrame {
            frame_kind: AnchoredFrameKind::Rectangle,
            self_id: None,
            bounds: None,
            item_transform: None,
            parent_story: None,
            setting: None,
            fill_color: None,
            stroke_color: None,
            stroke_weight: None,
            fill_tint: None,
            gradient_fill_angle: None,
            applied_object_style: None,
            image_link: None,
            image_item_transform: None,
            children: Vec::new(),
        }
    }

    fn run(text: &str, conditions: &[&str]) -> CharacterRun {
        CharacterRun {
            text: text.into(),
            applied_conditions: conditions.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn declared(defs: &[(&str, bool)]) -> BTreeMap<String, ConditionDef> {
        defs.iter()
            .map(|&(id, visible)| {
                (
                    id.to_string(),
                    ConditionDef {
                        self_id: id.into(),
                        visible: Some(visible),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    fn texts(p: &Paragraph) -> Vec<&str> {
        p.runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn run_prints_while_any_declared_condition_is_visible() {
        let d = declared(&[("V", true), ("H", false), ("H2", false)]);
        let prints =
            |c: &[&str]| run_prints(&c.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &d);
        assert!(prints(&[]));
        assert!(prints(&["V"]));
        assert!(!prints(&["H"]));
        assert!(prints(&["V", "H"]));
        assert!(prints(&["H", "V"]));
        assert!(!prints(&["H", "H2"]));
        // InDesign drops a list naming an undeclared condition on import.
        assert!(prints(&["Undeclared"]));
        assert!(prints(&["H", "Undeclared"]));
    }

    #[test]
    fn nothing_hidden_is_none() {
        let d = declared(&[("V", true), ("H", false)]);
        let p = Paragraph {
            runs: vec![run("a", &[]), run("b", &["V", "H"])],
            ..Default::default()
        };
        assert!(without_hidden_runs(&p, &d).is_none());
    }

    #[test]
    fn hidden_runs_drop_and_attachments_follow_their_markers() {
        let d = declared(&[("V", true), ("H", false)]);
        // "ab" | "CDE" (hidden) | "fg"
        //  0..2   2..5            5..7
        let p = Paragraph {
            runs: vec![run("ab", &[]), run("CDE", &["H"]), run("fg", &[])],
            // An object written before the hidden run's text (offset 2)
            // goes with it; one before "fg" (offset 5) stays, at 2.
            anchored_frames: vec![object(), object()],
            anchored_frame_offsets: vec![2, 5],
            // A reference written after "CDE" (offset 5) goes with it; one
            // after "ab" (offset 2) stays at 2, one after "fg" moves to 4.
            footnotes: vec![
                Footnote {
                    self_id: Some("hidden".into()),
                    anchor: Some(5),
                    ..Default::default()
                },
                Footnote {
                    self_id: Some("before".into()),
                    anchor: Some(2),
                    ..Default::default()
                },
                Footnote {
                    self_id: Some("after".into()),
                    anchor: Some(7),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let out = without_hidden_runs(&p, &d).expect("a run is hidden");
        assert_eq!(texts(&out), ["ab", "fg"]);
        assert_eq!(out.anchored_frames.len(), 1);
        assert_eq!(out.anchored_frame_offsets, [2]);
        let notes: Vec<_> = out
            .footnotes
            .iter()
            .map(|f| (f.self_id.as_deref().unwrap(), f.anchor.unwrap()))
            .collect();
        assert_eq!(notes, [("before", 2), ("after", 4)]);
    }

    #[test]
    fn a_range_holding_only_an_object_hides_it() {
        let d = declared(&[("H", false)]);
        let p = Paragraph {
            runs: vec![run("ab", &[]), run("", &["H"]), run("cd", &[])],
            anchored_frames: vec![object()],
            anchored_frame_offsets: vec![2],
            ..Default::default()
        };
        let out = without_hidden_runs(&p, &d).expect("a run is hidden");
        assert_eq!(texts(&out), ["ab", "cd"]);
        assert!(out.anchored_frames.is_empty());
    }

    #[test]
    fn table_cells_are_filtered() {
        let d = declared(&[("V", true), ("H", false)]);
        let cell = |runs| TableCell {
            paragraphs: vec![Paragraph {
                runs,
                ..Default::default()
            }],
            ..Default::default()
        };
        let p = Paragraph {
            table: Some(Table {
                cells: vec![
                    cell(vec![run("10 ", &[]), run("KILO", &["H"]), run(".", &[])]),
                    cell(vec![run("11 ", &[]), run("LIMA", &["V", "H"])]),
                ],
                ..Default::default()
            }),
            ..Default::default()
        };
        let out = without_hidden_runs(&p, &d).expect("a cell hides a run");
        let cells = &out.table.unwrap().cells;
        assert_eq!(texts(&cells[0].paragraphs[0]), ["10 ", "."]);
        assert_eq!(texts(&cells[1].paragraphs[0]), ["11 ", "LIMA"]);
    }
}
