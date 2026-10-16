# ADR 167 — Japanese lines, and vertical text as horizontal text turned

- **Status:** Accepted 2026-10-16.
- **Scope:**
  - `paged-text`: `shape_run_vertical_at` and `upright_in_vertical` (characters that stand upright
    shaped top to bottom: `vert`, vertical metrics, `vpal`, `vkrn`); `Upright` on `ShapedGlyph`
    and `PositionedGlyph`; `LayoutOptions::vertical`; break opportunities between CJK characters
    in `layout_runs` (and in `compose_paragraph`) whatever the hyphenation.
  - `paged-compose`: an upright glyph is drawn turned back a quarter (`glyph_transform`).
  - `paged-renderer`: a vertical story is laid out in `turned_for_vertical` frames and turned by
    `apply_vertical_writing_rotation`; upright glyphs are drawn from their column's centre line;
    ruby and custom kenten shaped top to bottom, drawn marks turned back; kinsoku only for a
    kinsoku set that names a table; `japanese_leading_drop_64`; vertical justification to the em
    box; no kenten on punctuation.
  - plugin-publish `idml-import`: reads `<KinsokuSet>` as InDesign writes it (a typed child).
  - No wire or protocol change.

## Context

A vertical story was laid out as horizontal text in its frame — a line as long as the frame is
wide — and every command turned a quarter, so kanji lay on their side and punctuation kept its
horizontal forms. Working through that turned up three things wrong with horizontal Japanese
too: the renderer's line composer never broke between Japanese characters (a paragraph with no
spaces ran on past its frame, or broke only where hyphenation found a Latin word), kinsoku was
enforced wherever `KinsokuType` was written (InDesign writes it always), and emphasis marks were
set on punctuation.

## The questions to InDesign

InDesign 20.0.1's scripting has no story orientation, so `vertical-text-probe` has InDesign build
the frames horizontally, turns each story's `StoryOrientation` to `Vertical` in the IDML, and has
InDesign open that, report its lines and export the PDF (Kozuka Mincho 20 pt in a 200 × 300 pt
frame: leading, insets, columns, first-baseline settings, alignment, vertical justification,
kerning, ruby, kenten, and horizontal controls). Read with `pdf-glyphs.py`:

- Vertical text is the frame's horizontal layout turned a quarter clockwise: lines become columns
  from the right, each the frame's height long, a leading apart; the first column's em box stands
  where the first line's would against the frame's top (Ascent and Em Box to 0.01 pt); insets
  stay on their sides (the right one before the first column, the top one above each column);
  vertical justification centres the columns across the frame.
- Kanji, kana and CJK punctuation stand upright in their vertical forms (the PDF draws them through
  `Identity-V`); Latin and digits turn with the column, their baseline 0.12 em in from the em
  box's left edge. Under Metrics kerning the upright ones take `vpal` and `vkrn`.
- Ruby stands upright to the right of its base (below: to the left), spread down it as across it;
  kenten marks are drawn upright, centred on their characters.
- A paragraph with no kinsoku set (`<KinsokuSet>Nothing</KinsokuSet>`, InDesign's default here and
  538 corpus styles') breaks before a full stop. This InDesign reads `HardKinsoku`, which 81 corpus
  styles carry, as `Nothing`, so a set's rules could not be measured; the engine keeps its JIS
  tables for one.
- Under a Japanese composer a `LeadingOffset` frame puts the em box bottom one leading below the
  top (Kozuka 27.6 at 30 pt leading, Minion Pro 26.5; the Paragraph Composer 30.0), and a
  vertically centred block ends at its last line's em box bottom.
- Of 24 characters marked with kenten, the five punctuation marks (。、、「」) take none.

## Decision

- **Lay a vertical story out in its frames turned on their side and turn it back**, as before, but
  with the frame's width and height swapped and its insets turned, so a column is as long as the
  frame is tall.
- **Shape the characters that stand upright top to bottom** and draw each turned back a quarter
  from its column's centre line, so the frame's turn leaves it upright; Latin keeps turning.
- **Break Japanese between any two characters, one of them CJK**; a kinsoku set that names a table
  takes positions away, `Nothing` or none none.
- The Japanese composer's first-baseline and vertical-justification rules above; no kenten on
  punctuation.

## Result

Every glyph of every vertical case against InDesign's PDF within 0.02 pt — columns, upright and
turned glyphs, ruby and kenten, and a line whose kenten change from character to character, once
runs kern across their boundary (ADR 166) — the kinsoku cases this InDesign cannot measure
dropped from the probe.

## Not yet

- Tate-chu-yoko, rotated Roman, and per-character rotation (`CharacterRotation`).
- A vertical frame that is not a rectangle, wraps around other items, anchored objects, tables and
  footnotes in vertical text; underlines down a column.
