# ADR 182 — Create Tagged PDF: stories and paragraphs as InDesign tags them

- **Status:** Accepted 2026-10-18.
- **Scope:**
  - `paged-compose`: `StructKey`, `GlyphRunTable.current`, `GlyphRunEntry.structure`.
  - `paged-renderer`: a body line's glyphs (and its drop cap) carry their paragraph's key.
  - `paged-export-pdf`: `ExportOptions.tagged` / `lang`, marked content in the page walk, and the
    structure tree.
  - `paged-canvas`: `ExportPdfWireOptions.tagged` (additive) and the document language.
  - `tools/indesign-export/tagged-pdf-probe`, `pdf-structure.py`.

## Context

The PDF export wrote no logical structure. Screen readers and reflow had no reading order, and a
PDF meant for accessible distribution could not be produced. InDesign's Create Tagged PDF is one
checkbox.

## What InDesign does (asked of InDesign 20.0.1)

`tagged-pdf-probe` opens a document and exports it with Create Tagged PDF on. `pdf-structure.py`
reads the result. For the `text` and `navigation` fixtures InDesign writes:

- **Catalog:** `/MarkInfo <</Marked true>>`, `/Lang` and `/ViewerPreferences`.
- **Tree:**
  - a `Document` holding one `Article` (role-mapped to `Art`);
  - in it, one `Story` (`Sect`) per text story, in document order;
  - in each story, one element per paragraph, named after its paragraph style and role-mapped to
    `P`.
- **Marked content:** each paragraph element references its text's marked content by MCID, page by
  page.

## Decision

- **Renderer.** While it emits a body line's glyphs, the renderer sets the glyph capture's current
  `StructKey`: the story's ordinal in document order, and the paragraph's index. Every captured
  glyph carries it.
- **Exporter, with `tagged` on:**
  - Each text object whose glyphs carry a key is wrapped in `/P <</MCID n>> BDC … EMC`, one MCID
    per text object.
  - It writes `Document` → `Article` → one `Story` per story → one element per paragraph, in key
    order. Each paragraph element is named after its paragraph style as InDesign names it (every
    character but an ASCII letter or digit becomes `_`, so `$ID/[No paragraph style]` is
    `_No_paragraph_style_`), and the role map sends `Article` to `Art`, `Story` to `Sect`, and each
    style name to its PDF export tag when that is a heading (`H`, `H1`…`H6`), else to `P`
    (amended 2026-10-18, `tagged-styles-probe`: InDesign mapped a style tagged `Artifact` to `P`). A
    paragraph on several pages references its later pages' marks through `/MCR` with `/Pg`.
  - The parent tree maps each page's MCIDs to their paragraphs, and every page gets
    `/StructParents`.
  - The catalog gets `/MarkInfo <</Marked true>>`, `/Lang`, and `/DisplayDocTitle`.
- **Untagged exports are unchanged.**
- **Language.** `/Lang` comes from `[No paragraph style]`'s `AppliedLanguage`, mapped to BCP 47. A
  document with none writes no `/Lang`; InDesign then writes its application's language, which an
  IDML does not carry.

## Amendment 2026-10-18: tables and paragraphs within one range

`tagged-tables-probe` asks InDesign 20.0.1 how it tags a table: the paragraph that holds the table
holds `Table` > one `TR` per row > `TH` for a header row's cells, `TD` for a body row's > one `P`
per cell paragraph holding the cell's text; the standard types need no role map. The engine now
writes the same tree: a cell paragraph's glyphs carry their cell (`StructKey::cell`: row, column,
paragraph, header), and the writer nests them under their paragraph. A model paragraph that holds
several of InDesign's paragraphs (its text split at paragraph marks) gives each its own element
(`StructKey::sub`), as InDesign does. Nested tables are left untagged; cell paragraphs are written
as `P`, where InDesign names them after their style.

## Consequences

- `tagged_pdf`:
  - the structure is present and untagged exports carry none;
  - language names map to BCP 47;
  - an ignored test writes the `navigation` export. `pdf-structure.py` reads it as the same tree
    as InDesign's (six elements, one marked-content id per paragraph).
- Not built yet:
  - reading a paragraph style's PDF export tag from IDML (`<StyleExportTagMap ExportType="PDF"
    ExportTag="H1">`, the importer's half). The model carries it (`pdf_export_tag`) and the role map
    already honours it: `H`, `H1`…`H6` map to themselves, anything else to `P`, as InDesign wrote a
    style tagged `Artifact`;
  - text inside transparency groups and soft masks (written into forms, left unmarked);
  - footnotes, figures with alt text, and artifacts for page furniture;
  - articles, XML tags and export tags as the reading order;
  - PDF/UA.
