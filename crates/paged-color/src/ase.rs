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

//! Concept 2 — Adobe Swatch Exchange (`.ase`) parse + write.
//!
//! ASE is the lingua franca of swatch libraries (the freieFarbe HLC
//! atlas ships as `.ase`). The format is community-documented and
//! stable: an `ASEF` signature, a u16.u16 version, a u32 block
//! count, then blocks of three kinds — group start (`0xC001`),
//! colour entry (`0x0001`), group end (`0xC002`). Every entry
//! carries a UTF-16BE name, a four-byte colour-model tag (`"RGB "`,
//! `"CMYK"`, `"LAB "`, `"Gray"`), big-endian f32 components, and a
//! colour type (0 global, 1 spot, 2 normal/process). All integers
//! big-endian.
//!
//! Component conventions differ from IDML's: ASE stores RGB and
//! CMYK channels 0..=1 and Lab L 0..=1 (a*/b* raw −128..127); IDML
//! stores RGB 0..=255, CMYK percentages 0..=100, Lab L 0..=100.
//! [`AseEntry::value`] is normalised to the IDML convention at the
//! parse boundary (and converted back on write) so the mapping to
//! `SwatchSpec` is identity.

/// Colour space of one entry, tagged as in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AseSpace {
    Rgb,
    Cmyk,
    Lab,
    Gray,
}

/// ASE colour type: `Global` and `Process` both map to IDML
/// `Model="Process"`; `Spot` maps to `Model="Spot"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AseKind {
    Global,
    Spot,
    Process,
}

/// One colour entry, components already in IDML units.
#[derive(Debug, Clone, PartialEq)]
pub struct AseEntry {
    pub name: String,
    pub space: AseSpace,
    pub value: Vec<f32>,
    pub kind: AseKind,
}

/// One named group block and its entries.
#[derive(Debug, Clone, PartialEq)]
pub struct AseGroup {
    pub name: String,
    pub entries: Vec<AseEntry>,
}

/// A parsed library: grouped entries + any entries outside a group.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AseLibrary {
    pub groups: Vec<AseGroup>,
    pub loose: Vec<AseEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum AseError {
    #[error("not an ASE file (missing ASEF signature)")]
    BadSignature,
    #[error("truncated ASE data at offset {0}")]
    Truncated(usize),
    #[error("unknown colour model tag {0:?}")]
    UnknownModel([u8; 4]),
    #[error("malformed UTF-16 name")]
    BadName,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], AseError> {
        let end = self.pos.checked_add(n).ok_or(AseError::Truncated(self.pos))?;
        if end > self.data.len() {
            return Err(AseError::Truncated(self.pos));
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16, AseError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, AseError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, AseError> {
        Ok(f32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    /// UTF-16BE string prefixed with its length in u16 units
    /// (INCLUDING the null terminator).
    fn name(&mut self) -> Result<String, AseError> {
        let units = self.u16()? as usize;
        let raw = self.take(units * 2)?;
        let mut points = Vec::with_capacity(units.saturating_sub(1));
        for ch in raw.chunks_exact(2) {
            points.push(u16::from_be_bytes([ch[0], ch[1]]));
        }
        // Strip the trailing null (and tolerate its absence).
        if points.last() == Some(&0) {
            points.pop();
        }
        String::from_utf16(&points).map_err(|_| AseError::BadName)
    }
}

const SIG: &[u8; 4] = b"ASEF";
const BLOCK_GROUP_START: u16 = 0xC001;
const BLOCK_GROUP_END: u16 = 0xC002;
const BLOCK_COLOR: u16 = 0x0001;

/// Parse `.ase` bytes. Unknown block types are skipped (the format
/// reserves room for extensions); unknown colour models fail loudly.
pub fn parse_ase(bytes: &[u8]) -> Result<AseLibrary, AseError> {
    let mut r = Reader { data: bytes, pos: 0 };
    if r.take(4)? != SIG {
        return Err(AseError::BadSignature);
    }
    let _ver_major = r.u16()?;
    let _ver_minor = r.u16()?;
    let block_count = r.u32()?;

    let mut lib = AseLibrary::default();
    let mut open_group: Option<AseGroup> = None;
    for _ in 0..block_count {
        let block_type = r.u16()?;
        let block_len = r.u32()? as usize;
        let block_end = r
            .pos
            .checked_add(block_len)
            .ok_or(AseError::Truncated(r.pos))?;
        match block_type {
            BLOCK_GROUP_START => {
                // An unterminated previous group closes implicitly.
                if let Some(g) = open_group.take() {
                    lib.groups.push(g);
                }
                open_group = Some(AseGroup {
                    name: r.name()?,
                    entries: Vec::new(),
                });
            }
            BLOCK_GROUP_END => {
                if let Some(g) = open_group.take() {
                    lib.groups.push(g);
                }
            }
            BLOCK_COLOR => {
                let name = r.name()?;
                let tag: [u8; 4] = r.take(4)?.try_into().unwrap();
                let (space, n) = match &tag {
                    b"RGB " => (AseSpace::Rgb, 3),
                    b"CMYK" => (AseSpace::Cmyk, 4),
                    b"LAB " | b"Lab " => (AseSpace::Lab, 3),
                    b"Gray" | b"GRAY" => (AseSpace::Gray, 1),
                    _ => return Err(AseError::UnknownModel(tag)),
                };
                let mut raw = Vec::with_capacity(n);
                for _ in 0..n {
                    raw.push(r.f32()?);
                }
                let kind = match r.u16()? {
                    1 => AseKind::Spot,
                    2 => AseKind::Process,
                    _ => AseKind::Global,
                };
                let entry = AseEntry {
                    name,
                    space,
                    value: to_idml_units(space, &raw),
                    kind,
                };
                match open_group.as_mut() {
                    Some(g) => g.entries.push(entry),
                    None => lib.loose.push(entry),
                }
            }
            _ => {
                // Skip unknown block payloads.
            }
        }
        // Re-sync to the declared block boundary — defensive against
        // writers that pad and against our own partial reads of
        // skipped blocks.
        if block_end > bytes.len() {
            return Err(AseError::Truncated(r.pos));
        }
        r.pos = block_end;
    }
    if let Some(g) = open_group.take() {
        lib.groups.push(g);
    }
    Ok(lib)
}

/// Serialise a library back to `.ase` bytes (the Swatches panel's
/// "Save .ase…"). Round-trips everything `parse_ase` reads.
pub fn write_ase(lib: &AseLibrary) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(SIG);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    let block_count: u32 = lib
        .groups
        .iter()
        .map(|g| g.entries.len() as u32 + 2)
        .sum::<u32>()
        + lib.loose.len() as u32;
    out.extend_from_slice(&block_count.to_be_bytes());

    let write_name = |buf: &mut Vec<u8>, name: &str| {
        let units: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        buf.extend_from_slice(&(units.len() as u16).to_be_bytes());
        for u in units {
            buf.extend_from_slice(&u.to_be_bytes());
        }
    };
    let write_color = |out: &mut Vec<u8>, e: &AseEntry| {
        let mut body = Vec::new();
        write_name(&mut body, &e.name);
        let (tag, _) = match e.space {
            AseSpace::Rgb => (*b"RGB ", 3),
            AseSpace::Cmyk => (*b"CMYK", 4),
            AseSpace::Lab => (*b"LAB ", 3),
            AseSpace::Gray => (*b"Gray", 1),
        };
        body.extend_from_slice(&tag);
        for v in from_idml_units(e.space, &e.value) {
            body.extend_from_slice(&v.to_be_bytes());
        }
        body.extend_from_slice(
            &match e.kind {
                AseKind::Global => 0u16,
                AseKind::Spot => 1u16,
                AseKind::Process => 2u16,
            }
            .to_be_bytes(),
        );
        out.extend_from_slice(&BLOCK_COLOR.to_be_bytes());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
    };

    for g in &lib.groups {
        let mut body = Vec::new();
        write_name(&mut body, &g.name);
        out.extend_from_slice(&BLOCK_GROUP_START.to_be_bytes());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        for e in &g.entries {
            write_color(&mut out, e);
        }
        out.extend_from_slice(&BLOCK_GROUP_END.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
    }
    for e in &lib.loose {
        write_color(&mut out, e);
    }
    out
}

/// ASE component conventions → IDML's (RGB 0..=255, CMYK 0..=100,
/// Lab L 0..=100 with raw a*/b*, Gray as ink percentage 0..=100).
/// ASE Gray stores LIGHTNESS 0..=1; IDML Gray stores INK coverage —
/// inverted on both trips.
fn to_idml_units(space: AseSpace, raw: &[f32]) -> Vec<f32> {
    match space {
        AseSpace::Rgb => raw.iter().map(|v| v * 255.0).collect(),
        AseSpace::Cmyk => raw.iter().map(|v| v * 100.0).collect(),
        AseSpace::Lab => {
            let mut v = raw.to_vec();
            if let Some(l) = v.first_mut() {
                *l *= 100.0;
            }
            v
        }
        AseSpace::Gray => raw.iter().map(|v| (1.0 - v) * 100.0).collect(),
    }
}

fn from_idml_units(space: AseSpace, idml: &[f32]) -> Vec<f32> {
    match space {
        AseSpace::Rgb => idml.iter().map(|v| v / 255.0).collect(),
        AseSpace::Cmyk => idml.iter().map(|v| v / 100.0).collect(),
        AseSpace::Lab => {
            let mut v = idml.to_vec();
            if let Some(l) = v.first_mut() {
                *l /= 100.0;
            }
            v
        }
        AseSpace::Gray => idml.iter().map(|v| 1.0 - v / 100.0).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hlc_like_library() -> AseLibrary {
        AseLibrary {
            groups: vec![AseGroup {
                name: "HLC Colour Atlas".into(),
                entries: vec![
                    AseEntry {
                        name: "HLC H010_L20_C010".into(),
                        space: AseSpace::Lab,
                        value: vec![20.0, 9.848, 1.736],
                        kind: AseKind::Global,
                    },
                    AseEntry {
                        name: "HLC H350_L85_C025".into(),
                        space: AseSpace::Lab,
                        value: vec![85.0, 24.62, -4.341],
                        kind: AseKind::Global,
                    },
                ],
            }],
            loose: vec![
                AseEntry {
                    name: "Warm Red".into(),
                    space: AseSpace::Cmyk,
                    value: vec![0.0, 80.0, 95.0, 0.0],
                    kind: AseKind::Process,
                },
                AseEntry {
                    name: "PANTONE-ish".into(),
                    space: AseSpace::Rgb,
                    value: vec![230.0, 30.0, 80.0],
                    kind: AseKind::Spot,
                },
                AseEntry {
                    name: "Ink Grey".into(),
                    space: AseSpace::Gray,
                    value: vec![40.0],
                    kind: AseKind::Global,
                },
            ],
        }
    }

    #[test]
    fn write_then_parse_round_trips() {
        let lib = hlc_like_library();
        let bytes = write_ase(&lib);
        let parsed = parse_ase(&bytes).expect("parse");
        // Float trips are exact for these values (BE f32 either way,
        // scale factors are powers of 10 applied symmetrically) —
        // compare structurally with a small epsilon.
        assert_eq!(parsed.groups.len(), 1);
        assert_eq!(parsed.groups[0].name, lib.groups[0].name);
        assert_eq!(parsed.loose.len(), 3);
        for (a, b) in parsed.groups[0]
            .entries
            .iter()
            .chain(parsed.loose.iter())
            .zip(lib.groups[0].entries.iter().chain(lib.loose.iter()))
        {
            assert_eq!(a.name, b.name);
            assert_eq!(a.space, b.space);
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.value.len(), b.value.len());
            for (x, y) in a.value.iter().zip(b.value.iter()) {
                assert!((x - y).abs() < 1e-3, "{} vs {}", x, y);
            }
        }
    }

    #[test]
    fn hlc_lab_entry_lands_in_idml_units() {
        let bytes = write_ase(&hlc_like_library());
        let parsed = parse_ase(&bytes).unwrap();
        let hlc = &parsed.groups[0].entries[0];
        // L back in 0..=100 (the ASE file stores 0.20).
        assert!((hlc.value[0] - 20.0).abs() < 1e-3);
        assert!((hlc.value[1] - 9.848).abs() < 1e-3);
        assert_eq!(hlc.space, AseSpace::Lab);
    }

    #[test]
    fn signature_is_enforced() {
        assert!(matches!(parse_ase(b"NOPE"), Err(AseError::BadSignature)));
        assert!(matches!(parse_ase(b"AS"), Err(AseError::Truncated(_))));
    }

    #[test]
    fn unknown_blocks_are_skipped() {
        // Hand-build: header with 2 blocks — one unknown type, one
        // colour. The unknown block's payload must not derail the
        // reader.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"ASEF");
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&2u32.to_be_bytes());
        // Unknown block (type 0xBEEF, 4 junk bytes).
        bytes.extend_from_slice(&0xBEEFu16.to_be_bytes());
        bytes.extend_from_slice(&4u32.to_be_bytes());
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        // One gray colour, loose.
        let lib = AseLibrary {
            groups: vec![],
            loose: vec![AseEntry {
                name: "G".into(),
                space: AseSpace::Gray,
                value: vec![25.0],
                kind: AseKind::Global,
            }],
        };
        let one = write_ase(&lib);
        bytes.extend_from_slice(&one[12..]); // strip its header
        let parsed = parse_ase(&bytes).unwrap();
        assert_eq!(parsed.loose.len(), 1);
        assert_eq!(parsed.loose[0].name, "G");
    }
}
