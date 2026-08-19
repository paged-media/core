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
# emits every known sample via `emit-all`.
#
# This script used to carry its own SAMPLES array, "kept in sync" with the
# match arms by hand. That guard only ever worked one way: an unknown NAME
# fails the build, but a sample left OUT of the list is silent. The editor
# kept a fourth copy of the same list and quietly lost `layers-z` and
# `paste-into`, so its CI never emitted them and four layers specs failed
# on a fixture that had never existed. There is one list now —
# paged_gen::samples::SAMPLES — and `emit-all` walks it.
set -euo pipefail

cargo build --release --bin paged-gen
./target/release/paged-gen emit-all >/dev/null
echo "regen-fixtures: emitted every paged-gen sample into corpus/generated/"
