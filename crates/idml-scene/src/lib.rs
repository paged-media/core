//! Resolved scene graph.
//!
//! `Document` is the single object consumers get from this crate. It
//! owns the parsed IDML container, the swatch palette, and pre-parsed
//! spreads and stories. Pipelines that used to open + parse everything
//! inline now accept a `&Document` and walk its fields.
//!
//! Style cascades (paragraph → character → local overrides), link
//! resolution, and master-spread inheritance will lean on this crate
//! as it matures. For the current slice it's a thin owning wrapper
//! that removes the duplicated parsing code from `idml-renderer`.

use std::collections::HashMap;

use idml_parse::{
    Bounds, CharacterRun, Container, Graphic, Paragraph, ParseError, Spread, Story, StoryRef,
    StyleSheet, TOCStyleDef, TextFrame,
};

pub mod value;
pub use value::Value;

/// Owned, parsed representation of an IDML document.
#[derive(Debug, Clone)]
pub struct Document {
    pub container: Container,
    pub palette: Graphic,
    pub spreads: Vec<ParsedSpread>,
    pub stories: Vec<ParsedStory>,
    /// Master spreads, indexed by their `Self` id (e.g.
    /// `MasterSpread/uad`). Pages reference these via
    /// `Page::applied_master`.
    pub master_spreads: HashMap<String, ParsedMasterSpread>,
    /// `TextFrame` indexed by its `ParentStory` id — built once so the
    /// pipeline doesn't have to scan every spread for each story.
    pub frame_for_story: HashMap<String, TextFrame>,
    /// `(spread_idx, frame_idx)` for each TextFrame keyed by its
    /// `Self` id. Built at open time so [`text_frame`] is O(1) and
    /// [`frame_chain`] walks long NextTextFrame chains in linear
    /// time rather than O(K × total_frames).
    pub text_frame_index: HashMap<String, (usize, usize)>,
    /// Paragraph + character style definitions loaded from
    /// `Resources/Styles.xml`. Empty when the archive has no styles
    /// resource (rare; typically only synthetic test docs).
    pub styles: StyleSheet,
}

/// A spread plus the path it came from in the container.
#[derive(Debug, Clone)]
pub struct ParsedSpread {
    pub src: String,
    pub spread: Spread,
}

/// A story plus its `Self` id (derived from the manifest src) and
/// source path.
#[derive(Debug, Clone)]
pub struct ParsedStory {
    pub src: String,
    pub self_id: String,
    pub story: Story,
}

/// A master spread plus the `Self` id pages reference it by. The
/// XML schema is identical to a regular `<Spread>`, so we reuse
/// `Spread` for the geometry payload.
#[derive(Debug, Clone)]
pub struct ParsedMasterSpread {
    pub src: String,
    pub self_id: String,
    pub spread: Spread,
}

/// Cap on the number of frames followed via `NextTextFrame`.
/// Real chains are 1–10 frames; the cap exists so a malformed
/// document with a missed cycle can't make the resolver loop.
const MAX_FRAME_CHAIN: usize = 256;

impl Document {
    /// Parse every resource the manifest points at. Missing spreads
    /// or stories produce an [`OpenError::MissingEntry`] — the parse
    /// layer's tolerant behaviour (skipping entries without an
    /// archive match) is lifted here to a structured error.
    pub fn open(bytes: &[u8]) -> Result<Self, OpenError> {
        let container = Container::open(bytes)?;
        let palette = match container.entry("Resources/Graphic.xml") {
            Some(raw) => Graphic::parse(raw)?,
            None => Graphic::default(),
        };
        let styles = match container.entry("Resources/Styles.xml") {
            Some(raw) => StyleSheet::parse(raw)?,
            None => StyleSheet::default(),
        };

        // Master spreads parse first so the page → master link is
        // available downstream. The IDML schema for a `<MasterSpread>`
        // is identical to a `<Spread>` (same Page / TextFrame /
        // Rectangle children), so we reuse `Spread::parse`.
        let mut master_spreads: HashMap<String, ParsedMasterSpread> = HashMap::new();
        for src in &container.designmap.master_spreads {
            let raw = container
                .entry(src)
                .ok_or_else(|| OpenError::MissingEntry(src.clone()))?;
            let parsed = Spread::parse(raw)?;
            let self_id = derive_master_id(src);
            master_spreads.insert(
                self_id.clone(),
                ParsedMasterSpread {
                    src: src.clone(),
                    self_id,
                    spread: parsed,
                },
            );
        }

        let mut spreads = Vec::with_capacity(container.designmap.spreads.len());
        let mut frame_for_story = HashMap::new();
        let mut text_frame_index: HashMap<String, (usize, usize)> = HashMap::new();
        for spread_ref in &container.designmap.spreads {
            let raw = container
                .entry(&spread_ref.src)
                .ok_or_else(|| OpenError::MissingEntry(spread_ref.src.clone()))?;
            let parsed = Spread::parse(raw)?;
            let spread_idx = spreads.len();
            for (frame_idx, frame) in parsed.text_frames.iter().enumerate() {
                if let Some(id) = frame.parent_story.clone() {
                    frame_for_story.insert(id, frame.clone());
                }
                if let Some(self_id) = frame.self_id.clone() {
                    text_frame_index.insert(self_id, (spread_idx, frame_idx));
                }
            }
            spreads.push(ParsedSpread {
                src: spread_ref.src.clone(),
                spread: parsed,
            });
        }

        let mut stories = Vec::with_capacity(container.designmap.stories.len());
        for story_ref in &container.designmap.stories {
            let raw = container
                .entry(&story_ref.src)
                .ok_or_else(|| OpenError::MissingEntry(story_ref.src.clone()))?;
            let parsed = Story::parse(raw)?;
            let self_id = derive_story_id(&story_ref.src);
            stories.push(ParsedStory {
                src: story_ref.src.clone(),
                self_id,
                story: parsed,
            });
        }

        Ok(Document {
            container,
            palette,
            spreads,
            stories,
            master_spreads,
            frame_for_story,
            text_frame_index,
            styles,
        })
    }

    /// Look up a master spread by its `Self` id (the suffix stripped
    /// from the manifest src) or by the full reference value used in
    /// `Page::applied_master` (e.g. `MasterSpread/uad`).
    pub fn master_spread(&self, reference: &str) -> Option<&ParsedMasterSpread> {
        if let Some(m) = self.master_spreads.get(reference) {
            return Some(m);
        }
        // `applied_master` is typically `MasterSpread/<id>`; our key is
        // the bare `<id>`. Strip the prefix when needed.
        let stripped = reference
            .rsplit_once('/')
            .map(|(_, id)| id)
            .unwrap_or(reference);
        self.master_spreads.get(stripped)
    }

    /// The frame that hosts a story, looked up by the story's
    /// `self_id`. `None` means the story is unplaced — permissible
    /// in IDML.
    pub fn frame_for(&self, story_id: &str) -> Option<&TextFrame> {
        self.frame_for_story.get(story_id)
    }

    /// Look up a `TextFrame` by its `Self` id (e.g. `frameA`).
    /// O(1) via the `text_frame_index` map built at open time.
    pub fn text_frame(&self, frame_self_id: &str) -> Option<&TextFrame> {
        let &(spread_idx, frame_idx) = self.text_frame_index.get(frame_self_id)?;
        self.spreads
            .get(spread_idx)
            .and_then(|s| s.spread.text_frames.get(frame_idx))
    }

    /// Frame chain for a story: starts at the frame that is the
    /// chain head (a frame hosting `story_id` whose `Self` id is
    /// not another frame's `NextTextFrame` target) and follows
    /// `NextTextFrame` links until exhaustion. Cycles are bounded
    /// by `MAX_FRAME_CHAIN` so a malformed document can't hang.
    /// Returns `Vec<&TextFrame>` borrowing from the document.
    pub fn frame_chain(&self, story_id: &str) -> Vec<&TextFrame> {
        // Collect every frame on this story (typically 1; can be N
        // when the story is threaded across multiple frames).
        let mut story_frames: Vec<&TextFrame> = Vec::new();
        for parsed in &self.spreads {
            for f in &parsed.spread.text_frames {
                if f.parent_story.as_deref() == Some(story_id) {
                    story_frames.push(f);
                }
            }
        }
        if story_frames.is_empty() {
            return Vec::new();
        }
        // The head is whichever frame on this story isn't another
        // frame's NextTextFrame target. If every frame is targeted
        // (i.e. a cycle), fall back to the first frame found.
        let targeted: std::collections::HashSet<&str> = story_frames
            .iter()
            .filter_map(|f| f.next_text_frame.as_deref())
            .collect();
        let head = story_frames
            .iter()
            .find(|f| match f.self_id.as_deref() {
                Some(id) => !targeted.contains(id),
                None => true,
            })
            .copied()
            .unwrap_or(story_frames[0]);

        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        out.push(head);
        if let Some(id) = head.self_id.as_deref() {
            seen.insert(id.to_string());
        }
        let mut cursor = head.next_text_frame.clone();
        for _ in 0..MAX_FRAME_CHAIN {
            let Some(id) = cursor else { break };
            if seen.contains(&id) {
                break;
            }
            let Some(next) = self.text_frame(&id) else {
                break;
            };
            out.push(next);
            seen.insert(id);
            cursor = next.next_text_frame.clone();
        }
        out
    }

    /// Bytes of a sub-resource in the underlying container (fonts,
    /// linked images, ICC profiles — anything the manifest or frames
    /// reference but that `Document` doesn't parse itself).
    pub fn entry(&self, path: &str) -> Option<&[u8]> {
        self.container.entry(path).map(|b| b.as_ref())
    }

    /// Resolve a run's effective character-level attributes by
    /// walking the cascade: direct on the run > applied character
    /// style > applied paragraph style. Each attribute falls
    /// through to the next layer only when unset above.
    pub fn resolved_run_attrs(
        &self,
        paragraph: &Paragraph,
        run: &CharacterRun,
    ) -> ResolvedRunAttrs {
        let mut acc = ResolvedRunAttrs::from_run(run);
        if let Some(id) = run.character_style.as_deref() {
            acc.merge_below_character(&self.styles.resolve_character(id));
        }
        if let Some(id) = paragraph.paragraph_style.as_deref() {
            acc.merge_below_paragraph(&self.styles.resolve_paragraph(id));
        }
        acc
    }

    /// Resolve a paragraph's effective paragraph-level attributes.
    /// The cascade is direct > applied paragraph style. Character
    /// styles don't carry paragraph attrs in IDML.
    pub fn resolved_paragraph_attrs(&self, paragraph: &Paragraph) -> ResolvedParagraphAttrs {
        let mut acc = ResolvedParagraphAttrs::from_paragraph(paragraph);
        if let Some(id) = paragraph.paragraph_style.as_deref() {
            acc.merge_below(&self.styles.resolve_paragraph(id));
        }
        acc
    }

    /// Manifest-advertised story metadata; a convenience for callers
    /// that need the original src paths without walking `stories`.
    pub fn story_refs(&self) -> &[StoryRef] {
        &self.container.designmap.stories
    }

    /// Resolve a `<TOCStyle>` into a flat ordered list of TOC entries.
    ///
    /// Walks every story's paragraphs in document order; whenever a
    /// paragraph's `AppliedParagraphStyle` matches an entry's
    /// `IncludeStyle`, we emit a [`TOCEntry`] carrying:
    ///
    /// - the entry's `level` / `separator` / page-number toggle,
    /// - the resolved entry text (concatenated runs, stripped of
    ///   placeholder markers),
    /// - the body page-index of the paragraph's host frame.
    ///
    /// The page-index assignment is conservative: it returns the page
    /// hosting the *head* frame of the paragraph's parent story. A
    /// long threaded story that breaks across many frames still
    /// resolves every TOC entry from its style to that head frame —
    /// the renderer's chain pass is what would distribute paragraphs
    /// across pages at emit time, and the scene layer doesn't run
    /// composition. Most real-world TOC fixtures use one frame per
    /// story per page anyway, so the heuristic matches InDesign's
    /// output in the common case.
    ///
    /// `None` for an entry's `page_number` means the document has no
    /// host frame for the paragraph (orphan story) — the renderer
    /// should suppress the page number for that row.
    pub fn resolve_toc(&self, toc_style: &TOCStyleDef) -> Vec<TOCEntry> {
        let mut out: Vec<TOCEntry> = Vec::new();
        let body_page_index = self.body_page_index_map();
        for parsed in &self.stories {
            for paragraph in &parsed.story.paragraphs {
                let Some(applied) = paragraph.paragraph_style.as_deref() else {
                    continue;
                };
                let Some(entry_def) = toc_style
                    .entries
                    .iter()
                    .find(|e| e.include_style.as_deref() == Some(applied))
                else {
                    continue;
                };
                let text = paragraph_plain_text(paragraph);
                if text.is_empty() {
                    continue;
                }
                let page_number = body_page_index.get(&parsed.self_id).copied();
                out.push(TOCEntry {
                    level: entry_def.level.unwrap_or(1),
                    text,
                    page_number,
                    separator: entry_def.separator.clone().unwrap_or_else(|| "^t".to_string()),
                    format_style: entry_def.format_style.clone(),
                    include_style: applied.to_string(),
                    page_number_visible: !matches!(
                        entry_def.page_number.as_deref(),
                        Some("Off") | Some("NoPageNumber")
                    ),
                });
            }
        }
        out
    }

    /// Map story-id → body page-index of the chain head frame.
    ///
    /// Body page indices are 0-based and match the renderer's page
    /// ordering: spreads are concatenated in manifest order, pages
    /// inside each spread land in document order. The map is keyed
    /// off the *parent* story (not threaded targets), so a chain
    /// hosted across multiple frames resolves to its head frame's
    /// page. Stories with no hosting frame are absent.
    fn body_page_index_map(&self) -> HashMap<String, usize> {
        let mut out: HashMap<String, usize> = HashMap::new();
        // Pre-compute per-spread page index offsets so we can map
        // (spread_idx, local_page_idx) → body page index in O(1).
        let mut spread_page_offsets = Vec::with_capacity(self.spreads.len() + 1);
        let mut running = 0usize;
        for parsed in &self.spreads {
            spread_page_offsets.push(running);
            running += parsed.spread.pages.len().max(1);
        }
        spread_page_offsets.push(running);

        for parsed_story in &self.stories {
            let chain = self.frame_chain(&parsed_story.self_id);
            let Some(head) = chain.first() else { continue };
            let Some(self_id) = head.self_id.as_deref() else {
                continue;
            };
            let Some(&(spread_idx, _frame_idx)) = self.text_frame_index.get(self_id) else {
                continue;
            };
            let spread = match self.spreads.get(spread_idx) {
                Some(s) => &s.spread,
                None => continue,
            };
            let local_page = page_index_for_bounds(&spread.pages, head.bounds, head.item_transform)
                .unwrap_or(0);
            out.insert(
                parsed_story.self_id.clone(),
                spread_page_offsets[spread_idx] + local_page,
            );
        }
        out
    }
}

/// Find the page in a spread that contains a frame's centroid. Uses
/// the frame's `GeometricBounds` after applying its `ItemTransform`
/// (since bounds are stored in the frame's inner coords). Returns
/// `None` if no page contains the centroid — caller defaults to the
/// first page.
fn page_index_for_bounds(
    pages: &[idml_parse::Page],
    bounds: Bounds,
    item_transform: Option<[f32; 6]>,
) -> Option<usize> {
    let cx = (bounds.left + bounds.right) * 0.5;
    let cy = (bounds.top + bounds.bottom) * 0.5;
    let (cx, cy) = match item_transform {
        Some([a, b, c, d, tx, ty]) => (a * cx + c * cy + tx, b * cx + d * cy + ty),
        None => (cx, cy),
    };
    pages.iter().position(|p| {
        let (left, right, top, bottom) = match p.item_transform {
            // Real IDML page ItemTransforms are pure translation
            // (InDesign limits the field to dx/dy plus 0/90/180/270
            // rotation); rotation is rare for body pages so we treat
            // the matrix as translation-only for containment.
            Some([_, _, _, _, tx, ty]) => (
                p.bounds.left + tx,
                p.bounds.right + tx,
                p.bounds.top + ty,
                p.bounds.bottom + ty,
            ),
            None => (p.bounds.left, p.bounds.right, p.bounds.top, p.bounds.bottom),
        };
        cx >= left && cx <= right && cy >= top && cy <= bottom
    })
}

/// Concatenate a paragraph's `runs` text into a plain string,
/// dropping the IDML auto-page-number / next-page-number sentinel
/// characters (they'd appear as private-use code-points in the TOC
/// output otherwise).
fn paragraph_plain_text(p: &Paragraph) -> String {
    let mut buf = String::new();
    for run in &p.runs {
        for ch in run.text.chars() {
            if ch == idml_parse::AUTO_PAGE_NUMBER_MARKER || ch == idml_parse::NEXT_PAGE_NUMBER_MARKER
            {
                continue;
            }
            buf.push(ch);
        }
    }
    buf.trim().to_string()
}

/// One resolved row of a Table of Contents.
///
/// The renderer composes one paragraph per entry, applying the
/// referenced `format_style`, the entry's `text` followed by
/// `separator`, then the formatted page number (when
/// `page_number_visible`).
#[derive(Debug, Clone, PartialEq)]
pub struct TOCEntry {
    /// Outline depth, 1-based. Top-level entries are level 1.
    pub level: u32,
    /// The trimmed text of the source paragraph.
    pub text: String,
    /// Body page-index of the paragraph's host frame head, or `None`
    /// for orphan stories (no hosting frame).
    pub page_number: Option<usize>,
    /// Separator string between `text` and the page number. IDML
    /// serialises tabs as `^t`; the renderer expands them at use
    /// time. Default is `"^t"`.
    pub separator: String,
    /// `FormatStyle` reference from the matching `<TOCStyleEntry>`,
    /// or `None` when the entry didn't declare one.
    pub format_style: Option<String>,
    /// The `IncludeStyle` reference this entry matched against — kept
    /// for debugging / book-style cross-referencing.
    pub include_style: String,
    /// When `false`, the IDML entry asked to suppress the page
    /// number (`PageNumber="Off"` / `NoPageNumber"`). The renderer
    /// should drop the separator + number for these rows.
    pub page_number_visible: bool,
}

impl ResolvedRunAttrs {
    /// Capture a run's directly-set fields into a fresh
    /// `ResolvedRunAttrs`. Style-cascade fallbacks apply via
    /// `merge_below_character` / `merge_below_paragraph`.
    pub fn from_run(run: &CharacterRun) -> Self {
        Self {
            font: run.font.clone(),
            font_style: run.font_style.clone(),
            point_size: run.point_size,
            fill_color: run.fill_color.clone(),
            fill_tint: run.fill_tint,
            stroke_color: run.stroke_color.clone(),
            stroke_weight: run.stroke_weight,
            capitalization: run.capitalization.clone(),
            baseline_shift: run.baseline_shift,
            horizontal_scale: run.horizontal_scale,
            vertical_scale: run.vertical_scale,
            skew: run.skew,
            position: run.position.clone(),
            tracking: run.tracking,
            underline: run.underline,
            strikethru: run.strikethru,
            leading: run.leading,
            ruby_flag: run.ruby_flag,
            ruby_type: run.ruby_type.clone(),
            ruby_string: run.ruby_string.clone(),
            kenten_kind: run.kenten_kind.clone(),
            kenten_character: run.kenten_character.clone(),
            kenten_font_size: run.kenten_font_size,
            overprint_fill: run.overprint_fill,
            overprint_stroke: run.overprint_stroke,
        }
    }

    /// Fill any unset field from a resolved character style.
    pub fn merge_below_character(&mut self, c: &idml_parse::ResolvedCharacter) {
        if self.font.is_none() {
            self.font = c.font.clone();
        }
        if self.font_style.is_none() {
            self.font_style = c.font_style.clone();
        }
        self.point_size = self.point_size.or(c.point_size);
        if self.fill_color.is_none() {
            self.fill_color = c.fill_color.clone();
        }
        self.fill_tint = self.fill_tint.or(c.fill_tint);
        if self.stroke_color.is_none() {
            self.stroke_color = c.stroke_color.clone();
        }
        self.stroke_weight = self.stroke_weight.or(c.stroke_weight);
        if self.capitalization.is_none() {
            self.capitalization = c.capitalization.clone();
        }
        self.baseline_shift = self.baseline_shift.or(c.baseline_shift);
        self.horizontal_scale = self.horizontal_scale.or(c.horizontal_scale);
        self.vertical_scale = self.vertical_scale.or(c.vertical_scale);
        self.skew = self.skew.or(c.skew);
        if self.position.is_none() {
            self.position = c.position.clone();
        }
        self.tracking = self.tracking.or(c.tracking);
        self.underline = self.underline.or(c.underline);
        self.strikethru = self.strikethru.or(c.strikethru);
        self.ruby_flag = self.ruby_flag.or(c.ruby_flag);
        if self.ruby_type.is_none() {
            self.ruby_type = c.ruby_type.clone();
        }
        if self.ruby_string.is_none() {
            self.ruby_string = c.ruby_string.clone();
        }
        if self.kenten_kind.is_none() {
            self.kenten_kind = c.kenten_kind.clone();
        }
        if self.kenten_character.is_none() {
            self.kenten_character = c.kenten_character.clone();
        }
        self.kenten_font_size = self.kenten_font_size.or(c.kenten_font_size);
        self.overprint_fill = self.overprint_fill.or(c.overprint_fill);
        self.overprint_stroke = self.overprint_stroke.or(c.overprint_stroke);
    }

    /// Fill any unset field from a resolved paragraph style.
    /// Run-level can pull font / size / fill out of paragraph
    /// styles but not the paragraph-only knobs.
    pub fn merge_below_paragraph(&mut self, p: &idml_parse::ResolvedParagraph) {
        if self.font.is_none() {
            self.font = p.font.clone();
        }
        if self.font_style.is_none() {
            self.font_style = p.font_style.clone();
        }
        self.point_size = self.point_size.or(p.point_size);
        if self.fill_color.is_none() {
            self.fill_color = p.fill_color.clone();
        }
        self.fill_tint = self.fill_tint.or(p.fill_tint);
        if self.stroke_color.is_none() {
            self.stroke_color = p.stroke_color.clone();
        }
        self.stroke_weight = self.stroke_weight.or(p.stroke_weight);
        if self.capitalization.is_none() {
            self.capitalization = p.capitalization.clone();
        }
        self.baseline_shift = self.baseline_shift.or(p.baseline_shift);
        self.horizontal_scale = self.horizontal_scale.or(p.horizontal_scale);
        self.vertical_scale = self.vertical_scale.or(p.vertical_scale);
        self.skew = self.skew.or(p.skew);
        if self.position.is_none() {
            self.position = p.position.clone();
        }
        self.tracking = self.tracking.or(p.tracking);
        self.underline = self.underline.or(p.underline);
        self.strikethru = self.strikethru.or(p.strikethru);
        self.overprint_fill = self.overprint_fill.or(p.overprint_fill);
        self.overprint_stroke = self.overprint_stroke.or(p.overprint_stroke);
    }
}

impl ResolvedParagraphAttrs {
    /// Capture a paragraph's directly-set fields. Style cascade
    /// fallbacks apply via `merge_below`. Local override values
    /// (BulletsAndNumberingListType / BulletChar) carried directly on
    /// the ParagraphStyleRange win over the cascaded paragraph style.
    pub fn from_paragraph(paragraph: &Paragraph) -> Self {
        Self {
            justification: paragraph.justification,
            first_line_indent: paragraph.first_line_indent,
            space_before: paragraph.space_before,
            space_after: paragraph.space_after,
            tab_list: paragraph.tab_list.clone(),
            bullets_list_type: paragraph.bullets_list_type.clone(),
            bullet_character: paragraph.bullet_character,
            bullets_text_after: None,
            numbering_format: None,
            bullets_character_style: None,
            bullets_and_numbering_digits_character_style: None,
            numbering_expression: None,
            numbering_start_at: None,
            numbering_continue: None,
            hyphenation: None,
            applied_language: None,
            minimum_word_spacing: None,
            desired_word_spacing: None,
            maximum_word_spacing: None,
            minimum_letter_spacing: None,
            desired_letter_spacing: None,
            maximum_letter_spacing: None,
            minimum_glyph_scaling: None,
            desired_glyph_scaling: None,
            maximum_glyph_scaling: None,
            drop_cap_characters: None,
            drop_cap_lines: None,
            drop_cap_detail: None,
            kinsoku_set: paragraph.kinsoku_set.clone(),
            kinsoku_type: paragraph.kinsoku_type.clone(),
            mojikumi_table: paragraph.mojikumi_table.clone(),
            mojikumi_set: paragraph.mojikumi_set.clone(),
            overprint_fill: paragraph.overprint_fill,
            overprint_stroke: paragraph.overprint_stroke,
            // Q-09: paragraph carries no per-paragraph shading
            // override today; the cascade pulls everything from the
            // applied paragraph style.
            shading: Default::default(),
            rule_above: Default::default(),
            rule_below: Default::default(),
            border: Default::default(),
        }
    }

    /// Fill any unset field from a resolved paragraph style.
    pub fn merge_below(&mut self, p: &idml_parse::ResolvedParagraph) {
        self.justification = self.justification.or(p.justification);
        self.first_line_indent = self.first_line_indent.or(p.first_line_indent);
        self.space_before = self.space_before.or(p.space_before);
        self.space_after = self.space_after.or(p.space_after);
        if self.tab_list.is_empty() && !p.tab_list.is_empty() {
            self.tab_list = p.tab_list.clone();
        }
        if self.bullets_list_type.is_none() {
            self.bullets_list_type = p.bullets_list_type.clone();
        }
        self.bullet_character = self.bullet_character.or(p.bullet_character);
        if self.bullets_text_after.is_none() {
            self.bullets_text_after = p.bullets_text_after.clone();
        }
        if self.numbering_format.is_none() {
            self.numbering_format = p.numbering_format.clone();
        }
        if self.bullets_character_style.is_none() {
            self.bullets_character_style = p.bullets_character_style.clone();
        }
        if self.bullets_and_numbering_digits_character_style.is_none() {
            self.bullets_and_numbering_digits_character_style =
                p.bullets_and_numbering_digits_character_style.clone();
        }
        if self.numbering_expression.is_none() {
            self.numbering_expression = p.numbering_expression.clone();
        }
        self.numbering_start_at = self.numbering_start_at.or(p.numbering_start_at);
        self.numbering_continue = self.numbering_continue.or(p.numbering_continue);
        self.hyphenation = self.hyphenation.or(p.hyphenation);
        if self.applied_language.is_none() {
            self.applied_language = p.applied_language.clone();
        }
        self.minimum_word_spacing = self.minimum_word_spacing.or(p.minimum_word_spacing);
        self.desired_word_spacing = self.desired_word_spacing.or(p.desired_word_spacing);
        self.maximum_word_spacing = self.maximum_word_spacing.or(p.maximum_word_spacing);
        // Q-20: letter / glyph spacing per-field inheritance.
        self.minimum_letter_spacing =
            self.minimum_letter_spacing.or(p.minimum_letter_spacing);
        self.desired_letter_spacing =
            self.desired_letter_spacing.or(p.desired_letter_spacing);
        self.maximum_letter_spacing =
            self.maximum_letter_spacing.or(p.maximum_letter_spacing);
        self.minimum_glyph_scaling =
            self.minimum_glyph_scaling.or(p.minimum_glyph_scaling);
        self.desired_glyph_scaling =
            self.desired_glyph_scaling.or(p.desired_glyph_scaling);
        self.maximum_glyph_scaling =
            self.maximum_glyph_scaling.or(p.maximum_glyph_scaling);
        self.drop_cap_characters = self.drop_cap_characters.or(p.drop_cap_characters);
        self.drop_cap_lines = self.drop_cap_lines.or(p.drop_cap_lines);
        self.drop_cap_detail = self.drop_cap_detail.or(p.drop_cap_detail);
        if self.kinsoku_set.is_none() {
            self.kinsoku_set = p.kinsoku_set.clone();
        }
        if self.kinsoku_type.is_none() {
            self.kinsoku_type = p.kinsoku_type.clone();
        }
        if self.mojikumi_table.is_none() {
            self.mojikumi_table = p.mojikumi_table.clone();
        }
        if self.mojikumi_set.is_none() {
            self.mojikumi_set = p.mojikumi_set.clone();
        }
        self.overprint_fill = self.overprint_fill.or(p.overprint_fill);
        self.overprint_stroke = self.overprint_stroke.or(p.overprint_stroke);
        // Q-09: paragraph-shading per-field inheritance.
        let s = &mut self.shading;
        let ps = &p.shading;
        s.on = s.on.or(ps.on);
        if s.color.is_none() {
            s.color = ps.color.clone();
        }
        s.tint = s.tint.or(ps.tint);
        if s.width.is_none() {
            s.width = ps.width.clone();
        }
        s.offset_top = s.offset_top.or(ps.offset_top);
        s.offset_left = s.offset_left.or(ps.offset_left);
        s.offset_bottom = s.offset_bottom.or(ps.offset_bottom);
        s.offset_right = s.offset_right.or(ps.offset_right);
        if s.top_origin.is_none() {
            s.top_origin = ps.top_origin.clone();
        }
        if s.bottom_origin.is_none() {
            s.bottom_origin = ps.bottom_origin.clone();
        }
        s.clip_to_frame = s.clip_to_frame.or(ps.clip_to_frame);
        s.overprint = s.overprint.or(ps.overprint);
        s.suppress_printing = s.suppress_printing.or(ps.suppress_printing);
        // Q-09: per-field rule_above / rule_below inheritance.
        merge_rule_attrs(&mut self.rule_above, &p.rule_above);
        merge_rule_attrs(&mut self.rule_below, &p.rule_below);
        // Q-09: per-field paragraph-border inheritance.
        merge_border_attrs(&mut self.border, &p.border);
    }
}

fn merge_rule_attrs(c: &mut idml_parse::ParagraphRule, p: &idml_parse::ParagraphRule) {
    c.on = c.on.or(p.on);
    if c.color.is_none() {
        c.color = p.color.clone();
    }
    c.tint = c.tint.or(p.tint);
    c.weight = c.weight.or(p.weight);
    c.offset = c.offset.or(p.offset);
    c.left_indent = c.left_indent.or(p.left_indent);
    c.right_indent = c.right_indent.or(p.right_indent);
    if c.width.is_none() {
        c.width = p.width.clone();
    }
}

fn merge_border_attrs(c: &mut idml_parse::ParagraphBorder, p: &idml_parse::ParagraphBorder) {
    c.on = c.on.or(p.on);
    if c.color.is_none() {
        c.color = p.color.clone();
    }
    c.tint = c.tint.or(p.tint);
    c.weight = c.weight.or(p.weight);
    c.offset_top = c.offset_top.or(p.offset_top);
    c.offset_left = c.offset_left.or(p.offset_left);
    c.offset_bottom = c.offset_bottom.or(p.offset_bottom);
    c.offset_right = c.offset_right.or(p.offset_right);
    if c.width.is_none() {
        c.width = p.width.clone();
    }
    for i in 0..4 {
        c.corners[i].option = c.corners[i].option.or(p.corners[i].option);
        c.corners[i].radius = c.corners[i].radius.or(p.corners[i].radius);
    }
}

/// Derive a Story's `Self` id from its manifest src. Turns
/// "Stories/Story_uXX.xml" → "uXX"; returns the stem otherwise.
pub fn derive_story_id(src: &str) -> String {
    let stem = src.rsplit_once('/').map(|(_, t)| t).unwrap_or(src);
    let without_ext = stem.strip_suffix(".xml").unwrap_or(stem);
    without_ext
        .strip_prefix("Story_")
        .map(|s| s.to_string())
        .unwrap_or_else(|| without_ext.to_string())
}

/// Derive a MasterSpread's `Self` id from its manifest src. Turns
/// "MasterSpreads/MasterSpread_uad.xml" → "uad".
pub fn derive_master_id(src: &str) -> String {
    let stem = src.rsplit_once('/').map(|(_, t)| t).unwrap_or(src);
    let without_ext = stem.strip_suffix(".xml").unwrap_or(stem);
    without_ext
        .strip_prefix("MasterSpread_")
        .map(|s| s.to_string())
        .unwrap_or_else(|| without_ext.to_string())
}

/// Effective character-level attributes after walking the cascade
/// (direct > applied character style > applied paragraph style).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedRunAttrs {
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    /// `FillTint` percentage (0..=100). `None` ⇒ use the swatch
    /// at full strength. The renderer scales the resolved RGB toward
    /// paper white by `(1 - tint/100)` when `Some`.
    pub fill_tint: Option<f32>,
    /// Cascaded `StrokeColor` for text outline. `None` ⇒ no outline
    /// (run renders fill-only). Resolves through the same direct >
    /// character-style > paragraph-style chain as every other field.
    /// See [`CharacterRun::stroke_color`].
    pub stroke_color: Option<String>,
    /// Cascaded `StrokeWeight` in pt. The renderer falls back to
    /// 1pt at emit time when `stroke_color` resolves but
    /// `stroke_weight` is None — matching IDML's `<TextDefault>`
    /// default for new documents.
    pub stroke_weight: Option<f32>,
    /// `Capitalization` value (Normal/AllCaps/SmallCaps/CapToSmallCap).
    /// Renderer uppercases the input string before shaping for
    /// `AllCaps` / `SmallCaps` (the latter without OT smcp lookup is
    /// just AllCaps until proper font-feature support arrives).
    pub capitalization: Option<String>,
    /// `BaselineShift` in pt. Applied as a per-glyph y-offset.
    pub baseline_shift: Option<f32>,
    /// `HorizontalScale` percentage (100 = identity). Folded into
    /// glyph x-advance + glyph affine at shape/emit time (P-08).
    pub horizontal_scale: Option<f32>,
    /// `VerticalScale` percentage (100 = identity). Parsed; not yet
    /// honoured.
    pub vertical_scale: Option<f32>,
    /// `Skew` in degrees (positive = right-leaning). Folded into the
    /// glyph affine alongside `HorizontalScale` (P-08).
    pub skew: Option<f32>,
    /// `Position` value (Normal / Superscript / Subscript / etc).
    /// Parsed; not yet honoured.
    pub position: Option<String>,
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// Explicit `Leading` in pt. `None` ⇒ Auto leading
    /// (`point_size × 1.2`).
    pub leading: Option<f32>,
    /// Cascaded `RubyFlag` — true when this run carries ruby
    /// annotation. Parser/scene-only today; renderer integration
    /// queued under Tier 4 — CJK Stage 4.
    pub ruby_flag: Option<bool>,
    /// Cascaded `RubyType` — `PerCharacter` / `GroupRuby`.
    pub ruby_type: Option<String>,
    /// Cascaded `RubyString` — the annotation text.
    pub ruby_string: Option<String>,
    /// Cascaded `KentenKind` — emphasis-mark glyph kind.
    pub kenten_kind: Option<String>,
    /// Cascaded `KentenCharacter` — custom emphasis mark codepoint
    /// when `kenten_kind == "Custom"`.
    pub kenten_character: Option<String>,
    /// Cascaded `KentenFontSize` — % of base size.
    pub kenten_font_size: Option<f32>,
    /// Cascaded `OverprintFill` flag — `true` means this run's fill
    /// should composite with darken (per-channel `min(top, bottom)`)
    /// instead of knocking out the underlying ink. The renderer's
    /// glyph emitter consumes this once run-level overprint is wired
    /// (frame-level overprint already flows through the display list).
    pub overprint_fill: Option<bool>,
    /// Cascaded `OverprintStroke` flag (rare on text — only outlined
    /// text strokes).
    pub overprint_stroke: Option<bool>,
}

/// Effective paragraph-level attributes after walking the cascade
/// (direct > applied paragraph style).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedParagraphAttrs {
    pub justification: Option<idml_parse::Justification>,
    pub first_line_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub tab_list: Vec<idml_parse::TabStop>,
    /// `BulletsAndNumberingListType` (BulletList / NumberedList /
    /// NoList). The renderer only consumes BulletList today.
    pub bullets_list_type: Option<String>,
    /// Unicode codepoint of the bullet glyph (from `<BulletChar>`).
    pub bullet_character: Option<u32>,
    /// Separator string rendered between the bullet and the
    /// paragraph text. IDML serialises tabs as `^t`; the
    /// renderer expands them to `\t` at use time.
    pub bullets_text_after: Option<String>,
    /// `NumberingFormat` sample string from the cascaded paragraph
    /// style. Used by the renderer's `format_number` helper to pick
    /// Arabic / Roman / alpha / zero-padded formatting for
    /// `NumberedList` paragraphs.
    pub numbering_format: Option<String>,
    /// `CharacterStyle/<id>` ref styling the bullet marker (font,
    /// size, colour) independently of the paragraph body. `None` ⇒
    /// inherit the first run's formatting (the historical fallback).
    /// IDML applies this only to `BulletList` paragraphs.
    pub bullets_character_style: Option<String>,
    /// `CharacterStyle/<id>` ref styling the digits of a
    /// `NumberedList` paragraph's marker. The InDesign UI surfaces a
    /// single "Character Style" picker per paragraph style regardless
    /// of list kind, so the renderer also treats this as a fallback
    /// bullet style when `bullets_character_style` is absent.
    pub bullets_and_numbering_digits_character_style: Option<String>,
    /// `NumberingExpression` template (`^#`, `^.`, `^t` tokens plus
    /// literal characters). `None` ⇒ renderer applies the IDML
    /// default `^#.^t`.
    pub numbering_expression: Option<String>,
    /// `NumberingStartAt` explicit integer override; the renderer
    /// resets the story-level counter to this value on paragraph
    /// entry. `None` ⇒ inherit (auto-increment off prior state).
    pub numbering_start_at: Option<i32>,
    /// `NumberingContinue` flag. `Some(true)` suppresses the
    /// auto-reset that fires when the bulleting kind changes
    /// across paragraphs; `Some(false)` forces a reset on entry.
    /// `None` ⇒ renderer default (continue).
    pub numbering_continue: Option<bool>,
    /// `Hyphenation` boolean from the cascaded paragraph style.
    /// Drives whether the composer wires up a hyphenator.
    pub hyphenation: Option<bool>,
    /// `AppliedLanguage` from the cascade — feeds dictionary picking
    /// for hyphenation. Strings like `"$ID/English: USA"`.
    pub applied_language: Option<String>,
    /// `MinimumWordSpacing` (% of normal). 100 = baseline.
    pub minimum_word_spacing: Option<f32>,
    /// `DesiredWordSpacing` (% of normal). 100 = baseline.
    pub desired_word_spacing: Option<f32>,
    /// `MaximumWordSpacing` (% of normal). 100 = baseline.
    pub maximum_word_spacing: Option<f32>,
    /// Q-20: cascaded `MinimumLetterSpacing` pt. Additive signed
    /// adjustment to the inter-glyph advance budget. None ⇒ 0 pt.
    pub minimum_letter_spacing: Option<f32>,
    pub desired_letter_spacing: Option<f32>,
    pub maximum_letter_spacing: Option<f32>,
    /// Q-20: cascaded `MinimumGlyphScaling` percent. 100 ⇒ identity.
    /// Allows the breaker to scale per-glyph x-advance for justification.
    pub minimum_glyph_scaling: Option<f32>,
    pub desired_glyph_scaling: Option<f32>,
    pub maximum_glyph_scaling: Option<f32>,
    /// `DropCapCharacters` from the cascaded paragraph style.
    /// Count of leading characters that drop down across
    /// `drop_cap_lines` lines. 0 / `None` ⇒ no drop cap.
    pub drop_cap_characters: Option<u32>,
    /// `DropCapLines` — vertical extent of the drop cap.
    pub drop_cap_lines: Option<u32>,
    /// `DropCapDetail` — InDesign's scaling integer.
    pub drop_cap_detail: Option<i32>,
    /// Cascaded `KinsokuSet` ref. Identifies the CJK line-break
    /// character set this paragraph follows. Parser/scene captures
    /// the ref; the composer uses a built-in "Hard Kinsoku" set when
    /// `kinsoku_type` triggers enforcement. See docs/plan.md Tier 4
    /// — CJK Stage 2.
    pub kinsoku_set: Option<String>,
    /// Cascaded `KinsokuType` flavour
    /// (`WordbreakWithJustification` / `PushIn` / `PushOut`). The
    /// composer keys "any value present" → "apply hard-kinsoku
    /// penalty" today.
    pub kinsoku_type: Option<String>,
    /// Cascaded `MojikumiTable` ref. Parser/scene-only; the
    /// renderer does not yet implement Mojikumi spacing
    /// adjustments. See docs/plan.md Tier 4 — CJK Stage 4.
    pub mojikumi_table: Option<String>,
    /// Cascaded `MojikumiSet` ref (older IDML attribute name).
    pub mojikumi_set: Option<String>,
    /// Cascaded `OverprintFill` flag from the paragraph cascade. Stage
    /// 3 frame-level overprint flows through `ResolvedFrame`; this
    /// surface lets the future glyph-level overprint path consume the
    /// paragraph cascade.
    pub overprint_fill: Option<bool>,
    /// Cascaded `OverprintStroke` flag.
    pub overprint_stroke: Option<bool>,
    /// Q-09: cascaded paragraph-shading parameters.
    pub shading: idml_parse::ParagraphShading,
    /// Q-09: cascaded horizontal rule above the first line.
    pub rule_above: idml_parse::ParagraphRule,
    /// Q-09: cascaded horizontal rule below the last line.
    pub rule_below: idml_parse::ParagraphRule,
    /// Q-09: cascaded rectangular paragraph border.
    pub border: idml_parse::ParagraphBorder,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("manifest lists {0} but archive has no such entry")]
    MissingEntry(String),
    #[error("parse: {0}")]
    Parse(#[from] ParseError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use idml_parse::{TOCStyleDef, TOCStyleEntryDef};
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    #[test]
    fn story_id_strips_dir_and_prefix() {
        assert_eq!(derive_story_id("Stories/Story_u10.xml"), "u10");
        assert_eq!(derive_story_id("u10.xml"), "u10");
        assert_eq!(derive_story_id("Stories/custom_u10.xml"), "custom_u10");
    }

    /// Pack an IDML with three body pages on a single spread, three
    /// host frames (one per page, each with one story), where each
    /// paragraph carries an explicit `AppliedParagraphStyle` so the
    /// resolver can filter against it. The three frames are arranged
    /// vertically so per-page centroid containment lands each frame
    /// on a different page.
    fn pack_toc_idml(paragraphs: &[(&str, &str, &str)]) -> Vec<u8> {
        // paragraphs: (story_id, applied_style, text) — one per
        // paragraph; multiple entries with the same story_id stack
        // inside that story in order.
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();

        let mut story_ids: Vec<&str> = Vec::new();
        for (sid, _, _) in paragraphs {
            if !story_ids.contains(sid) {
                story_ids.push(sid);
            }
        }

        // designmap references all stories + the single spread.
        let mut designmap = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
"#,
        );
        for sid in &story_ids {
            designmap.push_str(&format!(
                "  <idPkg:Story src=\"Stories/Story_{sid}.xml\"/>\n"
            ));
        }
        designmap.push_str("</Document>");
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(designmap.as_bytes()).unwrap();

        // One page per story, vertically stacked.
        let mut spread = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
"#,
        );
        for (i, _sid) in story_ids.iter().enumerate() {
            let top = (i as f32) * 100.0;
            let bottom = top + 100.0;
            spread.push_str(&format!(
                "    <Page Self=\"page{i}\" GeometricBounds=\"{top} 0 {bottom} 100\"/>\n"
            ));
        }
        for (i, sid) in story_ids.iter().enumerate() {
            let top = (i as f32) * 100.0;
            let bottom = top + 100.0;
            spread.push_str(&format!(
                "    <TextFrame Self=\"frame_{sid}\" ParentStory=\"{sid}\" GeometricBounds=\"{top} 0 {bottom} 100\"/>\n"
            ));
        }
        spread.push_str("  </Spread>\n</idPkg:Spread>");
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(spread.as_bytes()).unwrap();

        for sid in &story_ids {
            let mut story = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story>
"#,
            );
            for (psid, style, text) in paragraphs {
                if psid != sid {
                    continue;
                }
                story.push_str(&format!(
                    "    <ParagraphStyleRange AppliedParagraphStyle=\"{style}\">
      <CharacterStyleRange><Content>{text}</Content></CharacterStyleRange>
    </ParagraphStyleRange>
"
                ));
            }
            story.push_str("  </Story>\n</idPkg:Story>");
            zip.start_file(format!("Stories/Story_{sid}.xml"), deflated)
                .unwrap();
            zip.write_all(story.as_bytes()).unwrap();
        }

        zip.finish().unwrap().into_inner()
    }

    fn toc_style_with_two_entries() -> TOCStyleDef {
        TOCStyleDef {
            self_id: "TOCStyle/Main".to_string(),
            name: Some("Main".to_string()),
            title: Some("Contents".to_string()),
            title_style: Some("ParagraphStyle/TocTitle".to_string()),
            include_book_documents: Some(false),
            include_hidden: Some(false),
            run_in: Some(false),
            entries: vec![
                TOCStyleEntryDef {
                    name: Some("H1".to_string()),
                    include_style: Some("ParagraphStyle/Heading1".to_string()),
                    format_style: Some("ParagraphStyle/TocFormat1".to_string()),
                    level: Some(1),
                    page_number: Some("On".to_string()),
                    separator: Some("^t".to_string()),
                },
                TOCStyleEntryDef {
                    name: Some("H2".to_string()),
                    include_style: Some("ParagraphStyle/Heading2".to_string()),
                    format_style: Some("ParagraphStyle/TocFormat2".to_string()),
                    level: Some(2),
                    page_number: Some("On".to_string()),
                    separator: Some(" -- ".to_string()),
                },
            ],
        }
    }

    #[test]
    fn resolve_toc_picks_paragraphs_in_document_order() {
        // Three stories on three pages. Story 'intro' (page 0) holds
        // H1 "Intro" + a Body paragraph (should be ignored) + H2
        // "Background". Story 'mid' (page 1) has H2 "Setup". Story
        // 'tail' (page 2) has H1 "Results".
        let bytes = pack_toc_idml(&[
            ("intro", "ParagraphStyle/Heading1", "Intro"),
            ("intro", "ParagraphStyle/Body", "Skip me"),
            ("intro", "ParagraphStyle/Heading2", "Background"),
            ("mid", "ParagraphStyle/Heading2", "Setup"),
            ("tail", "ParagraphStyle/Heading1", "Results"),
        ]);
        let doc = Document::open(&bytes).expect("open IDML");
        let toc = toc_style_with_two_entries();
        let entries = doc.resolve_toc(&toc);
        assert_eq!(entries.len(), 4, "{:?}", entries);
        assert_eq!(entries[0].text, "Intro");
        assert_eq!(entries[0].level, 1);
        assert_eq!(entries[0].page_number, Some(0));
        assert_eq!(
            entries[0].format_style.as_deref(),
            Some("ParagraphStyle/TocFormat1")
        );
        assert_eq!(entries[0].separator, "^t");

        assert_eq!(entries[1].text, "Background");
        assert_eq!(entries[1].level, 2);
        assert_eq!(entries[1].page_number, Some(0));
        assert_eq!(entries[1].separator, " -- ");

        assert_eq!(entries[2].text, "Setup");
        assert_eq!(entries[2].level, 2);
        assert_eq!(entries[2].page_number, Some(1));

        assert_eq!(entries[3].text, "Results");
        assert_eq!(entries[3].level, 1);
        assert_eq!(entries[3].page_number, Some(2));
    }

    #[test]
    fn resolve_toc_respects_page_number_off_flag() {
        let bytes = pack_toc_idml(&[("intro", "ParagraphStyle/Heading1", "Foreword")]);
        let doc = Document::open(&bytes).expect("open IDML");
        let mut toc = toc_style_with_two_entries();
        toc.entries[0].page_number = Some("NoPageNumber".to_string());
        let entries = doc.resolve_toc(&toc);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].page_number_visible);
    }

    #[test]
    fn resolve_toc_uses_default_separator_when_absent() {
        let bytes = pack_toc_idml(&[("intro", "ParagraphStyle/Heading1", "Foreword")]);
        let doc = Document::open(&bytes).expect("open IDML");
        let mut toc = toc_style_with_two_entries();
        toc.entries[0].separator = None;
        let entries = doc.resolve_toc(&toc);
        assert_eq!(entries[0].separator, "^t");
    }

    #[test]
    fn character_style_fill_color_wins_over_paragraph_style() {
        use idml_parse::{CharacterStyleDef, ParagraphStyleDef, StyleSheet};

        let mut styles = StyleSheet::default();
        styles.paragraph_styles.insert(
            "ParagraphStyle/Body".to_string(),
            ParagraphStyleDef {
                self_id: "ParagraphStyle/Body".to_string(),
                fill_color: Some("Color/Black".to_string()),
                ..Default::default()
            },
        );
        styles.character_styles.insert(
            "CharacterStyle/Inverse".to_string(),
            CharacterStyleDef {
                self_id: "CharacterStyle/Inverse".to_string(),
                fill_color: Some("Color/Paper".to_string()),
                ..Default::default()
            },
        );

        let paragraph = Paragraph {
            paragraph_style: Some("ParagraphStyle/Body".to_string()),
            ..Default::default()
        };
        let run = CharacterRun {
            character_style: Some("CharacterStyle/Inverse".to_string()),
            ..Default::default()
        };

        let mut acc = ResolvedRunAttrs::from_run(&run);
        if let Some(id) = run.character_style.as_deref() {
            acc.merge_below_character(&styles.resolve_character(id));
        }
        if let Some(id) = paragraph.paragraph_style.as_deref() {
            acc.merge_below_paragraph(&styles.resolve_paragraph(id));
        }

        assert_eq!(acc.fill_color.as_deref(), Some("Color/Paper"));
    }
}
