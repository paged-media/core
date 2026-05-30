//! UCF/Zip package writer.
//!
//! IDML's container is a Zip archive with two strict requirements:
//!
//! * `mimetype` is the first entry, **stored** (no compression), no
//!   extra fields — readers can identify the file type from offset 30
//!   without parsing further.
//! * Every entry's path uses forward slashes; case sensitivity must
//!   match across platforms.
//!
//! On top of those constraints the writer enforces determinism:
//!
//! * Fixed timestamp (1980-01-01 00:00:00, the Zip epoch).
//! * Stable entry order, mimicking the order Sample-3.idml emits.
//! * No extra fields beyond what `zip` adds for stored entries.
//!
//! Two consecutive emissions of the same `Sample` must produce
//! byte-identical archives — the `tests/snapshot.rs` SHA-256 check
//! gates this.

use anyhow::{Context, Result};
use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

const IDML_MIMETYPE: &[u8] = b"application/vnd.adobe.indesign-idml-package";

fn fixed_timestamp() -> DateTime {
    DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).expect("Zip epoch is a valid timestamp")
}

/// One emitted IDML mega-file. The package writer takes the bag of
/// per-resource bytes the builders produced and stitches them into a
/// well-formed UCF archive.
pub struct Sample {
    pub container_xml: Vec<u8>,
    pub designmap_xml: Vec<u8>,
    pub graphic_xml: Vec<u8>,
    pub fonts_xml: Vec<u8>,
    pub styles_xml: Vec<u8>,
    pub preferences_xml: Vec<u8>,
    pub backing_story_xml: Vec<u8>,
    pub tags_xml: Vec<u8>,
    pub mapping_xml: Vec<u8>,
    /// `(MasterSpread/<id>, bytes)` — usually one master per page.
    pub master_spreads: Vec<(String, Vec<u8>)>,
    /// `(Spread/<id>, bytes)`.
    pub spreads: Vec<(String, Vec<u8>)>,
    /// `(Story/<id>, bytes)`.
    pub stories: Vec<(String, Vec<u8>)>,
}

/// Serialise a `Sample` to an in-memory `.idml` byte vector.
///
/// Entry order matches Sample-3.idml's observed layout:
/// `mimetype`, `designmap.xml`, `META-INF/container.xml`,
/// `Resources/{Graphic,Fonts,Styles,Preferences}.xml`,
/// `XML/{BackingStory,Tags,Mapping}.xml`, `MasterSpreads/*`,
/// `Spreads/*`, `Stories/*`.
pub fn write_idml(sample: &Sample) -> Result<Vec<u8>> {
    let buf = Cursor::new(Vec::<u8>::new());
    let mut zip = ZipWriter::new(buf);

    // mimetype: stored, first, no extras. The trailing newline is
    // omitted on purpose — Sample-3.idml's mimetype entry is exactly
    // 43 bytes (the mimetype string itself).
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(fixed_timestamp());
    zip.start_file("mimetype", stored)
        .context("start mimetype entry")?;
    zip.write_all(IDML_MIMETYPE)?;

    let deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(fixed_timestamp());

    let mut emit = |path: &str, body: &[u8]| -> Result<()> {
        zip.start_file(path, deflated).context("start file")?;
        zip.write_all(body)?;
        Ok(())
    };

    emit("designmap.xml", &sample.designmap_xml)?;
    emit("META-INF/container.xml", &sample.container_xml)?;
    emit("Resources/Graphic.xml", &sample.graphic_xml)?;
    emit("Resources/Fonts.xml", &sample.fonts_xml)?;
    emit("Resources/Styles.xml", &sample.styles_xml)?;
    emit("Resources/Preferences.xml", &sample.preferences_xml)?;
    emit("XML/BackingStory.xml", &sample.backing_story_xml)?;
    emit("XML/Tags.xml", &sample.tags_xml)?;
    emit("XML/Mapping.xml", &sample.mapping_xml)?;

    for (id, body) in &sample.master_spreads {
        emit(&format!("MasterSpreads/MasterSpread_{id}.xml"), body)?;
    }
    for (id, body) in &sample.spreads {
        emit(&format!("Spreads/Spread_{id}.xml"), body)?;
    }
    for (id, body) in &sample.stories {
        emit(&format!("Stories/Story_{id}.xml"), body)?;
    }

    let buf = zip.finish().context("finish zip")?;
    Ok(buf.into_inner())
}
