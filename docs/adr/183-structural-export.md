# ADR 183 — The IDML a save writes holds the document's structure as InDesign reads it

- **Status:** Accepted 2026-10-18.
- **Scope:**
  - plugin-publish `idml-export`: spreads dropped with their pages, master transparency, link
    file names (plugin-publish #52).
  - `paged-canvas`: the blank document's `Preferences.xml` (`blank.rs`).
  - `tools/indesign-export/document-probe`.

## Context

A save rewrites the source package in place. Three structural things went wrong:
- A page removed in the editor left its `Spreads/` entry in the package and its `idPkg:Spread`
  reference in the designmap, so InDesign opened the page again.
- The transparency pass ran over spreads but not over master spreads, so an opacity or blend
  mode on a master item was saved as it was loaded.
- A blank document (one minted with no source package) wrote no `DocumentPreference`.

## What InDesign does (asked of InDesign 20.0.1)

`document-probe` opens a package and reports its page size, facing pages, page count, masters and
pages:
- Without `DocumentPreference`, InDesign reads a package as Letter, facing pages.
- With the element written by the blank document, it reads the size the document was minted at
  (400 × 300 pt in the test) and no facing pages.

## Decision

- A source `Spreads/` entry that no spread of the model accounts for is not written, and its
  reference leaves the designmap.
- The transparency rewrite runs over master spreads as it does over spreads.
- The blank document writes `<DocumentPreference>` in `Preferences.xml` with its page width and
  height, `PagesPerDocument="1"`, `FacingPages="false"` and `Intent="PrintIntent"`.
- A link whose URI is a relative `file:` URI (`file:photo.png`) names its file without the scheme
  when a save collects links.

## Consequences

- `removed_page_spread_drops_out_of_the_export` (`idml_export_saveback.rs`),
  `master_transparency_roundtrip` (plugin-publish) and `blank_idml_states_its_document_setup`
  pin the three behaviours.
- Masters minted without a source entry and fonts declared in `Fonts.xml` (the rest of A-33)
  remain open.
