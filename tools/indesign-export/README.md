# tools/indesign-export

Automates the InDesign-side leg of the corpus pipeline. The Rust
generator emits `.idml` files into `corpus/generated/`; this script
opens each one in InDesign and writes a sibling `<stem>.pdf` reference
PDF using the `[High Quality Print]` preset.

## Prerequisites

- macOS with Adobe InDesign 2025 installed (override the default name
  via `INDESIGN_APP="Adobe InDesign 2024"` etc.).
- The fonts the generated samples reference must be installed in the
  system. Phase 0 only uses **Open Sans**; later phases will pin a
  larger fixture font set.

## Usage

```bash
# 1. Generate the IDMLs.
cargo run -p paged-gen -- emit --sample geometry --out corpus/generated

# 2. Run the InDesign export pass. The script activates InDesign,
#    iterates corpus/generated/*.idml, and writes corpus/generated/*.pdf.
bash tools/indesign-export/run-export.sh

#    Or re-export ONE fixture, which is what you want when a single
#    reference turns out to be wrong:
PAGED_EXPORT_ONLY=text bash tools/indesign-export/run-export.sh

# 3. Run the diff harness. corpus/generated/render-diff.sh resolves either
#    corpus/samples/<name>.{idml,pdf} or corpus/generated/<name>.{idml,pdf}.
bash corpus/generated/render-diff.sh geometry
```

## Outputs

For every `corpus/generated/<stem>.idml` the export pass writes:

```
corpus/generated/<stem>.pdf
corpus/generated/<stem>.export.meta.json
```

The `meta.json` records:

```json
{
  "idml": "geometry.idml",
  "pdf": "geometry.pdf",
  "indesign_version": "20.x",
  "preset": "[High Quality Print]",
  "exported_at": "2026-04-29T..."
}
```

**Re-export one fixture at a time.** A reference PDF is baked
evidence; rewriting all of them in one pass rebaselines every threshold
in `fidelity-thresholds.json` at once, and nobody can then tell a fixed
reference from a hidden regression. `PAGED_EXPORT_ONLY=<stem>` exists
for that reason. After a re-export, check what the new PDF actually
embeds (`pdffonts <stem>.pdf`) — the usual reason a reference is wrong
is that the export host lacked the font the IDML declares and InDesign
substituted its own.

Pin the InDesign version when the corpus is committed and re-export
only on conscious upgrades — InDesign's PDF output is not
deterministic across point releases.

## The InDesign round-trip lane

`lane.sh` asks InDesign whether it reads the engine's IDML **export** the
way the engine meant it. The engine's own round trip proves nothing about
that: a spelling the writer and the reader share can be one InDesign
ignores.

```bash
bash tools/indesign-export/lane.sh              # every scenario
bash tools/indesign-export/lane.sh bleed        # one
```

Each scenario (`crates/paged-canvas/tests/indesign_lane.rs`) is a gen sample,
edited through the canvas model and exported by the writer. `lane.jsx` opens
the export in InDesign, records InDesign's view of it (`lib/oracle.jsx`,
schema `paged.indesign-view/1`), saves and reopens it, records again, and
exports InDesign's own re-save. The recording lands in
`crates/paged-canvas/tests/fixtures/indesign-lane/<scenario>/`.

The test `indesign_lane::recordings_agree_with_indesign` runs in CI without
InDesign. On the fields a scenario declares, it compares the engine's view
of its export with InDesign's `opened` view, and the engine's view of
InDesign's re-save with InDesign's `reopened` view. When the writer's output
no longer matches the recorded digest the first comparison is skipped and
the scenario reported STALE; record it again with the change that moved it.
`PAGED_LANE_REQUIRE_FRESH=1` fails on a stale recording, for a release.

## Caveats

- The driver runs with `userInteractionLevel = NEVER_INTERACT`, which
  suppresses *most* dialogs but not all. Missing fonts, missing
  links, or permission prompts will still pop. Plan a one-time
  manual pass after each new sample lands.
- Every InDesign export advances the application's recently-opened-
  files list. There's no public API to suppress that.
- `ExportFormat.PDF_TYPE` corresponds to the print PDF path. For
  interactive samples (Phase 4+, if interactive PDF lands in scope)
  swap to `ExportFormat.INTERACTIVE_PDF` and a different preset.

## Probes

Besides the export pass, each `*-probe.sh` asks InDesign one question and
pins the answer (a `*-probe.json` beside it where the answer is data). They
take the InDesign GUI for a few seconds and share the lock
`/tmp/paged-indesign.lock` with other sessions.

- `cells-probe`, `conditions-probe`, `inline-objects-probe`,
  `nested-styles-probe`, `object-integrity-probe`, `reflow-probe`: open a
  generated fixture and read back what InDesign made of it.
- `spelling-and-spacing-probe`: builds its own sample through InDesign's
  scripting, so nothing Paged wrote stands between question and answer, and
  reports the attribute spellings InDesign exports (`DropcapDetail`,
  `RubyFlag="1"`, `CapToSmallCap`, `TextAlignment` / `PathAlignment` on a
  `<TextPath>`), letter spacing as a share of the space width, tracking and
  letter spacing left off a line's last character, superscript and
  subscript shifts, paragraph-rule offsets, and where type on a path sits
  for each text and path alignment (measured from InDesign's PDF).
- `cjk-composer-probe`: ruby and emphasis marks (kenten) print only under
  InDesign's Japanese composers (`HL Composer J`, `HL Single J`), never
  under the Paragraph or World-Ready Composer.
- `master-items-probe`: InDesign writes `ShowMasterItems` on `<Spread>`
  and ignores it on `<Page>`; a spread that shows master items prints them
  whatever its page says.
- `footnote-paragraphs-probe`: a note's own paragraph settings — alignment,
  indents, leading, tab stops, word and letter spacing, glyph scaling, a
  paragraph style — lay out its lines as they lay out body text, and its
  `SpaceBefore` / `SpaceAfter` count only between the note's own
  paragraphs. It also pins two body-text rules: every paragraph of a range
  (a `<Br/>` between them) keeps the range's space before and after, an
  empty one too, and a flush-right or centred line with a tab aligns only
  its text after the last tab. Answers in `footnote-paragraphs-probe.json`.
- `ruby-kenten-probe`: how big, and where, InDesign sets ruby and emphasis
  marks by default: both at half the base's size, each mark's em box
  standing on the base's (a face's `BASE` ideographic baseline, or one em
  centred on half its cap height), kenten centred on their characters,
  ruby spread 1:2:1 over a wider base and a narrower base spaced out to
  its ruby, per-character ruby split at spaces. Read from InDesign's PDF
  with `pdf-text.py`; answers in `ruby-kenten-probe.json`.
- `cjk-first-baseline-probe`: where a frame of CJK text sets its first
  baseline. A CJK face's ascent is the top of its `d` and its descent the
  bottom of its `p`, not its `hhea` (a variable one, Noto Sans JP: one em
  below the top of its ideographic character face); its em box stands one
  em above its `BASE` ideographic baseline, or, with no `BASE`, is centred
  on its 永. Answers in `cjk-first-baseline-probe.json`.
- `ruby-kenten-options-probe`: what each ruby and kenten setting does —
  size, horizontal and vertical scale, offsets (up, above or below the
  line), `BelowLeft`, the seven `RubyAlignment`s, `AlignKentenLeft`, and a
  custom mark's character and font. Read from InDesign's PDF; answers in
  `ruby-kenten-options-probe.json`.
- `object-style-corners-probe`: which of an object style's corner
  attributes InDesign reads. Like an item's, only the four per-corner
  ones: a style with just `CornerOption` / `CornerRadius` gives square
  corners, the per-corner attributes win where the two disagree, and an
  option with no radius is 12 pt whatever `CornerRadius` says. InDesign
  authors and exports the style; the script rewrites its corners into
  variants and InDesign reads each back through its DOM. Answers in
  `object-style-corners-probe.json`.
- `ruby-parent-spacing-probe`: what InDesign does with ruby wider than its
  base (ADR 166): it overhangs the kana, brackets or Latin beside the base
  by up to one ruby character a side (`RubyParentOverhangAmount`) and no
  more than the neighbour's natural width, never a kanji or past a line's
  edge, narrows if `RubyAutoScaling` allows, and
  spaces the base with what is left by `RubyParentSpacing` (1-2-1, both
  sides, equal aki, full justify, or none: centred). Answers in
  `ruby-parent-spacing-probe.json`.
- `kana-spacing-probe`: kana under each kerning method. Metrics sets
  Kozuka's kana at their proportional widths (`palt`) plus kerning pairs;
  kerning off, a full em each.
- `kerning-runs-probe`: whether the font's kerning applies across a style
  run's boundary (ADR 166). Minion Pro 30 pt "AVATAR To Wa" with every
  other character recoloured or underlined sets exactly as one run; at
  29 pt, or in Myriad Pro, each boundary sets as kerning off. Answers in
  `kerning-runs-probe.json`.
- `vertical-text-probe`: vertical text (ADR 167). InDesign's scripting has
  no story orientation, so InDesign builds the frames horizontally, the
  script turns each story vertical in the IDML, and InDesign opens and
  exports that. Vertical text is the horizontal layout turned a quarter
  clockwise: CJK upright in vertical forms (`vpal`, `vkrn` under Metrics),
  Latin turned, ruby and kenten upright to the right; a paragraph with no
  kinsoku set breaks before a full stop; punctuation takes no kenten; a
  kenten change between two characters keeps their kerning pair.
  Answers in `vertical-text-probe.json`.
- `pdf-glyphs.py`: every glyph a PDF page paints, one JSON object each —
  its character (from `ToUnicode`), where its outline is drawn from, its
  size and horizontal scale, its advance, its writing mode (`Identity-V`
  fonts) and angle. Fonts are resolved per page.
