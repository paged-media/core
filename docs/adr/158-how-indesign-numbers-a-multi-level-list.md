# ADR 158 — How InDesign numbers a multi-level list

- **Status:** Accepted 2026-10-15.
- **Scope:**
  - `paged-model`: `ListLevel {level, apply_restart, restart_policy}`, `RestartPolicy`,
    `RestartPolicyKind`, and the `list_level` field on `Paragraph`, `ParagraphStyleDef` and
    `ResolvedParagraph`.
  - `paged-scene`: `ResolvedParagraphAttrs::list_level`, cascaded like the other list attributes.
  - `paged-renderer`: `numbering::ListLevels`, the story emitter's per-level counts, and `^1` … `^9`
    in a numbering expression.
  - `paged-mutate` / `paged-introspect`: the paths `paragraphNumberingLevel` and
    `paragraphNumberingApplyRestartPolicy` (settable paths 239 → 241).
  - `paged-gen`: the `numbering-multilevel` sample.
  - plugin-publish `idml-import` / `idml-export`: `NumberingLevel`, `NumberingApplyRestartPolicy`
    and the `<NumberingRestartPolicies>` child, read and written.
  - No wire or protocol change: two property paths are additive.

## Context

The engine numbered every paragraph of a list on one counter. It read neither `NumberingLevel` nor
the restart policy, and a numbering expression's `^1` … `^9` printed as written. Every InDesign
document carries the three attributes on its paragraph styles. The census sees only level 1 there,
but any outline, legal or technical document numbered `1.a.i.` uses the levels.

## The question to InDesign

`numbering-multilevel` is one A4 page with sixteen paragraphs on one numbering list. They use four
paragraph styles:
- `L1`: level 1, `1, 2, 3, 4...`, expression `^1.^t`.
- `L2`: level 2, `a, b, c, d...`, `^1.^2.^t`, restart policy `AnyPreviousLevel`.
- `L3`: level 3, `i, ii, iii, iv...`, `^1.^2.^3.^t`, likewise.
- `L2Keep`: level 2, `1, 2, 3, 4...`, `^1-^2^t`, `NumberingApplyRestartPolicy="false"`.

The paragraphs go down and up the levels, skip level 2 once, and set the no-restart style in two
chapters. InDesign 20.0.1 exported the page, and the numbers were read with `pdftotext`:

```
1.  1.a.  1.b.  1.b.i.  1.b.ii.  1.c.  1.c.i.
2.  2.c.i.  2.a.
3.  3-2  3-3
4.  4-4  4.e.
```

InDesign reads the three as they appear here:
- `NumberingLevel` and `NumberingApplyRestartPolicy` are attributes of a paragraph style or a
  range.
- The policy is an empty `<Properties>` child,
  `<NumberingRestartPolicies RestartPolicy="AnyPreviousLevel" LowerLevel="0" UpperLevel="0"/>`.

## Decision

**A level starts again only when a higher level came after its own last paragraph.**
- The point after `Chapter two` is `i` again, and so is the section after it (`2.a.`).
- With no higher paragraph in between, a level goes on counting (`4-4` then `4.e.`: one count, two
  formats).
- The restart policy names which higher levels count: any previous level, one specific level, or a
  range of levels.

**A higher level never resets the lower ones.**
- The point under `Chapter two`, with no section yet, prints `2.c.i.`. Level 2 still holds the `c`
  of chapter one's last section.

**`^n` prints level n's count, in the format of the paragraph that last set level n.**
- `^1` in `2.c.i.` is level 1's `2`.
- `^2` is the `c` from an `L2` paragraph.
- `^#` is the paragraph's own level.

**`NumberingApplyRestartPolicy="false"` keeps the level counting across higher levels.**
- `L2Keep` shares level 2 with `L2`, so `3-2` follows chapter two's `2.a.` (count 1).
- `4.e.` continues from `4-4`.

**Level 1 keeps the engine's existing rules.** These are the implicit restart after a non-list
paragraph, `NumberingStartAt`, `NumberingContinue` and lists continued across stories. It also
records its count for the levels below.

The emitter keeps the per-level state (count, format and order of the last paragraph) next to its
level-1 counter. The state is saved with every resume mark, so an incremental re-render continues
the same counts.

## Result

- `pipeline::tests::multilevel_list_numbers_as_indesign_does` replays the sixteen paragraphs
  through the renderer's numbering.
- `break_records::a_multilevel_list_prints_indesigns_numbers` checks the same through the import
  and a full build.
- `numbering-multilevel` is gated on pixels (mean ΔE 0.0205, SSIM 0.99986) and on text: all sixteen
  lines read as InDesign printed them.

## Not yet

- **Lists that continue across documents.** `ContinueNumbersAcrossDocuments` needs books (D-10).
- **Levels on the list itself.** `editNumberingList` takes no level fields yet. A paragraph style
  carries a level's format and expression, as InDesign's own documents do.
- **The restart policy as a path.** It is read, kept and written. No path sets it yet, because the
  policy's lower and upper levels need a record value.
