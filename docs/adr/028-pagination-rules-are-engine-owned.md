# ADR 028 — Pagination rules are engine-owned and shared by every format

- **Status:** ACCEPTED 2026-10-02 (was PROPOSED 2026-10-01); implemented in
  `crates/paged-renderer/src/pipeline/keeps.rs`, the paragraph model in `crates/paged-model`
  and the settable paths of protocol 64. This is a forward-looking **ADR-as-memo**.
- **Extends:** [026](026-auto-growing-region-chains.md) (pages grow) and
  [007](007-carry-through-rendering-honesty.md) (no silent degradation).
- **Applies to:** `core` (`paged-model`, `paged-text`/`paged-renderer` story emission,
  `paged-mutate`, the wire), plugin-doc (lowering), plugin-publish (IDML import/export).

## The problem

Where a frame or page ends inside a story is decided by paragraph rules that both IDML and
DOCX carry. Core's state, verified 2026-10-01:

| Rule | IDML | DOCX | Core today |
|---|---|---|---|
| Keep with next (N lines) | `KeepWithNext` | `w:keepNext` | **Modelled and settable but not applied in layout.** It is parsed (`crates/paged-model/src/lib.rs:4338-4342`), settable (`crates/paged-mutate/src/apply/paragraph.rs:411`) and copied into the layout input (`crates/paged-renderer/src/pipeline/geom.rs:583-584`), but no code in `paged-renderer`, `paged-text` or `paged-compose` reads it. |
| Keep lines together | `KeepLinesTogether` | `w:keepLines` | Same as above (`crates/paged-model/src/lib.rs:4333-4337`, `crates/paged-mutate/src/apply/paragraph.rs:423`). |
| Widow/orphan control | `KeepFirstLines` / `KeepLastLines` | `w:widowControl` | Not modelled. |
| Break before (column/frame/page/odd/even) | `StartParagraph` | `w:pageBreakBefore`, `w:br w:type="page"`, section breaks | Not modelled. |

This inverts the usual capability gap, where the renderer honours a field that has no
setter: these fields are *settable* and *not rendered*. A user who sets keep-with-next sees no effect and gets no diagnostic.
[007](007-carry-through-rendering-honesty.md) forbids that.

Once pages grow ([026](026-auto-growing-region-chains.md)), these rules decide the page count.
They can no longer be skipped.

## The decision

1. **The rules live in core story emission, once, for every format.** IDML and DOCX lower
   their rule attributes onto one native paragraph vocabulary. The flow driver applies it when
   deciding where a frame ends. Format plugins do not paginate.
2. **Apply the two modelled rules first.** Keep-with-next and keep-lines-together get applied
   in layout before anything new is added. Until they are, setting them produces a diagnostic
   ("modelled, not yet applied"), not silence.
3. **Add the missing rules to the model and the wire:**
   - `keep_first_lines` and `keep_last_lines` (widow/orphan; DOCX `widowControl` lowers to
     2/2);
   - `start_paragraph` (anywhere / next column / next frame / next page / next odd / next even).

   Each comes with a property path and a setter, following [005](005-wire-recipe.md). This
   ships in the same protocol bump as [026](026-auto-growing-region-chains.md) where possible,
   so consumers move pins once.
4. **The rules only look locally.** A break decision may look at most one region ahead. This
   keeps the [027](027-incremental-flow-invalidation.md) early stop valid: comparing break
   positions *after* the rules is enough to know later frames are unchanged.
5. **Use InDesign's rule.** For IDML semantics, use what InDesign does: author the case with
   `paged-gen`, have `tools/indesign-export` answer, and gate on `diff.sh` (the method that settled `VerticalBalanceColumns`). For DOCX-only semantics, Word's export is the oracle, via
   the Office automation lane.

## Consequences

- Existing IDML documents that use keep-with-next will **re-paginate** when the rule starts
  being applied. That is a fidelity *gain* against InDesign, but it will change fidelity-gate
  baselines. Re-baseline in the same change and say so in the commit.
- Contradictions resolve the way InDesign resolves them: when a keep group can't fit, the
  rule yields and a diagnostic is emitted. No infinite push.
- Footnote placement at the page bottom stays out of scope. Today a footnote taller than its
  column is reported, not split (`crates/paged-renderer/src/pipeline/deltas.rs` module doc).

## What this ADR does NOT decide

- Vertical justification and balancing interactions beyond what already exists
  (`VerticalBalanceColumns`).
- Table row keep rules (rows already split between rows with header repeat in plugin-web; core
  tables are a separate review).

## 2026-10-01 implementation record: keep options shipped, measured against InDesign

**Asked InDesign first** (§5). A new paged-gen fixture, `keeps`, puts each rule
at a column break on an exact line grid (12 pt leading, `LeadingOffset`, 126 pt
two-column frames = ten lines per column, ~80 pt tokens so a K-token paragraph
is K lines), each beside a no-keep control. InDesign 2025's export answered:

| Rule | InDesign |
|---|---|
| KeepWithNext on a 1-line paragraph | the paragraph moves |
| KeepWithNext on a 3-line paragraph | **only its last line moves** (2 \| 1) |
| KeepAllLinesTogether, 2 \| 2 straddle | whole paragraph moves |
| KeepFirstLines=2, 1 \| 3 | whole paragraph moves |
| KeepLastLines=2, 3 \| 1 | one line pulled over: 2 \| 2 |

The KeepWithNext row overturns the "move the paragraph" reading this ADR's
table implied.

**The model was also wrong in a way the table did not show.** IDML's
`KeepLinesTogether="true"` only switches keeps ON; `KeepAllLinesTogether` picks
all-lines over the `KeepFirstLines` / `KeepLastLines` counts (default 2 / 2).
The model documented the boolean as "all lines", which would have been wrong
for most real documents. And the style parser read no keep attribute at all,
although real documents set keeps on paragraph styles.

**What shipped.**
- core `4ef7b21`: the full keep set on `Paragraph`, `ParagraphStyleDef`,
  `ResolvedParagraph` and `ResolvedParagraphAttrs`, cascading style → paragraph.
  `pipeline/keeps.rs` turns every rule into "this line opens the next frame",
  decided from the previous pass's real line placement. The body-story
  re-emit loop (the footnote fixpoint) now also iterates on those breaks
  (`MAX_KEEP_PASSES`); a story without keeps takes exactly its old passes.
- plugin-publish `1aad636`: imports the attributes on ranges and on styles,
  and exports the new range attributes.
- core (after moving its plugin-publish pin `412b884` → `1aad636`):
  `crates/paged-renderer/tests/keeps_pipeline.rs` pins all 8 InDesign splits, and `keeps` joins the
  fidelity gate with InDesign's PDF as reference (8/8 pages; worst page mean ΔE
  0.453).

**Still open (this ADR):** `start_paragraph` (break before column/frame/page,
odd/even) is not modelled. Setters for the three new property paths ride the
protocol 64 bump with [026](026-auto-growing-region-chains.md). The rule is
evaluated at a paragraph's FIRST break only; a paragraph spanning three
frames gets its keeps at the first break.

## Addendum 2026-10-01 (later): break-before, and the style setters

**`start_paragraph` is built.** InDesign 2025 answered first: the paged-gen
`start-paragraph` fixture (ten cases, each one story threaded through a four-page chain
whose page 1 holds a two-column frame and a second frame) was exported by InDesign, and
where each case's rule paragraph landed is the specification:

| Rule | InDesign 2025 |
|---|---|
| NextColumn / NextFrame / NextPage | opens the next column / frame / page |
| NextOddPage / NextEvenPage | opens the next page of that parity (skipping one if needed) |
| NextPage on a story's first paragraph | does not move (it already opens a page) |
| NextEvenPage on a story's first paragraph, on an odd page | moves to the next even page |
| NextFrame on a paragraph that already opens a frame | does not move |
| NextColumn on a paragraph that already opens a column | does not move |

So the rule is "open a new unit unless the paragraph already opens one", and parity is
checked even at the top. With no unit left the rest of the story oversets, which a growing
chain ([026](026-auto-growing-region-chains.md)) answers with pages. Unlike the keeps this
needs no fixpoint: it is decided at the paragraph's first line (`keeps::start_target`).
Shipped: core `1a64483` (model), plugin-publish `912d2d0` (import/export), core `6b85b5d`
(pipeline, `crates/paged-renderer/tests/start_paragraph_pipeline.rs`, fidelity gate 40/40 pages, worst mean ΔE
0.130). The wire setter rides protocol 64 with the three keep paths.

**Found on the way: styles refused most paragraph paths.** `SetStyleProperty` accepted
only size, tracking, fill, family, face, leading, spacing, first-line indent and
justification. Keep-with-next, keep-lines-together, the indents, hyphenation, tabs, lists,
and (on both style kinds) case, position, baseline shift, underline and strike-through
were refused, and a refused child rolls back its whole batch. plugin-doc lowers Word's
direct formatting to synthesized styles (ADR 029),
so all of these were lost on a Word import. The editor spec only counted pages, and 5 pages
came out either way. Fixed in core `f6ba605`: each style path uses the paragraph-level
setter's own field helper, so the two levels cannot disagree.

## Addendum 2026-10-01 (evening): span and split columns, list markers

**Span and split columns are built** (core `ef4cb28` model, plugin-publish `b490080`
import/export, core `879465d` fixture + `a3e5a0c` pipeline). InDesign 2025 answered with the
17-case `span-columns` fixture, read from its own DOM and its PDF: text above a span balances
over the spanned columns by line count (`ceil(lines / k)` each), the span sets at the spanned
width below the deepest line above it, with `max(SpaceBefore, SpanColumnMinSpaceBefore)`
above and the same rule below; consecutive split paragraphs form one block of `k`
sub-columns `(column − 2 × outside − (k − 1) × inside) / k` wide, balanced by line count,
and the next paragraph starts below the deepest sub-column. `SpanSplitColumnCount` is only
read as a typed `<Properties>` child, never as an attribute (the attribute silently means
All). All 256 lines land within 0.13 pt of InDesign's, the rest being InDesign's half-stroke
text inset, which the engine does not apply. Gate: 17/17 pages, worst mean ΔE 0.203. The
wire setters ride protocol 64; plugin-doc can then lower a Word section that changes the
column count mid-page to a split block instead of a page break.

**List markers** (core `b3ff836`): an undeclared `BulletsTextAfter` is a tab; a tab goes to
the first stop past the pen among the tab list plus a hanging left indent, else the next
36 pt default stop from the frame edge; a left tab may be narrower than the tab glyph.
Measured with the `list-markers` fixture (14 cases); `numbering` tightened from mean 0.471 to
0.091 by it. Still open: local (non-style) bullet and numbering overrides are dropped on
import, and line breaking still measures a tab at its glyph width.

## Amendment — 2026-10-02

Checked against the code at `9f933f1`. The three records above describe what was built on
2026-10-01 and still hold. The body of the ADR (its table, decisions and consequences) was
written before that work and is superseded in the places listed below. The items the records
leave open are closed, except the one in item 6.

**1. The table in "The problem" is superseded.** All four rows are modelled and applied.

- `crates/paged-model/src/lib.rs:4815-4838` (`Paragraph`), `:3933-3955` (`ParagraphStyleDef`),
  `:4233-4249` (`ResolvedParagraph`) — `keep_lines_together`, `keep_all_lines_together`,
  `keep_first_lines`, `keep_last_lines`, `keep_with_next`, `start_paragraph`, `span_columns`.
- `crates/paged-model/src/lib.rs:91-99` — `StartParagraph`: `Anywhere`, `NextColumn`,
  `NextFrame`, `NextPage`, `NextOddPage`, `NextEvenPage`.
- `crates/paged-renderer/src/pipeline/keeps.rs:15-36` — the keep rules as applied;
  `:273-279` — `start_target`, the break-before rule.

**2. The setters are on the wire.** This closes "Setters for the three new property paths
ride the protocol 64 bump", "The wire setter rides protocol 64" and "The wire setters ride
protocol 64" in the records: protocol 64 is the version at the pinned commit
(`crates/paged-canvas/src/channel.rs:510`).

- `crates/paged-introspect/catalog.json:1232-1243` — the settable paths
  `paragraphKeepLinesTogether`, `paragraphKeepWithNext`, `paragraphKeepAllLinesTogether`,
  `paragraphKeepFirstLines`, `paragraphKeepLastLines`, `paragraphStartParagraph` and the six
  span and split paths.
- `crates/paged-mutate/src/apply/layer.rs:2004-2047` — the same paths on a paragraph style.

**3. Decision 2's interim diagnostic was not added.** The two modelled rules were applied
instead, so "Until they are, setting them produces a diagnostic" never took effect. The
string "not yet applied" does not occur in `crates/`.

**4. Decision 4 describes a lookahead; the code decides from measured placement.** A story
is emitted, the frame of every line is recorded, violations become forced breaks, and the
story is emitted again until the set of breaks stops changing
(`crates/paged-renderer/src/pipeline/keeps.rs:17-21`). A break applies only while the frame it
was decided in still starts with the same line (`:38-49`). The extra passes are capped by
`MAX_KEEP_PASSES = 4` on top of the footnote passes and two per chain frame
(`crates/paged-renderer/src/pipeline/build_engine.rs:22-29`, `:3092`). This supersedes "A break
decision may look at most one region ahead".

**5. A rule that cannot be met yields without a diagnostic.** This supersedes, in
Consequences, "the rule yields and a diagnostic is emitted". A break is never forced onto a
line that already opens its frame, and only where a next frame exists
(`crates/paged-renderer/src/pipeline/keeps.rs:35-36`, `:215-218`). No diagnostic code refers to
keeps (`crates/paged-renderer/src/diagnostics.rs`).

**6. Still as recorded.** The within-paragraph keeps are evaluated at a paragraph's first
break only (`crates/paged-renderer/src/pipeline/keeps.rs:187-192`). A footnote taller than its
column is reported, not split (`crates/paged-renderer/src/pipeline/deltas.rs:43-46`).

**7. The two list remarks at the end of the last record are closed.**

- Local list overrides: `crates/paged-model/src/lib.rs:4719` — `Paragraph` carries the list
  attributes itself (core `d10ffc9`, rendered in `6192c72`);
  `plugin-publish: crates/idml-import/src/story.rs:607` — the importer reads
  `BulletsAndNumberingListType` on a paragraph range. The paths `paragraphListType` to
  `paragraphNumberingCharacterStyle` are settable
  (`crates/paged-introspect/catalog.json:1247-1255`).
- Tabs in line breaking: `crates/paged-text/src/layout.rs:186`, `:208` — the layout options
  carry a `TabLayout`, so a line breaks where its tabs land (core `6b1c11a`).

**8. Gates.** The fixtures `keeps`, `keeps-reflow`, `start-paragraph`, `span-columns` and
`list-markers` are in the fidelity gate (`corpus/generated/fidelity-thresholds.json:152`,
`:168`, `:384`, `:432`, `:408`), and the pipeline tests are
`crates/paged-renderer/tests/keeps_pipeline.rs`, `keeps_reflow_pipeline.rs`,
`start_paragraph_pipeline.rs` and `span_columns_pipeline.rs`. `keeps-reflow` was added on
2026-10-02 for keeps on a growing chain (core `dd3bfe7`).

Outside this ADR's scope, but changed since: a table row now carries `keep_with_next_row`
(`crates/paged-model/src/lib.rs:5196-5200`).
