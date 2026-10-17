# ADR 169 — Cross-references: a live page number, and Update Cross-References

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-model`:
    - `Story.cross_reference_sources` (`CrossReferenceSourceDef`);
    - `Story.destinations` (`StoryDestination`);
    - `DesignMap.cross_reference_formats` (`CrossReferenceFormat`, `CrossReferenceBlock`).
  - `paged-renderer`:
    - `XRefPageNumber` resolved from the destination's laid-out page, on the post-layout pass;
    - `pipeline::story_list_labels`.
  - `paged-wire`: `Mutation::UpdateCrossReferences`.
  - `paged-canvas`: lowered to `ReplaceStoryParagraphs`.
  - `paged-script`: `paged.updateCrossReferences`.
  - `paged-gen`: the `xref` sample.
  - `tools/indesign-export`: `xref-probe`, `xref-update-probe`.
  - plugin-publish `idml-import`: formats, sources and destinations read in InDesign's spelling.

## Context

A cross-reference source tagged its runs like a hyperlink source, and nothing more was modelled:
- its format and the format's building blocks were not read;
- a `<ParagraphDestination>`, where InDesign points a reference at a paragraph, was not read, so
  such a reference pointed nowhere;
- the page number printed its cached `ResultText`;
- nothing could update a reference.

## What InDesign does (asked of InDesign 20.0.1)

`xref-probe` builds references in each default format in InDesign's DOM. `xref-update-probe` opens
the `xref` sample, whose sources are stale (the text names "Old Heading", the page variable
stores 9). It records each source before and after Update Cross-References. Its record is
`tools/indesign-export/xref-update-probe.json`.

**A format is a list of building blocks,** stored in the document in its own words. InDesign
writes its default formats in the UI's language (`„…“ auf Seite` in a German install).
- Each `<CrossReferenceFormat>` holds `<BuildingBlock BlockType CustomText AppliedCharacterStyle
  AppliedDelimiter IncludeDelimiter>`s.
- The custom text is verbatim, so `” on page ` keeps its spaces.

**A source is a `<CrossReferenceSource AppliedFormat>`** wrapping its text.
- Its page number is a range with `PageNumberType="TextVariable"`, holding a
  `<TextVariableInstance … AssociatedTextVariable="…XRefPageNumber" ResultText>`.
- A `<Hyperlink Source>` points it at a destination: a `<ParagraphDestination>` at the start of the
  paragraph, or a `<HyperlinkTextDestination>` at a point in it. Both stand inline in the story.

**The page number is live.** With the stored 9 and the heading on page 2, InDesign prints `page 2`,
before any update.

**The text is not live.** InDesign prints the stored "Old Heading" until the update. The update
rewrites the text blocks:

| Block | InDesign writes |
|---|---|
| Full paragraph | `1. Where Things Are`: the list label `1.⇥` with its tab as a space, then the text |
| Paragraph text | `Where Things Are` |
| Paragraph number | `1`: the label without its separator |
| Text anchor name | `the anchor` for a text destination; empty for a paragraph destination |
| Custom string | its text |
| Page number | stays the variable |

## Decision

**The model reads what InDesign writes:**
- formats with their blocks;
- each story's sources with their format;
- each story's destinations, with the paragraph and offset where they stand;
- a paragraph destination is registered like a text anchor, so links resolve to it.

**The renderer prints a reference's page number live.**
- A document with an `XrefPageNumberType` variable takes the post-layout pass (as running headers
  do).
- The pass maps each source to the page its destination paragraph was laid out on. A run of the
  variable inside the source prints that page's label.

**`UpdateCrossReferences { sourceId? }` rewrites the text blocks.** It updates one source, or every
source when no id is given, as Update All Cross-References does.
- Each block becomes a run with the first old run's attributes, still tagged with the source. A
  block's own character style, or the format's, applies.
- The page number stays a run of the variable, printing the current page.
- The list label comes from `story_list_labels`, which numbers the destination's story as the
  emitter does.
- It lowers to one `ReplaceStoryParagraphs` per story it changes, so undo is exact. An unknown
  source is refused.

## Result

- `xref_ops` (canvas) tests:
  - a stale source prints its stored text;
  - Update All writes exactly InDesign's five texts;
  - undo restores the stale ones;
  - updating one source leaves the others;
  - the update survives a save and a reload.
- plugin-publish tests: `reads_cross_reference_formats_as_indesign_writes_them` and
  `cross_reference_sources_and_destinations_are_recorded`.
- The `xref` fixture is in the fidelity and text gates against InDesign's PDF. That PDF shows the
  stale text with the live `page 2`, and so does the engine.

## Not yet

- **A list continued from another story,** for a paragraph number. `story_list_labels` counts within
  the destination's story.
- **Chapter-number blocks across a book** (D-10), which print empty.
- **Inserting a cross-reference** and editing formats.
- **Stale references reported as preflight findings.**
