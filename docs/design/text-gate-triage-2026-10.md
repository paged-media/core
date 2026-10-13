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
| `preflight` | The missing family "Phantom Display" is set by InDesign in its substitute, Myriad Pro; and a ragged line keeps a word that overruns its measure by up to 1/32 em, which the engine broke off. | Mapped to Myriad Pro from `corpus/fonts/local/` when a machine has copied it there (it ships with InDesign and is not redistributable); the allowance is ADR 141. Set in Inter since the engine takes InDesign's factory 36 pt hyphenation zone (ADR 141). Text-gated. |
| `footnotes` | References and note numbers were not drawn; the note area overlapped the body. | ADR 136. |
| `list-overrides` | A number in a larger character style is a line of its own in InDesign's PDF, which the gate dropped as a bare marker while the engine's stayed part of its line; a marker with no space after it (`1.o02`) was dropped from neither side. | The inspector marks what the engine generated (below); the gate drops a list marker from the engine's line exactly, and from InDesign's where a line on that page opens with a marker the engine printed there. Text-gated. |
| `text-advanced` | InDesign's PDF carries tab leaders as text; the engine's line records did not. A price on a line of its own (`10.`) was also dropped as a bare marker. | The record marks each leader on its tab with the characters drawn, and the gate prints them on the engine's side too: InDesign's leaders and the engine's agree dot for dot (17, 21 and 21). Text-gated. |
| `navigation`, `text-letterspacing` | The gate dropped a short word ending in a full stop from the start of InDesign's line as if it were a list number (`pears.`, `vow.`, `quack.`; the engine's `dog.`), so a different line break read as a missing word. | Markers are dropped only where the engine printed one. The characters agree; the breaks still differ (below for `composer`; InDesign runs `navigation`'s headings into the next paragraph, an A-08 follow-up). Text-gated. |
| `text-on-path` (records) | The inspector wrote no line records for type on a path. | One record per `<TextPath>`, holding the text that fits between its brackets. Pages 1, 2, 4, 5 and 6 agree; page 3 is open (below). |

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

## Open, with the package that owns them

| Fixture | Cause | Package |
|---|---|---|
| `annual-base`, `showcase-base` | The master folio is `<?ACE 18?>`. The engine prints the page's baked `Page@Name`, which these fixtures use as a descriptor (`annual-base · p003 · front`); InDesign recomputes the number from the sections and prints `3`. The baked name is preferred on purpose (`SectionWalk::next_label`): it carries numbering InDesign computed from rules the engine does not model, a book's continued numbering through `DocumentPreference@StartPageNumber` among them. | L-08 (page numbering), after the start page number is read. |
| `markers` | InDesign prints the generator's text variables (`Season`, the page count) as empty: it does not resolve the generator's spelling of them. | A-08 follow-up (generator spelling), asked of InDesign. |
| `paste-into` | InDesign's PDF holds five lines of the nested text frame, the engine's line records two. The container clips the frame, and a clip leaves text in the PDF's text layer, so whether the engine sets fewer lines or only records fewer is still to be checked. | To verify (nested text frames). |
| `composer` | Line breaks and hyphenation points differ; the characters that differ are those the two layouts carry onto, or past, the last line. | R-20 (letter and glyph spacing), R-23. |
| `text-in-shape` | The engine fits more words per line in a non-rectangular frame. | R-30 (wrap and shaped frames). |
| `justification` | Case 1.34's label sits at its frame edge; InDesign oversets one word more. | Label only; the justification cases agree. |
| `list-marker-styles` | The markers agree now. Case m15's label, `number: size 20, auto leading`, sits in a frame one line deep: InDesign sets `number: size 20, auto` and oversets `leading`; the engine breaks before `auto` (`number: size 20,` / `auto leading`), so `auto` is overset in the engine only. Both set the label at the same widths (`number: size 20, run bold` is 142.96 pt on either side), so the break choice is the composer's. | R-23 (composer). |
| `text-on-path` (page 3) | InDesign sets type on a path as one line and oversets it at a line break, here a hyphenation point: its PDF reads `This sen-`. The engine drops the glyphs whose origin passes `EndBracket` and draws `This sente`. | Type on a path (overset at a break). |
| `styles-cascade` (note, gated) | The engine numbers the paragraph in `ListDerived` (`2.`); InDesign prints no number, and sets the paragraph at the frame edge without the list's tab, as if the style were not based on `ListBase`. The sample writes that `BasedOn` as an attribute, where InDesign writes a `<Properties>` element, and the engine reads the attribute. The gate drops markers on both sides, so the fixture passes and the note reports it. | A-08 follow-up (generator spelling). |
