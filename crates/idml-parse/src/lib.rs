//! IDML parser.
//!
//! Consumes an IDML ZIP archive and produces a typed AST. Schema coverage
//! is driven by the reference-reading week described in the development
//! plan (Scribus `importidmlplugin.cpp`, SimpleIDML, Adobe's IDML spec).
//!
//! The current surface is intentionally thin: it opens the container,
//! confirms the mimetype, locates the root `designmap.xml`, and exposes a
//! streaming reader the higher layers can pull from. Typed scene
//! extraction lives in `idml-scene`; this crate stays focused on ZIP+XML
//! plumbing.

use std::io::{self, Cursor, Read};

use bytes::Bytes;

pub mod designmap;
pub mod graphic;
pub mod spread;
pub mod story;
pub mod styles;
mod util;

pub use designmap::{ColorSettings, DesignMap, Layer, SpreadRef, StoryRef, TextVariable};
pub use graphic::{
    ColorEntry, ColorModel, ColorSpace, GradientEntry, GradientKind, GradientStopRef, Graphic,
    SwatchEntry,
};
pub use spread::{
    AutoSizingReferencePoint, AutoSizingType, Bounds, CornerOption, CornerSpec,
    DirectionalFeatherParams, DropShadowSetting, FirstBaselineOffset, FrameEffects,
    FrameFittingOption, FrameRef, GradientFeatherParams, GradientFeatherStop, GraphicLine, Group,
    GroupTransparency, Oval, Page, PathAnchor, Polygon, Rectangle, Spread, TextFrame, TextPath,
    TextWrap, TextWrapMode, VerticalJustification,
};
pub use story::{
    AnchoredFrame, AnchoredFrameKind, AnchoredObjectSetting, CellDiagonal, CharacterRun,
    Justification, Paragraph, Story, TabStop, Table, TableBorder, TableCell, TableColumn,
    TableLineStrokes, TableRow, AUTO_PAGE_NUMBER_MARKER, NEXT_PAGE_NUMBER_MARKER,
};
pub use styles::{
    CellStyleDef, CharacterStyleDef, ObjectStyleDef, ParagraphRule, ParagraphShading,
    ParagraphStyleDef, ResolvedCell, ResolvedCharacter, ResolvedObject, ResolvedParagraph,
    ResolvedTable, StyleSheet, TOCStyleDef, TOCStyleEntryDef, TableStyleDef,
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
#[derive(Debug, Clone)]
pub struct Container {
    pub mimetype: String,
    pub designmap_raw: Bytes,
    pub designmap: DesignMap,
    /// Full decompressed archive contents keyed by entry path.
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
