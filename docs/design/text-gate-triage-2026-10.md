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
| `preflight` | The missing family "Phantom Display" is set by InDesign in its substitute, Myriad Pro. | Read from `corpus/fonts/local/` when a machine has copied it there (Myriad Pro ships with InDesign and is not redistributable). |
| `footnotes` | References and note numbers were not drawn; the note area overlapped the body. | ADR 136. |

## Open, with the package that owns them

| Fixture | Cause | Package |
|---|---|---|
| `annual-base`, `showcase-base` | The master folio is `<?ACE 18?>`. The engine prints the page's baked `Page@Name`, which these fixtures use as a descriptor (`annual-base · p003 · front`); InDesign recomputes the number from the sections and prints `3`. The baked name is preferred on purpose (`SectionWalk::next_label`): it carries numbering InDesign computed from rules the engine does not model, a book's continued numbering through `DocumentPreference@StartPageNumber` among them. | L-08 (page numbering), after the start page number is read. |
| `markers` | InDesign prints the generator's text variables (`Season`, the page count) as empty: it does not resolve the generator's spelling of them. | A-08 follow-up (generator spelling), asked of InDesign. |
| `navigation` | InDesign runs each heading into the paragraph after it: the generator's paragraph boundary between the two ranges is not one to InDesign. | A-08 follow-up. |
| `paste-into` | InDesign sets five lines of the nested text frame; the engine sets two. | Engine bug (nested text frames), Wave 3. |
| `composer`, `text-letterspacing` | Line breaks and hyphenation points differ; the characters that differ are those the two layouts carry onto, or past, the last line. | R-20 (letter and glyph spacing), R-23. |
| `text-in-shape` | The engine fits more words per line in a non-rectangular frame. | R-30 (wrap and shaped frames). |
| `justification` | Case 1.34's label sits at its frame edge; InDesign oversets one word more. | Label only; the justification cases agree. |
| `list-overrides`, `list-marker-styles` | A number in a larger character style is a line of its own in InDesign's PDF, which the gate drops as a bare marker, while the engine's is part of its line. | Gate: have the inspector mark generated characters so both sides drop them. |
| `text-advanced` | InDesign's PDF carries tab leaders as text; the engine's line records do not. | Gate, with the generated-character marking above. |
| `text-on-path` | The inspector writes no line records for type on a path. | Inspector. |
