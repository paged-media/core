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
#  @copyright  Copyright (c) And The Next GmbH
#  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
#

# Spike C: WASM size measurement.
#
# Builds spike-wasm-size for wasm32-unknown-unknown with aggressive size
# optimisation, runs wasm-opt -Oz, compresses with brotli, and prints
# each artefact size. Pass criterion: compressed ≤ 3.5 MB.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT_DIR="$SCRIPT_DIR/out"
mkdir -p "$OUT_DIR"

echo "==> cargo build --release --target wasm32-unknown-unknown -p spike-wasm-size"
(
    cd "$WORKSPACE_ROOT"
    # LTO + panic MUST go through the Cargo PROFILE, not RUSTFLAGS. Setting
    # `-C lto=fat` in RUSTFLAGS applies it to every crate including rlib deps
    # (paged-sdk), which rustc rejects ("lto can only be run for executables,
    # cdylibs and static library outputs"); it also makes Cargo inject
    # `-C embed-bitcode=no`, incompatible with lto on rustc 1.94. Driving LTO
    # through the profile lets Cargo apply it only at the final cdylib link
    # and handle bitcode correctly. opt-level/codegen-units/strip stay in
    # RUSTFLAGS (no such conflict).
    RUSTFLAGS="-C opt-level=z -C codegen-units=1 -C strip=symbols" \
        cargo build --release \
        --config 'profile.release.lto="fat"' \
        --config 'profile.release.panic="abort"' \
        --target wasm32-unknown-unknown \
        -p spike-wasm-size
)

RAW="$WORKSPACE_ROOT/target/wasm32-unknown-unknown/release/spike_wasm_size.wasm"
OPT="$OUT_DIR/spike_wasm_size.opt.wasm"
BR="$OUT_DIR/spike_wasm_size.opt.wasm.br"

echo "==> wasm-opt -Oz"
if ! command -v wasm-opt >/dev/null 2>&1; then
    echo "wasm-opt not found; install binaryen to run the full pipeline." >&2
    cp "$RAW" "$OPT"
else
    # --all-features: wasm-bindgen emits reference-types/bulk-memory; a bare
    # `wasm-opt -Oz` fails to validate them ("error validating input"). Needs
    # a recent binaryen (the CI job pins one; apt's is too old).
    wasm-opt -Oz --all-features "$RAW" -o "$OPT"
fi

echo "==> brotli --best"
if ! command -v brotli >/dev/null 2>&1; then
    echo "brotli not found; skipping compression step." >&2
    cp "$OPT" "$BR"
else
    brotli --best --force --output="$BR" "$OPT"
fi

printf '\n==> sizes\n'
for f in "$RAW" "$OPT" "$BR"; do
    if [[ -f "$f" ]]; then
        printf '  %-50s %s\n' "$(basename "$f")" "$(du -h "$f" | cut -f1)"
    fi
done
