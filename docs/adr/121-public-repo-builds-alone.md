# ADR 121 — The public engine repo builds and gates without any private repo

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `scripts/`, `corpus/` (fonts, profiles, generated fixtures), `.github/workflows/`

## Context

This repository is public. A larger sample corpus and the project's status registry are kept in
private repositories. The scripts record two problems at that boundary.

The script that renders and diffs one fixture used to live in the private corpus repository, and
the gate skipped itself when the script was absent. A clean public checkout is that case, so the
step named "generated-corpus fidelity gate (hard)" reported success on both runners without
gating anything (`corpus/generated/render-diff.sh:7-19`).

The fonts are copies of faces that the private corpus also holds. The two sets drifted on the
first day: one face was added here alone, "and nothing said so"
(`scripts/check-font-hashes.sh:4-8`). The same script states the rule: "core is PUBLIC and must
build standalone", so it cannot depend on the private corpus repository.

## Decision

What the build, the tests and the gates need is committed in this repository or fetched from a
pinned public source; tools such as poppler and python3 come from the runner. The exceptions are
listed under Consequences. Four mechanisms apply the rule.

- **Fonts are copied and checked by manifest.** `corpus/fonts/` holds the engine's own copies of
  open-licence faces. `corpus/fonts/CANONICAL.sha256` is the private corpus's published hash list,
  committed verbatim; `scripts/check-font-hashes.sh` runs in CI and compares each face the list
  names with the local file of that name. A listed face absent here is accepted (a subset by
  design); differing bytes fail; a local face the list does not name is not examined. In the
  script's words: "The fix is a manifest, not a dependency."
- **Colour profiles are fetched, not committed.** `corpus/profiles/` ignores `*.icc`;
  `scripts/fetch-profiles.sh` downloads Ghostscript's default CMYK profile from a pinned release
  tag. Its stated reason: profile licences vary, and Adobe's CoatedFOGRA39 is not redistributable.
- **Fixtures are regenerated.** The `.idml` fixtures are ignored by git; CI runs
  `scripts/regen-fixtures.sh` (`paged-gen emit-all`) before the tests.
- **The fidelity engine is in the repository.** `corpus/generated/render-diff.sh` was moved here,
  and `corpus/generated/diff.sh` treats a missing engine as an error (exit 2), not as a skip.

Test evidence leaves as a build artifact. The `nextest` job uploads `nextest-junit`, and the
workflow note says the private side pulls it, because a public workflow cannot resolve an action
that lives in a private repository.

## Evidence

- `scripts/check-font-hashes.sh:4-19`, `:32-34`, `.github/workflows/ci.yml:50-58` — the rule, the
  manifest check, the subset allowance, the CI step
- `scripts/fetch-profiles.sh:15-19`, `:34-38`, `corpus/profiles/README.md:3-6` — why no profile is
  committed; the pinned source
- `scripts/regen-fixtures.sh:4-9`, `.github/workflows/ci.yml:60-65` — fixtures regenerated in CI
- `corpus/generated/render-diff.sh:7-19`, `corpus/generated/diff.sh:44-58` — the engine moved in;
  a missing engine is a broken checkout
- `.github/workflows/ci.yml:129-147` — the artifact upload and the note on the private action
- `Cargo.toml:71-72`, `deny.toml:59-81` — the git sources allowed, with the IDML adapter's
  repository described as public

## Alternatives considered

Depending on the private corpus: rejected for the fonts in the sentence quoted above, and what
the fidelity gate did until commit `bbda68a` (2026-08-19). Committing a colour profile: rejected
for licence reasons (`scripts/fetch-profiles.sh:15-19`). Publishing test results through the
private repository's action: not possible from a public workflow; commit `8498d53` dropped it.

## Consequences

When the private corpus changes a face, its manifest is copied here by hand
(`scripts/check-font-hashes.sh:17-19`). The build still needs two other repositories: Cargo fetches
the IDML adapter from `paged-media/plugin-publish` and a patched `flo_curves` from a fork, both
from git at pinned revisions (`Cargo.lock:1103`, `:1665`).

The InDesign references were exported in Coated FOGRA39, which is not redistributable. The
workflow comment says no runner has it; without it a failing fixture ends the gate step
"inconclusive" and the job continues (`.github/workflows/fidelity.yml:98-121`). The fetched
Ghostscript profile serves the colour tests and the PDF export diff, not this gate.

The rule is not applied everywhere. Nine variants of the `images` sample link a photo by an
absolute `file:` URI outside the repository (`crates/paged-gen/src/samples/images.rs:336-339`); the
file is not tracked. The contributor-agreement workflow records signatures in a separate
repository that its setup comment says must be private (`.github/workflows/cla.yml:5-13`); it is
not part of building or testing.
- Also outside the repository: an `#[ignore]` test reads a private corpus checkout beside this one
  when `PAGED_GRID_CORPUS` is set (`crates/paged-scene/tests/grid_matrix_corpus.rs:30-50`, `:112`);
  colour profiles are probed at absolute Adobe paths (`crates/paged-color/src/profiles.rs:154-158`,
  `crates/paged-export-pdf/tests/export_x4.rs:65`, `crates/paged-canvas/tests/export_pdf.rs:110`,
  `corpus/generated/render-diff.sh:186`, `corpus/generated/export-diff.sh:99`); secrets are used by
  the docs dispatch, `.github/workflows/notify-docs.yml` (`DOCS_DISPATCH_TOKEN`), and the release,
  `.github/workflows/publish-wasm.yml` (`NPM_TOKEN`).

Several texts are stale. `scripts/fidelity-deps.sh:7-8` and `:80` send the reader to the private
corpus for reference PDFs, which are committed. `.github/workflows/ci.yml:85-94` and
`.config/nextest.toml:3-4` describe publishing results through the private action. Line 146 of that
workflow calls the plugin repositories private; `Cargo.toml:71-72` calls the adapter's one public.

## Related

- [ADR 120](120-indesign-is-the-oracle.md), [ADR 105](105-fidelity-gate.md) — the references and the gate that this rule keeps runnable
- [ADR 109](109-engine-does-no-io.md) — the engine ships no fonts or profiles at run time either
- [ADR 650](https://github.com/paged-media/plugin-publish/blob/main/docs/adr/650-mutual-git-revision-pins.md) — the git dependency on the IDML adapter
- ADR 902 — the consuming side of the test-evidence artifact
