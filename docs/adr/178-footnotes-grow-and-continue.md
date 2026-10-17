# ADR 178 — A growing story grows a page for its footnotes, and a note continues from where it stopped

- **Status:** Accepted 2026-10-17.
- **Scope:**
  - `paged-renderer`:
    - the page-growth loop (`pipeline/mod.rs`);
    - the footnote flow (`pipeline/footnotes.rs`: `EmittedFootnote.resume_at`,
      `ComposedFootnote::resume_point`).
  - `paged-gen`: the `footnote-split-narrow` variant.
  - `tools/indesign-export/footnote-reflow-probe`.

## Context

A story with a grow rule (ADR 026) gains pages while its text is overset. The rule exists for
documents that paginate themselves, such as Word's (ADR 034). It grew only for dropped body text.
A note that ran past the last frame was cut off with a `FootnoteOverflow` diagnostic, even when the
story could have grown a page for it.

A note too tall for its frame continues in the next area (ADR 161). The continuation was counted in
lines of the note as composed in its first area: "line 9 onward". In an area of another width, the
note composes to different lines, and that count drops text or repeats it.

## What InDesign does (asked of InDesign 20.0.1)

`footnote-reflow-probe` turns on Smart Text Reflow and builds four documents. In the one where a
split note runs past the last frame, InDesign reports the note's end as overset, while the story's
`overflows` stays false. A note's overset is not the story's in InDesign's model. InDesign's
documents carry no grow rule; the measurement records how the two models differ.

The `footnote-split-narrow` variant continues the `footnote-split` note in a 300 pt frame instead
of a 420 pt one. InDesign's PDF resumes the note exactly after the hyphenated "ordi-" of its last
kept line ("nary words. N1 sentence 20 …"), and sets the rest at the new width.

## Decision

- **Growth.** The growth loop grows a story for a `FootnoteOverflow` of its own as it does for
  dropped text. A trailing grown page is kept while it holds a note of the story, even when it holds
  no body line.
- **Continuation from a text position.** A split note's continuation carries `resume_at`: the
  segment of the note, and the byte in that segment's shaped text where the next line starts. It is
  found from the lines the note kept (`ComposedFootnote::resume_point`). The continuation composes
  from there at its own area's width:
  - it skips the segments before the resume point and drops the text already set;
  - it keeps the run attributes, nested styles included;
  - its first line is not indented, and nothing stands above it.
- **Planning.** The planner composes the continuation again at the next area's width to know the
  room it takes there.
- **Fallback.** A split with no resume point (none found) keeps counting lines, as before.

## Consequences

- `footnote_growth` covers both cases:
  - with no grow rule, the note keeps the nine lines that fit and reports its overflow;
  - with one, a page is grown and every word of the note is set once, recomposed at the grown
    page's width.
- The `footnote-*` fixtures stay within their gates.
- `footnote-split-narrow` also shows the body text in a 300 pt frame set at the 420 pt measure of
  the frame before it. That is a separate defect of the chain layout, recorded and fixed on its own.
