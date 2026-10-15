# ADR 162 — The document's footnote options are editable, and saved as InDesign writes them

- **Status:** Accepted 2026-10-15.
- **Scope:**
  - `paged-mutate` / `paged-introspect`: twelve `document*` property paths on `doc` (catalog
    rows with the `doc` scope; the element roster stays at 241).
  - `paged-canvas`: reads them back through `doc`.
  - plugin-publish `idml-export`: `patch_footnote_option`, which writes `<FootnoteOption>` in
    `Resources/Preferences.xml`.
  - No wire or protocol change: property paths are additive.

## Context

ADRs 156, 160 and 161 taught the engine how InDesign numbers, places and splits footnotes, from
`<FootnoteOption>`. Nothing could change those options. The IDML writer also never wrote the
element, so an option a host set some other way would have been lost on save.

## Decision

**Twelve paths on `doc` set the options a footnote layout depends on.**

| Path | IDML |
|---|---|
| `documentFootnoteNumberingStyle` | `<FootnoteNumberingStyle>` child: a page-number style name, or `Asterisks` |
| `documentFootnoteStartAt` | `StartAt`, a whole number from 1 |
| `documentFootnoteRestartNumbering` | `<RestartNumbering>` child: `DontRestart` / `PageRestart` / `SpreadRestart` / `SectionRestart` |
| `documentFootnotePrefix`, `documentFootnoteSuffix` | `Prefix`, `Suffix` |
| `documentFootnoteShowPrefixSuffix` | `<ShowPrefixSuffix>` child |
| `documentFootnoteEnableStraddling` | `EnableStraddling` (ADR 160) |
| `documentFootnoteNoSplitting` | `NoSplitting` (ADR 161) |
| `documentFootnoteSpaceBetween`, `documentFootnoteSpacer` | `SpaceBetween`, `Spacer` (pt, from 0) |
| `documentFootnoteRuleOn`, `documentFootnoteContinuingRuleOn` | `RuleOn`, `ContinuingRuleOn` |

**What InDesign cannot state is refused with the slot untouched.** That covers an unknown name, a
start below 1 and a negative space. Each write returns the prior value in the path's own form, so
undo restores it. A boolean written to InDesign's default is stored as unset. Every write re-lays
the whole document.

**The writer keeps the bytes while the part agrees with the model.**
- `patch_footnote_option` compares each value as InDesign reads it; an absent value is the
  default.
- A value that differs is rewritten in place: an attribute, or the typed enumeration child InDesign
  writes and reads (ADR 156).
- A missing value is added. An empty element opens for its children.
- A part with no element gains one only when the model departs from the defaults, and only if no
  other part spells the options. The generator's older designmap spelling stays as it was.

## Result

- `footnote_option_paths` writes a `PageRestart` and lower roman to `doc` on `footnote-numbering`:
  page 2's notes print `[i]` and `[ii]` instead of `[3]` and `[4]`, and undo gives them back. Four
  values InDesign cannot state are refused.
- plugin-publish's `footnote_option` tests check the writer on InDesign 20.0.1's own spelling:
  - an unchanged element keeps its bytes;
  - a changed one is rewritten in place and reads back;
  - a bare part gains an element only when it must;
  - an empty element opens for its children.

## Not yet

- The separator rule's weight, width, offset, indent and colour, and the continuing rule's, have no
  path.
- A frame's own footnote override (ADR 160) has no path.
- Footnote ops (`insertFootnote`, `deleteFootnote`, editing a note's text) need a protocol bump.
