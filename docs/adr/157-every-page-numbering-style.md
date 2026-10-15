# ADR 157 — Every page-numbering style InDesign offers, formatted as InDesign formats it

- **Status:** Accepted 2026-10-15.
- **Scope:**
  - `paged-model`: `NumberingStyle`'s eight new values, `as_idml`, and their formatting.
  - `paged-renderer`: chapter-number formats with leading zeros.
  - `tools/indesign-export/numbering-styles-probe.*`.
  - plugin-publish `idml-export`: writes a section's style through `NumberingStyle::as_idml`.
  - No wire change: the section ops already take IDML style names.

## Context

The engine knew five page-numbering styles: Arabic, upper and lower Roman, upper and lower
letters. Any other `PageNumberStyle` a section named fell back to Arabic. A footnote's numbering
style went through its own copy of the same five. Roman numerals stopped at 3999.

## The question to InDesign

`numbering-styles-probe.jsx` builds a document through InDesign's DOM:
- It reads the styles from the `PageNumberStyle` enumeration itself, so none is missed. InDesign
  20.0.1, Roman edition, offers thirteen.
- It gives each style one-page sections starting at chosen numbers, 191 numbers in all. Each
  page's name is the label InDesign prints.
- It saves the document as IDML, so each style's spelling can be read from the file.

The results are committed as `numbering-styles-probe.json`.

## Decision

Each style formats as InDesign labels its pages:

| Style (IDML) | Rule | Examples from InDesign |
|---|---|---|
| `UpperRoman` / `LowerRoman` | as many M as thousands up to 4999; above that, the thousands in Roman, then M | 4000 MMMM, 5001 VMI, 10000 XM |
| `UpperLetters` / `LowerLetters` | bijective base 26 | 199 GQ, 703 AAA |
| `Kanji` | one Kanji digit per decimal digit | 12 一二, 2026 二〇二六 |
| `ArabicAlifBaTah` / `ArabicAbjad` | the `(n−1) mod 28`-th letter, written `⌊(n−1)/28⌋+1` times and joined by U+200C | 29 أ‌أ |
| `HebrewBiblical` | Hebrew numerals: ת for each 400; 15 and 16 as טו and טז; thousands prefixed as their own numeral; no geresh | 999 תתקצט, 9999 טתתקצט |
| `HebrewNonStandard` | ת for each 22, then the remaining letter; above 395, א | 23 תא, 396 א |
| `SingleLeadingZeros` / `DoubleLeadingZeros` / `TripleLeadingZeros` | padded to 2, 3 or 4 digits | 12 → 12, 012, 0012 |

Notes on the table:
- The abjad order ends ض غ ظ.
- Hebrew Biblical writes רע and שמד as they come, with no reordering.

Other uses of the styles:
- A footnote's `FootnoteNumberingStyle` takes the same styles; `Asterisks` stays its own case.
- A chapter-number format spelled by example (`01, 02, 03...`) maps to the leading-zero styles.

## Result

`numbering_style_tests::every_style_formats_as_indesign_labels_it` replays all 191 numbers InDesign
labelled, in every style, and every one formats the same.

## Not yet

- **Other editions.** Styles only other editions offer (full-width Arabic, Hindi, Thai, Farsi)
  are not measured here; they still fall back to Arabic.
- **List numbers.** These go through `NumberingFormat` example strings (`numbering.rs`), which this
  probe did not ask about.
- **Rendered fixture.** No rendered sample covers the Hebrew and Arabic styles, because the
  corpus ships no face with those scripts. The DOM's labels are the oracle.
