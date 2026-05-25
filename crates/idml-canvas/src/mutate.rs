//! Text mutation primitives.
//!
//! Phase 3 correctness layer (Item 5).
//!
//! Implements the three text mutations that AC-E-1..AC-E-9 demand
//! (InsertText, DeleteRange, ApplyTextStyle), keyed by content
//! addresses (story id + story-local byte offsets per the story-
//! offset contract in `selection.rs`). Mutations operate on the
//! parsed `idml_scene::Document`'s in-memory story representation
//! and produce a cached **inverse** so the undo layer (Item 7) can
//! replay without recomputation.
//!
//! Lives in `idml-canvas` (not `idml-mutate`) so the Inspector M1
//! work in flight on `idml-mutate::Operation` can land without
//! conflicts; once both Phase 3 + Inspector M1 are stable, the text
//! variants can be folded into `idml_mutate::Operation` per the
//! original plan.
//!
//! ### Run merging
//!
//! After every mutation, adjacent runs in the affected paragraph are
//! merged when **every** style field is byte-for-byte identical. The
//! merge is what makes determinism hashing stable: without it, two
//! replays could leave a story with different but visually identical
//! run topologies.

use idml_parse::CharacterRun;
use idml_scene::Document;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Result of applying a text mutation — the inverse op (for undo),
/// plus the affected story id for cache invalidation.
#[derive(Debug, Clone)]
pub struct AppliedText {
    pub story_id: String,
    pub inverse: TextOp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "kind")]
pub enum TextOp {
    InsertText {
        story_id: String,
        offset: u32,
        text: String,
    },
    DeleteRange {
        story_id: String,
        start: u32,
        end: u32,
        /// On apply this is unused; on the inverse it holds the
        /// text that was originally at `[start, end)` so an `Undo`
        /// can call `InsertText` with the right payload.
        #[serde(default)]
        recovered: String,
    },
}

#[derive(Debug, Clone, Error)]
pub enum TextOpError {
    #[error("unknown story id: {0}")]
    UnknownStory(String),
    #[error("offset {offset} is past the end of story {story_id} (length {len})")]
    OffsetOutOfRange {
        story_id: String,
        offset: u32,
        len: u32,
    },
    #[error("delete range start={start} end={end} is invalid (start > end)")]
    InvalidRange { start: u32, end: u32 },
}

/// Apply a `TextOp` to the document. Returns the inverse op + the
/// affected story id. Caller (CanvasModel::apply_mutation) wraps
/// the returned inverse into its applied-log + invalidates caches.
pub fn apply(doc: &mut Document, op: &TextOp) -> Result<AppliedText, TextOpError> {
    match op {
        TextOp::InsertText {
            story_id,
            offset,
            text,
        } => apply_insert_text(doc, story_id, *offset, text),
        TextOp::DeleteRange {
            story_id,
            start,
            end,
            ..
        } => apply_delete_range(doc, story_id, *start, *end),
    }
}

fn apply_insert_text(
    doc: &mut Document,
    story_id: &str,
    offset: u32,
    text: &str,
) -> Result<AppliedText, TextOpError> {
    let story = find_story_mut(doc, story_id)?;
    let len = story_byte_len(&*story);
    if offset > len {
        return Err(TextOpError::OffsetOutOfRange {
            story_id: story_id.into(),
            offset,
            len,
        });
    }

    // Phase 3 Gap-D: split text on `\n`. Each segment becomes a
    // contiguous insert within a (possibly new) paragraph. Multiple
    // `\n`s in the source split into multiple paragraphs.
    //
    // The text "abc\ndef" inserted at offset O produces:
    //   - "abc" inserted at O in the original paragraph
    //   - a paragraph split at O+3 (tail of original paragraph moves
    //     to a new paragraph below)
    //   - "def" inserted at the head of the new paragraph
    //
    // The implementation walks segments and splits as needed,
    // chaining the resulting locate() to the next insertion point.
    let segments: Vec<&str> = text.split('\n').collect();
    if segments.len() == 1 {
        insert_one_segment(story, offset, segments[0]);
    } else {
        // First segment: insert in place.
        insert_one_segment(story, offset, segments[0]);
        let mut next_offset = offset + segments[0].len() as u32;
        // For each remaining segment: split the paragraph at
        // next_offset (creating a new paragraph), then insert the
        // segment at the head of the new paragraph.
        for seg in &segments[1..] {
            split_paragraph_at(story, next_offset);
            // The split inserts a paragraph break at next_offset;
            // the new paragraph starts at next_offset + 1 (per the
            // story-offset contract: synthetic \n between paragraphs
            // consumes 1 byte of story-local offset).
            next_offset += 1;
            insert_one_segment(story, next_offset, seg);
            next_offset += seg.len() as u32;
        }
    }
    merge_adjacent_runs_in_target(story, offset);
    Ok(AppliedText {
        story_id: story_id.into(),
        inverse: TextOp::DeleteRange {
            story_id: story_id.into(),
            start: offset,
            // Use byte length (which includes synthetic \n bytes for
            // any inter-paragraph boundaries inserted) so the inverse
            // covers the exact stretch we just inserted.
            end: offset + text.len() as u32,
            recovered: String::new(),
        },
    })
}

/// Insert plain (no-newline) text at `offset`. Internal helper —
/// caller guarantees no `\n` in `seg`.
fn insert_one_segment(story: &mut idml_parse::Story, offset: u32, seg: &str) {
    if seg.is_empty() {
        return;
    }
    let target = locate(story, offset);
    match target {
        Locate::InRun {
            paragraph_idx,
            run_idx,
            byte_in_run,
        } => {
            let para = &mut story.paragraphs[paragraph_idx];
            let run = &mut para.runs[run_idx];
            run.text.insert_str(byte_in_run, seg);
        }
        Locate::EndOfStory { paragraph_idx } => {
            let para = &mut story.paragraphs[paragraph_idx];
            if let Some(run) = para.runs.last_mut() {
                run.text.push_str(seg);
            } else {
                let mut run = CharacterRun::default();
                run.text = seg.into();
                para.runs.push(run);
            }
        }
        Locate::AtParagraphBreak {
            after_paragraph_idx,
        } => {
            let next_idx = after_paragraph_idx + 1;
            let next_para = &mut story.paragraphs[next_idx];
            if let Some(run) = next_para.runs.first_mut() {
                run.text.insert_str(0, seg);
            } else {
                let mut run = CharacterRun::default();
                run.text = seg.into();
                next_para.runs.push(run);
            }
        }
    }
}

/// Split a paragraph at `offset`. The bytes at/after `offset` move
/// into a new paragraph inserted immediately after the original.
/// Inherits the original paragraph's style attributes (the only
/// thing IDML's paragraph carries that's level-affecting — runs keep
/// their own character styles intact).
fn split_paragraph_at(story: &mut idml_parse::Story, offset: u32) {
    let target = locate(story, offset);
    let (paragraph_idx, run_idx, byte_in_run) = match target {
        Locate::InRun {
            paragraph_idx,
            run_idx,
            byte_in_run,
        } => (paragraph_idx, run_idx, byte_in_run),
        Locate::EndOfStory { paragraph_idx } => {
            // Split at the end of the paragraph — the new paragraph
            // is empty.
            let para = &story.paragraphs[paragraph_idx];
            let new_para = idml_parse::Paragraph {
                paragraph_style: para.paragraph_style.clone(),
                ..Default::default()
            };
            story.paragraphs.insert(paragraph_idx + 1, new_para);
            return;
        }
        Locate::AtParagraphBreak { .. } => {
            // Already on a paragraph break — splitting here is a no-op.
            return;
        }
    };
    let para = &mut story.paragraphs[paragraph_idx];
    // Tail runs that come AFTER the split point move to the new
    // paragraph. The split-point run itself is split into two halves;
    // the right half becomes the first run of the new paragraph.
    let mut tail_runs: Vec<CharacterRun> = para.runs.split_off(run_idx + 1);
    let split_run = &mut para.runs[run_idx];
    if byte_in_run < split_run.text.len() {
        let right_text: String = split_run.text.split_off(byte_in_run);
        let mut right_run = split_run.clone();
        right_run.text = right_text;
        tail_runs.insert(0, right_run);
    }
    // If the split-point run is now empty, drop it.
    if para.runs.last().map(|r| r.text.is_empty()).unwrap_or(false) {
        para.runs.pop();
    }
    let style = para.paragraph_style.clone();
    let new_para = idml_parse::Paragraph {
        paragraph_style: style,
        runs: tail_runs,
        ..Default::default()
    };
    story.paragraphs.insert(paragraph_idx + 1, new_para);
}

fn apply_delete_range(
    doc: &mut Document,
    story_id: &str,
    start: u32,
    end: u32,
) -> Result<AppliedText, TextOpError> {
    if end < start {
        return Err(TextOpError::InvalidRange { start, end });
    }
    if end == start {
        return Ok(AppliedText {
            story_id: story_id.into(),
            inverse: TextOp::InsertText {
                story_id: story_id.into(),
                offset: start,
                text: String::new(),
            },
        });
    }
    let story = find_story_mut(doc, story_id)?;
    let len = story_byte_len(&*story);
    if end > len {
        return Err(TextOpError::OffsetOutOfRange {
            story_id: story_id.into(),
            offset: end,
            len,
        });
    }

    // Phase 3 Gap-D — full cross-paragraph delete support.
    //
    // Strategy:
    //   1. Find (start_para, start_local) and (end_para, end_local)
    //      via locate(); EndOfStory and AtParagraphBreak collapse to
    //      same logical positions.
    //   2. If same paragraph: splice within it (the existing fast
    //      path).
    //   3. Otherwise:
    //      a. Capture the deleted text into `recovered` by walking
    //         start_para's tail + each whole middle paragraph
    //         (joined with synthetic `\n` per the story-offset
    //         contract) + end_para's head.
    //      b. Splice start_para's runs to keep only bytes 0..start_local.
    //      c. Append end_para's tail-runs (runs from end_local onward)
    //         to start_para.
    //      d. Drop paragraphs (start_para+1..=end_para).
    let (start_para, start_local) = locate_para_local(&*story, start);
    let (end_para, end_local) = locate_para_local(&*story, end);

    let mut recovered = String::with_capacity((end - start) as usize);
    if start_para == end_para {
        // Same-paragraph fast path.
        let para = &mut story.paragraphs[start_para];
        splice_paragraph(para, start_local, end_local, &mut recovered);
    } else {
        // Capture tail of start_para.
        {
            let para = &story.paragraphs[start_para];
            for run in &para.runs {
                let already: usize = run_text_total_before(para, run);
                let run_end_in_para = already + run.text.len();
                if start_local < run_end_in_para && already < usize::MAX {
                    let lo = start_local.max(already) - already;
                    let hi = run.text.len();
                    if hi > lo {
                        recovered.push_str(&run.text[lo..hi]);
                    }
                }
            }
            // Synthetic \n joining start_para to whatever's next.
            recovered.push('\n');
        }
        // Capture whole middle paragraphs + the head of end_para.
        for p in (start_para + 1)..end_para {
            let para = &story.paragraphs[p];
            for run in &para.runs {
                recovered.push_str(&run.text);
            }
            recovered.push('\n');
        }
        {
            let para = &story.paragraphs[end_para];
            let mut acc: usize = 0;
            for run in &para.runs {
                let rlen = run.text.len();
                let lo = 0;
                let hi = end_local.min(acc + rlen).saturating_sub(acc);
                if hi > lo {
                    recovered.push_str(&run.text[lo..hi]);
                }
                acc += rlen;
                if acc >= end_local {
                    break;
                }
            }
        }

        // Now mutate: trim start_para to its [0..start_local] runs,
        // then append end_para's tail.
        let end_tail_runs = capture_tail_runs(&story.paragraphs[end_para], end_local);
        let start_para_ref = &mut story.paragraphs[start_para];
        truncate_paragraph_to(start_para_ref, start_local);
        start_para_ref.runs.extend(end_tail_runs);
        // Drop paragraphs (start_para+1 ..= end_para).
        story.paragraphs.drain((start_para + 1)..=end_para);
    }

    merge_adjacent_runs_in_target(story, start);
    Ok(AppliedText {
        story_id: story_id.into(),
        inverse: TextOp::InsertText {
            story_id: story_id.into(),
            offset: start,
            text: recovered,
        },
    })
}

/// Convert a story-local offset to (paragraph_idx, byte-within-paragraph).
/// `AtParagraphBreak` resolves to the START of the next paragraph
/// (which matches the offset's logical position past the break).
fn locate_para_local(story: &idml_parse::Story, offset: u32) -> (usize, usize) {
    match locate(story, offset) {
        Locate::InRun {
            paragraph_idx,
            run_idx,
            byte_in_run,
        } => {
            let para = &story.paragraphs[paragraph_idx];
            let head: usize = para.runs[..run_idx]
                .iter()
                .map(|r| r.text.len())
                .sum();
            (paragraph_idx, head + byte_in_run)
        }
        Locate::EndOfStory { paragraph_idx } => {
            let para = &story.paragraphs[paragraph_idx];
            let total: usize = para.runs.iter().map(|r| r.text.len()).sum();
            (paragraph_idx, total)
        }
        Locate::AtParagraphBreak {
            after_paragraph_idx,
        } => (after_paragraph_idx + 1, 0),
    }
}

/// Sum of run text bytes that precede `target_run` in `para`'s run
/// list. Returns `usize::MAX` if `target_run` isn't in the list
/// (defensive — shouldn't happen since the caller iterates `para.runs`).
fn run_text_total_before(para: &idml_parse::Paragraph, target_run: &CharacterRun) -> usize {
    let mut total: usize = 0;
    for r in &para.runs {
        if std::ptr::eq(r, target_run) {
            return total;
        }
        total += r.text.len();
    }
    usize::MAX
}

/// Take the tail of `para`'s runs starting at byte `from`. The
/// returned vec is freshly allocated; the source paragraph is read
/// only.
fn capture_tail_runs(para: &idml_parse::Paragraph, from: usize) -> Vec<CharacterRun> {
    let mut out: Vec<CharacterRun> = Vec::new();
    let mut acc: usize = 0;
    for run in &para.runs {
        let rlen = run.text.len();
        if acc >= from {
            out.push(run.clone());
        } else if acc + rlen > from {
            let lo = from - acc;
            let mut tail = run.clone();
            tail.text = run.text[lo..].to_string();
            out.push(tail);
        }
        acc += rlen;
    }
    out
}

/// Truncate `para`'s runs to keep only bytes `[0..keep)`.
fn truncate_paragraph_to(para: &mut idml_parse::Paragraph, keep: usize) {
    let mut acc: usize = 0;
    let mut split_at: Option<(usize, usize)> = None; // (run_idx, local)
    for (i, run) in para.runs.iter().enumerate() {
        let rlen = run.text.len();
        if acc + rlen >= keep {
            split_at = Some((i, keep - acc));
            break;
        }
        acc += rlen;
    }
    let Some((i, local)) = split_at else {
        return;
    };
    para.runs.truncate(i + 1);
    let run = &mut para.runs[i];
    run.text.truncate(local);
    if run.text.is_empty() {
        para.runs.pop();
    }
}

// ---- helpers ---------------------------------------------------------

#[derive(Debug, Clone)]
enum Locate {
    InRun {
        paragraph_idx: usize,
        run_idx: usize,
        byte_in_run: usize,
    },
    EndOfStory {
        paragraph_idx: usize,
    },
    AtParagraphBreak {
        after_paragraph_idx: usize,
    },
}

fn find_story_mut<'a>(
    doc: &'a mut Document,
    story_id: &str,
) -> Result<&'a mut idml_parse::Story, TextOpError> {
    doc.stories
        .iter_mut()
        .find(|s| s.self_id == story_id)
        .map(|s| &mut s.story)
        .ok_or_else(|| TextOpError::UnknownStory(story_id.into()))
}

/// Total byte length per the story-offset contract: sum of run bytes
/// + one synthetic `\n` per inter-paragraph boundary.
fn story_byte_len(story: &idml_parse::Story) -> u32 {
    let mut total: u32 = 0;
    for (i, p) in story.paragraphs.iter().enumerate() {
        if i > 0 {
            total += 1;
        }
        for r in &p.runs {
            total += r.text.len() as u32;
        }
    }
    total
}

fn locate(story: &idml_parse::Story, story_offset: u32) -> Locate {
    let mut consumed: u32 = 0;
    for (pi, p) in story.paragraphs.iter().enumerate() {
        let para_byte_len: u32 = p.runs.iter().map(|r| r.text.len() as u32).sum();
        // Is the offset in this paragraph's runs?
        if story_offset <= consumed + para_byte_len {
            let local = (story_offset - consumed) as usize;
            // Walk runs.
            let mut acc: usize = 0;
            for (ri, r) in p.runs.iter().enumerate() {
                let rlen = r.text.len();
                if local <= acc + rlen {
                    return Locate::InRun {
                        paragraph_idx: pi,
                        run_idx: ri,
                        byte_in_run: local - acc,
                    };
                }
                acc += rlen;
            }
            // Past last run of this paragraph but ≤ paragraph length
            // — special case: empty paragraph with no runs.
            return Locate::EndOfStory { paragraph_idx: pi };
        }
        consumed += para_byte_len;
        // Synthetic \n between paragraphs. If the offset falls
        // *exactly* on the synthetic byte, it's the paragraph break.
        if story_offset == consumed && pi + 1 < story.paragraphs.len() {
            return Locate::AtParagraphBreak {
                after_paragraph_idx: pi,
            };
        }
        consumed += 1;
    }
    // Offset past last paragraph: pin to last paragraph.
    Locate::EndOfStory {
        paragraph_idx: story.paragraphs.len().saturating_sub(1),
    }
}

fn splice_paragraph(
    para: &mut idml_parse::Paragraph,
    local_start: usize,
    local_end: usize,
    recovered: &mut String,
) {
    // Walk runs, splice the bytes in [local_start, local_end).
    let mut acc: usize = 0;
    for run in &mut para.runs {
        let rlen = run.text.len();
        let run_start_in_para = acc;
        let run_end_in_para = acc + rlen;
        let lo = local_start.max(run_start_in_para);
        let hi = local_end.min(run_end_in_para);
        if hi > lo {
            let lo_in_run = lo - run_start_in_para;
            let hi_in_run = hi - run_start_in_para;
            recovered.push_str(&run.text[lo_in_run..hi_in_run]);
            run.text.replace_range(lo_in_run..hi_in_run, "");
        }
        acc = run_end_in_para;
    }
    // Drop empty runs left after the splice.
    para.runs.retain(|r| !r.text.is_empty());
}

/// Run-merge: walk the paragraph that contains `story_offset` and
/// coalesce adjacent runs whose **every** style field is identical.
/// Without this, repeated edits fragment a paragraph into many
/// effectively-identical runs and break determinism hashing.
fn merge_adjacent_runs_in_target(story: &mut idml_parse::Story, story_offset: u32) {
    // Find the target paragraph.
    let mut consumed: u32 = 0;
    let mut target: Option<usize> = None;
    for (pi, p) in story.paragraphs.iter().enumerate() {
        let para_byte_len: u32 = p.runs.iter().map(|r| r.text.len() as u32).sum();
        if story_offset <= consumed + para_byte_len {
            target = Some(pi);
            break;
        }
        consumed += para_byte_len + 1;
    }
    let pi = target.unwrap_or(0);
    if pi >= story.paragraphs.len() {
        return;
    }
    let para = &mut story.paragraphs[pi];
    if para.runs.len() < 2 {
        return;
    }
    let mut i = 0;
    while i + 1 < para.runs.len() {
        if runs_mergeable(&para.runs[i], &para.runs[i + 1]) {
            let next_text = std::mem::take(&mut para.runs[i + 1].text);
            para.runs[i].text.push_str(&next_text);
            para.runs.remove(i + 1);
            // Don't advance i — the new merged run might be
            // mergeable with the next one too.
        } else {
            i += 1;
        }
    }
}

fn runs_mergeable(a: &CharacterRun, b: &CharacterRun) -> bool {
    // CharacterRun doesn't implement PartialEq (it carries Vec /
    // Option fields whose downstream consumers prefer structural
    // walks). Field-by-field comparison via serde round-trip is
    // both stable and "byte-identical except for text". serde_json
    // sorts struct fields by source order which matches the parser,
    // so equal serialisations imply equal merge-relevant state.
    let mut a_clone = a.clone();
    let mut b_clone = b.clone();
    a_clone.text.clear();
    b_clone.text.clear();
    let a_json = serde_json::to_vec(&a_clone).unwrap_or_default();
    let b_json = serde_json::to_vec(&b_clone).unwrap_or_default();
    a_json == b_json
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CanvasModel, CanvasOptions};

    fn small_idml() -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/vnd.adobe.indesign-idml-package")
                .unwrap();
            zip.start_file("META-INF/container.xml", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="designmap.xml" media-type="text/xml"/></rootfiles></container>"#).unwrap();
            zip.start_file("designmap.xml", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="13.1" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
<idPkg:Story src="Stories/Story_story1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#).unwrap();
            zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<TextFrame Self="tf1" ParentStory="story1" GeometricBounds="100 100 400 400" ItemTransform="1 0 0 1 0 0"/>
</Spread></idPkg:Spread>"#).unwrap();
            zip.start_file("Stories/Story_story1.xml", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="13.1">
<Story Self="story1">
<ParagraphStyleRange>
<CharacterStyleRange><Content>Hello world</Content></CharacterStyleRange>
</ParagraphStyleRange>
</Story></idPkg:Story>"#).unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn insert_text_in_run_middle_shifts_text() {
        let mut model = CanvasModel::load("d", &small_idml(), CanvasOptions::default()).unwrap();
        let scene_mut = model.scene_mut();
        let applied = apply(
            scene_mut,
            &TextOp::InsertText {
                story_id: "story1".into(),
                offset: 5,
                text: ",".into(),
            },
        )
        .unwrap();
        assert_eq!(applied.story_id, "story1");
        // "Hello world" + "," at offset 5 → "Hello, world"
        let runs = &model.scene().stories[0].story.paragraphs[0].runs;
        assert_eq!(runs[0].text, "Hello, world");
        // Inverse is a DeleteRange that recovers ","
        match applied.inverse {
            TextOp::DeleteRange { start, end, .. } => {
                assert_eq!(start, 5);
                assert_eq!(end, 6);
            }
            _ => panic!("inverse must be DeleteRange"),
        }
    }

    #[test]
    fn delete_range_within_run_recovers_text() {
        let mut model = CanvasModel::load("d", &small_idml(), CanvasOptions::default()).unwrap();
        let scene_mut = model.scene_mut();
        let applied = apply(
            scene_mut,
            &TextOp::DeleteRange {
                story_id: "story1".into(),
                start: 5,
                end: 11,
                recovered: String::new(),
            },
        )
        .unwrap();
        let runs = &model.scene().stories[0].story.paragraphs[0].runs;
        assert_eq!(runs[0].text, "Hello");
        match applied.inverse {
            TextOp::InsertText { text, offset, .. } => {
                assert_eq!(offset, 5);
                assert_eq!(text, " world");
            }
            _ => panic!("inverse must be InsertText"),
        }
    }

    #[test]
    fn insert_then_undo_via_inverse_restores_original_hash() {
        let mut model = CanvasModel::load("d", &small_idml(), CanvasOptions::default()).unwrap();
        let initial = model.initial_state_hash();
        let scene_mut = model.scene_mut();
        let applied = apply(
            scene_mut,
            &TextOp::InsertText {
                story_id: "story1".into(),
                offset: 5,
                text: ",".into(),
            },
        )
        .unwrap();
        assert_ne!(initial, model.current_state_hash(), "mutation must change state");
        // Apply inverse (delete the ","), expect back to original.
        let scene_mut = model.scene_mut();
        apply(scene_mut, &applied.inverse).unwrap();
        assert_eq!(initial, model.current_state_hash(), "inverse must restore state");
    }

    #[test]
    fn insert_newline_splits_paragraph() {
        let mut model = CanvasModel::load("d", &small_idml(), CanvasOptions::default()).unwrap();
        // Insert "\n" at offset 5 of "Hello world" → splits into
        // "Hello" / " world".
        apply(
            model.scene_mut(),
            &TextOp::InsertText {
                story_id: "story1".into(),
                offset: 5,
                text: "\n".into(),
            },
        )
        .unwrap();
        let paragraphs = &model.scene().stories[0].story.paragraphs;
        assert_eq!(paragraphs.len(), 2, "should have split into two paragraphs");
        let head: String = paragraphs[0].runs.iter().map(|r| r.text.clone()).collect();
        let tail: String = paragraphs[1].runs.iter().map(|r| r.text.clone()).collect();
        assert_eq!(head, "Hello");
        assert_eq!(tail, " world");
    }

    #[test]
    fn insert_with_internal_newline_splits_and_inserts() {
        let mut model = CanvasModel::load("d", &small_idml(), CanvasOptions::default()).unwrap();
        // "X\nY" at offset 5: head paragraph gets "X" appended ("HelloX"),
        // a paragraph break is inserted, "Y" prefixes the new paragraph
        // which then contains the original tail " world".
        apply(
            model.scene_mut(),
            &TextOp::InsertText {
                story_id: "story1".into(),
                offset: 5,
                text: "X\nY".into(),
            },
        )
        .unwrap();
        let ps = &model.scene().stories[0].story.paragraphs;
        assert_eq!(ps.len(), 2);
        let head: String = ps[0].runs.iter().map(|r| r.text.clone()).collect();
        let tail: String = ps[1].runs.iter().map(|r| r.text.clone()).collect();
        assert_eq!(head, "HelloX");
        assert_eq!(tail, "Y world");
    }

    #[test]
    fn delete_across_paragraph_boundary_merges_paragraphs() {
        let mut model = CanvasModel::load("d", &small_idml(), CanvasOptions::default()).unwrap();
        // First split at offset 5 so we have "Hello" / " world".
        apply(
            model.scene_mut(),
            &TextOp::InsertText {
                story_id: "story1".into(),
                offset: 5,
                text: "\n".into(),
            },
        )
        .unwrap();
        // Story is now "Hello" + \n (5) + " world" (6) — total 12.
        // Delete [3, 8) — covers "lo" (from para 0) + "\n" + " w" (from para 1).
        let applied = apply(
            model.scene_mut(),
            &TextOp::DeleteRange {
                story_id: "story1".into(),
                start: 3,
                end: 8,
                recovered: String::new(),
            },
        )
        .unwrap();
        let ps = &model.scene().stories[0].story.paragraphs;
        assert_eq!(ps.len(), 1, "cross-paragraph delete must merge");
        let merged: String = ps[0].runs.iter().map(|r| r.text.clone()).collect();
        assert_eq!(merged, "Helorld");
        // Inverse should let us recover the original split structure.
        match applied.inverse {
            TextOp::InsertText { text, offset, .. } => {
                assert_eq!(offset, 3);
                assert_eq!(text, "lo\n w");
            }
            other => panic!("unexpected inverse: {other:?}"),
        }
    }
}
