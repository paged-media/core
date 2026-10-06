# ADR 129 — Pages and masters for presentations

- **Status:** Accepted 2026-10-06.
- **Scope:** `crates/paged-mutate/src/apply/{batch_page,master,duplicate_page,hyperlink,layer}.rs`,
  `crates/paged-canvas/src/{model,snapshot}.rs`, `crates/paged-scene` (`ParsedMasterSpread::name`),
  `crates/paged-model` (`gradient_fill_start`, cell edge stroke types), the renderer's fill and
  table modules, and the operations `movePage`, `setPageMetadata`, `onMaster`, `createMaster`,
  `deleteMaster`, `renameMaster`, `insertHyperlink.page` and `requestSnapshot.hideItems`
  (protocol 69)

## Context

The presentation plugin opens a PowerPoint deck as a native document: one single-page spread per
slide and one master spread per slide layout. Building its editing surfaces and slideshow on the
engine at protocol 68 found the gaps below.

1. **No reorder.** A slide sorter could only delete a page and insert it again, which loses the
   page's ids and everything keyed by them.
2. **No page-level plugin state.** A slide's notes, transition and hidden flag belong to the page.
   `setPluginMetadata` reached page items only, so that state had to live in container parts,
   outside undo, and did not follow a page through duplicate or delete.
3. **No link to a page.** `insertHyperlink` took a URL only. The model already had a page
   destination, and PowerPoint's slide jumps need it.
4. **No build steps.** A slide whose shapes appear one click at a time needs the slide rendered
   with some items left out, without editing the document.
5. **Masters could not be edited or made over the wire.** A layout's logo or footer lives on a
   master spread. No operation reached master items, and none created, renamed or removed a master.
6. **Imported decks render wrongly in three places.** A radial gradient ignored
   `GradientFillStart`, so a centred glow sat in a corner. A cell edge ignored its stroke type, so a
   double border drew as one solid line. An item stroked Thick - Thick also drew solid.

## Decision

- **`movePage`** moves a page's single-page spread to follow another page, or to the front. The
  spreads restack in their new order. One undo step restores the old order.
- **Page metadata is item metadata on a page.** `setPluginMetadata` accepts a page as its target
  (`setPageMetadata` on the wire), with the same gate: the `x-paged:` namespace, the caller
  namespace, the 64 KiB cap and the JSON envelope. The entries live in the spread's label map,
  keyed by the page id, like an item's. They therefore move, duplicate (under the copy's new ids)
  and undo with the page. In IDML they are a `Properties/Label` on the `<Page>`.
- **`insertHyperlink` takes an optional `page`.** With it the destination is that page. The
  operation refuses a page the document does not have.
- **`requestSnapshot.hideItems`** leaves the named items out of that snapshot only. It is a
  query: the document and its undo history do not change.
- **`onMaster`** wraps any page-item, style or property mutation, or a batch of them, and applies
  it to one master spread's items. Item lookup searches master spreads too. Page and spread
  operations are refused inside it. Text needs no wrapper, because a master frame's story is a
  story like any other.
- **`createMaster`** makes a master with one page of a given size, or copies another master with
  fresh ids for its pages and items. The caller chooses the id. The created master is captured, so
  redo recreates the same ids. **`deleteMaster`** is refused while any page or master applies the
  master; its undo restores the master exactly. **`renameMaster`** sets or clears the master's
  `Name`, which the importer now reads and the master-page list shows as its label.
- **Radial gradients are placed by their start.** Rectangles, text frames, ovals and polygons
  carry `gradient_fill_start`. A radial gradient with a start and a positive
  `GradientFillLength` is centred at the start, with the length as its radius. Without them it
  keeps InDesign's swatch default. Linear gradients are unchanged.
- **A cell edge draws in its stroke style.** Cells and cell styles carry the four
  `…EdgeStrokeType` attributes, inline over the cell style. Cell edges go through the same typed
  emitters as the table's rules. Thick - Thick is also a built-in striped stroke for items: two
  equal rules a third of the weight apart.

## Consequences

- The IDML adapter reads and writes page labels, `GradientFillStart`, cell edge stroke types and
  master names. It also saves masters that the model created, renamed or deleted: a created
  master is written in full and referenced from the designmap; a deleted one loses its part and
  its reference.
- Two-level masters (a slide master with layouts under it) are still not modelled. An importer
  flattens each layout onto its own master. Editing what would be the shared slide master means
  editing each flattened master.
- `hideItems` hides items, not paragraphs. A build that reveals one bullet at a time is rendered
  as its whole shape.
