# ADR 165 — Update Table of Contents, as InDesign writes it

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-wire`: `Mutation::UpdateTableOfContents` (`v0.71.13`, on protocol 71).
  - `paged-mutate`: `Operation::ReplaceStoryParagraphs` (`apply::story`).
  - `paged-model`: `TOCStyleEntryDef.page_number_position`, `.page_number_style`,
    `.separator_style`, `.sort_alphabet`, and `TOCStyleDef.create_bookmarks` (serde default).
  - `paged-scene`: `resolve_toc_on` reads an entry as InDesign writes it.
  - `paged-renderer`: `pipeline::toc_paragraphs`. The render-time rebuild of an empty TOC story is
    gone.
  - `paged-canvas`: the op is lowered over the current layout.
  - `paged-script`: `paged.updateTableOfContents`.
  - `paged-gen`: the `toc` sample.
  - `tools/indesign-export`: `toc-probe`, `toc-update-probe`.
  - plugin-publish:
    - `idml-import` reads a TOC style entry in InDesign's spelling;
    - `idml-export` drops a source range whose run the model no longer has.

## Context

InDesign prints the text a TOC story holds until the user chooses Update Table of Contents
(ADR 120, docs audit 2026-10). The engine already printed stored entries. Three gaps remained:

1. **An empty TOC story was rebuilt at every render.** InDesign prints nothing there.
2. **Nothing could update a TOC.** No op wrote one.
3. **The importer read a spelling InDesign does not write.** It expected `IncludeStyle`,
   `FormatStyle` and `PageNumber` as attributes. So a TOC style from an InDesign document collected
   no headings at all.

## What InDesign does (asked of InDesign 20.0.1)

`toc-probe` builds a document in InDesign, with headings at three levels on three pages and a TOC
style with a title and three entry rules. It then lets InDesign generate the table of contents.
`toc-update-probe` runs Update Table of Contents on the `toc` sample, whose stored TOC is stale.
Its record is `tools/indesign-export/toc-update-probe.json`.

**The style, in `Resources/Styles.xml`:**
- An entry's `Name` is the name of the paragraph style it collects. There is no `IncludeStyle`.
- Its format, page-number and separator styles are typed `<Properties>` children.
  `$ID/[Same Style]` means no style of its own.
- Where the number stands is `PageNumberPosition`: `AfterEntry`, `BeforeEntry` or `None`.
- The separator carries a `$ID/` prefix (`$ID/^t`).
- The style also says `CreateBookmarks="true"`, InDesign's default.

**The story it writes:**
- It keeps `AppliedTOCStyle` on `<Story>`.
- The title comes first, in `TitleStyle`, when the style has a title.
- Then one paragraph per heading, in document order, in the entry's format style:
  - `AfterEntry`: the text, the separator, then the number;
  - `BeforeEntry`: the number, the separator, then the text;
  - `None`: the text alone.
- The number is set in `PageNumberStyle`, the separator in `SeparatorStyle`. The text has no
  character style.
- The last paragraph has no return.
- On update, the stale entries are replaced in place, in the same story.

**What else it writes:** each entry, from its text through its number, becomes the source of a
hidden hyperlink to a hidden page destination on the heading's page. With `CreateBookmarks` it also
writes a bookmark tree that mirrors the entry levels.

## Decision

**The engine never writes TOC entries at render.**
- A TOC story prints the text it holds, and an empty one prints nothing.
- The entries come only from `updateTableOfContents`.

**`UpdateTableOfContents { storyId }` rewrites the story from its TOC style.**
- The style is the one the story names (`Story@AppliedTOCStyle`), or else its frame.
- It lowers to `ReplaceStoryParagraphs`, whose inverse carries the paragraphs it replaced. So undo
  puts the stale entries back exactly, and redo replays the same text.
- A story that names no TOC style, or a style the document lacks, is refused.

**The entries come from the current layout.**
- They are built with `toc_paragraphs` over `resolve_toc_on` and the last build's paragraph pages,
  so a heading in a threaded story takes the page it stands on (L-06).
- Page labels come from the pages' names, so section numbering carries through.
- The op lowers in the canvas's `apply_mutation`, not in the translating pass. A batch that edits
  a heading and then updates first settles the build it owes, so the update lists the edit.
- A table of contents does not list its own entries.

**The resolver reads both spellings.**
- An entry collects `IncludeStyle`, or else the paragraph style its `Name` names (`:` written
  `%3a`, as in a style group's `Self`).
- `PageNumberPosition` wins over the older `PageNumber`.
- `$ID/` is stripped from the separator, and `[Same Style]` is no character style.
- Separator metacharacters expand: `^t` and `^y` to a tab; `^m`, `^>`, `^<`, `^|`, `^s` and `^f`
  to the spaces they name.

**plugin-publish reads InDesign's spelling** of every field above. Its older attributes still read
and win where present.

**plugin-publish saves a rewritten story's text only.** A source `<CharacterStyleRange>` past its
paragraph's last model run has text the story no longer holds. The writer now drops it and keeps
only its paragraph mark. A range holding foreign markup, such as an anchored frame, still replays.
Before this, the stale page number `8` outlived the update in the saved file.

## Result

- `toc_ops` (canvas) tests:
  - the stale TOC prints its stored entries;
  - the update writes exactly what InDesign wrote: the title, `Getting Started⇥1` with `1` in
    `TOC Page`, `1 First Steps`, `Going Further⇥2`, `Aside`, `3 Last Words`;
  - undo and redo hold;
  - a batched heading edit is listed;
  - the update saves as IDML and reads back the same;
  - a story that is not a TOC is refused.
- Renderer tests: an empty TOC story prints nothing, and updated entries print each heading with its
  page. Resolver test: InDesign's entry spelling.
- plugin-publish tests: `parses_indesigns_toc_style_spelling` (with the legacy test kept as
  `legacy_parses_toc_style_with_entries`) and `rewritten_paragraphs`.
- The `toc` fixture is in the fidelity gate against InDesign's PDF, which shows the stale entries.
- The render-effect sweep records the op as painting.
- The script surface reaches 134 of 135 wire ops.

## Not yet

- **The hidden hyperlinks and bookmarks** InDesign writes with the entries. The model has no
  `Hidden` flag on links or destinations, and bookmarks have no tree (ADR 164).
- **Further TOC style options:**
  - `RunIn` (entries run into one paragraph);
  - `SortAlphabet`;
  - `IncludeHidden` (text on hidden layers);
  - `NumberedParagraphs` (whether a list number joins the entry);
  - `IncludeBookDocuments`.
- **Inserting a new TOC** (`insertTableOfContents`) and TOC style editing.
- **Cross-references** (L-03) take the same route next; the index followed in ADR 168.
