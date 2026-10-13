# ADR 141 — A line may overrun its measure by a sliver; an unstated hyphenation zone is 36 pt

- **Status:** Accepted 2026-10-13.
- **Scope:** `paged-text` (`ComposeOptions::fit_tolerance`, the Knuth–Plass path of `layout_runs`),
  `paged-renderer` (`metrics::fit_tolerance_64`, set per paragraph; `compose_opts`'s default
  hyphenation zone). No model, wire or protocol change.

## Context

A ragged line in InDesign keeps a word the engine moved to the next line: the text gate showed
`preflight`'s line carrying a word InDesign kept and the engine broke off. The question for
InDesign was whether it squeezes word spaces in ragged text, and if not, what lets the word stay.

## Decision

Asked of InDesign 20.0.1 (`tools/indesign-export/fit-tolerance-probe.*`): a one-line paragraph,
"justified, spaces never shrink" in Open Sans, in a frame its natural width minus *d* wide, with
*d* bisected to where the last word leaves the line, at 6 to 48 pt.

- Ragged text does not squeeze word spaces. The allowance is the same at minimum word spacing
  80, 90 and 100, and with one, three or twelve spaces on the line.
- A line may still run past its measure before the composer breaks it: `size / 32 + 0.254 / size`
  pt in ragged text (left, centred), `0.0466 × size + 0.178 / size` pt in justified text. The two
  forms reproduce all fourteen measured points within 0.003 pt (0.397 pt ragged and 0.574 pt
  justified at 12 pt).

**The hyphenation zone a paragraph does not state is 36 pt.** InDesign sets `preflight`'s story
with "story" whole on its second line, in the 40 pt frame that oversets it and in a 400 pt frame
alike, and hyphenates nothing in the paragraph; without a hyphen that line ends 1.6 pt short of the
360 pt measure. The fixture states no `HyphenationZone`, and the engine took that as no zone, so it
hyphenated "sto-ry". InDesign's `[No paragraph style]` carries `HyphenationZone="36"` in all 60
census templates sampled; a paragraph that states none now takes that value (ragged text only, as
before), the way the hyphenation limits already took InDesign's factory values.

`ComposeOptions::fit_tolerance` widens the measure the Knuth–Plass breaker fits lines to by that
much; the renderer sets it from the paragraph's size and alignment. Alignment still uses the true
measure. The minimum-raggedness breaker for shaped and wrapped frames keeps its own measured
squeeze, which was fitted before this allowance was known and may be the same effect.

## Consequences

- `preflight` passes the text gate; across the corpus three fixtures measure closer to InDesign
  (`tab-breaks`, `hard-hyphen`, `layout`) and none fails its gate.
- `nested-styles` moves one line away from InDesign: the first line of a drop-cap paragraph now
  keeps "across", which InDesign carries over. InDesign's line would overrun by about 0.42 pt, past
  its 0.397 pt allowance at 12 pt; the engine's comes out a few hundredths shorter, so the engine's
  measure beside a drop cap is that much wider than InDesign's.
- `preflight`, now set in Inter as InDesign sets it, measures mean ΔE 0.077 (it measured 0.551 in
  the default face) and passes the text gate.
