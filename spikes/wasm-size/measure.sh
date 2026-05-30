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
    RUSTFLAGS="-C opt-level=z -C lto=fat -C codegen-units=1 -C panic=abort -C strip=symbols" \
        cargo build --release \
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
    wasm-opt -Oz "$RAW" -o "$OPT"
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
