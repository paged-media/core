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
    CharacterRun, Container, Graphic, Paragraph, ParseError, Spread, Story, StoryRef, StyleSheet,
    TextFrame,
};

/// Owned, parsed representation of an IDML document.
#[derive(Debug)]
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
            tracking: run.tracking,
            underline: run.underline,
            strikethru: run.strikethru,
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
        self.tracking = self.tracking.or(c.tracking);
        self.underline = self.underline.or(c.underline);
        self.strikethru = self.strikethru.or(c.strikethru);
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
        self.tracking = self.tracking.or(p.tracking);
        self.underline = self.underline.or(p.underline);
        self.strikethru = self.strikethru.or(p.strikethru);
    }
}

impl ResolvedParagraphAttrs {
    /// Capture a paragraph's directly-set fields. Style cascade
    /// fallbacks apply via `merge_below`.
    pub fn from_paragraph(paragraph: &Paragraph) -> Self {
        Self {
            justification: paragraph.justification.clone(),
            first_line_indent: paragraph.first_line_indent,
            space_before: paragraph.space_before,
            space_after: paragraph.space_after,
            tab_list: paragraph.tab_list.clone(),
        }
    }

    /// Fill any unset field from a resolved paragraph style.
    pub fn merge_below(&mut self, p: &idml_parse::ResolvedParagraph) {
        if self.justification.is_none() {
            self.justification = p.justification.clone();
        }
        self.first_line_indent = self.first_line_indent.or(p.first_line_indent);
        self.space_before = self.space_before.or(p.space_before);
        self.space_after = self.space_after.or(p.space_after);
        if self.tab_list.is_empty() && !p.tab_list.is_empty() {
            self.tab_list = p.tab_list.clone();
        }
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
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
}

/// Effective paragraph-level attributes after walking the cascade
/// (direct > applied paragraph style).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedParagraphAttrs {
    pub justification: Option<String>,
    pub first_line_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub tab_list: Vec<idml_parse::TabStop>,
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

    #[test]
    fn story_id_strips_dir_and_prefix() {
        assert_eq!(derive_story_id("Stories/Story_u10.xml"), "u10");
        assert_eq!(derive_story_id("u10.xml"), "u10");
        assert_eq!(derive_story_id("Stories/custom_u10.xml"), "custom_u10");
    }
}
