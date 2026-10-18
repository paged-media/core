# ADR 171 — PDF bookmarks, as InDesign exports them

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-export-pdf`: `ExportOptions.bookmarks` (`PdfBookmark`), written as `/Outlines`.
  - `paged-model`: `DesignMap.bookmark_parents` (serde default).
  - `paged-canvas`: `ExportPdfWireOptions.bookmarks` (additive) and `export::pdf_bookmarks`.
  - plugin-publish `idml-import`: bookmark nesting.
  - `tools/indesign-export/pdf-outlines.py`.

## Context

The PDF export wrote no bookmarks, though documents carry them. A document's `<Bookmark>`s were read
as a flat list, so their nesting was lost. (Tagged PDF is a separate decision.)

## What InDesign does (asked of InDesign 20.0.1)

InDesign updated the `toc` sample's table of contents, which writes a bookmark per entry, and
exported it with Include Bookmarks. `pdf-outlines.py` reads the PDF's outline.

- Each bookmark is an outline item titled with the bookmark's `Name`, in document order.
- A nested `<Bookmark>` is a child of its parent's item. The TOC's level-3 entry under a level-1
  heading sits one level down, directly under the heading.
- Items are open: a parent's `/Count` is its number of descendants.
- A page destination goes to its page fitted in the window, through a named destination
  `[page /Fit]`.
- In the designmap, a child `<Bookmark>` nests inside its parent's element.

InDesign's print presets leave Include Bookmarks off.

## Decision

**`ExportOptions.bookmarks` lists the bookmarks to write.** Each has a title, a depth, and the page
it goes to.
- An item nests under the nearest earlier item one level up, and its destination is
  `[page /Fit]`, written as a direct destination.
- A bookmark whose page is not exported (a page range) is left out, together with the bookmarks
  under it.
- An empty list writes no outline.

**The canvas fills the list when the export asks** (`bookmarks: true`; off by default). A bookmark's
page is resolved as follows:
- a page destination: its page;
- a text or paragraph destination: the page its paragraph was laid out on (ADR 169), else its
  story's first page.

A bookmark that resolves to no page is left out. Its depth is the length of its chain through
`bookmark_parents`.

**plugin-publish records each nested bookmark's parent.**

## Result

- `paged-export-pdf` tests: `bookmarks_export_as_a_nested_outline` (titles, nesting, `/Fit`, the
  root `/Count`), `no_bookmarks_write_no_outline` and `a_page_range_keeps_its_own_bookmarks`.
- Canvas tests: `export_bookmarks` (with the flag on, `navigation`'s two bookmarks are the outline;
  with it off, there is none).
- plugin-publish: `nested_bookmarks_record_their_parent`.

## Not yet

- **A text destination's position** on its page (`/XYZ`). Text destinations go to their page fitted.
- **The bookmarks InDesign writes with an updated TOC** (ADR 165).
- **Bookmark editing ops.**
- **Tagged PDF.**
