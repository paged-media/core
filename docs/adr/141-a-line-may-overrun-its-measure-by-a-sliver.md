# ADR 141 — A line may overrun its measure by a sliver; the hyphenation zone does not widen the gaps

- **Status:** Accepted 2026-10-13.
- **Scope:** `paged-text` (`ComposeOptions::fit_tolerance`, the Knuth–Plass path of `layout_runs`,
  the zone in `layout_runs` and `compose_paragraph`), `paged-renderer` (`metrics::fit_tolerance_64`,
  set per paragraph). No model, wire or protocol change.

## Context

The text gate showed lines where InDesign keeps a word that the engine breaks off. Two questions
went to InDesign 20.0.1: does it squeeze word spaces in ragged text, and what does the hyphenation
zone do under the Paragraph Composer.

## Decision

**A line may overrun its measure by a sliver** (`tools/indesign-export/fit-tolerance-probe.*`: a
one-line paragraph, "justified, spaces never shrink" in Open Sans, in a frame its natural width
minus *d* wide, with *d* bisected to where the last word leaves the line, at 6 to 48 pt).

- Ragged text does not squeeze word spaces. The allowance is the same at minimum word spacing
  80, 90 and 100, and with one, three or twelve spaces on the line.
- A line keeps its last word while it overruns the measure by at most `size / 32 + 0.254 / size`
  pt in ragged text (left, centred) and `0.0466 × size + 0.178 / size` pt in justified text. The
  two forms reproduce all fourteen measured points within 0.003 pt (0.397 pt ragged and 0.574 pt
  justified at 12 pt).

`ComposeOptions::fit_tolerance` widens the measure the Knuth–Plass breaker fits lines to by that
much; the renderer sets it from the paragraph's size and alignment. Alignment still uses the true
measure. The minimum-raggedness breaker for shaped and wrapped frames keeps its own measured
squeeze, which was fitted before this allowance was known and may be the same effect.

**The hyphenation zone does not widen the gaps.** With a zone set, the engine widened every word
space's stretch to the zone, so the breaker preferred short, balanced ragged lines. InDesign does
not: twenty-six short words in Inter 10 pt on a 360 pt measure set 18 + 8 at zone 0, 36 and 100,
where the engine set 14 + 12 at 36. Every InDesign document states its zone (36 on the root style
of all 60 census templates sampled), so its ragged paragraphs were composed the engine's way. The
widening is gone; the zone still keeps hyphenation out of a word that starts inside it, as it did.

A hyphen the word already has is a break whatever the zone: breaking there hyphenates nothing
(`hard-hyphen`'s `two-` / `way`).

What the zone is not, as asked of InDesign: a paragraph that states no zone has none (the
`tints` labels hyphenate "Ma-genta" as at zone 0, not as at 36); and `preflight`'s "story", which
the engine hyphenates, stays whole at every zone under both composers, so that difference lies in
how the breaker weighs a hyphen.

## Consequences

- `preflight` passes the text gate in the default face; across the corpus three fixtures measure
  closer to InDesign (`tab-breaks`, `hard-hyphen`, `layout`) and none fails its gate. Fixtures
  state no zone, so the zone change reaches only documents that do, which is every InDesign one.
- `hard-hyphen`'s 80 pt `pages 10-20 and / so-on` now matches InDesign.
- `nested-styles` moves one line away from InDesign: the first line of a drop-cap paragraph now
  keeps "across", which InDesign carries over. InDesign's line would overrun by about 0.42 pt, past
  its 0.397 pt allowance at 12 pt; the engine's comes out a few hundredths shorter, so the engine's
  measure beside a drop cap is that much wider than InDesign's.
