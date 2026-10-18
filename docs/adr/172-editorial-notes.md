# ADR 172 — Editorial notes, kept and edited as InDesign keeps them

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-model`: `NOTE_MARKER`, `Note`, `Paragraph.notes` (kept on their characters by
    `shift_anchors`).
  - `paged-renderer` / `paged-text`: a note's place prints nothing.
  - `paged-mutate`: `InsertNote`, `EditNote`, `DeleteNote`.
  - `paged-wire` / `paged-canvas`:
    - the `insertNote`, `editNote`, `deleteNote` and `convertNoteToText` mutations;
    - the `notes` collection;
    - a text delete, split or merge carries the notes with their characters.
  - `paged-script`: `paged.insertNote`, `editNote`, `deleteNote`, `convertNoteToText`.
  - `paged-gen`: the `notes` sample. `tools/indesign-export/notes-probe`.
  - plugin-publish:
    - `idml-import` reads `<Note>`;
    - `idml-export` writes each note back, and saves a story holding notes whole.

## Context

The importer dropped every `<Note>`: a document's editorial notes were gone on open, and an
editor had no way to see, add or answer one.

The writer was worse off. It read a note's own ranges as the story's next paragraph, so even
an unedited save of a story with a note dropped the note's start tag and the text before it,
and wrote malformed XML. That was fixed first: the writer carries a note, hidden text or a
tracked deletion whole.

## What InDesign does (asked of InDesign 20.0.1)

`notes-probe` adds notes in InDesign's DOM and exports the document. With `PROBE_IN`, it opens
a document and reports the notes InDesign reads in it.

**A note is a character of the story.** InDesign keeps a U+FEFF where it stands.
- The story's length counts it.
- `note.storyOffset` is its index.
- Text typed before it moves it on by what was typed.
- The PDF prints none of it.

**Its markup** is `<Note Collapsed CreationDate ModificationDate UserName
AppliedDocumentUser>`, inline in the character range that holds it.
- It has no `Self`.
- It holds a range of its own, in `[No paragraph style]` and `[No character style]`.
- A `<Br/>` separates its paragraphs; its `contents` reads them separated by U+000D.
- `AppliedDocumentUser` names a design-map `<DocumentUser Self UserName>` with a `UserColor`.

**Deleting a note** removes its character.

**Convert to Text** replaces the note's character with a space and the note's text. A note of
two paragraphs makes the story's paragraph break there.

In three probes, conversion also dropped the last character of two of three notes. It did so
whichever order they were converted in, and kept the third whole. No rule explained which
notes lost it. The engine does not copy that and converts the whole text.

## Decision

**A note is a character of its paragraph** (`NOTE_MARKER`) and a `Note` on the paragraph.
- The k-th marker is the k-th note, as with endnote references.
- The note also keeps its `anchor`, so the edits that move footnotes move it too.
- The renderer prints nothing at the marker. It is a zero-length generated span, so the line's
  byte mapping still counts the marker's model bytes.
- Text extractors drop it, and a path that shapes it unsubstituted draws nothing.

**InDesign writes no id for a note.** The importer numbers a story's notes and prefixes the
story's id. The canvas mints an id per insert.

**The ops:**
- `insertNote` takes the note's place (the footnote address space) and its text. `user` and
  `date` are its `UserName` and its dates: the engine reads no clock, so the host passes them.
- `editNote` sets the text. Without a date, the note keeps its modification date.
- `deleteNote` removes the note and its character; undo puts the note back as it was.
- `convertNoteToText` lowers to `ReplaceStoryParagraphs`.

Each op is one undo step. A text delete over a note's character takes the note, as InDesign
deletes it with the character.

**plugin-publish:**
- The reader leaves the marker and reads the note with its attributes.
- The writer keeps each `<Note>` subtree with its run:
  - an unedited story saves byte-identically;
  - a rewritten run writes back the source subtree of every unchanged note;
  - a new or edited note is a `<Note>` written from the model.
- A run whose notes changed is rewritten even when its text did not change.
- A story with no source part writes its notes from the model.

## Result

- The `notes` fixture is in the fidelity and text gates against InDesign's PDF, which prints
  none of the four notes. With `PROBE_IN`, InDesign reads all four of them from the sample, at
  their offsets.
- Tests:
  - `paged-canvas`: `note_ops` (read, insert, edit, delete, convert, a delete over a note, and
    saved as IDML).
  - plugin-publish: `notes_round_trip` (read, unedited save byte-identical, an edit beside a
    note, a note edited, deleted and inserted).

## Not yet

- **A note's formatting.** An edited note is written as plain text in its default styles. An
  unchanged note keeps its own subtree whole.
- **Document users.** `<DocumentUser>` is replayed. A note inserted with a new author names no
  document user.
- **A note outside a character range,** which InDesign does not write, is replayed and not read.
- **Hidden text** (`<HiddenText>`) is still carried and not read.
