//! Rope (paragraph-major) for editable stories.
//!
//! M2's text editing model. Per-story tree:
//!   `StoryRope` → `Vec<ParagraphRope>`
//!   `ParagraphRope` → paragraph-level attrs + `Vec<RunSeg>` + optional table
//!   `RunSeg` → run-level attrs + text (owned `String`)
//!
//! The rope is the source of truth for text content and run/paragraph
//! attributes during editing. The original `idml_parse::Story` keeps
//! the Tables and other rare structures we don't yet edit (they ride
//! along untouched). After every text command, `Project` rebuilds the
//! affected `idml_parse::Story` from the rope so the existing render
//! pipeline (which still reads `&Document`) sees the updated text.
//! M3+ will replace this synchroniser with an overlay-aware view.
//!
//! Byte addressing: every offset is the **byte offset within a
//! paragraph's concatenated text** (paragraph text = sum of its
//! runs' `text` fields, in run order). This keeps the caret model
//! agnostic to where run boundaries fall.
//!
//! M2 deliberately keeps this simple — a true gap buffer / red-black
//! rope arrives with M3 if we measure typing-latency lag on long
//! paragraphs. For the M2 target (single-frame editing) the
//! `String`-per-run shape is fast enough.

use idml_parse::{
    CharacterRun, Paragraph as ParsedParagraph, Story as ParsedStory, TabStop, Table,
};

/// Run-level attributes (everything on `CharacterRun` except `text`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RunAttrs {
    pub character_style: Option<String>,
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    pub fill_tint: Option<f32>,
    pub capitalization: Option<String>,
    pub baseline_shift: Option<f32>,
    pub horizontal_scale: Option<f32>,
    pub vertical_scale: Option<f32>,
    pub position: Option<String>,
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    pub leading: Option<f32>,
}

impl RunAttrs {
    fn from_run(r: &CharacterRun) -> Self {
        Self {
            character_style: r.character_style.clone(),
            font: r.font.clone(),
            font_style: r.font_style.clone(),
            point_size: r.point_size,
            fill_color: r.fill_color.clone(),
            fill_tint: r.fill_tint,
            capitalization: r.capitalization.clone(),
            baseline_shift: r.baseline_shift,
            horizontal_scale: r.horizontal_scale,
            vertical_scale: r.vertical_scale,
            position: r.position.clone(),
            tracking: r.tracking,
            underline: r.underline,
            strikethru: r.strikethru,
            leading: r.leading,
        }
    }

    fn into_run(self, text: String) -> CharacterRun {
        CharacterRun {
            character_style: self.character_style,
            font: self.font,
            font_style: self.font_style,
            point_size: self.point_size,
            fill_color: self.fill_color,
            fill_tint: self.fill_tint,
            capitalization: self.capitalization,
            baseline_shift: self.baseline_shift,
            horizontal_scale: self.horizontal_scale,
            vertical_scale: self.vertical_scale,
            position: self.position,
            tracking: self.tracking,
            underline: self.underline,
            strikethru: self.strikethru,
            leading: self.leading,
            text,
        }
    }
}

/// Paragraph-level attributes (everything on `Paragraph` except
/// `runs` and `table`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParagraphAttrs {
    pub paragraph_style: Option<String>,
    pub justification: Option<String>,
    pub first_line_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub tab_list: Vec<TabStop>,
}

impl ParagraphAttrs {
    fn from_paragraph(p: &ParsedParagraph) -> Self {
        Self {
            paragraph_style: p.paragraph_style.clone(),
            justification: p.justification.clone(),
            first_line_indent: p.first_line_indent,
            space_before: p.space_before,
            space_after: p.space_after,
            tab_list: p.tab_list.clone(),
        }
    }
}

/// A single run inside a paragraph.
#[derive(Debug, Clone)]
pub struct RunSeg {
    pub attrs: RunAttrs,
    pub text: String,
}

impl RunSeg {
    pub fn new(attrs: RunAttrs, text: String) -> Self {
        Self { attrs, text }
    }
}

#[derive(Debug, Clone)]
pub struct ParagraphRope {
    pub attrs: ParagraphAttrs,
    pub runs: Vec<RunSeg>,
    /// Tables ride through unedited. M2 doesn't expose table editing;
    /// the field is here so paragraph rebuild preserves them.
    pub table: Option<Table>,
}

impl ParagraphRope {
    /// Total byte length across all runs.
    pub fn len_bytes(&self) -> usize {
        self.runs.iter().map(|r| r.text.len()).sum()
    }

    /// Concatenated text — useful for tests, diff harnesses, and
    /// the find/replace command path.
    pub fn text(&self) -> String {
        let mut s = String::with_capacity(self.len_bytes());
        for r in &self.runs {
            s.push_str(&r.text);
        }
        s
    }

    /// Locate `byte_offset` within a paragraph: returns `(run_idx,
    /// offset_in_run)`. `byte_offset == len_bytes()` lands at the end
    /// of the last run.
    pub fn locate_byte(&self, byte_offset: usize) -> (usize, usize) {
        let mut acc = 0usize;
        for (i, r) in self.runs.iter().enumerate() {
            let next = acc + r.text.len();
            if byte_offset <= next {
                return (i, byte_offset - acc);
            }
            acc = next;
        }
        // Past end → clamp to (last_run, last_run.len()).
        let last_idx = self.runs.len().saturating_sub(1);
        let last_len = self.runs.last().map(|r| r.text.len()).unwrap_or(0);
        (last_idx, last_len)
    }

    /// Insert `s` at byte offset `at`. Splits the hosting run if `at`
    /// is mid-run. Returns the inserted byte length (`s.len()`).
    pub fn insert_str(&mut self, at: usize, s: &str) -> usize {
        if s.is_empty() {
            return 0;
        }
        if self.runs.is_empty() {
            self.runs
                .push(RunSeg::new(RunAttrs::default(), s.to_string()));
            return s.len();
        }
        let (run_idx, off) = self.locate_byte(at);
        // Guard: insertion at the very end of the paragraph appends
        // to the last run rather than creating an empty new run.
        let r = &mut self.runs[run_idx];
        if !r.text.is_char_boundary(off) {
            // Snap to nearest preceding boundary — shouldn't happen
            // with a well-behaved caret, but cheap insurance.
            let mut snap = off;
            while snap > 0 && !r.text.is_char_boundary(snap) {
                snap -= 1;
            }
            r.text.insert_str(snap, s);
        } else {
            r.text.insert_str(off, s);
        }
        s.len()
    }

    /// Delete the byte range `[from, to)`. Returns the deleted bytes
    /// in their original (concatenated) order. Spans run boundaries
    /// — runs entirely inside the range collapse to empty and are
    /// removed (a paragraph keeps at least one empty run for caret
    /// arithmetic).
    pub fn delete_range(&mut self, from: usize, to: usize) -> String {
        if to <= from || self.runs.is_empty() {
            return String::new();
        }
        let total = self.len_bytes();
        let to = to.min(total);
        let from = from.min(to);
        let mut out = String::with_capacity(to - from);

        // Track positions in the *original* concat so the cut bounds
        // never shift mid-loop. Iterate by index; if a run gets
        // removed for being empty, decrement `i` so the next slot is
        // re-examined.
        let mut acc_orig = 0usize;
        let mut i = 0usize;
        while i < self.runs.len() {
            let orig_len = self.runs[i].text.len();
            let run_end_orig = acc_orig + orig_len;
            if run_end_orig <= from {
                acc_orig = run_end_orig;
                i += 1;
                continue;
            }
            if acc_orig >= to {
                break;
            }
            let local_from = from.saturating_sub(acc_orig);
            let local_to = (to - acc_orig).min(orig_len);
            let local_from = snap_left(&self.runs[i].text, local_from);
            let local_to = snap_right(&self.runs[i].text, local_to);
            let removed: String = self.runs[i].text.drain(local_from..local_to).collect();
            out.push_str(&removed);
            acc_orig = run_end_orig;
            if self.runs[i].text.is_empty() && self.runs.len() > 1 {
                self.runs.remove(i);
                // Don't advance i — next run shifted into this slot.
            } else {
                i += 1;
            }
        }
        out
    }
}

fn snap_left(s: &str, mut i: usize) -> usize {
    if i > s.len() {
        i = s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn snap_right(s: &str, mut i: usize) -> usize {
    if i > s.len() {
        i = s.len();
    }
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Per-story rope. Owns the editable representation; the original
/// `ParsedStory` stays around so we can copy through tables and any
/// other future passthrough fields on rebuild.
#[derive(Debug, Clone)]
pub struct StoryRope {
    pub paragraphs: Vec<ParagraphRope>,
}

impl StoryRope {
    pub fn from_story(story: &ParsedStory) -> Self {
        Self {
            paragraphs: story
                .paragraphs
                .iter()
                .map(|p| ParagraphRope {
                    attrs: ParagraphAttrs::from_paragraph(p),
                    runs: p
                        .runs
                        .iter()
                        .map(|r| RunSeg::new(RunAttrs::from_run(r), r.text.clone()))
                        .collect(),
                    table: p.table.clone(),
                })
                .collect(),
        }
    }

    /// Materialize back into a `ParsedStory` shape so the existing
    /// pipeline can read it. Empty runs are preserved if a paragraph
    /// would otherwise have zero — IDML doesn't allow runless
    /// paragraphs.
    pub fn to_story(&self) -> ParsedStory {
        let paragraphs = self
            .paragraphs
            .iter()
            .map(|p| {
                let runs: Vec<CharacterRun> = if p.runs.is_empty() {
                    vec![RunAttrs::default().into_run(String::new())]
                } else {
                    p.runs
                        .iter()
                        .map(|r| r.attrs.clone().into_run(r.text.clone()))
                        .collect()
                };
                ParsedParagraph {
                    paragraph_style: p.attrs.paragraph_style.clone(),
                    justification: p.attrs.justification.clone(),
                    first_line_indent: p.attrs.first_line_indent,
                    space_before: p.attrs.space_before,
                    space_after: p.attrs.space_after,
                    tab_list: p.attrs.tab_list.clone(),
                    bullets_list_type: None,
                    bullet_character: None,
                    runs,
                    table: p.table.clone(),
                }
            })
            .collect();
        ParsedStory { paragraphs }
    }

    pub fn paragraph(&self, idx: usize) -> Option<&ParagraphRope> {
        self.paragraphs.get(idx)
    }

    pub fn paragraph_mut(&mut self, idx: usize) -> Option<&mut ParagraphRope> {
        self.paragraphs.get_mut(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> ParagraphRope {
        ParagraphRope {
            attrs: ParagraphAttrs::default(),
            runs: vec![RunSeg::new(RunAttrs::default(), text.to_string())],
            table: None,
        }
    }

    #[test]
    fn locate_byte_handles_multi_run() {
        let mut para = p("");
        para.runs = vec![
            RunSeg::new(RunAttrs::default(), "ab".to_string()),
            RunSeg::new(RunAttrs::default(), "cdef".to_string()),
        ];
        assert_eq!(para.locate_byte(0), (0, 0));
        assert_eq!(para.locate_byte(2), (0, 2));
        assert_eq!(para.locate_byte(3), (1, 1));
        assert_eq!(para.locate_byte(6), (1, 4));
    }

    #[test]
    fn insert_at_run_boundary_extends_left_run() {
        let mut para = p("");
        para.runs = vec![
            RunSeg::new(RunAttrs::default(), "ab".to_string()),
            RunSeg::new(RunAttrs::default(), "cd".to_string()),
        ];
        para.insert_str(2, "X");
        assert_eq!(para.text(), "abXcd");
    }

    #[test]
    fn insert_at_end_appends_to_last_run() {
        let mut para = p("hello");
        para.insert_str(5, "!");
        assert_eq!(para.text(), "hello!");
        assert_eq!(para.runs.len(), 1);
    }

    #[test]
    fn delete_within_single_run() {
        let mut para = p("abcdef");
        let removed = para.delete_range(1, 4);
        assert_eq!(removed, "bcd");
        assert_eq!(para.text(), "aef");
    }

    #[test]
    fn delete_across_run_boundary() {
        let mut para = p("");
        para.runs = vec![
            RunSeg::new(RunAttrs::default(), "abc".to_string()),
            RunSeg::new(RunAttrs::default(), "def".to_string()),
        ];
        let removed = para.delete_range(2, 5);
        assert_eq!(removed, "cde");
        assert_eq!(para.text(), "abf");
    }

    #[test]
    fn delete_drops_fully_consumed_runs_but_keeps_one() {
        let mut para = p("");
        para.runs = vec![
            RunSeg::new(RunAttrs::default(), "ab".to_string()),
            RunSeg::new(RunAttrs::default(), "cd".to_string()),
            RunSeg::new(RunAttrs::default(), "ef".to_string()),
        ];
        let removed = para.delete_range(0, 6);
        assert_eq!(removed, "abcdef");
        assert_eq!(para.text(), "");
        assert_eq!(para.runs.len(), 1, "paragraph keeps one empty run");
    }

    #[test]
    fn round_trip_through_story() {
        let original = ParsedStory {
            paragraphs: vec![ParsedParagraph {
                paragraph_style: Some("ParagraphStyle/Body".into()),
                justification: Some("LeftAlign".into()),
                first_line_indent: None,
                space_before: None,
                space_after: None,
                tab_list: vec![],
                runs: vec![
                    CharacterRun {
                        character_style: None,
                        font: Some("Helvetica".into()),
                        font_style: None,
                        point_size: Some(12.0),
                        fill_color: None,
                        fill_tint: None,
                        capitalization: None,
                        baseline_shift: None,
                        horizontal_scale: None,
                        vertical_scale: None,
                        position: None,
                        tracking: None,
                        underline: None,
                        strikethru: None,
                        leading: None,
                        text: "Hello, ".into(),
                    },
                    CharacterRun {
                        character_style: None,
                        font: Some("Helvetica".into()),
                        font_style: Some("Bold".into()),
                        point_size: Some(12.0),
                        fill_color: None,
                        fill_tint: None,
                        capitalization: None,
                        baseline_shift: None,
                        horizontal_scale: None,
                        vertical_scale: None,
                        position: None,
                        tracking: None,
                        underline: None,
                        strikethru: None,
                        leading: None,
                        text: "world".into(),
                    },
                ],
                table: None,
            }],
        };
        let rope = StoryRope::from_story(&original);
        assert_eq!(rope.paragraph(0).unwrap().text(), "Hello, world");
        let back = rope.to_story();
        assert_eq!(back.paragraphs.len(), 1);
        assert_eq!(back.paragraphs[0].runs.len(), 2);
        assert_eq!(
            back.paragraphs[0].runs[1].font_style.as_deref(),
            Some("Bold")
        );
        assert_eq!(back.paragraphs[0].runs[0].text, "Hello, ");
    }
}
