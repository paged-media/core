# ADR 110 — One undo timeline, by pre-captured inverses

- **Status:** Accepted. Recorded retroactively on 2026-10-02 from the code at `9f933f1`.
- **Scope:** `crates/paged-canvas/src/model.rs` (`applied_log`, `redo_log`, `LoggedMutation`), `crates/paged-canvas/src/mutate.rs` (`TextOp`), `crates/paged-mutate` (`AppliedOperation`, `invert.rs`)

## Context

The canvas model edits its scene through two separate lanes. Frame, style and structure
edits go through `paged_mutate::apply`, which returns an `AppliedOperation` holding the
operation and its inverse. Text insertion and deletion go through `TextOp`, a second
vocabulary that lives in `paged-canvas`. The module doc says why it sits there: so that
work then in flight on `paged-mutate::Operation` could "land without conflicts", with the
text variants to be folded into `paged_mutate::Operation` afterwards
(`crates/paged-canvas/src/mutate.rs:27-31`). That folding has not happened: text edits
"have no `Operation` form" (`crates/paged-canvas/src/model.rs:2310-2311`).

A user has one undo command. The log entry type was "generalized to hold both text edits
(legacy `TextOp` path) and frame mutations (canonical `paged_mutate::AppliedOperation`) so
a single Cmd-Z timeline covers both" (`crates/paged-canvas/src/model.rs:1422-1424`).

Undo is a log of inverses, not of document snapshots. The repository does not record why.

## Decision

The canvas model keeps one undo log and one redo stack for both lanes. Every entry carries
the forward operation and the inverse that was captured when it was applied.

- `applied_log: Vec<AppliedRecord>` is the undo stack; `redo_log` is filled by `undo` and
  cleared when a new mutation is applied.
- An entry is one of four kinds: `Text { op, inverse }`, `Frame(AppliedOperation)`,
  `Composite(Vec<LoggedMutation>)`, and `Defaults { prev, next }`.
- `undo` pops the newest entry, applies its inverse through the entry's own lane, and
  rebuilds. `redo` applies the forward operation again and captures the inverse afresh.
- A batch is one entry. A batch whose children all translate is one `Frame` record holding
  an `Operation::Batch`; a batch that spans both lanes is one `Composite`, undone by
  reverting its children newest first.
- A committed gesture is one entry: the preview is reverted and the final change is
  applied through `paged_mutate::apply`.
- The log is capped at `MAX_APPLIED_LOG` = 10,000 entries; the oldest are evicted first.
  The redo stack has no separate cap.
- The log is the undo stack and nothing else. Its doc comment says it is "NOT the save-back
  source" and "NOT the determinism replay source".
- A top-level `SetDocumentDefaults` or `SetColorSettings` writes no entry and cannot be
  undone. Inside a batch, `SetDocumentDefaults` logs a `Defaults` record so that the batch
  stays all-or-nothing.

## Evidence

- `crates/paged-canvas/src/model.rs:1244-1250` — `applied_log` and `redo_log`
- `crates/paged-canvas/src/model.rs:1395-1418` — the cap, its reasoning, and what the log is not
- `crates/paged-canvas/src/model.rs:1476-1507` — `LoggedMutation` and its four variants
- `crates/paged-canvas/src/model.rs:4156-4273` — `undo` and `redo`
- `crates/paged-canvas/src/model.rs:9199-9219` — `push_applied`, the one place the cap is enforced
- `crates/paged-canvas/src/model.rs:1898-1903`, `:1928-1932` — defaults and colour settings bypass the log
- `crates/paged-canvas/src/gesture.rs:591-598` — a gesture commits as one canonical entry
- `crates/paged-mutate/src/invert.rs:15-29` — the inverse of each operation shape

## Alternatives considered

- Folding `TextOp` into `paged_mutate::Operation`: stated as the plan in
  `crates/paged-canvas/src/mutate.rs:29-31` and marked "**out of scope**" at
  `crates/paged-canvas/src/model.rs:1424-1426`. Not done.
- A byte-accounted cap: rejected for a count, because "the per-entry size is bounded in
  practice and a count is O(1) to enforce without walking the payloads"
  (`crates/paged-canvas/src/model.rs:1415-1417`).

## Consequences

A batch has to be applied across both lanes, which is why the mixed-batch executor exists
([ADR 107](107-whole-document-build.md)). Scripts call the model's undo directly
(`paged.undo`, `crates/paged-script/src/lib.rs:3687-3691`); the CLI has no undo verb
(`crates/paged-cli/tests/cli_surface.rs:98-101`). Undo past 10,000 steps is not possible;
the comment calls this "a deliberate, documented trade most editors make".

There is a second undo implementation in the repository. `paged_mutate::Project` owns a
`History` with a default capacity of 1,000 entries
(`crates/paged-mutate/src/history.rs:15-26`, `crates/paged-mutate/src/lib.rs:73-83`). The
inspector wasm (`crates/paged-introspect-wasm/src/lib.rs:47-81`), the `paged-inspect`
binary and tests use it. The canvas model, the dispatcher, the script bridge and the CLI do
not, so the two histories never meet.

The module doc at `crates/paged-canvas/src/mutate.rs:19-20` lists three text mutations
including `ApplyTextStyle`; the `TextOp` enum has two, `InsertText` and `DeleteRange`.

## Related

- [ADR 005](005-wire-recipe.md) — every operation is invertible by construction
- [ADR 107](107-whole-document-build.md) — the batch as the unit of rebuild and of undo
- [ADR 116](116-mutations-lowered-onto-operations.md) — how a wire mutation becomes an `Operation`
- [ADR 012](https://github.com/paged-media/plugin-sdk/blob/main/docs/adr/012-k1-modal-session-undo-coalescing.md) — undo coalescing for plugin modal sessions
