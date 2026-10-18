# ADR 168 — Generate Index, as InDesign writes it

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-wire`: `Mutation::GenerateIndex` (protocol 72).
  - `paged-model`: `DesignMap.index_topic_parents`, `.index_cross_references` and `.index_options`
    (`IndexCrossReference`, `IndexOptions`, serde default).
  - `paged-scene`: `Document::resolve_index_tree_on` (`IndexTreeEntry`, `IndexCrossRef`).
  - `paged-renderer`: `pipeline::index_paragraphs`, which replaces `build_index_paragraphs`.
  - `paged-canvas`: the op is lowered over the current layout, and missing index styles are
    created.
  - `paged-script`: `paged.generateIndex`.
  - `paged-gen`: the `index` sample.
  - `tools/indesign-export/index-probe`.
  - plugin-publish `idml-import`: topics, references and options read in InDesign's spelling.

## Context

The model carried an index's topics and the page references in the text. It could not generate an
index. `build_index_paragraphs` wrote one `topic⇥pages` paragraph per topic, a shape InDesign never
writes. Nothing called it outside tests.

The importer also missed InDesign's own spelling. A `<PageReference>` names its topic with
`ReferencedTopic`, and the parser read only `TopicName` and `AppliedTopic`. So an InDesign document's
page references were all dropped. It also flattened subtopics, never read See references, and never
read `<IndexOptions>`.

## What InDesign does (asked of InDesign 20.0.1)

`index-probe` asks InDesign twice. First it builds a document in InDesign's DOM and generates its
index. Then it opens the `index` sample and generates that one. Both give the same index. The probe's
record is `tools/indesign-export/index-probe.json`. An opened document must be recomposed before
Generate Index, or InDesign finds no page for any reference and lists none.

**The IDML:**
- `<Index>` in `designmap.xml` holds the `<Topic>`s, before the story includes. A subtopic's
  `<Topic>` nests inside its parent's.
- A See reference is a `<CrossReference ReferencedTopic CrossReferenceType CustomTypeString>` inside
  its topic.
- A `<PageReference PageReferenceType="CurrentPage" ReferencedTopic Id>` stands in the text, in a
  `<CharacterStyleRange>` of its own.
- `<IndexOptions>` lives in `Preferences.xml`.

**The default options:**
- Title `Index`, with section headings on and empty sections off.
- Nested format.
- Separators: two spaces after the topic, `, ` between page numbers, `. ` before a cross-reference,
  `^=` for a range, and nothing at the entry's end.
- Styles: `$ID/Index Title`, `$ID/Index Section Head`, `$ID/Index Level 1` and `2`, and the character
  style `$ID/Index Cross-reference`.

**The index it writes:**
- The title in Index Title.
- A section head per initial letter, in Index Section Head. Letters with no topic are left out.
- Each topic in its level's style: the topic, then two spaces, then its pages joined by `, `.
  - A page appears once, however many references stand on it.
  - Separate references on consecutive pages stay separate (`2, 3`). A range comes only from a
    range reference.
  - A subtopic follows its topic at level 2.
- A topic with only a See reference: `Fruit. ` + `See` in Index Cross-reference + a no-break space +
  `Apples`. The word is written in the UI's language.
- Topics in alphabetical order. The last paragraph has no return.

**The styles it creates when the document lacks them,** all based on [Basic Paragraph]:

| Style | Attributes |
|---|---|
| Index Title | 30 pt, 18 pt after, centred |
| Index Section Head | 7.2 pt before and after |
| Index Level 1 | 10 pt, indented 28 pt, first line −28 pt |
| Index Level 2 | 10 pt, indented 28 pt, first line −21 pt |
| Index Cross-reference (character style) | Minion Pro Italic 10 pt |

## Decision

**`GenerateIndex { storyId }` writes the index into a story.** It replaces the story's text, as
Replace Existing Index does. InDesign writes no marker on an index story, so the caller names it. The
op is one batch:
- it creates the index styles the paragraphs name that the document lacks, with InDesign's
  attributes;
- then it replaces the story's paragraphs (`ReplaceStoryParagraphs`).

Undo takes both back off.

**The index comes from the current layout,** like Update Table of Contents (ADR 165). A build that a
batch still owes is settled first.

**`resolve_index_tree_on` lists the index.**
- A topic is listed when it, or one of its subtopics, has a page reference or a cross-reference.
- Siblings are sorted by `SortOrder`, else the name, case-insensitive.
- A reference finds its topic by id. A reference in the engine's older spelling (`TopicName` alone)
  makes a level 1 topic of that name.

**`index_paragraphs` writes the measured shape** from `IndexOptions`, with InDesign's defaults where
the document has none. The reference words are InDesign's English: `See`, `See also`, `See herein`,
`See also herein`, `See [also]`, or a custom reference's own words.

**plugin-publish reads InDesign's spelling:**
- `ReferencedTopic`;
- topic nesting, into `index_topic_parents`;
- topic cross-references, into `index_cross_references`;
- `IndexOptions`.

## Result

- `index_ops` (canvas) tests:
  - the stale index prints as stored;
  - Generate Index writes exactly InDesign's twelve paragraphs, with its German names mapped back,
    and creates its styles with their attributes;
  - undo removes the styles too, and redo holds;
  - the index saves as IDML, styles included;
  - an unknown story is refused.
- The navigation and pipeline tests now expect InDesign's shape.
- plugin-publish tests: `reads_the_index_as_indesign_writes_it` and
  `a_page_reference_names_its_topic_as_indesign_writes_it`.
- The `index` fixture is in the fidelity and text gates against InDesign's PDF.
- The script surface reaches 135 of 136 wire ops.

## Not yet

- **Run-in format** (`RunInFormat`), **page ranges** (range references and `PageRangeSeparator`),
  and **levels 3 and 4**, whose created styles' attributes were not measured.
- **Empty sections** (`IncludeEmptyIndexSections`), and the heading for topics that do not start with
  a letter (`Symbols`, not measured).
- **Topic, reference and option editing,** and an index marker op.
- **Book documents and hidden layers** (`IncludeBookDocuments`, `IncludeHiddenEntries`).
