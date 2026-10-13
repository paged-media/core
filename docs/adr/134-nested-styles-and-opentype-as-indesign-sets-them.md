# ADR 134 — Nested styles and OpenType features as InDesign sets them

- **Status:** Accepted 2026-10-12.
- **Scope:** `paged-model` (`NestedDelimiter`, `LineStyle`, `GrepStyle`, the style-list fields of
  `ParagraphStyleDef` and `Paragraph`, `ParagraphStyleDef::otf`, `OtfFeatures` `historical` and
  `stylistic_alternate`, `CharacterRun::nested_character_style`), `paged-scene` (the run and
  paragraph cascades), `paged-renderer` (`pipeline/nested_styles.rs`, `metrics.rs`), `paged-text`
  (`ShapingFeatures`, `OtPosition`). The IDML spelling is plugin-publish's (`idml-import`
  `style_lists`, `idml-export` `nested_styles`). All additions are serde-default; no protocol bump.

## Context

Nested styles rendered, but only from the engine's own spelling. The importer read a
`<NestedStyle AppliedCharacterStyle=… Delimiter=…/>` child of a `<ParagraphStyle>`; the generator
and the exporter wrote it. InDesign does not read that element, and does not write it: a nested
style in an InDesign document never reached the renderer, and a nested style the engine wrote
never reached InDesign.

The OpenType side had the same shape. A paragraph style's `Ligatures`, `KerningMethod` and `OTF*`
attributes were not read at all; `Position="OTSuperscript"` (and its three siblings) was drawn as
a scaled, raised glyph; historical forms, stylistic alternates and the case-sensitive forms of All
Caps were never asked of the font.

InDesign 20.0.1 was asked. `tools/indesign-export/nested-styles-probe.jsx` builds a document in
the DOM, reads every character's effective formatting back (InDesign resolves nested styles into
it), and exports the document; its answers are in `nested-styles-probe.json`. The `nested-styles`
and `opentype` fixtures are the pixel oracles, and the `opentype` reference's glyph ids say which
features InDesign applied.

## Decision

**Spelling.** A nested style is a record of InDesign's `<Properties><AllNestedStyles type="list">`
(`AppliedCharacterStyle`, `Delimiter` as an `enumeration` or a `string`, `Repetition`,
`Inclusive`); line and GREP styles are `<AllLineStyles>` (`LineCount`, `RepeatLast`) and
`<AllGREPStyles>` (`GrepExpression`). The lists sit on a `<ParagraphStyle>`, on a
`<ParagraphStyleRange>` (the range's own list replaces its style's) and on `<TextDefault>`. The
older element still reads, and the exporter rewrites it into the list.

**Cascade.** `EmptyNestedStyles="true"` (and its line and GREP twins) marks a style whose list is
empty on purpose: a style that cleared its parent's list writes no list and `true`, one that
inherits writes no list and `false`. The nearest level that gives a list, or says it has none,
settles it.

**Layering.** A nested style sits UNDER a character style applied by hand and over the paragraph
style: a hand-applied underline-only style inside a nested 20 pt red range prints 20 pt red and
underlined. The renderer splits the runs at the ranges and records the nested style on each piece
as `CharacterRun::nested_character_style`, which the run cascade merges after the run's own
character style. The field is the renderer's; it is never read from or written to IDML.

**Ranges.** The rules are in `pipeline/nested_styles.rs`, each pinned by a test:

- "Up to N" stops before the Nth delimiter. For `AnyCharacter` the Nth character is the delimiter,
  so up to 3 characters styles two.
- A word ends at white space other than a no-break space, and through takes the whole run of it.
- A sentence ends at `.`, `!` or `?` and any closing quote, followed by white space. `3.14` ends
  nothing. Up to keeps the full stop.
- A literal delimiter of several characters ends at any one of them.
- `Repeat` loops the entries before it.
- A delimiter that never comes runs to the end of the paragraph.

**Drop caps.** A drop cap's character style is the FIRST nested style, with the `Dropcap`
delimiter, and covers the cap; the next entry starts after it. Without such an entry the first
style counts from the paragraph's first character. A paragraph style's `DropCapCharacters` /
`DropCapLines` now reach the renderer as the range's do.

**OpenType.** `OTSuperscript`, `OTSubscript`, `OTNumerator` and `OTDenominator` are the font's
`sups`, `subs` (not `sinf`), `numr` and `dnom` glyphs at the run's size on its baseline. In a face
without the feature the plain glyph prints, neither scaled nor raised. `Capitalization="AllCaps"`
switches on `case` and `cpsp`; capitals typed as such get neither. `OTFHistorical` is `hist`,
`OTFStylisticAlternate` is `salt`. A paragraph style carries `Ligatures`, `KerningMethod` and every
`OTF*` toggle, and they reach each run whose own and character-style values leave them unset.

**Not yet applied.** Line and GREP styles are read, kept and written, but not rendered. The DOM
answered the order for when they are: a line style beats a nested style and a GREP style where
they set the same attribute, and a nested style beats a GREP style.

## Consequences

- InDesign documents' nested styles, drop-cap styles, paragraph-style typography and OpenType
  positions render. On the two oracles, measured at 144 dpi:

  | Fixture | Mean ΔE before | Mean ΔE after | p99 before | p99 after | SSIM before | SSIM after |
  |---|---|---|---|---|---|---|
  | `nested-styles` | 0.566 | 0.197 | 24.6 | 4.9 | 0.9854 | 0.9951 |
  | `opentype` | 0.744 | 0.133 | 21.1 | 4.2 | 0.9689 | 0.9982 |
- A run that starts right after a drop cap took its paint one character late, because the body is
  laid out without the cap and the paint and stroke pickers counted from the paragraph's first
  byte. Both pickers now skip the cap's bytes.
- `<?ACE 3?>` (End Nested Style Here) is not kept by the story reader, so a nested style that ends
  there runs on to its other limits.
- An inline graphic ends a range only at the placeholder the anchored-object pass inserts. Indent
  to Here (`<?ACE 7?>`) is not kept.
- `cswh` has no IDML attribute and no corpus face to ask with. InDesign's `OTFSwash` is `swsh`.
