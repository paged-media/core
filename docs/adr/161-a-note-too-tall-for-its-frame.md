# ADR 161 — A note too tall for its frame: continued in the next, or its reference moved on

- **Status:** Accepted 2026-10-15.
- **Scope:**
  - `paged-model`: `FootnoteOptions::{no_splitting, continuing_rule}` (`ContinuingRule`).
  - `paged-renderer`: the footnote flow plan (`plan_footnote_flow`, `apply_footnote_flow`),
    which replaces the reservation cap. `EmittedFootnote::{first_line, end_line, continued}`.
    The area layout in line windows. The continuing rule.
  - `paged-gen`: the `footnote-split` and `footnote-nosplit` samples.
  - plugin-publish `idml-import`: `NoSplitting` and the `ContinuingRule*` attributes of
    `<FootnoteOption>`, with the rule's colour as a typed child.
  - No wire or protocol change.

## Context

A frame's notes stand at its foot, below its body text. Until now the engine reserved the room its
notes need, but never more than the room below the *lowest* reference in the frame, and cut what
did not fit. Two things followed when a note was too tall for the room under its reference:
- The text after the reference stayed in the frame, under the cut note.
- The lines that were cut were lost, not continued.

## The question to InDesign

`footnote-split` and `footnote-nosplit` are one A4 page with one story threaded through two
frames:
- The first paragraph runs to near frame 1's foot and carries an 11-line note.
- The next two paragraphs carry one-line notes.
- The continuing rule is on.
- The two samples differ only in `NoSplitting`.

InDesign 20.0.1 exported both, and the lines were read with `pdftotext -bbox`.

## Decision

**Each note must fit below its own reference line.**
- A frame's notes stand at its foot in reference order.
- Each note must fit between its reference line (plus `Spacer`) and the foot.
- Body text stops above them.

**The first note that does not fit ends the frame at its reference line.**
- With splitting allowed (`footnote-split`), the note keeps the whole lines that fit: nine, its
  last one hyphenated. Its last line sits on frame 1's foot.
- The text after the reference line moves to frame 2.
- The note's two remaining lines head frame 2's area under the continuing rule, before frame 2's
  own notes. Frame 2's area then ends on its foot.
- The continuing rule replaces the separator rule there. Its defaults are 1 pt, 288 pt wide, black.

**With `NoSplitting`, the reference line moves on** (`footnote-nosplit`).
- InDesign moves the line holding the reference to frame 2, not the whole paragraph. Frame 1 ends
  one line early.
- The whole note follows its reference.
- A column's first line cannot move on, so such a note splits after all.

**How the fixpoint reaches it.**
- The emitter still lays the story out with a reservation per frame and repeats until the
  reservation holds.
- Each pass, a flow plan replaces the measure-and-cap step. In chain order it walks each area's
  notes against their references, carries the lines of a split note into the next area, and sets
  each frame's reservation.
- A frame that moved a reference line on keeps that reservation from pass to pass. Without its
  note it would reserve nothing, and the line would come back.
- Once the layout settles, the plan limits the split note to its kept lines and adds a continued
  copy at the head of the next area. That copy is drawn from the line where the first stopped.

## Result

- `footnote_split::a_note_too_tall_continues_in_the_next_frame` and
  `::with_no_splitting_the_reference_line_moves_on` pin every note line within 0.05 pt of
  InDesign's.
- Both samples are gated on pixels and text. Mean ΔE is 0.285 and 0.279; main's engine, with
  text over the cut note and nothing continued, measured 1.868 and 2.818.

## Not yet

- **Growth.** An overset area does not grow its frame (L-01f).
- **Deferred rule settings.** The continuing rule's type and gap are not read.
- **Fixpoint cap.** The fixpoint still stops at its pass cap; a layout that has not settled by
  then keeps its last pass.
