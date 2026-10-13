# ADR 136 — Tables and footnote references are characters of their line, and the footnote area sits as InDesign sets it

- **Status:** Accepted 2026-10-13.
- **Scope:** `paged-model` (`Paragraph::table_anchor`, `FOOTNOTE_REFERENCE_MARKER`,
  `is_generated_marker`, `FootnoteOptions::{start_at, numbering_style, label}`,
  `Paragraph::shift_anchors`), `paged-renderer` (`pipeline/build_engine.rs`: the table split and
  the footnote queue; `pipeline/footnotes.rs`: the footnote area), `paged-gen`
  (`table-inline-text`, `footnote-references`), `tools/indesign-export/footnote-pool-probe.*`. The
  IDML spelling is plugin-publish's (`idml-import` `story.rs` and `designmap.rs`, `idml-export`
  `rewrite.rs`). Every model addition is serde-default: no protocol bump.

## Context

InDesign's story holds one character where a table stands and one (U+0004) where a footnote's
reference stands. The engine modelled neither. A table that shared a `<CharacterStyleRange>` with
text was set after all of the paragraph's text, so "Before table … After table" printed as one line
above the table. A footnote's reference printed nothing, and the note printed a number the engine
made up. After the importer learned where references stand (ADR 034's anchor), the footnote area
was floored at the reference line's baseline, which overlapped that line.

InDesign 20.0.1 was asked (ADR 120):

- `table-inline-text`: nine stories, one per case, written as InDesign's own export spells them.
- `footnote-references`: references inside a line, `StartAt` 3, notes whose text holds InDesign's
  `<?ACE 4?>` and a tab.
- `footnote-pool-probe.jsx`: reopens `footnotes` eleven times, changes one footnote option, the
  body's leading or a note through the DOM each time, and reads back the frame's bounds and every
  body and note baseline.

## Decision

**A table is a character of its paragraph.** `Paragraph::table_anchor` is the character offset
where the table stands. The text before it ends a line, the table takes a line of its own, and the
text after it starts the next. All of these are lines of the one paragraph: the text after the table
takes no list number, no space before and no first-line indent. The line before the table is
justified like any line that does not end its paragraph. The table's line takes the paragraph's
alignment. A paragraph mark before the table (`text<Br/><Table/>`) still makes two paragraphs, the
table opening the second.

**A reference is a character of its line.** The importer leaves `FOOTNOTE_REFERENCE_MARKER` at
each footnote's anchor. The renderer prints the note's label in its place, in the formatting of
the run that holds it (InDesign writes the reference's superscript and marker style on a range of
its own), so it takes width and can move a line break. A note prints its number where its own text
holds the marker (InDesign's `<?ACE 4?>`). A note without one prints no number, as InDesign's
reference PDF of the `footnotes` fixture shows. Labels count from `StartAt` in
`FootnoteNumberingStyle`; a style the engine does not spell prints Arabic.

**The footnote area sits on the frame's foot** (the probe):

- the last kept line's baseline is the frame's content bottom;
- a note's first line sits its first-baseline offset below the previous note's last baseline plus
  `SpaceBetween`; the offset is the line's leading, InDesign's default;
- a taller area keeps as many lines as fit with its top no higher than the lowest reference
  line's baseline plus `Spacer`, and the rest are overset (`Spacer` 0, 4 and 12 keep three, two
  and two lines of the fixture's third note);
- the separator rule is centred its `RuleOffset` above the area's top and takes no room: neither
  its offset nor its weight moves a line.

`footnotes` now measures mean ΔE 0.068, p99 0.2, SSIM 0.9989. It was 0.434, 2.67 and 0.9725, with
no references or numbers drawn.

**Writing back.** The writer turns the page-number and section markers back into `<?ACE 18?>` and
`<?ACE 19?>` inside `<Content>`, where they used to leak out as private-use characters. A
reference's character writes nothing, because its `<Footnote>` element carries it.

## Consequences

- Edits keep the anchors on their characters: `shift_anchors` moves the table's and the
  footnotes' anchors with the anchored frames'.
- Still open: other `FootnoteFirstBaselineOffset` choices (the probe measured `CAP_HEIGHT`) are not
  read; a split note does not continue in the next frame (overset lines are dropped, with a
  `FootnoteOverflow` diagnostic); a minted story writes no `<Footnote>` yet; a break record's line
  index counts within the piece of a split paragraph, as it does for a forced line break.
