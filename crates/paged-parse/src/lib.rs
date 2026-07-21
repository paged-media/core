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
    ColorSettings, DesignMap, DocumentPreference, Hyperlink, HyperlinkDestination,
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

/// Parsed IDML container. Holds decompressed entries in memory.
///
/// The raw-entry map keeps `Bytes` so downstream crates can slice sub-
/// resources (individual `Stories/Story_*.xml` etc.) without copying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Container {
    pub mimetype: String,
    /// Raw `designmap.xml` bytes. IDML carry-through only — never part of the
    /// native model serialization (the structured `designmap` is the truth);
    /// defaults to empty on native deserialize (N1, Approach A).
    #[serde(skip)]
    pub designmap_raw: Bytes,
    pub designmap: DesignMap,
    /// Full decompressed archive contents keyed by entry path. IDML
    /// carry-through only (render-dead) — `#[serde(skip)]` so the native model
    /// never stores the raw IDML package; empty after native deserialize.
    #[serde(skip)]
    pub entries: std::collections::BTreeMap<String, Bytes>,
}

impl Container {
    /// Open an IDML container from raw bytes.
    pub fn open(bytes: &[u8]) -> Result<Self, ParseError> {
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
        let designmap = DesignMap::parse(&designmap_raw)?;

        Ok(Self {
            mimetype: mimetype_str,
            designmap_raw,
            designmap,
            entries,
        })
    }

    /// Fetch a sub-resource by archive path (e.g. "Stories/Story_u123.xml").
    pub fn entry(&self, path: &str) -> Option<&Bytes> {
        self.entries.get(path)
    }
}
