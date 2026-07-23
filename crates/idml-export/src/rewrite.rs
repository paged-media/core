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

//! Attribute-preserving streaming rewrite of Spread / Story XML.
//!
//! Both rewriters share one shape: a `quick_xml::Reader` feeds events,
//! a `quick_xml::Writer` re-emits them. The vast majority of events
//! (processing instructions, comments, `<Properties>`, `<PathGeometry>`,
//! unknown elements, all attributes we don't own) pass through
//! **verbatim** — we hand the original [`Event`] straight to the writer
//! so its bytes are reproduced. Only the start tags of page items
//! (spreads) / style ranges (stories) and `<Content>` text are
//! reconstructed, and even then only the model-owned attributes change;
//! every other attribute keeps its original key, value, and position.
//!
//! # The model→XML mapping is positional within an element family
//!
//! IDML carries no model index on its elements, so we walk the model in
//! the same document order the parser walked it:
//!
//! * Spread page items are matched by their `Self` id (stable, present
//!   on every page item) — robust against reordering.
//! * Story `<ParagraphStyleRange>` / `<CharacterStyleRange>` carry no
//!   `Self` id, so they're matched **positionally** against
//!   `Story::paragraphs[i].runs[j]` in document order. This is the same
//!   order the parser produced them, so an unmutated story round-trips,
//!   and a mutated story (which edits values in place, never inserts /
//!   deletes ranges) stays aligned.
//!
//! # Patch inventory (what is save-able)
//!
//! Spread page items (`TextFrame` / `Rectangle` / `Oval` / `Polygon` /
//! `GraphicLine`), patched on the element start tag:
//!   - `ItemTransform`     (FrameTransform / rotate / scale / flip / move)
//!   - `FillColor`         (FrameFillColor)
//!   - `FillTint`          (FrameFillTint)
//!   - `StrokeColor`       (FrameStrokeColor)
//!   - `StrokeWeight`      (FrameStrokeWeight)
//!   - `NextTextFrame`     (LinkFrames / UnlinkFrames; TextFrame only)
//!   - `Nonprinting`       (FrameNonprinting)
//!   - `GeometricBounds`   (FrameBounds) — patched when the source
//!     element carries the attribute. When the frame's geometry instead
//!     lives in `<PathGeometry>`/`<PathPointArray>` (the real-export +
//!     generator shape), the path anchors are rewritten directly: a
//!     `FrameBounds` resize regenerates a rectangle's corners, and
//!     `FramePathPoint` / `FramePath` edits write the moved anchors. See
//!     [`ModelGeometry`].
//!
//! Story ranges:
//!   - `<ParagraphStyleRange AppliedParagraphStyle>` (AppliedParagraphStyle)
//!   - `<CharacterStyleRange AppliedCharacterStyle>` (AppliedCharacterStyle)
//!   - `<CharacterStyleRange PointSize>`   (CharacterFontSize)
//!   - `<CharacterStyleRange FillColor>`   (CharacterFillColor)
//!   - `<CharacterStyleRange Leading / Tracking / BaselineShift /
//!     HorizontalScale / VerticalScale / Skew / FillTint / StrokeWeight>`
//!     (the matching Character* paths)
//!   - `<CharacterStyleRange AppliedFont / FontStyle / Capitalization /
//!     Position / KerningMethod / AppliedLanguage / StrokeColor /
//!     Underline / StrikeThru / Ligatures>` (the matching Character* paths)
//!   - run text — replaced across the run's `<Content>` / `<Br/>` /
//!     `<Tab/>` structure. The parser collapses
//!     `<Content>A</Content><Br/><Content>B</Content>` into one run
//!     string `"A\nB"`; the rewrite splits the model text back the same
//!     way (`\n` → `<Br/>`, `\t` → `<Tab/>`). A run carrying foreign
//!     inline markup (an `<?ACE?>` page-number PI, a
//!     `<TextVariableInstance>`, an anchored frame, an unknown entity)
//!     passes through verbatim — never clobbered (see Known losses).
//!
//! # Structural edits (W1.15 — landed)
//!
//! * **Page-item inserts / removes.** A page item created by an
//!   `InsertNode` op (a frame / rect / oval / polygon since load) is
//!   serialised as a new element at the spread's close, in the canonical
//!   `paged_gen` shape (geometry in `<Properties><PathGeometry>` at the
//!   model bounds, identity `ItemTransform`); an item removed by
//!   `RemoveNode` is dropped from the XML (element + subtree). See
//!   [`write_inserted_items`] / the `remove_depth` skip in
//!   [`rewrite_spread`].
//! * **New resources.** Swatches / gradients / paragraph + character
//!   styles created by ops are injected into `Resources/Graphic.xml` /
//!   `Resources/Styles.xml` (see the `resources` module), so a frame
//!   referencing a freshly-minted `Color/u3` resolves on re-open.
//! * **Table-cell text + styles.** A `<Cell Self="...">` is matched to
//!   its model `TableCell`, and its `<ParagraphStyleRange>` /
//!   `<CharacterStyleRange>` patch against the cell paragraphs with
//!   cell-local cursors (text + character-style attrs save).
//! * **Group-member transforms.** The composed group∘member
//!   `item_transform` is de-composed back to the on-disk member
//!   transform by inverting the group-transform accumulation (see
//!   [`recover_member_transform`]).
//!
//! # Known losses (documented, not silent)
//!
//! * **Removed PAGES leave an orphaned entry.** A `RemovePage` drops
//!   the `ParsedSpread` from the model, but the writer doesn't delete
//!   the spread's ZIP entry or its `designmap.xml` `<idPkg:Spread>` ref
//!   — the page survives on reopen. (INSERTED pages/spreads — and
//!   stories minted by InsertTextFrame — DO save since C-8: the `emit`
//!   module serialises a full part for any model spread/story with no
//!   source entry and references it from designmap.) Master-spread
//!   inserts and the removal manifest-drop remain deferred.
//! * **Singular group transform.** A group whose `ItemTransform` linear
//!   part is non-invertible can't have its member transforms de-composed;
//!   such a member keeps its `ItemTransform` verbatim (degenerate case;
//!   InDesign never emits one for a translate/rotate/scale group).
//! * **Group-member PATH anchors.** A group member's `<PathPointArray>`
//!   still passes through verbatim. (The parser does NOT compose the
//!   group transform into member anchors — it stores them raw — so a
//!   `FramePathPoint` edit on a grouped item is not yet written; the
//!   transform lane above covers the common move/scale/rotate gesture.)
//! * **Runs with foreign inline markup.** A run whose text body carries
//!   an `<?ACE?>` page-number marker, a `<TextVariableInstance>`, an
//!   anchored frame, or an unknown entity passes through verbatim (its
//!   attributes still patch). The structured text rewrite only fires on
//!   pure `<Content>` / `<Br/>` / `<Tab/>` runs.
//! * **MoveNode / sections.** Reparenting a node across spreads
//!   (`MoveNode`) and new `<Section>` definitions are not yet reflected.
//! * Anything the parser never modeled (preferences, fonts, tags, the
//!   XML backing store, master-spread item internals beyond the patched
//!   attributes) is carried through verbatim and so is always faithful.

use std::io::Cursor;

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};

use idml_import::{Bounds, CharacterRun, PathAnchor, Spread, Story, TableCell, TextFrame};

/// Mirror of `paged_gen::xml::format_f32`: round to 4 decimals, drop
/// trailing zeros + a dangling `.`, normalise `-0` to `0`. Kept as a
/// small local copy rather than depending on `paged-gen` (a dev/CLI
/// crate that pulls clap/anyhow) so this runtime crate stays minimal +
/// wasm-clean. InDesign serialises floats this way, so patched values
/// match the surrounding hand-written / exported numbers.
pub(crate) fn format_f32(v: f32) -> String {
    let rounded = (v * 10_000.0).round() / 10_000.0;
    if rounded == 0.0 {
        return "0".to_string();
    }
    let mut s = format!("{rounded:.4}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

/// Format a `[a b c d tx ty]` matrix the IDML way (space-separated,
/// fixed precision).
pub(crate) fn format_matrix(m: &[f32; 6]) -> String {
    let parts: Vec<String> = m.iter().map(|v| format_f32(*v)).collect();
    parts.join(" ")
}

/// Parse a `"a b c d tx ty"` IDML matrix. Local copy of the parser's
/// helper (private to `paged-parse`).
fn parse_matrix(s: &str) -> Option<[f32; 6]> {
    let mut it = s.split_whitespace();
    let mut m = [0.0f32; 6];
    for slot in &mut m {
        *slot = it.next()?.parse().ok()?;
    }
    Some(m)
}

/// `a ∘ b` — compose two affine matrices, byte-for-byte matching
/// `idml_import`'s `compose_matrix` (apply `b` first, then `a`). Used to
/// rebuild the group-transform accumulation the parser composes into a
/// group member's `item_transform`, so the writer can invert it back to
/// the on-disk member transform (W1.15 lane 4).
fn compose_matrix(a: &[f32; 6], b: &[f32; 6]) -> [f32; 6] {
    let [a1, b1, c1, d1, tx1, ty1] = *a;
    let [a2, b2, c2, d2, tx2, ty2] = *b;
    [
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * tx2 + c1 * ty2 + tx1,
        b1 * tx2 + d1 * ty2 + ty1,
    ]
}

/// Accumulate a group-transform stack the way the parser does (outer
/// groups apply first). `None` ⇒ no group carries a transform (identity).
fn accumulate_group_xforms(stack: &[Option<[f32; 6]>]) -> Option<[f32; 6]> {
    let mut acc: Option<[f32; 6]> = None;
    for g in stack {
        match (acc, g) {
            (None, Some(m)) => acc = Some(*m),
            (Some(a), Some(m)) => acc = Some(compose_matrix(&a, m)),
            (acc_, None) => acc = acc_,
        }
    }
    acc
}

/// Recover a group member's ON-DISK `ItemTransform` from its composed
/// model `item_transform` and the accumulated group transform `accum`:
/// `member_on_disk = inverse(accum) ∘ composed`. `None` when the group
/// transform is singular (the member then keeps its on-disk transform
/// verbatim — a documented loss for that degenerate case).
fn recover_member_transform(
    accum: Option<[f32; 6]>,
    composed: Option<[f32; 6]>,
) -> Option<Option<[f32; 6]>> {
    match accum {
        // No group transform ⇒ the model value IS the on-disk transform.
        None => Some(composed),
        Some(g) => {
            let inv = invert_matrix(&g)?;
            // A member with no composed transform under a non-identity
            // group is unusual; `None` falls through to verbatim (the
            // outer `None` suppresses the patch at the call site).
            composed.map(|c| Some(compose_matrix(&inv, &c)))
        }
    }
}

/// Invert an affine `[a b c d tx ty]`. `None` when the linear part is
/// singular (a degenerate group transform — the member then can't be
/// de-composed and keeps its on-disk transform verbatim).
fn invert_matrix(m: &[f32; 6]) -> Option<[f32; 6]> {
    let [a, b, c, d, tx, ty] = *m;
    let det = a * d - b * c;
    if det.abs() < 1e-9 {
        return None;
    }
    let inv_det = 1.0 / det;
    let ia = d * inv_det;
    let ib = -b * inv_det;
    let ic = -c * inv_det;
    let id = a * inv_det;
    // Inverse translation: -(inv_linear · t).
    let itx = -(ia * tx + ic * ty);
    let ity = -(ib * tx + id * ty);
    Some([ia, ib, ic, id, itx, ity])
}

/// One attribute patch: the value to write for `key`, or `Remove` to
/// drop the attribute entirely (model value went to `None` on an
/// attribute that was present).
enum Patch {
    Set(String),
    Remove,
}

/// Rewrite one page-item / range start tag: emit it with the same name,
/// every original attribute in its original order (model-owned keys take
/// their new value; `Remove` keys are dropped), then append any
/// model-owned keys that were newly set (absent from the source).
///
/// `lookup(key) -> Option<Patch>`: `None` ⇒ not model-owned, pass the
/// original attribute through. `Some(Set)` / `Some(Remove)` ⇒ patch it.
/// `extras`: `(key, value)` pairs to append if the key wasn't already
/// present (newly-set model attributes). Returns the rebuilt
/// `BytesStart` preserving the element name exactly.
fn patch_start<F>(
    src: &BytesStart,
    lookup: F,
    extras: &[(&str, String)],
) -> Result<BytesStart<'static>, quick_xml::Error>
where
    F: Fn(&[u8]) -> Option<Patch>,
{
    // Rebuild the start tag's raw inner content (`name attr="v" ...`)
    // by hand so unchanged attributes reproduce their ON-DISK bytes
    // exactly — no decode→re-escape round-trip that could normalise an
    // entity form and break byte-identity. `BytesStart::from_content`
    // takes this raw content and the writer emits it verbatim. IDML +
    // the generator both serialise attributes as ` key="value"` (single
    // space, double quote, no spaces around `=`); we match that so an
    // unmutated frame reproduces the source byte-for-byte.
    let name = src.name().as_ref().to_vec();
    let mut content: Vec<u8> = name.clone();
    let mut seen: Vec<Vec<u8>> = Vec::new();
    for attr in src.attributes() {
        let attr = attr?;
        let key = attr.key.as_ref().to_vec();
        match lookup(&key) {
            None => {
                // Not model-owned — copy the raw escaped value bytes.
                content.push(b' ');
                content.extend_from_slice(&key);
                content.extend_from_slice(b"=\"");
                content.extend_from_slice(attr.value.as_ref());
                content.push(b'"');
            }
            Some(Patch::Set(v)) => {
                content.push(b' ');
                content.extend_from_slice(&key);
                content.extend_from_slice(b"=\"");
                content.extend_from_slice(escape_attr(&v).as_bytes());
                content.push(b'"');
            }
            Some(Patch::Remove) => { /* dropped */ }
        }
        seen.push(key);
    }
    for (k, v) in extras {
        if !seen.iter().any(|s| s.as_slice() == k.as_bytes()) {
            content.push(b' ');
            content.extend_from_slice(k.as_bytes());
            content.extend_from_slice(b"=\"");
            content.extend_from_slice(escape_attr(v).as_bytes());
            content.push(b'"');
        }
    }
    let content = String::from_utf8(content).map_err(|e| {
        quick_xml::Error::Io(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e,
        )))
    })?;
    Ok(BytesStart::from_content(content, name.len()).into_owned())
}

/// Escape the five XML entities for an attribute value we synthesise.
/// Patched values are IDML ids / numbers / colour refs that almost never
/// contain these, but a style name could — so escape defensively to keep
/// the output well-formed.
pub(crate) fn escape_attr(s: &str) -> std::borrow::Cow<'_, str> {
    if s.bytes()
        .any(|b| matches!(b, b'&' | b'<' | b'>' | b'"' | b'\''))
    {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                _ => out.push(c),
            }
        }
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

// ---------------------------------------------------------------------
// Path geometry
// ---------------------------------------------------------------------

/// Parse a `"x y"` IDML coordinate pair. Local copy of the parser's
/// helper (it is private to `paged-parse`).
fn parse_xy_pair(s: &str) -> Option<(f32, f32)> {
    let mut it = s.split_whitespace();
    let x: f32 = it.next()?.parse().ok()?;
    let y: f32 = it.next()?.parse().ok()?;
    Some((x, y))
}

/// Format one `(x, y)` pair the IDML way (`"x y"`, fixed precision) for a
/// `PathPointType` `Anchor` / `LeftDirection` / `RightDirection` value.
fn format_xy(p: (f32, f32)) -> String {
    format!("{} {}", format_f32(p.0), format_f32(p.1))
}

/// Stable string key for one anchor, formatted exactly the way the
/// generator / a faithful export serialises it. Comparing keys (rather
/// than raw `f32`s) gives the float-format care the round-trip needs: an
/// unchanged anchor re-formats to the same bytes, so it compares equal
/// and passes through verbatim.
fn anchor_key(a: &PathAnchor) -> (String, String, String) {
    (format_xy(a.anchor), format_xy(a.left), format_xy(a.right))
}

/// AABB of an anchor set, mirroring the parser's `bounds_from_anchors`
/// (anchors only — control handles are ignored). Empty ⇒ a zero box.
fn bounds_of(anchors: &[PathAnchor]) -> Bounds {
    let mut it = anchors.iter();
    let Some(first) = it.next() else {
        return Bounds {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        };
    };
    let (mut min_x, mut max_x) = (first.anchor.0, first.anchor.0);
    let (mut min_y, mut max_y) = (first.anchor.1, first.anchor.1);
    for a in it {
        let (x, y) = a.anchor;
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    Bounds {
        top: min_y,
        left: min_x,
        bottom: max_y,
        right: max_x,
    }
}

/// Two bounds equal under `format_f32` (the on-disk precision).
fn bounds_eq_formatted(a: Bounds, b: Bounds) -> bool {
    format_f32(a.top) == format_f32(b.top)
        && format_f32(a.left) == format_f32(b.left)
        && format_f32(a.bottom) == format_f32(b.bottom)
        && format_f32(a.right) == format_f32(b.right)
}

/// Degenerate-handle corner anchor (handles coincide with the anchor —
/// what the generator emits for a plain rectangle corner).
fn corner(x: f32, y: f32) -> PathAnchor {
    PathAnchor {
        anchor: (x, y),
        left: (x, y),
        right: (x, y),
    }
}

/// The four corner anchors of `bounds`, walked in the generator's order
/// (`top-left, bottom-left, bottom-right, top-right`) so a rectangle
/// resized via `FrameBounds` re-emits the same corner sequence InDesign
/// and `paged-gen` use.
fn rect_corners(b: Bounds) -> Vec<PathAnchor> {
    vec![
        corner(b.left, b.top),
        corner(b.left, b.bottom),
        corner(b.right, b.bottom),
        corner(b.right, b.top),
    ]
}

/// The model's path geometry for one spread page item, plus a hint at
/// how to reconcile a divergence.
struct ModelGeometry {
    /// Flat anchor list across all contours (model order).
    anchors: Vec<PathAnchor>,
    /// Per-contour start offsets into `anchors` (see
    /// [`idml_import::Polygon::subpath_starts`]). Empty ⇒ one contour.
    subpath_starts: Vec<usize>,
    /// Model AABB. For a `FrameBounds` edit the anchors stay stale while
    /// this moves, so a divergence here (with unchanged anchors) means
    /// "rectangle resized" — regenerate the corners from these bounds.
    bounds: Bounds,
}

impl ModelGeometry {
    /// The target anchors for the contour starting at `parsed`'s
    /// position. `contour` indexes into `subpath_starts`. `parsed` is
    /// the on-disk anchor set for this `<PathPointArray>`. Returns
    /// `Some(target)` when the contour must be rewritten, or `None` to
    /// pass it through verbatim.
    fn target_for_contour(&self, contour: usize, parsed: &[PathAnchor]) -> Option<Vec<PathAnchor>> {
        // Bounds-only model (a plain rectangle): the parser keeps no
        // anchors for a 4-corner AABB Rectangle — its geometry lives in
        // `bounds` alone. A `FrameBounds` resize moves `bounds` while the
        // on-disk path stays, so reconcile by regenerating the corners
        // from the model bounds when they diverged (and the on-disk path
        // really is that single 4-corner rectangle).
        if self.anchors.is_empty() {
            if contour == 0
                && is_axis_aligned_rect(parsed)
                && !bounds_eq_formatted(self.bounds, bounds_of(parsed))
            {
                return Some(rect_corners(self.bounds));
            }
            return None;
        }
        let model = self.contour_slice(contour);
        // Anchor-edit path (FramePathPoint / FramePath): the model's
        // anchors for this contour diverged from disk → write them.
        if !anchors_eq_formatted(model, parsed) {
            return Some(model.to_vec());
        }
        // Bounds-only edit (FrameBounds): the anchors match disk but the
        // model AABB moved. Only safe to reconstruct for the rectangle
        // case — a single contour of 4 corners that *was* the old AABB.
        // (Non-rectangular bounds-only edits are ambiguous and stay a
        // documented loss.)
        if self.subpath_starts.len() <= 1
            && is_axis_aligned_rect(parsed)
            && !bounds_eq_formatted(self.bounds, bounds_of(parsed))
        {
            return Some(rect_corners(self.bounds));
        }
        None
    }

    fn contour_slice(&self, contour: usize) -> &[PathAnchor] {
        if self.subpath_starts.is_empty() {
            return &self.anchors;
        }
        let start = self.subpath_starts[contour];
        let end = self
            .subpath_starts
            .get(contour + 1)
            .copied()
            .unwrap_or(self.anchors.len());
        self.anchors.get(start..end).unwrap_or(&[])
    }
}

/// True when a 4-anchor contour is an axis-aligned rectangle: each
/// anchor sits on an AABB corner (degenerate handles) and all four
/// corners are present. This is the only shape a `FrameBounds` resize
/// can faithfully reconstruct from bounds alone — a non-rectangular
/// path needs an explicit `FramePathPoint` / `FramePath` edit, so a
/// bounds-only change there stays a documented loss.
fn is_axis_aligned_rect(anchors: &[PathAnchor]) -> bool {
    if anchors.len() != 4 {
        return false;
    }
    let b = bounds_of(anchors);
    // Each anchor must be one of the four corners (handles degenerate to
    // the anchor), and every corner must be covered exactly once.
    let corners = [
        (b.left, b.top),
        (b.left, b.bottom),
        (b.right, b.bottom),
        (b.right, b.top),
    ];
    let mut covered = [false; 4];
    for a in anchors {
        if format_xy(a.left) != format_xy(a.anchor) || format_xy(a.right) != format_xy(a.anchor) {
            return false; // a real Bezier handle — not a plain corner
        }
        let key = format_xy(a.anchor);
        match corners.iter().position(|c| format_xy(*c) == key) {
            Some(i) if !covered[i] => covered[i] = true,
            _ => return false,
        }
    }
    covered.iter().all(|&c| c)
}

/// Two anchor sets equal under `format_f32` (on-disk precision).
fn anchors_eq_formatted(a: &[PathAnchor], b: &[PathAnchor]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| anchor_key(x) == anchor_key(y))
}

/// The model path geometry for the page item `name`/`self_id` carries,
/// if that kind tracks anchors (TextFrame / Rectangle / Polygon /
/// GraphicLine). Oval geometry is bounds-only in the model (no anchors),
/// so its `<PathPointArray>` always passes through verbatim.
fn model_geometry(
    name: &[u8],
    self_id: &str,
    frames: &std::collections::HashMap<&str, &TextFrame>,
    rectangles: &[idml_import::Rectangle],
    polygons: &[idml_import::Polygon],
    graphic_lines: &[idml_import::GraphicLine],
) -> Option<ModelGeometry> {
    match name {
        b"TextFrame" => frames.get(self_id).map(|f| ModelGeometry {
            anchors: f.anchors.clone(),
            subpath_starts: f.subpath_starts.clone(),
            bounds: f.bounds,
        }),
        b"Rectangle" => rectangles
            .iter()
            .find(|r| r.self_id.as_deref() == Some(self_id))
            .map(|r| ModelGeometry {
                anchors: r.anchors.clone(),
                subpath_starts: r.subpath_starts.clone(),
                bounds: r.bounds,
            }),
        b"Polygon" => polygons
            .iter()
            .find(|r| r.self_id.as_deref() == Some(self_id))
            .map(|r| ModelGeometry {
                anchors: r.anchors.clone(),
                subpath_starts: r.subpath_starts.clone(),
                bounds: r.bounds,
            }),
        b"GraphicLine" => graphic_lines
            .iter()
            .find(|r| r.self_id.as_deref() == Some(self_id))
            .map(|r| ModelGeometry {
                anchors: r.anchors.clone(),
                subpath_starts: r.subpath_starts.clone(),
                bounds: r.bounds,
            }),
        _ => None,
    }
}

/// Read one `<PathPointType>` element into a [`PathAnchor`], mirroring
/// the parser: a missing `LeftDirection` / `RightDirection` defaults to
/// the anchor (degenerate handle).
fn path_point_anchor(e: &BytesStart) -> Option<PathAnchor> {
    let a = attr_value(e, b"Anchor").and_then(|s| parse_xy_pair(&s))?;
    let left = attr_value(e, b"LeftDirection")
        .and_then(|s| parse_xy_pair(&s))
        .unwrap_or(a);
    let right = attr_value(e, b"RightDirection")
        .and_then(|s| parse_xy_pair(&s))
        .unwrap_or(a);
    Some(PathAnchor {
        anchor: a,
        left,
        right,
    })
}

/// Emit one `<PathPointType Anchor="x y" LeftDirection="x y"
/// RightDirection="x y"/>` self-closing element, matching the
/// generator's attribute order + `format_f32` precision.
fn write_path_point(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    a: &PathAnchor,
) -> Result<(), quick_xml::Error> {
    let mut e = BytesStart::new("PathPointType");
    e.push_attribute(("Anchor", format_xy(a.anchor).as_str()));
    e.push_attribute(("LeftDirection", format_xy(a.left).as_str()));
    e.push_attribute(("RightDirection", format_xy(a.right).as_str()));
    writer.write_event(Event::Empty(e))?;
    Ok(())
}

// ---------------------------------------------------------------------
// New page-item emission (structural inserts — W1.15)
// ---------------------------------------------------------------------
//
// A page item created by an op since load (`InsertNode`) has a model
// entry but no XML element. We serialise it here in the canonical
// `paged_gen` shape so the writer's own parser round-trips it:
//
//   * geometry lives in `<Properties><PathGeometry>` (inner coords),
//     NOT in a `GeometricBounds` attribute. The parser derives
//     `bounds = bounds_from_anchors(raw anchors)`, so we emit corner
//     anchors directly AT the model's spread-space bounds with an
//     identity `ItemTransform`. (Inserted nodes carry their placement
//     in `bounds`; `item_transform` is `None`/identity — see
//     `paged_mutate::apply::new_rectangle` et al.)
//   * an explicit `StrokeWeight="0"` makes "no stroke" survive
//     InDesign's object-style cascade, matching the generator.

/// `<PathGeometry>` for an axis-aligned box whose corners sit at the
/// given spread-space bounds (top-left, bottom-left, bottom-right,
/// top-right — the generator + `rect_corners` order). The parser reads
/// the anchors back verbatim, so `bounds_from_anchors` reproduces these
/// bounds exactly.
fn write_box_path_geometry(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    b: Bounds,
) -> Result<(), quick_xml::Error> {
    writer.write_event(Event::Start(BytesStart::new("PathGeometry")))?;
    let mut gp = BytesStart::new("GeometryPathType");
    gp.push_attribute(("PathOpen", "false"));
    writer.write_event(Event::Start(gp))?;
    writer.write_event(Event::Start(BytesStart::new("PathPointArray")))?;
    for a in rect_corners(b) {
        write_path_point(writer, &a)?;
    }
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new(
        "PathPointArray",
    )))?;
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new(
        "GeometryPathType",
    )))?;
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new("PathGeometry")))?;
    Ok(())
}

/// `<PathGeometry>` carrying explicit anchor contours (the Polygon /
/// GraphicLine inserted-node case). `subpath_starts` splits `anchors`
/// into `<GeometryPathType>` contours; `subpath_open` marks the open
/// ones (`PathOpen="true"`). An empty `subpath_starts` is one closed
/// contour over all anchors.
fn write_contour_path_geometry(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
) -> Result<(), quick_xml::Error> {
    writer.write_event(Event::Start(BytesStart::new("PathGeometry")))?;
    let starts: Vec<usize> = if subpath_starts.is_empty() {
        vec![0]
    } else {
        subpath_starts.to_vec()
    };
    for (ci, &start) in starts.iter().enumerate() {
        let end = starts.get(ci + 1).copied().unwrap_or(anchors.len());
        let open = subpath_open.get(ci).copied().unwrap_or(false);
        let mut gp = BytesStart::new("GeometryPathType");
        gp.push_attribute(("PathOpen", if open { "true" } else { "false" }));
        writer.write_event(Event::Start(gp))?;
        writer.write_event(Event::Start(BytesStart::new("PathPointArray")))?;
        for a in anchors.get(start..end).unwrap_or(&[]) {
            write_path_point(writer, a)?;
        }
        writer.write_event(Event::End(quick_xml::events::BytesEnd::new(
            "PathPointArray",
        )))?;
        writer.write_event(Event::End(quick_xml::events::BytesEnd::new(
            "GeometryPathType",
        )))?;
    }
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new("PathGeometry")))?;
    Ok(())
}

/// Common fill/stroke/transform attributes every inserted page item
/// carries, in the generator's order. `kind`-specific attrs (ParentStory
/// etc.) are pushed by the caller before this runs.
fn push_common_item_attrs(
    attrs: &mut Vec<(&'static str, String)>,
    item_transform: Option<[f32; 6]>,
    fill_color: &Option<String>,
    stroke_color: &Option<String>,
    stroke_weight: Option<f32>,
) {
    attrs.push(("AppliedObjectStyle", "ObjectStyle/$ID/[None]".to_string()));
    attrs.push((
        "ItemTransform",
        format_matrix(&item_transform.unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])),
    ));
    attrs.push((
        "FillColor",
        fill_color
            .clone()
            .unwrap_or_else(|| "Swatch/None".to_string()),
    ));
    attrs.push((
        "StrokeColor",
        stroke_color
            .clone()
            .unwrap_or_else(|| "Swatch/None".to_string()),
    ));
    // Always emit StrokeWeight so the "no stroke" intent survives the
    // object-style cascade (the generator's rationale).
    attrs.push(("StrokeWeight", format_f32(stroke_weight.unwrap_or(0.0))));
}

/// Build a start/empty tag's `BytesStart` from `(key, value)` pairs
/// (values escaped). Element name is taken verbatim.
fn tag_with_attrs(
    name: &str,
    attrs: &[(&str, String)],
) -> Result<BytesStart<'static>, quick_xml::Error> {
    let mut content = name.as_bytes().to_vec();
    for (k, v) in attrs {
        content.push(b' ');
        content.extend_from_slice(k.as_bytes());
        content.extend_from_slice(b"=\"");
        content.extend_from_slice(escape_attr(v).as_bytes());
        content.push(b'"');
    }
    let content = String::from_utf8(content).map_err(|e| {
        quick_xml::Error::Io(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e,
        )))
    })?;
    Ok(BytesStart::from_content(content, name.len()).into_owned())
}

/// Emit a start tag from `(key, value)` pairs (values escaped).
pub(crate) fn emit_start_with_attrs(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    attrs: &[(&str, String)],
) -> Result<(), quick_xml::Error> {
    writer.write_event(Event::Start(tag_with_attrs(name, attrs)?))?;
    Ok(())
}

/// Emit a self-closing tag from `(key, value)` pairs (values escaped).
pub(crate) fn emit_empty_with_attrs(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    attrs: &[(&str, String)],
) -> Result<(), quick_xml::Error> {
    writer.write_event(Event::Empty(tag_with_attrs(name, attrs)?))?;
    Ok(())
}

/// Serialise an inserted `<TextFrame>`. The model classification is
/// authoritative — the element is always emitted as `<TextFrame>` so the
/// re-parse files it back under `Spread::text_frames` (the parser keys
/// on element name, not on `ParentStory`). A frame the model carries
/// without a story still emits `ParentStory="n"` / `ContentType` so it
/// reads back as a (currently empty) text frame.
fn write_new_text_frame(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    f: &TextFrame,
) -> Result<(), quick_xml::Error> {
    let Some(self_id) = f.self_id.as_deref() else {
        return Ok(());
    };
    let mut attrs: Vec<(&str, String)> = vec![("Self", self_id.to_string())];
    // A wire-minted story id (`Story/u<n>`) is written SANITIZED (`/` →
    // `_`) so the reference matches the id `derive_story_id` re-derives
    // from the emitted `Stories/Story_<sanitized>.xml` entry on reopen
    // (C-8). Parsed story ids carry no slash and pass through unchanged.
    attrs.push((
        "ParentStory",
        f.parent_story
            .as_deref()
            .map(crate::emit::sanitize_id)
            .unwrap_or_else(|| "n".to_string()),
    ));
    attrs.push(("PreviousTextFrame", "n".to_string()));
    attrs.push((
        "NextTextFrame",
        f.next_text_frame.clone().unwrap_or_else(|| "n".to_string()),
    ));
    attrs.push(("ContentType", "TextType".to_string()));
    push_common_item_attrs(
        &mut attrs,
        f.item_transform,
        &f.fill_color,
        &f.stroke_color,
        f.stroke_weight,
    );
    if f.nonprinting {
        attrs.push(("Nonprinting", "true".to_string()));
    }
    emit_start_with_attrs(writer, "TextFrame", &attrs)?;
    writer.write_event(Event::Start(BytesStart::new("Properties")))?;
    write_box_path_geometry(writer, f.bounds)?;
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new("Properties")))?;
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new("TextFrame")))?;
    Ok(())
}

/// Serialise an inserted bounds-only vector frame (`<Rectangle>` /
/// `<Oval>`). Geometry is the four-corner box at the model bounds.
fn write_new_box_item(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    kind: &str,
    self_id: &str,
    item_transform: Option<[f32; 6]>,
    fill_color: &Option<String>,
    stroke_color: &Option<String>,
    stroke_weight: Option<f32>,
    nonprinting: bool,
    bounds: Bounds,
) -> Result<(), quick_xml::Error> {
    let mut attrs: Vec<(&str, String)> = vec![("Self", self_id.to_string())];
    push_common_item_attrs(
        &mut attrs,
        item_transform,
        fill_color,
        stroke_color,
        stroke_weight,
    );
    if nonprinting {
        attrs.push(("Nonprinting", "true".to_string()));
    }
    emit_start_with_attrs(writer, kind, &attrs)?;
    writer.write_event(Event::Start(BytesStart::new("Properties")))?;
    write_box_path_geometry(writer, bounds)?;
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new("Properties")))?;
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new(kind)))?;
    Ok(())
}

/// Serialise an inserted path-bearing vector frame (`<Polygon>` /
/// `<GraphicLine>`). Geometry is the explicit anchor contours; when the
/// model has no anchors (rare for these kinds) it falls back to the
/// bounds box so the element still parses.
#[allow(clippy::too_many_arguments)]
fn write_new_path_item(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    kind: &str,
    self_id: &str,
    item_transform: Option<[f32; 6]>,
    fill_color: &Option<String>,
    stroke_color: &Option<String>,
    stroke_weight: Option<f32>,
    nonprinting: bool,
    bounds: Bounds,
    anchors: &[PathAnchor],
    subpath_starts: &[usize],
    subpath_open: &[bool],
    extra_attrs: &[(&'static str, String)],
) -> Result<(), quick_xml::Error> {
    let mut attrs: Vec<(&str, String)> = vec![("Self", self_id.to_string())];
    push_common_item_attrs(
        &mut attrs,
        item_transform,
        fill_color,
        stroke_color,
        stroke_weight,
    );
    if nonprinting {
        attrs.push(("Nonprinting", "true".to_string()));
    }
    for (k, v) in extra_attrs {
        attrs.push((k, v.clone()));
    }
    emit_start_with_attrs(writer, kind, &attrs)?;
    writer.write_event(Event::Start(BytesStart::new("Properties")))?;
    if anchors.is_empty() {
        write_box_path_geometry(writer, bounds)?;
    } else {
        write_contour_path_geometry(writer, anchors, subpath_starts, subpath_open)?;
    }
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new("Properties")))?;
    writer.write_event(Event::End(quick_xml::events::BytesEnd::new(kind)))?;
    Ok(())
}

/// Append every model page item whose `Self` id was NOT seen in the
/// source XML — the inserted nodes. Emitted at the spread's close in
/// the model's per-kind vec order. Group members are skipped (a group's
/// own insertion is a separate, deferred lane — see Known losses).
pub(crate) fn write_inserted_items(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    spread: &Spread,
    seen: &std::collections::HashSet<String>,
) -> Result<(), quick_xml::Error> {
    // Collect the `Self` ids that live inside a group so we don't emit
    // a group member as a stray top-level item.
    let mut grouped: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for g in &spread.groups {
        collect_group_member_ids(spread, g, &mut grouped);
    }
    for f in &spread.text_frames {
        if let Some(id) = f.self_id.as_deref() {
            if !seen.contains(id) && !grouped.contains(id) {
                write_new_text_frame(writer, f)?;
            }
        }
    }
    for r in &spread.rectangles {
        if let Some(id) = r.self_id.as_deref() {
            if !seen.contains(id) && !grouped.contains(id) {
                write_new_box_item(
                    writer,
                    "Rectangle",
                    id,
                    r.item_transform,
                    &r.fill_color,
                    &r.stroke_color,
                    r.stroke_weight,
                    r.nonprinting,
                    r.bounds,
                )?;
            }
        }
    }
    for o in &spread.ovals {
        if let Some(id) = o.self_id.as_deref() {
            if !seen.contains(id) && !grouped.contains(id) {
                write_new_box_item(
                    writer,
                    "Oval",
                    id,
                    o.item_transform,
                    &o.fill_color,
                    &o.stroke_color,
                    o.stroke_weight,
                    o.nonprinting,
                    o.bounds,
                )?;
            }
        }
    }
    for p in &spread.polygons {
        if let Some(id) = p.self_id.as_deref() {
            if !seen.contains(id) && !grouped.contains(id) {
                write_new_path_item(
                    writer,
                    "Polygon",
                    id,
                    p.item_transform,
                    &p.fill_color,
                    &p.stroke_color,
                    p.stroke_weight,
                    p.nonprinting,
                    p.bounds,
                    &p.anchors,
                    &p.subpath_starts,
                    &p.subpath_open,
                    &[],
                )?;
            }
        }
    }
    for l in &spread.graphic_lines {
        if let Some(id) = l.self_id.as_deref() {
            if !seen.contains(id) && !grouped.contains(id) {
                // v43 — an inserted line that was given arrowheads
                // before save keeps them (the patch lane only covers
                // items that exist in the source XML).
                let mut extra: Vec<(&'static str, String)> = Vec::new();
                for (k, t) in [
                    ("LeftLineEnd", l.start_arrow),
                    ("RightLineEnd", l.end_arrow),
                ] {
                    if t.draws() && !t.as_idml().is_empty() {
                        extra.push((k, t.as_idml().to_string()));
                    }
                }
                write_new_path_item(
                    writer,
                    "GraphicLine",
                    id,
                    l.item_transform,
                    &None,
                    &l.stroke_color,
                    l.stroke_weight,
                    l.nonprinting,
                    l.bounds,
                    &l.anchors,
                    &l.subpath_starts,
                    &l.subpath_open,
                    &extra,
                )?;
            }
        }
    }
    Ok(())
}

/// Recursively gather the `Self` ids of every page item referenced by a
/// group (and its sub-groups) so inserted-item emission skips them.
fn collect_group_member_ids<'a>(
    spread: &'a Spread,
    group: &'a idml_import::Group,
    out: &mut std::collections::HashSet<&'a str>,
) {
    use idml_import::FrameRef;
    for m in &group.members {
        match *m {
            FrameRef::TextFrame(i) => {
                if let Some(id) = spread.text_frames.get(i).and_then(|f| f.self_id.as_deref()) {
                    out.insert(id);
                }
            }
            FrameRef::Rectangle(i) => {
                if let Some(id) = spread.rectangles.get(i).and_then(|r| r.self_id.as_deref()) {
                    out.insert(id);
                }
            }
            FrameRef::Oval(i) => {
                if let Some(id) = spread.ovals.get(i).and_then(|o| o.self_id.as_deref()) {
                    out.insert(id);
                }
            }
            FrameRef::GraphicLine(i) => {
                if let Some(id) = spread
                    .graphic_lines
                    .get(i)
                    .and_then(|l| l.self_id.as_deref())
                {
                    out.insert(id);
                }
            }
            FrameRef::Polygon(i) => {
                if let Some(id) = spread.polygons.get(i).and_then(|p| p.self_id.as_deref()) {
                    out.insert(id);
                }
            }
            FrameRef::Group(i) => {
                if let Some(sub) = spread.groups.get(i) {
                    collect_group_member_ids(spread, sub, out);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// Spread rewrite
// ---------------------------------------------------------------------

/// Rewrite a `Spread_*.xml` body so its page-item start tags reflect the
/// current model. Untouched bytes pass through verbatim; the result is
/// byte-identical to `original` when nothing in `spread` diverged from it.
pub fn rewrite_spread(original: &[u8], spread: &Spread) -> Result<Vec<u8>, quick_xml::Error> {
    // Index every page item by its `Self` id so a start tag can find its
    // model counterpart regardless of element ordering.
    let mut frames: std::collections::HashMap<&str, &TextFrame> = std::collections::HashMap::new();
    for f in &spread.text_frames {
        if let Some(id) = f.self_id.as_deref() {
            frames.insert(id, f);
        }
    }

    // W1.15 — structural inserts/removes. `model_ids` is every page-item
    // `Self` the model still carries; `seen_ids` accumulates the ids that
    // appear in the source XML. A top-level XML item whose id left the
    // model is a REMOVE (the element is dropped); a model id never seen
    // in the XML is an INSERT (emitted at the spread's close in model
    // order). Group members are not removed structurally here — a group
    // dissolve / regroup is a separate deferred lane (see Known losses).
    let mut model_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for f in &spread.text_frames {
        if let Some(id) = f.self_id.as_deref() {
            model_ids.insert(id);
        }
    }
    for r in &spread.rectangles {
        if let Some(id) = r.self_id.as_deref() {
            model_ids.insert(id);
        }
    }
    for o in &spread.ovals {
        if let Some(id) = o.self_id.as_deref() {
            model_ids.insert(id);
        }
    }
    for p in &spread.polygons {
        if let Some(id) = p.self_id.as_deref() {
            model_ids.insert(id);
        }
    }
    for l in &spread.graphic_lines {
        if let Some(id) = l.self_id.as_deref() {
            model_ids.insert(id);
        }
    }
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Depth of the open element being dropped as a REMOVE, plus the
    // depth it opened at; while `> 0` every event passes through to the
    // bit-bucket until the matching close. `0` ⇒ not removing.
    let mut remove_depth: usize = 0;

    let mut reader = Reader::from_reader(original);
    let config = reader.config_mut();
    config.expand_empty_elements = false;
    config.trim_text(false);

    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut buf = Vec::new();

    // Depth of open `<Group>` elements. Inside a group the parser
    // COMPOSES the group transform into each member's `item_transform`
    // (see `effective_item_transform`), so the model value is the
    // group∘member matrix, NOT the on-disk member transform. W1.15 lane
    // 4 recovers the on-disk transform by inverting the accumulated
    // group stack (`group_xforms`): `member_on_disk = inverse(accum) ∘
    // model.item_transform`. The stack mirrors the parser's: each open
    // `<Group ItemTransform>` pushes its RAW transform (parsed straight
    // off the XML, same source the parser composed from). Fills /
    // strokes / colours are not composed and patch safely at any depth.
    let mut group_depth: usize = 0;
    // RAW `<Group ItemTransform>` per open group, outermost first. `None`
    // for a group with no ItemTransform (identity).
    let mut group_xforms: Vec<Option<[f32; 6]>> = Vec::new();

    // ---- plugin-metadata Label patching state ----
    // Element-name stack (depth tracking) + the innermost open page
    // item that the model labels. The model's `spread.labels` map IS
    // the truth: an item's `<Label>` contents are replaced wholesale
    // with the model entries; a labelled item whose source has no
    // `<Properties>`/`<Label>` gets the block synthesised; an item the
    // model no longer labels has its `<Label>` dropped.
    let mut depth: usize = 0;
    struct LabelCtx {
        /// Depth of the item element itself.
        item_depth: usize,
        /// Model entries; `None` ⇒ the model has no labels for it.
        entries: Option<Vec<(String, String)>>,
        /// A direct `<Properties>` child is currently open.
        in_direct_properties: bool,
        /// We are inside the item's `<Label>` (original KVPs drop).
        in_label: bool,
        /// The model entries have been written.
        handled: bool,
    }
    let mut label_ctx: Vec<LabelCtx> = Vec::new();
    const ITEM_KINDS: [&[u8]; 5] = [
        b"TextFrame",
        b"Rectangle",
        b"Oval",
        b"GraphicLine",
        b"Polygon",
    ];
    fn write_label_entries(
        writer: &mut Writer<Cursor<Vec<u8>>>,
        entries: &[(String, String)],
    ) -> Result<(), quick_xml::Error> {
        writer.write_event(Event::Start(BytesStart::new("Label")))?;
        for (k, v) in entries {
            let mut kvp = BytesStart::new("KeyValuePair");
            kvp.push_attribute(("Key", k.as_str()));
            kvp.push_attribute(("Value", v.as_str()));
            writer.write_event(Event::Empty(kvp))?;
        }
        writer.write_event(Event::End(quick_xml::events::BytesEnd::new("Label")))?;
        Ok(())
    }

    // ---- PathPointArray rewrite state ----
    // The innermost open page item that tracks anchors (TextFrame /
    // Rectangle / Polygon / GraphicLine). Real InDesign exports (and
    // every generated fixture) carry frame geometry as a
    // `<PathPointArray>` of `<PathPointType>` anchors rather than a
    // `GeometricBounds` attribute, so a `FramePathPoint` / `FramePath`
    // edit — or a `FrameBounds` resize of a rectangular frame — has to
    // rewrite those anchors to save. We buffer each `<PathPointArray>`
    // and, at its close, either re-emit the model anchors (when the
    // contour diverged) or replay the original points verbatim (so an
    // unmutated path stays byte-identical).
    struct PathCtx {
        /// Depth of the page-item element.
        item_depth: usize,
        /// Model geometry, or `None` for a kind that doesn't track
        /// anchors (Oval) / an item with no model match.
        geom: Option<ModelGeometry>,
        /// Index of the next `<GeometryPathType>` contour / its
        /// `<PathPointArray>`.
        contour: usize,
        /// Depth of the open `<PathPointArray>`, or 0 when not in one.
        array_depth: usize,
        /// Buffered events inside the open `<PathPointArray>` (point
        /// elements + any whitespace between them).
        buffered: Vec<Event<'static>>,
        /// On-disk anchors parsed from the buffered points.
        parsed: Vec<PathAnchor>,
    }
    let mut path_ctx: Vec<PathCtx> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) => {
                depth += 1;
                let name_owned = e.name().as_ref().to_vec();
                // Inside a REMOVE drop everything until the matching
                // close — the element and its whole subtree vanish.
                if remove_depth != 0 {
                    buf.clear();
                    continue;
                }
                // A top-level page item whose `Self` left the model is a
                // structural REMOVE: drop the element + its subtree.
                if group_depth == 0 && ITEM_KINDS.contains(&name_owned.as_slice()) {
                    if let Some(id) = attr_value(&e, b"Self") {
                        seen_ids.insert(id.clone());
                        if !model_ids.contains(id.as_str()) {
                            remove_depth = depth;
                            buf.clear();
                            continue;
                        }
                    }
                }
                // Buffer a `<PathPointArray>` for the innermost path
                // item so its points can be rewritten at close.
                if name_owned == b"PathPointArray" {
                    if let Some(ctx) = path_ctx.last_mut() {
                        if ctx.array_depth == 0 {
                            ctx.array_depth = depth;
                            ctx.buffered.clear();
                            ctx.parsed.clear();
                            writer.write_event(Event::Start(e.into_owned()))?;
                            buf.clear();
                            continue;
                        }
                    }
                }
                if let Some(ctx) = path_ctx.last_mut() {
                    if ctx.array_depth != 0 {
                        // Nested element inside the array — buffer it.
                        ctx.buffered.push(Event::Start(e.into_owned()));
                        buf.clear();
                        continue;
                    }
                }
                // Label handling for the innermost labelled item.
                if let Some(ctx) = label_ctx.last_mut() {
                    if name_owned == b"Properties" && depth == ctx.item_depth + 1 {
                        ctx.in_direct_properties = true;
                    } else if name_owned == b"Label"
                        && ctx.in_direct_properties
                        && depth == ctx.item_depth + 2
                    {
                        // Replace (or drop) the Label wholesale.
                        ctx.in_label = true;
                        if let Some(entries) = ctx.entries.as_deref() {
                            write_label_entries(&mut writer, entries)?;
                        }
                        ctx.handled = true;
                        buf.clear();
                        continue; // original <Label> start not written
                    } else if ctx.in_label {
                        // Unexpected child inside a replaced Label —
                        // drop it with the rest of the Label body.
                        buf.clear();
                        continue;
                    }
                }
                let group_accum = if group_depth > 0 {
                    Some(accumulate_group_xforms(&group_xforms))
                } else {
                    None
                };
                let patched = patch_spread_item(
                    &e,
                    &frames,
                    &spread.rectangles,
                    &spread.ovals,
                    &spread.polygons,
                    &spread.graphic_lines,
                    group_accum,
                )?;
                match patched {
                    Some(start) => writer.write_event(Event::Start(start))?,
                    None => writer.write_event(Event::Start(e.clone().into_owned()))?,
                }
                if name_owned == b"Group" {
                    group_depth += 1;
                    group_xforms
                        .push(attr_value(&e, b"ItemTransform").and_then(|s| parse_matrix(&s)));
                }
                if ITEM_KINDS.contains(&name_owned.as_slice()) {
                    let self_id = attr_value(&e, b"Self");
                    let entries = self_id
                        .as_deref()
                        .and_then(|id| spread.labels.get(id).cloned())
                        .filter(|v| !v.is_empty());
                    label_ctx.push(LabelCtx {
                        item_depth: depth,
                        entries,
                        in_direct_properties: false,
                        in_label: false,
                        handled: false,
                    });
                    // Group-member geometry is composed into the model's
                    // anchors the same way the transform is (see the
                    // group note in `rewrite_spread`), so we don't rewrite
                    // a member's path either — leave `geom: None` inside a
                    // group so its points pass through verbatim.
                    let geom = if group_depth > 0 {
                        None
                    } else {
                        self_id.as_deref().and_then(|id| {
                            model_geometry(
                                &name_owned,
                                id,
                                &frames,
                                &spread.rectangles,
                                &spread.polygons,
                                &spread.graphic_lines,
                            )
                        })
                    };
                    path_ctx.push(PathCtx {
                        item_depth: depth,
                        geom,
                        contour: 0,
                        array_depth: 0,
                        buffered: Vec::new(),
                        parsed: Vec::new(),
                    });
                }
            }
            Event::Empty(e) => {
                // Inside a REMOVE every empty element vanishes too.
                if remove_depth != 0 {
                    buf.clear();
                    continue;
                }
                // A self-closing top-level page item: track it as seen,
                // and drop it when its `Self` left the model (REMOVE).
                if group_depth == 0 && ITEM_KINDS.contains(&e.name().as_ref()) {
                    if let Some(id) = attr_value(&e, b"Self") {
                        seen_ids.insert(id.clone());
                        if !model_ids.contains(id.as_str()) {
                            buf.clear();
                            continue;
                        }
                    }
                }
                // Buffer a `<PathPointType>` (or any empty element)
                // inside an open `<PathPointArray>`.
                if let Some(ctx) = path_ctx.last_mut() {
                    if ctx.array_depth != 0 {
                        if e.name().as_ref() == b"PathPointType" {
                            if let Some(a) = path_point_anchor(&e) {
                                ctx.parsed.push(a);
                            }
                        }
                        ctx.buffered.push(Event::Empty(e.into_owned()));
                        buf.clear();
                        continue;
                    }
                }
                // KeyValuePairs inside a replaced Label drop (the
                // model entries were already written).
                if let Some(ctx) = label_ctx.last() {
                    if ctx.in_label {
                        buf.clear();
                        continue;
                    }
                }
                let name_is_item = ITEM_KINDS.contains(&e.name().as_ref());
                let group_accum = if group_depth > 0 {
                    Some(accumulate_group_xforms(&group_xforms))
                } else {
                    None
                };
                let patched = patch_spread_item(
                    &e,
                    &frames,
                    &spread.rectangles,
                    &spread.ovals,
                    &spread.polygons,
                    &spread.graphic_lines,
                    group_accum,
                )?;
                // A labelled item serialised as an EMPTY tag must grow
                // children — expand to Start + Properties/Label + End.
                let pending_entries = if name_is_item {
                    attr_value(&e, b"Self")
                        .and_then(|id| spread.labels.get(&id).cloned())
                        .filter(|v| !v.is_empty())
                } else {
                    None
                };
                if let Some(entries) = pending_entries {
                    let name_owned = e.name().as_ref().to_vec();
                    match patched {
                        Some(start) => writer.write_event(Event::Start(start))?,
                        None => writer.write_event(Event::Start(e.clone().into_owned()))?,
                    }
                    writer.write_event(Event::Start(BytesStart::new("Properties")))?;
                    write_label_entries(&mut writer, &entries)?;
                    writer
                        .write_event(Event::End(quick_xml::events::BytesEnd::new("Properties")))?;
                    writer.write_event(Event::End(quick_xml::events::BytesEnd::new(
                        String::from_utf8_lossy(&name_owned).into_owned(),
                    )))?;
                } else {
                    match patched {
                        Some(start) => writer.write_event(Event::Empty(start))?,
                        None => writer.write_event(Event::Empty(e.into_owned()))?,
                    }
                }
            }
            Event::End(e) => {
                let name_owned = e.name().as_ref().to_vec();
                // Closing a REMOVE: when this End matches the removed
                // element's open depth the drop ends; otherwise it is a
                // child of the removed subtree and also vanishes.
                if remove_depth != 0 {
                    if depth == remove_depth {
                        remove_depth = 0;
                    }
                    depth = depth.saturating_sub(1);
                    buf.clear();
                    continue;
                }
                // Closing the `<Spread>` / `<MasterSpread>`: before the
                // tag, flush every model page item the source XML never
                // carried — the structural INSERTs.
                if name_owned == b"Spread" || name_owned == b"MasterSpread" {
                    write_inserted_items(&mut writer, spread, &seen_ids)?;
                    depth = depth.saturating_sub(1);
                    writer.write_event(Event::End(e))?;
                    buf.clear();
                    continue;
                }
                // Close of the buffered `<PathPointArray>`: decide whether
                // this contour diverged and emit the model anchors, or
                // replay the original points verbatim.
                if let Some(ctx) = path_ctx.last_mut() {
                    if ctx.array_depth != 0 {
                        if name_owned == b"PathPointArray" && depth == ctx.array_depth {
                            let contour = ctx.contour;
                            ctx.contour += 1;
                            let target = ctx
                                .geom
                                .as_ref()
                                .and_then(|g| g.target_for_contour(contour, &ctx.parsed));
                            match target {
                                Some(anchors) => {
                                    for a in &anchors {
                                        write_path_point(&mut writer, a)?;
                                    }
                                }
                                None => {
                                    for ev in ctx.buffered.drain(..) {
                                        writer.write_event(ev)?;
                                    }
                                }
                            }
                            ctx.buffered.clear();
                            ctx.parsed.clear();
                            ctx.array_depth = 0;
                            depth = depth.saturating_sub(1);
                            writer.write_event(Event::End(e))?;
                            buf.clear();
                            continue;
                        }
                        // A nested End inside the array — buffer it.
                        ctx.buffered.push(Event::End(e.into_owned()));
                        depth = depth.saturating_sub(1);
                        buf.clear();
                        continue;
                    }
                    if depth == ctx.item_depth && ITEM_KINDS.contains(&name_owned.as_slice()) {
                        path_ctx.pop();
                    }
                }
                if let Some(ctx) = label_ctx.last_mut() {
                    if ctx.in_label && name_owned == b"Label" && depth == ctx.item_depth + 2 {
                        // Closing the replaced Label — the new entries
                        // (with their own End) were already written.
                        ctx.in_label = false;
                        depth = depth.saturating_sub(1);
                        buf.clear();
                        continue;
                    }
                    if ctx.in_label {
                        // Closing a dropped child inside the Label.
                        depth = depth.saturating_sub(1);
                        buf.clear();
                        continue;
                    }
                    if name_owned == b"Properties" && depth == ctx.item_depth + 1 {
                        // Direct Properties closing without a Label —
                        // synthesise one when the model has entries.
                        if !ctx.handled {
                            if let Some(entries) = ctx.entries.take() {
                                write_label_entries(&mut writer, &entries)?;
                                ctx.handled = true;
                            }
                        }
                        ctx.in_direct_properties = false;
                    }
                    if depth == ctx.item_depth && ITEM_KINDS.contains(&name_owned.as_slice()) {
                        // Item closing without any Properties at all —
                        // synthesise the whole block.
                        if !ctx.handled {
                            if let Some(entries) = ctx.entries.take() {
                                writer.write_event(Event::Start(BytesStart::new("Properties")))?;
                                write_label_entries(&mut writer, &entries)?;
                                writer.write_event(Event::End(
                                    quick_xml::events::BytesEnd::new("Properties"),
                                ))?;
                            }
                        }
                        label_ctx.pop();
                    }
                }
                if name_owned == b"Group" {
                    group_depth = group_depth.saturating_sub(1);
                    group_xforms.pop();
                }
                depth = depth.saturating_sub(1);
                writer.write_event(Event::End(e))?;
            }
            Event::Text(t) => {
                // Text inside a removed subtree (incl. the indentation
                // around it) vanishes with the element.
                if remove_depth != 0 {
                    buf.clear();
                    continue;
                }
                // Whitespace/indentation inside a buffered
                // `<PathPointArray>` rides with the buffered points so a
                // verbatim replay stays byte-exact.
                if let Some(ctx) = path_ctx.last_mut() {
                    if ctx.array_depth != 0 {
                        ctx.buffered.push(Event::Text(t.into_owned()));
                        buf.clear();
                        continue;
                    }
                }
                // Indentation between KVPs of a replaced Label drops
                // with the rest of the original Label body.
                if label_ctx.last().is_some_and(|c| c.in_label) {
                    buf.clear();
                    continue;
                }
                writer.write_event(Event::Text(t))?;
            }
            other => {
                // PIs / comments inside a removed subtree vanish too.
                if remove_depth != 0 {
                    buf.clear();
                    continue;
                }
                // Any other event inside a buffered array is foreign —
                // keep the original points (drop the rewrite) by leaving
                // the buffer intact and replaying it at array close.
                if let Some(ctx) = path_ctx.last_mut() {
                    if ctx.array_depth != 0 {
                        ctx.buffered.push(other.into_owned());
                        // Mark the parsed set as "do not rewrite" by
                        // poisoning it: a length mismatch vs the model
                        // contour forces verbatim. Simpler: clear geom so
                        // every contour of this item passes through.
                        ctx.geom = None;
                        buf.clear();
                        continue;
                    }
                }
                writer.write_event(other)?;
            }
        }
        buf.clear();
    }

    Ok(writer.into_inner().into_inner())
}

/// Decide whether to patch a page item's `ItemTransform`, and with what
/// value. `group_accum` is `None` for a top-level item (the model
/// transform IS the on-disk transform → patch it). For a group member it
/// is `Some(accumulated_group_transform)`; the on-disk transform is
/// recovered by inverting the group accumulation (W1.15 lane 4). When the
/// group transform is singular the recovery fails and the patch is
/// suppressed (the attribute passes through verbatim — a documented loss
/// for that degenerate case).
fn resolve_item_transform(
    group_accum: Option<Option<[f32; 6]>>,
    model_transform: Option<[f32; 6]>,
) -> (bool, Option<[f32; 6]>) {
    match group_accum {
        None => (true, model_transform),
        Some(accum) => match recover_member_transform(accum, model_transform) {
            Some(on_disk) => (true, on_disk),
            None => (false, model_transform),
        },
    }
}

/// If `e` is a page-item start tag whose `Self` matches a model item,
/// return the patched start tag. `None` ⇒ not a page item we patch
/// (caller emits the original verbatim). `group_accum` carries the
/// accumulated group transform for a member (see [`resolve_item_transform`]).
#[allow(clippy::too_many_arguments)]
fn patch_spread_item(
    e: &BytesStart,
    frames: &std::collections::HashMap<&str, &TextFrame>,
    rectangles: &[idml_import::Rectangle],
    ovals: &[idml_import::Oval],
    polygons: &[idml_import::Polygon],
    graphic_lines: &[idml_import::GraphicLine],
    group_accum: Option<Option<[f32; 6]>>,
) -> Result<Option<BytesStart<'static>>, quick_xml::Error> {
    let name = e.name();
    let self_id = attr_value(e, b"Self");
    let Some(self_id) = self_id else {
        return Ok(None);
    };

    match name.as_ref() {
        b"TextFrame" => {
            let Some(frame) = frames.get(self_id.as_str()) else {
                return Ok(None);
            };
            let (patch_tx, item_transform) =
                resolve_item_transform(group_accum, frame.item_transform);
            let fill = frame.fill_color.clone();
            let fill_tint = frame.fill_tint;
            let stroke = frame.stroke_color.clone();
            let stroke_weight = frame.stroke_weight;
            let next = frame.next_text_frame.clone();
            let nonprinting = frame.nonprinting;
            let bounds = frame.bounds;
            let start = patch_start(
                e,
                |k| {
                    frame_attr_patch(
                        k,
                        patch_tx,
                        item_transform,
                        &fill,
                        fill_tint,
                        &stroke,
                        stroke_weight,
                        Some(&next),
                        nonprinting,
                        bounds,
                        None,
                        None,
                    )
                },
                &frame_attr_extras(
                    patch_tx,
                    item_transform,
                    &fill,
                    &stroke,
                    stroke_weight,
                    next.as_deref(),
                    nonprinting,
                    None,
                    None,
                ),
            )?;
            Ok(Some(start.into_owned()))
        }
        b"Rectangle" => {
            let item = rectangles
                .iter()
                .find(|r| r.self_id.as_deref() == Some(self_id.as_str()));
            let (patch_tx, tx) =
                resolve_item_transform(group_accum, item.and_then(|r| r.item_transform));
            patch_vector_item(
                e,
                patch_tx,
                item.map(|r| VectorItem {
                    item_transform: tx,
                    fill_color: r.fill_color.clone(),
                    fill_tint: r.fill_tint,
                    stroke_color: r.stroke_color.clone(),
                    stroke_weight: r.stroke_weight,
                    nonprinting: r.nonprinting,
                    bounds: r.bounds,
                    start_arrow: None,
                    end_arrow: None,
                }),
            )
        }
        b"Oval" => {
            let item = ovals
                .iter()
                .find(|r| r.self_id.as_deref() == Some(self_id.as_str()));
            let (patch_tx, tx) =
                resolve_item_transform(group_accum, item.and_then(|r| r.item_transform));
            patch_vector_item(
                e,
                patch_tx,
                item.map(|r| VectorItem {
                    item_transform: tx,
                    fill_color: r.fill_color.clone(),
                    fill_tint: r.fill_tint,
                    stroke_color: r.stroke_color.clone(),
                    stroke_weight: r.stroke_weight,
                    nonprinting: r.nonprinting,
                    bounds: r.bounds,
                    start_arrow: None,
                    end_arrow: None,
                }),
            )
        }
        b"Polygon" => {
            let item = polygons
                .iter()
                .find(|r| r.self_id.as_deref() == Some(self_id.as_str()));
            let (patch_tx, tx) =
                resolve_item_transform(group_accum, item.and_then(|r| r.item_transform));
            patch_vector_item(
                e,
                patch_tx,
                item.map(|r| VectorItem {
                    item_transform: tx,
                    fill_color: r.fill_color.clone(),
                    fill_tint: r.fill_tint,
                    stroke_color: r.stroke_color.clone(),
                    stroke_weight: r.stroke_weight,
                    nonprinting: r.nonprinting,
                    bounds: r.bounds,
                    start_arrow: None,
                    end_arrow: None,
                }),
            )
        }
        b"GraphicLine" => {
            let item = graphic_lines
                .iter()
                .find(|r| r.self_id.as_deref() == Some(self_id.as_str()));
            let (patch_tx, tx) =
                resolve_item_transform(group_accum, item.and_then(|r| r.item_transform));
            patch_vector_item(
                e,
                patch_tx,
                item.map(|r| VectorItem {
                    item_transform: tx,
                    fill_color: None,
                    fill_tint: None,
                    stroke_color: r.stroke_color.clone(),
                    stroke_weight: r.stroke_weight,
                    nonprinting: r.nonprinting,
                    bounds: r.bounds,
                    start_arrow: Some(r.start_arrow),
                    end_arrow: Some(r.end_arrow),
                }),
            )
        }
        _ => Ok(None),
    }
}

/// The frame attributes shared by every page-item kind, lifted into one
/// shape so a single patch routine covers Rectangle / Oval / Polygon /
/// GraphicLine.
struct VectorItem {
    item_transform: Option<[f32; 6]>,
    fill_color: Option<String>,
    fill_tint: Option<f32>,
    stroke_color: Option<String>,
    stroke_weight: Option<f32>,
    nonprinting: bool,
    bounds: idml_import::Bounds,
    /// v43 — `LeftLineEnd` / `RightLineEnd`. `None` for the kinds that
    /// don't carry the fields (Rectangle / Oval / Polygon), so their
    /// source attributes pass through verbatim.
    start_arrow: Option<idml_import::ArrowheadType>,
    end_arrow: Option<idml_import::ArrowheadType>,
}

fn patch_vector_item(
    e: &BytesStart,
    patch_tx: bool,
    item: Option<VectorItem>,
) -> Result<Option<BytesStart<'static>>, quick_xml::Error> {
    let Some(item) = item else {
        return Ok(None);
    };
    let start = patch_start(
        e,
        |k| {
            frame_attr_patch(
                k,
                patch_tx,
                item.item_transform,
                &item.fill_color,
                item.fill_tint,
                &item.stroke_color,
                item.stroke_weight,
                None,
                item.nonprinting,
                item.bounds,
                item.start_arrow,
                item.end_arrow,
            )
        },
        &frame_attr_extras(
            patch_tx,
            item.item_transform,
            &item.fill_color,
            &item.stroke_color,
            item.stroke_weight,
            None,
            item.nonprinting,
            item.start_arrow,
            item.end_arrow,
        ),
    )?;
    Ok(Some(start.into_owned()))
}

/// Patch decision for one frame attribute key. `next` is `Some` only for
/// TextFrame (`NextTextFrame` lives there); `None` skips that key for
/// other kinds. Bounds patch only fires for a `GeometricBounds`
/// attribute that the source element already carries. `patch_tx` false
/// passes `ItemTransform` through verbatim (group member — see
/// [`rewrite_spread`]).
#[allow(clippy::too_many_arguments)]
fn frame_attr_patch(
    key: &[u8],
    patch_tx: bool,
    item_transform: Option<[f32; 6]>,
    fill: &Option<String>,
    fill_tint: Option<f32>,
    stroke: &Option<String>,
    stroke_weight: Option<f32>,
    next: Option<&Option<String>>,
    nonprinting: bool,
    bounds: idml_import::Bounds,
    start_arrow: Option<idml_import::ArrowheadType>,
    end_arrow: Option<idml_import::ArrowheadType>,
) -> Option<Patch> {
    match key {
        b"ItemTransform" if !patch_tx => None,
        b"ItemTransform" => Some(match item_transform {
            Some(m) => Patch::Set(format_matrix(&m)),
            None => Patch::Remove,
        }),
        b"FillColor" => Some(opt_string_patch(fill)),
        b"FillTint" => Some(opt_f32_patch(fill_tint)),
        b"StrokeColor" => Some(opt_string_patch(stroke)),
        b"StrokeWeight" => Some(opt_f32_patch(stroke_weight)),
        b"Nonprinting" => Some(if nonprinting {
            Patch::Set("true".to_string())
        } else {
            // The parser defaults absent → false; drop the attribute to
            // restore the implicit default rather than write "false".
            Patch::Remove
        }),
        b"NextTextFrame" => next.map(opt_string_patch),
        b"LeftLineEnd" => arrow_patch(start_arrow),
        b"RightLineEnd" => arrow_patch(end_arrow),
        b"GeometricBounds" => Some(Patch::Set(format!(
            "{} {} {} {}",
            format_f32(bounds.top),
            format_f32(bounds.left),
            format_f32(bounds.bottom),
            format_f32(bounds.right),
        ))),
        _ => None,
    }
}

/// Extras to append when a model attribute is set but the source element
/// didn't carry the key. Only emitted for genuinely-set values (so an
/// unmutated frame appends nothing and round-trips byte-identically).
/// `GeometricBounds` is intentionally NOT an extra: a path-geometry
/// frame's bounds are saved by rewriting its `<PathPointArray>` anchors
/// (see [`ModelGeometry`]), not by inventing a `GeometricBounds`
/// attribute the source never had.
#[allow(clippy::too_many_arguments)]
fn frame_attr_extras(
    patch_tx: bool,
    item_transform: Option<[f32; 6]>,
    fill: &Option<String>,
    stroke: &Option<String>,
    stroke_weight: Option<f32>,
    next: Option<&str>,
    nonprinting: bool,
    start_arrow: Option<idml_import::ArrowheadType>,
    end_arrow: Option<idml_import::ArrowheadType>,
) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if patch_tx {
        if let Some(m) = item_transform {
            out.push(("ItemTransform", format_matrix(&m)));
        }
    }
    if let Some(c) = fill {
        out.push(("FillColor", c.clone()));
    }
    if let Some(c) = stroke {
        out.push(("StrokeColor", c.clone()));
    }
    if let Some(w) = stroke_weight {
        out.push(("StrokeWeight", format_f32(w)));
    }
    if let Some(n) = next {
        out.push(("NextTextFrame", n.to_string()));
    }
    if nonprinting {
        out.push(("Nonprinting", "true".to_string()));
    }
    for (key, arrow) in [("LeftLineEnd", start_arrow), ("RightLineEnd", end_arrow)] {
        // `None` (the variant) is IDML's implicit default — absence of
        // the attribute restores it, so only drawable, representable
        // ends are appended.
        if let Some(t) = arrow {
            if t.draws() && !t.as_idml().is_empty() {
                out.push((key, t.as_idml().to_string()));
            }
        }
    }
    out
}

/// Patch decision for a `LeftLineEnd` / `RightLineEnd` attribute. The
/// kinds that don't carry the model fields pass `None` — their source
/// attribute survives verbatim. So does `Other` (an out-of-vocabulary
/// source token the parse layer couldn't keep): patching it would
/// clobber a spelling we can't reproduce.
fn arrow_patch(v: Option<idml_import::ArrowheadType>) -> Option<Patch> {
    use idml_import::ArrowheadType as A;
    match v {
        None | Some(A::Other) => None,
        Some(A::None) => Some(Patch::Remove),
        Some(t) => Some(Patch::Set(t.as_idml().to_string())),
    }
}

fn opt_string_patch(v: &Option<String>) -> Patch {
    match v {
        Some(s) => Patch::Set(s.clone()),
        None => Patch::Remove,
    }
}

fn opt_f32_patch(v: Option<f32>) -> Patch {
    match v {
        Some(n) => Patch::Set(format_f32(n)),
        None => Patch::Remove,
    }
}

// ---------------------------------------------------------------------
// Story rewrite
// ---------------------------------------------------------------------

/// Index every `<Table>` cell in the story by its `Self` id so a `<Cell
/// Self="...">` start tag in the XML can find its model counterpart
/// (W1.15 lane 3). Cells hang off `Paragraph::table.cells`. IDML DOES
/// allow a table nested inside a cell's paragraph, so this recurses into
/// every cell's nested table — otherwise the inner cells aren't matched
/// and their `AppliedParagraphStyle`/`AppliedCharacterStyle` drop on a
/// rewrite. A cell with no `Self` id (rare) is skipped — its content
/// keeps passing through verbatim.
fn collect_story_cells(story: &Story) -> std::collections::HashMap<&str, &TableCell> {
    let mut out: std::collections::HashMap<&str, &TableCell> = std::collections::HashMap::new();
    for p in &story.paragraphs {
        if let Some(table) = &p.table {
            collect_table_cells(table, &mut out);
        }
    }
    out
}

/// Collect a table's cells (by `Self`) and recurse into any table nested
/// in a cell's paragraph.
fn collect_table_cells<'a>(
    table: &'a idml_import::Table,
    out: &mut std::collections::HashMap<&'a str, &'a TableCell>,
) {
    for cell in &table.cells {
        if let Some(id) = cell.self_id.as_deref() {
            out.insert(id, cell);
        }
        for cp in &cell.paragraphs {
            if let Some(inner) = &cp.table {
                collect_table_cells(inner, out);
            }
        }
    }
}

/// Rewrite a `Story_*.xml` body so its `<ParagraphStyleRange>` /
/// `<CharacterStyleRange>` attributes + single-Content text reflect the
/// current model. Ranges are matched positionally (IDML carries no id on
/// them); the parser produced them in this same order.
pub fn rewrite_story(original: &[u8], story: &Story) -> Result<Vec<u8>, quick_xml::Error> {
    let mut reader = Reader::from_reader(original);
    let config = reader.config_mut();
    config.expand_empty_elements = false;
    config.trim_text(false);

    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut buf = Vec::new();

    // Positional cursors into the model.
    let mut para_idx: isize = -1;
    let mut run_idx: isize = -1;
    // The run currently open (for Content text + attribute patching).
    let mut current_run: Option<&CharacterRun> = None;
    // Buffered inline body of the open `<CharacterStyleRange>`. The
    // parser collapses a run's `<Content>A</Content><Br/><Content>B
    // </Content>` (and `<Tab/>` between segments) into one run string
    // with `\n` / `\t` separators, so a faithful save has to split the
    // model text back across that Content/Br/Tab structure — not just
    // patch a single Content. We buffer the whole contiguous inline
    // region (it is always the LAST thing in a run; `<Properties>` and
    // anchored frames come first and stream out immediately) so the
    // replace-or-passthrough decision can be made once at the run's
    // close, when the full reconstructed text is known. See
    // [`RunBody`].
    let mut body = RunBody::default();
    // Depth of open `<Table>` elements. Inside a table the
    // `<ParagraphStyleRange>` / `<CharacterStyleRange>` belong to CELL
    // paragraphs, which the parser stores on `paragraph.table.cells[]`,
    // NOT on the story's top-level `paragraphs`. Patching them against
    // `story.paragraphs` would misalign, so the story-level cursors do
    // NOT advance inside a table.
    let mut table_depth: usize = 0;

    // W1.8 — depth of open `<Footnote>` elements. A footnote is a
    // self-contained paragraph stream anchored mid-run; the parser keeps
    // its body on `paragraph.footnotes[].paragraphs`, NOT on the story's
    // top-level `paragraphs` (see `idml_import::story`'s footnote stack).
    // So the story-level positional cursors must NOT advance inside a
    // footnote, and the footnote's own `<ParagraphStyleRange>` /
    // `<CharacterStyleRange>` / `<Content>` must NOT patch against the
    // host story. While `footnote_depth > 0` the entire subtree is
    // treated as opaque inline markup of the *host* run: it buffers into
    // the open `RunBody` as foreign (so the host run replays verbatim and
    // never rewrites over the anchor) and the matching `</Footnote>`
    // restores normal flow. Without this guard the footnote's inner
    // ranges escaped the buffer, advanced the cursors, and left the host
    // run's `<Content>` + `<Footnote>` open tag dropped — yielding a
    // mismatched `</Footnote>` and a re-parse failure (zero pages).
    let mut footnote_depth: usize = 0;

    // W1.15 lane 3 — table-cell text write-back. Inside a `<Cell
    // Self="...">` the `<ParagraphStyleRange>` / `<CharacterStyleRange>`
    // patch against the matched model `TableCell.paragraphs[]` with
    // cell-local positional cursors (reset on each `<Cell>` open). When
    // a cell has no model match — or the cell text is unchanged — its
    // content passes through verbatim, exactly as before.
    let cells = collect_story_cells(story);
    let mut current_cell: Option<&TableCell> = None;
    let mut cell_depth: usize = 0; // depth of the open `<Cell>`, or 0
    let mut cell_para_idx: isize = -1;
    let mut cell_run_idx: isize = -1;
    // Nested tables (a table in a cell's paragraph) nest `<Cell>`s, so the
    // cell-local cursor state is a stack: each `<Cell>` open parks the
    // enclosing cell's state, each `</Cell>` restores it.
    type CellFrame<'a> = (Option<&'a TableCell>, usize, isize, isize);
    let mut cell_stack: Vec<CellFrame> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) => {
                // True while patching cell content: inside a `<Cell>`
                // that matched a model cell. The ranges then resolve
                // against the cell's paragraphs with cell-local cursors.
                let in_cell = table_depth > 0 && current_cell.is_some();
                match e.name().as_ref() {
                    b"Footnote" => {
                        // Enter a footnote: its body is a separate stream
                        // the host story doesn't model. Buffer the whole
                        // subtree into the open host run as foreign inline
                        // markup so it replays verbatim and the host run's
                        // text is never rewritten over the anchor. Activate
                        // the body if the footnote leads the run (no prior
                        // `<Content>`), so the buffer captures it.
                        footnote_depth += 1;
                        body.active = true;
                        body.foreign = true;
                        body.events.push(Event::Start(e.into_owned()));
                    }
                    // Inside a footnote, every element is opaque host-run
                    // markup — buffer it, don't patch it against the story.
                    _ if footnote_depth > 0 => {
                        body.events.push(Event::Start(e.into_owned()));
                    }
                    b"Table" => {
                        table_depth += 1;
                        writer.write_event(Event::Start(e.into_owned()))?;
                    }
                    b"Cell" if table_depth > 0 => {
                        // Enter a cell — park the enclosing cell's cursors
                        // (nested tables nest cells) then bind this cell's
                        // model counterpart (by `Self`) + reset the
                        // cell-local cursors. The start tag passes through
                        // verbatim (cell-level attributes patched elsewhere).
                        cell_stack.push((current_cell, cell_depth, cell_para_idx, cell_run_idx));
                        cell_depth = table_depth;
                        cell_para_idx = -1;
                        cell_run_idx = -1;
                        current_cell =
                            attr_value(&e, b"Self").and_then(|id| cells.get(id.as_str()).copied());
                        writer.write_event(Event::Start(e.into_owned()))?;
                    }
                    b"ParagraphStyleRange" if table_depth == 0 => {
                        para_idx += 1;
                        run_idx = -1;
                        let para = story.paragraphs.get(para_idx as usize);
                        let start = patch_paragraph_range(&e, para)?;
                        writer.write_event(Event::Start(start))?;
                    }
                    b"ParagraphStyleRange" if in_cell => {
                        cell_para_idx += 1;
                        cell_run_idx = -1;
                        let para =
                            current_cell.and_then(|c| c.paragraphs.get(cell_para_idx as usize));
                        let start = patch_paragraph_range(&e, para)?;
                        writer.write_event(Event::Start(start))?;
                    }
                    b"CharacterStyleRange" if table_depth == 0 => {
                        run_idx += 1;
                        current_run = story
                            .paragraphs
                            .get(para_idx as usize)
                            .and_then(|p| p.runs.get(run_idx as usize));
                        body = RunBody::default();
                        let start = patch_character_range(&e, current_run)?;
                        writer.write_event(Event::Start(start))?;
                    }
                    b"CharacterStyleRange" if in_cell => {
                        cell_run_idx += 1;
                        current_run = current_cell
                            .and_then(|c| c.paragraphs.get(cell_para_idx as usize))
                            .and_then(|p| p.runs.get(cell_run_idx as usize));
                        body = RunBody::default();
                        let start = patch_character_range(&e, current_run)?;
                        writer.write_event(Event::Start(start))?;
                    }
                    b"Content" if table_depth == 0 || in_cell => {
                        // A `<Content>` opens the inline body region (or
                        // continues it). Buffer the start; the text /
                        // entities inside accumulate into the body, and
                        // the matching End is buffered too. Once any
                        // inline leaf appears, every later event in the
                        // run buffers (foreign markup flips the guard).
                        body.active = true;
                        body.in_content = true;
                        body.events.push(Event::Start(e.into_owned()));
                    }
                    _ => {
                        if body.active {
                            // A non-inline element opened inside the
                            // buffered region (e.g. an unexpected child
                            // of `<Content>`). Never rewrite over it.
                            body.foreign = true;
                            body.events.push(Event::Start(e.into_owned()));
                        } else {
                            writer.write_event(Event::Start(e.into_owned()))?;
                        }
                    }
                }
            }
            Event::Empty(e) => {
                let in_cell = table_depth > 0 && current_cell.is_some();
                // Inside a footnote every empty element is opaque host-run
                // markup (a footnote anchor's own `<Br/>` etc.) — buffer it
                // so it replays verbatim and never advances the story
                // cursors. A self-closing `<Footnote/>` (no body) opens and
                // closes in one event, so it never changes `footnote_depth`.
                if footnote_depth > 0 {
                    body.events.push(Event::Empty(e.into_owned()));
                    buf.clear();
                    continue;
                }
                // A self-closing CharacterStyleRange / ParagraphStyleRange
                // still advances the positional cursor + patches attrs.
                match e.name().as_ref() {
                    b"ParagraphStyleRange" if table_depth == 0 => {
                        para_idx += 1;
                        run_idx = -1;
                        let para = story.paragraphs.get(para_idx as usize);
                        let start = patch_paragraph_range(&e, para)?;
                        writer.write_event(Event::Empty(start))?;
                    }
                    b"ParagraphStyleRange" if in_cell => {
                        cell_para_idx += 1;
                        cell_run_idx = -1;
                        let para =
                            current_cell.and_then(|c| c.paragraphs.get(cell_para_idx as usize));
                        let start = patch_paragraph_range(&e, para)?;
                        writer.write_event(Event::Empty(start))?;
                    }
                    b"CharacterStyleRange" if table_depth == 0 => {
                        run_idx += 1;
                        current_run = None;
                        body = RunBody::default();
                        let run = story
                            .paragraphs
                            .get(para_idx as usize)
                            .and_then(|p| p.runs.get(run_idx as usize));
                        let start = patch_character_range(&e, run)?;
                        writer.write_event(Event::Empty(start))?;
                    }
                    b"CharacterStyleRange" if in_cell => {
                        cell_run_idx += 1;
                        current_run = None;
                        body = RunBody::default();
                        let run = current_cell
                            .and_then(|c| c.paragraphs.get(cell_para_idx as usize))
                            .and_then(|p| p.runs.get(cell_run_idx as usize));
                        let start = patch_character_range(&e, run)?;
                        writer.write_event(Event::Empty(start))?;
                    }
                    b"Br" if (table_depth == 0 || in_cell) && !body.in_content => {
                        // `<Br/>` is an inline leaf → `\n` in the parser's
                        // run text. It opens (or continues) the body
                        // region — a run can start with `\n` (a leading
                        // `<Br/>` before the first `<Content>`). Mirror
                        // the newline so the split survives a rewrite.
                        body.active = true;
                        body.text.push('\n');
                        body.events.push(Event::Empty(e.into_owned()));
                    }
                    b"Tab" if (table_depth == 0 || in_cell) && !body.in_content => {
                        // `<Tab/>` is an inline leaf → `\t`. Opens or
                        // continues the body region (see `<Br/>`).
                        body.active = true;
                        body.text.push('\t');
                        body.events.push(Event::Empty(e.into_owned()));
                    }
                    _ => {
                        if body.active {
                            // An empty element inside the span (PI-like
                            // marker, anchored frame, unknown) — never
                            // rewrite over it.
                            body.foreign = true;
                            body.events.push(Event::Empty(e.into_owned()));
                        } else {
                            writer.write_event(Event::Empty(e.into_owned()))?;
                        }
                    }
                }
            }
            Event::Text(t) => {
                if body.active && body.in_content {
                    // Buffer — the replace decision happens at the run
                    // close once the whole (possibly entity-split) span
                    // is known.
                    let decoded = t.decode().unwrap_or_default();
                    let orig = quick_xml::escape::unescape(&decoded)
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| decoded.into_owned());
                    body.text.push_str(&orig);
                    body.events.push(Event::Text(t.into_owned()));
                } else if body.active {
                    // Indentation/whitespace between inline leaves —
                    // buffer it so a verbatim replay stays byte-exact.
                    body.events.push(Event::Text(t.into_owned()));
                } else {
                    writer.write_event(Event::Text(t))?;
                }
            }
            Event::GeneralRef(r) => {
                if body.active && body.in_content {
                    // Resolve the reference (predefined five + numeric)
                    // so the comparison sees the parsed run's chars.
                    let name = String::from_utf8_lossy(r.as_ref()).into_owned();
                    let resolved = quick_xml::escape::unescape(&format!("&{name};"))
                        .map(|c| c.into_owned())
                        .unwrap_or_default();
                    if resolved.is_empty() {
                        // Unknown entity — never rewrite over it.
                        body.foreign = true;
                    }
                    body.text.push_str(&resolved);
                    body.events.push(Event::GeneralRef(r.into_owned()));
                } else if body.active {
                    body.foreign = true;
                    body.events.push(Event::GeneralRef(r.into_owned()));
                } else {
                    writer.write_event(Event::GeneralRef(r))?;
                }
            }
            Event::End(e) => {
                // Inside a footnote every End buffers into the host run
                // (foreign) so the subtree replays verbatim; the matching
                // `</Footnote>` (when depth returns to 0) restores normal
                // flow. The inner `</CharacterStyleRange>` must NOT trigger
                // a host-run flush, and the inner `</ParagraphStyleRange>`
                // must NOT touch the story cursors.
                if e.name().as_ref() == b"Footnote" {
                    footnote_depth = footnote_depth.saturating_sub(1);
                    body.events.push(Event::End(e.into_owned()));
                    buf.clear();
                    continue;
                }
                if footnote_depth > 0 {
                    body.events.push(Event::End(e.into_owned()));
                    buf.clear();
                    continue;
                }
                match e.name().as_ref() {
                    b"Table" => table_depth = table_depth.saturating_sub(1),
                    b"Cell" if cell_depth != 0 && table_depth == cell_depth => {
                        // Leave the cell — restore the enclosing cell's
                        // cursors (a nested table's cell pops back to its
                        // host cell; a top-level cell pops back to None) so
                        // siblings + post-table markup patch correctly.
                        let (cc, cd, cp, cr) = cell_stack.pop().unwrap_or((None, 0, -1, -1));
                        current_cell = cc;
                        cell_depth = cd;
                        cell_para_idx = cp;
                        cell_run_idx = cr;
                    }
                    b"Content" if body.active => {
                        body.in_content = false;
                        body.events.push(Event::End(e.into_owned()));
                        continue; // already buffered + advanced
                    }
                    b"CharacterStyleRange" => {
                        flush_run_body(&mut writer, &mut body, current_run)?;
                        current_run = None;
                    }
                    _ => {}
                }
                writer.write_event(Event::End(e))?;
            }
            other => {
                if body.active {
                    // PI (e.g. InDesign's <?ACE 18?> marker) or other
                    // markup inside the span — buffer in order and
                    // never rewrite over it.
                    body.foreign = true;
                    body.events.push(other.into_owned());
                } else {
                    writer.write_event(other)?;
                }
            }
        }
        buf.clear();
    }

    Ok(writer.into_inner().into_inner())
}

/// Buffered inline body (`<Content>` / `<Br/>` / `<Tab/>` leaves) of one
/// open `<CharacterStyleRange>`. The decision to rewrite the run's text
/// — possibly across several `<Content>` segments — can only be made at
/// the run's close, once the whole reconstructed string is known. Until
/// then every inline event is buffered here in document order so an
/// unchanged run (or one with foreign markup) can be replayed
/// byte-for-byte.
#[derive(Default)]
struct RunBody {
    /// True once the first inline leaf has been seen — from that point
    /// every event in the run buffers rather than streaming out.
    active: bool,
    /// True while inside a `<Content>` element (its text accumulates).
    in_content: bool,
    /// Reconstructed run text: Content text verbatim, `\n` per `<Br/>`,
    /// `\t` per `<Tab/>` — exactly how the parser collapses the run.
    text: String,
    /// Any markup the rewrite must not clobber appeared in the body (a
    /// PI / ACE page-number marker, an anchored frame, a TextVariable
    /// instance, an unknown entity, …). When set, the body replays
    /// verbatim regardless of the model text.
    foreign: bool,
    /// Buffered events, in document order.
    events: Vec<Event<'static>>,
}

/// Emit the buffered inline body of a closing run. When the model text
/// diverged from the reconstructed source AND the body is pure
/// Content/Br/Tab (no foreign markup to preserve), re-serialise the
/// model text across the Content/Br/Tab structure (mirroring
/// `paged_gen`'s `write_run_content`: `\n` → `<Br/>`, `\t` → `<Tab/>`,
/// runs of plain text → `<Content>…</Content>`). Otherwise replay the
/// original events so an unchanged run — or one carrying markers — stays
/// byte-identical.
fn flush_run_body(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    body: &mut RunBody,
    run: Option<&CharacterRun>,
) -> Result<(), quick_xml::Error> {
    if !body.active {
        return Ok(());
    }
    let replace = match run {
        Some(r) => r.text != body.text && !body.foreign,
        None => false,
    };
    if replace {
        write_run_content(writer, &run.expect("checked above").text)?;
    } else {
        for ev in body.events.drain(..) {
            writer.write_event(ev)?;
        }
    }
    body.active = false;
    body.in_content = false;
    body.events.clear();
    Ok(())
}

/// Serialise a run's text body back into IDML `<Content>` / `<Br/>` /
/// `<Tab/>` structure, byte-for-byte matching `paged_gen`'s emitter so
/// a saved edit re-parses to the same model. Empty text emits an empty
/// `<Content></Content>` (the IDML form for a zero-length run).
pub(crate) fn write_run_content(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    text: &str,
) -> Result<(), quick_xml::Error> {
    fn flush(
        writer: &mut Writer<Cursor<Vec<u8>>>,
        buf: &mut String,
    ) -> Result<(), quick_xml::Error> {
        if !buf.is_empty() {
            writer.write_event(Event::Start(BytesStart::new("Content")))?;
            writer.write_event(Event::Text(BytesText::new(buf)))?;
            writer.write_event(Event::End(quick_xml::events::BytesEnd::new("Content")))?;
            buf.clear();
        }
        Ok(())
    }
    if text.is_empty() {
        writer.write_event(Event::Start(BytesStart::new("Content")))?;
        writer.write_event(Event::End(quick_xml::events::BytesEnd::new("Content")))?;
        return Ok(());
    }
    let mut buf = String::new();
    for ch in text.chars() {
        match ch {
            '\t' => {
                flush(writer, &mut buf)?;
                writer.write_event(Event::Empty(BytesStart::new("Tab")))?;
            }
            '\n' => {
                flush(writer, &mut buf)?;
                writer.write_event(Event::Empty(BytesStart::new("Br")))?;
            }
            _ => buf.push(ch),
        }
    }
    flush(writer, &mut buf)?;
    Ok(())
}

fn patch_paragraph_range(
    e: &BytesStart,
    para: Option<&idml_import::Paragraph>,
) -> Result<BytesStart<'static>, quick_xml::Error> {
    let style = para.and_then(|p| p.paragraph_style.clone());
    let extras: Vec<(&str, String)> = match &style {
        Some(s) => vec![("AppliedParagraphStyle", s.clone())],
        None => Vec::new(),
    };
    let start = patch_start(
        e,
        |k| match k {
            b"AppliedParagraphStyle" => Some(opt_string_patch(&style)),
            _ => None,
        },
        &extras,
    )?;
    Ok(start.into_owned())
}

fn patch_character_range(
    e: &BytesStart,
    run: Option<&CharacterRun>,
) -> Result<BytesStart<'static>, quick_xml::Error> {
    let Some(run) = run else {
        // No model run aligns with this range — pass through verbatim.
        return Ok(e.clone().into_owned());
    };
    let r = run.clone();
    let extras = character_extras(&r);
    let start = patch_start(e, |k| character_attr_patch(k, &r), &extras)?;
    Ok(start.into_owned())
}

/// Patch decision for one `<CharacterStyleRange>` attribute. Covers the
/// character paths the mutation surface writes.
fn character_attr_patch(key: &[u8], r: &CharacterRun) -> Option<Patch> {
    match key {
        b"AppliedCharacterStyle" => Some(opt_string_patch(&r.character_style)),
        b"AppliedFont" => Some(opt_string_patch(&r.font)),
        b"FontStyle" => Some(opt_string_patch(&r.font_style)),
        b"PointSize" => Some(opt_f32_patch(r.point_size)),
        b"FillColor" => Some(opt_string_patch(&r.fill_color)),
        b"FillTint" => Some(opt_f32_patch(r.fill_tint)),
        b"StrokeColor" => Some(opt_string_patch(&r.stroke_color)),
        b"StrokeWeight" => Some(opt_f32_patch(r.stroke_weight)),
        b"Leading" => Some(opt_f32_patch(r.leading)),
        b"Tracking" => Some(opt_f32_patch(r.tracking)),
        b"BaselineShift" => Some(opt_f32_patch(r.baseline_shift)),
        b"HorizontalScale" => Some(opt_f32_patch(r.horizontal_scale)),
        b"VerticalScale" => Some(opt_f32_patch(r.vertical_scale)),
        b"Skew" => Some(opt_f32_patch(r.skew)),
        b"Capitalization" => Some(opt_string_patch(&r.capitalization)),
        b"Position" => Some(opt_string_patch(&r.position)),
        b"KerningMethod" => Some(opt_string_patch(&r.kerning_method)),
        b"AppliedLanguage" => Some(opt_string_patch(&r.applied_language)),
        b"Underline" => Some(opt_bool_patch(r.underline)),
        b"StrikeThru" => Some(opt_bool_patch(r.strikethru)),
        b"Ligatures" => Some(opt_bool_patch(r.ligatures_on)),
        _ => None,
    }
}

/// Newly-set character attributes to append when absent from the source.
/// Only the high-frequency authoring fields are appended; the rest patch
/// in place when present but aren't invented (keeps unmutated round-trips
/// byte-identical and avoids spraying defaults).
fn character_extras(r: &CharacterRun) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(s) = &r.fill_color {
        out.push(("FillColor", s.clone()));
    }
    if let Some(sz) = r.point_size {
        out.push(("PointSize", format_f32(sz)));
    }
    if let Some(s) = &r.character_style {
        out.push(("AppliedCharacterStyle", s.clone()));
    }
    out
}

fn opt_bool_patch(v: Option<bool>) -> Patch {
    match v {
        Some(b) => Patch::Set(b.to_string()),
        None => Patch::Remove,
    }
}

/// Read an attribute's decoded value off a start tag.
fn attr_value(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|Attribute { value, .. }| std::str::from_utf8(&value).ok().map(|s| s.to_string()))
}
