# ADR 166 — A ruby wider than its base, and kana at their proportional widths

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-model`: `MarkSettings` gains `RubyParentSpacing`, `RubyParentOverhangAmount`,
    `RubyAutoScaling` and `RubyParentScalingPercent`; `OtfFeatures` gains
    `OTFProportionalMetrics`.
  - `paged-text`: `LayoutOptions::ruby_aki` (a `RubyAki` per base: the space before and after each
    base character mid-line, at a line's start, at its end and both) replaces the symmetric
    `glyph_aki`; each line takes the variant its base's place on it asks for.
    `ShapingFeatures::proportional_widths` (`palt`) and `ShapingFeatures::ruby()`.
  - `paged-renderer`: `pipeline::ruby` fits a wider ruby (`ParentSpacing`, `Overhang`, `fit_wide`);
    `metrics::shaping_features_from` sets `palt` under Metrics kerning.
  - plugin-publish `idml-import` / `idml-export`: reads and writes the five attributes.
  - No wire or protocol change.

## Context

A ruby wider than its base spaced the base out 1-2-1, always. InDesign does that only when it
has to: in its default it lets the ruby reach over the kana beside the base and leaves the base
solid. The base's settings for this (`RubyParentSpacing` and the overhang and scaling options)
were not modelled. Kana were set a full em apart, where InDesign's default Metrics kerning sets
them at their proportional widths.

## The questions to InDesign

`ruby-parent-spacing-probe` (InDesign 20.0.1, Kozuka Mincho 30 pt, 15 pt ruby, read from its
PDF with the new per-glyph reader `pdf-glyphs.py`) sets かんじかんじ (90 pt) over 漢字 (60 pt)
and variants, with kanji or kana beside the base, at a line's start and end, and each setting
changed in turn; `kana-spacing-probe` sets a line of kana, kanji and brackets under each kerning
method.

With E the room a ruby still needs past its base and n the base's characters:

| Setting | InDesign |
|---|---|
| `RubyParent121Aki` (default) | 2n shares: one before, two between each two, one after |
| `RubyParentBothSides` | E/2 before and after |
| `RubyParentEqualAki` | E/(n+1) before, between and after |
| `RubyParentFullJustify` | E/(n−1) between, none at the ends; one character: both sides |
| `RubyParentNoAdjustment` | base solid, ruby centred over it, overhanging anything |
| `RubyParentOverhangAmount` | how far over each neighbour: one ruby character (default), half, one base character, half, no limit, none |
| `RubyAutoScaling` | narrows the ruby, down to `RubyParentScalingPercent` (66 % default) |

- The ruby overhangs first: over kana, brackets and Latin, never a kanji, never past a line's start
  or end, and by at most half its excess on each side. Then it narrows, if allowed; what is left
  spaces the base.
- A base starting a line drops its share before, one ending a line its share after, and the room
  goes to the shares left (line start, 15 pt over two characters: 0, 10 between, 5 after).
- Ruby is set full width (no kerning, no proportional widths) in the face's ruby forms: the PDF
  draws ruby か as Adobe-Japan1 CID 12681, not CID 843.
- Metrics kerning sets Kozuka's kana where `palt` puts them; kerning pairs then move katakana
  further. With kerning off they stand a full em apart, and `OTFProportionalMetrics` alone sets
  them as Metrics does. 「」。 halve the same way.
- `RubyOverhang` (the flag) and `RubyAutoAlign` changed nothing measured.

## Decision

- **Fit a wider ruby the way InDesign does.** `fit_wide` computes, for each of the four places a
  base can take on a line, the overhang on each side, the ruby's scale and the space before and
  after each base character, in that order. `RubyAki` carries the four; the breaker measures the
  mid-line one and each line swaps in its own, then the ruby is drawn from the slot its base took.
- **Metrics kerning sets proportional widths** (`palt`, with `kern`); ruby is shaped with
  `ShapingFeatures::ruby()` (kerning off, `ruby` forms).

## Result

Paged against the probes' PDFs: every one of the 40 ruby cases within 0.02 pt but one (Latin
beside the base, 0.66 pt: InDesign's spacing between Japanese and Latin); the kana lines within
0.012 pt under Metrics and with kerning off.

## Not yet

- Optical kerning (InDesign's outline-driven pass) is set with the face's kerning pairs only.
- Kerning pairs across style runs (a kenten or colour change inside a pair) are not applied: runs
  are shaped one by one.
