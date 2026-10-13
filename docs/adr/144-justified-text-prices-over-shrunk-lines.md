# ADR 144 — Justified text prices an over-shrunk line instead of forbidding it

- **Status:** Accepted 2026-10-14.
- **Scope:** `paged-text` (`justified.rs`, called from `layout_runs` for justified text outside
  shaped and wrapped frames; alignment squeezes such a line's spaces onto the measure),
  `paged-gen` (`justified-breaks`, the gate fixture; `justified-probe`, the paragraphs the breaker
  was fitted on). No model, wire or protocol change. Supersedes ADR 141's allowance for justified
  text in plain frames.

## Context

Justified text broke by Knuth–Plass (`paragraph_breaker::total_fit`) at TeX's settings: a line
whose word spaces would have to shrink past `MinimumWordSpacing` was infeasible, and so was one
that stretched past the tolerance; ADR 141 widened every measure by a fixed allowance. InDesign
20.0.1 does neither. Its PDF of `justified-breaks` sets lines whose spaces are 73 to 78 % of their
width, below the 80 % minimum, and with hyphenation off it sets "Considerable experimentation" in
140 pt with its one space at 41 % rather than stretch "Considerable" across the measure.

The question went to InDesign through its DOM (`tools/indesign-export/justified-lines-probe.*`): ten
paragraphs of plain prose, `LeftJustified` in Open Sans 10/12, English: USA, in eight frame widths
from 120 to 260 pt, hyphenated and not: 160 paragraphs, 1276 lines. (A document a script creates
takes the application's language, which in a German installation hyphenates English words as
German; the probe sets English: USA. Fixture IDMLs that name no language open as English.)

## Decision

Justified text in a plain frame breaks by least total demerits over every legal break, as
Knuth–Plass does, with these departures, each found by fitting the probe:

- **A line may shrink past `MinimumWordSpacing`.** Below the minimum its badness grows a hundred
  times faster (`OVERSHRINK`), up to three times the stated shrink (`MAX_SHRINK`); alignment then
  squeezes the spaces onto the measure.
- **A line may stretch as far as it must.** There is no tolerance; badness, `100·r³`, prices a
  loose line, uncapped.
- **The measure is the frame's.** ADR 141's allowance is not added; it made the breaker worse on
  the probe (72 paragraphs against 80 without it, at TeX's other settings).
- The rest is TeX's: line penalty 20 (TeX 10), `(line penalty + badness)²` per line, the hyphen
  penalty squared, 30 000 for two hyphenated lines in a row, 10 000 for adjacent lines more than
  one fitness class apart.

99 of the 160 paragraphs now set every line as InDesign does, against 73 with Knuth–Plass at TeX's
settings; lines in position, 1026 of 1276 against 819. Of InDesign's 92 hyphenation points in the
probe, 86 are among the engine's (`hyph_en_US.dic`); for-ty, lon-ger, re-cords, tight-er, spac-es
and di-ox-ide are not. The badness exponent, the demerits power, shrink and stretch scaled
separately, and a price on the last line's hyphen were each tried and changed nothing.

Shaped and wrapped frames keep Knuth–Plass: there the rows a hole joins and the lines past the
frame are priced by the minimum-raggedness rules (ADR 142), which this breaker does not model.
`shaped-bands` measured worse under it (mean ΔE 0.336 → 0.592).

## Consequences

- `justified-breaks` (new, gate): 15 of its 24 frames match InDesign line for line (12 before);
  mean ΔE 2.464 → 1.645 on its hyphenated page.
- `composer` 0.445 → 0.376 and `hard-hyphen` 0.074 → 0.044; no other fixture moves.
- `justified_lines` (`paged-renderer` tests) holds the 99 as a ratchet.
- Open: the remaining 61 probe paragraphs differ by a word or a hyphenation point. Some of
  InDesign's choices lose to the engine's under every cost tried, which suggests its composer is
  not a whole-paragraph optimum: unhyphenated at 200 pt it sets "The harbour master kept a
  ledger of" with its spaces at stretch ratio +6.1 where "every" fits at +0.6, and pays for it
  on the next two lines (+2.1, +1.6 against +4.9, +4.0); at 180 pt it sets "Considerable
  experimentation" alone (+44.9) rather than add "showed" (−0.5). InDesign's own dictionary has
  points the vendored one lacks. Justified text in shaped frames keeps Knuth–Plass.
