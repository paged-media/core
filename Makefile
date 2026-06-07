# Makefile — the local Full-Green scoreboard for paged-media/core (W0.4).
#
#   make verify         # run every lane, print a PASS/FAIL/SKIP table,
#                        # exit nonzero on ANY FAIL (the local mirror of CI)
#
# Individual lanes (each runnable on its own; each appends one scoreboard
# row when invoked through `verify`, and runs verbatim when invoked direct):
#
#   make clippy         # cargo clippy --workspace --all-targets -D warnings
#   make fmt            # cargo fmt --all --check
#   make test           # cargo test --workspace
#   make check-wasm     # wasm32 build of the four wasm-target crates
#   make test-wasm      # headless wasm test lane (scripts/test-wasm.sh)
#   make bench          # W1.24 (B17) criterion perf benches. ADDITIVE —
#                        # NOT part of `verify`: benches profile, they
#                        # don't gate. `make bench-smoke` for a fast
#                        # compile+one-iteration check (what CI can run).
#   make fidelity       # the hard fidelity gate (delegates to diff.sh),
#                        # or SKIP with a reason when its deps are absent
#   make fidelity-deps  # the local-runnability doctor for the gate
#
# Plain Make + bash, no new tooling. Lane orchestration lives in
# scripts/verify-lane.sh (one row per lane) + scripts/verify-report.sh
# (table + exit code). `verify` runs each lane with `-` so one red lane
# never aborts the table; the final exit code comes from the scoreboard.
#
# `cargo fmt --all --check` IS kept clean since the one-time reformat
# (see .git-blame-ignore-revs). A red `fmt` lane means new drift — run
# `cargo fmt --all` before committing.

SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c

# wasm32 target + per-crate features (mirrors publish-wasm.yml):
#   canvas-wasm / sdk need --features gpu (Vello/WebGPU surface);
#   introspect-wasm / write build with defaults.
WASM_TARGET := wasm32-unknown-unknown

.PHONY: verify clippy fmt test check-wasm test-wasm fidelity fidelity-deps \
        bench bench-smoke \
        _board_init _board_report \
        _verify_clippy _verify_fmt _verify_test _verify_check_wasm \
        _verify_test_wasm _verify_fidelity

# --- aggregate ------------------------------------------------------------

# `verify` runs the lanes via the scoreboard helpers. Each lane recipe is
# prefixed with `-` so a FAIL records a row but does not abort the run;
# the real exit code is decided by _board_report scanning the board.
verify: _board_init
	-$(MAKE) --no-print-directory _verify_clippy
	-$(MAKE) --no-print-directory _verify_fmt
	-$(MAKE) --no-print-directory _verify_test
	-$(MAKE) --no-print-directory _verify_check_wasm
	-$(MAKE) --no-print-directory _verify_test_wasm
	-$(MAKE) --no-print-directory _verify_fidelity
	@$(MAKE) --no-print-directory _board_report

# A per-run scoreboard temp file, shared by every lane via the env var.
# Recreated at the start of each `verify`.
VERIFY_SCOREBOARD := $(CURDIR)/target/verify-scoreboard.tsv
export VERIFY_SCOREBOARD

_board_init:
	@mkdir -p $(CURDIR)/target
	@: > $(VERIFY_SCOREBOARD)

_board_report:
	@bash scripts/verify-report.sh

# --- lane wrappers (scoreboard rows) -------------------------------------

_verify_clippy:
	@bash scripts/verify-lane.sh clippy -- \
	  cargo clippy --workspace --all-targets -- -D warnings

_verify_fmt:
	@bash scripts/verify-lane.sh fmt -- \
	  cargo fmt --all --check

_verify_test:
	@bash scripts/verify-lane.sh test -- \
	  cargo test --workspace

_verify_check_wasm:
	@bash scripts/verify-lane.sh check-wasm -- \
	  $(MAKE) --no-print-directory check-wasm

_verify_test_wasm:
	@bash scripts/verify-lane.sh test-wasm -- \
	  $(MAKE) --no-print-directory test-wasm

# Fidelity is the delegating lane: run diff.sh IF its deps exist, else
# record SKIP with the doctor's reason (never FAIL for absent corpus).
_verify_fidelity:
	@if bash scripts/fidelity-deps.sh >/dev/null 2>&1; then \
	  bash scripts/verify-lane.sh fidelity -- ./corpus/generated/diff.sh; \
	else \
	  bash scripts/verify-lane.sh fidelity --skip \
	    "fidelity-gate deps absent (run \`make fidelity-deps\` for the checklist)"; \
	fi

# --- direct lanes (also usable standalone) -------------------------------

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all --check

test:
	cargo test --workspace

# wasm32 build check of the four wasm-target crates. canvas-wasm + sdk
# carry the Vello/WebGPU surface behind --features gpu; introspect-wasm +
# write build with defaults. `cargo check` (not build) — we gate that the
# wasm surfaces compile, not that they link an artifact.
check-wasm:
	rustup target add $(WASM_TARGET) >/dev/null 2>&1 || true
	cargo check --target $(WASM_TARGET) -p paged-canvas-wasm --features gpu
	cargo check --target $(WASM_TARGET) -p paged-introspect-wasm
	cargo check --target $(WASM_TARGET) -p paged-sdk --features gpu
	cargo check --target $(WASM_TARGET) -p paged-write

# Headless wasm test lane. scripts/test-wasm.sh is a STUB exiting 0
# (pending W0.8); the W0.8 crate-side work replaces it with the real
# runner. The CI job + this target are wired to it now so the lane exists.
test-wasm:
	bash scripts/test-wasm.sh

# The hard fidelity gate, run directly. Errors if deps are missing —
# use `make fidelity-deps` first (or rely on `make verify`, which SKIPs
# this lane gracefully when the deps are absent).
fidelity:
	./corpus/generated/diff.sh

# The local-runnability doctor: what the fidelity gate needs, and what's
# missing. Exit 0 = gate runnable; exit 1 = something to install/fetch.
fidelity-deps:
	bash scripts/fidelity-deps.sh

# W1.24 (audit B17) — criterion perf benches. Times the full pipeline
# (build_document), the mutation→rebuild round-trip, hit-test, and the
# display-list digest on regenerated paged-gen fixtures (text + tables).
# ADDITIVE: deliberately NOT a `verify` lane — benches are a profiling
# tool, not a pass/fail gate. See crates/paged-canvas/benches/pipeline.rs.
bench:
	cargo bench -p paged-canvas --bench pipeline

# Fast compile + single-iteration smoke of the bench lane (no timing
# statistics). This is the CI-affordable check that the benches still
# build and run; the full `make bench` is for local profiling.
bench-smoke:
	cargo bench -p paged-canvas --bench pipeline -- --test
