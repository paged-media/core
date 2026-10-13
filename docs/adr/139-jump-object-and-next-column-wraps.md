# ADR 139 — Jump-object and next-column wraps, and how wrapped lines break

- **Status:** Accepted 2026-10-13.
- **Scope:** `paged-renderer` (`geom::WrapKind`, `collect_wrap_rects_per_page`,
  `text_frame::jump_object_wraps` / `meets_next_column_wrap`, the wrap plan's breaker choice, the
  emitter's overflow test), `paged-gen` (`text-wrap`). No model, wire or protocol change.

## Context

The `text-wrap` fixture set 11 lines that ended above every obstacle, so none of its six wrap
modes touched a line and the pixel gate compared nothing about wrapping. Its story now runs past
the obstacles. InDesign 20.0.1's reference then showed three differences:

- **Jump object and next column were skipped.** The engine collected only bounding-box and
  contour wraps; text ran straight past the other two.
- **Contour was spelled `ContourTextWrap`.** InDesign opens that value as no wrap; its own value
  is `Contour` (every contour wrap in the corpus). The generator, the model's canonical string
  and plugin-publish's writer now spell it `Contour`; both spellings still read.
- **Wrapped lines broke differently.** Beside an obstacle InDesign set `Curabitur pretium
  tincidunt` in a 154 pt segment where the words measure 0.38 pt more: it squeezed the two word
  spaces. The engine's Knuth–Plass breaker for ragged text does not squeeze.

## Decision

Measured on the fixture's reference PDF:

- **Jump object.** No line sits beside the object. A line whose slug (from the row above's
  baseline down to its own) meets the wrap zone moves down one leading, row by row, until its
  slug clears; every following line moves with it. After a line at 265.88 pt, a 276–384 pt zone
  puts the next line at 409.88 pt, ten leadings down.
- **Next column.** The line whose slug meets the zone, and everything after it, continue in the
  next frame of the chain. In the last frame they are overset.
- **The breaker.** A frame whose wraps give its lines different measures breaks ragged text by
  minimum raggedness, which may squeeze word spaces down to `MinimumWordSpacing` to keep a word
  (`paged_text` `ragged.rs`, measured earlier on shaped frames). A frame whose wraps change no
  measure keeps Knuth–Plass.

## Consequences

- `text-wrap`'s pages measure mean ΔE 0.13–0.54 against InDesign (from 1.4–6.0 once the story
  reached the obstacles). Every line's text matches InDesign's on all six pages.
- A line that overflows into the next frame is placed there by the frame's own rules; that
  frame's jump-object wraps are not applied to it.
- Contour wraps still use the obstacle's bounding box.
