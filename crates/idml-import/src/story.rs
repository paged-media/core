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

//! Story_*.xml parser.
//!
//! An IDML Story is a tree:
//! ```text
//! <Story>
//!   <ParagraphStyleRange AppliedParagraphStyle="...">
//!     <CharacterStyleRange AppliedCharacterStyle="..." PointSize="12" AppliedFont="...">
//!       <Content>Some text</Content>
//!       <Br/>
//!       <Content>more text</Content>
//!     </CharacterStyleRange>
//!     <CharacterStyleRange ...>
//!       <Content>bold bit</Content>
//!     </CharacterStyleRange>
//!   </ParagraphStyleRange>
//!   <ParagraphStyleRange>...</ParagraphStyleRange>
//! </Story>
//! ```
//!
//! The parser collapses all `<Content>` children of a character range
//! into a single string, preserving paragraph boundaries. Full style
//! resolution (font cascade, local overrides, etc.) is the job of
//! `paged-scene`; this module stays focused on shape extraction.

use quick_xml::events::Event;

use crate::util::{attr, parse_tint_attr};
use crate::ParseError;

pub use paged_model::{
    AnchoredFrame, AnchoredFrameKind, AnchoredObjectSetting, CellDiagonal, CharacterRun, Footnote,
    IndexMarker, Paragraph, PlaceholderField, StoryDirection, Table, TableBorder, TableCell,
    TableColumn, TableLineStrokes, TableRow,
};

pub use paged_model::{Justification, OtfFeatures, TabStop};

pub use paged_model::{AUTO_PAGE_NUMBER_MARKER, NEXT_PAGE_NUMBER_MARKER};

pub use paged_model::Story;

/// Parse the discrete `OTF*` attributes off a CharacterStyleRange /
/// CharacterStyle start tag. Returns an all-`None` bag when none of
/// the attributes are present (the common case) so the cascade can
/// distinguish "nothing declared here" from "declared off".
/// (De-inherented from `OtfFeatures::from_attrs` so the type can move to
/// `paged-model`; the XML parsing stays in the parser — N6.)
pub(crate) fn parse_otf_features(e: &quick_xml::events::BytesStart) -> OtfFeatures {
    let b = |k: &[u8]| attr(e, k).and_then(|s| s.parse::<bool>().ok());
    OtfFeatures {
        fraction: b(b"OTFFraction"),
        ordinal: b(b"OTFOrdinal"),
        swash: b(b"OTFSwash"),
        discretionary_ligatures: b(b"OTFDiscretionaryLigature"),
        slashed_zero: b(b"OTFSlashedZero"),
        titling: b(b"OTFTitling"),
        contextual_alternates: b(b"OTFContextualAlternate"),
        figure_style: attr(e, b"OTFFigureStyle"),
        stylistic_sets: attr(e, b"OTFStylisticSets").and_then(|s| s.parse::<i32>().ok()),
    }
}

/// Phase 5 — one stack frame of in-progress footnote parsing. Holds
/// the `Footnote` being assembled plus the parked `current_paragraph`
/// / `current_run` from the host context (the body paragraph
/// containing the footnote anchor). When the footnote closes, the
/// parker is drained back into the parser's current slots so the
/// host paragraph continues to accept further runs.
///
/// Why a stack? Footnotes inside footnotes are exotic but legal in
/// IDML; treating the parser state as a stack makes each nesting
/// level self-contained and matches the existing TableContext idiom.
struct FootnoteContext {
    footnote: Footnote,
    outer_paragraph: Option<Paragraph>,
    outer_run: Option<CharacterRun>,
}

/// Phase 5 — one stack frame of in-progress table parsing. Holds the
/// `Table` being assembled plus parker slots for the parser's three
/// "current" pieces of state at each nesting level:
///
/// - `outer_paragraph` / `outer_run`: the paragraph / run inside the
///   *cell currently being parsed within this table*. The flat parser
///   used a single pair for these; per-table parking lets a nested
///   table's `<Cell>` boundaries stash their own cell-paragraph /
///   cell-run without trampling the outer table's parked state.
/// - `outer_cell`: the `current_cell` captured at this table's
///   `<Table>` open. When this is the outer table, that's `None`;
///   when this is a nested table inside another cell, that's the
///   outer cell. Restored at `</Table>` close so the outer cell
///   continues to accept further content.
struct TableContext {
    table: Table,
    outer_paragraph: Option<Paragraph>,
    outer_run: Option<CharacterRun>,
    outer_cell: Option<TableCell>,
}

pub fn parse_story(xml: &[u8]) -> Result<Story, ParseError> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(false);

    let mut out = Story::default();
    let mut current_paragraph: Option<Paragraph> = None;
    let mut current_run: Option<CharacterRun> = None;
    // Phase 5 — table context stack. Each `<Table>` push, each
    // `</Table>` pop. Nested tables (a table inside a `<Cell>`'s
    // `<Paragraph>`) stack their contexts so the inner table's
    // rows / columns / cells don't bleed into the outer table,
    // and so the outer cell's saved paragraph / run state survives
    // the inner table's `<Cell>` boundaries. Each frame carries
    // the table itself plus the parked outer-paragraph / outer-run
    // for the cell *being parsed inside this table* (the same
    // slots the flat parser used as `outer_paragraph` /
    // `outer_run`, but now per-table instead of global).
    let mut table_stack: Vec<TableContext> = Vec::new();
    let mut current_cell: Option<TableCell> = None;
    // Phase 5 — footnote context stack. Each `<Footnote>` open
    // pushes; `</Footnote>` close pops, attaches the captured
    // body to the host paragraph, and restores the parker state.
    // Nesting is rare but handled.
    let mut footnote_stack: Vec<FootnoteContext> = Vec::new();
    // W1.4 — hyperlink / cross-reference *source* span stack. IDML
    // wraps the character ranges a hyperlink covers in a
    // `<HyperlinkTextSource Self="...">` (or `<CrossReferenceSource
    // Self="...">`) element; every CharacterStyleRange opened while
    // such a wrapper is on the stack inherits its `Self` as
    // `hyperlink_source`. The designmap's `<Hyperlink Source=...>`
    // then resolves the run back to a destination. A vec (not a
    // single slot) because IDML technically permits nesting, though
    // it's exotic — the innermost source wins.
    let mut source_stack: Vec<String> = Vec::new();
    let mut in_content = false;
    let mut buf = Vec::new();
    // `<Properties>` child elements appear *inside* a CharacterStyleRange
    // or ParagraphStyleRange to carry typed values that the spec lets
    // InDesign serialise either as XML attributes or as nested elements
    // with `type="string"|"unit"|"enumeration"`. Real exports prefer the
    // child-element form for AppliedFont, Leading, BulletsFont, etc., so
    // a parser that only reads attributes loses the data entirely. We
    // track the *enclosing* container of the Properties block plus the
    // currently-open child name so the Text event can accumulate the
    // value.
    //
    //   1 → Properties under a CharacterStyleRange (run-level)
    //   2 → Properties under a ParagraphStyleRange (paragraph-level)
    //
    // 0 / None means Properties belongs to a Story / TextFrame / other
    // container we don't extract typed children from yet.
    let mut properties_kind: u8 = 0;
    let mut properties_field: Option<Vec<u8>> = None;
    let mut properties_text = String::new();
    // Anchored-frame state. When a <TextFrame> / <Rectangle> /
    // <Group> opens as a child of a CharacterStyleRange, we
    // record it as an anchored object on the current paragraph
    // and recurse through its body until the matching close.
    //
    // `anchored_depth` counts open XML elements currently inside
    // the outermost anchored body (it bumps on every Start,
    // decrements on every End). 0 ⇒ outside any anchored frame.
    //
    // `anchored_stack` holds the open frame records: the bottom
    // is the outermost anchored frame, deeper entries are nested
    // children inside Groups. When an End event for a frame
    // element name (`TextFrame` / `Rectangle` / `Group`) fires
    // we pop the top frame; if it leaves the stack non-empty we
    // attach it as a child of the new top, otherwise we attach
    // it to the host paragraph.
    //
    // The (Image / Link, AnchoredObjectSetting) attribute capture
    // mutates the top of the stack so attributes always land on
    // the nearest enclosing frame.
    let mut anchored_depth: u32 = 0;
    let mut anchored_stack: Vec<AnchoredFrame> = Vec::new();
    // Suppressed-subtree depth. IDML uses `<HiddenText>` (authored
    // but not flowed), `<Note>` (sticky-note annotations), and
    // `<Index>` / `<IndexEntry>` (index markers — the marker is a
    // zero-width metadata point; the entry text is metadata, not
    // body copy). While `suppress_depth > 0` every Start bumps it,
    // every End decrements it, and Content / inline glyph events
    // (Br, Tab, TextVariableInstance) are dropped. The wrapper
    // itself does not insert any character into the host run, so
    // the surrounding flow is uninterrupted.
    let mut suppress_depth: u32 = 0;
    // Parallel to `anchored_stack`: tracks whether the top frame's
    // `bounds` were derived from a `<PathPointType>` chain (`true`)
    // or from a `GeometricBounds` attribute (`false`). The
    // PathPointType handler only extends bounds when the flag is
    // `true`, so an explicit `GeometricBounds="…"` always wins.
    let mut bounds_from_path: Vec<bool> = Vec::new();

    // Helper: build an `AnchoredFrame` record from a frame
    // element's start tag. Mirrors `spread.rs::read_common_attrs`
    // for the cross-cutting attribute set so the renderer sees
    // the full styling alongside geometry + setting.
    fn make_anchored_frame(
        e: &quick_xml::events::BytesStart,
        kind: AnchoredFrameKind,
    ) -> AnchoredFrame {
        let bounds = attr(e, b"GeometricBounds").and_then(|s| parse_bounds_local(&s));
        let item_transform = attr(e, b"ItemTransform").and_then(|s| parse_matrix_local(&s));
        let parent_story = if matches!(kind, AnchoredFrameKind::TextFrame) {
            attr(e, b"ParentStory")
        } else {
            None
        };
        AnchoredFrame {
            frame_kind: kind,
            self_id: attr(e, b"Self"),
            bounds,
            item_transform,
            parent_story,
            setting: None,
            fill_color: attr(e, b"FillColor"),
            stroke_color: attr(e, b"StrokeColor"),
            stroke_weight: attr(e, b"StrokeWeight").and_then(|s| s.parse().ok()),
            fill_tint: parse_tint_attr(e, b"FillTint"),
            gradient_fill_angle: attr(e, b"GradientFillAngle").and_then(|s| s.parse().ok()),
            applied_object_style: attr(e, b"AppliedObjectStyle"),
            image_link: None,
            image_item_transform: None,
            children: Vec::new(),
        }
    }

    // Helper: classify an element name as a frame element (i.e.
    // one that opens an anchored sub-frame on the stack).
    fn anchored_kind_from_name(name: &[u8]) -> Option<AnchoredFrameKind> {
        match name {
            b"TextFrame" => Some(AnchoredFrameKind::TextFrame),
            b"Rectangle" => Some(AnchoredFrameKind::Rectangle),
            b"Group" => Some(AnchoredFrameKind::Group),
            _ => None,
        }
    }

    // Helper: pop the top anchored frame and attach it to its
    // parent (the new top of stack) or the host paragraph. Pops
    // the parallel `bounds_from_path` flag at the same time.
    fn finalise_anchored_top(
        anchored_stack: &mut Vec<AnchoredFrame>,
        bounds_from_path: &mut Vec<bool>,
        current_paragraph: &mut Option<Paragraph>,
    ) {
        bounds_from_path.pop();
        if let Some(frame) = anchored_stack.pop() {
            if let Some(parent) = anchored_stack.last_mut() {
                parent.children.push(frame);
            } else if let Some(para) = current_paragraph.as_mut() {
                para.anchored_frames.push(frame);
            }
        }
    }

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let n = e.name();
                let name = n.as_ref();
                // Anchored-frame handling. A `<TextFrame>` /
                // `<Rectangle>` / `<Group>` nested directly
                // inside a `<CharacterStyleRange>` is an
                // inline-anchored object — capture geometry +
                // attributes; recurse into Group children.
                if anchored_depth > 0 {
                    // Nested frame element: open a new frame on
                    // the stack so its attributes / children
                    // capture independently of the parent.
                    if let Some(kind) = anchored_kind_from_name(name) {
                        let frame = make_anchored_frame(&e, kind);
                        bounds_from_path.push(frame.bounds.is_none());
                        anchored_stack.push(frame);
                    } else if name == b"AnchoredObjectSetting" {
                        if let Some(p) = anchored_stack.last_mut() {
                            p.setting = Some(parse_anchored_object_setting(&e));
                        }
                    } else if name == b"Image" || name == b"Link" {
                        anchored_capture_image_attrs(&e, &mut anchored_stack);
                    } else if name == b"PathPointType" {
                        anchored_extend_path_bounds(&e, &mut anchored_stack, &mut bounds_from_path);
                    }
                    // Always bump depth on Start so we stay
                    // inside the anchored body until the
                    // matching End fires.
                    anchored_depth += 1;
                    buf.clear();
                    continue;
                }
                if current_run.is_some() {
                    if let Some(kind) = anchored_kind_from_name(name) {
                        let frame = make_anchored_frame(&e, kind);
                        // `false` ⇒ bounds came from
                        // GeometricBounds attribute; `true` ⇒
                        // bounds will be derived from
                        // `<PathPointType>` anchor coordinates.
                        bounds_from_path.push(frame.bounds.is_none());
                        anchored_stack.push(frame);
                        anchored_depth = 1;
                        buf.clear();
                        continue;
                    }
                }
                // Suppressed subtree: enter on the wrapper, bump
                // depth on every child so the matching End pairs
                // up. Skip the body handlers entirely.
                if suppress_depth > 0 {
                    suppress_depth += 1;
                    buf.clear();
                    continue;
                }
                if matches!(name, b"HiddenText" | b"Note") {
                    suppress_depth = 1;
                    buf.clear();
                    continue;
                }
                match name {
                    // `<Story Self="..." StoryDirection="...">` is the
                    // document root inside `<idPkg:Story>`. We only
                    // surface the writing-direction flag today;
                    // additional Story-level attributes (`AppliedTOCStyle`,
                    // `TrackChanges`, etc.) land in followup parser slices.
                    b"Story" => {
                        if let Some(v) = attr(&e, b"StoryDirection") {
                            out.story_direction = StoryDirection::from_idml(&v);
                        }
                    }
                    // W1.4 — a hyperlink / cross-reference source span
                    // wraps the character ranges it covers. Push its
                    // `Self` so every run opened inside inherits the id;
                    // the matching End pops it. (Self-closing sources
                    // arrive via `Event::Empty` and never reach here, so
                    // they don't unbalance the stack.)
                    b"HyperlinkTextSource" | b"CrossReferenceSource" => {
                        if let Some(self_id) = attr(&e, b"Self") {
                            source_stack.push(self_id);
                        } else {
                            // Keep the stack depth-balanced with the
                            // End even when the id is missing.
                            source_stack.push(String::new());
                        }
                    }
                    // <StoryPreference> may also appear with
                    // children (e.g. nested <Properties>) instead of
                    // self-closing. Read the attributes off the Start
                    // event as well so the data lands either way.
                    b"StoryPreference" => {
                        if let Some(v) = attr(&e, b"OpticalMarginAlignment") {
                            if let Ok(b) = v.parse::<bool>() {
                                out.optical_margin_alignment = b;
                            }
                        }
                        if let Some(v) = attr(&e, b"OpticalMarginSize") {
                            if let Ok(f) = v.parse::<f32>() {
                                out.optical_margin_size = f;
                            }
                        }
                    }
                    b"ParagraphStyleRange" => {
                        current_paragraph = Some(Paragraph {
                            paragraph_style: attr(&e, b"AppliedParagraphStyle"),
                            justification: attr(&e, b"Justification")
                                .as_deref()
                                .and_then(Justification::from_idml),
                            first_line_indent: attr(&e, b"FirstLineIndent")
                                .and_then(|s| s.parse().ok()),
                            left_indent: attr(&e, b"LeftIndent").and_then(|s| s.parse().ok()),
                            right_indent: attr(&e, b"RightIndent").and_then(|s| s.parse().ok()),
                            space_before: attr(&e, b"SpaceBefore").and_then(|s| s.parse().ok()),
                            space_after: attr(&e, b"SpaceAfter").and_then(|s| s.parse().ok()),
                            tab_list: Vec::new(),
                            bullets_list_type: attr(&e, b"BulletsAndNumberingListType"),
                            bullet_character: None,
                            numbering_format: attr(&e, b"NumberingFormat"),
                            applied_numbering_list: match attr(&e, b"AppliedNumberingList")
                                .as_deref()
                            {
                                Some("n") | Some("NumberingList/n") | Some("") => None,
                                Some(s) if s.ends_with("[No numbering list]") => None,
                                _ => attr(&e, b"AppliedNumberingList"),
                            },
                            drop_cap_characters: attr(&e, b"DropCapCharacters")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0),
                            drop_cap_lines: attr(&e, b"DropCapLines")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0),
                            drop_cap_detail: attr(&e, b"DropCapDetail")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0),
                            hyphenation: attr(&e, b"Hyphenation")
                                .and_then(|s| s.parse::<bool>().ok()),
                            keep_lines_together: attr(&e, b"KeepLinesTogether")
                                .and_then(|s| s.parse::<bool>().ok()),
                            keep_with_next: attr(&e, b"KeepWithNext")
                                .and_then(|s| s.parse::<u32>().ok()),
                            rule_above: crate::styles::parse_paragraph_rule(&e, "RuleAbove"),
                            rule_below: crate::styles::parse_paragraph_rule(&e, "RuleBelow"),
                            kinsoku_set: attr(&e, b"KinsokuSet"),
                            kinsoku_type: attr(&e, b"KinsokuType"),
                            mojikumi_table: attr(&e, b"MojikumiTable"),
                            mojikumi_set: attr(&e, b"MojikumiSet"),
                            runs: Vec::new(),
                            anchored_frames: Vec::new(),
                            table: None,
                            overprint_fill: attr(&e, b"OverprintFill")
                                .and_then(|s| s.parse::<bool>().ok()),
                            overprint_stroke: attr(&e, b"OverprintStroke")
                                .and_then(|s| s.parse::<bool>().ok()),
                            footnotes: Vec::new(),
                            index_markers: Vec::new(),
                        });
                    }
                    b"Table" => {
                        // Tables nest inside a CharacterStyleRange; the
                        // run that hosts the table is typically
                        // contentless, so we let it pass through as-is.
                        // Push a fresh frame; an outer table already
                        // on the stack stays untouched. Save the
                        // current `current_cell` (Some when this
                        // table opens inside an outer cell, None at
                        // the story level) so we can restore it after
                        // the inner table closes.
                        table_stack.push(TableContext {
                            outer_paragraph: None,
                            outer_run: None,
                            outer_cell: current_cell.take(),
                            table: Table {
                                self_id: attr(&e, b"Self"),
                                header_row_count: attr(&e, b"HeaderRowCount")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0),
                                footer_row_count: attr(&e, b"FooterRowCount")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0),
                                body_row_count: attr(&e, b"BodyRowCount")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0),
                                column_count: attr(&e, b"ColumnCount")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0),
                                repeating_header: attr(&e, b"RepeatingHeader")
                                    .and_then(|s| s.parse::<bool>().ok()),
                                repeating_footer: attr(&e, b"RepeatingFooter")
                                    .and_then(|s| s.parse::<bool>().ok()),
                                applied_table_style: attr(&e, b"AppliedTableStyle"),
                                rows: Vec::new(),
                                columns: Vec::new(),
                                cells: Vec::new(),
                                border: TableBorder {
                                    top_color: attr(&e, b"TopBorderStrokeColor"),
                                    top_type: attr(&e, b"TopBorderStrokeType"),
                                    top_weight: attr(&e, b"TopBorderStrokeWeight")
                                        .and_then(|s| s.parse().ok()),
                                    top_tint: parse_tint_attr(&e, b"TopBorderStrokeTint"),
                                    top_gap_color: attr(&e, b"TopBorderStrokeGapColor"),
                                    top_gap_tint: parse_tint_attr(&e, b"TopBorderStrokeGapTint"),
                                    bottom_color: attr(&e, b"BottomBorderStrokeColor"),
                                    bottom_type: attr(&e, b"BottomBorderStrokeType"),
                                    bottom_weight: attr(&e, b"BottomBorderStrokeWeight")
                                        .and_then(|s| s.parse().ok()),
                                    bottom_tint: parse_tint_attr(&e, b"BottomBorderStrokeTint"),
                                    bottom_gap_color: attr(&e, b"BottomBorderStrokeGapColor"),
                                    bottom_gap_tint: parse_tint_attr(
                                        &e,
                                        b"BottomBorderStrokeGapTint",
                                    ),
                                    left_color: attr(&e, b"LeftBorderStrokeColor"),
                                    left_type: attr(&e, b"LeftBorderStrokeType"),
                                    left_weight: attr(&e, b"LeftBorderStrokeWeight")
                                        .and_then(|s| s.parse().ok()),
                                    left_tint: parse_tint_attr(&e, b"LeftBorderStrokeTint"),
                                    left_gap_color: attr(&e, b"LeftBorderStrokeGapColor"),
                                    left_gap_tint: parse_tint_attr(&e, b"LeftBorderStrokeGapTint"),
                                    right_color: attr(&e, b"RightBorderStrokeColor"),
                                    right_type: attr(&e, b"RightBorderStrokeType"),
                                    right_weight: attr(&e, b"RightBorderStrokeWeight")
                                        .and_then(|s| s.parse().ok()),
                                    right_tint: parse_tint_attr(&e, b"RightBorderStrokeTint"),
                                    right_gap_color: attr(&e, b"RightBorderStrokeGapColor"),
                                    right_gap_tint: parse_tint_attr(
                                        &e,
                                        b"RightBorderStrokeGapTint",
                                    ),
                                },
                                row_strokes: TableLineStrokes {
                                    start_count: attr(&e, b"StartRowStrokeCount")
                                        .and_then(|s| s.parse().ok()),
                                    start_color: attr(&e, b"StartRowStrokeColor"),
                                    start_type: attr(&e, b"StartRowStrokeType"),
                                    start_weight: attr(&e, b"StartRowStrokeWeight")
                                        .and_then(|s| s.parse().ok()),
                                    start_tint: parse_tint_attr(&e, b"StartRowStrokeTint"),
                                    start_gap_color: attr(&e, b"StartRowStrokeGapColor"),
                                    start_gap_tint: parse_tint_attr(&e, b"StartRowStrokeGapTint"),
                                    end_count: attr(&e, b"EndRowStrokeCount")
                                        .and_then(|s| s.parse().ok()),
                                    end_color: attr(&e, b"EndRowStrokeColor"),
                                    end_type: attr(&e, b"EndRowStrokeType"),
                                    end_weight: attr(&e, b"EndRowStrokeWeight")
                                        .and_then(|s| s.parse().ok()),
                                    end_tint: parse_tint_attr(&e, b"EndRowStrokeTint"),
                                    end_gap_color: attr(&e, b"EndRowStrokeGapColor"),
                                    end_gap_tint: parse_tint_attr(&e, b"EndRowStrokeGapTint"),
                                },
                                column_strokes: TableLineStrokes {
                                    start_count: attr(&e, b"StartColumnStrokeCount")
                                        .and_then(|s| s.parse().ok()),
                                    start_color: attr(&e, b"StartColumnStrokeColor"),
                                    start_type: attr(&e, b"StartColumnStrokeType"),
                                    start_weight: attr(&e, b"StartColumnStrokeWeight")
                                        .and_then(|s| s.parse().ok()),
                                    start_tint: parse_tint_attr(&e, b"StartColumnStrokeTint"),
                                    start_gap_color: attr(&e, b"StartColumnStrokeGapColor"),
                                    start_gap_tint: parse_tint_attr(
                                        &e,
                                        b"StartColumnStrokeGapTint",
                                    ),
                                    end_count: attr(&e, b"EndColumnStrokeCount")
                                        .and_then(|s| s.parse().ok()),
                                    end_color: attr(&e, b"EndColumnStrokeColor"),
                                    end_type: attr(&e, b"EndColumnStrokeType"),
                                    end_weight: attr(&e, b"EndColumnStrokeWeight")
                                        .and_then(|s| s.parse().ok()),
                                    end_tint: parse_tint_attr(&e, b"EndColumnStrokeTint"),
                                    end_gap_color: attr(&e, b"EndColumnStrokeGapColor"),
                                    end_gap_tint: parse_tint_attr(&e, b"EndColumnStrokeGapTint"),
                                },
                            },
                        });
                    }
                    b"Footnote" => {
                        // Park the host-paragraph/run state on the
                        // new footnote frame; the next
                        // `<ParagraphStyleRange>` will start the
                        // footnote body in a fresh `current_paragraph`.
                        // On `</Footnote>` we restore and attach.
                        footnote_stack.push(FootnoteContext {
                            footnote: Footnote {
                                self_id: attr(&e, b"Self"),
                                paragraphs: Vec::new(),
                            },
                            outer_paragraph: current_paragraph.take(),
                            outer_run: current_run.take(),
                        });
                    }
                    b"Cell" => {
                        // Park outer paragraph/run on the active
                        // table frame so cell content can re-use the
                        // same slots without leaking, and so nested
                        // tables get their own slot.
                        if let Some(ctx) = table_stack.last_mut() {
                            ctx.outer_paragraph = current_paragraph.take();
                            ctx.outer_run = current_run.take();
                        }
                        current_cell = Some(TableCell {
                            self_id: attr(&e, b"Self"),
                            name: attr(&e, b"Name"),
                            row_span: attr(&e, b"RowSpan")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(1),
                            column_span: attr(&e, b"ColumnSpan")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(1),
                            text_top_inset: attr(&e, b"TextTopInset")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0.0),
                            text_left_inset: attr(&e, b"TextLeftInset")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0.0),
                            text_bottom_inset: attr(&e, b"TextBottomInset")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0.0),
                            text_right_inset: attr(&e, b"TextRightInset")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0.0),
                            applied_cell_style: attr(&e, b"AppliedCellStyle"),
                            top_edge_stroke_color: attr(&e, b"TopEdgeStrokeColor"),
                            top_edge_stroke_weight: attr(&e, b"TopEdgeStrokeWeight")
                                .and_then(|s| s.parse().ok()),
                            top_edge_stroke_tint: parse_tint_attr(&e, b"TopEdgeStrokeTint"),
                            bottom_edge_stroke_color: attr(&e, b"BottomEdgeStrokeColor"),
                            bottom_edge_stroke_weight: attr(&e, b"BottomEdgeStrokeWeight")
                                .and_then(|s| s.parse().ok()),
                            bottom_edge_stroke_tint: parse_tint_attr(&e, b"BottomEdgeStrokeTint"),
                            left_edge_stroke_color: attr(&e, b"LeftEdgeStrokeColor"),
                            left_edge_stroke_weight: attr(&e, b"LeftEdgeStrokeWeight")
                                .and_then(|s| s.parse().ok()),
                            left_edge_stroke_tint: parse_tint_attr(&e, b"LeftEdgeStrokeTint"),
                            right_edge_stroke_color: attr(&e, b"RightEdgeStrokeColor"),
                            right_edge_stroke_weight: attr(&e, b"RightEdgeStrokeWeight")
                                .and_then(|s| s.parse().ok()),
                            right_edge_stroke_tint: parse_tint_attr(&e, b"RightEdgeStrokeTint"),
                            fill_color: attr(&e, b"FillColor"),
                            first_baseline_offset: attr(&e, b"FirstBaselineOffset"),
                            minimum_first_baseline_offset: attr(&e, b"MinimumFirstBaselineOffset")
                                .and_then(|s| s.parse().ok()),
                            diagonal: CellDiagonal {
                                left_line_drawn: attr(&e, b"LeftLineDrawn")
                                    .and_then(|s| s.parse().ok()),
                                left_line_color: attr(&e, b"LeftLineStrokeColor"),
                                left_line_weight: attr(&e, b"LeftLineStrokeWeight")
                                    .and_then(|s| s.parse().ok()),
                                left_line_tint: parse_tint_attr(&e, b"LeftLineStrokeTint"),
                                right_line_drawn: attr(&e, b"RightLineDrawn")
                                    .and_then(|s| s.parse().ok()),
                                right_line_color: attr(&e, b"RightLineStrokeColor"),
                                right_line_weight: attr(&e, b"RightLineStrokeWeight")
                                    .and_then(|s| s.parse().ok()),
                                right_line_tint: parse_tint_attr(&e, b"RightLineStrokeTint"),
                                diagonal_in_front: attr(&e, b"DiagonalLineInFront")
                                    .and_then(|s| s.parse().ok()),
                            },
                            rotation_angle: attr(&e, b"RotationAngle").and_then(|s| s.parse().ok()),
                            vertical_justification: attr(&e, b"VerticalJustification"),
                            paragraphs: Vec::new(),
                        });
                    }
                    b"TabStop" => {
                        // <TabStop Position="..." Alignment="..."/>
                        // appears nested inside <TabList><ListItem>.
                        // Append to the open paragraph's list.
                        if let Some(stop) = parse_tab_stop(&e) {
                            if let Some(p) = current_paragraph.as_mut() {
                                p.tab_list.push(stop);
                            }
                        }
                    }
                    b"CharacterStyleRange" => {
                        current_run = Some(CharacterRun {
                            character_style: attr(&e, b"AppliedCharacterStyle"),
                            font: attr(&e, b"AppliedFont"),
                            font_style: attr(&e, b"FontStyle"),
                            point_size: attr(&e, b"PointSize").and_then(|s| s.parse().ok()),
                            fill_color: attr(&e, b"FillColor"),
                            fill_tint: parse_tint_attr(&e, b"FillTint"),
                            capitalization: attr(&e, b"Capitalization"),
                            baseline_shift: attr(&e, b"BaselineShift").and_then(|s| s.parse().ok()),
                            horizontal_scale: attr(&e, b"HorizontalScale")
                                .and_then(|s| s.parse().ok()),
                            vertical_scale: attr(&e, b"VerticalScale").and_then(|s| s.parse().ok()),
                            skew: attr(&e, b"Skew").and_then(|s| s.parse().ok()),
                            position: attr(&e, b"Position"),
                            tracking: attr(&e, b"Tracking").and_then(|s| s.parse().ok()),
                            underline: attr(&e, b"Underline").and_then(|s| s.parse::<bool>().ok()),
                            strikethru: attr(&e, b"StrikeThru")
                                .and_then(|s| s.parse::<bool>().ok()),
                            leading: attr(&e, b"Leading").and_then(|s| s.parse::<f32>().ok()),
                            ruby_flag: attr(&e, b"RubyFlag").and_then(|s| s.parse::<bool>().ok()),
                            ruby_type: attr(&e, b"RubyType"),
                            ruby_string: attr(&e, b"RubyString"),
                            kenten_kind: attr(&e, b"KentenKind"),
                            kenten_character: attr(&e, b"KentenCharacter"),
                            kenten_font_size: attr(&e, b"KentenFontSize")
                                .and_then(|s| s.parse().ok()),
                            overprint_fill: attr(&e, b"OverprintFill")
                                .and_then(|s| s.parse::<bool>().ok()),
                            overprint_stroke: attr(&e, b"OverprintStroke")
                                .and_then(|s| s.parse::<bool>().ok()),
                            // `Swatch/None` is IDML's literal for
                            // "no stroke"; treat it as a missing
                            // colour so the cascade fall-through can
                            // see "no run-level override" rather
                            // than "Swatch/None override".
                            stroke_color: attr(&e, b"StrokeColor").and_then(|s| match s.as_str() {
                                "Swatch/None" | "n" | "" => None,
                                _ => Some(s),
                            }),
                            stroke_weight: attr(&e, b"StrokeWeight")
                                .and_then(|s| s.parse::<f32>().ok()),
                            ligatures_on: attr(&e, b"Ligatures")
                                .and_then(|s| s.parse::<bool>().ok()),
                            kerning_method: attr(&e, b"KerningMethod"),
                            applied_language: attr(&e, b"AppliedLanguage"),
                            // OpenType feature tags have no single IDML
                            // attribute; left None at parse time and
                            // owned by the mutate API as a free-form
                            // authoring string. The discrete parsed
                            // attributes land in `otf` below.
                            otf_features: None,
                            otf: parse_otf_features(&e),
                            applied_conditions: attr(&e, b"AppliedConditions")
                                .map(|s| s.split_whitespace().map(|t| t.to_string()).collect())
                                .unwrap_or_default(),
                            // Inherit the enclosing hyperlink/xref source
                            // span (if any) so the run carries the source
                            // id the designmap's <Hyperlink> references.
                            hyperlink_source: source_stack.last().cloned(),
                            text_variable: None,
                            placeholder: None,
                            text: String::new(),
                        });
                    }
                    b"Content" => {
                        in_content = true;
                    }
                    b"Properties" => {
                        // Disambiguate by which container is currently
                        // open. A Properties child of CharacterStyleRange
                        // takes precedence (current_run is open while we
                        // walk the run's children). The paragraph-level
                        // form fires when we're between runs but still
                        // inside an open ParagraphStyleRange.
                        properties_kind = if current_run.is_some() {
                            1
                        } else if current_paragraph.is_some() {
                            2
                        } else {
                            0
                        };
                    }
                    other if properties_kind != 0 => {
                        // Capture the next Text events as the value of
                        // this typed child element. The `type` attribute
                        // (`string` / `unit` / `enumeration`) is
                        // informational; we infer the destination field
                        // from the element name on End.
                        properties_field = Some(other.to_vec());
                        properties_text.clear();
                    }
                    _ => {}
                } // close inner `match name { ... }`
            }
            Event::End(e) => {
                let n = e.name();
                let name = n.as_ref();
                // Anchored-frame close: pop depth; when an End
                // for a frame element fires we pop the top of
                // the anchored stack and attach it to its parent
                // (Group child) or the host paragraph (outermost).
                if anchored_depth > 0 {
                    anchored_depth -= 1;
                    if anchored_kind_from_name(name).is_some() {
                        finalise_anchored_top(
                            &mut anchored_stack,
                            &mut bounds_from_path,
                            &mut current_paragraph,
                        );
                    }
                    buf.clear();
                    continue;
                }
                if suppress_depth > 0 {
                    suppress_depth -= 1;
                    buf.clear();
                    continue;
                }
                match name {
                    b"Content" => {
                        in_content = false;
                    }
                    b"Properties" => {
                        properties_kind = 0;
                        properties_field = None;
                        properties_text.clear();
                    }
                    name if properties_kind != 0 && properties_field.as_deref() == Some(name) => {
                        let value = properties_text.trim().to_string();
                        match (properties_kind, name) {
                            // CharacterStyleRange Properties.
                            (1, b"AppliedFont") => {
                                if let Some(run) = current_run.as_mut() {
                                    if !value.is_empty() {
                                        run.font = Some(value);
                                    }
                                }
                            }
                            (1, b"FontStyle") => {
                                if let Some(run) = current_run.as_mut() {
                                    if !value.is_empty() {
                                        run.font_style = Some(value);
                                    }
                                }
                            }
                            (1, b"Leading") => {
                                if let Some(run) = current_run.as_mut() {
                                    if let Ok(v) = value.parse::<f32>() {
                                        run.leading = Some(v);
                                    }
                                }
                            }
                            // ParagraphStyleRange Properties: no fields
                            // surfaced on Paragraph yet; the typed
                            // children land in followup parser slices.
                            _ => {}
                        }
                        properties_field = None;
                        properties_text.clear();
                    }
                    b"CharacterStyleRange" => {
                        if let (Some(run), Some(para)) =
                            (current_run.take(), current_paragraph.as_mut())
                        {
                            if !run.text.is_empty() {
                                para.runs.push(run);
                            }
                        }
                    }
                    // W1.4 — close the hyperlink / cross-reference
                    // source span so following runs stop inheriting it.
                    b"HyperlinkTextSource" | b"CrossReferenceSource" => {
                        source_stack.pop();
                    }
                    b"ParagraphStyleRange" => {
                        if let Some(para) = current_paragraph.take() {
                            // Keep paragraphs that have either a
                            // shaped run or a hosted table; drop
                            // truly empty ones.
                            if !para.runs.is_empty() || para.table.is_some() {
                                // Route by parser nesting: footnote
                                // wins over cell wins over story root.
                                // The footnote check has to come
                                // first because a footnote anchored
                                // inside a cell paragraph still wants
                                // its body paragraphs to live on the
                                // footnote, not on the cell.
                                if let Some(ctx) = footnote_stack.last_mut() {
                                    ctx.footnote.paragraphs.push(para);
                                } else if let Some(cell) = current_cell.as_mut() {
                                    cell.paragraphs.push(para);
                                } else {
                                    out.paragraphs.push(para);
                                }
                            }
                        }
                    }
                    b"Cell" => {
                        if let (Some(cell), Some(ctx)) =
                            (current_cell.take(), table_stack.last_mut())
                        {
                            ctx.table.cells.push(cell);
                        }
                        // Restore the outer paragraph/run state from
                        // the active table frame so the next Cell or
                        // the closing Table sees the host paragraph
                        // again. Nested tables: the inner cell's
                        // close restores the inner table's outer
                        // state (which is the inner-table's host
                        // paragraph, i.e. a paragraph inside the
                        // outer table's cell).
                        if let Some(ctx) = table_stack.last_mut() {
                            current_paragraph = ctx.outer_paragraph.take();
                            current_run = ctx.outer_run.take();
                        }
                    }
                    b"Table" => {
                        // Pop the active table frame and attach its
                        // table to the current host paragraph. For
                        // nested tables, the host paragraph is one of
                        // the OUTER table's cell paragraphs (which
                        // were restored by the inner table's last
                        // `</Cell>` close — both sat on the inner
                        // frame's `outer_paragraph` slot). Restore the
                        // outer `current_cell` so the outer cell
                        // continues to accept further content.
                        if let Some(ctx) = table_stack.pop() {
                            if let Some(p) = current_paragraph.as_mut() {
                                p.table = Some(ctx.table);
                            }
                            if ctx.outer_cell.is_some() {
                                current_cell = ctx.outer_cell;
                            }
                        }
                    }
                    b"Footnote" => {
                        // Pop the active footnote frame, restore the
                        // host-paragraph / host-run state, then
                        // attach the captured footnote to the host
                        // paragraph. The host paragraph keeps
                        // accumulating runs after this point.
                        if let Some(ctx) = footnote_stack.pop() {
                            current_paragraph = ctx.outer_paragraph;
                            current_run = ctx.outer_run;
                            if let Some(p) = current_paragraph.as_mut() {
                                p.footnotes.push(ctx.footnote);
                            }
                        }
                    }
                    _ => {}
                } // close inner `match name { ... }`
            }
            Event::Empty(e) => {
                let n = e.name();
                let name = n.as_ref();
                // Anchored-frame self-closing forms. These never
                // visit the End arm so attribute capture must
                // happen inline; nested self-closing frames push
                // and immediately finalise.
                if anchored_depth > 0 {
                    if let Some(kind) = anchored_kind_from_name(name) {
                        // Self-closing nested frame: push, then
                        // finalise so the parent picks it up.
                        let frame = make_anchored_frame(&e, kind);
                        bounds_from_path.push(frame.bounds.is_none());
                        anchored_stack.push(frame);
                        finalise_anchored_top(
                            &mut anchored_stack,
                            &mut bounds_from_path,
                            &mut current_paragraph,
                        );
                    } else if name == b"AnchoredObjectSetting" {
                        if let Some(p) = anchored_stack.last_mut() {
                            p.setting = Some(parse_anchored_object_setting(&e));
                        }
                    } else if name == b"Image" || name == b"Link" {
                        anchored_capture_image_attrs(&e, &mut anchored_stack);
                    } else if name == b"PathPointType" {
                        anchored_extend_path_bounds(&e, &mut anchored_stack, &mut bounds_from_path);
                    }
                    // No depth bump — Empty events have no
                    // matching End.
                    buf.clear();
                    continue;
                }
                // Self-closing suppressed wrappers carry no flow
                // content; drop them. Inline glyph events
                // (`<Br/>` / `<Tab/>` / `<TextVariableInstance/>`)
                // nested inside an open suppressed subtree are
                // also dropped to keep the wrapper truly silent.
                if suppress_depth > 0 || matches!(name, b"HiddenText" | b"Note") {
                    buf.clear();
                    continue;
                }
                // Phase 5 — `<PageReference>` / `<IndexEntry>` /
                // `<Index>` self-closing markers. Capture the
                // indexed term onto the current paragraph so the
                // index-resolution pass can collect entries.
                // IDML serialises both element-only (`<Index ...>`
                // wrapping) and self-closing (`<IndexEntry ...>`
                // marker) forms; both carry the same attributes.
                if matches!(name, b"PageReference" | b"IndexEntry" | b"Index") {
                    if let Some(marker) = parse_index_marker(&e) {
                        if let Some(p) = current_paragraph.as_mut() {
                            p.index_markers.push(marker);
                        }
                    }
                    buf.clear();
                    continue;
                }
                if current_run.is_some() {
                    if let Some(kind) = anchored_kind_from_name(name) {
                        // Self-closing outermost anchored frame:
                        // push + finalise so it lands on the
                        // host paragraph immediately.
                        let frame = make_anchored_frame(&e, kind);
                        bounds_from_path.push(frame.bounds.is_none());
                        anchored_stack.push(frame);
                        finalise_anchored_top(
                            &mut anchored_stack,
                            &mut bounds_from_path,
                            &mut current_paragraph,
                        );
                        buf.clear();
                        continue;
                    }
                }
                match name {
                    // <StoryPreference OpticalMarginAlignment="true"
                    // OpticalMarginSize="12" .../> appears once per
                    // story near the top. Drives hanging punctuation
                    // (`apply_optical_margin` in paged-text) when the
                    // renderer is wired up to call it.
                    b"StoryPreference" => {
                        if let Some(v) = attr(&e, b"OpticalMarginAlignment") {
                            if let Ok(b) = v.parse::<bool>() {
                                out.optical_margin_alignment = b;
                            }
                        }
                        if let Some(v) = attr(&e, b"OpticalMarginSize") {
                            if let Ok(f) = v.parse::<f32>() {
                                out.optical_margin_size = f;
                            }
                        }
                    }
                    // <BulletChar BulletCharacterType="UnicodeWithFont"
                    // BulletCharacterValue="187"/> appears inside
                    // <Properties> of an open <ParagraphStyleRange> as
                    // a local override of the cascaded bullet glyph.
                    // Valid only at paragraph-level Properties (kind 2).
                    b"BulletChar" if properties_kind == 2 => {
                        if let Some(p) = current_paragraph.as_mut() {
                            if let Some(v) = attr(&e, b"BulletCharacterValue") {
                                if let Ok(cp) = v.parse::<u32>() {
                                    p.bullet_character = Some(cp);
                                }
                            }
                        }
                    }
                    // Line breaks inside a paragraph surface as <Br/> — treat
                    // them as a logical newline in the current run.
                    b"Br" => {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push('\n');
                        }
                    }
                    // <TextVariableInstance ResultText="..."
                    // AssociatedTextVariable="TextVariable/<id>" />
                    // appears inside <Content> as a placeholder for a
                    // computed value (running header, file name,
                    // chapter number, …). InDesign bakes the last
                    // composed value into ResultText.
                    //
                    // W1.4: split the instance into its OWN run so the
                    // renderer can re-resolve the value per variable
                    // type at emit time (real page count, document name,
                    // custom content, formatted dates) instead of always
                    // trusting the stale baked string. The dedicated run
                    // clones the open run's style, carries ResultText as
                    // its `text`, and tags `text_variable` with the
                    // associated id. Any text accumulated in the open run
                    // before the instance is flushed first so byte order
                    // is preserved; a fresh continuation run (same style,
                    // empty text) stays open for content after it.
                    b"TextVariableInstance" => {
                        if let Some(run) = current_run.as_mut() {
                            let result_text = attr(&e, b"ResultText").unwrap_or_default();
                            // Flush text that preceded the instance as a
                            // plain run.
                            if !run.text.is_empty() {
                                let mut flushed = run.clone();
                                flushed.text_variable = None;
                                if let Some(para) = current_paragraph.as_mut() {
                                    para.runs.push(flushed);
                                }
                                run.text.clear();
                            }
                            // Emit the variable run (style cloned from the
                            // open run; text = baked ResultText).
                            let mut var_run = run.clone();
                            var_run.text = result_text;
                            var_run.text_variable = attr(&e, b"AssociatedTextVariable");
                            if !var_run.text.is_empty() {
                                if let Some(para) = current_paragraph.as_mut() {
                                    para.runs.push(var_run);
                                }
                            }
                            // `run` continues open with empty text for
                            // any content following the instance.
                        }
                    }
                    // Tab characters surface as <Tab/>; the layout
                    // pass treats '\t' as wide whitespace until a
                    // proper TabList-aware breaker lands.
                    b"Tab" => {
                        if let Some(run) = current_run.as_mut() {
                            run.text.push('\t');
                        }
                    }
                    // Self-closing <TabStop .../> inside the
                    // paragraph's TabList.
                    b"TabStop" => {
                        if let Some(stop) = parse_tab_stop(&e) {
                            if let Some(p) = current_paragraph.as_mut() {
                                p.tab_list.push(stop);
                            }
                        }
                    }
                    // <Row Self="..." Name="..." SingleRowHeight="..."/>
                    b"Row" => {
                        if let Some(ctx) = table_stack.last_mut() {
                            ctx.table.rows.push(TableRow {
                                self_id: attr(&e, b"Self"),
                                name: attr(&e, b"Name"),
                                single_row_height: attr(&e, b"SingleRowHeight")
                                    .and_then(|s| s.parse().ok()),
                                minimum_height: attr(&e, b"MinimumHeight")
                                    .and_then(|s| s.parse().ok()),
                                maximum_height: attr(&e, b"MaximumHeight")
                                    .and_then(|s| s.parse().ok()),
                            });
                        }
                    }
                    // <Column Self="..." Name="..." SingleColumnWidth="..."/>
                    b"Column" => {
                        if let Some(ctx) = table_stack.last_mut() {
                            ctx.table.columns.push(TableColumn {
                                self_id: attr(&e, b"Self"),
                                name: attr(&e, b"Name"),
                                single_column_width: attr(&e, b"SingleColumnWidth")
                                    .and_then(|s| s.parse().ok()),
                            });
                        }
                    }
                    _ => {}
                } // close inner `match name { ... }`
            }
            Event::Text(t) => {
                if in_content {
                    if let Some(run) = current_run.as_mut() {
                        // Normalise Unicode line/paragraph
                        // separators (U+2028, U+2029) emitted by
                        // InDesign for "Forced Line Break"
                        // (Shift+Enter) into `\n`. The downstream
                        // composer splits paragraphs on `\n` and
                        // treats consecutive newlines as empty
                        // sub-paragraphs that advance y_cursor by
                        // one line — which is how InDesign visibly
                        // spaces blocks separated by Shift+Enter.
                        // Without this normalisation the shaper
                        // filters `\u{2028}` as a control glyph
                        // and the visual gap collapses.
                        let raw = t
                            .xml_content(quick_xml::XmlVersion::Implicit1_0)
                            .unwrap_or_default();
                        for ch in raw.chars() {
                            if matches!(ch, '\u{2028}' | '\u{2029}') {
                                run.text.push('\n');
                            } else {
                                run.text.push(ch);
                            }
                        }
                    }
                } else if properties_field.is_some() {
                    properties_text.push_str(
                        &t.xml_content(quick_xml::XmlVersion::Implicit1_0)
                            .unwrap_or_default(),
                    );
                }
            }
            Event::GeneralRef(r) => {
                // quick-xml ≥0.40 emits entity references as their
                // own events, SPLITTING the surrounding text — so
                // `wizard&apos;s` arrives as Text("wizard"),
                // GeneralRef(apos), Text("s"). Dropping them here
                // silently lost the entity characters from run
                // text (apostrophes never parsed → never rendered,
                // never round-tripped). Resolve the predefined
                // five + numeric references and append.
                if in_content || properties_field.is_some() {
                    let name = String::from_utf8_lossy(r.as_ref());
                    let resolved = quick_xml::escape::unescape(&format!("&{name};"))
                        .map(|c| c.into_owned())
                        .unwrap_or_default();
                    if in_content {
                        if let Some(run) = current_run.as_mut() {
                            for ch in resolved.chars() {
                                if matches!(ch, '\u{2028}' | '\u{2029}') {
                                    run.text.push('\n');
                                } else {
                                    run.text.push(ch);
                                }
                            }
                        }
                    } else {
                        properties_text.push_str(&resolved);
                    }
                }
            }
            Event::PI(pi) => {
                // InDesign serialises auto-page-number markers
                // inside <Content> as `<?ACE 18?>` processing
                // instructions. Map them to private-use chars
                // so the renderer can substitute the actual
                // page number per emission. ACE 18 is the
                // current-page-number marker; ACE 19 is the
                // next-page-number marker.
                if in_content {
                    if let Some(run) = current_run.as_mut() {
                        let body = pi.as_ref();
                        let body_str = std::str::from_utf8(body).unwrap_or("");
                        if body_str.trim_start().starts_with("ACE 18") {
                            run.text.push(AUTO_PAGE_NUMBER_MARKER);
                        } else if body_str.trim_start().starts_with("ACE 19") {
                            run.text.push(NEXT_PAGE_NUMBER_MARKER);
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// Parse a "y1 x1 y2 x2" `GeometricBounds` attribute. Local copy
/// (the spread parser owns the public version) so the story parser
/// stays self-contained.
fn parse_bounds_local(s: &str) -> Option<crate::Bounds> {
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() != 4 {
        return None;
    }
    Some(crate::Bounds {
        top: parts[0],
        left: parts[1],
        bottom: parts[2],
        right: parts[3],
    })
}

fn parse_matrix_local(s: &str) -> Option<[f32; 6]> {
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() != 6 {
        return None;
    }
    Some([parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]])
}

/// Capture `<Image>` / `<Link>` attributes onto the top of the
/// anchored stack. Mirrors the Rectangle / Polygon image-link
/// plumbing in `spread.rs`: an `<Image>` element nested inside an
/// anchored Rectangle (or its `<Link>` child) carries the
/// `LinkResourceURI` (or `href`) and the image's pixel-to-frame
/// `ItemTransform`. First-write-wins so the outer `<Image>` beats
/// its nested `<Link>`.
fn anchored_capture_image_attrs(
    e: &quick_xml::events::BytesStart,
    anchored_stack: &mut [AnchoredFrame],
) {
    let Some(top) = anchored_stack.last_mut() else {
        return;
    };
    if let Some(uri) = attr(e, b"LinkResourceURI").or_else(|| attr(e, b"href")) {
        if top.image_link.is_none() {
            top.image_link = Some(uri);
        }
    }
    if e.name().as_ref() == b"Image" {
        if let Some(m) = attr(e, b"ItemTransform").and_then(|s| parse_matrix_local(&s)) {
            if top.image_item_transform.is_none() {
                top.image_item_transform = Some(m);
            }
        }
    }
}

/// Capture a `<PathPointType Anchor="x y" .../>` event and union the
/// anchor coordinate into the top frame's running min/max. Real-world
/// InDesign exports skip the `GeometricBounds` attribute on
/// TextFrames / Rectangles and serialise the geometry as a
/// `<PathPointArray>` of four corner anchors instead — without this
/// fallback, anchored frames in such IDMLs ship `bounds=None` and
/// the renderer can't draw anything because frame_w/frame_h come
/// out as 0. `bounds_from_path` is a parallel stack: `true` ⇒ the
/// top frame's bounds were initialised from a `<PathPointType>` (so
/// extend), `false` ⇒ bounds came from `GeometricBounds` and we
/// leave them alone.
fn anchored_extend_path_bounds(
    e: &quick_xml::events::BytesStart,
    anchored_stack: &mut [AnchoredFrame],
    bounds_from_path: &mut [bool],
) {
    let Some(top) = anchored_stack.last_mut() else {
        return;
    };
    let Some(from_path) = bounds_from_path.last_mut() else {
        return;
    };
    let Some(s) = attr(e, b"Anchor") else {
        return;
    };
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    if parts.len() != 2 {
        return;
    }
    let (x, y) = (parts[0], parts[1]);
    match top.bounds.as_mut() {
        Some(b) if *from_path => {
            b.left = b.left.min(x);
            b.right = b.right.max(x);
            b.top = b.top.min(y);
            b.bottom = b.bottom.max(y);
        }
        Some(_) => {
            // GeometricBounds attribute already pinned the bounds;
            // ignore the path geometry to avoid clobbering it.
        }
        None => {
            top.bounds = Some(crate::Bounds {
                top: y,
                left: x,
                bottom: y,
                right: x,
            });
            *from_path = true;
        }
    }
}

fn parse_anchored_object_setting(e: &quick_xml::events::BytesStart) -> AnchoredObjectSetting {
    AnchoredObjectSetting {
        anchored_position: attr(e, b"AnchoredPosition"),
        spine_relative: attr(e, b"SpineRelative")
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false),
        anchor_x_offset: attr(e, b"AnchorXoffset")
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(0.0),
        anchor_y_offset: attr(e, b"AnchorYoffset")
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(0.0),
        anchor_point: attr(e, b"AnchorPoint"),
        lock_position: attr(e, b"LockPosition")
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(false),
        horizontal_reference_point: attr(e, b"HorizontalReferencePoint"),
        horizontal_alignment: attr(e, b"HorizontalAlignment"),
        vertical_reference_point: attr(e, b"VerticalReferencePoint"),
        vertical_alignment: attr(e, b"VerticalAlignment"),
    }
}

/// Phase 5 — extract an [`IndexMarker`] from a `<PageReference>` /
/// `<IndexEntry>` / `<Index>` self-closing marker element. Returns
/// `None` when the element carries neither `TopicName` nor
/// `AppliedTopic` — without one of those there's nothing to index.
fn parse_index_marker(e: &quick_xml::events::BytesStart) -> Option<IndexMarker> {
    let topic_name = attr(e, b"TopicName");
    let applied_topic = attr(e, b"AppliedTopic");
    // At least one of the two is required for the marker to mean
    // something. When both are absent the element is a structural
    // placeholder we can safely drop.
    if topic_name.is_none() && applied_topic.is_none() {
        return None;
    }
    Some(IndexMarker {
        // Prefer the inline `TopicName`; fall back to the topic id
        // (the renderer's index-resolution pass dereferences this
        // via the document-level Topic table).
        topic_name: topic_name
            .clone()
            .unwrap_or_else(|| applied_topic.clone().unwrap_or_default()),
        applied_topic,
        sort_order: attr(e, b"SortOrder"),
    })
}

fn parse_tab_stop(e: &quick_xml::events::BytesStart) -> Option<TabStop> {
    let position = attr(e, b"Position").and_then(|s| s.parse::<f32>().ok())?;
    Some(TabStop {
        position,
        alignment: attr(e, b"Alignment"),
        alignment_character: attr(e, b"AlignmentCharacter"),
        leader: attr(e, b"Leader"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="u10">
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
      <CharacterStyleRange AppliedCharacterStyle="CharacterStyle/$ID/[No character style]"
                           AppliedFont="Minion Pro" PointSize="11">
        <Content>Hello, </Content>
      </CharacterStyleRange>
      <CharacterStyleRange FontStyle="Bold" AppliedFont="Minion Pro" PointSize="11">
        <Content>world</Content>
      </CharacterStyleRange>
      <CharacterStyleRange AppliedFont="Minion Pro" PointSize="11">
        <Content>.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
      <CharacterStyleRange AppliedFont="Minion Pro" PointSize="11">
        <Content>Second paragraph.</Content>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#;

    #[test]
    fn extracts_paragraphs_and_runs() {
        let s = parse_story(SAMPLE).unwrap();
        assert_eq!(s.paragraphs.len(), 2);

        let p1 = &s.paragraphs[0];
        assert_eq!(p1.paragraph_style.as_deref(), Some("ParagraphStyle/Body"));
        assert_eq!(p1.runs.len(), 3);
        assert_eq!(p1.runs[0].text, "Hello, ");
        assert_eq!(p1.runs[1].text, "world");
        assert_eq!(p1.runs[1].font_style.as_deref(), Some("Bold"));
        assert_eq!(p1.runs[1].point_size, Some(11.0));
        assert_eq!(p1.runs[2].text, ".");

        let p2 = &s.paragraphs[1];
        assert_eq!(p2.runs.len(), 1);
        assert_eq!(p2.runs[0].text, "Second paragraph.");
    }

    #[test]
    fn hyperlink_text_source_tags_enclosed_runs() {
        // W1.4 — a <HyperlinkTextSource> wrapping a CharacterStyleRange
        // tags the enclosed run with its Self id; runs outside stay
        // untagged.
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange><Content>Visit </Content></CharacterStyleRange>
            <HyperlinkTextSource Self="HyperlinkTextSource/src1" Name="web">
              <CharacterStyleRange><Content>paged.media</Content></CharacterStyleRange>
            </HyperlinkTextSource>
            <CharacterStyleRange><Content> now.</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let runs = &s.paragraphs[0].runs;
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].hyperlink_source, None);
        assert_eq!(
            runs[1].hyperlink_source.as_deref(),
            Some("HyperlinkTextSource/src1")
        );
        assert_eq!(runs[1].text, "paged.media");
        assert_eq!(runs[2].hyperlink_source, None);
    }

    #[test]
    fn text_variable_instance_becomes_tagged_run() {
        // W1.4 — a <TextVariableInstance> splits into its own run
        // carrying the AssociatedTextVariable id and the baked
        // ResultText, flushing preceding text into a plain run.
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>Season: </Content>
              <TextVariableInstance ResultText="Spring 2026" AssociatedTextVariable="TextVariable/v1"/>
              <Content>.</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let runs = &s.paragraphs[0].runs;
        // "Season: " | variable run "Spring 2026" | "."
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "Season: ");
        assert_eq!(runs[0].text_variable, None);
        assert_eq!(runs[1].text, "Spring 2026");
        assert_eq!(runs[1].text_variable.as_deref(), Some("TextVariable/v1"));
        assert_eq!(runs[2].text, ".");
        assert_eq!(runs[2].text_variable, None);
    }

    #[test]
    fn track5c_hidden_text_block_drops_content() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <HiddenText><Content>secret</Content></HiddenText>
              <Content>visible</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(s.paragraphs[0].runs.len(), 1);
        assert_eq!(s.paragraphs[0].runs[0].text, "visible");
    }

    #[test]
    fn track5c_note_skipped() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Note><Content>annotation</Content></Note>
              <Content>visible</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(s.paragraphs[0].runs.len(), 1);
        assert_eq!(s.paragraphs[0].runs[0].text, "visible");
    }

    #[test]
    fn br_becomes_newline_in_run_text() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>line one</Content>
              <Br/>
              <Content>line two</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(s.paragraphs[0].runs[0].text, "line one\nline two");
    }

    #[test]
    fn tab_element_becomes_tab_character() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>name</Content>
              <Tab/>
              <Content>value</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(s.paragraphs[0].runs[0].text, "name\tvalue");
    }

    #[test]
    fn tab_list_attaches_to_paragraph() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <Properties>
              <TabList>
                <ListItem><TabStop Position="36" Alignment="LeftAlign"/></ListItem>
                <ListItem><TabStop Position="144" Alignment="RightAlign" Leader="."/></ListItem>
              </TabList>
            </Properties>
            <CharacterStyleRange>
              <Content>x</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let stops = &s.paragraphs[0].tab_list;
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].position, 36.0);
        assert_eq!(stops[0].alignment.as_deref(), Some("LeftAlign"));
        assert_eq!(stops[1].position, 144.0);
        assert_eq!(stops[1].leader.as_deref(), Some("."));
    }

    #[test]
    fn tab_stop_leader_preserves_multichar_and_whitespace() {
        // IDML's `Leader` attribute is a short string the renderer tiles
        // across the snapped tab gap. Multi-character leaders (e.g.
        // `". "` for space-separated dots) and trailing whitespace are
        // significant — the parser must round-trip them verbatim.
        // Raw byte literals can't embed non-ASCII — build the XML as
        // a regular string (which allows `…`) and parse its bytes.
        let xml = r#"<Story>
          <ParagraphStyleRange>
            <Properties>
              <TabList>
                <ListItem><TabStop Position="72" Alignment="RightAlign" Leader=". "/></ListItem>
                <ListItem><TabStop Position="144" Alignment="RightAlign" Leader="-"/></ListItem>
                <ListItem><TabStop Position="216" Alignment="RightAlign" Leader="…"/></ListItem>
              </TabList>
            </Properties>
            <CharacterStyleRange>
              <Content>x</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml.as_bytes()).unwrap();
        let stops = &s.paragraphs[0].tab_list;
        assert_eq!(stops[0].leader.as_deref(), Some(". "));
        assert_eq!(stops[1].leader.as_deref(), Some("-"));
        assert_eq!(stops[2].leader.as_deref(), Some("…"));
    }

    #[test]
    fn parses_table_with_rows_columns_and_cells() {
        // Mirrors the IDML serialisation: a Table nested in a
        // CharacterStyleRange, with Row/Column/Cell siblings inside
        // the Table. Each cell carries its own paragraph + run.
        let xml =
            br#"<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Story Self="s1">
            <ParagraphStyleRange>
              <CharacterStyleRange>
                <Table Self="t1" HeaderRowCount="1" BodyRowCount="2" ColumnCount="2"
                       AppliedTableStyle="TableStyle/Demo">
                  <Row Self="r0" Name="0" SingleRowHeight="20"/>
                  <Row Self="r1" Name="1" SingleRowHeight="18"/>
                  <Column Self="c0" Name="0" SingleColumnWidth="100"/>
                  <Column Self="c1" Name="1" SingleColumnWidth="60"/>
                  <Cell Self="cell00" Name="0:0" RowSpan="1" ColumnSpan="1"
                        TextTopInset="2" TextLeftInset="3"
                        TextBottomInset="2" TextRightInset="3">
                    <ParagraphStyleRange>
                      <CharacterStyleRange>
                        <Content>Header A</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                  <Cell Self="cell10" Name="1:0">
                    <ParagraphStyleRange>
                      <CharacterStyleRange>
                        <Content>Header B</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                  <Cell Self="cell01" Name="0:1">
                    <ParagraphStyleRange>
                      <CharacterStyleRange>
                        <Content>Body A1</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                  <Cell Self="cell11" Name="1:1">
                    <ParagraphStyleRange>
                      <CharacterStyleRange>
                        <Content>Body B1</Content>
                      </CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                </Table>
              </CharacterStyleRange>
            </ParagraphStyleRange>
          </Story>
        </idPkg:Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(s.paragraphs.len(), 1, "table-host paragraph kept");
        let table = s.paragraphs[0]
            .table
            .as_ref()
            .expect("paragraph hosts a table");
        assert_eq!(table.column_count, 2);
        assert_eq!(table.body_row_count, 2);
        assert_eq!(table.header_row_count, 1);
        assert_eq!(
            table.applied_table_style.as_deref(),
            Some("TableStyle/Demo")
        );
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].single_row_height, Some(20.0));
        assert_eq!(table.columns.len(), 2);
        assert_eq!(table.columns[0].single_column_width, Some(100.0));
        assert_eq!(table.cells.len(), 4);
        assert_eq!(table.cells[0].coords(), Some((0, 0)));
        assert_eq!(table.cells[3].coords(), Some((1, 1)));
        // Cell content lives in cell.paragraphs.
        let header_a = &table.cells[0].paragraphs[0].runs[0].text;
        assert_eq!(header_a, "Header A");
        let body_b1 = &table.cells[3].paragraphs[0].runs[0].text;
        assert_eq!(body_b1, "Body B1");
        // Cell insets carried through.
        assert_eq!(table.cells[0].text_top_inset, 2.0);
        assert_eq!(table.cells[0].text_left_inset, 3.0);
    }

    #[test]
    fn parses_cell_vertical_justification() {
        // W3.A1 — `<Cell VerticalJustification="…">` is parsed onto the
        // new `vertical_justification` field so the cell-vjust write
        // path round-trips. Absent attribute ⇒ `None`.
        let xml =
            br#"<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Story Self="s1">
            <ParagraphStyleRange><CharacterStyleRange>
              <Table Self="t1" BodyRowCount="1" ColumnCount="2">
                <Row Self="r0" Name="0"/>
                <Column Self="c0" Name="0"/>
                <Column Self="c1" Name="1"/>
                <Cell Self="cell00" Name="0:0" VerticalJustification="CenterAlign">
                  <ParagraphStyleRange><CharacterStyleRange><Content>X</Content></CharacterStyleRange></ParagraphStyleRange>
                </Cell>
                <Cell Self="cell10" Name="1:0">
                  <ParagraphStyleRange><CharacterStyleRange><Content>Y</Content></CharacterStyleRange></ParagraphStyleRange>
                </Cell>
              </Table>
            </CharacterStyleRange></ParagraphStyleRange>
          </Story>
        </idPkg:Story>"#;
        let s = parse_story(xml).unwrap();
        let table = s.paragraphs[0].table.as_ref().unwrap();
        let cell00 = table
            .cells
            .iter()
            .find(|c| c.coords() == Some((0, 0)))
            .unwrap();
        assert_eq!(
            cell00.vertical_justification.as_deref(),
            Some("CenterAlign")
        );
        let cell10 = table
            .cells
            .iter()
            .find(|c| c.coords() == Some((1, 0)))
            .unwrap();
        assert_eq!(cell10.vertical_justification, None);
    }

    #[test]
    fn parses_table_with_header_body_footer_and_corner_cells() {
        // 3x3 grid with one header row, one body row, one footer row.
        // Covers all four corner cells + verifies the section counts
        // come through as distinct attributes (vs. just BodyRowCount).
        // IDML serialises cells column-major: every cell of column 0
        // before column 1, etc. Cell `Name` is `col:row` zero-indexed
        // (the `coords()` helper returns `(column, row)`).
        let xml =
            br#"<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Story Self="s1">
            <ParagraphStyleRange>
              <CharacterStyleRange>
                <Table Self="t1" HeaderRowCount="1" BodyRowCount="1" FooterRowCount="1"
                       ColumnCount="3">
                  <Row Self="r0" Name="0" SingleRowHeight="24"/>
                  <Row Self="r1" Name="1" SingleRowHeight="36"/>
                  <Row Self="r2" Name="2" SingleRowHeight="20"/>
                  <Column Self="c0" Name="0" SingleColumnWidth="80"/>
                  <Column Self="c1" Name="1" SingleColumnWidth="100"/>
                  <Column Self="c2" Name="2" SingleColumnWidth="60"/>
                  <Cell Self="c00" Name="0:0">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>TL</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                  <Cell Self="c01" Name="0:1">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>ml</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                  <Cell Self="c02" Name="0:2">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>BL</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                  <Cell Self="c10" Name="1:0">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>tm</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                  <Cell Self="c11" Name="1:1">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>mm</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                  <Cell Self="c12" Name="1:2">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>bm</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                  <Cell Self="c20" Name="2:0">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>TR</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                  <Cell Self="c21" Name="2:1">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>mr</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                  <Cell Self="c22" Name="2:2">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>BR</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                </Table>
              </CharacterStyleRange>
            </ParagraphStyleRange>
          </Story>
        </idPkg:Story>"#;
        let s = parse_story(xml).unwrap();
        let table = s.paragraphs[0]
            .table
            .as_ref()
            .expect("paragraph hosts a table");
        assert_eq!(table.header_row_count, 1);
        assert_eq!(table.body_row_count, 1);
        assert_eq!(table.footer_row_count, 1);
        assert_eq!(table.column_count, 3);

        // Row heights / column widths preserved in document order.
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.rows[0].single_row_height, Some(24.0));
        assert_eq!(table.rows[1].single_row_height, Some(36.0));
        assert_eq!(table.rows[2].single_row_height, Some(20.0));
        assert_eq!(table.columns.len(), 3);
        assert_eq!(table.columns[0].single_column_width, Some(80.0));
        assert_eq!(table.columns[1].single_column_width, Some(100.0));
        assert_eq!(table.columns[2].single_column_width, Some(60.0));

        assert_eq!(table.cells.len(), 9);

        // Build a `(col, row) -> &TableCell` lookup so corner
        // assertions are order-independent (IDML serialises
        // column-major, but the test only cares about content).
        let cell_at = |col: u32, row: u32| -> &TableCell {
            table
                .cells
                .iter()
                .find(|c| c.coords() == Some((col, row)))
                .unwrap_or_else(|| panic!("missing cell at ({col}, {row})"))
        };

        // Four-corner content: TL / TR top row (header), BL / BR
        // bottom row (footer). Confirms cells at the extremes parse
        // correctly regardless of serialisation order.
        assert_eq!(cell_at(0, 0).paragraphs[0].runs[0].text, "TL");
        assert_eq!(cell_at(2, 0).paragraphs[0].runs[0].text, "TR");
        assert_eq!(cell_at(0, 2).paragraphs[0].runs[0].text, "BL");
        assert_eq!(cell_at(2, 2).paragraphs[0].runs[0].text, "BR");
        // Interior body-row cell.
        assert_eq!(cell_at(1, 1).paragraphs[0].runs[0].text, "mm");
    }

    #[test]
    fn parses_multi_paragraph_cell_content() {
        // A single cell can host multiple `<ParagraphStyleRange>`
        // children — the parser must append each to
        // `TableCell::paragraphs` instead of collapsing them onto the
        // story-level paragraph list (which would discard cell
        // membership entirely).
        let xml =
            br#"<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Story Self="s1">
            <ParagraphStyleRange>
              <CharacterStyleRange>
                <Table Self="t1" HeaderRowCount="0" BodyRowCount="1" ColumnCount="1">
                  <Row Self="r0" Name="0" SingleRowHeight="60"/>
                  <Column Self="c0" Name="0" SingleColumnWidth="200"/>
                  <Cell Self="c00" Name="0:0">
                    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Heading">
                      <CharacterStyleRange><Content>Title</Content></CharacterStyleRange>
                    </ParagraphStyleRange>
                    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
                      <CharacterStyleRange><Content>First body paragraph.</Content></CharacterStyleRange>
                    </ParagraphStyleRange>
                    <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
                      <CharacterStyleRange><Content>Second body paragraph.</Content></CharacterStyleRange>
                    </ParagraphStyleRange>
                  </Cell>
                </Table>
              </CharacterStyleRange>
            </ParagraphStyleRange>
          </Story>
        </idPkg:Story>"#;
        let s = parse_story(xml).unwrap();
        // Cell content stays nested in the cell — story-level
        // paragraphs should only carry the table-host paragraph,
        // never the cell's own paragraphs.
        assert_eq!(s.paragraphs.len(), 1);
        let cell = &s.paragraphs[0].table.as_ref().unwrap().cells[0];
        assert_eq!(
            cell.paragraphs.len(),
            3,
            "all three cell paragraphs retained"
        );
        assert_eq!(
            cell.paragraphs[0].paragraph_style.as_deref(),
            Some("ParagraphStyle/Heading")
        );
        assert_eq!(cell.paragraphs[0].runs[0].text, "Title");
        assert_eq!(
            cell.paragraphs[1].paragraph_style.as_deref(),
            Some("ParagraphStyle/Body")
        );
        assert_eq!(cell.paragraphs[1].runs[0].text, "First body paragraph.");
        assert_eq!(cell.paragraphs[2].runs[0].text, "Second body paragraph.");
    }

    #[test]
    fn parses_table_repeating_header_footer_and_row_max_min_height() {
        // `RepeatingHeader` / `RepeatingFooter` toggle whether the
        // header / footer rows duplicate at frame splits when a
        // table flows across a NextTextFrame chain. `MaximumHeight`
        // caps content-driven row growth.
        let xml =
            br#"<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Story Self="s1">
            <ParagraphStyleRange>
              <CharacterStyleRange>
                <Table Self="t1" HeaderRowCount="1" BodyRowCount="1" FooterRowCount="1"
                       ColumnCount="1" RepeatingHeader="false" RepeatingFooter="true">
                  <Row Self="r0" Name="0" SingleRowHeight="20" MinimumHeight="18" MaximumHeight="200"/>
                  <Row Self="r1" Name="1" SingleRowHeight="30"/>
                  <Row Self="r2" Name="2" SingleRowHeight="20"/>
                  <Column Self="c0" Name="0" SingleColumnWidth="100"/>
                  <Cell Self="c00" Name="0:0"><ParagraphStyleRange><CharacterStyleRange>
                    <Content>H</Content>
                  </CharacterStyleRange></ParagraphStyleRange></Cell>
                  <Cell Self="c01" Name="0:1"><ParagraphStyleRange><CharacterStyleRange>
                    <Content>B</Content>
                  </CharacterStyleRange></ParagraphStyleRange></Cell>
                  <Cell Self="c02" Name="0:2"><ParagraphStyleRange><CharacterStyleRange>
                    <Content>F</Content>
                  </CharacterStyleRange></ParagraphStyleRange></Cell>
                </Table>
              </CharacterStyleRange>
            </ParagraphStyleRange>
          </Story>
        </idPkg:Story>"#;
        let s = parse_story(xml).unwrap();
        let table = s.paragraphs[0].table.as_ref().unwrap();
        assert_eq!(table.repeating_header, Some(false));
        assert_eq!(table.repeating_footer, Some(true));
        assert_eq!(table.rows[0].minimum_height, Some(18.0));
        assert_eq!(table.rows[0].maximum_height, Some(200.0));
        // Absent on r1 / r2 — kept as None so the renderer treats
        // them as unbounded.
        assert_eq!(table.rows[1].maximum_height, None);
        assert_eq!(table.rows[2].minimum_height, None);
    }

    #[test]
    fn parses_drop_cap_attributes_on_paragraph_style_range() {
        let xml = br#"<Story>
          <ParagraphStyleRange DropCapCharacters="1" DropCapLines="3" DropCapDetail="0">
            <CharacterStyleRange><Content>The quick brown fox</Content></CharacterStyleRange>
          </ParagraphStyleRange>
          <ParagraphStyleRange>
            <CharacterStyleRange><Content>No drop cap here.</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(s.paragraphs.len(), 2);
        assert_eq!(s.paragraphs[0].drop_cap_characters, 1);
        assert_eq!(s.paragraphs[0].drop_cap_lines, 3);
        assert_eq!(s.paragraphs[0].drop_cap_detail, 0);
        // No drop cap on the second paragraph — fields default to 0.
        assert_eq!(s.paragraphs[1].drop_cap_characters, 0);
        assert_eq!(s.paragraphs[1].drop_cap_lines, 0);
    }

    #[test]
    fn anchored_text_frame_captures_full_attribute_set() {
        // Mirror anchored.idml's `with-text-wrap` variant: the
        // anchored TextFrame ships FillColor / StrokeColor /
        // StrokeWeight / AppliedObjectStyle in addition to the
        // geometry pair. The renderer paints these instead of the
        // fallback frame fill once Task-1 lands.
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>before</Content>
              <TextFrame Self="anchor1" ParentStory="u99"
                         GeometricBounds="0 0 36 60"
                         ItemTransform="1 0 0 1 0 0"
                         FillColor="Color/Paper"
                         StrokeColor="Color/Black"
                         StrokeWeight="0.5"
                         FillTint="50"
                         GradientFillAngle="45"
                         AppliedObjectStyle="ObjectStyle/$ID/[None]">
                <AnchoredObjectSetting AnchoredPosition="InlinePosition"/>
              </TextFrame>
              <Content>after</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let af = &s.paragraphs[0].anchored_frames[0];
        assert_eq!(af.fill_color.as_deref(), Some("Color/Paper"));
        assert_eq!(af.stroke_color.as_deref(), Some("Color/Black"));
        assert_eq!(af.stroke_weight, Some(0.5));
        assert_eq!(af.fill_tint, Some(50.0));
        assert_eq!(af.gradient_fill_angle, Some(45.0));
        assert_eq!(
            af.applied_object_style.as_deref(),
            Some("ObjectStyle/$ID/[None]")
        );
        assert!(af.image_link.is_none());
        assert!(af.children.is_empty());
    }

    #[test]
    fn anchored_text_frame_derives_bounds_from_path_point_array() {
        // Real-world InDesign exports skip `GeometricBounds` and
        // serialise the geometry via a `<PathPointArray>` of corner
        // anchors. Without this fallback the renderer ships
        // `bounds=None` and draws nothing.
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <TextFrame Self="anchor1" ParentStory="u99"
                         FillColor="Color/Paper"
                         StrokeColor="Color/Black"
                         StrokeWeight="0.5">
                <Properties>
                  <PathGeometry>
                    <GeometryPathType PathOpen="false">
                      <PathPointArray>
                        <PathPointType Anchor="0 0"
                                       LeftDirection="0 0"
                                       RightDirection="0 0"/>
                        <PathPointType Anchor="0 36"
                                       LeftDirection="0 36"
                                       RightDirection="0 36"/>
                        <PathPointType Anchor="60 36"
                                       LeftDirection="60 36"
                                       RightDirection="60 36"/>
                        <PathPointType Anchor="60 0"
                                       LeftDirection="60 0"
                                       RightDirection="60 0"/>
                      </PathPointArray>
                    </GeometryPathType>
                  </PathGeometry>
                </Properties>
                <AnchoredObjectSetting AnchoredPosition="InlinePosition"/>
              </TextFrame>
              <Content>x</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let af = &s.paragraphs[0].anchored_frames[0];
        let b = af.bounds.expect("bounds derived from path");
        assert_eq!(b.left, 0.0);
        assert_eq!(b.top, 0.0);
        assert_eq!(b.right, 60.0);
        assert_eq!(b.bottom, 36.0);
        // Cross-cutting attrs from Task-1 still populate.
        assert_eq!(af.fill_color.as_deref(), Some("Color/Paper"));
    }

    #[test]
    fn anchored_geometric_bounds_attribute_wins_over_path_points() {
        // When both `GeometricBounds` and a `<PathPointArray>` are
        // present, the explicit attribute wins. The parser must
        // refuse to clobber it with the path's min/max.
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <TextFrame Self="a1" GeometricBounds="0 0 50 80">
                <Properties>
                  <PathGeometry>
                    <GeometryPathType>
                      <PathPointArray>
                        <PathPointType Anchor="-100 -100"/>
                        <PathPointType Anchor="999 999"/>
                      </PathPointArray>
                    </GeometryPathType>
                  </PathGeometry>
                </Properties>
              </TextFrame>
              <Content>x</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let b = s.paragraphs[0].anchored_frames[0]
            .bounds
            .expect("GeometricBounds attribute");
        assert_eq!(b.right, 80.0, "GeometricBounds wins, not path -100..999");
        assert_eq!(b.bottom, 50.0);
    }

    #[test]
    fn anchored_rectangle_captures_image_link_and_item_transform() {
        // An anchored Rectangle that hosts a placed image carries an
        // `<Image>` + `<Link>` pair inside its body, just like a
        // spread-level Rectangle. The parser must mirror that
        // plumbing onto the anchored record.
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Rectangle Self="r1" GeometricBounds="0 0 50 80"
                         FillColor="Swatch/None">
                <Image Self="img1" ItemTransform="0.5 0 0 0.5 10 20">
                  <Link Self="link1" LinkResourceURI="file:///tmp/photo.jpg"/>
                </Image>
                <AnchoredObjectSetting AnchoredPosition="InlinePosition"/>
              </Rectangle>
              <Content>x</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let af = &s.paragraphs[0].anchored_frames[0];
        assert_eq!(af.frame_kind, AnchoredFrameKind::Rectangle);
        assert_eq!(af.image_link.as_deref(), Some("file:///tmp/photo.jpg"));
        assert_eq!(
            af.image_item_transform,
            Some([0.5, 0.0, 0.0, 0.5, 10.0, 20.0])
        );
        assert_eq!(af.fill_color.as_deref(), Some("Swatch/None"));
    }

    #[test]
    fn anchored_group_recurses_into_children() {
        // An anchored `<Group>` wraps one or more page-items. The
        // parser must surface each child as a fully populated
        // `AnchoredFrame` so the renderer can walk the group's
        // z-order and emit each item with its own attributes.
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Group Self="g1" GeometricBounds="0 0 80 100"
                     ItemTransform="1 0 0 1 5 7">
                <Rectangle Self="rA" GeometricBounds="0 0 30 40"
                           FillColor="Color/Red" StrokeWeight="1"/>
                <TextFrame Self="tB" ParentStory="uABC"
                           GeometricBounds="0 0 20 60"
                           FillColor="Color/Paper"/>
                <AnchoredObjectSetting AnchoredPosition="InlinePosition"/>
              </Group>
              <Content>x</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let g = &s.paragraphs[0].anchored_frames[0];
        assert_eq!(g.frame_kind, AnchoredFrameKind::Group);
        assert_eq!(g.children.len(), 2, "group exposes its child frames");
        assert_eq!(g.children[0].frame_kind, AnchoredFrameKind::Rectangle);
        assert_eq!(g.children[0].fill_color.as_deref(), Some("Color/Red"));
        assert_eq!(g.children[0].stroke_weight, Some(1.0));
        assert_eq!(g.children[1].frame_kind, AnchoredFrameKind::TextFrame);
        assert_eq!(g.children[1].parent_story.as_deref(), Some("uABC"));
        assert_eq!(g.children[1].fill_color.as_deref(), Some("Color/Paper"));
        // The setting on the outer Group is captured (sits at the
        // top of the stack while children are pushed and popped).
        assert!(g.setting.is_some());
    }

    #[test]
    fn anchored_text_frame_inside_character_style_range_is_captured() {
        // A `<TextFrame>` nested under a `<CharacterStyleRange>` is
        // an inline-anchored object. It should NOT be parsed as
        // story content (no nested <Content> picked up); the
        // pending record should land on the host paragraph's
        // `anchored_frames` with the parent_story reference and
        // the AnchoredObjectSetting block.
        let xml = br#"<Story Self="u1">
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>Before the marker</Content>
              <TextFrame Self="anchor1" ParentStory="u99"
                         GeometricBounds="0 0 50 80"
                         ItemTransform="1 0 0 1 5 7">
                <Properties/>
                <AnchoredObjectSetting AnchoredPosition="InlinePosition"
                                       SpineRelative="false"
                                       AnchorXoffset="0"
                                       AnchorYoffset="-2"
                                       AnchorPoint="TopLeftAnchor"
                                       LockPosition="false"/>
              </TextFrame>
              <Content>After the marker</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(s.paragraphs.len(), 1);
        let p = &s.paragraphs[0];
        assert_eq!(p.runs.len(), 1, "the two Content blocks merged");
        assert_eq!(p.runs[0].text, "Before the markerAfter the marker");
        assert_eq!(p.anchored_frames.len(), 1);
        let af = &p.anchored_frames[0];
        assert_eq!(af.frame_kind, AnchoredFrameKind::TextFrame);
        assert_eq!(af.self_id.as_deref(), Some("anchor1"));
        assert_eq!(af.parent_story.as_deref(), Some("u99"));
        let bounds = af.bounds.expect("bounds parsed");
        assert_eq!(bounds.top, 0.0);
        assert_eq!(bounds.right, 80.0);
        assert_eq!(af.item_transform, Some([1.0, 0.0, 0.0, 1.0, 5.0, 7.0]));
        let setting = af.setting.as_ref().expect("anchored object setting");
        assert_eq!(setting.anchored_position.as_deref(), Some("InlinePosition"));
        assert_eq!(setting.anchor_y_offset, -2.0);
        assert_eq!(setting.anchor_point.as_deref(), Some("TopLeftAnchor"));
        assert!(!setting.lock_position);
    }

    #[test]
    fn anchored_rectangle_inside_csr_records_kind_rectangle() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Rectangle Self="r1" GeometricBounds="0 0 30 30"/>
              <Content>x</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let p = &s.paragraphs[0];
        assert_eq!(p.anchored_frames.len(), 1);
        assert_eq!(
            p.anchored_frames[0].frame_kind,
            AnchoredFrameKind::Rectangle
        );
        // Rectangles never carry a parent_story.
        assert!(p.anchored_frames[0].parent_story.is_none());
    }

    #[test]
    fn parses_story_preference_optical_margin() {
        // <StoryPreference OpticalMarginAlignment="true"
        //                  OpticalMarginSize="12"/> populates the
        // story-level fields. Default values when the element is
        // absent should remain false / 0.0.
        let xml =
            br#"<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Story Self="u1">
            <StoryPreference OpticalMarginAlignment="true" OpticalMarginSize="12"/>
            <ParagraphStyleRange>
              <CharacterStyleRange><Content>Hello</Content></CharacterStyleRange>
            </ParagraphStyleRange>
          </Story>
        </idPkg:Story>"#;
        let s = parse_story(xml).unwrap();
        assert!(s.optical_margin_alignment);
        assert_eq!(s.optical_margin_size, 12.0);
        assert_eq!(s.paragraphs.len(), 1);
    }

    #[test]
    fn story_preference_absent_keeps_defaults() {
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange><Content>Hi</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert!(!s.optical_margin_alignment);
        assert_eq!(s.optical_margin_size, 0.0);
    }

    #[test]
    fn story_preference_with_children_still_reads_attributes() {
        // Some IDMLs serialise <StoryPreference> with a `<Properties>`
        // child rather than self-closing. Attributes on the Start
        // event still need to be picked up.
        let xml = br#"<Story>
          <StoryPreference OpticalMarginAlignment="true" OpticalMarginSize="9">
            <Properties/>
          </StoryPreference>
          <ParagraphStyleRange>
            <CharacterStyleRange><Content>X</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert!(s.optical_margin_alignment);
        assert_eq!(s.optical_margin_size, 9.0);
    }

    #[test]
    fn justification_from_idml_covers_every_variant_and_unknowns() {
        // Every documented IDML attribute string maps to its enum
        // variant. Unknown / typo'd strings return `None` so the
        // renderer's fallback (Left alignment) kicks in.
        for (raw, expected) in [
            ("LeftAlign", Justification::LeftAlign),
            ("CenterAlign", Justification::CenterAlign),
            ("RightAlign", Justification::RightAlign),
            ("LeftJustified", Justification::LeftJustified),
            ("CenterJustified", Justification::CenterJustified),
            ("RightJustified", Justification::RightJustified),
            ("FullyJustified", Justification::FullyJustified),
            ("ToBindingSide", Justification::ToBindingSide),
            ("AwayFromBindingSide", Justification::AwayFromBindingSide),
        ] {
            assert_eq!(Justification::from_idml(raw), Some(expected));
            // Round-trip: enum -> string -> enum stays stable.
            assert_eq!(Justification::from_idml(expected.as_idml()), Some(expected));
        }
        assert_eq!(Justification::from_idml("leftalign"), None);
        assert_eq!(Justification::from_idml(""), None);
        assert_eq!(Justification::from_idml("Unknown"), None);
    }

    #[test]
    fn paragraph_style_range_parses_justification_into_enum() {
        let xml = br#"<Story>
          <ParagraphStyleRange Justification="CenterAlign">
            <CharacterStyleRange><Content>X</Content></CharacterStyleRange>
          </ParagraphStyleRange>
          <ParagraphStyleRange Justification="NotARealValue">
            <CharacterStyleRange><Content>Y</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(
            s.paragraphs[0].justification,
            Some(Justification::CenterAlign)
        );
        // Unrecognised string ⇒ None; renderer falls back to Left.
        assert_eq!(s.paragraphs[1].justification, None);
    }

    // ---- CJK Stage 1 (parser surface) ----

    #[test]
    fn parses_story_direction_vertical() {
        let xml = br#"<Story Self="u1" StoryDirection="VerticalWritingDirection">
          <ParagraphStyleRange>
            <CharacterStyleRange><Content>CJK body</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(
            s.story_direction,
            Some(StoryDirection::VerticalWritingDirection)
        );
    }

    #[test]
    fn parses_story_direction_horizontal_explicit() {
        let xml = br#"<Story Self="u1" StoryDirection="HorizontalWritingDirection">
          <ParagraphStyleRange>
            <CharacterStyleRange><Content>x</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(
            s.story_direction,
            Some(StoryDirection::HorizontalWritingDirection)
        );
    }

    #[test]
    fn story_direction_absent_defaults_to_none() {
        let xml = br#"<Story Self="u1">
          <ParagraphStyleRange>
            <CharacterStyleRange><Content>x</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(s.story_direction, None);
    }

    #[test]
    fn paragraph_style_range_parses_kinsoku_and_mojikumi() {
        let xml = br#"<Story Self="u1">
          <ParagraphStyleRange
              KinsokuSet="KinsokuTable/$ID/PhotoshopKinsokuHard"
              KinsokuType="PushIn"
              MojikumiTable="MojikumiTable/$ID/PhotoshopMojikumiSet4"
              MojikumiSet="MojikumiSet/$ID/SomeOldSet">
            <CharacterStyleRange><Content>body</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let p = &s.paragraphs[0];
        assert_eq!(
            p.kinsoku_set.as_deref(),
            Some("KinsokuTable/$ID/PhotoshopKinsokuHard")
        );
        assert_eq!(p.kinsoku_type.as_deref(), Some("PushIn"));
        assert_eq!(
            p.mojikumi_table.as_deref(),
            Some("MojikumiTable/$ID/PhotoshopMojikumiSet4")
        );
        assert_eq!(
            p.mojikumi_set.as_deref(),
            Some("MojikumiSet/$ID/SomeOldSet")
        );
    }

    #[test]
    fn kinsoku_and_mojikumi_default_to_none_when_absent() {
        let xml = br#"<Story Self="u1">
          <ParagraphStyleRange>
            <CharacterStyleRange><Content>x</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let p = &s.paragraphs[0];
        assert!(p.kinsoku_set.is_none());
        assert!(p.kinsoku_type.is_none());
        assert!(p.mojikumi_table.is_none());
        assert!(p.mojikumi_set.is_none());
    }

    #[test]
    fn paragraph_style_range_parses_w02_paragraph_attrs() {
        // W0.2 — LeftIndent / RightIndent / Hyphenation /
        // KeepLinesTogether / KeepWithNext / NumberingFormat and the
        // RuleAbove* family land on the paragraph instance.
        let xml = br#"<Story Self="u1">
          <ParagraphStyleRange
              LeftIndent="18"
              RightIndent="9"
              Hyphenation="false"
              KeepLinesTogether="true"
              KeepWithNext="2"
              NumberingFormat="^#.^t"
              RuleAbove="true"
              RuleAboveLineWeight="1.5"
              RuleAboveColor="Color/Black"
              RuleAboveOffset="3">
            <CharacterStyleRange><Content>body</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let p = &s.paragraphs[0];
        assert_eq!(p.left_indent, Some(18.0));
        assert_eq!(p.right_indent, Some(9.0));
        assert_eq!(p.hyphenation, Some(false));
        assert_eq!(p.keep_lines_together, Some(true));
        assert_eq!(p.keep_with_next, Some(2));
        assert_eq!(p.numbering_format.as_deref(), Some("^#.^t"));
        assert_eq!(p.rule_above.on, Some(true));
        assert_eq!(p.rule_above.weight, Some(1.5));
        assert_eq!(p.rule_above.color.as_deref(), Some("Color/Black"));
        assert_eq!(p.rule_above.offset, Some(3.0));
        // Absent RuleBelow stays all-None.
        assert_eq!(p.rule_below.on, None);
    }

    #[test]
    fn w02_paragraph_attrs_default_to_none_when_absent() {
        let xml = br#"<Story Self="u1">
          <ParagraphStyleRange>
            <CharacterStyleRange><Content>x</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let p = &s.paragraphs[0];
        assert!(p.left_indent.is_none());
        assert!(p.right_indent.is_none());
        assert!(p.hyphenation.is_none());
        assert!(p.keep_lines_together.is_none());
        assert!(p.keep_with_next.is_none());
        assert!(p.numbering_format.is_none());
        assert_eq!(p.rule_above.on, None);
        assert_eq!(p.rule_below.on, None);
    }

    #[test]
    fn character_style_range_parses_ruby_and_kenten() {
        let xml = br#"<Story Self="u1">
          <ParagraphStyleRange>
            <CharacterStyleRange
                RubyFlag="true"
                RubyType="PerCharacter"
                RubyString="furigana"
                KentenKind="SesameDot"
                KentenCharacter=""
                KentenFontSize="50">
              <Content>base</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let r = &s.paragraphs[0].runs[0];
        assert_eq!(r.ruby_flag, Some(true));
        assert_eq!(r.ruby_type.as_deref(), Some("PerCharacter"));
        assert_eq!(r.ruby_string.as_deref(), Some("furigana"));
        assert_eq!(r.kenten_kind.as_deref(), Some("SesameDot"));
        assert_eq!(r.kenten_font_size, Some(50.0));
    }

    #[test]
    fn ruby_and_kenten_default_to_none_when_absent() {
        let xml = br#"<Story Self="u1">
          <ParagraphStyleRange>
            <CharacterStyleRange><Content>plain</Content></CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let r = &s.paragraphs[0].runs[0];
        assert!(r.ruby_flag.is_none());
        assert!(r.ruby_type.is_none());
        assert!(r.ruby_string.is_none());
        assert!(r.kenten_kind.is_none());
        assert!(r.kenten_character.is_none());
        assert!(r.kenten_font_size.is_none());
    }

    #[test]
    fn nested_table_inside_cell_paragraph_round_trips() {
        // A 1-row 1-col outer table whose single cell holds a
        // paragraph that itself hosts a 2-row 2-col inner table.
        // The parser's table-context stack must preserve the outer
        // table state across the inner table's open/close.
        let xml = br#"<Story Self="story1">
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Table Self="t1" HeaderRowCount="0" FooterRowCount="0"
                     BodyRowCount="1" ColumnCount="1">
                <Row Self="t1r0" Name="0" SingleRowHeight="40"/>
                <Column Self="t1c0" Name="0" SingleColumnWidth="100"/>
                <Cell Self="t1.0.0" Name="0:0" RowSpan="1" ColumnSpan="1">
                  <ParagraphStyleRange>
                    <CharacterStyleRange>
                      <Table Self="t2" HeaderRowCount="0" FooterRowCount="0"
                             BodyRowCount="2" ColumnCount="2">
                        <Row Self="t2r0" Name="0" SingleRowHeight="20"/>
                        <Row Self="t2r1" Name="1" SingleRowHeight="20"/>
                        <Column Self="t2c0" Name="0" SingleColumnWidth="50"/>
                        <Column Self="t2c1" Name="1" SingleColumnWidth="50"/>
                        <Cell Self="t2.0.0" Name="0:0">
                          <ParagraphStyleRange>
                            <CharacterStyleRange>
                              <Content>inner-cell-text</Content>
                            </CharacterStyleRange>
                          </ParagraphStyleRange>
                        </Cell>
                      </Table>
                    </CharacterStyleRange>
                  </ParagraphStyleRange>
                </Cell>
              </Table>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let outer = s.paragraphs[0]
            .table
            .as_ref()
            .expect("outer table attached");
        assert_eq!(outer.self_id.as_deref(), Some("t1"));
        assert_eq!(outer.cells.len(), 1);
        assert_eq!(outer.rows.len(), 1);
        assert_eq!(outer.columns.len(), 1);

        // The outer cell's first paragraph hosts the inner table.
        let outer_cell = &outer.cells[0];
        let host_para = outer_cell
            .paragraphs
            .iter()
            .find(|p| p.table.is_some())
            .expect("inner table should be attached to a cell paragraph");
        let inner = host_para.table.as_ref().unwrap();
        assert_eq!(inner.self_id.as_deref(), Some("t2"));
        assert_eq!(inner.rows.len(), 2, "inner rows preserved");
        assert_eq!(inner.columns.len(), 2, "inner columns preserved");
        assert_eq!(inner.cells.len(), 1);

        // The inner cell content survives the round-trip.
        let inner_cell = &inner.cells[0];
        assert_eq!(inner_cell.paragraphs[0].runs[0].text, "inner-cell-text");
    }

    #[test]
    fn index_markers_are_captured_on_host_paragraph() {
        // `<PageReference>` and `<IndexEntry>` self-closing markers
        // inside body runs surface as IndexMarker entries on the
        // host paragraph. The renderer's index-resolution pass
        // collects these, groups by topic, alphabetises, and emits.
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>The apple is a fruit</Content>
              <PageReference Self="PR1" TopicName="Apple" SortOrder="apple"/>
              <Content>. The banana also.</Content>
              <IndexEntry Self="IE1" AppliedTopic="Topic/u42"/>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let para = &s.paragraphs[0];
        assert_eq!(para.index_markers.len(), 2);
        // First marker: explicit topic name.
        assert_eq!(para.index_markers[0].topic_name, "Apple");
        assert_eq!(para.index_markers[0].sort_order.as_deref(), Some("apple"));
        // Second marker: AppliedTopic ref, topic_name falls back to
        // the id (renderer dereferences via the document-level
        // Topic table at resolution time).
        assert_eq!(para.index_markers[1].topic_name, "Topic/u42");
        assert_eq!(
            para.index_markers[1].applied_topic.as_deref(),
            Some("Topic/u42")
        );
    }

    #[test]
    fn footnote_body_is_captured_on_host_paragraph() {
        // A `<Footnote>` element nested inside a CharacterStyleRange
        // carries its own paragraphs (the footnote body). The parser
        // captures it onto the host paragraph's `footnotes` field,
        // and crucially the footnote body text does NOT leak into
        // the host story's runs.
        let xml = br#"<Story>
          <ParagraphStyleRange>
            <CharacterStyleRange>
              <Content>before</Content>
              <Footnote Self="Footnote/u1">
                <ParagraphStyleRange>
                  <CharacterStyleRange>
                    <Content>This is the footnote body.</Content>
                  </CharacterStyleRange>
                </ParagraphStyleRange>
              </Footnote>
              <Content>after</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let host = &s.paragraphs[0];

        // Host runs preserve their content; the footnote body does
        // NOT appear anywhere in the host paragraph's body text.
        let host_text: String = host.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(host_text.contains("before"));
        assert!(host_text.contains("after"));
        assert!(
            !host_text.contains("footnote body"),
            "footnote body leaked into host story: {host_text:?}"
        );

        // The footnote was captured onto the host paragraph with its
        // body paragraphs intact.
        assert_eq!(host.footnotes.len(), 1);
        let fn0 = &host.footnotes[0];
        assert_eq!(fn0.self_id.as_deref(), Some("Footnote/u1"));
        assert_eq!(fn0.paragraphs.len(), 1);
        assert_eq!(fn0.paragraphs[0].runs[0].text, "This is the footnote body.");
    }

    #[test]
    fn overprint_round_trips_on_paragraph_and_run() {
        // `OverprintFill` / `OverprintStroke` lift off the
        // `<ParagraphStyleRange>` (paragraph-level) and the
        // `<CharacterStyleRange>` (run-level). Absent ⇒ `None`.
        let xml = br#"<Story>
          <ParagraphStyleRange OverprintFill="true" OverprintStroke="false">
            <CharacterStyleRange OverprintFill="true">
              <Content>Hello</Content>
            </CharacterStyleRange>
            <CharacterStyleRange>
              <Content>World</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        assert_eq!(s.paragraphs[0].overprint_fill, Some(true));
        assert_eq!(s.paragraphs[0].overprint_stroke, Some(false));
        assert_eq!(s.paragraphs[0].runs[0].overprint_fill, Some(true));
        assert_eq!(s.paragraphs[0].runs[1].overprint_fill, None);
    }

    #[test]
    fn parses_cell_diagonal_strokes_with_tint_and_in_front() {
        // Cell 0:0 carries a TL→BR ("Left") diagonal with a tint;
        // cell 1:0 carries a TR→BL ("Right") diagonal painted in
        // front of the content.
        let xml =
            br#"<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
          <Story Self="s1">
            <ParagraphStyleRange>
              <CharacterStyleRange>
                <Table Self="t1" BodyRowCount="1" ColumnCount="2">
                  <Row Self="r0" Name="0" SingleRowHeight="20"/>
                  <Column Self="c0" Name="0" SingleColumnWidth="100"/>
                  <Column Self="c1" Name="1" SingleColumnWidth="100"/>
                  <Cell Self="cell00" Name="0:0"
                        LeftLineDrawn="true" LeftLineStrokeColor="Color/Red"
                        LeftLineStrokeWeight="1.5" LeftLineStrokeTint="60">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>A</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                  <Cell Self="cell10" Name="1:0"
                        RightLineDrawn="true" RightLineStrokeColor="Color/Blue"
                        RightLineStrokeWeight="2" RightLineStrokeTint="100"
                        DiagonalLineInFront="true">
                    <ParagraphStyleRange><CharacterStyleRange>
                      <Content>B</Content>
                    </CharacterStyleRange></ParagraphStyleRange>
                  </Cell>
                </Table>
              </CharacterStyleRange>
            </ParagraphStyleRange>
          </Story>
        </idPkg:Story>"#;
        let s = parse_story(xml).unwrap();
        let table = s.paragraphs[0].table.as_ref().unwrap();
        let c00 = &table.cells[0].diagonal;
        assert_eq!(c00.left_line_drawn, Some(true));
        assert_eq!(c00.left_line_color.as_deref(), Some("Color/Red"));
        assert_eq!(c00.left_line_weight, Some(1.5));
        assert_eq!(c00.left_line_tint, Some(60.0));
        assert_eq!(c00.right_line_drawn, None);
        assert_eq!(c00.diagonal_in_front, None);
        let c10 = &table.cells[1].diagonal;
        assert_eq!(c10.right_line_drawn, Some(true));
        assert_eq!(c10.right_line_color.as_deref(), Some("Color/Blue"));
        assert_eq!(c10.right_line_weight, Some(2.0));
        assert_eq!(c10.right_line_tint, Some(100.0));
        assert_eq!(c10.diagonal_in_front, Some(true));
        assert_eq!(c10.left_line_drawn, None);
    }

    #[test]
    fn parses_discrete_otf_feature_attributes_on_runs() {
        let xml = br#"<Story Self="u1">
          <ParagraphStyleRange AppliedParagraphStyle="ParagraphStyle/Body">
            <CharacterStyleRange AppliedFont="F" PointSize="10"
                                 OTFFraction="true" OTFOrdinal="true"
                                 OTFSwash="true" OTFDiscretionaryLigature="true"
                                 OTFFigureStyle="ProportionalOldstyle"
                                 OTFStylisticSets="5">
              <Content>1/2 1st</Content>
            </CharacterStyleRange>
            <CharacterStyleRange AppliedFont="F" PointSize="10">
              <Content>plain</Content>
            </CharacterStyleRange>
          </ParagraphStyleRange>
        </Story>"#;
        let s = parse_story(xml).unwrap();
        let otf = &s.paragraphs[0].runs[0].otf;
        assert_eq!(otf.fraction, Some(true));
        assert_eq!(otf.ordinal, Some(true));
        assert_eq!(otf.swash, Some(true));
        assert_eq!(otf.discretionary_ligatures, Some(true));
        assert_eq!(otf.figure_style.as_deref(), Some("ProportionalOldstyle"));
        assert_eq!(otf.stylistic_sets, Some(5));
        // A run with no OTF attributes leaves every field unset.
        assert_eq!(s.paragraphs[0].runs[1].otf, OtfFeatures::default());
    }

    #[test]
    fn otf_features_merge_below_fills_only_unset_fields() {
        let mut top = OtfFeatures {
            fraction: Some(true),
            ..OtfFeatures::default()
        };
        let below = OtfFeatures {
            fraction: Some(false), // already set on top — must NOT override
            ordinal: Some(true),
            figure_style: Some("Lining".to_string()),
            stylistic_sets: Some(2),
            ..OtfFeatures::default()
        };
        top.merge_below(&below);
        assert_eq!(top.fraction, Some(true), "set field wins");
        assert_eq!(top.ordinal, Some(true), "unset field inherits");
        assert_eq!(top.figure_style.as_deref(), Some("Lining"));
        assert_eq!(top.stylistic_sets, Some(2));
    }
}
