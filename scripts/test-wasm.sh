#!/usr/bin/env bash
# scripts/test-wasm.sh — wasm test lane entrypoint (STUB, pending W0.8).
#
# The substance of this lane (headless wasm-bindgen-test runs of the
# canvas-wasm / introspect-wasm / sdk surfaces against a browser or
# node+wasm runtime) is implemented under W0.8 by the crate-side work.
# Until that lands this is a deliberate pass-through stub so the CI job
# (`make test-wasm`) and the Makefile target are wired end-to-end and a
# green pipeline reflects "the lane exists and runs", not "the lane has
# substance".
#
# Contract for the W0.8 implementer who replaces this file:
#   - exit 0 on all-pass, nonzero on any failure (the CI job + `make
#     verify`'s test-wasm lane read the exit code).
#   - keep it plain bash; install hints belong in the workflow, not here.
#   - emit a one-line summary so the Makefile lane table stays legible.
#
# This stub intentionally exits 0 so the placeholder CI job is green; it
# does NOT assert any wasm behaviour yet.

set -euo pipefail

echo "test-wasm: STUB (pending W0.8) — no wasm behaviour asserted yet."
echo "test-wasm: the W0.8 crate-side work replaces scripts/test-wasm.sh with the real runner."
exit 0
