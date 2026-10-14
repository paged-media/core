# ADR 145 — A page's number comes from its sections, not from `Page@Name`

- **Status:** Accepted 2026-10-14.
- **Scope:** `paged-renderer` (`build_engine`: page labels from `SectionWalk::derived_label`), which
  feeds folios (`<?ACE 18?>`), page-number variables and the editor's page labels. No model, wire or
  protocol change.

## Context

Every `<Page>` carries a `Name`. The engine printed it as the page's label and computed a label
from the document's `<Section>` rules only when it was missing, on the grounds that the name holds
numbering InDesign computed from rules the engine might not model. Generated fixtures use the name
as a descriptor (`annual-base · p003 · front`), and InDesign 20.0.1's exports of them print `3`:
InDesign recomputes the label on open and ignores the stored name. A last-page-number variable
(`markers`' page count) likewise printed the last page's name where InDesign prints the section's
last number.

## Decision

A page's label is what the section rules give it: the section's start number or the previous
page's number plus one, in the section's numbering style, with its prefix where the section says
to include it. `Page@Name` is not read for it. InDesign writes the label it computed into
`Page@Name` itself, so its own documents read the same either way; the census supports the rules
the engine models (the first section continues numbering in 249 of 271 InDesign documents; a
custom start in 8, a non-Arabic style in 2).

## Consequences

- `annual-base`'s 120 pages and `markers` pass the character text gate (both now gated);
  `showcase-base`'s folios match.
- Open: numbering styles the engine does not format (Kanji, full-width, Arabic-script and the
  zero-padded ones, plan L-08), and book-continued numbering, which a standalone IDML does not
  carry.
