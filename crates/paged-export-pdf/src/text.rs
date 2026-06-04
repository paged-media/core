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

//! Text-as-text: glyph runs from the side-channel become real PDF
//! text (`BT/Tf/Tm/TJ/ET`) over subset, CID-keyed Type0 fonts with
//! `/ToUnicode` CMaps — selectable, searchable, accessible. Fonts
//! whose fsType forbids embedding stay as outlines + a diagnostic
//! (concept E8: surfaced, never silent).

use std::collections::BTreeMap;

use pdf_writer::{Content, Finish, Name, Pdf, Ref, Str};

use crate::writer::RefAllocator;
use crate::{ExportDiagnostic, ExportError, ExportInput, RestrictedFontPolicy};

/// Per-document font usage, accumulated across pages; subsets are
/// written at finish time.
#[derive(Default)]
pub struct FontPool {
    /// font_id → (pre-allocated Type0 ref, used glyph ids,
    /// glyph→unicode, embeddable).
    pub fonts: BTreeMap<u32, FontUsage>,
}

pub struct FontUsage {
    pub type0_ref: Ref,
    /// Old→new gid mapping, assigned INCREMENTALLY at use time so
    /// the content stream can write the remapped (subset) gid
    /// directly; the SAME remapper drives `subsetter::subset` at
    /// finish, so the two agree by construction.
    pub remapper: subsetter::GlyphRemapper,
    /// new gid → unicode (for /ToUnicode).
    pub to_unicode: BTreeMap<u16, char>,
    pub embeddable: Option<bool>,
    pub outlined_runs: usize,
}

impl FontPool {
    /// Record one glyph's use; returns the page-resource font name,
    /// the (stable, pre-allocated) Type0 ref, and the REMAPPED gid
    /// to show in the content stream.
    pub fn use_glyph(
        &mut self,
        refs: &mut RefAllocator,
        font_id: u32,
        glyph_id: u32,
        unicode: Option<char>,
    ) -> (String, Ref, u16) {
        let usage = self.fonts.entry(font_id).or_insert_with(|| FontUsage {
            type0_ref: refs.alloc(),
            remapper: subsetter::GlyphRemapper::new(),
            to_unicode: BTreeMap::new(),
            embeddable: None,
            outlined_runs: 0,
        });
        let new_gid = usage.remapper.remap(glyph_id as u16);
        if let Some(u) = unicode {
            usage.to_unicode.entry(new_gid).or_insert(u);
        }
        (format!("F{font_id}"), usage.type0_ref, new_gid)
    }

    /// fsType gate, memoised per font. `true` = embeddable.
    pub fn check_embeddable(&mut self, font_id: u32, face_bytes: &[u8]) -> bool {
        if let Some(usage) = self.fonts.get(&font_id) {
            if let Some(known) = usage.embeddable {
                return known;
            }
        }
        let embeddable = fs_type_allows_embedding(face_bytes);
        if let Some(usage) = self.fonts.get_mut(&font_id) {
            usage.embeddable = Some(embeddable);
        }
        embeddable
    }
}

/// OS/2 fsType bit 0x0002 = "restricted licence embedding" (the
/// only combination that forbids embedding outright when no other
/// permissive bit is set).
fn fs_type_allows_embedding(face_bytes: &[u8]) -> bool {
    let Ok(face) = ttf_parser::Face::parse(face_bytes, 0) else {
        return false;
    };
    match face.tables().os2 {
        Some(os2) => {
            let permissions = os2.permissions();
            !matches!(permissions, Some(ttf_parser::Permissions::Restricted))
        }
        None => true,
    }
}

/// Emit one consecutive glyph slice as a text object. The entries
/// share (font_id, font_size, paint) — the caller groups them.
///
/// Exactness contract: the captured outline affine maps FONT-UNIT
/// outline points to the page (`P · [sx 0; 0 -sy] + (gx, gy)`, with
/// `sx = size·x_scale/upem`). PDF text maps EM-unit glyph space
/// through `Tf size` then `Tm`, i.e. `P/upem · size · Tm`. Equality
/// ⇒ `Tm_linear = transform_linear · upem / size`,
/// `Tm_t = (gx, gy)` — the glyph lands pixel-identical to the
/// outline it replaces, REMAPPED to the subset gid at finish.
pub fn emit_text_slice(
    content: &mut Content,
    font_name: &str,
    font_size: f32,
    units_per_em: f32,
    entries: &[(&paged_compose::GlyphRunEntry, u16)],
) {
    content.begin_text();
    content.set_font(Name(font_name.as_bytes()), font_size);
    let k = units_per_em / font_size.max(1e-6);
    for (e, new_gid) in entries {
        let t = e.transform.0;
        content.set_text_matrix([t[0] * k, t[1] * k, t[2] * k, t[3] * k, t[4], t[5]]);
        content.show(Str(&new_gid.to_be_bytes()));
    }
    content.end_text();
}

/// Subset + write every used font. Returns name → Type0 ref (refs
/// were pre-allocated at use time, so page resources already point
/// at the right objects).
pub fn write_fonts(
    pdf: &mut Pdf,
    refs: &mut RefAllocator,
    pool: &mut FontPool,
    input: &ExportInput<'_>,
    diagnostics: &mut Vec<ExportDiagnostic>,
) -> Result<BTreeMap<String, Ref>, ExportError> {
    let mut out = BTreeMap::new();
    let Some(fonts) = input.fonts else {
        return Ok(out);
    };
    for (font_id, usage) in &pool.fonts {
        let name = format!("F{font_id}");
        let Some(face_bytes) = fonts.font_bytes(*font_id) else {
            diagnostics.push(ExportDiagnostic::FontNotEmbeddable {
                font_id: *font_id,
                runs_outlined: usage.remapper.num_gids() as usize,
            });
            continue;
        };
        if usage.embeddable == Some(false) {
            match input.options.restricted_fonts {
                RestrictedFontPolicy::Fail => {
                    return Err(ExportError::FontNotEmbeddable { font_id: *font_id })
                }
                RestrictedFontPolicy::Outline => {
                    diagnostics.push(ExportDiagnostic::FontNotEmbeddable {
                        font_id: *font_id,
                        runs_outlined: usage.outlined_runs,
                    });
                    continue;
                }
            }
        }

        // Subset with the SAME remapper the content streams used.
        let glyph_remapper = &usage.remapper;
        let subset = subsetter::subset(face_bytes, 0, glyph_remapper)
            .map_err(|e| ExportError::Subset(format!("{e:?}")))?;
        let is_cff = {
            let face = ttf_parser::Face::parse(face_bytes, 0).ok();
            face.map(|f| f.tables().cff.is_some() || f.raw_face().table(ttf_parser::Tag::from_bytes(b"CFF2")).is_some())
                .unwrap_or(false)
        };

        // Metrics for the CIDFont (from the ORIGINAL face; ids are
        // remapped in the subset, widths must be in subset-gid
        // space).
        let face = ttf_parser::Face::parse(face_bytes, 0)
            .map_err(|e| ExportError::Subset(format!("face parse: {e:?}")))?;
        let units = face.units_per_em() as f32;
        let scale = 1000.0 / units;

        let old_gids: Vec<u16> = glyph_remapper.remapped_gids().collect();
        let base_name = format!("{}+PAGED{}", subset_tag(&old_gids), font_id);

        // Font program stream.
        let file_ref = refs.alloc();
        {
            let data = subset.as_ref();
            let compressed = {
                use std::io::Write as _;
                let mut enc = flate2::write::ZlibEncoder::new(
                    Vec::new(),
                    flate2::Compression::default(),
                );
                let _ = enc.write_all(data);
                enc.finish().unwrap_or_default()
            };
            let mut stream = pdf.stream(file_ref, &compressed);
            stream.filter(pdf_writer::Filter::FlateDecode);
            if is_cff {
                stream.pair(Name(b"Subtype"), Name(b"OpenType"));
            } else {
                stream.pair(Name(b"Length1"), data.len() as i32);
            }
            stream.finish();
        }

        // FontDescriptor.
        let desc_ref = refs.alloc();
        {
            let bbox = face.global_bounding_box();
            let mut d = pdf.font_descriptor(desc_ref);
            d.name(Name(base_name.as_bytes()));
            d.flags(pdf_writer::types::FontFlags::SYMBOLIC);
            d.bbox(pdf_writer::Rect::new(
                bbox.x_min as f32 * scale,
                bbox.y_min as f32 * scale,
                bbox.x_max as f32 * scale,
                bbox.y_max as f32 * scale,
            ));
            d.italic_angle(face.italic_angle());
            d.ascent(face.ascender() as f32 * scale);
            d.descent(face.descender() as f32 * scale);
            d.cap_height(face.capital_height().unwrap_or(face.ascender()) as f32 * scale);
            d.stem_v(95.0);
            if is_cff {
                d.font_file3(file_ref);
            } else {
                d.font_file2(file_ref);
            }
            d.finish();
        }

        // CIDFont (Type0 descendant). CIDs are the REMAPPED subset
        // gids; widths from the original gids.
        let cid_ref = refs.alloc();
        {
            let mut cid = pdf.cid_font(cid_ref);
            cid.subtype(if is_cff {
                pdf_writer::types::CidFontType::Type0
            } else {
                pdf_writer::types::CidFontType::Type2
            });
            cid.base_font(Name(base_name.as_bytes()));
            cid.system_info(pdf_writer::types::SystemInfo {
                registry: Str(b"Adobe"),
                ordering: Str(b"Identity"),
                supplement: 0,
            });
            cid.font_descriptor(desc_ref);
            if !is_cff {
                cid.cid_to_gid_map_predefined(Name(b"Identity"));
            }
            cid.default_width(face
                .glyph_hor_advance(ttf_parser::GlyphId(0))
                .map(|a| a as f32 * scale)
                .unwrap_or(500.0));
            // `remapped_gids()` yields old ids in NEW-gid order
            // (0, 1, 2, …) — one consecutive widths run.
            let advances: Vec<f32> = old_gids
                .iter()
                .map(|old| {
                    face.glyph_hor_advance(ttf_parser::GlyphId(*old))
                        .map(|a| a as f32 * scale)
                        .unwrap_or(0.0)
                })
                .collect();
            let mut widths = cid.widths();
            widths.consecutive(0, advances.iter().copied());
            widths.finish();
            cid.finish();
        }

        // ToUnicode CMap (remapped gid → unicode).
        let tounicode_ref = refs.alloc();
        {
            let mut cmap = String::new();
            cmap.push_str(
                "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
            );
            let pairs: Vec<(u16, char)> = usage
                .to_unicode
                .iter()
                .map(|(gid, ch)| (*gid, *ch))
                .collect();
            cmap.push_str(&format!("{} beginbfchar\n", pairs.len()));
            for (gid, ch) in pairs {
                let mut buf = [0u16; 2];
                let encoded = ch.encode_utf16(&mut buf);
                let hex: String =
                    encoded.iter().map(|u| format!("{u:04X}")).collect();
                cmap.push_str(&format!("<{gid:04X}> <{hex}>\n"));
            }
            cmap.push_str("endbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
            let stream = pdf.stream(tounicode_ref, cmap.as_bytes());
            stream.finish();
        }

        // Type0 root at the PRE-ALLOCATED ref.
        {
            let mut t0 = pdf.type0_font(usage.type0_ref);
            t0.base_font(Name(base_name.as_bytes()));
            t0.encoding_predefined(Name(b"Identity-H"));
            t0.descendant_font(cid_ref);
            t0.to_unicode(tounicode_ref);
            t0.finish();
        }
        out.insert(name, usage.type0_ref);
    }
    Ok(out)
}

/// The conventional 6-letter subset tag, derived deterministically
/// from the glyph set (NOT random — determinism).
fn subset_tag(gids: &[u16]) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for g in gids {
        h ^= *g as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    let mut tag = String::with_capacity(6);
    let mut v = h;
    for _ in 0..6 {
        tag.push(char::from(b'A' + (v % 26) as u8));
        v /= 26;
    }
    tag
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_glyph_remaps_in_first_use_order() {
        let mut pool = FontPool::default();
        let mut refs = crate::writer::RefAllocator::new();
        // .notdef is pinned at 0, so the first real glyph gets 1,
        // the second 2, and a REPEAT of the first stays 1.
        let (_, _, a) = pool.use_glyph(&mut refs, 7, 42, Some('A'));
        let (_, _, b) = pool.use_glyph(&mut refs, 7, 17, Some('B'));
        let (_, _, a2) = pool.use_glyph(&mut refs, 7, 42, Some('A'));
        assert_eq!((a, b, a2), (1, 2, 1));
        // ToUnicode keys by the REMAPPED gid.
        let usage = pool.fonts.get(&7).unwrap();
        assert_eq!(usage.to_unicode.get(&1), Some(&'A'));
        assert_eq!(usage.to_unicode.get(&2), Some(&'B'));
        // Same font ⇒ same resource name + ref; other font differs.
        let (name_a, ref_a, _) = pool.use_glyph(&mut refs, 7, 99, None);
        let (name_b, ref_b, _) = pool.use_glyph(&mut refs, 8, 99, None);
        assert_eq!(name_a, "F7");
        assert_eq!(name_b, "F8");
        assert_ne!(ref_a, ref_b);
    }

    #[test]
    fn text_matrix_is_exact_for_the_glyph_affine() {
        // Contract: Tm = glyph_transform_linear × (upem / font_size),
        // translation untouched — so Tf(font_size) × Tm lands glyphs
        // at IDENTICAL page coordinates to the outline path fill.
        let mut content = Content::new();
        let entry = paged_compose::GlyphRunEntry {
            command_index: 0,
            font_id: 1,
            glyph_id: 5,
            font_size: 12.0,
            // sx = 12/1000 (point_size/upem), y negated, at (100, 200).
            transform: paged_compose::Transform([0.012, 0.0, 0.0, -0.012, 100.0, 200.0]),
            paint: paged_compose::Paint::Solid(paged_compose::Color::BLACK),
            unicode: Some('x'),
            is_stroke: false,
        };
        emit_text_slice(&mut content, "F1", 12.0, 1000.0, &[(&entry, 1)]);
        let ops = String::from_utf8(content.finish().to_vec()).unwrap();
        // 0.012 × (1000/12) = 1 exactly.
        assert!(ops.contains("BT"), "{ops}");
        assert!(ops.contains("/F1 12 Tf"), "{ops}");
        assert!(ops.contains("1 0 0 -1 100 200 Tm"), "{ops}");
        // pdf-writer encodes the 2-byte CID as a literal string.
        assert!(ops.contains("(\\000\\001) Tj"), "{ops}");
    }

    #[test]
    fn subset_tag_is_deterministic_and_wellformed() {
        let a = subset_tag(&[0, 1, 2, 3]);
        let b = subset_tag(&[0, 1, 2, 3]);
        let c = subset_tag(&[0, 1, 2, 4]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 6);
        assert!(a.chars().all(|ch| ch.is_ascii_uppercase()));
    }
}
