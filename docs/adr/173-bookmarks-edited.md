# ADR 173 — Bookmarks created, renamed, moved and deleted as InDesign does it

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-mutate`: `CreateBookmark`, `RenameBookmark`, `MoveBookmark`, `DeleteBookmark`, and
    the inverse-only `RestoreBookmarks`.
  - `paged-wire` / `paged-canvas`: the four mutations; `BookmarkSummary.parent`.
  - `paged-script`: `paged.createBookmark`, `deleteBookmark`, `renameBookmark`, `moveBookmark`.
  - `tools/indesign-export/bookmarks-probe`.
  - plugin-publish `idml-export`: the bookmark tree written whole once it changes; a page
    destination made for a bookmark in InDesign's spelling.

## Context

ADR 171 reads a document's bookmarks, nested, and exports them as the PDF's outline. Nothing
could change them, so an editor's Bookmarks panel was read-only.

## What InDesign does (asked of InDesign 20.0.1)

`bookmarks-probe` builds a document in InDesign's DOM, edits its bookmarks, and reports the tree
and the document's destinations after each step. With `PROBE_IN`, it opens a document and
reports its tree.

**Creating a bookmark to a page** makes a page destination of its own.
- The bookmark is `<Bookmark Self Name Destination>`.
- The destination is a `<HyperlinkPageDestination>` named "Bookmark N" (localized: "Lesezeichen
  1"), with `ViewSetting="FitWindow"`, `Hidden="true"`, and `<ViewBounds>` of `1e+256` on every
  side.

**Creating a bookmark to text** goes to a text destination that already exists.

**A child bookmark** is a `<Bookmark>` inside its parent's.

**Renaming** changes `Name`. **Moving** a bookmark carries its children with it, to the top level
or under another bookmark, at a position among its siblings.

**Deleting a bookmark**:
- deletes the bookmarks under it;
- deletes the page destinations it made;
- keeps a text destination, which is an anchor in the text.

## Decision

**The tree** is `DesignMap::bookmarks` in document order (a parent before its children, each
subtree contiguous) plus `DesignMap::bookmark_parents`. Every op keeps that order. Each one
captures the bookmarks, their nesting and the destinations, so its inverse,
`RestoreBookmarks`, lands on the exact prior state.

**The ops:**
- `createBookmark` goes to page `page` through a page destination of its own (the canvas mints
  both ids), or to an existing destination `destination`. It takes a `parent` and an `index`
  among its siblings; absent, the top level and the end.
- `moveBookmark` refuses to move a bookmark under itself.
- `deleteBookmark` takes the bookmark's children with it. It also removes the page destinations
  that no remaining bookmark, hyperlink or cross-reference goes to.
- Bookmarks print nothing. A PDF export writes the tree as its outline when it runs (ADR 171).

**plugin-publish:**
- An unchanged tree replays byte for byte.
- A changed tree is written whole and nested from the model, with the new navigation, which
  comes after every destination. The first save wrote the tree where the source's first
  bookmark stood, before a new page destination, and InDesign dropped the bookmark that went to
  it: InDesign drops a bookmark whose destination it has not read yet.
- A page destination that only bookmarks go to is written as InDesign makes one for a bookmark.

## Result

- InDesign reads a saved, edited tree back as edited. `bookmark_ops`'s ignored
  `writes_the_saved_tree_for_indesign`, opened with `bookmarks-probe` and `PROBE_IN`, gives
  "Introduction" with "Page one" under it (through its page destination), then the renamed
  "Reading list".
- Tests:
  - `paged-canvas`: `bookmark_ops` (a page bookmark's own destination, nesting and moves, a
    rename, deleting a parent with its children, and the tree saved as IDML and as the PDF's
    outline).
  - plugin-publish: `bookmark_tree` (an unchanged tree byte-identical; renamed and moved;
    a new page bookmark after its destination in InDesign's spelling; deleted with its children).

## Not yet

- **Bookmarks to text or a page item made in one op.** A text bookmark needs its text
  destination to exist first, as in InDesign's DOM.
- **The page destination's name.** InDesign names it "Bookmark N" in the UI's language; the
  engine names it by its id.
