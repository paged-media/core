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

//! Font table + glyph-metrics parsing (FontTable, FontMetrics). Extracted from pipeline/mod.rs (1.6b).

use super::*;
use std::collections::HashMap;

use bytes::Bytes;
use paged_scene::Document;



/// Per-render font cache. Pre-resolves every distinct (family, style)
/// pair referenced anywhere in the document via the configured
/// `AssetResolver`. Falls back to `options.font` when nothing
/// resolves. Also extracts OS/2 / hhea metrics per font_id at
/// build time so baseline math doesn't have to re-parse the font
/// per paragraph.
///
/// Field declaration order matters: `faces` is declared FIRST so on
/// drop it is dropped FIRST — before `face_bytes`. The cached
/// `rustybuzz::Face<'static>` values borrow from the `Bytes` stored
/// in `face_bytes`; if we dropped `face_bytes` first the Faces would
/// briefly hold dangling references. Rust drops struct fields in
/// declaration order (first declared = first dropped), so keeping
/// `faces` above `face_bytes` is load-bearing for soundness.
/// Per-render shaping resource: the resolved bytes + configured
/// rustybuzz Faces for every (family, style, wght) referenced by the
/// document. Built once per `build_document` call by default; the
/// caller can pre-build it (via `FontTable::build`) and pass it
/// through `PipelineOptions::pre_built_font_table` to amortise the
/// ~225ms harvest-and-resolve cost across gesture rebuilds on
/// image-heavy fixtures. See the SAFETY contract on `faces` below
/// — the struct can be moved + held by a long-lived caller (e.g.
/// `CanvasModel`) without invalidating the Face references.
pub struct FontTable {
    /// Pre-configured rustybuzz `Face` cache keyed by
    /// `(font_id, wght_bits)`. The `Face<'static>` lifetime is a
    /// LIE narrowed back to `&self` at the public accessor.
    ///
    /// SAFETY contract (also enforced at each insertion site):
    ///   1. Each cached Face borrows from the `Bytes` stored under
    ///      the matching `font_id` key in `face_bytes`. `bytes::Bytes`
    ///      is refcounted with a stable heap pointer — the underlying
    ///      buffer cannot move while any clone is alive.
    ///   2. `face_bytes` is never removed-from or overwritten after
    ///      `FontTable::build` returns: it is populated inside
    ///      `build` and never touched by any later method. Therefore
    ///      the buffer a cached Face borrows from outlives that Face.
    ///   3. `faces` is declared before `face_bytes`, so on `Drop` the
    ///      Faces are dropped first — they never see a freed `Bytes`.
    ///   4. The accessor [`Self::face`] returns `&rustybuzz::Face<'_>`
    ///      with the lifetime narrowed to `&self`. No caller ever
    ///      observes the `'static` lifetime, so the lie can't escape.
    ///   5. Variations are baked in at insert time. The cached Face
    ///      is never mutated post-insert (no `&mut Face` is ever
    ///      exposed). Two runs with the same bytes but different
    ///      `wght` use distinct cache keys → distinct cached Faces.
    pub(super) faces: HashMap<(u32, u32), rustybuzz::Face<'static>>,
    /// Bytes kept alive for `faces` to point into. One entry per
    /// distinct `font_id` (the wght variant is irrelevant — same
    /// buffer, just different variation state on the Face).
    ///
    /// Marked `dead_code`-allow: the field is never read after
    /// `build`, but its EXISTENCE is load-bearing — drop-time
    /// soundness of `faces` (which holds `Face<'static>` references
    /// into these buffers) depends on this map keeping the `Bytes`
    /// values alive for at least as long as `faces`. See the SAFETY
    /// contract on `faces` above.
    #[allow(dead_code)]
    pub(super) face_bytes: HashMap<u32, Bytes>,
    pub(super) cache: HashMap<(String, Option<String>), Bytes>,
    pub(super) fallback: Option<Bytes>,
    /// Metrics keyed by `fnv_1a_u32(bytes)` (same id the rest of
    /// the pipeline uses for glyph-cache routing).
    pub(super) metrics: HashMap<u32, FontMetrics>,
    /// Per-IDML-family metric override. Populated from
    /// `PipelineOptions::font_metrics_overrides` and consulted FIRST
    /// by `metrics_for_family` so a substitute font doesn't force its
    /// own ascender / cap-height onto baseline math when the IDML
    /// names a different family. Empty when no overrides were set.
    pub(super) family_metrics: HashMap<String, FontMetrics>,
}

/// Per-font metrics the renderer reads at baseline-placement time.
/// All values are scale-free (unit = font units / `units_per_em`)
/// so callers can multiply by `point_size` to get pt.
#[derive(Debug, Clone, Copy)]
pub(super) struct FontMetrics {
    /// `OS/2.sCapHeight`, fraction of em. `None` when the font
    /// doesn't expose it (legacy fonts without the OS/2 v2+ field).
    pub(super) cap_height: Option<f32>,
    /// `OS/2.sxHeight`, fraction of em.
    pub(super) x_height: Option<f32>,
    /// `hhea.ascender`, fraction of em. Always present.
    pub(super) ascender: f32,
    /// `hhea.descender`, fraction of em, stored as a POSITIVE distance
    /// below the baseline (ttf-parser returns a negative value; we flip
    /// the sign at parse time). Used by the anchored `TopOfLeading`
    /// vertical reference point to split a line's leading into its
    /// above- and below-baseline portions in the font's own
    /// ascent:descent proportion (InDesign's leading model).
    pub(super) descender: f32,
}

impl FontTable {
    /// Concept 3 (PDF export) — the ORIGINAL face bytes for a
    /// font-table id, for subsetting/embedding. `None` for unknown
    /// ids.
    pub fn face_data(&self, font_id: u32) -> Option<&[u8]> {
        self.face_bytes.get(&font_id).map(|b| b.as_ref())
    }

    pub fn build(document: &Document, options: &PipelineOptions) -> Self {
        let fallback = options.font.map(Bytes::copy_from_slice);
        let mut cache: HashMap<(String, Option<String>), Bytes> = HashMap::new();
        if let Some(resolver) = options.assets {
            // Walk every run in every story and collect distinct
            // keys before calling the resolver — `resolve_font`
            // may be a JS Promise wrapper or a disk read, so
            // deduping matters. Each run's effective (family,
            // style) comes from the cascade (run direct > applied
            // character style > applied paragraph style) so a run
            // that only carries `AppliedCharacterStyle` still
            // requests the right font.
            let mut keys: std::collections::HashSet<(String, Option<String>)> =
                std::collections::HashSet::new();
            // Helper: harvest font keys from a paragraph + every
            // run nested inside its table cells (cells host their
            // own ParagraphStyleRange children; their runs never
            // surface through the outer story paragraph list).
            fn harvest_keys(
                document: &Document,
                paragraph: &paged_parse::Paragraph,
                keys: &mut std::collections::HashSet<(String, Option<String>)>,
            ) {
                for run in &paragraph.runs {
                    let resolved = document.resolved_run_attrs(paragraph, run);
                    if let Some(family) = resolved.font {
                        keys.insert((family, resolved.font_style));
                    }
                }
                if let Some(table) = paragraph.table.as_ref() {
                    for cell in &table.cells {
                        for inner in &cell.paragraphs {
                            harvest_keys(document, inner, keys);
                        }
                    }
                }
            }
            for parsed in &document.stories {
                for paragraph in &parsed.story.paragraphs {
                    harvest_keys(document, paragraph, &mut keys);
                }
            }
            cache.reserve(keys.len());
            for key in keys {
                if let Some(bytes) = resolver.resolve_font(&key.0, key.1.as_deref()) {
                    cache.insert(key, bytes);
                }
            }
        }
        // Pre-build the shaping-Face cache. Walk every run again to
        // collect each distinct `(font_id, wght_bits)` actually used
        // across all stories (incl. nested table cells). The first
        // pass above resolves bytes from the asset resolver; the wght
        // axis value comes from the per-run resolved `FontStyle`.
        // Storing the configured Face here (vs. per-paragraph) lets
        // the shaping sites in `emit_paragraph_into_chain`,
        // `emit_cell_paragraph`, and `measure_cell_paragraph` share
        // one rustybuzz::Face across the entire render — Adobe-typical
        // docs reuse the same (font, weight) thousands of times.
        let mut face_keys: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        let mut id_to_bytes: HashMap<u32, Bytes> = HashMap::new();
        let harvest_face_keys =
            |paragraph: &paged_parse::Paragraph,
             face_keys: &mut std::collections::HashSet<(u32, u32)>,
             id_to_bytes: &mut HashMap<u32, Bytes>| {
                // Inner walk: handle both top-level paragraphs and
                // recursive table-cell paragraphs.
                fn walk(
                    document: &Document,
                    cache: &HashMap<(String, Option<String>), Bytes>,
                    fallback: &Option<Bytes>,
                    paragraph: &paged_parse::Paragraph,
                    face_keys: &mut std::collections::HashSet<(u32, u32)>,
                    id_to_bytes: &mut HashMap<u32, Bytes>,
                ) {
                    for run in &paragraph.runs {
                        let resolved = document.resolved_run_attrs(paragraph, run);
                        // Mirror `FontTable::bytes_for`: (family, style)
                        // direct hit, then bare-family, then fallback.
                        let bytes = resolved
                            .font
                            .as_deref()
                            .and_then(|f| {
                                cache
                                    .get(&(f.to_string(), resolved.font_style.clone()))
                                    .or_else(|| cache.get(&(f.to_string(), None)))
                            })
                            .or(fallback.as_ref());
                        if let Some(b) = bytes {
                            let font_id = fnv_1a_u32(b.as_ref());
                            let wght = wght_for_font_style(resolved.font_style.as_deref());
                            face_keys.insert((font_id, wght.to_bits()));
                            id_to_bytes.entry(font_id).or_insert_with(|| b.clone());
                        }
                    }
                    if let Some(table) = paragraph.table.as_ref() {
                        for cell in &table.cells {
                            for inner in &cell.paragraphs {
                                walk(document, cache, fallback, inner, face_keys, id_to_bytes);
                            }
                        }
                    }
                }
                walk(
                    document,
                    &cache,
                    &fallback,
                    paragraph,
                    face_keys,
                    id_to_bytes,
                );
            };
        for parsed in &document.stories {
            for paragraph in &parsed.story.paragraphs {
                harvest_face_keys(paragraph, &mut face_keys, &mut id_to_bytes);
            }
        }
        // Build `face_bytes` first (so the buffers are owned before
        // any Face borrows from them), then build `faces`. Per the
        // SAFETY contract on `faces`, the cached Face<'static>
        // borrows from the Bytes stored at the same `font_id` in
        // `face_bytes`; `Bytes` is a refcounted heap buffer whose
        // pointer is stable across clones, so the buffer is alive
        // for as long as the `face_bytes` map holds an entry.
        let face_bytes: HashMap<u32, Bytes> = id_to_bytes;
        let mut faces: HashMap<(u32, u32), rustybuzz::Face<'static>> =
            HashMap::with_capacity(face_keys.len());
        let wght_tag = ttf_parser::Tag::from_bytes(b"wght");
        for (font_id, wght_bits) in face_keys {
            let Some(buf) = face_bytes.get(&font_id) else {
                continue;
            };
            // SAFETY: extending the byte slice's lifetime to 'static.
            //  1. `buf` is a `Bytes` stored in `face_bytes` and owned
            //     by `Self`. `bytes::Bytes` is a refcounted heap
            //     buffer with a stable interior pointer — the
            //     underlying allocation cannot move while any clone
            //     exists.
            //  2. The map `face_bytes` is never mutated after this
            //     `build` returns (it has no exposed `&mut` accessor)
            //     so the buffer survives as long as the `FontTable`.
            //  3. The cached `Face<'static>` is dropped before
            //     `face_bytes`: `faces` is declared above `face_bytes`
            //     in `FontTable`, and Rust drops struct fields in
            //     declaration order (first declared = first dropped).
            //  4. The public accessor [`Self::face`] returns
            //     `&rustybuzz::Face<'_>` with the lifetime re-anchored
            //     to `&self`, so the 'static lie never escapes the
            //     module.
            //  5. The Face is never mutated post-insert: no `&mut`
            //     reference to it is exposed. Variations are baked
            //     in at insert time below; (font_id, wght_bits) keys
            //     guarantee a bold-vs-regular pair sharing the same
            //     bytes ends up in distinct cache slots.
            let bytes_static: &'static [u8] =
                unsafe { std::mem::transmute::<&[u8], &'static [u8]>(buf.as_ref()) };
            let Some(mut face) = rustybuzz::Face::from_slice(bytes_static, 0) else {
                continue;
            };
            // Only bake a wght variation when the face actually exposes
            // a `wght` axis — otherwise `set_variations` silently no-ops
            // and the bold/light slot reuses Regular metrics (P-06).
            let has_wght_axis = face
                .variation_axes()
                .into_iter()
                .any(|axis| axis.tag == wght_tag);
            let wght = f32::from_bits(wght_bits);
            if has_wght_axis {
                face.set_variations(&[rustybuzz::Variation {
                    tag: wght_tag,
                    value: wght,
                }]);
            }
            faces.insert((font_id, wght_bits), face);
        }
        // Parse metrics for every distinct byte buffer we ended up
        // caching, plus the fallback. Keyed by the same fnv hash
        // emit_paragraph uses for font_id — so the lookup is direct.
        let mut metrics: HashMap<u32, FontMetrics> = HashMap::new();
        let mut record = |bytes: &[u8]| {
            let id = fnv_1a_u32(bytes);
            if metrics.contains_key(&id) {
                return;
            }
            if let Some(m) = parse_font_metrics(bytes) {
                metrics.insert(id, m);
            }
        };
        for b in cache.values() {
            record(b.as_ref());
        }
        if let Some(b) = fallback.as_ref() {
            record(b.as_ref());
        }
        // Family-keyed override map. The override carries an
        // ascender (mandatory) plus optional cap-height / x-height —
        // missing optional values fall back to whichever metrics
        // `parse_font_metrics` extracted from the substitute font
        // (looked up by lifting them out of `metrics` via the cache's
        // bytes hash for the same family). This lets a caller pin
        // only the ascender (the dominant axis for first-baseline
        // drift) while leaving the rest at the substitute's natural
        // values.
        let mut family_metrics: HashMap<String, FontMetrics> = HashMap::new();
        for (family, ov) in options.font_metrics_overrides {
            // Find the substitute's parsed metrics for sensible
            // defaults on missing optional fields.
            let substitute = cache
                .get(&(family.clone(), None))
                .or_else(|| {
                    cache
                        .iter()
                        .find_map(|((f, _), b)| if f == family { Some(b) } else { None })
                })
                .map(|b| fnv_1a_u32(b.as_ref()))
                .and_then(|id| metrics.get(&id))
                .copied()
                .unwrap_or(FontMetrics {
                    cap_height: None,
                    x_height: None,
                    ascender: ov.ascender,
                    descender: 0.0,
                });
            family_metrics.insert(
                family.clone(),
                FontMetrics {
                    ascender: ov.ascender,
                    cap_height: ov.cap_height.or(substitute.cap_height),
                    x_height: ov.x_height.or(substitute.x_height),
                    // Descender isn't an override axis (it only feeds the
                    // anchored leading split); inherit the substitute's.
                    descender: substitute.descender,
                },
            );
        }
        Self {
            faces,
            face_bytes,
            cache,
            fallback,
            metrics,
            family_metrics,
        }
    }

    /// Returns the cached, pre-configured shaping Face for the given
    /// `(font_id, wght_bits)` key. The returned reference's lifetime
    /// is narrowed to `&self`, hiding the underlying `'static` lie
    /// (see the SAFETY contract on `FontTable::faces`).
    ///
    /// Callers must NOT call `set_variations` on the returned Face —
    /// variations are baked in at cache-insert time. The signature
    /// (`&Face`, not `&mut Face`) enforces that at compile time.
    pub(super) fn face(&self, font_id: u32, wght_bits: u32) -> Option<&rustybuzz::Face<'_>> {
        self.faces.get(&(font_id, wght_bits))
    }

    /// Look up the bytes a paragraph should shape with.
    /// Resolver hit > options.font fallback. `None` means no font
    /// is available — caller skips the paragraph.
    pub(super) fn bytes_for(&self, family: Option<&str>, style: Option<&str>) -> Option<Bytes> {
        if let Some(family) = family {
            // Direct (family, style) hit, then bare-family hit, so
            // a doc that only registers "Body Font" still picks up
            // its bold runs.
            if let Some(b) = self
                .cache
                .get(&(family.to_string(), style.map(str::to_string)))
            {
                return Some(b.clone());
            }
            if let Some(b) = self.cache.get(&(family.to_string(), None)) {
                return Some(b.clone());
            }
        }
        self.fallback.clone()
    }

    /// Resolve a paragraph's per-run font bytes, filling any
    /// individually-unresolvable run with a paragraph-level fallback
    /// so a single bad run doesn't drop the entire paragraph. The
    /// per-paragraph fallback is, in order: the first sibling run
    /// that DID resolve (keeps the visual style closest to what the
    /// rest of the paragraph uses), then [`FontTable::fallback`]
    /// (the renderer-wide default font), then `None` — signalling
    /// no font is available anywhere and the caller should skip.
    ///
    /// Returns `None` when no run resolves AND no document-wide
    /// fallback is configured. In that case the paragraph still has
    /// to be dropped because there's nothing to shape with.
    pub(super) fn resolve_paragraph_bytes(
        &self,
        runs: &[paged_scene::ResolvedRunAttrs],
    ) -> Option<Vec<Bytes>> {
        if runs.is_empty() {
            return None;
        }
        let per_run: Vec<Option<Bytes>> = runs
            .iter()
            .map(|r| self.bytes_for(r.font.as_deref(), r.font_style.as_deref()))
            .collect();
        let paragraph_fallback: Option<Bytes> = per_run
            .iter()
            .find_map(|b| b.clone())
            .or_else(|| self.fallback.clone());
        let paragraph_fallback = paragraph_fallback?;
        Some(
            per_run
                .into_iter()
                .map(|b| b.unwrap_or_else(|| paragraph_fallback.clone()))
                .collect(),
        )
    }

    pub(super) fn metrics_for(&self, font_id: u32) -> Option<&FontMetrics> {
        self.metrics.get(&font_id)
    }

    /// Override-aware metrics lookup keyed by IDML family name.
    /// Returns the per-family override when present, otherwise falls
    /// through so the caller can try the byte-hash path.
    pub(super) fn metrics_for_family(&self, family: &str) -> Option<&FontMetrics> {
        self.family_metrics.get(family)
    }
}

pub(super) fn parse_font_metrics(bytes: &[u8]) -> Option<FontMetrics> {
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;
    let upem = face.units_per_em() as f32;
    if upem <= 0.0 {
        return None;
    }
    Some(FontMetrics {
        cap_height: face.capital_height().map(|v| v as f32 / upem),
        x_height: face.x_height().map(|v| v as f32 / upem),
        ascender: face.ascender() as f32 / upem,
        // `hhea.descender` is negative (below the baseline); store the
        // magnitude so the leading-split math reads naturally.
        descender: (face.descender() as f32 / upem).abs(),
    })
}

pub(super) fn fnv_1a_u32(bytes: &[u8]) -> u32 {
    // Stable per-render font-cache key; the u32 range collides in
    // ~2B fonts — enough for any realistic document.
    const FNV_OFFSET: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}
