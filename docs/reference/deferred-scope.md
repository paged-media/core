# Deferred scope — deep-defer records

**2026-06-07 · deferral record · status: ACCEPTED.**

The engine **badges and defers** the deepest IDML feature
families rather than implementing them. This file is the single dated rationale
record for each deferred row family: what InDesign/IDML expresses, what the
engine does **today** (every "today" line carries a code pointer), why it is
deferred, what "done" would look like, and the trigger that would re-prioritise
it.

The record was written against commit `819bdf9`. The code pointers were
re-located on 2026-10-02 at commit `9f933f1`. The IDML reader has since moved
out of this repository into `plugin-publish` (crate `idml-import`; see
[ADR 022](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/022-idml-relocates-to-plugin-publish.md)),
so parser pointers are written `plugin-publish: crates/idml-import/…` and were
checked at that repository's commit `6994ad1`. Where the code does something
other than the 2026-06-07 text says, a status note at the top of the section
says so; the text below it is unchanged.

Source comments cite this file by its earlier name, `deep-defer-records.md`,
and by the `§section` names below.

The companion is the feature registry (kept outside this repository): every row
below is aligned to status `planned` (or `partial` ONLY where something
genuinely ships, note-cited), with a `note:` back-pointing at the matching
`§section` here. No registry row claims `shipped` for any deferred family. The
"Registry alignment" line in each section names the affected rows.

**House note on "partial".** Several CJK rows are honestly `partial`, not
`planned`: the parser already captures the IDML attributes and the renderer has
an MVP emit/enforce path. The deferral is the *full* layout engine (vertical
writing, per-character ruby, table-driven mojikumi, flavour-specific kinsoku) —
not the absence of any code. Each section calls out the exact seam.

---

## §cjk-vertical-text — CJK vertical writing, kinsoku, mojikumi, ruby, kenten

*Status note (2026-10-02): "no vertical layout" below is incomplete. The
renderer rotates the commands of a vertical story by 90° after laying it out
horizontally (`crates/paged-renderer/src/pipeline/build_engine.rs:3251`,
`crates/paged-renderer/src/pipeline/text_frame.rs:1253`); there is still no
vertical layout axis, and frame auto-sizing skips vertical stories
(`crates/paged-renderer/src/pipeline/auto_size.rs:175`).*

**What IDML expresses.** `<Story StoryDirection="VerticalWritingDirection">`
(tate-gaki). Paragraph attrs `KinsokuSet` / `KinsokuType` (line-break
forbidden-char rules, push-in/push-out justification). `MojikumiTable` /
`MojikumiSet` (per-adjacency CJK punctuation spacing). Character-run attrs
`RubyFlag` / `RubyType` (`PerCharacter` | `GroupRuby`) / `RubyString` (furigana),
plus the full `RubyAlignment` / `RubyXOffset` family. `KentenKind` /
`KentenCharacter` / `KentenFontSize` (emphasis marks). Warichu, tcy
(tate-chu-yoko), and CJK-grid frames are further constructs.

**What the engine does today.**
- **Vertical writing** — `StoryDirection` (Horizontal/Vertical) is *parsed* and
  stored on the story; no vertical layout. `crates/paged-model/src/lib.rs:4674`
  (`enum StoryDirection`), `plugin-publish: crates/idml-import/src/story.rs:532`
  (attr read).
- **Kinsoku** — parsed (`plugin-publish: crates/idml-import/src/story.rs:692`,
  `kinsoku_set`/`kinsoku_type`); cascaded (`crates/paged-scene/src/lib.rs:1222`);
  the production renderer enables **hard-kinsoku enforcement keyed on presence
  only** — `crates/paged-renderer/src/pipeline/compose_opts.rs:214`
  (`lopts.compose.kinsoku_enforce = resolved.kinsoku_type.is_some()`); the
  composer adds per-character breaks at `crates/paged-text/src/compose.rs:128`.
  Push-in/push-out *flavour* behaviour is not implemented.
- **Mojikumi** — parsed (`plugin-publish: crates/idml-import/src/story.rs:694`);
  renderer enables a **uniform half-width tightening MVP** when a table/set
  resolves — `crates/paged-renderer/src/pipeline/compose_opts.rs:220`; the pass
  is `crates/paged-text/src/layout.rs:623` (`apply_mojikumi_half_width`).
  Per-table per-adjacency lookups are not implemented.
- **Ruby** — parsed (`plugin-publish: crates/idml-import/src/story.rs:967`,
  `ruby_flag`/`ruby_type`/`ruby_string`); cascaded
  (`crates/paged-scene/src/lib.rs:949`); renderer **emits a GroupRuby MVP**
  (`crates/paged-renderer/src/pipeline/decorations.rs:126`,
  `emit_ruby_for_line`); `PerCharacter` collapses to the same centred-group
  placement (no per-char distribution).
- **Kenten** — parsed (`plugin-publish: crates/idml-import/src/story.rs:970`);
  renderer **emits a filled-circle emphasis mark** above each glyph
  (`crates/paged-renderer/src/pipeline/decorations.rs:37`,
  `emit_kenten_for_line`); per-`KentenKind` glyph variants
  (Sesame/FisheyeDot/custom) collapse to the circle.

**Why deferred.** Full CJK fidelity needs a writing-mode-aware layout engine
across `paged-text` / `paged-compose` / `paged-renderer`: a vertical advance
axis, glyph rotation/substitution (vert/vrt2 OpenType features), tate-chu-yoko,
warichu, baseline-grid in the inline-progression axis, and ruby that
participates in line metrics rather than being painted as an overlay. That is a
layout-axis rewrite, not an attribute pass. Demand is low in the Western-print
corpus that drives the fidelity gate.

**What "done" looks like.**
- Vertical inline-progression axis end-to-end (compose → layout → emit), glyphs
  rotated/substituted per writing mode.
- Per-character ruby aligned over its base run, contributing to line height.
- Table-driven mojikumi (per-adjacency width adjustment) and flavour-specific
  kinsoku (push-in vs push-out justification).
- Tate-chu-yoko + warichu + CJK frame grid.
- Per-`KentenKind` mark glyphs.

**Trigger to re-prioritise.** A CJK-heavy sample lands in the test corpus, a
customer asks for Japanese/Chinese vertical layout, or the fidelity gate gains a
CJK exemplar. Until then this is the "CJK Stage 3/4" backlog.

**Registry alignment.** `typography.cjk-vertical-writing` (renderer `planned`),
`typography.kinsoku` / `typography.mojikumi` / `typography.ruby` /
`typography.kenten` (renderer `partial`, note-cited to the seams above).

---

## §optical-kerning — true optical kerning

**What IDML expresses.** `KerningMethod="Optical"` on a `<CharacterStyleRange>`
(also `"Metrics"` | `"None"`). Optical kerning derives inter-glyph spacing from
glyph *outlines* rather than the font's GPOS/`kern` table.

**What the engine does today.** Parsed
(`plugin-publish: crates/idml-import/src/story.rs:991`, `kerning_method`); the
shaper recognises the enum but **falls through to Metrics**:
`crates/paged-text/src/shape.rs:182` ("`Optical` falls through to Metrics for
now") and `:302` (`Optical` variant comment: "fall back to Metrics until the
outline-… pass"). NB: this is distinct from `OpticalMarginAlignment` (hanging
punctuation), which **is** implemented (`crates/paged-text/src/shape.rs:520`,
`optical_margin_offset`).

**Why deferred.** True optical kerning requires per-glyph outline analysis —
rasterise or sample the glyph contours, measure the white-space area between
adjacent glyph silhouettes, and solve for a spacing that equalises perceived
gaps. That is a glyph-geometry subsystem with its own calibration target, and
metrics kerning is a faithful-enough fallback for the vast majority of fonts
that ship real `kern`/GPOS data.

**What "done" looks like.**
- Glyph-outline sampling (area or distance-field) per adjacent pair.
- A spacing solver matching InDesign's optical output within the fidelity
  tolerance.
- Caching keyed on (font, size, pair) so it doesn't re-sample every compose.
- Calibrated against an optical-kerning exemplar.

**Trigger to re-prioritise.** A corpus sample sets `KerningMethod="Optical"` and
fails the fidelity gate visibly, or a customer authoring with optical kerning
asks. Metrics fallback keeps these documents *rendering* meanwhile.

**Registry alignment.** `typography.tracking-kerning` (renderer `partial`,
note "optical kerning → metrics fallback").

---

## §eps-import — EPS / PostScript image decode

*Status note (2026-10-02): this section predates two changes. The IDML element
is `<EPS>`, not `<EPSImage>`, and the reader matches `<EPS>` since 2026-08-04
(`plugin-publish: crates/idml-import/src/spread.rs:2100`). And the frame no
longer gets the grey placeholder: an EPS whose bytes resolve is reported as
`ImageDecodeFailed` with the frame's own fill left in place
(`crates/paged-renderer/src/pipeline/images.rs:37`, `:138`), and an EPS embedded
as inline content is flagged `has_inline_eps` and also keeps the frame's fill
(`crates/paged-model/src/lib.rs:1171`,
`crates/paged-renderer/src/pipeline/images.rs:140`). The EPS content is still
not decoded.*

**What IDML expresses.** `<EPSImage>` nested in a page-item rectangle/oval (a
placed `.eps` link), parallel to `<Image>` / `<PDF>` / `<ImportedPage>`.

**What the engine does today.** The parser **recognises `<EPSImage>`** as an
image-bearing child (`plugin-publish: crates/idml-import/src/spread.rs:2107`);
the renderer **sniffs the `%!PS-Adobe` magic and emits the missing-image
placeholder** rather than decoding —
`crates/paged-renderer/src/pipeline/image_decode.rs:33` (magic) and `:186`
("EPS / PostScript image detected; emitting missing-image placeholder"). So the
frame renders (grey + diagonal X + diagnostic), the EPS content does not.

**Why deferred.** Decoding EPS means embedding a PostScript interpreter (or
shelling to Ghostscript) — a large, security-sensitive dependency for a
legacy-vector format that modern InDesign workflows have largely replaced with
PDF/AI placement. The placeholder + diagnostic is honest and keeps the rest of
the page faithful.

**What "done" looks like.**
- A sandboxed PS/EPS rasteriser (or vectoriser) producing a raster or display
  list for the frame.
- Bounding-box (`%%BoundingBox`) honoured for fit/crop.
- Embedded preview (TIFF/WMF) used as a fast path when present.
- Diagnostic downgraded from "missing" to "rendered".

**Trigger to re-prioritise.** EPS-bearing corpus samples appear, or a customer's
print pipeline still places EPS. Until then the placeholder is the contract.

**Registry alignment.** `images-graphics.eps-decode` (parser `partial` —
"recognised + sniffed; emits placeholder, no decode"; renderer `planned`).

---

## §mixed-ink — mixed-ink spectral simulation

**What IDML expresses.** `<Color Model="MixedInk">` / `"MixedInkGroup"` — a
swatch defined as percentages of two or more named spot inks (e.g. Pantone +
process black), whose on-press appearance is a spectral blend.

**What the engine does today.** The model **recognises** the colour model
(`crates/paged-model/src/lib.rs:2267`, `ColorModel::MixedInk`) but
**treats it as `Unknown`** — "we don't ship the per-ink decomposition"
(`crates/paged-model/src/lib.rs:2258`-`2262`); resolution falls back to the
swatch's CMYK/RGB preview value. (For contrast, the same
`color-swatches.lab-mixed-ink` row's **Lab** half *is* shipped analytically —
D50→D65 Bradford → linear sRGB, `crates/paged-color/src/cmm.rs:141`, `:255`.)

**Why deferred.** Faithful mixed-ink requires spectral ink modelling: per-ink
spectral reflectance curves, a Neugebauer/Yule-Nielsen-style overprint model,
and a viewing-illuminant convolution — a colour-science subsystem. The CMYK-
fallback preview matches what InDesign shows for un-spotted output, so documents
render with a defensible approximation.

**What "done" looks like.**
- Per-ink spectral curves (measured or from an ink library).
- A spectral overprint/mixing model with illuminant.
- Decomposition of `MixedInkGroup` master + variant swatches.
- Calibrated against a mixed-ink exemplar under a known illuminant.

**Trigger to re-prioritise.** A prepress customer needs mixed-ink soft-proofing,
or a corpus sample with mixed inks fails proof. Preview fallback holds meanwhile.

**Registry alignment.** `color-swatches.lab-mixed-ink` (renderer `partial`,
note "Lab analytic; mixed-ink + spot-no-CMYK fall back to RGB/CMYK").

---

## §tagged-xml — tagged XML structure / XML import-export

**What IDML expresses.** `<XMLElement>` / `<XMLStory>` / `<XMLTag>` /
`<XMLAttribute>` — the document's tagged-XML backing store (structure pane), tag
markup mappings, and the import/export round-trip that drives data-merge and
structured workflows. Stories carry `AppliedXMLTag`; the `XML/` folder holds the
backing store.

**What the engine does today.** **Not parsed.** No `<XMLElement>` / `<XMLStory>`
consumer exists in the IDML reader (`plugin-publish: crates/idml-import`). (The
only `XMLStory` / `XMLTag` references in this repository are in the *fixture
generator* `crates/paged-gen/src/builders/xml_folder.rs:32` — it *emits* a
minimal XML folder for valid packages; nothing reads it back. The blank-package
writers `crates/paged-store/src/package.rs:70` and
`crates/paged-canvas/src/blank.rs:78` emit the same kind of stub.) The renderer
lays out the *visible* stories regardless; the XML structure layer is invisible
to it.

**Why deferred.** The tagged-XML backing store is a parallel data model
(element tree + tag-to-style mappings + import/export) that does not affect
visual rendering — a print-faithful renderer reads the laid-out stories, not the
structure pane. Building it is an editor/data-workflow feature, not a fidelity
one.

**What "done" looks like.**
- Parse `<XMLElement>` tree + `<XMLTag>` / `<XMLAttribute>` + `AppliedXMLTag`.
- Tag-to-style and style-to-tag mapping tables.
- XML import (merge into structure) and export (round-trip).
- Structure-pane read surface on the wire for the editor.

**Trigger to re-prioritise.** A structured-content / data-merge customer
workflow, or a round-trip requirement that must preserve the XML backing store.

**Registry alignment.** `tagged-xml.structure` (parser `planned`, note "backing
store skipped by design").

---

## §companion-formats — .indb books, .idms snippets, .icml InCopy

**What IDML expresses.** Sibling package formats around the `.idml` document:
`.indb` (book — an ordered list of member documents with shared
styles/swatches/numbering and book-level page/chapter numbering), `.idms`
(snippet — a fragment of page items / stories for reuse), `.icml` (InCopy
story — a standalone exported story), and InDesign object libraries.

**What the engine does today.** **Full-document entry only.** The IDML reader's
`open_source_archive` confirms the IDML mimetype
(`application/vnd.adobe.indesign-idml-package`) and locates the root
`designmap.xml` — `plugin-publish: crates/idml-import/src/lib.rs:112`-`150`.
There is no `.indb` / `.idms` / `.icml` entry point; no book-member resolution
(the "from sibling book documents" flag, `IncludeBookDocuments`, is
noted-but-not-followed at `crates/paged-model/src/lib.rs:3396`).

**Why deferred.** Each is a distinct container with its own root manifest and
cross-document resolution (books especially: shared resources, synchronised
styles, book-level numbering across N member documents). The engine's contract
is "one IDML package → faithful pages"; multi-document and fragment entry are
orchestration above that contract.

**What "done" looks like.**
- `.idms` / `.icml` fragment entry (parse a partial story / page-item set).
- `.indb` book manifest parse + ordered member resolution.
- Book-level page/chapter numbering across members.
- Synchronised-resource (style/swatch) cascade from the book master.

**Trigger to re-prioritise.** A customer with a book-based workflow, or a
snippet/InCopy round-trip requirement. Single-document IDML is the v1 contract.

**Registry alignment.** `companion-formats.idms-icml` (parser `planned`).
`.indb` books are **untracked — intentionally** (see below).

---

## §interactive-dynamic — buttons, behaviors, media, animation

**What IDML expresses.** Interactive-PDF / fixed-layout-EPUB constructs:
`<Button>` (multi-state object, states Up/RollOver/Down), button behaviors
(goto-page/state, show-hide, submit-form, zoom), `<Sound>` / `<Movie>` media,
and the animation + timing (`<AnimationSetting>`, motion presets, MSO timing).

**What the engine does today.** **Not parsed / not rendered.** The
`interactive-dynamic` chapter is tracked purely so the gap is visible
(the feature registry's chapter header: "Print-first renderer: none of this is
implemented"). No `<Button>` / `<Sound>` / `<Movie>` /
`<AnimationSetting>` consumer exists in the IDML reader
(`plugin-publish: crates/idml-import`).

**Why deferred.** These are dynamic/interactive behaviours with no print
output — out of scope for a print-faithful renderer by design. They imply an
event model, a media pipeline, and a timeline, none of which the page renderer
has or should grow.

**What "done" looks like.**
- Parse button multi-state + behaviors; render the default (Up) state for print.
- Media poster-frame rendering for print fallback.
- (Beyond print) an interactive runtime — explicitly out of the engine's charter.

**Trigger to re-prioritise.** A fixed-layout-EPUB or interactive-PDF *export*
product line — a different product surface, not the print renderer.

**Registry alignment.** `interactive-dynamic.buttons` / `.behaviors` / `.media`
/ `.animation` (all parser `planned`).

---

## §editorial-text — tracked changes, notes, endnotes, hidden text, drag-drop

**What IDML expresses.** Editorial / authoring constructs:
`<Change>` tracked changes (insertions/deletions with author + date),
`<Note>` editorial notes (anchored annotations, invisible in output),
endnotes (`<Endnote>` / endnote story), `<HiddenText>` (author-hidden runs), and
drag-and-drop text editing (an editor gesture, not an IDML element).

**What the engine does today.**
- **Hidden text** — the parser **recognises `<HiddenText>` and actively drops
  its content** (matches InDesign's default "hidden text not output"):
  `plugin-publish: crates/idml-import/src/story.rs:520`
  (`matches!(name, b"HiddenText" | b"Note")` → suppress) and `:1437`; tested by
  `track5c_hidden_text_block_drops_content` (`story.rs:2222`). So it is *not*
  "treated as normal text" — it is correctly suppressed.
- **Notes** — same suppression path; `<Note>` content is recognised and dropped
  (correct: editorial notes never render). Tested by `track5c_note_skipped`
  (`story.rs:2237`).
- **Tracked changes** — **not parsed.** `Story`-level `TrackChanges` is noted as
  a "followup parser slice" (`plugin-publish: crates/idml-import/src/story.rs:530`
  comment); no `<Change>` consumer.
- **Endnotes** — **not parsed.** No `<Endnote>` / endnote-story consumer. (NB:
  *footnotes* are a separate, partially-shipped feature — not in this family.)
- **Drag-and-drop text** — **not implemented** (editor gesture; no wire path).

**Why deferred.** Tracked changes and endnotes are authoring/review data with no
faithful-render obligation for the print product (changes render as their
accepted state; endnotes are a layout feature behind footnotes in priority).
Hidden text and notes are *already handled correctly* by suppression. Drag-drop
text is an editor-ergonomics gesture gated behind the caret/selection model.

**What "done" looks like.**
- Tracked-changes: parse `<Change>` spans; an editor mode to show/accept/reject;
  render accepted state for print.
- Endnotes: parse the endnote story + markers; lay out endnote bodies at story/
  document end.
- Hidden text / notes: a read surface to *list* them (already correctly hidden
  from layout) — an editor affordance, not a render change.
- Drag-drop text: a gesture that moves a selected range (cut+insert) with undo.

**Trigger to re-prioritise.** An editorial-workflow customer (tracked changes),
an endnote-bearing corpus sample, or the caret-model maturing enough to make
drag-drop a cheap follow-on.

**Registry alignment.** `stories-text.hidden-text` (parser `partial` —
"recognised + suppressed, not output"), `stories-text.notes` (parser `partial` —
"`<Note>` recognised + suppressed"), `stories-text.tracked-changes` (parser
`planned`), `stories-text.endnotes` (parser `planned`),
`stories-text.drag-drop-text` + `editor-tools.text.drag-drop` (gesture
`planned`).

---

## Untracked — intentionally

Deep rows the matrix never tracked, and which this record deliberately does
**not** invent registry rows for (no false coverage):

- **`.indb` books** — no `companion-formats.indb` row. The single
  `companion-formats.idms-icml` row covers snippets/InCopy; books are a distinct,
  larger workflow noted here under §companion-formats but not given a row until a
  book product line exists. (The book-document seam is flagged at
  `crates/paged-model/src/lib.rs:3396`.)
- **Tate-chu-yoko / warichu / CJK frame grid** — sub-constructs of CJK vertical
  text; folded into §cjk-vertical-text rather than tracked individually.
- **OpenType `vert`/`vrt2` vertical-glyph substitution** — a dependency of
  vertical writing; folded into §cjk-vertical-text.
- **Object libraries (`.indl`)** — InDesign library files; same family as
  snippets, no separate row, no demand.
