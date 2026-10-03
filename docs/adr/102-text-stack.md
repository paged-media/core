# ADR 102 — The text stack: harfrust shaping, total-fit line breaking, composers measured against InDesign

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-text` (`shape.rs`, `compose.rs`, `layout.rs`, `single_line.rs`, `ragged.rs`, `first_fit.rs`); `crates/paged-renderer/src/pipeline/compose_opts.rs`

## Context

The crate lists among its responsibilities "Knuth-Plass line breaking with
InDesign-calibrated penalty weights" (`crates/paged-text/src/lib.rs:19`). The fidelity gate
([ADR 105](105-fidelity-gate.md)) compares each page with InDesign's own output.

InDesign offers two composers per paragraph. The model carries the choice as `Composer`
(`"HL Composer"`, the Paragraph Composer, and `"HL Single"`, the Single-line Composer, plus
their World-Ready variants; `crates/paged-model/src/lib.rs:3136-3142`). Two module docs
record where Knuth–Plass does not describe InDesign. The Single-line Composer sets "one
line at a time, never looking back" (`crates/paged-text/src/single_line.rs:15-16`). For
ragged text in a shaped frame "Knuth–Plass prices a ragged line by how far its word spaces
would have to stretch", while "InDesign does the opposite" (`crates/paged-text/src/ragged.rs:19-22`).

Shaping moved from `rustybuzz` to `harfrust` on 2026-08-18 (commit `19f0530`). The manifest
states the reason: harfrust is "rustybuzz's official successor on the fontations stack"
(`crates/paged-text/Cargo.toml:15-17`).

## Decision

`paged-text` shapes with `harfrust`, breaks the Paragraph Composer's lines with the
`paragraph-breaker` crate's `total_fit`, and carries its own breakers for the cases where
total-fit does not reproduce InDesign.

- **Shaping.** Every homogeneous run goes through `harfrust`. Metrics are read through a
  parallel `ttf_parser::Face` over the same font bytes. Advances are integers in 1/64 pt.
- **Paragraph Composer.** `paragraph_breaker::total_fit` with defaults "calibrated against
  InDesign's Paragraph Composer": tolerance 8, stretch ratio 0.33, shrink ratio 0.2. The
  ratios mirror InDesign's word-spacing preset of 80 / 100 / 133 percent.
- **Single-line Composer.** A hand-written breaker in `single_line.rs`, used when the
  paragraph's composer is one of the two single-line values. Its rules were "measured on
  InDesign 20.0.1".
- **Ragged text in a shaped frame.** `ragged.rs` minimises the sum of the squared space
  each line leaves at the end of its band. The renderer turns it on when a frame in the
  chain has a shape; justified text does not use it.
- **When total-fit finds no breaks.** The tolerance is retried at 4 times, 16 times and
  1000; ragged text is then retried with fill glue; the last resort is greedy first-fit in
  `first_fit.rs`.

The `paragraph-breaker` crate is used, not an own Knuth–Plass implementation.
The repository does not record why.

## Evidence

- `crates/paged-text/Cargo.toml:18-22` — `harfrust = "0.13"`, `ttf-parser`, `paragraph-breaker = "0.4"`, `hypher`, `unicode-bidi`
- `crates/paged-text/src/shape.rs:23-45`, `:107-108` — the `Face` wrapper (harfrust view plus `ttf_parser::Face`); `ADVANCE_PRECISION = 64.0`
- `crates/paged-text/src/compose.rs:276-316` — `ComposeOptions::new` and the calibration note ("50% match → 100% with the new ratios")
- `crates/paged-text/src/layout.rs:1285-1375` — `knuth_plass_breaks`: single-line, minimum raggedness, total-fit, the looser retries, fill glue, first-fit
- `crates/paged-text/src/single_line.rs:15-56` — the measured rules and how they were measured; "the `composer` fixture is the regression record"
- `crates/paged-text/src/ragged.rs:15-33` — the measurement on the `shaped-bands` fixture
- `crates/paged-renderer/src/pipeline/compose_opts.rs:143-171`, `crates/paged-renderer/src/pipeline/text_frame.rs:685-691` — where the renderer selects the single-line breaker and minimum raggedness
- `spikes/composer-calibration/sweep.sh:16-33` — the sweep over tolerance, stretch and shrink against `corpus/calibration/*.json` (six entries with `expected_lines`)

## Alternatives considered

- `rustybuzz`, removed in `19f0530`; it remains in `Cargo.lock` only as a dependency of `usvg`.
- Glue ratios of 1.0 / 0.5: "too permissive", costing line-break parity on the calibration
  corpus (`crates/paged-text/src/compose.rs:282-286`).
- For ragged text in shapes: minimising slack relative to the band's width, or "the cube
  of the slack per stretchable space as the breaker does", do not reproduce the fixture
  (`crates/paged-text/src/ragged.rs:31-33`).
- As the last resort, putting every box on its own line: replaced by first-fit because it
  "split a word at every hyphenation opportunity" (`crates/paged-text/src/first_fit.rs:23-30`).

## Consequences

Glyph positions are part of the display list, so a breaker change that moves a line
changes the digest ([ADR 101](101-display-list-single-intermediate.md)) of the pages it
touches. Positions stay in 1/64 pt until `crates/paged-compose/src/text.rs` divides by 64
at emission.

The single-line and minimum-raggedness rules were measured on InDesign 20.0.1 with one
typeface, Inter (`crates/paged-text/src/single_line.rs:18-19`,
`crates/paged-text/src/ragged.rs:22-24`). The World-Ready composers are treated as their
plain counterparts (`crates/paged-model/src/lib.rs:3172-3176`).

Documentation in the repo is behind the code. `README.md:30` and `:56` name `rustybuzz` as
the shaper. `crates/paged-text/src/lib.rs:20-25` mentions "Proximity if licensed", which no
code implements, and describes the calibration as happening "before this crate takes a hard
dependency on any specific penalty configuration"; the calibrated values are the defaults.

## Related

- [ADR 103](103-hyphenation-sources.md) — where hyphenation points come from
- [ADR 105](105-fidelity-gate.md), [ADR 120](120-indesign-is-the-oracle.md) — how the result is gated; the measuring method
