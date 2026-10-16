# ADR 164 — Hyperlinks deleted and edited, whatever made them

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-wire`: `Mutation::DeleteHyperlink`, `Mutation::EditHyperlink` (protocol 73).
  - `paged-mutate`: `Operation::DeleteHyperlink`, `Operation::EditHyperlink` and the inverse-only
    `Operation::RestoreHyperlink` (`apply::hyperlink_edit`).
  - `paged-script`: `paged.deleteHyperlink`, `paged.editHyperlink`.
  - plugin-publish `idml-export`: a hyperlink InDesign wrote is edited in place on save.

## Context

`InsertHyperlink` (v53, v70) makes a link, and its inverse removes it. That inverse carries the span
and the URL the insert made, so it reaches only links made in the same session. A link read from an
InDesign document has no op that knows its span. There was no way to delete one, rename it, or send
it somewhere else.

A link is three records:
- the runs its source tags (`HyperlinkTextSource`);
- the `Hyperlink`, naming its source and its destination;
- the destination: a URL, a page, or a text anchor in a story.

A destination can be shared. Another hyperlink, a bookmark or a cross-reference may point at it.

On save, the writer replayed an unchanged InDesign hyperlink verbatim. It rewrote a changed one whole,
with its own defaults for the appearance settings: `Visible`, `Highlight`, `Width`, `BorderStyle`
and `BorderColor`. So a rename was lost, and a new destination reset how the link looked.

## Decision

**Address a link by its `Hyperlink` `Self` id alone.** Its ids come from the `hyperlinks`
collection. The op finds the rest itself:
- the runs its source tags, in whichever story they are;
- whether its destination is its own.

**`DeleteHyperlink { hyperlinkId }` removes the link.**
- Its text stops linking. The runs stay split where the link split them, so the glyph stream does
  not change.
- The `Hyperlink` record goes.
- Its destination goes when nothing else points at it and it is a URL or a page.
- A text destination is an anchor in its story's text and stays, as in InDesign.

**`EditHyperlink { hyperlinkId, name?, url?, page? }` changes the link.**
- `name` renames the link. An empty name clears it.
- `page`, or else `url`, sends the link somewhere new. `page` wins, as in `InsertHyperlink`.
- If the link's own destination is of the same kind, it changes in place and keeps its id.
- Otherwise the link moves to a new destination, which the model mints. This covers a shared
  destination, a text anchor, and a destination of the other kind.
- The destination the link leaves goes by the delete rule.
- Fields left out are unchanged. A page the document lacks is refused.

**Both ops undo in one step through `RestoreHyperlink`.**
- It puts back a captured state as JSON:
  - the link record at its index;
  - every destination the op touched, at its index or absent;
  - the source tag over the same runs.
- Destinations come back in ascending index order, so their order is rebuilt exactly.
- Its own inverse is the state it replaced, so redo holds.

**A save edits an InDesign hyperlink in place** (plugin-publish).
- This applies when the source names its destination as a typed `Properties` child, which is
  InDesign's spelling.
- `Name`, `Source` and `DestinationUniqueKey` are brought to the model, and so is the `Destination`
  child's text.
- Every appearance setting is kept.
- An unedited link still replays byte for byte. The engine's older attribute spelling is still
  normalised by a full rewrite.

## Result

- `paged-mutate` tests:
  - a delete takes only the link's own destination, and leaves a shared one and a text anchor;
  - an edit rewrites in place or moves off a shared destination;
  - undo of each restores the exact prior state.
- `hyperlink_ops` (canvas) tests:
  - delete over the wire, with undo and redo;
  - edit to a new URL and then to a page, each undone;
  - an unknown link and an unknown page are refused;
  - a package saved, reopened, edited and deleted, then saved again, reads back with the rename,
    the new URL and only the kept link's destination.
- plugin-publish `an_edited_hyperlink_keeps_its_appearance`: a renamed, repointed InDesign link keeps
  its outline, width, style and colour, and a second save is byte-identical.
- The render-effect sweep records both ops as inert. A link is a region beside the display list, so
  where it goes is not drawn.
- The script surface reaches 133 of 134 wire ops.

## Not yet

- **A destination's `Hidden` flag.** InDesign keeps a shared (non-hidden) URL destination in its
  destination list after the last link to it goes. The model does not carry `Hidden`, so such a
  destination goes with its last link.
- **Page-item sources** (`HyperlinkPageItemSource`) are not modelled, so a link from a frame cannot be
  addressed yet.
- **Bookmarks.** Bookmarks form a tree in IDML, and the model holds a flat list with no parent, so
  bookmark editing waits for that model.
