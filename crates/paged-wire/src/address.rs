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

//! One address for every object in a document (ADR 131).
//!
//! [`ElementId`] names the six page-item kinds, a story range, a table and
//! a cell. Everything else a surface wants to point at — the document, a
//! page, a spread, a layer, a master, a story, a style, a swatch, a field,
//! an object a plugin owns — had no name, and table and cell had no string
//! form. [`Address`] is the one spelling for all of them. `parse` accepts
//! the canonical form and the aliases; `Display` writes the canonical form;
//! the two round-trip.
//!
//! ## Grammar (canonical first; aliases parse and are never emitted)
//!
//! ```text
//! doc
//! textFrame:<id>  rectangle:<id>  oval:<id>  polygon:<id>  graphicLine:<id>  group:<id>
//!                 aliases: textframe: rect: graphicline:
//! frame:<id>      a page item of unknown kind — resolved by a document, never emitted
//! page:<id>       page:#<n>   (n-th body page, 1-based, document order)
//! spread:<id>  layer:<id>  master:<id>  story:<id>
//! storyRange:<storyId>@<start>..<end>    alias: story:<storyId>@<start>..<end>, storyrange:
//! field:<storyId>@<offset>               a field anchored at a story character offset
//! table:<tableId>                        the table's Self, unique per document
//! cell:<tableId>/<row>,<col>             zero-based
//! style:<family>/<selfId or name>        family = paragraph|character|object|cell|table
//! swatch:<rest>  gradient:<rest>  colorGroup:<rest>  numberingList:<rest>
//! condition:<rest>  conditionSet:<rest>
//! guide:<id>  section:<id>  hyperlink:<id>
//! master:<id> > <item address>           an item on a master spread
//! plugin:<pluginId>/<kind>/<rest>        opaque to core, carried through
//! $h:<name>                              a batch handle
//! ```
//!
//! - `<rest>` runs to the end of the address and may contain anything, so
//!   self ids such as `Color/Red` or `ParagraphStyle/Body Text` need no
//!   escaping. A plain `<id>` does too: everything after the first `:` is
//!   the id, exactly as `ElementId::parse` has always read it.
//! - `%` escapes only where an id must stop at a delimiter: the `@` ending
//!   a story id in `storyRange:`/`field:`, the `/` ending a table id in
//!   `cell:` and the plugin id / kind in `plugin:`, and the ` > ` ending
//!   the master in a master-scoped item. The formatter escapes exactly
//!   those (and `%` itself) in exactly those places.
//! - An address is a SESSION handle. InDesign renumbers every `Self` on
//!   save, so anything stored in a document addresses by name or label
//!   (a selector), never by a raw `Self`.

use serde::{Deserialize, Serialize};
use tsify_next::Tsify;

use crate::ElementId;

/// A style family — which of the document's five style tables a
/// `style:` address names.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Tsify,
)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub enum StyleFamily {
    Paragraph,
    Character,
    Object,
    Cell,
    Table,
}

impl StyleFamily {
    pub const ALL: [StyleFamily; 5] = [
        StyleFamily::Paragraph,
        StyleFamily::Character,
        StyleFamily::Object,
        StyleFamily::Cell,
        StyleFamily::Table,
    ];

    /// The spelling in an address (`style:<this>/…`).
    pub fn as_str(self) -> &'static str {
        match self {
            StyleFamily::Paragraph => "paragraph",
            StyleFamily::Character => "character",
            StyleFamily::Object => "object",
            StyleFamily::Cell => "cell",
            StyleFamily::Table => "table",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == s)
    }
}

/// Every object in a document, by one name. See the module docs for the
/// string grammar, which is the canonical form; this enum is its typed
/// mirror on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Address {
    Doc,
    TextFrame {
        id: String,
    },
    Rectangle {
        id: String,
    },
    Oval {
        id: String,
    },
    Polygon {
        id: String,
    },
    GraphicLine {
        id: String,
    },
    Group {
        id: String,
    },
    /// A page item whose kind is not known yet (`frame:<id>`). Resolved
    /// against a document; never handed out by core.
    Frame {
        id: String,
    },
    Page {
        id: String,
    },
    /// `page:#<n>` — the n-th body page, 1-based, in document order.
    PageNumber {
        n: u32,
    },
    Spread {
        id: String,
    },
    Layer {
        id: String,
    },
    Master {
        id: String,
    },
    Story {
        id: String,
    },
    #[serde(rename_all = "camelCase")]
    StoryRange {
        story_id: String,
        start: u32,
        end: u32,
    },
    /// A text field anchored at a story character offset.
    #[serde(rename_all = "camelCase")]
    Field {
        story_id: String,
        offset: u32,
    },
    /// A table by its `Self`, which is unique per document. The owning
    /// story is found by the resolver.
    Table {
        id: String,
    },
    #[serde(rename_all = "camelCase")]
    Cell {
        table_id: String,
        row: u32,
        col: u32,
    },
    /// A style by `Self` id (or by display name; the resolver tries the
    /// id first, and every address core hands out carries the id).
    Style {
        family: StyleFamily,
        id: String,
    },
    Swatch {
        id: String,
    },
    Gradient {
        id: String,
    },
    ColorGroup {
        id: String,
    },
    NumberingList {
        id: String,
    },
    Guide {
        id: String,
    },
    Condition {
        id: String,
    },
    ConditionSet {
        id: String,
    },
    Section {
        id: String,
    },
    Hyperlink {
        id: String,
    },
    /// `master:<id> > <item>` — an item on a master spread.
    OnMaster {
        master: String,
        item: Box<Address>,
    },
    /// An object a plugin owns. Opaque to core.
    #[serde(rename_all = "camelCase")]
    Plugin {
        plugin_id: String,
        object_kind: String,
        id: String,
    },
    /// `$h:<name>` — a batch handle bound earlier in the same batch.
    Handle {
        name: String,
    },
}

/// The kind of an [`Address`], without its payload — what a selector's
/// kind step names, and the key the published id grammar is generated
/// from.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Tsify,
)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub enum AddressKind {
    Doc,
    TextFrame,
    Rectangle,
    Oval,
    Polygon,
    GraphicLine,
    Group,
    Frame,
    Page,
    Spread,
    Layer,
    Master,
    Story,
    StoryRange,
    Field,
    Table,
    Cell,
    Style,
    Swatch,
    Gradient,
    ColorGroup,
    NumberingList,
    Guide,
    Condition,
    ConditionSet,
    Section,
    Hyperlink,
    Plugin,
    Handle,
}

/// One published address form: what the catalog's `idGrammar` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressForm {
    pub kind: AddressKind,
    pub form: &'static str,
    pub example: &'static str,
    pub note: &'static str,
}

impl AddressKind {
    /// Every kind, in the order the grammar is published.
    pub const ALL: [AddressKind; 29] = [
        AddressKind::TextFrame,
        AddressKind::Rectangle,
        AddressKind::Oval,
        AddressKind::Polygon,
        AddressKind::GraphicLine,
        AddressKind::Group,
        AddressKind::Frame,
        AddressKind::StoryRange,
        AddressKind::Doc,
        AddressKind::Page,
        AddressKind::Spread,
        AddressKind::Layer,
        AddressKind::Master,
        AddressKind::Story,
        AddressKind::Field,
        AddressKind::Table,
        AddressKind::Cell,
        AddressKind::Style,
        AddressKind::Swatch,
        AddressKind::Gradient,
        AddressKind::ColorGroup,
        AddressKind::NumberingList,
        AddressKind::Guide,
        AddressKind::Condition,
        AddressKind::ConditionSet,
        AddressKind::Section,
        AddressKind::Hyperlink,
        AddressKind::Plugin,
        AddressKind::Handle,
    ];

    /// The canonical prefix (before the `:`). `doc` has no `:`; a handle
    /// is `$h`.
    pub fn prefix(self) -> &'static str {
        match self {
            AddressKind::Doc => "doc",
            AddressKind::TextFrame => "textFrame",
            AddressKind::Rectangle => "rectangle",
            AddressKind::Oval => "oval",
            AddressKind::Polygon => "polygon",
            AddressKind::GraphicLine => "graphicLine",
            AddressKind::Group => "group",
            AddressKind::Frame => "frame",
            AddressKind::Page => "page",
            AddressKind::Spread => "spread",
            AddressKind::Layer => "layer",
            AddressKind::Master => "master",
            AddressKind::Story => "story",
            AddressKind::StoryRange => "storyRange",
            AddressKind::Field => "field",
            AddressKind::Table => "table",
            AddressKind::Cell => "cell",
            AddressKind::Style => "style",
            AddressKind::Swatch => "swatch",
            AddressKind::Gradient => "gradient",
            AddressKind::ColorGroup => "colorGroup",
            AddressKind::NumberingList => "numberingList",
            AddressKind::Guide => "guide",
            AddressKind::Condition => "condition",
            AddressKind::ConditionSet => "conditionSet",
            AddressKind::Section => "section",
            AddressKind::Hyperlink => "hyperlink",
            AddressKind::Plugin => "plugin",
            AddressKind::Handle => "$h",
        }
    }

    /// Parse a selector kind name (case-insensitive: `textframe` is the
    /// text frame).
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.prefix().eq_ignore_ascii_case(s))
    }

    /// The six page-item kinds — what `frame` matches in a selector.
    pub fn is_page_item(self) -> bool {
        matches!(
            self,
            AddressKind::TextFrame
                | AddressKind::Rectangle
                | AddressKind::Oval
                | AddressKind::Polygon
                | AddressKind::GraphicLine
                | AddressKind::Group
        )
    }

    /// The published form of this kind. An exhaustive match, so a new
    /// kind does not compile until it has a documented, worked example —
    /// the catalog's `idGrammar` is generated from this and nothing else.
    pub fn form(self) -> AddressForm {
        let (form, example, note) = match self {
            AddressKind::TextFrame => (
                "textFrame:<id>",
                "textFrame:u123",
                "A text frame. Same scheme: rectangle:/oval:/polygon:/graphicLine:/group:.",
            ),
            AddressKind::Rectangle => ("rectangle:<id>", "rectangle:u12", "A rectangle (graphic frame)."),
            AddressKind::Oval => ("oval:<id>", "oval:u13", "An oval."),
            AddressKind::Polygon => ("polygon:<id>", "polygon:u14", "A polygon or open path."),
            AddressKind::GraphicLine => ("graphicLine:<id>", "graphicLine:u15", "A graphic line."),
            AddressKind::Group => (
                "group:<id>",
                "group:u88",
                "A group. `paged.set(\"group:<id>\", \"groupTransform\", [a,b,c,d,tx,ty])` moves it as a unit.",
            ),
            AddressKind::Frame => (
                "frame:<id>",
                "frame:u123",
                "Any page item, kind unknown (alias). paged.address / paged.query resolve it to the kind-specific form.",
            ),
            AddressKind::StoryRange => (
                "storyRange:<storyId>@<start>..<end>",
                "storyRange:Story/u1@0..6",
                "A character range within a story (half-open). storyId comes from paged.stories()[].selfId. Alias: story:<storyId>@<start>..<end>.",
            ),
            AddressKind::Doc => ("doc", "doc", "The document itself."),
            AddressKind::Page => (
                "page:<id>",
                "page:u1a2",
                "A body page, by its Self id (paged.pages()[].selfId), or page:#<n> for the n-th page (1-based).",
            ),
            AddressKind::Spread => ("spread:<id>", "spread:u1a1", "A spread, by its Self id."),
            AddressKind::Layer => ("layer:<id>", "layer:ub3", "A layer, by its Self id (paged.layers())."),
            AddressKind::Master => (
                "master:<id>",
                "master:ua9",
                "A master spread. `master:<id> > textFrame:<id>` names an item on it.",
            ),
            AddressKind::Story => ("story:<id>", "story:Story/u1", "A story, by its Self id."),
            AddressKind::Field => (
                "field:<storyId>@<offset>",
                "field:Story/u1@12",
                "A text field anchored at a story character offset (paged.placeholders / RequestDocumentPlaceholders).",
            ),
            AddressKind::Table => ("table:<id>", "table:u2f", "A table, by its Self id (unique per document)."),
            AddressKind::Cell => (
                "cell:<tableId>/<row>,<col>",
                "cell:u2f/0,1",
                "A table cell, zero-based row and column.",
            ),
            AddressKind::Style => (
                "style:<family>/<selfId>",
                "style:paragraph/ParagraphStyle/Body",
                "A style; family is paragraph | character | object | cell | table. The display name is accepted in place of the Self id.",
            ),
            AddressKind::Swatch => ("swatch:<id>", "swatch:Color/Red", "A colour or swatch, by its Self id."),
            AddressKind::Gradient => ("gradient:<id>", "gradient:Gradient/u1", "A gradient, by its Self id."),
            AddressKind::ColorGroup => ("colorGroup:<id>", "colorGroup:ColorGroup/u7", "A colour group."),
            AddressKind::NumberingList => (
                "numberingList:<id>",
                "numberingList:NumberingList/Steps",
                "A numbering list.",
            ),
            AddressKind::Guide => ("guide:<id>", "guide:u5c", "A ruler guide."),
            AddressKind::Condition => ("condition:<id>", "condition:Condition/Draft", "A text condition."),
            AddressKind::ConditionSet => ("conditionSet:<id>", "conditionSet:u3d", "A condition set."),
            AddressKind::Section => ("section:<id>", "section:u4e", "A numbering section."),
            AddressKind::Hyperlink => ("hyperlink:<id>", "hyperlink:u6a", "A hyperlink."),
            AddressKind::Plugin => (
                "plugin:<pluginId>/<kind>/<id>",
                "plugin:paged.image/layer/7",
                "An object a plugin owns. Opaque to core and carried through unchanged.",
            ),
            AddressKind::Handle => (
                "$h:<name>",
                "$h:newFrame",
                "A batch handle bound earlier in the same batch.",
            ),
        };
        AddressForm {
            kind: self,
            form,
            example,
            note,
        }
    }
}

/// Every published address form, in grammar order. The catalog's
/// `idGrammar` is exactly this list.
pub fn address_forms() -> Vec<AddressForm> {
    AddressKind::ALL
        .into_iter()
        .map(AddressKind::form)
        .collect()
}

/// A malformed address or selector, with the byte offset the problem
/// starts at — user input, so an answer, never a panic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "camelCase")]
pub struct GrammarError {
    /// Byte offset into the input.
    pub position: u32,
    pub message: String,
}

impl GrammarError {
    pub fn new(position: usize, message: impl Into<String>) -> Self {
        Self {
            position: position as u32,
            message: message.into(),
        }
    }

    /// The same error, `by` bytes further into an enclosing input.
    pub fn shifted(mut self, by: usize) -> Self {
        self.position += by as u32;
        self
    }
}

impl std::fmt::Display for GrammarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at {}: {}", self.position, self.message)
    }
}

impl std::error::Error for GrammarError {}

/// Escape `%` and every char of `delims` as `%XX`.
fn enc(s: &str, delims: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || delims.contains(c) {
            for b in c.to_string().bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Undo [`enc`]. A `%` not followed by two hex digits is kept literally.
fn dec(s: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        (b as char).to_digit(16).map(|d| d as u8)
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn parse_u32(s: &str, at: usize, what: &str) -> Result<u32, GrammarError> {
    s.parse()
        .map_err(|_| GrammarError::new(at, format!("{what} {s:?} is not a number")))
}

/// `"<a>..<b>"` → `(a, b)`, `b > a`.
fn parse_range(s: &str, at: usize) -> Result<(u32, u32), GrammarError> {
    let (a, b) = s
        .split_once("..")
        .ok_or_else(|| GrammarError::new(at, "a story range is <start>..<end>"))?;
    let start = parse_u32(a, at, "range start")?;
    let end = parse_u32(b, at + a.len() + 2, "range end")?;
    if end <= start {
        return Err(GrammarError::new(
            at,
            format!("an empty range {start}..{end}: end must be greater than start"),
        ));
    }
    Ok((start, end))
}

const MASTER_SCOPE: &str = " > ";

impl Address {
    pub fn kind(&self) -> AddressKind {
        match self {
            Address::Doc => AddressKind::Doc,
            Address::TextFrame { .. } => AddressKind::TextFrame,
            Address::Rectangle { .. } => AddressKind::Rectangle,
            Address::Oval { .. } => AddressKind::Oval,
            Address::Polygon { .. } => AddressKind::Polygon,
            Address::GraphicLine { .. } => AddressKind::GraphicLine,
            Address::Group { .. } => AddressKind::Group,
            Address::Frame { .. } => AddressKind::Frame,
            Address::Page { .. } | Address::PageNumber { .. } => AddressKind::Page,
            Address::Spread { .. } => AddressKind::Spread,
            Address::Layer { .. } => AddressKind::Layer,
            Address::Master { .. } => AddressKind::Master,
            Address::Story { .. } => AddressKind::Story,
            Address::StoryRange { .. } => AddressKind::StoryRange,
            Address::Field { .. } => AddressKind::Field,
            Address::Table { .. } => AddressKind::Table,
            Address::Cell { .. } => AddressKind::Cell,
            Address::Style { .. } => AddressKind::Style,
            Address::Swatch { .. } => AddressKind::Swatch,
            Address::Gradient { .. } => AddressKind::Gradient,
            Address::ColorGroup { .. } => AddressKind::ColorGroup,
            Address::NumberingList { .. } => AddressKind::NumberingList,
            Address::Guide { .. } => AddressKind::Guide,
            Address::Condition { .. } => AddressKind::Condition,
            Address::ConditionSet { .. } => AddressKind::ConditionSet,
            Address::Section { .. } => AddressKind::Section,
            Address::Hyperlink { .. } => AddressKind::Hyperlink,
            // A master-scoped item IS the item.
            Address::OnMaster { item, .. } => item.kind(),
            Address::Plugin { .. } => AddressKind::Plugin,
            Address::Handle { .. } => AddressKind::Handle,
        }
    }

    /// A page item of `kind` (one of the six, or `Frame`). `None` for any
    /// other kind.
    pub fn page_item(kind: AddressKind, id: impl Into<String>) -> Option<Self> {
        let id = id.into();
        Some(match kind {
            AddressKind::TextFrame => Address::TextFrame { id },
            AddressKind::Rectangle => Address::Rectangle { id },
            AddressKind::Oval => Address::Oval { id },
            AddressKind::Polygon => Address::Polygon { id },
            AddressKind::GraphicLine => Address::GraphicLine { id },
            AddressKind::Group => Address::Group { id },
            AddressKind::Frame => Address::Frame { id },
            _ => return None,
        })
    }

    /// The page item's `Self` id, for the six kinds, `Frame`, and a
    /// master-scoped item.
    pub fn page_item_id(&self) -> Option<&str> {
        match self {
            Address::TextFrame { id }
            | Address::Rectangle { id }
            | Address::Oval { id }
            | Address::Polygon { id }
            | Address::GraphicLine { id }
            | Address::Group { id }
            | Address::Frame { id } => Some(id),
            Address::OnMaster { item, .. } => item.page_item_id(),
            _ => None,
        }
    }

    /// Parse the canonical string form or an alias. See the module docs.
    pub fn parse(s: &str) -> Result<Self, GrammarError> {
        if s == "doc" {
            return Ok(Address::Doc);
        }
        if let Some(name) = s.strip_prefix("$h:") {
            if name.is_empty() {
                return Err(GrammarError::new(3, "a batch handle needs a name"));
            }
            return Ok(Address::Handle {
                name: name.to_string(),
            });
        }
        let Some((kind, rest)) = s.split_once(':') else {
            return Err(GrammarError::new(
                0,
                format!("{s:?} is not an address: expected `doc` or `<kind>:<id>`"),
            ));
        };
        let at = kind.len() + 1;
        if rest.is_empty() {
            return Err(GrammarError::new(at, format!("`{kind}:` has an empty id")));
        }
        let id = rest.to_string();
        Ok(match kind {
            "textFrame" | "textframe" => Address::TextFrame { id },
            "rectangle" | "rect" => Address::Rectangle { id },
            "oval" => Address::Oval { id },
            "polygon" => Address::Polygon { id },
            "graphicLine" | "graphicline" => Address::GraphicLine { id },
            "group" => Address::Group { id },
            "frame" => Address::Frame { id },
            "spread" => Address::Spread { id },
            "layer" => Address::Layer { id },
            "swatch" => Address::Swatch { id },
            "gradient" => Address::Gradient { id },
            "colorGroup" => Address::ColorGroup { id },
            "numberingList" => Address::NumberingList { id },
            "guide" => Address::Guide { id },
            "condition" => Address::Condition { id },
            "conditionSet" => Address::ConditionSet { id },
            "section" => Address::Section { id },
            "hyperlink" => Address::Hyperlink { id },
            "table" => Address::Table { id },
            "page" => match rest.strip_prefix('#') {
                Some(n) => {
                    let n = parse_u32(n, at + 1, "page number")?;
                    if n == 0 {
                        return Err(GrammarError::new(at + 1, "page numbers start at 1"));
                    }
                    Address::PageNumber { n }
                }
                None => Address::Page { id },
            },
            "master" => match rest.split_once(MASTER_SCOPE) {
                Some((master, item)) => {
                    let item_at = at + master.len() + MASTER_SCOPE.len();
                    let item = Address::parse(item).map_err(|e| e.shifted(item_at))?;
                    if !item.kind().is_page_item() && item.kind() != AddressKind::Frame {
                        return Err(GrammarError::new(
                            item_at,
                            "only a page item can be scoped to a master",
                        ));
                    }
                    if master.is_empty() {
                        return Err(GrammarError::new(at, "a master scope needs a master id"));
                    }
                    Address::OnMaster {
                        master: dec(master),
                        item: Box::new(item),
                    }
                }
                None => Address::Master { id },
            },
            "story" | "storyRange" | "storyrange" => {
                let range_required = kind != "story";
                match rest.rsplit_once('@') {
                    Some((story, range)) if range_required || range.contains("..") => {
                        if story.is_empty() {
                            return Err(GrammarError::new(at, "a story range needs a story id"));
                        }
                        let (start, end) = parse_range(range, at + story.len() + 1)?;
                        Address::StoryRange {
                            story_id: dec(story),
                            start,
                            end,
                        }
                    }
                    _ if range_required => {
                        return Err(GrammarError::new(
                            at,
                            "a story range is <storyId>@<start>..<end>",
                        ))
                    }
                    _ => Address::Story { id },
                }
            }
            "field" => {
                let (story, offset) = rest
                    .rsplit_once('@')
                    .ok_or_else(|| GrammarError::new(at, "a field is field:<storyId>@<offset>"))?;
                if story.is_empty() {
                    return Err(GrammarError::new(at, "a field needs a story id"));
                }
                Address::Field {
                    story_id: dec(story),
                    offset: parse_u32(offset, at + story.len() + 1, "field offset")?,
                }
            }
            "cell" => {
                let (table, rc) = rest
                    .rsplit_once('/')
                    .ok_or_else(|| GrammarError::new(at, "a cell is cell:<tableId>/<row>,<col>"))?;
                let rc_at = at + table.len() + 1;
                let (r, c) = rc
                    .split_once(',')
                    .ok_or_else(|| GrammarError::new(rc_at, "a cell ends in /<row>,<col>"))?;
                let row = parse_u32(r, rc_at, "row")?;
                let col = parse_u32(c, rc_at + r.len() + 1, "column")?;
                if table.is_empty() {
                    return Err(GrammarError::new(at, "a cell needs a table id"));
                }
                Address::Cell {
                    table_id: dec(table),
                    row,
                    col,
                }
            }
            "style" => {
                let (family, name) = rest
                    .split_once('/')
                    .ok_or_else(|| GrammarError::new(at, "a style is style:<family>/<selfId>"))?;
                let family = StyleFamily::parse(family).ok_or_else(|| {
                    GrammarError::new(
                        at,
                        format!(
                            "{family:?} is not a style family: paragraph | character | object | cell | table"
                        ),
                    )
                })?;
                if name.is_empty() {
                    return Err(GrammarError::new(at, "a style needs a Self id or a name"));
                }
                Address::Style {
                    family,
                    id: name.to_string(),
                }
            }
            "plugin" => {
                let mut parts = rest.splitn(3, '/');
                let (Some(plugin), Some(object_kind), Some(id)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    return Err(GrammarError::new(
                        at,
                        "a plugin address is plugin:<pluginId>/<kind>/<id>",
                    ));
                };
                if plugin.is_empty() || object_kind.is_empty() || id.is_empty() {
                    return Err(GrammarError::new(
                        at,
                        "a plugin address needs a plugin id, a kind and an id",
                    ));
                }
                Address::Plugin {
                    plugin_id: dec(plugin),
                    object_kind: dec(object_kind),
                    id: id.to_string(),
                }
            }
            other => {
                return Err(GrammarError::new(
                    0,
                    format!("{other:?} is not an address kind"),
                ))
            }
        })
    }

    /// The [`ElementId`] this address names, when it names one WITHOUT a
    /// document: the six item kinds (master-scoped too) and a story range.
    /// A table or a cell needs the document to find its story — see the
    /// query module's resolver — and `Frame` its kind.
    pub fn to_element_id(&self) -> Option<ElementId> {
        Some(match self {
            Address::TextFrame { id } => ElementId::TextFrame(id.clone()),
            Address::Rectangle { id } => ElementId::Rectangle(id.clone()),
            Address::Oval { id } => ElementId::Oval(id.clone()),
            Address::Polygon { id } => ElementId::Polygon(id.clone()),
            Address::GraphicLine { id } => ElementId::GraphicLine(id.clone()),
            Address::Group { id } => ElementId::Group(id.clone()),
            Address::OnMaster { item, .. } => return item.to_element_id(),
            Address::StoryRange {
                story_id,
                start,
                end,
            } => ElementId::StoryRange {
                story_id: story_id.clone(),
                start: *start,
                end: *end,
            },
            _ => return None,
        })
    }
}

/// Total, per ADR 131. The table's story rides the `ElementId` tuple and
/// is dropped here: the table `Self` is unique per document, and the
/// resolver finds the story again.
impl From<ElementId> for Address {
    fn from(id: ElementId) -> Self {
        match id {
            ElementId::TextFrame(id) => Address::TextFrame { id },
            ElementId::Rectangle(id) => Address::Rectangle { id },
            ElementId::Oval(id) => Address::Oval { id },
            ElementId::Polygon(id) => Address::Polygon { id },
            ElementId::GraphicLine(id) => Address::GraphicLine { id },
            ElementId::Group(id) => Address::Group { id },
            ElementId::StoryRange {
                story_id,
                start,
                end,
            } => Address::StoryRange {
                story_id,
                start,
                end,
            },
            ElementId::Table { table_id, .. } => Address::Table { id: table_id },
            ElementId::TableCell {
                table_id, row, col, ..
            } => Address::Cell { table_id, row, col },
        }
    }
}

impl std::str::FromStr for Address {
    type Err = GrammarError;
    fn from_str(s: &str) -> Result<Self, GrammarError> {
        Address::parse(s)
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = self.kind().prefix();
        match self {
            Address::Doc => f.write_str("doc"),
            Address::Handle { name } => write!(f, "$h:{name}"),
            Address::TextFrame { id }
            | Address::Rectangle { id }
            | Address::Oval { id }
            | Address::Polygon { id }
            | Address::GraphicLine { id }
            | Address::Group { id }
            | Address::Frame { id }
            | Address::Page { id }
            | Address::Spread { id }
            | Address::Layer { id }
            | Address::Master { id }
            | Address::Story { id }
            | Address::Table { id }
            | Address::Swatch { id }
            | Address::Gradient { id }
            | Address::ColorGroup { id }
            | Address::NumberingList { id }
            | Address::Guide { id }
            | Address::Condition { id }
            | Address::ConditionSet { id }
            | Address::Section { id }
            | Address::Hyperlink { id } => write!(f, "{p}:{id}"),
            Address::PageNumber { n } => write!(f, "page:#{n}"),
            Address::StoryRange {
                story_id,
                start,
                end,
            } => write!(f, "{p}:{}@{start}..{end}", enc(story_id, "@")),
            Address::Field { story_id, offset } => {
                write!(f, "{p}:{}@{offset}", enc(story_id, "@"))
            }
            Address::Cell { table_id, row, col } => {
                write!(f, "{p}:{}/{row},{col}", enc(table_id, "/"))
            }
            Address::Style { family, id } => write!(f, "{p}:{}/{id}", family.as_str()),
            Address::OnMaster { master, item } => {
                write!(f, "master:{}{MASTER_SCOPE}{item}", enc(master, ">"))
            }
            Address::Plugin {
                plugin_id,
                object_kind,
                id,
            } => write!(
                f,
                "{p}:{}/{}/{id}",
                enc(plugin_id, "/"),
                enc(object_kind, "/")
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn a(s: &str) -> Address {
        Address::parse(s).unwrap_or_else(|e| panic!("{s:?}: {e}"))
    }

    #[test]
    fn every_kind_has_a_form_whose_example_parses_to_that_kind_and_round_trips() {
        for form in address_forms() {
            let parsed = a(form.example);
            assert_eq!(parsed.kind(), form.kind, "{}", form.example);
            assert_eq!(
                parsed.to_string(),
                form.example,
                "{} is not canonical",
                form.example
            );
            assert!(
                form.form.starts_with(form.kind.prefix()),
                "{:?}: form {} does not start with {}",
                form.kind,
                form.form,
                form.kind.prefix()
            );
        }
        assert_eq!(address_forms().len(), AddressKind::ALL.len());
    }

    #[test]
    fn the_adr_forms_parse() {
        assert_eq!(a("doc"), Address::Doc);
        assert_eq!(a("page:u1"), Address::Page { id: "u1".into() });
        assert_eq!(a("page:#3"), Address::PageNumber { n: 3 });
        assert_eq!(
            a("storyRange:Story/u1@3..9"),
            Address::StoryRange {
                story_id: "Story/u1".into(),
                start: 3,
                end: 9
            }
        );
        assert_eq!(
            a("story:Story/u1"),
            Address::Story {
                id: "Story/u1".into()
            }
        );
        assert_eq!(
            a("cell:u2f/2,3"),
            Address::Cell {
                table_id: "u2f".into(),
                row: 2,
                col: 3
            }
        );
        assert_eq!(
            a("style:paragraph/ParagraphStyle/$ID/[No paragraph style]"),
            Address::Style {
                family: StyleFamily::Paragraph,
                id: "ParagraphStyle/$ID/[No paragraph style]".into()
            }
        );
        assert_eq!(
            a("plugin:paged.sheet/cell/Sheet1!B4"),
            Address::Plugin {
                plugin_id: "paged.sheet".into(),
                object_kind: "cell".into(),
                id: "Sheet1!B4".into()
            }
        );
        assert_eq!(
            a("master:ua9 > textFrame:u5"),
            Address::OnMaster {
                master: "ua9".into(),
                item: Box::new(Address::TextFrame { id: "u5".into() })
            }
        );
        assert_eq!(a("$h:new"), Address::Handle { name: "new".into() });
        assert_eq!(
            a("field:u1@12"),
            Address::Field {
                story_id: "u1".into(),
                offset: 12
            }
        );
    }

    #[test]
    fn aliases_parse_and_are_never_emitted() {
        for (alias, canonical) in [
            ("story:Story/u1@0..6", "storyRange:Story/u1@0..6"),
            ("storyrange:Story/u1@0..6", "storyRange:Story/u1@0..6"),
            ("textframe:u1", "textFrame:u1"),
            ("rect:u1", "rectangle:u1"),
            ("graphicline:u1", "graphicLine:u1"),
        ] {
            assert_eq!(a(alias).to_string(), canonical);
        }
    }

    #[test]
    fn errors_carry_positions() {
        let e = Address::parse("storyRange:Story/u1@6..2").unwrap_err();
        assert_eq!(e.position, 20, "{e}");
        let e = Address::parse("cell:t/1,x").unwrap_err();
        assert_eq!(e.position, 9, "{e}");
        let e = Address::parse("style:fancy/Body").unwrap_err();
        assert_eq!(e.position, 6, "{e}");
        let e = Address::parse("nope:x").unwrap_err();
        assert_eq!(e.position, 0, "{e}");
        let e = Address::parse("master:ua9 > page:u1").unwrap_err();
        assert_eq!(e.position, 13, "{e}");
        for bad in [
            "",
            "page:",
            "page:#0",
            "page:#x",
            "storyRange:Story/u1",
            "storyRange:@0..6",
            "plugin:a/b",
            "field:u1",
            "$h:",
        ] {
            assert!(Address::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn delimiters_inside_ids_are_escaped_where_they_would_end_the_id() {
        let r = Address::StoryRange {
            story_id: "odd@id%".into(),
            start: 1,
            end: 2,
        };
        assert_eq!(r.to_string(), "storyRange:odd%40id%25@1..2");
        assert_eq!(a(&r.to_string()), r);
        let c = Address::Cell {
            table_id: "t/1".into(),
            row: 0,
            col: 0,
        };
        assert_eq!(c.to_string(), "cell:t%2F1/0,0");
        assert_eq!(a(&c.to_string()), c);
    }

    #[test]
    fn element_ids_convert_to_addresses_and_back_where_no_document_is_needed() {
        let ids = [
            ElementId::TextFrame("u1".into()),
            ElementId::Rectangle("u2".into()),
            ElementId::Oval("u3".into()),
            ElementId::Polygon("u4".into()),
            ElementId::GraphicLine("u5".into()),
            ElementId::Group("u6".into()),
            ElementId::StoryRange {
                story_id: "Story/u1".into(),
                start: 0,
                end: 6,
            },
        ];
        for id in ids {
            let addr = Address::from(id.clone());
            assert_eq!(addr.to_element_id(), Some(id.clone()));
            assert_eq!(a(&addr.to_string()).to_element_id(), Some(id));
        }
        let cell = Address::from(ElementId::TableCell {
            story_id: "Story/u1".into(),
            table_id: "t1".into(),
            row: 4,
            col: 2,
        });
        assert_eq!(cell.to_string(), "cell:t1/4,2");
        assert_eq!(
            cell.to_element_id(),
            None,
            "needs the document for its story"
        );
        assert_eq!(Address::Frame { id: "u1".into() }.to_element_id(), None);
        assert_eq!(Address::Doc.to_element_id(), None);
    }

    #[test]
    fn serde_shape_is_kind_tagged() {
        let v = serde_json::to_value(a("cell:t1/2,3")).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind":"cell","tableId":"t1","row":2,"col":3})
        );
        let v = serde_json::to_value(a("style:object/ObjectStyle/Frame")).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind":"style","family":"object","id":"ObjectStyle/Frame"})
        );
        assert_eq!(
            serde_json::to_value(Address::Doc).unwrap(),
            serde_json::json!({"kind":"doc"})
        );
        let back: Address = serde_json::from_value(serde_json::json!({
            "kind":"onMaster","master":"ua9","item":{"kind":"oval","id":"u1"}
        }))
        .unwrap();
        assert_eq!(back.to_string(), "master:ua9 > oval:u1");
    }

    // ------------------------------------------------------------ proptest

    /// A plain id: anything non-empty, `:` and `/` included.
    fn any_id() -> impl Strategy<Value = String> {
        "[A-Za-z0-9_$:/ .%@>\\[\\]-]{1,16}"
    }
    /// An id that must survive being followed by a delimiter.
    fn any_escaped() -> impl Strategy<Value = String> {
        "[A-Za-z0-9_$:/ .%@>\\[\\]-]{1,16}"
    }

    fn page_item() -> impl Strategy<Value = Address> {
        (0usize..6, any_id())
            .prop_map(|(k, id)| Address::page_item(AddressKind::ALL[k], id).expect("an item kind"))
    }

    fn any_address() -> impl Strategy<Value = Address> {
        let simple = (0usize..20, any_id()).prop_map(|(k, id)| match k {
            0 => Address::Frame { id },
            1 => Address::Spread { id },
            2 => Address::Layer { id },
            3 => Address::Table { id },
            4 => Address::Swatch { id },
            5 => Address::Gradient { id },
            6 => Address::ColorGroup { id },
            7 => Address::NumberingList { id },
            8 => Address::Guide { id },
            9 => Address::Condition { id },
            10 => Address::ConditionSet { id },
            11 => Address::Section { id },
            12 => Address::Hyperlink { id },
            13 => Address::Handle { name: id },
            _ => Address::Doc,
        });
        prop_oneof![
            simple,
            page_item(),
            any_id()
                .prop_filter("not a page number", |s| !s.starts_with('#'))
                .prop_map(|id| Address::Page { id }),
            (1u32..10_000).prop_map(|n| Address::PageNumber { n }),
            any_id()
                .prop_filter("not a master scope", |s| !s.contains(" > "))
                .prop_map(|id| Address::Master { id }),
            any_id()
                .prop_filter("not a range", |s| !s.contains('@'))
                .prop_map(|id| Address::Story { id }),
            (any_escaped(), 0u32..10_000, 1u32..10_000).prop_map(|(story_id, start, len)| {
                Address::StoryRange {
                    story_id,
                    start,
                    end: start + len,
                }
            }),
            (any_escaped(), any::<u32>())
                .prop_map(|(story_id, offset)| Address::Field { story_id, offset }),
            (any_escaped(), any::<u32>(), any::<u32>())
                .prop_map(|(table_id, row, col)| Address::Cell { table_id, row, col }),
            (0usize..5, any_id()).prop_map(|(f, id)| Address::Style {
                family: StyleFamily::ALL[f],
                id
            }),
            (any_escaped(), any_escaped(), any_id()).prop_map(|(plugin_id, object_kind, id)| {
                Address::Plugin {
                    plugin_id,
                    object_kind,
                    id,
                }
            }),
            (any_escaped(), page_item()).prop_map(|(master, item)| Address::OnMaster {
                master,
                item: Box::new(item)
            }),
        ]
    }

    proptest! {
        #[test]
        fn display_then_parse_is_identity(addr in any_address()) {
            let s = addr.to_string();
            prop_assert_eq!(Address::parse(&s), Ok(addr));
        }

        #[test]
        fn parse_never_panics(s in "\\PC{0,40}") {
            let _ = Address::parse(&s);
        }

        #[test]
        fn whatever_parses_reprints_to_a_fixed_point(
            s in "[a-zA-Z$]{1,12}:[A-Za-z0-9/@.,:$# >%-]{0,24}"
        ) {
            if let Ok(addr) = Address::parse(&s) {
                let once = addr.to_string();
                prop_assert_eq!(Address::parse(&once), Ok(addr));
            }
        }
    }
}
