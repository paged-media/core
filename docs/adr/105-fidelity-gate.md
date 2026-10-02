# ADR 105 — The fidelity gate: CPU raster against a raster of InDesign's own PDF, thresholds that only tighten

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `corpus/generated/` (`diff.sh`, `render-diff.sh`, `fidelity-thresholds.json`, the committed reference PDFs), `crates/paged-fidelity`, `.github/workflows/fidelity.yml`

## Context

`CONTRIBUTING.md:38-39` states the rule: "Rendering changes are gated on visual fidelity
against InDesign-exported references." The measuring crate came first. `paged-fidelity` "is
built first (before the renderer itself) so every downstream change is measurable from day
one" (`crates/paged-fidelity/src/lib.rs:21-22`).

The gate's engine script once lived outside this repository, and the gate skipped itself
when the script was absent, which was every clean checkout. `corpus/generated/diff.sh:44-53`
records the result: the CI step named "(hard)" "had never gated anything here". The script
was moved in, next to the fixtures it runs against.

Two differences between the two rasters are not layout errors and had to be removed. A
CMYK profile mismatch adds about 4 ΔE to every solid CMYK fill
(`corpus/generated/render-diff.sh:163-184`). Poppler draws a glyph at a floored sub-pixel
phase, so a layout change below a pixel "reads as a regression"
(`crates/paged-gpu/src/lib.rs:130-140`).

## Decision

Rendering correctness is measured against InDesign's output, per page, with per-fixture
thresholds that are tightened after a fix and not loosened to pass a regression.

- **Fixture.** `paged-gen emit --sample <name>` writes the IDML on every run. The paired
  reference PDF, exported by InDesign, and its `*.export.meta.json` are committed.
- **Candidate and reference.** `paged-inspect --render` draws each page on the CPU
  rasteriser ([ADR 100](100-two-rasterisers-one-trait.md)) at 144 dpi. `pdftoppm` (poppler)
  rasterises the PDF at the same resolution.
- **Metric.** `paged-diff` reports mean ΔE2000, 99th-percentile ΔE2000 and SSIM over 8×8
  blocks for each page pair.
- **Thresholds.** `fidelity-thresholds.json` holds, per fixture, the worst page allowed:
  `max_mean_de`, `max_p99_de`, `min_ssim`. A page that misses any of the three fails the
  fixture. A fixture that compares zero pages fails.
- **The rule.** Thresholds are sized to the worst measured page plus about 15 to 25
  percent, and are "*tightened* (never loosened to mask a regression)"; "loosening any
  threshold needs a written rationale in the commit message".
- **Like for like.** Both sides use one CMYK profile: Coated FOGRA39 from the host's
  install, or the file named by `PAGED_CMYK_PROFILE`, which ranks above the document's
  declared profile and is also handed to `pdftoppm`. For generated fixtures the candidate's
  glyph origins are snapped to poppler's phase.
- **Outcomes.** Exit 0 passes, 1 is a regression, 2 is a broken setup (the thresholds
  file, the engine script, `pdftoppm` or `python3` is missing), and 3 is "inconclusive": a
  fixture failed in a run where no profile was available, so the two sides were in
  different colour spaces. `IDML_DIFF_GATE=advisory` turns 1 into 0.

## Evidence

- `corpus/generated/diff.sh:4-16` — the five steps; `:143-155` — zero gated pages fail; `:157-170` — the three comparisons; `:43-60` — exit 2; `:213-240` — exits 0, 3 and 1
- `corpus/generated/render-diff.sh:55`, `:110-119`, `:141-150`, `:185-216` — 144 dpi, glyph snapping, the `paged-inspect` call, the profile handed to `pdftoppm`
- `corpus/generated/fidelity-thresholds.json:7-28` — semantics, calibration, the tightening rule, the long-term target; 58 fixture entries, each with a `rationale`
- `crates/paged-fidelity/src/diff.rs:39-72`, `crates/paged-fidelity/src/ssim.rs:15-20` — the metric
- `crates/paged-color/src/profiles.rs:51-68` — why the environment variable outranks the declared profile: "Both halves now read the same variable, so both move together"
- `.github/workflows/fidelity.yml:3-9`, `:34-35`, `:90-121` — runs on pushes to `main` and on pull requests touching `crates/**` or `corpus/**`, on Ubuntu and macOS
- `CLAUDE.md:126-129`, `CONTRIBUTING.md:46-48` — the no-loosening rule as a contributor rule

## Alternatives considered

Comparisons against the engine's own earlier output exist beside the gate and are kept
apart from it: a golden-snapshot test over one seed fixture
(`.github/workflows/fidelity.yml:84-88`), and the PDF exporter's self-consistency lane,
described as "NOT the InDesign fidelity gate" (`.github/workflows/ci.yml:338-339`).

`paged-fidelity` defines one global pass criterion (mean ≤ 1.0, p99 ≤ 2.5, SSIM ≥ 0.99;
`crates/paged-fidelity/src/lib.rs:28-31`). The gate does not apply it; the thresholds file
names it as the long-term goal. The crate doc names Ghostscript and CoreGraphics as
reference rasterisers (`crates/paged-fidelity/src/lib.rs:17`); the gate uses `pdftoppm`.

## Consequences

The reference is poppler's raster of InDesign's PDF, not InDesign's own screen or print
raster. A new or corrected reference needs InDesign (`tools/indesign-export/README.md:10-14`).

The workflow does not give its runners a CMYK profile: it sets no `PAGED_CMYK_PROFILE`,
and its comment says the references were exported in Coated FOGRA39 and "no runner has
that profile" (`.github/workflows/fidelity.yml:98-111`). On a runner without a profile
`diff.sh` exits 3, not 1, whenever a fixture fails (`corpus/generated/diff.sh:213-240`), and
the step reports exit 3 as a warning and exits 0 (`.github/workflows/fidelity.yml:112-121`).
A failing fixture fails the job only where the Adobe-installed profile exists or
`PAGED_CMYK_PROFILE` names a profile file.

At this commit 46 of the 58 fixtures have at least one threshold looser than the long-term
target. Only the first `max_pages_with_pdf` pages of a fixture are gated
(`corpus/generated/fidelity-thresholds.json:10-12`). The Vello lane is outside this gate
([ADR 100](100-two-rasterisers-one-trait.md)).

## Related

- [ADR 120](120-indesign-is-the-oracle.md), [ADR 100](100-two-rasterisers-one-trait.md) — the generator, the InDesign export and the probes; why the CPU rasteriser is the gated lane
- [ADR 106](106-colour-resolved-at-build-time.md), [ADR 121](121-public-repo-builds-alone.md) — the colour path the profile feeds; the gate running without any private repository
