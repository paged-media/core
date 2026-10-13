# ADR 142 — Ragged text breaks by minimum raggedness, and a line keeps its last word by a share of it

- **Status:** Accepted 2026-10-14.
- **Scope:** `paged-renderer` (`build_engine`: ragged paragraphs set `minimum_raggedness`,
  `visible_lines` and `overrun_size`; the drop cap's size; `text_frame::visible_rows`), `paged-text`
  (`ragged.rs`: the overrun allowance, the hyphen weight, no break at the glue that closes a
  paragraph; `ComposeOptions::overrun_size`). No model, wire or protocol change. Amends ADR 141 for
  ragged text.

## Context

The engine broke ragged text two ways. In a shaped frame, or beside a wrap, it used the
minimum-raggedness breaker that `shaped-bands` and `text-in-shape` were measured against: the
breaks that make the sum of the squared space each line leaves smallest, the last line and the
lines past the frame costing nothing (all but the first of them). Everywhere else it used
Knuth–Plass, which prices a ragged line by how far its word spaces would stretch and so prefers
balanced lines.

InDesign's own lines in plain rectangular frames do not look balanced: `text-letterspacing`'s
control paragraph sets "The quick brown fox / jumps over the lazy dog. / …" where Knuth–Plass set
"The quick brown / fox jumps over …"; twenty-six short words fill 18 + 8; `preflight` keeps
"story" whole where Knuth–Plass hyphenated it.

ADR 141 gave every line one allowance past its measure, measured on a single line whose last word
was "shrink", and found that ragged text does not squeeze its word spaces. Both were incomplete.

## Decision

**Every ragged paragraph breaks by minimum raggedness** (left, centred and right aligned; justified
text and its last-line rules are untouched). The lines a paragraph may still set in its frame chain
are counted from its first baseline down each frame's text area (`visible_rows`), so the lines past
the chain cost nothing but the first, as measured on the shaped frames. A chain whose last frame
grows has no such limit.

**A line keeps its last word while it overruns its measure by a share of that word**
(`tools/indesign-export/ragged-squeeze-probe.*`: Open Sans at 8, 12 and 18 pt, five last words from
"m" to a 27-letter word, after one, three and seven words; the frame is bisected to where the last
word leaves the line). The allowance is

    word / 99 + size / 320 × ln(1 + before / word)

where `word` is the last word's width and `before` the width of the line ahead of it. It reproduces
all 45 measured lines within 0.12 pt (rms 0.04 pt); ADR 141's single allowance missed them by up
to 1.7 pt. A long last word carries a long allowance: a 152 pt word keeps its line 1.54 pt past the
measure, an "m" only 0.26 pt.

**InDesign squeezes the overrun out of the word spaces**, evenly, and past `MinimumWordSpacing`: a
0.3 pt overrun over three spaces takes 0.1 pt from each, and a 1.4 pt overrun over one 3.1 pt space
takes all of it from that space; the line ends on the measure. The minimum-raggedness breaker keeps
such a line at no cost, and alignment squeezes its spaces onto the measure. Shaped and wrapped
frames keep their own measured squeeze (`SQUEEZE_WEIGHT`), which was fitted before this rule was
known.

The breaker no longer breaks at the glue that closes a paragraph: that break leaves an empty last
line, which costs nothing either and tied with a kept overrun.

With every ragged paragraph on this breaker, two more fixtures narrow the hyphen weight inside the
range it was fitted to (4 to 24): the `tints` labels hyphenate "Ma-genta" and "Yel-low" in their
one-line frames as InDesign does up to 8 (not at 10), and `text-letterspacing` keeps "Trebek" whole
as InDesign does from 6 (not at 4). The weight is 7.

Justified text keeps ADR 141's allowance: its breaker (Knuth–Plass) widens the measure once per
paragraph and cannot price each line's last word.

## Consequences

- Across the 73 generator fixtures, 11 measure closer to InDesign and none further (summed mean
  ΔE 19.44 → 17.90): `text-letterspacing` 0.667 → 0.037, `composer` 0.688 → 0.445, `soft-hyphens`
  0.436 → 0.218, `paste-into` 0.499 → 0.339, `nested-styles` 0.232 → 0.121, `object-style-frames`
  0.143 → 0.066, `hard-hyphen`, `text-advanced`, `text-autosize`, `list-marker-styles`, `text`.
  Their thresholds are tightened to the new measurements.
- Line for line, 46 of the 73 fixtures now set every line as InDesign does (41 before). The five
  this brought to agreement are text-gated in `"lines"` mode, which fails a different break as
  well as a different character.
- `preflight` is gated in Inter, the face InDesign set it in: the overset story keeps "story" whole
  on its 360 pt measure. Mean ΔE 0.551 → 0.077 with the substitute face for its missing family,
  0.305 without it.
- `hard-hyphen`'s 50 pt "A state- / of-the-art / x-ray" now matches InDesign; one frame of the six
  rows still differs (a justified line the Paragraph Composer refuses).
- `nested-styles` first measured further from InDesign: its drop-cap paragraph kept "across" on
  the first line. The cause was the drop cap, not the breaker. The engine sized the cap from the
  paragraph's auto leading (13.2 pt at 11 pt) where the paragraph states 14 pt, and from a 0.7
  cap-height heuristic where the face carries its own (Inter 0.7275), because the lookup only
  consulted per-family overrides. InDesign sets the cap at 30.24 pt and starts the body 19.17 pt
  in; the engine set 29.86 pt and 18.92 pt, a measure 0.25 pt wider, so "across" fitted. The cap
  now spans the paragraph's leading, stated or auto, at the face's cap height (the rule measured
  in 2026-09 on paragraphs whose leading was auto), and the body starts within 0.004 pt of
  InDesign's (`drop_cap_pipeline`). `text-advanced` page 1, the other drop cap, goes from 0.448 to
  0.078.
- Open: the allowance is fitted within 0.12 pt, so a line whose last word overruns by about the
  allowance can still break the other way; justified text keeps one allowance per paragraph; and
  hyphenation points follow TeX patterns where InDesign uses its own dictionaries (`composer`).
