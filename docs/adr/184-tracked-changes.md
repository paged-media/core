# ADR 184 — Tracked changes kept, accepted and rejected as InDesign keeps them

- **Status:** Accepted 2026-10-18.
- **Scope:**
  - `paged-model`: `TrackedChange`, `Paragraph::tracked_deletions`, `TRACKED_DELETION_MARKER`.
  - `paged-renderer`: a deletion's place prints nothing.
  - `paged-canvas`: text edits carry deletions; the `trackedChanges` collection.
  - `paged-mutate` / wire, protocol 74: `acceptTrackedChange`, `rejectTrackedChange`.
  - plugin-publish: the importer and writer half.
  - `tools/indesign-export/track-changes-probe`.

## Context

A document saved with Track Changes on holds text that a change inserted and text that a change
deleted. The engine printed inserted text and dropped deleted text on read, which is how InDesign
prints a story, but it also lost the deletions on save and could not show or resolve them.

## What InDesign does (asked of InDesign 20.0.1)

`track-changes-probe` builds a story "Alpha beta gamma delta.", turns Track Changes on, inserts
" INSERTED" after "beta" and deletes "gamma ":

- The package spells each change as `<Change ChangeType="InsertedText" | "DeletedText" UserName
  Date AppliedDocumentUser>` inside the character range. An insertion wraps its `<Content>`; a
  deletion holds its own paragraph and character ranges.
- Its PDF prints the story as if every change were accepted: "Alpha beta INSERTED delta.".
- Accepting the insertion and rejecting the deletion reads "Alpha beta INSERTED gamma delta.".

## Decision

- **A deletion is a character of its paragraph** that prints nothing, `TRACKED_DELETION_MARKER`,
  and the paragraph's `tracked_deletions` entry of the same order: its type, user, date, document
  user, its text as InDesign's `contents` reads it, and its children verbatim. This is how editorial
  notes are kept (ADR 172), so a text edit carries a deletion as it carries a note: deleting its
  character deletes it, a split moves it with its text, a merge brings it back.
- **The writer** writes each place's deletion back as it was read; one the model no longer holds is
  not written. An insertion's `<Change>` wrapper stays around its text through edits
  (plugin-publish #53).
- **`trackedChanges`** lists each deletion at the contiguous story char offset of its place.
- **Protocol 74:** `acceptTrackedChange {storyId, offset}` takes the place and the entry out, the
  text stays deleted; `rejectTrackedChange` puts the deleted text back where the place stood. Each
  undoes in one step through the inverse-only `RestoreTrackedChange`.

## Consequences

- Insertions are not listed and cannot be accepted or rejected yet: their text reads as the story's.
- Recording changes while typing (Track Changes on in the editor) is not built.
