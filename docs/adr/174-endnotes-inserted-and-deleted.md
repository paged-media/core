# ADR 174 — Endnotes inserted, deleted and converted from footnotes, as InDesign does it

- **Status:** Accepted 2026-10-17.
- **Scope:**
  - `paged-model`: `Spread.endnote_frames`.
  - `paged-mutate`: `InsertEndnote`, `DeleteEndnote`, `CreateEndnoteStory`.
  - `paged-wire` / `paged-canvas`:
    - the `insertEndnote`, `deleteEndnote` and `convertFootnoteToEndnote` mutations;
    - the `endnotes` collection.
  - `paged-script`: `paged.insertEndnote`, `deleteEndnote`, `convertFootnoteToEndnote`.
  - `tools/indesign-export/endnote-probe`.
  - plugin-publish:
    - `idml-import` records `<EndnoteTextFrame>` as the endnote frame;
    - `idml-export` writes ranges, references, the endnote frame and the endnote story as
      InDesign does.

## Context

ADR 170 reads endnotes and numbers them as InDesign does, but nothing could add or remove one.
The writer also had gaps:
- It replayed an endnote range verbatim, so an edited endnote's text was not saved.
- It replayed an `<Endnote/>` reference as foreign markup, so a deleted reference came back.
- It wrote the source's `<EndnoteTextFrame>` a second time, because the frame was missing from
  its list of item kinds.

## What InDesign does (asked of InDesign 20.0.1)

`endnote-probe` builds a story with a footnote and endnotes in InDesign's DOM. It reports the
document's pages, stories and frames after each step.

**The first endnote** of a document makes the endnote story (`FrameCreateOption NewPage`):
- InDesign adds a page at the end, holding an `<EndnoteTextFrame>` over its margin box.
- The story in it is `IsEndnoteStory`. It holds the title (`EndnoteTitle`, `Endnoten` in a
  German install), then the note's range.

**The endnote story** keeps one range per note, in reference order: `\u{FEFF}`, the number
(`<?ACE 4?>`), the separator, the text, `\u{FEFF}`. A break separates each range from the one
before. A note inserted before the others takes the first range.

**`deleteEndnote`** removes the reference and the range. With every note deleted, the story
keeps its title, and the page and frame stay.

**Conversion.** InDesign's DOM has no conversion from a footnote. A footnote offers `remove` and
`convertToText`; the UI's Convert Footnote and Endnote is a dialog.

## Decision

**The ops:**
- `insertEndnote` puts the reference at a story offset (the `insertFootnote` address space). It
  puts the range after the range of the reference before it in reference order (stories in
  document order), or first, after the title.
- When the document has no endnote story, the canvas lowers the insert to a batch:
  `CreateEndnoteStory`, then `InsertEndnote`.
  - `CreateEndnoteStory` adds the page after the last one, on that page's master.
  - It places a text frame over the page's margin box and marks it in `Spread.endnote_frames`.
  - The story holds the title, in the title style.
  - Its inverse removes the frame (with the story it made) and the page.
- `deleteEndnote` takes the reference and the range, with the break before it.
- `convertFootnoteToEndnote` is a batch: the footnote's removal, then an endnote of its text at
  its reference's place. These are the two operations InDesign has, in one undo step.
- Every op undoes exactly.

**plugin-publish:**
- `<EndnoteRange>` in a run is the run's text, and its `<?ACE 4?>` is the number marker, as the
  parser reads it. A run that changed is written again around the model's ranges, each in its
  `<EndnoteRange Self SourceEndnote>`.
- `<Endnote/>` is the reference character of its run.
- A minted frame in `Spread.endnote_frames` is written as `<EndnoteTextFrame>`, and a fresh
  endnote story as `IsEndnoteStory="true"`.

## Result

- InDesign reads the saves back as made: `endnote_ops`' ignored `writes_the_saved_endnotes_for_indesign`,
  opened with `endnote-probe` and `PROBE_IN`.
  - The `endnotes` sample with a note inserted and one deleted gives three endnotes, their
    ranges in reference order, in the `<EndnoteTextFrame>`.
  - A first endnote in a document without one gives the endnote story in an
    `<EndnoteTextFrame>` on the new page.
- Tests:
  - `paged-canvas`: `endnote_ops` (reading, inserting in order, deleting, the first endnote's
    story and page, converting a footnote, saving as IDML).
  - plugin-publish: `endnote_story` (InDesign's own endnote story saved byte-identically, a new,
    a deleted and an edited range).

## Not yet

- **Section scope and restarts** (`ScopeValue` section, `RestartEndnoteNumbering`). Notes are
  numbered through the document.
- **The endnote frame's own threading.** InDesign threads more frames, on more pages, as the
  endnote story grows; the engine places the story in its one frame.
- **Converting an endnote back to a footnote.**
