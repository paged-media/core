# ADR 127 — Field offsets, typing at a field, document labels and delete undo

- **Status:** Accepted 2026-10-05.
- **Scope:** `crates/paged-canvas/src/mutate.rs`, `crates/paged-mutate/src/apply/path_topology.rs`,
  `crates/paged-mutate/src/apply/layer.rs`, `crates/paged-model` (`DesignMap::labels`,
  `char_offset_of_content_offset`), the `insertField.contentOffset` field and the
  `setDocumentMetadata` operation (protocol 69)

## Context

The data-publishing plugin places placeholder fields in text, refreshes their values and lowers
records into frames and tables. Running it against the engine at protocol 67 found four problems.

1. **Two offset units.** `insertText`, `deleteRange`, the text selection and the host's caret door
   count UTF-8 bytes plus one synthetic `\n` per paragraph boundary. `insertField`,
   `setFieldValue`, the placeholder enumeration, `applyStyle` and story ranges count characters
   with no separator. Both units are documented. They agree only inside the first paragraph of
   ASCII text, so a field placed at the caret landed early anywhere else.
2. **Typing at a field's edge.** `insertText` at the start of a field that opens its paragraph, or
   at the end of any field, grew the field's own run. The next refresh replaced the run's text
   and removed what the user had typed.
3. **No document-scoped label.** `setPluginMetadata` reaches page items only. State that belongs
   to no frame, such as a data session or the version of a plugin's container parts that is
   live, had no undoable place to live.
4. **Lossy delete undo.** The inverse of `deleteRange` typed the deleted characters back into one
   run. A field came back as plain text and every deleted run lost its formatting. An in-place
   re-lower (clear the story, pour new content) therefore could not be undone to the document it
   replaced.

## Decision

- **The caret unit is accepted where a field is placed.** `insertField` takes an optional
  `contentOffset` in the caret unit. The engine converts it against the story as it is when the
  operation applies, so a batch that types first and then places a field still lands correctly.
  It echoes the char offset it resolved and refuses an offset inside a character or past the end.
  The caret door keeps its documented unit. Changing it would break the `insertText` contract it
  states. The conversion is defined once, in `paged_model::char_offset_of_content_offset`.
- **A field is one character for typing.** Text inserted at either edge of a placeholder or
  text-variable run joins the neighbouring ordinary run. If there is none, it becomes a run of its
  own with the field's formatting and without its identity. This matches InDesign, where a caret
  can sit before or after a text variable but not inside it. Offsets and the inverse are
  unchanged. An insert strictly inside a field's text keeps the old behaviour.
- **`setDocumentMetadata`** writes one Label entry on the document (`DesignMap::labels`). It uses
  the same gate as the page-item carrier: the `x-paged:` namespace, the optional caller namespace,
  a 64 KiB cap and the JSON envelope. The gate is now one shared function. The operation is one
  undo step and composes in a batch. It is read back as `DocumentMeta.pluginMetadata`, which a v69
  worker always sends (possibly empty). It is persisted in the `.paged` native model part. The IDML
  adapter does not write it yet. Its IDML home would be a `Properties/Label` on the designmap's
  `<Document>`, which is a change in the adapter repository.
- **Delete undo restores paragraphs.** `deleteRange` snapshots the paragraphs it merges. Its
  inverse puts them back: runs, field identity, paragraph attributes, tables and anchors. The
  inverse still carries the recovered text, so offsets, selection shifts and edit spans read it
  as before.
- **Container parts stay outside undo.** Making part writes undoable would put part bytes into
  the undo log and couple the parts overlay to the mutation channel. The supported pattern is the
  one the web plugin uses:
  - write each version to a content-addressed part, never overwritten;
  - name the live version in an undoable label;
  - collect unreachable parts at save.

  `setDocumentMetadata` extends that pattern to state that has no frame.

## Evidence

- `crates/paged-canvas/tests/field_boundary_typing.rs`:
  - typing at a field's start, at its end, and after a field that ends the story survives a
    refresh;
  - a field is placed at the caret, including inside a batch, with non-ASCII text and across
    paragraphs;
  - undo of a delete restores a field and each run's size.

  The edge-typing and delete-undo cases fail with their fix reverted.
- `crates/paged-canvas/tests/document_metadata.rs`: write, replace, delete, undo and redo; the
  gates; one undo step inside a batch; survival through a `.paged` save and reload.
- `crates/paged-canvas/tests/data_relower_one_batch.rs`: a first lower is one batch and one undo
  step. It contains a frame, text, a field, a table with cell content, the frame's label and the
  document label. An in-place re-lower keeps the frame and story, creates no second frame, and
  undoes to the previous content with its field intact. Both use only existing handles (C-15)
  and `deleteTable`.

## Consequences

- A host passes the caret straight to `insertField.contentOffset` on an engine with protocol 69
  or later. An older engine ignores the field, so the host checks the protocol version first.
- A data refresh can name the live session in a document label that undo and redo keep correct.
- A document label written to an `.idml` export is lost until the adapter writes it.
- The undo log of a delete now holds the merged paragraphs. Their size is bounded by what was
  deleted.

## Related

- [ADR 125](125-snapping-lives-in-the-engine.md) — the previous protocol batch
