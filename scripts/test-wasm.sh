#!/usr/bin/env bash
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
#
# This file is part of paged (https://paged.media) and is additionally
# available under the Paged Media Enterprise License (PMEL). Full
# copyright and license information is available in LICENSE.md which is
# distributed with this source code.
#
#   @copyright  Copyright (c) And The Next GmbH
#   @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
#
# Run the paged-canvas-wasm `wasm_bindgen_test` lane headless (audit
# B11/B13). This exercises the wasm-only surface — `CanvasWorker`
# construction, `handleMessage` round-trips, and the `js_sys::Date`
# timing path — that `cargo test` on native can't reach.
#
# Transport: the Node-hosted `wasm-bindgen-test-runner`. The tests use no
# DOM (no `run_in_browser`), so Node is sufficient and avoids needing a
# headless browser + chromedriver in CI. To force the browser instead,
# set PAGED_WASM_BROWSER=1 (requires chromedriver on PATH).
#
# Dependencies (already present in a standard wasm-bindgen-cli + rustup
# wasm setup):
#   - rustup target wasm32-unknown-unknown
#   - wasm-bindgen-test-runner (from the wasm-bindgen-cli crate; its
#     version MUST match the `wasm-bindgen` dep — 0.2.x — or the runner
#     rejects the module. See MEMORY: keep binaryen/wasm-bindgen aligned.)
#   - node (for the default Node transport)
#
# Exit code: 0 on all-pass, nonzero on any failure. This is what
# `make test-wasm` calls.

set -euo pipefail

# Resolve the repo root (this script lives in <root>/scripts).
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TARGET="wasm32-unknown-unknown"
CRATE="paged-canvas-wasm"

# 1. The wasm target must be installed.
if ! rustup target list --installed 2>/dev/null | grep -q "$TARGET"; then
  echo "test-wasm: error — rust target $TARGET is not installed."
  echo "           run: rustup target add $TARGET"
  exit 1
fi

# 2. The test runner must be on PATH. cargo invokes it via the
#    CARGO_TARGET_*_RUNNER env var below; surface a clear message if it
#    is missing rather than a cryptic cargo error.
if ! command -v wasm-bindgen-test-runner >/dev/null 2>&1; then
  echo "test-wasm: error — wasm-bindgen-test-runner not found on PATH."
  echo "           install it (version-matched to the wasm-bindgen dep):"
  echo "             cargo install wasm-bindgen-cli --version 0.2.122"
  exit 1
fi

# 3. Pick the transport.
if [[ "${PAGED_WASM_BROWSER:-0}" == "1" ]]; then
  # Browser mode: the runner drives a headless Chrome via chromedriver.
  # Requires chromedriver on PATH. Documented for parity with
  # `wasm-pack test --headless --chrome`; the default Node transport is
  # what CI uses (no browser dependency).
  if ! command -v chromedriver >/dev/null 2>&1; then
    echo "test-wasm: error — PAGED_WASM_BROWSER=1 but chromedriver not on PATH."
    exit 1
  fi
  echo "test-wasm: browser transport (headless chrome via chromedriver)"
else
  echo "test-wasm: node transport (headless)"
fi

# Point cargo's wasm32 test runner at wasm-bindgen-test-runner. Cargo
# reads CARGO_TARGET_<TARGET>_RUNNER (target triple upper-cased, dashes →
# underscores) to know how to "run" the produced .wasm test binary; the
# runner then hosts it under Node (default) or a browser.
export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER="wasm-bindgen-test-runner"

echo "test-wasm: cargo test --target $TARGET -p $CRATE --test wasm"
# `--test wasm` scopes to the wasm_bindgen_test lane (tests/wasm.rs); the
# native dispatch suite is covered by plain `cargo test`.
cargo test --target "$TARGET" -p "$CRATE" --test wasm "$@"

echo "test-wasm: PASS — paged-canvas-wasm wasm_bindgen_test lane green."
