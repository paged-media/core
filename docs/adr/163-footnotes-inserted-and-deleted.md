# ADR 163 — Footnotes inserted and deleted, over the wire and through a save

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-wire`: `Mutation::InsertFootnote`, `Mutation::DeleteFootnote` (protocol 73).
  - `paged-mutate`: `Operation::InsertFootnote`, `Operation::RemoveFootnote` (`apply::footnote`),
    and footnote ids in the id scan.
  - `paged-canvas`:
    - the `footnotes` collection (`FootnoteSummary`);
    - a text delete that covers a reference takes its note;
    - a merge of two paragraphs keeps the end paragraph's notes, table and anchored frames.
  - `paged-script`: `paged.insertFootnote`, `paged.deleteFootnote`.
  - plugin-publish `idml-export`: a run's footnotes follow the model through a save.

## Context

The model carries a footnote as InDesign writes it:
- a reference character in its host paragraph's text, where the note's number prints;
- the note itself on that paragraph, whose own text opens with the same character (its number)
  and the separator.

The engine rendered imported notes (ADRs 156, 160, 161), but nothing could add or remove one.
Text edits did not know about them either:
- Deleting a reference left its note behind. The renderer, finding fewer references than notes,
  fell back to the notes' recorded anchors and still printed it.
- Merging two paragraphs dropped the second paragraph's notes after the deletion.

The IDML writer replayed a run holding a `<Footnote>` verbatim, so neither change could have saved.

## Decision

**`InsertFootnote { storyId, offset, paragraph?, text }` adds both parts.**
- It writes the reference at a story character offset, in the same contiguous address space as
  `InsertHyperlink` and `InsertAnchoredFrame`.
- It writes the note: its number, the document's separator (`SeparatorText`, a tab by default),
  then `text`. The model mints the note's id.
- Every anchor after the reference in its paragraph moves on by one character.
- `paragraph` names the paragraph outright where an offset is ambiguous: the end of one paragraph
  and the start of the next share an offset.
- The inverse is `RemoveFootnote`.

**`DeleteFootnote { storyId, footnoteId }` removes both parts.**
- The reference is found as the renderer finds it: the paragraph's k-th reference character for
  its k-th note when the paragraph holds one per note, the note's anchor otherwise.
- Its inverse puts the note back as it was: its paragraphs travel as a JSON blob on the inverse,
  as table deletes carry theirs. The paragraph is named in the inverse.

**A note goes with its reference in a text edit, as in InDesign.**
- `DeleteRange` drops the notes whose references it deletes.
- When a delete merges two paragraphs, what follows the deletion in the second paragraph moves into
  the first with its text: its notes, its table and its anchored frames.
- Undo restores the paragraphs from the delete's snapshot, notes included.

**The `footnotes` collection lists every note:** its id, story, reference offset, the label it
prints and the page it is set on (from the last layout), and its text.

**A save writes what the model holds** (plugin-publish).
- A `<Footnote>` is the reference character in its run's text, as the parser reads it.
- An unchanged run replays byte for byte.
- A changed run writes its text around each reference and, at each, its note. A note the run held
  already keeps its own bytes. A new one is written as InDesign writes a note:
  `<Content><?ACE 4?>⇥text</Content>`.

## Result

- `footnote_ops` covers:
  - a note inserted mid-story, which numbers in its place and undoes;
  - a note deleted, which takes its reference, and undo restores both exactly;
  - a text delete over a reference, which takes its note;
  - a save that writes both an inserted and a deleted note.
- The render-effect sweep covers both ops.
- The script surface reaches 131 of 132 wire ops.

## Not yet

- **Editing a note's own text.** A note's paragraphs are not addressable by the text ops yet.
- **Endnotes** (L-02).
- **Frame-level footnote options.** A frame's own footnote override has no property path (ADR 160).
