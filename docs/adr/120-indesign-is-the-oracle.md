# ADR 120 — InDesign is the oracle: a generator authors, InDesign answers, a diff gates

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-gen`, `tools/indesign-export`, `corpus/generated`, and the measurement
  notes in `crates/paged-text`, `crates/paged-compose` and `crates/paged-renderer`

## Context

The repository describes itself as "Pixel-faithful Adobe IDML rendering" (`README.md:3`), and
rendering changes are gated against references that InDesign exported (`CONTRIBUTING.md:38-40`).
The reference set is generated and committed as a licence-clear golden set so that outside
contributors can run the gate (`README.md:197-199`); third-party samples are not cleared for
redistribution and run only as an advisory step (`.github/workflows/fidelity.yml:134-141`).

The generator's module comment gives the reason for the shape of the set: each document is a
multi-page file with one feature variant per page, so one InDesign export covers many test cases
and a failure is attributed to a page by its name (`crates/paged-gen/src/lib.rs:17-21`). The
introducing commit (`e54a485`) sets this against "600+ atomic files". The comment points to
`docs/idml-sample-generator.md` for the wider argument; that file is not in this repository.

Two comments record what an unmeasured rule cost: shadow offsets negated on both axes, unseen
because the fixtures carried explicit offsets (`crates/paged-renderer/src/module/effects.rs:240-245`),
and an effect's `Size` used as a Gaussian sigma when it is the width of the effect's band
(`crates/paged-compose/src/mask.rs:634-647`).

## Decision

InDesign's own output is the reference for the engine's behaviour. It is obtained in three steps.

- **A generator authors.** `paged-gen` writes the IDML sample documents (59 built-in samples).
  Its package writer is byte-deterministic (fixed timestamp, stable entry order) and a hash test
  checks that. The library depends on `zip` and `quick-xml`, not on the engine's model or
  importer; the importer is a dev-dependency. Generated `.idml` files are not committed.
- **InDesign answers.** `tools/indesign-export/run-export.sh` drives a local InDesign through
  `export-pdfs.jsx`: open each IDML, export a PDF with the High Quality Print preset, write
  `<stem>.export.meta.json` (InDesign version, preset, time). Both files are committed. Where an
  export of the unedited file cannot show the behaviour, a probe script has InDesign act and report
  JSON: `reflow-probe` (Smart Text Reflow after an edit) and `inline-objects-probe` (line
  baselines and object bounds read from the DOM).
- **A diff gates.** `corpus/generated/diff.sh` regenerates each IDML, renders it, rasterises the
  paired PDF and compares each page against per-fixture thresholds ([ADR 105](105-fidelity-gate.md)).
  Probe answers are pinned by Rust tests; a measured rule carries its measurement in a doc comment.

## Evidence

- `crates/paged-gen/src/samples.rs:89-149`, `:238-245` — the sample list; its length pinned at 59
- `crates/paged-gen/src/package.rs:25-33`, `crates/paged-gen/tests/snapshot.rs:41-46`,
  `crates/paged-gen/Cargo.toml:13-22` — determinism, its test, and the dependencies
- `tools/indesign-export/export-pdfs.jsx:113-138`, `corpus/generated/.gitignore:1-4` — the export
  loop and its metadata record; IDMLs ignored, PDFs committed
- `corpus/generated/diff.sh:6-16` — the five steps of the gate
- `tools/indesign-export/reflow-probe.jsx:1-14`,
  `tools/indesign-export/inline-objects-probe.jsx:15-21` — the two probes;
  `crates/paged-renderer/tests/reflow_pipeline.rs:15-24`,
  `crates/paged-renderer/tests/inline_objects_pipeline.rs:15-18` — tests that pin their answers
- `crates/paged-text/src/single_line.rs:18-21`, `crates/paged-renderer/src/pipeline/keeps.rs:24-25`
  — measurement notes that name the fixture holding the rule

## Alternatives considered

One small file per case, and third-party samples as the gate: both set aside for the reasons
above. `corpus/generated/breaks-diff.sh:7-10` and `corpus/generated/export-diff.sh:4-9` complement
the gate and say they do not replace it ("This is not the InDesign fidelity gate").

## Consequences

The gate runs wherever the Rust toolchain, poppler and python3 are installed
(`corpus/generated/diff.sh:59-60`). Adding or re-measuring a reference needs InDesign, and the
shell drivers are macOS-only (`tools/indesign-export/run-export.sh:19`). All 58 committed
references record InDesign 20.0.1.32; the rule is to re-export one fixture at a time and to pin
the version (`tools/indesign-export/README.md:56-67`). A reference also depends on the exporting machine's
fonts (`CLAUDE.md:132-136`) and on a colour profile that may not be redistributed
(`.github/workflows/fidelity.yml:98-101`; [ADR 121](121-public-repo-builds-alone.md)).

Coverage is not complete. The sample `docx-pagination` has no reference PDF and no entry in
`corpus/generated/fidelity-thresholds.json`; a fixture gates only its first `max_pages_with_pdf`
pages (lines 10-12 of that file). The effect measurements name probes
(`crates/paged-compose/src/mask.rs:1013`) that are not in `tools/indesign-export/`, which holds
the export driver and two probes, so they cannot be repeated from this repository.
`README.md:199-200` and `CONTRIBUTING.md:58-59` call the InDesign export harness internal and
private; it is `tools/indesign-export/`. `README.md:73` and `CLAUDE.md:120` call it
"Python/ExtendScript"; it is bash and ExtendScript. Comments citing "thoughts ADR 026" and
"thoughts ADR 028" mean the two records linked below.

## Related

- [ADR 105](105-fidelity-gate.md) — metrics, thresholds and the rasteriser the gate uses
- [ADR 102](102-text-stack.md), [ADR 104](104-effects-follow-indesign-parameters.md) — rules obtained this way
- [ADR 026](026-auto-growing-region-chains.md), [ADR 028](028-pagination-rules-are-engine-owned.md) — rules measured with the reflow probe and the `keeps` fixture
- [ADR 653](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/653-indesign-is-the-oracle.md) — InDesign as the oracle for the IDML adapter
