# Contributing to paged

Thanks for your interest in the **paged** engine. This repository is the
**open** render pipeline (MPL-2.0 OR PMEL); the editor and plugins live in
separate, also-open repositories — the editor (`paged-media/editor`) is
AGPL-3.0 OR PMEL.

## License of contributions

`paged` is dual-licensed — **MPL-2.0 OR the Paged Media Enterprise License
(PMEL)**. By contributing you agree to the **Contributor License
Agreement** ([`CLA.md`](./CLA.md)), which allows And The Next GmbH to
distribute your contribution under **both** the open-source license
(MPL-2.0) **and** the commercial license (PMEL). You retain copyright to
your contribution.

A CLA bot will ask you to sign on your first pull request.

New source files must carry the standard MPL-2.0 header — copy it verbatim
from the top of any existing `crates/**/*.rs`.

## Building & testing

The toolchain is pinned in `rust-toolchain.toml`.

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Do **not** run `cargo fmt --all` across the whole workspace — it produces
large drifts on unrelated files. Format only the files you touched.

## Fidelity gate

Rendering changes are gated on visual fidelity against InDesign-exported
references. The **public golden set** lives in `corpus/generated/` and is
all you need to pass CI:

```bash
./corpus/generated/diff.sh   # needs pdftoppm (poppler-utils)
```

Don't loosen the per-fixture thresholds in
`corpus/generated/fidelity-thresholds.json` to make a regression pass —
fix the regression first, then tighten the threshold.

## Dependency licensing

CI runs `cargo deny check` (see [`deny.toml`](./deny.toml)). New
dependencies must be permissively licensed so they can be combined into an
MPL distribution.

## Scope note

The full Envato-backed fidelity corpus and the InDesign export harness are
internal (private) — you do **not** need them to contribute. The public
golden set above is sufficient.
