# ADR 153 — A paragraph's line setting: auto leading, a lone word, the last line, balance, no break

- **Status:** Accepted 2026-10-14.
- **Scope:**
  - `paged-model`: `LineSetting` on paragraphs and paragraph styles; `no_break` on runs and
    character styles.
  - `paged-scene`: the cascade of both.
  - `paged-text`: `LayoutOptions::{auto_leading, single_word_alignment, no_break}`,
    `ComposeOptions::{last_line_indent, balance_ragged}`, and `ragged::RaggedLines::balance_lines`.
  - `paged-renderer`: `set_auto_leading` and `apply_no_break` (`pipeline/compose_opts.rs`), for
    body text and footnotes; table cells take the auto leading.
  - `paged-gen`: the `paragraph-attrs` sample.
  - plugin-publish `idml-import`: reads `AutoLeading`, `SingleWordJustification`,
    `LastLineIndent`, `<BalanceRaggedLines>` and `NoBreak`.
  - No wire or protocol change.

## Context

Five paragraph attributes that change where lines break and how they sit were not read:
- **Auto leading** was always 1.2 × the line's largest size.
- **A justified line that holds one word** sat flush left.
- **A last-line indent** was ignored.
- **Balance ragged lines** did nothing.
- **No-break text** broke like any other.

## The question to InDesign

`paragraph-attrs` (a `paged-gen` sample) is a grid of 126 pt frames, one case each, in Open Sans
9 pt with hyphenation off. InDesign 20.0.1 exported it, and the PDF was read case by case.

## Decision

**Auto leading.**
- A line's auto leading is its largest size × `AutoLeading` / 100; the default is 120.
- A 14 pt word on a 175 % paragraph leads its line 24.5 pt, and the next line returns to
  9 × 1.75.

**Single-word justification.**
- On a justified paragraph, a line other than the last that holds one word, with no space to
  stretch, follows `SingleWordJustification`.
- Absent, InDesign letter-spaces it to the measure (`FullyJustified`, its default; 572 of 582
  corpus values).
- `LeftAlign`, `CenterAlign` and `RightAlign` place it at natural width.

**Last-line indent.**
- `LastLineIndent` narrows the measure of the paragraph's last line, and the paragraph breaks
  against that. At 36 pt, "go" moves to a line of its own.
- It is an invisible box of that width before the finishing glue, as in TeX, so every breaker
  honours it alike. It is set only on the text that ends the paragraph: a forced line break ends
  a line, not the paragraph.

**Balance ragged lines.**
- `BalanceRaggedLines` is a typed `<Properties>` child, an enumeration: `NoBalancing`,
  `FullyBalanced`, `PyramidShape`, `VeeShape`. InDesign ignores `BalanceRaggedLines="true"`.
- `FullyBalanced` keeps the paragraph's line count and evens the lines, the last included.
- The minimum-raggedness breaker (ADR 142) runs again with the count fixed and the last line's
  slack priced, which reproduces all three balanced cases.

**No break.**
- No line breaks inside a `NoBreak` span, at a space or at a hyphenation point. An infinite
  penalty before the span's glue does it.

## Consequences

- `paragraph-attrs`: mean ΔE 0.51 → 0.177, ink 14.97 → 4.98.
- **Case C3** (justified, a 36 pt last-line indent) still differs. The justified breaker spreads
  the looseness over two lines; InDesign leaves it on one. This is the composer residual ADR 144
  records.
- **A no-break span wider than its line:** InDesign sets nothing from it on and oversets the rest
  of the story. The engine sets it squeezed or overfull. The layout has no way to declare a
  paragraph's tail overset.
- **Not yet modelled:**
  - `PyramidShape` and `VeeShape` balance; `NoBreak` on a paragraph style.

## Editable and saved (2026-10-14)

- **Saved.** The IDML export writes all five back in InDesign's spellings (plugin-publish 788cff5):
  - the three attributes with a range's or a paragraph style's other attributes;
  - `NoBreak` with a run's or a character style's;
  - `<BalanceRaggedLines>` as the `<Properties>` child.
- **Editable.** Five property paths set them, at paragraph and paragraph-style level, and
  `characterNoBreak` at run and character-style level:

  | Path | Wire | Notes |
  |---|---|---|
  | `paragraphAutoLeading` | `Length`, per cent | 0–500, default 120 |
  | `paragraphSingleWordJustification` | `Text`, IDML enum | `""` clears |
  | `paragraphLastLineIndent` | `Length`, pt | |
  | `paragraphBalanceRaggedLines` | `Text`, IDML enum | `""` clears |
  | `characterNoBreak` | `Bool` | |

  - An unknown enum string is refused and the slot is left untouched.
  - Each path has its catalog row with the IDML mapping. Settable paths: 234 → 239.
  - `tests/line_setting_paths.rs` writes each attribute onto the fixture's bare twin over the wire
    and gets the set twin's lines.
