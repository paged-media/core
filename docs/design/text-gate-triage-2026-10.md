# Text-gate triage, October 2026

The text gate (`corpus/generated/text-diff.sh`) compares, page by page, the characters InDesign's
reference PDF carries with the lines the engine lays out. Run advisory over every fixture that is not
yet text-gated, it reported 18 mismatches. This note records the cause of each and where it is
fixed.

## Fixed in the gate

| Fixture | Cause | Fix |
|---|---|---|
| `images`, `effects` | The sample grew after its reference was exported (14 pages against 5, 22 against 12); the extra pages were compared against nothing. | The gate compares only the pages the reference has, as the pixel gate does. |
| `links-broken`, `links-ok`, `text-autosize`, `text-overset` | Rendered in the fallback serif: the gate did not register Inter, the face InDesign set them in. The fallback face fit a third line in `text-overset`'s frame where Inter carries it over. | `<stem>.fonts.sh` registers Inter; thresholds tightened, text-gated. |
| `annual-base` (fonts) | Five families InDesign used were drawn in the fallback serif. | `annual-base.fonts.sh`; thresholds tightened. |
| `preflight` | The missing family "Phantom Display" is set by InDesign in its substitute, Myriad Pro; and a ragged line keeps a word that overruns its measure by up to 1/32 em, which the engine broke off. | Mapped to Myriad Pro from `corpus/fonts/local/` when a machine has copied it there (it ships with InDesign and is not redistributable); the allowance is ADR 141. Text-gated in the default face. |
| `footnotes` | References and note numbers were not drawn; the note area overlapped the body. | ADR 136. |
| `list-overrides` | A number in a larger character style is a line of its own in InDesign's PDF, which the gate dropped as a bare marker while the engine's stayed part of its line; a marker with no space after it (`1.o02`) was dropped from neither side. | The inspector marks what the engine generated (below); the gate drops a list marker from the engine's line exactly, and from InDesign's where a line on that page opens with a marker the engine printed there. Text-gated. |
| `text-advanced` | InDesign's PDF carries tab leaders as text; the engine's line records did not. A price on a line of its own (`10.`) was also dropped as a bare marker. | The record marks each leader on its tab with the characters drawn, and the gate prints them on the engine's side too: InDesign's leaders and the engine's agree dot for dot (17, 21 and 21). Text-gated. |
| `navigation`, `text-letterspacing` | The gate dropped a short word ending in a full stop from the start of InDesign's line as if it were a list number (`pears.`, `vow.`, `quack.`; the engine's `dog.`), so a different line break read as a missing word. | Markers are dropped only where the engine printed one. The characters agree; the breaks still differ (below for `composer`). Text-gated. |
| `text-on-path` (records) | The inspector wrote no line records for type on a path. | One record per `<TextPath>`, holding the text that fits between its brackets. Pages 1, 2, 4, 5 and 6 agree; page 3 is open (below). |
| `preflight` (Inter) | Set in Inter, the engine hyphenated "sto-ry" on the overset story's second line where InDesign's Paragraph Composer keeps "story" whole on the 360 pt measure. | Ragged text breaks by minimum raggedness, and a line keeps its last word by the allowance InDesign gives it (ADR 142); Inter is registered in `preflight.fonts.sh`. |
| `paste-into` | InDesign's PDF holds five lines of the nested text frame where the engine's line records held two. | The engine set those lines; they were broken differently (ADR 142). Every line matches: text gate `"lines"`. |
| `list-marker-styles` | Case m15's label `number: size 20, auto leading` broke before `auto` where InDesign oversets `leading`. | ADR 142. Every line matches: text gate `"lines"`. |
| `annual-base`, `showcase-base` (folios), `markers` (page count) | The engine printed a page's `Page@Name`, which these fixtures use as a descriptor (`annual-base · p003 · front`); InDesign ignores it and computes the number from the sections: the folio prints `3`, and `markers`' page count (a last-page-number variable) the section's last number. | A page's label is the section rules' (`SectionWalk::derived_label`). InDesign writes the computed label into `Page@Name` itself, so its own documents read the same either way. `annual-base` and `markers` text-gated. |
| `navigation` | The generator ended no paragraph with `<Br/>`: InDesign ran the headings into the body paragraph. | `navigation.rs` writes `<Br/>` as InDesign does; reference re-exported. |
| `markers` (spelling) | The generator wrote the custom variable's text as `<TextVariablePreference Contents>` and the page count as `PageCountType`, neither of which InDesign reads: both printed empty. | Asked of InDesign 20.0.1 (a document built in its DOM and exported): `<CustomTextVariablePreference>` with a typed `Contents`, and `LastPageNumberType` with a `<PageNumberVariablePreference Scope="SectionScope">`. Reference re-exported. |
| `styles-cascade` | `ListDerived`'s `BasedOn` (and the derived cell and table styles') was an attribute, which InDesign ignores: it printed the derived list paragraph unnumbered. | Written as InDesign writes it, `<Properties><BasedOn type="object">`; reference re-exported, and InDesign numbers the paragraph `2.` as the engine does. |

### What the line records mark

`paged-inspect --emit-breaks` writes one JSON record per laid-out line. Two fields were added
without changing the others, and each is left out where it does not apply, so a record written
before them reads the same:

- `generated`: `[{"kind", "start", "end", "text"?}]`, byte ranges of `source_text` that the engine
  printed and the story does not hold. `kind` is `list_marker` (the bullet or number with the text
  after it), `tab_leader` (the tab a leader fills; `text` is the leader's repeats as drawn),
  `page_number`, `section_marker`, `footnote_number` (the reference in the body, or the number
  opening a note), `text_variable` or `anchored_object` (an inline object's placeholder).
- `text_path`: on the record of type on a path, the `<TextPath>`'s `Self`. That record is the
  path's one line: its bytes index the story's text, `baseline_y_pt` is page-local, where the text
  starts on the path, and `width_pt` the arc length the set glyphs cover.

The gate drops `list_marker` spans, prints `tab_leader` text in place of its tab, and keeps the
rest, which InDesign prints too. A marker the engine printed that no line of InDesign's opens with
is reported as a note, not failed: the marker is dropped on both sides, so the characters agree.

### Line by line

The gate's `"lines"` mode fails a different break as well as a different character. Run over every fixture, 46 of 73 match InDesign line for line (41 before ADR 142, none worse). The five that ADR 142 brought to agreement are gated that way: `list-marker-styles`, `object-style-frames`, `paste-into`, `text-autosize`, `text-letterspacing`.

## Open, with the package that owns them

| Fixture | Cause | Package |
|---|---|---|
| `showcase-base` (page 11) | InDesign prints the master's caption `Paged Showcase Report` where the engine prints the `Part Two · Plates and Notes` heading: the heading's frame lands on a different page. | To verify (page routing). |
| `composer` | Line breaks and hyphenation points differ; the characters that differ are those the two layouts carry onto, or past, the last line. Since ragged text breaks by minimum raggedness (ADR 142) the pixel gate measures it at mean ΔE 0.445 (was 0.688); the remaining breaks are justified paragraphs and hyphenation points from TeX patterns, where InDesign uses its own dictionaries. | R-20 (letter and glyph spacing), hyphenation dictionary. |
| `text-in-shape` | The engine fits more words per line in a non-rectangular frame. | R-30 (wrap and shaped frames). |
| `justification` | Case 1.34's label sits at its frame edge; InDesign oversets one word more. | Label only; the justification cases agree. |
| `text-on-path` (page 3) | InDesign sets type on a path as one line and oversets it at a line break, here a hyphenation point: its PDF reads `This sen-`. The engine drops the glyphs whose origin passes `EndBracket` and draws `This sente`. | Type on a path (overset at a break). |
