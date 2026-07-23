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

//! IDML parser.
//!
//! Consumes an IDML ZIP archive and produces a typed AST. Schema coverage
//! is driven by the reference-reading week described in the development
//! plan (Scribus `importidmlplugin.cpp`, SimpleIDML, Adobe's IDML spec).
//!
//! The current surface is intentionally thin: it opens the container,
//! confirms the mimetype, locates the root `designmap.xml`, and exposes a
//! streaming reader the higher layers can pull from. Typed scene
//! extraction lives in `paged-scene`; this crate stays focused on ZIP+XML
//! plumbing.

use std::io::{self, Cursor, Read};

use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub mod designmap;
pub mod graphic;
pub mod spread;
pub mod story;
pub mod styles;
mod util;

pub use designmap::{
    parse_designmap, ColorSettings, DesignMap, DocumentPreference, Hyperlink, HyperlinkDestination,
    HyperlinkDestinationKind, Layer, NumberingStyle, Section, SpreadRef, StoryRef, TextVariable,
};
pub use graphic::{
    parse_graphic, ColorEntry, ColorModel, ColorSpace, GradientEntry, GradientKind,
    GradientStopRef, Graphic, SwatchEntry,
};
pub use spread::{
    parse_spread, ArrowheadType, AutoSizingReferencePoint, AutoSizingType, BevelEmbossParams,
    Bounds, ClippingPathSettings, ClippingType, ContourOptionType, CornerOption, CornerSpec,
    DirectionalFeatherParams, DropShadowSetting, FeatherParams, FirstBaselineOffset, FrameEffects,
    FrameFittingOption, FrameRef, GradientFeatherParams, GradientFeatherStop, GraphicLine, Group,
    GroupTransparency, GuideOrientation, ImageMetadata, InnerGlowParams, InnerShadowParams,
    MarginPreference, OuterGlowParams, Oval, Page, PathAnchor, Polygon, Rectangle, RulerGuide,
    SatinParams, Spread, TextFrame, TextPath, TextWrap, TextWrapMode, VerticalJustification,
};
pub use story::{
    parse_story, AnchoredFrame, AnchoredFrameKind, AnchoredObjectSetting, CellDiagonal,
    CharacterRun, Justification, OtfFeatures, Paragraph, PlaceholderField, Story, TabStop, Table,
    TableBorder, TableCell, TableColumn, TableLineStrokes, TableRow, AUTO_PAGE_NUMBER_MARKER,
    NEXT_PAGE_NUMBER_MARKER,
};
pub use styles::{
    parse_stylesheet, CellStyleDef, CharacterStyleDef, ConditionDef, NestedDelimiter, NestedStyle,
    ObjectStyleDef, ParagraphBorder, ParagraphRule, ParagraphShading, ParagraphStyleDef,
    ResolvedCell, ResolvedCharacter, ResolvedObject, ResolvedParagraph, ResolvedTable, StripeDef,
    StrokeStyleDef, StrokeStyleKind, StyleSheet, TOCStyleDef, TOCStyleEntryDef, TableStyleDef,
};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("not an IDML container: {0}")]
    NotIdml(String),
    #[error("missing required entry {0}")]
    MissingEntry(&'static str),
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("xml: {0}")]
    Xml(#[from] quick_xml::Error),
}

/// The raw IDML source archive — decompressed entries held in memory (IDML
/// carry-through only; no model data lives here — N7). Renamed from `Container`.
///
/// The raw-entry map keeps `Bytes` so downstream crates can slice sub-
/// resources (individual `Stories/Story_*.xml` etc.) without copying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceArchive {
    pub mimetype: String,
    /// Raw `designmap.xml` bytes. IDML carry-through only — never part of the
    /// native model serialization (the structured `designmap` on `Document` is
    /// the truth); defaults to empty on native deserialize (N1, Approach A).
    #[serde(skip)]
    pub designmap_raw: Bytes,
    /// Full decompressed archive contents keyed by entry path. IDML
    /// carry-through only (render-dead) — `#[serde(skip)]` so the native model
    /// never stores the raw IDML package; empty after native deserialize.
    #[serde(skip)]
    pub entries: std::collections::BTreeMap<String, Bytes>,
}

/// Open an IDML source archive from raw bytes — unzips the archive and confirms
/// the mimetype, retaining `designmap.xml` bytes for the scene layer to parse.
/// (De-inherented from `Container::open` — N7.)
pub fn open_source_archive(bytes: &[u8]) -> Result<SourceArchive, ParseError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut entries = std::collections::BTreeMap::<String, Bytes>::new();

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        entries.insert(name, Bytes::from(buf));
    }

    let mimetype = entries
        .get("mimetype")
        .ok_or(ParseError::MissingEntry("mimetype"))?;
    let mimetype_str = std::str::from_utf8(mimetype)
        .map_err(|e| ParseError::NotIdml(format!("mimetype not utf-8: {e}")))?
        .trim()
        .to_string();
    // Adobe's IDML mimetype constant.
    if mimetype_str != "application/vnd.adobe.indesign-idml-package" {
        return Err(ParseError::NotIdml(format!(
            "unexpected mimetype {mimetype_str:?}"
        )));
    }

    let designmap_raw = entries
        .get("designmap.xml")
        .cloned()
        .ok_or(ParseError::MissingEntry("designmap.xml"))?;

    Ok(SourceArchive {
        mimetype: mimetype_str,
        designmap_raw,
        entries,
    })
}

impl SourceArchive {
    /// Fetch a sub-resource by archive path (e.g. "Stories/Story_u123.xml").
    pub fn entry(&self, path: &str) -> Option<&Bytes> {
        self.entries.get(path)
    }
}

/// Error assembling a [`paged_scene::Document`] from an IDML package via
/// [`import_idml`]. (Moved here with the import orchestrator — N9.)
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("manifest lists {0} but archive has no such entry")]
    MissingEntry(String),
    #[error("parse: {0}")]
    Parse(#[from] ParseError),
}

/// Import an IDML package into a [`paged_scene::Document`] + its raw
/// [`SourceArchive`]. The IDML *import orchestrator* — moved out of
/// `paged-scene` (N9) so the model no longer parses IDML: it opens the archive,
/// parses the `designmap.xml` manifest + every referenced Spread/Story/Resource
/// part, assembles the `Document` (which carries NO raw archive — that rides
/// separately), and returns both. The caller decides where the archive lives
/// (the canvas holds it for the parts door + IDML re-export).
pub fn import_idml(bytes: &[u8]) -> Result<(paged_scene::Document, SourceArchive), OpenError> {
    let archive = open_source_archive(bytes)?;
    let document = import_idml_archive(&archive)?;
    Ok((document, archive))
}

/// Convenience over [`import_idml`] for callers that only want the model and
/// don't need the raw source archive (most tests + tools). Drop-in for the old
/// `Document::open`.
pub fn import_idml_doc(bytes: &[u8]) -> Result<paged_scene::Document, OpenError> {
    import_idml(bytes).map(|(doc, _archive)| doc)
}

/// Parse a [`paged_scene::Document`] from an already-opened [`SourceArchive`]
/// (no re-unzip). The caller keeps the archive (e.g. the canvas load sniff,
/// which opened it to check for the native `document.pgm` part first).
pub fn import_idml_archive(archive: &SourceArchive) -> Result<paged_scene::Document, OpenError> {
    use std::collections::HashMap;

    // The structured manifest is parsed here (not in `open_source_archive`) so
    // the raw source archive carries no model data (N7).
    let designmap = parse_designmap(&archive.designmap_raw)?;
    let palette = match archive.entry("Resources/Graphic.xml") {
        Some(raw) => parse_graphic(raw)?,
        None => Graphic::default(),
    };
    let styles = match archive.entry("Resources/Styles.xml") {
        Some(raw) => parse_stylesheet(raw)?,
        None => StyleSheet::default(),
    };

    // Master spreads parse first so the page → master link is available
    // downstream. A `<MasterSpread>` has the same schema as a `<Spread>`.
    let mut master_spreads: HashMap<String, paged_scene::ParsedMasterSpread> = HashMap::new();
    for src in &designmap.master_spreads {
        let raw = archive
            .entry(src)
            .ok_or_else(|| OpenError::MissingEntry(src.clone()))?;
        let parsed = parse_spread(raw)?;
        let self_id = paged_scene::derive_master_id(src);
        master_spreads.insert(
            self_id.clone(),
            paged_scene::ParsedMasterSpread {
                src: src.clone(),
                self_id,
                spread: parsed,
            },
        );
    }

    let mut spreads = Vec::with_capacity(designmap.spreads.len());
    for spread_ref in &designmap.spreads {
        let raw = archive
            .entry(&spread_ref.src)
            .ok_or_else(|| OpenError::MissingEntry(spread_ref.src.clone()))?;
        let parsed = parse_spread(raw)?;
        spreads.push(paged_scene::ParsedSpread {
            src: spread_ref.src.clone(),
            spread: parsed,
        });
    }

    let mut stories = Vec::with_capacity(designmap.stories.len());
    for story_ref in &designmap.stories {
        let raw = archive
            .entry(&story_ref.src)
            .ok_or_else(|| OpenError::MissingEntry(story_ref.src.clone()))?;
        let parsed = parse_story(raw)?;
        let self_id = paged_scene::derive_story_id(&story_ref.src);
        stories.push(paged_scene::ParsedStory {
            src: story_ref.src.clone(),
            self_id,
            story: parsed,
        });
    }

    let mut document = paged_scene::Document {
        designmap,
        palette,
        spreads,
        stories,
        master_spreads,
        frame_for_story: HashMap::new(),
        text_frame_index: HashMap::new(),
        styles,
        anchors: Vec::new(),
    };
    document.rebuild_indexes();
    Ok(document)
}
