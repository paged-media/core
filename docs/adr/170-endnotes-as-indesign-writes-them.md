# ADR 170 — Endnotes, read and numbered as InDesign writes them

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-model`:
    - `ENDNOTE_REFERENCE_MARKER`, `ENDNOTE_NUMBER_MARKER`;
    - `Paragraph.endnote_references` / `.endnote_ranges`;
    - `Story.is_endnote_story`;
    - `DesignMap.endnote_options` (`EndnoteOptions` with `label`, `reference_label` and
      `text_label`).
  - `paged-scene`: `Document::endnote_ordinals`.
  - `paged-renderer`:
    - the two markers print their note's number;
    - segments take their own endnote entries;
    - the tab pass reads the text as shaped.
  - `paged-gen`: the `endnotes` sample.
  - plugin-publish:
    - `idml-import` reads `<Endnote>`, `<EndnoteRange>` with its `<?ACE 4?>`, `<EndnoteOption>`
      and `<EndnoteTextFrame>`;
    - `idml-export` writes the references and numbers back, and knows the endnote frame as a page
      item.

## Context

InDesign 2020 added endnotes, and the engine read none of their markup.
- A document's references printed nothing.
- The endnote story lost each note's number.
- Its frame, an `<EndnoteTextFrame>`, was not read at all, so the endnote story was never placed.

## What InDesign does (asked of InDesign 20.0.1)

`endnote-probe` makes two endnotes in InDesign's DOM and exports the document. The `endnotes`
sample, written in that spelling, gives InDesign's PDF.

**The reference** is `<Endnote Self EndnoteTextRange/>` on a superscript range of its own in the
host text. InDesign prints the note's number there.

**The endnote story** is `<Story IsEndnoteStory="true">`, hosted in an `<EndnoteTextFrame>`, not a
`<TextFrame>`.
- It holds the title (`EndnoteTitle`, written in the UI's language: `Endnoten`), then a `<Br/>` and
  an `<EndnoteRange Self SourceEndnote>` per note.
- A range's text is `\u{FEFF}<?ACE 4?>⇥text\u{FEFF}`: `<?ACE 4?>` is the note's number, and the
  separator follows it.
- Given a `<TextFrame>` instead, InDesign does not take the story for the document's endnotes. It
  numbers every reference with the start number and makes a new endnote story.

**`<EndnoteOption>`** stands in `designmap.xml`.
- Attributes: title and title style, `StartEndnoteNumberAt`, marker style, text style,
  `EndnoteSeparatorText`, prefix and suffix.
- Typed `Properties` children: `EndnoteNumberingStyle`, `RestartEndnoteNumbering`,
  `EndnoteMarkerPositioning`, `ScopeValue`, `FrameCreateOption` and `ShowEndnotePrefixSuffix`.

**Numbering.** The notes are numbered in reference order from the start number, in the numbering
style. Starting at 3 in lower roman prints `iii`, `iv`, `v` at both the references and the notes.

## Decision

**The importer reads all of it:**
- the reference as `ENDNOTE_REFERENCE_MARKER` in the host text, with its `<Endnote>` in
  `Paragraph.endnote_references`;
- the range's `<?ACE 4?>` as `ENDNOTE_NUMBER_MARKER`, with its range in `Paragraph.endnote_ranges`;
- the story flag, the options, and the endnote frame as a text frame.

**The renderer numbers a note by its place in the endnote story,** which InDesign keeps in reference
order. A reference no endnote story lists takes the next number after those that are listed, in
story order.
- The reference prints `EndnoteOptions::reference_label`; the note's own number prints `text_label`.
- Both use the numbering style and the start number. The prefix and suffix are added where
  `ShowEndnotePrefixSuffix` names that side.

**A paragraph split at its breaks hands its endnote entries to the segments their markers stand
in.** The endnote story is one model paragraph with a `\n` per note.

**The tab pass now reads the text as shaped.**
- It rebuilt the paragraph's text from the model's runs, while the cluster offsets index the
  substituted text.
- So a tab after any generated number shorter than its 3-byte marker landed late. That affected an
  endnote's `iv`, and equally a page number before a tab.

**plugin-publish writes it back.**
- A changed host run writes each reference as its `<Endnote Self EndnoteTextRange/>`.
- A changed note writes its number as `<?ACE 4?>` inside `<Content>`.
- The `<EndnoteTextFrame>` counts as a page item, so the writer neither mints it again nor drops it.

## Result

- The `endnotes` fixture is in the fidelity and text gates against InDesign's PDF: references `iii`,
  `iv`, `v` and the notes numbered to match.
- plugin-publish tests: `endnote_references_and_ranges_are_read` and `reads_the_endnote_options`.

## Not yet

- **Endnote ops:** insert, delete, and convert a footnote to an endnote.
- **Editing an endnote's text** in the endnote story: the range is replayed, so an edit inside it does
  not save.
- **Section scope and restarts:** `ScopeValue` section and `RestartEndnoteNumbering` other than
  continuous.
- **Moving the endnote frame:** `<EndnoteTextFrame>` is replayed, not patched.
