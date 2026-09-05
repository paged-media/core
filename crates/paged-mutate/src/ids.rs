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

//! The ONE `u<hex>` number line every minted id draws from.
//!
//! InDesign spells a document's `Self` ids as `u<hex>` off a single
//! document-wide counter: layers, spreads, pages, stories, page items,
//! tables, anchored frames — and, under a kind prefix, the hyperlink
//! trio (`Hyperlink/u…`, `HyperlinkTextSource/u…`,
//! `HyperlinkURLDestination/u…`), bookmarks and cross-references. The
//! canvas mints the same way: one successor of the highest number
//! present, spelled bare for a page item / table / anchored frame and
//! under the three kind prefixes for a link.
//!
//! What a minter must therefore start past is the highest number of
//! ANY of those kinds, not of the kind it happens to be minting. A scan
//! that looked at the six page-item vectors only could not see a
//! hyperlink or a table it had minted a moment earlier — each of those
//! lands in the designmap or a story paragraph — and handed the same
//! successor out again: a real document came back holding
//! `Hyperlink/ueef094` twice with a table `ueef094` beside them.
//! [`highest_u_hex_id`] is the one floor, shared by the canvas's
//! translation-time minter and the applier's `mint_group_id` twin.
//!
//! Out of scope, by design: the minter-owned decimal namespaces
//! (`Story/u<n>`, `Section/u<n>`, `Color/u<n>`) and the positional
//! guide ids (`Guide/<spread>/<index>`) — each is numbered against its
//! own collection and never spelled from this number line.

use paged_scene::Document;

/// The kind prefixes whose `u<hex>` suffix shares the document counter:
/// what the canvas minter produces from one base for a link, plus the
/// sibling kinds InDesign numbers the same way.
const SHARED_KIND_PREFIXES: &[&str] = &[
    "Hyperlink",
    "HyperlinkTextSource",
    "HyperlinkURLDestination",
    "HyperlinkPageDestination",
    "HyperlinkTextDestination",
    "Bookmark",
    "CrossReferenceSource",
];

/// The number an id was minted from, when it is spelled on the shared
/// line: a bare `u<hex>`, or `<Kind>/u<hex>` for a kind in
/// [`SHARED_KIND_PREFIXES`]. `None` for every other spelling — the
/// parser's non-hex fixture ids, and the prefixed decimal namespaces
/// this line does not own.
pub fn u_hex_number(id: &str) -> Option<u64> {
    let bare = match id.rsplit_once('/') {
        Some((kind, rest)) => {
            if !SHARED_KIND_PREFIXES.contains(&kind) {
                return None;
            }
            rest
        }
        None => id,
    };
    let hex = bare.strip_prefix('u')?;
    if hex.is_empty() || hex.len() > 12 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

/// Raise `max` to `id`'s number when `id` is on the shared line.
pub fn scan_u_hex_id(max: &mut u64, id: Option<&str>) {
    if let Some(n) = id.and_then(u_hex_number) {
        *max = (*max).max(n);
    }
}

/// The highest `u<hex>` number the document holds, across every kind
/// on the shared line; `0` for a document with none. A fresh mint is
/// `u<this + 1>` (plus whatever a batch has already minted ahead of
/// it, which is the canvas minter's `offset`).
pub fn highest_u_hex_id(doc: &Document) -> u64 {
    let mut max: u64 = 0;
    for parsed in &doc.spreads {
        scan_spread(&mut max, &parsed.spread);
    }
    for master in doc.master_spreads.values() {
        scan_u_hex_id(&mut max, Some(&master.self_id));
        scan_spread(&mut max, &master.spread);
    }
    for story in &doc.stories {
        scan_u_hex_id(&mut max, Some(&story.self_id));
        for para in &story.story.paragraphs {
            if let Some(table) = &para.table {
                scan_u_hex_id(&mut max, table.self_id.as_deref());
            }
            for anchored in &para.anchored_frames {
                scan_anchored(&mut max, anchored);
            }
        }
    }
    let dm = &doc.designmap;
    for layer in &dm.layers {
        scan_u_hex_id(&mut max, Some(&layer.self_id));
    }
    for section in &dm.sections {
        scan_u_hex_id(&mut max, Some(&section.self_id));
    }
    for link in &dm.hyperlinks {
        scan_u_hex_id(&mut max, Some(&link.self_id));
        scan_u_hex_id(&mut max, link.source.as_deref());
        scan_u_hex_id(&mut max, link.destination.as_deref());
    }
    for dest in &dm.hyperlink_destinations {
        scan_u_hex_id(&mut max, Some(&dest.self_id));
    }
    for bookmark in &dm.bookmarks {
        scan_u_hex_id(&mut max, Some(&bookmark.self_id));
        scan_u_hex_id(&mut max, bookmark.destination.as_deref());
    }
    for xref in &dm.cross_references {
        scan_u_hex_id(&mut max, Some(&xref.self_id));
        scan_u_hex_id(&mut max, xref.destination.as_deref());
    }
    max
}

fn scan_spread(max: &mut u64, s: &paged_model::Spread) {
    scan_u_hex_id(max, s.self_id.as_deref());
    for p in &s.pages {
        scan_u_hex_id(max, p.self_id.as_deref());
    }
    for f in &s.text_frames {
        scan_u_hex_id(max, f.self_id.as_deref());
    }
    for r in &s.rectangles {
        scan_u_hex_id(max, r.self_id.as_deref());
    }
    for o in &s.ovals {
        scan_u_hex_id(max, o.self_id.as_deref());
    }
    for l in &s.graphic_lines {
        scan_u_hex_id(max, l.self_id.as_deref());
    }
    for p in &s.polygons {
        scan_u_hex_id(max, p.self_id.as_deref());
    }
    for g in &s.groups {
        scan_u_hex_id(max, g.self_id.as_deref());
    }
}

fn scan_anchored(max: &mut u64, a: &paged_model::AnchoredFrame) {
    scan_u_hex_id(max, a.self_id.as_deref());
    for child in &a.children {
        scan_anchored(max, child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_and_shared_kind_spellings_are_on_the_line() {
        assert_eq!(u_hex_number("u1f"), Some(0x1f));
        assert_eq!(u_hex_number("ueef094"), Some(0xeef094));
        assert_eq!(u_hex_number("Hyperlink/u20"), Some(0x20));
        assert_eq!(u_hex_number("HyperlinkTextSource/u20"), Some(0x20));
        assert_eq!(u_hex_number("HyperlinkURLDestination/u20"), Some(0x20));
        assert_eq!(u_hex_number("Bookmark/u3a"), Some(0x3a));
    }

    #[test]
    fn other_spellings_are_not() {
        // The parser's fixture ids and the kinds with their own line.
        assert_eq!(u_hex_number("tf1"), None);
        assert_eq!(u_hex_number("s0"), None);
        assert_eq!(u_hex_number("u"), None);
        assert_eq!(u_hex_number("u2c1i0"), None, "a cell id is not pure hex");
        assert_eq!(
            u_hex_number("Story/u7"),
            None,
            "the story minter's decimal line"
        );
        assert_eq!(u_hex_number("Section/u1"), None);
        assert_eq!(u_hex_number("Color/u3"), None);
        assert_eq!(u_hex_number("Guide/s1/0"), None);
        assert_eq!(u_hex_number("ParagraphStyle/u9"), None);
    }
}
