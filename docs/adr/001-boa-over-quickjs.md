# ADR 001 — Scripting runtime: Boa, reversing the QuickJS decision

**2026-06-07 · decision record · status: ACCEPTED (records a reversal already
shipped in code).**

**Sources:** `crates/paged-script/Cargo.toml:13` (`boa_engine = "0.21"`,
`default-features = false`); `crates/paged-script/src/lib.rs:26` (comment: *"the
previous rquickjs/QuickJS-in-C path"*); the original scripting-layer design
document, §"Runtime Choice" + §"Why not Boa" (the reversed decision) + its
2026-06-07 Status block; the plugin trust-gate checklist
(`plugin-sdk: docs/reference/trust-gate.md`).

## The decision

**The embedded JS scripting runtime is Boa (`boa_engine`, pure-Rust), reversing
the original scripting-layer design's explicit "QuickJS via `rquickjs`, *not* Boa" choice.**
That document argued Boa was "interesting in two or three years; QuickJS is the
right choice today." Code did the exact opposite, and ships Boa.

## Why (reconstructed rationale)

The original doc never recorded the *why* of the reversal — this is a
**reconstructed rationale**, inferred from the code comment at
`crates/paged-script/src/lib.rs:26` and the toolchain facts, not from a prior written
decision:

- **wasm toolchain simplicity beat raw speed.** Boa compiles to
  `wasm32-unknown-unknown` as a single Rust module with no libc sysroot, no
  wasm-capable clang, and no WASI polyfill. `rquickjs`/QuickJS-in-C required all
  of those — a C cross-compilation lane inside an otherwise pure-Rust engine
  that already targets wasm. Embedding Boa in the editor worker is one more Rust
  crate; embedding QuickJS-in-wasm was a second build system.
- The original scripting-layer design's thesis ("the engine is secondary to the Operation
  unification") held — so trading QuickJS's speed/ES-compliance lead for
  build-graph simplicity was acceptable, because the runtime was never the
  load-bearing decision.

## Consequences

- **No per-plugin isolate primitive.** QuickJS gives a cheap context-per-script
  isolate; stock Boa does not. The plugin-security isolate story that
  the original scripting-layer design (§10/§11) leaned on must be re-derived as a
  **Boa-context-per-plugin or, more likely, worker-per-plugin** boundary. This
  is the open structural gate in the plugin trust-gate checklist
  (`plugin-sdk: docs/reference/trust-gate.md`).
- **Sandbox is a metering ceiling, not a hard isolate.** The realized sandbox is
  the **RuntimeLimits budget surface** (`ScriptBudget { loop_iterations,
  recursion_depth, stack_size, wall_clock_ms }`, `crates/paged-script/src/lib.rs:132`),
  mapping onto Boa's `RuntimeLimits` plus a bridge-level wall-clock deadline
  checked at host-call boundaries. Runaway scripts ARE killed (the runtime-budget
  item the crate's comments call B-09 is RESOLVED).
- **Still not built:** per-context memory cap (stock Boa has none) and true
  preemption of a host-call-free busy loop (needs worker termination). These are
  the budget surface's stated ceilings, tracked in the plugin trust-gate
  checklist.
- The Proxy-based `SetProperty` mechanism (the original scripting-layer design,
  §"Write access") is engine-agnostic JS; it does not depend on the rejected engine.

## Doc follow-up

The original scripting-layer design document carries an in-place Status block
recording this reversal; the QuickJS→Boa noun sweep across the other internal
design documents is the separate doc-maintenance task, not this ADR.

## Amendment — 2026-10-02

Checked against the code at `9f933f1`. The decision stands: `boa_engine` 0.21 is the only
script runtime (`crates/paged-script/Cargo.toml:13`), and the comment cited in **Sources** is
still at `crates/paged-script/src/lib.rs:26`. The second Consequences bullet describes the
budget surface; two parts of it are not described there.

**1. The budget is a wire parameter (protocol 63).** A host sets the ceilings per run in the
`ExecuteScript` message instead of taking the crate's defaults.

- `crates/paged-script/src/lib.rs:146-154` — `ScriptBudget`, the struct the ADR cites at
  `:132`; the defaults are at `:160-174` (10,000,000 loop iterations, recursion depth 512, a
  2,000 ms wall clock).
- `crates/paged-canvas/src/channel.rs:512-534` — `ScriptBudgetWire`: four optional fields;
  `wall_clock_ms` of `0` disables the deadline.
- `crates/paged-canvas/src/channel.rs:1170-1176` — `ExecuteScript { source, budget }`; an
  absent `budget` means the engine defaults.
- `crates/paged-canvas-wasm/src/dispatch.rs:1204-1220` — the dispatcher starts from
  `ScriptBudget::default()`, overrides only the fields the message names, and calls
  `execute_script_with`.
- `crates/paged-canvas/src/channel.rs:491-502` — the reason the comment gives: the command
  line needed a longer wall clock than the editor's REPL and had been calling
  `execute_script_with` directly, "a second door, for a parameter".
- `crates/paged-cli/src/script.rs:86-98` — `paged script` builds its budget from
  `--timeout` and `--max-loop-iterations` and sends it in the message.

**2. Exhaustion is a typed outcome.** A script stopped by a budget is reported with the budget
that stopped it, separately from an ordinary script error.

- `crates/paged-script/src/lib.rs:73-90` — `ScriptBudgetKind { Iterations, Recursion,
  StackSize, WallClock }`.
- `crates/paged-script/src/lib.rs:188-198` — `ScriptResult.budget_kind`, set together with
  `error` when the abort was a budget; `:408-430` recovers the kind from Boa's message text
  and the crate's own wall-clock sentinel (`:237`).
- `crates/paged-canvas/src/channel.rs:1309-1321`, `:1851-1856` — the wire mirror of the enum
  and the optional `budget_kind` field on the `ScriptResult` reply;
  `crates/paged-canvas-wasm/src/dispatch.rs:49-60` maps one onto the other.

The preemption limit in the third Consequences bullet still holds: the wall clock is checked
only at the entry of a host function, so a loop that makes no host call is bounded by
`loop_iterations` alone (`crates/paged-script/src/lib.rs:119-144`, `:245-259`).
