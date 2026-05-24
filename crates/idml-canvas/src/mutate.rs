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
#[serde(rename_all = "camelCase", tag = "kind")]
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
    #[error("delete range crosses a paragraph boundary; not supported in Phase 3 v1")]
    CrossParagraphDelete,
    #[error("insert of `\\n` (paragraph split) not supported in Phase 3 v1")]
    ParagraphSplittingInsert,
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
    if text.contains('\n') {
        return Err(TextOpError::ParagraphSplittingInsert);
    }
    let story = find_story_mut(doc, story_id)?;
    let len = story_byte_len(&*story);
    if offset > len {
        return Err(TextOpError::OffsetOutOfRange {
            story_id: story_id.into(),
            offset,
            len,
        });
    }
    // Locate target paragraph + run + intra-run byte position.
    let target = locate(&*story, offset);
    match target {
        Locate::InRun {
            paragraph_idx,
            run_idx,
            byte_in_run,
        } => {
            let para = &mut story.paragraphs[paragraph_idx];
            let run = &mut para.runs[run_idx];
            run.text.insert_str(byte_in_run, text);
        }
        Locate::EndOfStory { paragraph_idx } => {
            let para = &mut story.paragraphs[paragraph_idx];
            // Append to last run if one exists; otherwise create one.
            if let Some(run) = para.runs.last_mut() {
                run.text.push_str(text);
            } else {
                let mut run = CharacterRun::default();
                run.text = text.into();
                para.runs.push(run);
            }
        }
        Locate::AtParagraphBreak {
            after_paragraph_idx,
        } => {
            // Insert at the synthetic \n between paragraph N and N+1.
            // The story-offset contract puts the insertion at the START
            // of paragraph N+1's first run (matches Cocoa convention:
            // text typed at a line break appears on the next line).
            let next_idx = after_paragraph_idx + 1;
            let next_para = &mut story.paragraphs[next_idx];
            if let Some(run) = next_para.runs.first_mut() {
                run.text.insert_str(0, text);
            } else {
                let mut run = CharacterRun::default();
                run.text = text.into();
                next_para.runs.push(run);
            }
        }
    }
    merge_adjacent_runs_in_target(story, offset);
    Ok(AppliedText {
        story_id: story_id.into(),
        inverse: TextOp::DeleteRange {
            story_id: story_id.into(),
            start: offset,
            end: offset + text.chars().count() as u32,
            recovered: String::new(),
        },
    })
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
    let start_loc = locate(&*story, start);
    let end_loc = locate(&*story, end);

    // Phase 3 v1: same-paragraph deletes only.
    let para_idx = match (&start_loc, &end_loc) {
        (
            Locate::InRun {
                paragraph_idx: a, ..
            },
            Locate::InRun {
                paragraph_idx: b, ..
            },
        ) if a == b => *a,
        (
            Locate::InRun {
                paragraph_idx: a, ..
            },
            Locate::EndOfStory { paragraph_idx: b },
        ) if a == b => *a,
        _ => return Err(TextOpError::CrossParagraphDelete),
    };

    // Pre-compute the paragraph-start offset before the &mut borrow.
    let para_start_byte = paragraph_start_byte_in_story(&*story, para_idx);
    let para = &mut story.paragraphs[para_idx];
    let local_start = (start - para_start_byte) as usize;
    let local_end = (end - para_start_byte) as usize;
    let mut recovered = String::with_capacity(local_end - local_start);
    splice_paragraph(para, local_start, local_end, &mut recovered);

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

fn paragraph_start_byte_in_story(story: &idml_parse::Story, paragraph_idx: usize) -> u32 {
    let mut total: u32 = 0;
    for (i, p) in story.paragraphs.iter().enumerate() {
        if i == paragraph_idx {
            return total;
        }
        total += p.runs.iter().map(|r| r.text.len() as u32).sum::<u32>();
        total += 1; // synthetic \n
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
    fn insert_text_with_newline_returns_unsupported() {
        let mut model = CanvasModel::load("d", &small_idml(), CanvasOptions::default()).unwrap();
        let err = apply(
            model.scene_mut(),
            &TextOp::InsertText {
                story_id: "story1".into(),
                offset: 0,
                text: "\n".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, TextOpError::ParagraphSplittingInsert));
    }
}
