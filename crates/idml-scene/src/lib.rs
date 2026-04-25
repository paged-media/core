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
        for spread_ref in &container.designmap.spreads {
            let raw = container
                .entry(&spread_ref.src)
                .ok_or_else(|| OpenError::MissingEntry(spread_ref.src.clone()))?;
            let parsed = Spread::parse(raw)?;
            for frame in &parsed.text_frames {
                if let Some(id) = frame.parent_story.clone() {
                    frame_for_story.insert(id, frame.clone());
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
        let char_resolved = run
            .character_style
            .as_deref()
            .map(|id| self.styles.resolve_character(id))
            .unwrap_or_default();
        let para_resolved = paragraph
            .paragraph_style
            .as_deref()
            .map(|id| self.styles.resolve_paragraph(id))
            .unwrap_or_default();
        ResolvedRunAttrs {
            font: run
                .font
                .clone()
                .or(char_resolved.font)
                .or(para_resolved.font),
            font_style: run
                .font_style
                .clone()
                .or(char_resolved.font_style)
                .or(para_resolved.font_style),
            point_size: run
                .point_size
                .or(char_resolved.point_size)
                .or(para_resolved.point_size),
            fill_color: run
                .fill_color
                .clone()
                .or(char_resolved.fill_color)
                .or(para_resolved.fill_color),
            tracking: run
                .tracking
                .or(char_resolved.tracking)
                .or(para_resolved.tracking),
        }
    }

    /// Resolve a paragraph's effective paragraph-level attributes.
    /// The cascade is direct > applied paragraph style. Character
    /// styles don't carry paragraph attrs in IDML.
    pub fn resolved_paragraph_attrs(&self, paragraph: &Paragraph) -> ResolvedParagraphAttrs {
        let para = paragraph
            .paragraph_style
            .as_deref()
            .map(|id| self.styles.resolve_paragraph(id))
            .unwrap_or_default();
        ResolvedParagraphAttrs {
            justification: paragraph.justification.clone().or(para.justification),
            first_line_indent: paragraph.first_line_indent.or(para.first_line_indent),
            space_before: paragraph.space_before.or(para.space_before),
            space_after: paragraph.space_after.or(para.space_after),
        }
    }

    /// Manifest-advertised story metadata; a convenience for callers
    /// that need the original src paths without walking `stories`.
    pub fn story_refs(&self) -> &[StoryRef] {
        &self.container.designmap.stories
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
}

/// Effective paragraph-level attributes after walking the cascade
/// (direct > applied paragraph style).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedParagraphAttrs {
    pub justification: Option<String>,
    pub first_line_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
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
