#!/usr/bin/env bash
# Regenerate the gitignored corpus/generated/*.idml fixtures.
#
# These fixtures are deterministic outputs of `paged-gen` (gitignored by
# corpus/generated/.gitignore because they reproduce from source). A
# handful of tests read them at runtime — paged-canvas/tests/inspector_wire.rs,
# the round-trip + conformance lanes — and panic with "read fixture:
# NotFound" if they're absent. Local dev regenerates ad hoc; CI must run
# this before `cargo test` / `cargo nextest`, or those tests fail spuriously.
#
# Idempotent: re-emitting overwrites. Builds paged-gen once (release) and
# emits every known sample. Keep SAMPLES in sync with the match arms in
# crates/paged-gen/src/bin/paged-gen.rs (the build fails loudly on an
# unknown name, so drift surfaces immediately).
set -euo pipefail

SAMPLES=(
  geometry geometry-groups strokes-fills text text-advanced text-autosize
  text-letterspacing text-on-path text-overset text-in-shape text-wrap
  effects footnotes gradients tables images image-clipping anchored
  transparency markers masters corners links-broken links-ok preflight
  numbering variables conditions swatches navigation styles-cascade layout
  nested-groups
)

cargo build --release --bin paged-gen
for s in "${SAMPLES[@]}"; do
  ./target/release/paged-gen emit --sample "$s" >/dev/null
done
echo "regen-fixtures: emitted ${#SAMPLES[@]} samples into corpus/generated/"
