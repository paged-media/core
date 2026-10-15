# ADR 156 — How InDesign numbers footnotes: per story, restarted by page, spread or section

- **Status:** Accepted 2026-10-15.
- **Scope:**
  - `paged-model`: `FootnoteOptions::{prefix, suffix, show_prefix_suffix, restart_numbering}`,
    `FootnoteRestart`, `reference_label`, `text_label`.
  - `paged-renderer`: the story emitter's restart scope (`footnote_scope`, `footnote_ordinal_at`),
    `PageNumbering::spread_of`.
  - `paged-gen`: `FootnoteOptionDef`'s numbering fields; the `footnote-numbering` and
    `footnote-restart` samples.
  - plugin-publish `idml-import`: `Prefix`, `Suffix`, and the `RestartNumbering`,
    `ShowPrefixSuffix` and `RuleColor` children of `<FootnoteOption>`.
  - No wire or protocol change.

## Context

The engine numbered a story's notes from `StartAt` in their story's order. It knew nothing of
`RestartNumbering`, `Prefix`, `Suffix` or `ShowPrefixSuffix`. InDesign writes the last two
enumerations, and the rule colour, as typed `<Properties>` children of `<FootnoteOption>`, where
the importer did not look.

The campaign plan (L-01g) assumed InDesign numbers notes continuously through the whole document,
and that the engine was wrong to start each story from one. InDesign was asked before acting on
that.

## The question to InDesign

`footnote-numbering` and `footnote-restart` are two `paged-gen` samples with the same two A4 pages,
each with one document-wide `<FootnoteOption>`:
- Story A is threaded from page 1's top frame to page 2's; its third paragraph starts the next
  frame.
- Story B stands below it on page 1, and story C below it on page 2.
- Every paragraph carries one note.
- The designmap lists the stories C, B, A, the reverse of where they stand.

The options:
- `footnote-numbering`: `DontRestart`, with `Prefix` "[" and `Suffix` "]" in both places
  (`PrefixSuffixBoth`).
- `footnote-restart`: `PageRestart`, `LowerRoman`, with "(" and ")" in the reference only
  (`PrefixSuffixReference`).

InDesign 20.0.1 exported both, and the numbers were read with `pdftotext`.

## Decision

**Numbering runs through each story, not the document.**
- With `DontRestart`, story A's notes are [1] [2] on page 1 and [3] [4] on page 2.
- B's and C's are [1] [2] each, though B shares page 1 with A.
- The engine's per-story counter was right, and L-01g's premise was wrong.

**A restart starts a story's numbering again in each new scope.**
- With `PageRestart`, A's page-2 notes are i, ii again.
- The scope is the page the reference lands on, its spread for `SpreadRestart`, or its section for
  `SectionRestart`.
- The emitter remembers the scope of the last note it numbered and starts the story's counter
  again when a note lands in another.

**The prefix and suffix stand where `ShowPrefixSuffix` says.**
- `PrefixSuffixReference`: the reference reads "(i)" and the note "i".
- `PrefixSuffixText`: the note only. `PrefixSuffixBoth`: both.
- `NoPrefixSuffix` (the default) shows neither.

**A reference is labelled before its line is placed.**
- Its width can move a break, so the label is composed with the paragraph. When the paragraph's
  start rule sends it to another frame (`StartParagraph`), it is labelled for that frame's page.

## Result

| fixture | before (main) | after | gate |
|---|---|---|---|
| `footnote-numbering` | 0.062 / 0.078 | 0.035 / 0.051 | 0.061 |
| `footnote-restart` | — | 0.051 / 0.058 | 0.07 |

- The "after" figures are each page's mean ΔE.
- Every note's number and every reference matches InDesign's text: the text gate is on for both
  fixtures.
- `tests/footnote_numbering.rs` pins each note's page, story and both labels to InDesign's.

## Not yet

- A reference on a line that overflows into a new restart scope after its paragraph was composed
  keeps the label it was composed with.
- `MarkerPositioning`, `FootnoteMarkerStyle` and `FootnoteTextStyle` are not modelled. InDesign
  applies them to a note when it is made (`footnote-references`), so they concern the coming
  insert-footnote operation, not rendering.
- `EosPlacement`, `NoSplitting`, `EnableStraddling`, the continuing rule and the area's first
  baseline are read by neither side yet; the corpus sets none of them away from the default except
  `EnableStraddling`.
