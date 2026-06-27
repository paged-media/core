/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! Scripting Stage 2 — embedded Boa (pure-Rust JS) bridge.
//!
//! Hosts a Boa JS context inside the canvas worker so user scripts
//! can mutate the document through the same Operation channel the
//! Inspector + REPL already use. Per `docs/paged/scripting-layer.md`
//! every write goes through `paged_mutate::apply`; the host functions
//! installed here are the only path JS can take to reach it.
//!
//! Boa is pure Rust, so the wasm build needs nothing more than
//! `cargo build --target wasm32-unknown-unknown` — no libc sysroot,
//! no wasm-capable clang, no WASI polyfill (which were all required
//! by the previous rquickjs/QuickJS-in-C path).
//!
//! v1 surface (function-style + Proxy sugar via the bootstrap JS):
//!   paged.set(idStr, pathStr, value)
//!   paged.get(idStr, pathStr) -> value | null
//!   paged.insertText(storyId, offset, text) -> bool       (Stage 1 authoring)
//!   paged.deleteRange(storyId, start, end) -> bool
//!   paged.insertTextFrame(pageId, [t,l,b,r]) -> bool
//!   paged.insertFrame(pageId, [t,l,b,r]) -> bool           (Stage 2 authoring)
//!   paged.insertPage(afterPageId?) -> bool
//!   paged.placeImage(frameId, uri, fit?) -> bool
//!   paged.applyStyle(storyId, start, end, styleRef) -> bool
//!   paged.createGroup([id, ...]) -> bool
//!   paged.inspect(idStr) -> ElementProperties JSON
//!   paged.layers() -> LayerSummary[]
//!   paged.tree() -> SceneTreeNode[]
//!   paged.selection() -> ElementId[] JSON (current element selection)
//!   paged.contentSelection() -> ContentSelection JSON | null
//!   paged.undo() / paged.redo()
//!   paged.frame(idStr) -> Proxy whose `prop = value` writes go
//!                          through paged.set
//!   console.log(...) -> captured into the output log

use std::cell::Cell;
use std::cell::RefCell;

use boa_engine::{
    js_string, object::ObjectInitializer, property::Attribute, Context, JsArgs, JsNativeError,
    JsResult, JsValue, NativeFunction, Source,
};
use paged_canvas::channel::{CollectionName, Mutation};
use paged_canvas::CanvasModel;
use paged_canvas::PageId;
use serde::{Deserialize, Serialize};

// The capability catalog is owned by `paged-introspect` (the neutral, published
// home — ADR 019); re-exported here so existing consumers (`paged-run`) and the
// Boa bridge keep referencing it as before.
pub use paged_introspect::catalog::{api_catalog, ApiCatalog};

/// Which runtime budget a script exhausted. Surfaced as the typed
/// half of a `ScriptResult` so the host (editor REPL, plugin runner,
/// headless conformance) can distinguish a *budget* abort from an
/// ordinary script exception and react accordingly (e.g. show a
/// "script hit its time/iteration limit" banner instead of a generic
/// error). B-09 / W-08: this is the typed-exhaustion contract the
/// plugin repos asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScriptBudgetKind {
    /// `loop_iteration_limit` tripped — a runaway / pathologically
    /// long pure-JS loop. Enforced natively by Boa's bytecode loop
    /// opcode.
    Iterations,
    /// `recursion_limit` tripped — unbounded / too-deep recursion.
    Recursion,
    /// `stack_size_limit` tripped — VM value-stack overflow guard.
    StackSize,
    /// The wall-clock deadline elapsed while the script was inside a
    /// host call (`paged.*` / `console.*`). The single-threaded
    /// wasm worker cannot preempt a pure-JS loop, so this fires at
    /// the next host-function boundary — see the `ScriptBudget`
    /// wall-clock note.
    WallClock,
}

impl ScriptBudgetKind {
    /// Stable lower-case tag for log lines and message matching.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ScriptBudgetKind::Iterations => "iterations",
            ScriptBudgetKind::Recursion => "recursion",
            ScriptBudgetKind::StackSize => "stackSize",
            ScriptBudgetKind::WallClock => "wallClock",
        }
    }
}

/// Per-execution runtime budget. Hosts tighten or loosen this through
/// `execute_script_with`; `execute_script` uses `Default`. Every field
/// maps onto a concrete Boa guard or the bridge's wall-clock deadline:
///
/// - `loop_iterations` → `RuntimeLimits::set_loop_iteration_limit`
///   (kills `while (true) {}` and friends; generous enough that a
///   script touching every element of a large document never trips
///   it, tight enough that an infinite loop dies promptly).
/// - `recursion_depth` → `RuntimeLimits::set_recursion_limit`.
/// - `stack_size`      → `RuntimeLimits::set_stack_size_limit`.
/// - `wall_clock_ms`   → the bridge-level deadline (this crate; Boa
///   has no native instruction-level wall-clock interrupt in its
///   synchronous run loop — see the wall-clock note below).
///
/// ## Wall-clock guarantee (and the wasm limitation, stated precisely)
///
/// Boa 0.21 runs scripts to completion **synchronously** inside
/// `Context::eval`. Its bytecode loop consults no clock and exposes no
/// host interrupt hook on the synchronous path (the only instruction
/// counter, `instructions_remaining`, is `#[cfg(feature = "fuzz")]`
/// only). The canvas worker is single-threaded wasm, so there is no
/// second thread that could set an interrupt flag mid-loop. We
/// therefore CANNOT preempt an arbitrary pure-JS busy loop on the wall
/// clock — that class is bounded only by `loop_iterations`.
///
/// What `wall_clock_ms` *does* guarantee: the deadline is checked at
/// the entry of **every** `paged.*` / `console.*` host function (the
/// only way a script reaches the document, the only way it can block
/// on a slow native, and the cadence at which any host-call-driven
/// loop ticks). So a script blocked in a long native call CHAIN, a
/// pathological per-iteration host call, or a loop that does any work
/// through the bridge terminates within one host call of the deadline,
/// with a typed `WallClock` exhaustion. A breach raises Boa's
/// **non-catchable** `RuntimeLimit` error, so user `try/catch` cannot
/// swallow it and the engine unwinds cleanly back to the embedder.
///
/// True preemption of a host-call-free CPU loop would require
/// terminating the whole Web Worker from the main thread (an editor-
/// side concern, outside this crate). `loop_iterations` is the
/// in-crate backstop for that class.
#[derive(Debug, Clone, Copy)]
pub struct ScriptBudget {
    pub loop_iterations: u64,
    pub recursion_depth: usize,
    pub stack_size: usize,
    /// Wall-clock ceiling in milliseconds, enforced at host-call
    /// boundaries. `None` disables the deadline (loop/recursion/stack
    /// guards still apply).
    pub wall_clock_ms: Option<u64>,
}

/// Default budget. Preserves the values that shipped with the PARTIAL
/// B-09 landing (10M loop iterations, Boa's default 512 recursion) and
/// adds the previously-absent wall-clock ceiling + an explicit stack
/// guard.
impl Default for ScriptBudget {
    fn default() -> Self {
        Self {
            loop_iterations: 10_000_000,
            recursion_depth: 512,
            // Boa's own default stack size limit; made explicit so the
            // guard classifies as a typed `StackSize` exhaustion.
            stack_size: boa_engine::vm::RuntimeLimits::default().stack_size_limit(),
            // 2 s: long enough for a heavy document sweep that calls
            // through the bridge, short enough that a stuck native
            // chain in the editor REPL doesn't feel like a hang.
            wall_clock_ms: Some(2_000),
        }
    }
}

/// Wall-clock source, in milliseconds (same unit + shape as the canvas
/// worker's `Clock`: `js_sys::Date::now` on wasm, a monotonic-ish
/// native clock in tests). Injected by the host so this crate stays
/// wasm-clean — it never touches `std::time` on its own.
pub type Clock<'a> = dyn Fn() -> f64 + 'a;

/// Result of one `execute_script` call. Output is the accumulated
/// `console.log` / `console.warn` / etc. lines (in emission order).
/// `error` is set when the script threw an unhandled exception.
/// `budget_kind` is set (and `error` is also set) when the abort was a
/// runtime-budget exhaustion rather than an ordinary script error —
/// the typed half of the B-09 / W-08 contract.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptResult {
    pub output: Vec<String>,
    pub error: Option<String>,
    /// `Some(kind)` iff the script aborted on a runtime budget. Always
    /// accompanied by a human-readable `error`. Additive: absent in
    /// the JSON for ordinary results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_kind: Option<ScriptBudgetKind>,
}

// SAFETY pattern: Boa runs host functions synchronously inside the
// `ctx.eval(...)` call from `execute_script`. We stash a raw pointer
// to the CanvasModel in thread-local storage on entry and clear it
// on exit. The host functions read it via `with_model`. Wasm is
// single-threaded so the thread-local pattern is safe by
// construction; native (test-only) usage is also serial.
thread_local! {
    static MODEL_PTR: RefCell<Option<*mut CanvasModel>> = const { RefCell::new(None) };
    static OUTPUT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Wall-clock deadline state for the in-flight `execute_script`
    /// call. `clock` returns "now" in ms; `deadline_ms` is the absolute
    /// ms value past which any host-function entry aborts. Both are
    /// cleared on exit so a deadline can never leak across calls. The
    /// clock is a boxed closure (the host injects it) — single-threaded
    /// by construction, like `MODEL_PTR`.
    static DEADLINE: RefCell<Option<DeadlineState>> = const { RefCell::new(None) };
    /// Set once when the deadline first trips, so every subsequent host
    /// call in the same execution short-circuits to the same typed
    /// abort (and so the classifier can recover the kind even though
    /// Boa's `RuntimeLimit` message is opaque).
    static DEADLINE_TRIPPED: Cell<bool> = const { Cell::new(false) };
}

struct DeadlineState {
    /// Borrowed clock reference, lifetime-erased to `'static`. SAFETY
    /// contract (same as `MODEL_PTR`): only dereferenced inside
    /// `check_deadline`, which only runs while this `execute_script_with`
    /// call is on the stack; the slot is cleared before the call
    /// returns, so the reference is never read after the real clock is
    /// gone. Single-threaded by construction.
    clock: &'static Clock<'static>,
    deadline_ms: f64,
}

/// Sentinel embedded in the wall-clock abort message so `classify_*`
/// can tell a deadline breach apart from Boa's own loop/recursion
/// `RuntimeLimit` messages (both arrive as opaque native errors).
const WALL_CLOCK_SENTINEL: &str = "paged:wall-clock-deadline";

/// Returns `Err(non-catchable RuntimeLimit)` when the wall-clock
/// deadline has passed; `Ok(())` otherwise (or when no deadline is
/// set). Called at the top of every host function — the only place a
/// single-threaded synchronous engine can observe the clock without a
/// native VM interrupt hook (which Boa 0.21 does not expose). Once
/// tripped it stays tripped for the rest of the execution.
fn check_deadline() -> JsResult<()> {
    if DEADLINE_TRIPPED.with(Cell::get) {
        return Err(deadline_error());
    }
    let over = DEADLINE.with(|d| {
        d.borrow()
            .as_ref()
            .is_some_and(|s| (s.clock)() >= s.deadline_ms)
    });
    if over {
        DEADLINE_TRIPPED.with(|c| c.set(true));
        return Err(deadline_error());
    }
    Ok(())
}

fn deadline_error() -> boa_engine::JsError {
    // `RuntimeLimit` is NON-catchable in Boa (see `JsNativeErrorKind::
    // is_catchable`), so a user `try { ... } catch {}` cannot swallow
    // the abort — it unwinds straight back to the embedder, exactly
    // like the native loop/recursion limits. The sentinel lets the
    // classifier recover the `WallClock` kind.
    JsNativeError::runtime_limit()
        .with_message(format!(
            "{WALL_CLOCK_SENTINEL}: script exceeded its time budget"
        ))
        .into()
}

fn with_model<R>(f: impl FnOnce(&mut CanvasModel) -> R) -> R {
    MODEL_PTR.with(|p| {
        let ptr = p
            .borrow()
            .expect("paged-script: host fn called outside execute_script");
        // SAFETY: pointer valid for the duration of the enclosing
        // execute_script call (set/cleared by execute_script itself).
        unsafe { f(&mut *ptr) }
    })
}

fn push_output(line: String) {
    OUTPUT.with(|o| o.borrow_mut().push(line));
}

/// B-09 (plugin platform) / W-08 (plugin-web transforms) — runtime
/// budgets. Boa runs synchronously inside the worker, so a runaway
/// script hangs the editor unless the engine itself bails. The budget
/// combines Boa's native `RuntimeLimits` (loop / recursion / stack)
/// with a bridge-level wall-clock deadline checked at host-call
/// boundaries. See `ScriptBudget` for the full mechanism + the precise
/// wasm wall-clock guarantee.
fn budgeted_context(budget: &ScriptBudget) -> Context {
    let mut ctx = Context::default();
    let mut limits = boa_engine::vm::RuntimeLimits::default();
    limits.set_loop_iteration_limit(budget.loop_iterations);
    limits.set_recursion_limit(budget.recursion_depth);
    limits.set_stack_size_limit(budget.stack_size);
    ctx.set_runtime_limits(limits);
    ctx
}

/// Run `source` against the given `CanvasModel` with the default
/// budget and a native wall-clock. Every write the script issues lands
/// as an `Operation` via `apply_mutation`, so undo/redo work
/// identically to any UI-driven change.
///
/// Convenience wrapper over [`execute_script_with`]; hosts that need to
/// tighten or loosen the budget (the editor REPL, the plugin runner,
/// headless conformance) — or that must inject a wasm-safe clock —
/// call `execute_script_with` directly.
pub fn execute_script(model: &mut CanvasModel, source: &str) -> ScriptResult {
    execute_script_with(model, source, ScriptBudget::default(), &native_clock)
}

/// Native millisecond clock used by `execute_script`. Monotonic enough
/// for a deadline: `Instant` since a process-lifetime anchor. Kept
/// behind a `cfg` so the crate never references `std::time` on wasm32,
/// where the host injects `js_sys::Date::now` instead.
#[cfg(not(target_arch = "wasm32"))]
fn native_clock() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    let anchor = ANCHOR.get_or_init(Instant::now);
    anchor.elapsed().as_secs_f64() * 1000.0
}

/// On wasm32 the host MUST supply a clock via `execute_script_with`;
/// the default `execute_script` falls back to a zero clock, which
/// disables only the wall-clock deadline (loop / recursion / stack
/// guards still apply). The wasm worker always uses
/// `execute_script_with(js_sys::Date::now)`, so this fallback is never
/// the live path — it exists so the crate compiles wasm-clean without
/// `std::time`.
#[cfg(target_arch = "wasm32")]
fn native_clock() -> f64 {
    0.0
}

/// Run `source` with an explicit `budget` and host-injected `clock`.
/// The per-execution entry point for embedders. The clock returns "now"
/// in milliseconds (`js_sys::Date::now` on wasm); the wall-clock
/// deadline, if any, is `clock() + budget.wall_clock_ms` captured at
/// entry and enforced at every host-function boundary.
pub fn execute_script_with(
    model: &mut CanvasModel,
    source: &str,
    budget: ScriptBudget,
    clock: &Clock<'_>,
) -> ScriptResult {
    let ptr = model as *mut CanvasModel;
    MODEL_PTR.with(|p| *p.borrow_mut() = Some(ptr));
    OUTPUT.with(|o| o.borrow_mut().clear());
    DEADLINE_TRIPPED.with(|c| c.set(false));
    // Clear any stale deadline up front too, so a `wall_clock_ms: None`
    // run never inherits a previous call's clock reference (defensive:
    // the end-of-call cleanup already clears it on the normal path).
    DEADLINE.with(|d| *d.borrow_mut() = None);

    // Capture the deadline up front; the stored clock reference re-reads
    // "now" on each host call (see `check_deadline`). The borrow of
    // `clock` does not outlive this call — the DEADLINE slot is cleared
    // before we return.
    if let Some(ms) = budget.wall_clock_ms {
        let deadline_ms = clock() + ms as f64;
        // Lifetime-erase the borrowed clock to `'static` so it can live
        // in the thread-local for the duration of this call. SAFETY:
        // the DEADLINE slot is cleared before this function returns
        // (below), and `check_deadline` — the only reader — only runs
        // synchronously inside `run` on this same stack. So the erased
        // reference is never dereferenced after `clock` goes out of
        // scope. Single-threaded, same contract as `MODEL_PTR`.
        let clock_static: &'static Clock<'static> =
            unsafe { std::mem::transmute::<&Clock<'_>, &'static Clock<'static>>(clock) };
        DEADLINE.with(|d| {
            *d.borrow_mut() = Some(DeadlineState {
                clock: clock_static,
                deadline_ms,
            });
        });
    }

    let mut ctx = budgeted_context(&budget);

    let error = run(&mut ctx, source);
    let budget_kind = error.as_deref().and_then(classify_budget_message);

    let output = OUTPUT.with(|o| std::mem::take(&mut *o.borrow_mut()));
    MODEL_PTR.with(|p| *p.borrow_mut() = None);
    DEADLINE.with(|d| *d.borrow_mut() = None);
    DEADLINE_TRIPPED.with(|c| c.set(false));

    ScriptResult {
        output,
        error,
        budget_kind,
    }
}

/// Recover the typed `ScriptBudgetKind` from a budget-abort message.
/// Boa surfaces every `RuntimeLimit` as an opaque string, so we match
/// on the (stable) message text + our own wall-clock sentinel. Returns
/// `None` for ordinary script errors.
fn classify_budget_message(msg: &str) -> Option<ScriptBudgetKind> {
    // Only "runtime budget exceeded: ..." lines are budget aborts;
    // `format_error` prefixes exactly that for native RuntimeLimits.
    if !msg.contains("runtime budget exceeded") {
        return None;
    }
    // NB: match on the sentinel only — the generic prefix
    // "runtime budget exceeded" itself contains the substring
    // "time budget", so we must not key WallClock off that.
    if msg.contains(WALL_CLOCK_SENTINEL) {
        Some(ScriptBudgetKind::WallClock)
    } else if msg.contains("loop iteration") {
        Some(ScriptBudgetKind::Iterations)
    } else if msg.contains("recursive calls") {
        Some(ScriptBudgetKind::Recursion)
    } else if msg.contains("call stack") {
        Some(ScriptBudgetKind::StackSize)
    } else {
        // Unknown RuntimeLimit message — still a budget abort; default
        // to Iterations rather than mislabel it as an ordinary error.
        Some(ScriptBudgetKind::Iterations)
    }
}

fn run(ctx: &mut Context, source: &str) -> Option<String> {
    if let Err(e) = install_bridge(ctx) {
        return Some(format!("bridge install: {}", format_error(&e, ctx)));
    }
    let bootstrap = r#"
        (function () {
            const baseSet = paged.set;
            const baseGet = paged.get;
            paged.frame = function (id) {
                return new Proxy({}, {
                    set(_t, prop, value) {
                        baseSet(id, String(prop), value);
                        return true;
                    },
                    get(_t, prop) {
                        const raw = baseGet(id, String(prop));
                        if (raw === null || raw === undefined) return null;
                        try { return JSON.parse(raw); } catch (_) { return raw; }
                    },
                });
            };
        })();
    "#;
    if let Err(e) = ctx.eval(Source::from_bytes(bootstrap.as_bytes())) {
        return Some(format!("bootstrap: {}", format_error(&e, ctx)));
    }
    match ctx.eval(Source::from_bytes(source.as_bytes())) {
        Ok(v) => {
            if !v.is_undefined() {
                let line = format_value(&v, ctx);
                push_output(line);
            }
            None
        }
        Err(e) => Some(format_error(&e, ctx)),
    }
}

/// Wrap a host function pointer so the wall-clock deadline is checked
/// before it runs. This is the single point where the synchronous
/// engine observes the clock (Boa 0.21 has no native per-instruction
/// interrupt on its sync run loop): every `paged.*` / `console.*` call
/// — the only way a script reaches the document or blocks on a slow
/// native — passes through here. A fn pointer is `Copy`, so the `move`
/// closure satisfies `from_copy_closure`'s `Copy` bound.
fn guarded(f: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>) -> NativeFunction {
    NativeFunction::from_copy_closure(move |this, args, ctx| {
        check_deadline()?;
        f(this, args, ctx)
    })
}

fn install_bridge(ctx: &mut Context) -> JsResult<()> {
    let paged = ObjectInitializer::new(ctx)
        .function(guarded(paged_set), js_string!("set"), 3)
        .function(guarded(paged_get), js_string!("get"), 2)
        .function(guarded(paged_insert_text), js_string!("insertText"), 3)
        .function(guarded(paged_delete_range), js_string!("deleteRange"), 3)
        .function(
            guarded(paged_insert_text_frame),
            js_string!("insertTextFrame"),
            2,
        )
        .function(guarded(paged_insert_frame), js_string!("insertFrame"), 2)
        .function(guarded(paged_insert_page), js_string!("insertPage"), 1)
        .function(guarded(paged_place_image), js_string!("placeImage"), 3)
        .function(guarded(paged_apply_style), js_string!("applyStyle"), 4)
        .function(guarded(paged_create_group), js_string!("createGroup"), 1)
        .function(guarded(paged_undo), js_string!("undo"), 0)
        .function(guarded(paged_redo), js_string!("redo"), 0)
        .function(guarded(paged_inspect), js_string!("inspect"), 1)
        .function(guarded(paged_layers), js_string!("layers"), 0)
        .function(guarded(paged_tree), js_string!("tree"), 0)
        .function(guarded(paged_pages), js_string!("pages"), 0)
        .function(guarded(paged_stories), js_string!("stories"), 0)
        .function(guarded(paged_swatches), js_string!("swatches"), 0)
        .function(
            guarded(paged_paragraph_styles),
            js_string!("paragraphStyles"),
            0,
        )
        .function(
            guarded(paged_character_styles),
            js_string!("characterStyles"),
            0,
        )
        .function(guarded(paged_object_styles), js_string!("objectStyles"), 0)
        .function(guarded(paged_links), js_string!("links"), 0)
        .function(guarded(paged_conditions), js_string!("conditions"), 0)
        .function(
            guarded(paged_condition_sets),
            js_string!("conditionSets"),
            0,
        )
        .function(guarded(paged_color_groups), js_string!("colorGroups"), 0)
        .function(guarded(paged_gradients), js_string!("gradients"), 0)
        .function(guarded(paged_collection), js_string!("collection"), 1)
        .function(guarded(paged_document_meta), js_string!("documentMeta"), 0)
        .function(guarded(paged_selection), js_string!("selection"), 0)
        .function(
            guarded(paged_content_selection),
            js_string!("contentSelection"),
            0,
        )
        // --- complete mutation surface (additive) ---
        // pages & masters
        .function(guarded(paged_delete_page), js_string!("deletePage"), 1)
        .function(
            guarded(paged_duplicate_page),
            js_string!("duplicatePage"),
            1,
        )
        .function(guarded(paged_resize_page), js_string!("resizePage"), 2)
        .function(
            guarded(paged_apply_master_to_page),
            js_string!("applyMasterToPage"),
            2,
        )
        // frames & groups
        .function(
            guarded(paged_delete_element),
            js_string!("deleteElement"),
            1,
        )
        .function(
            guarded(paged_dissolve_group),
            js_string!("dissolveGroup"),
            1,
        )
        .function(guarded(paged_move_frame), js_string!("moveFrame"), 2)
        .function(guarded(paged_resize_frame), js_string!("resizeFrame"), 2)
        .function(guarded(paged_link_frames), js_string!("linkFrames"), 2)
        .function(guarded(paged_unlink_frames), js_string!("unlinkFrames"), 1)
        // shape inserts
        .function(guarded(paged_insert_line), js_string!("insertLine"), 3)
        .function(guarded(paged_insert_oval), js_string!("insertOval"), 2)
        .function(guarded(paged_insert_path), js_string!("insertPath"), 4)
        // path-point editing
        .function(
            guarded(paged_path_point_insert),
            js_string!("pathPointInsert"),
            4,
        )
        .function(
            guarded(paged_path_point_remove),
            js_string!("pathPointRemove"),
            2,
        )
        .function(
            guarded(paged_path_point_curve_type),
            js_string!("pathPointCurveType"),
            3,
        )
        .function(
            guarded(paged_path_point_set),
            js_string!("pathPointSet"),
            4,
        )
        .function(guarded(paged_path_open_at), js_string!("pathOpenAt"), 2)
        .function(
            guarded(paged_outline_stroke),
            js_string!("outlineStroke"),
            5,
        )
        .function(guarded(paged_offset_path), js_string!("offsetPath"), 4)
        .function(guarded(paged_simplify_path), js_string!("simplifyPath"), 2)
        .function(
            guarded(paged_pathfinder_boolean),
            js_string!("pathfinderBoolean"),
            3,
        )
        // fields & images
        .function(guarded(paged_insert_field), js_string!("insertField"), 3)
        .function(
            guarded(paged_set_field_value),
            js_string!("setFieldValue"),
            3,
        )
        .function(
            guarded(paged_replace_image_bytes),
            js_string!("replaceImageBytes"),
            2,
        )
        // tables
        .function(guarded(paged_insert_table), js_string!("insertTable"), 2)
        .function(guarded(paged_set_row_height), js_string!("setRowHeight"), 4)
        .function(
            guarded(paged_set_column_width),
            js_string!("setColumnWidth"),
            4,
        )
        .function(
            guarded(paged_insert_table_row),
            js_string!("insertTableRow"),
            3,
        )
        .function(
            guarded(paged_delete_table_row),
            js_string!("deleteTableRow"),
            3,
        )
        .function(
            guarded(paged_insert_table_column),
            js_string!("insertTableColumn"),
            3,
        )
        .function(
            guarded(paged_delete_table_column),
            js_string!("deleteTableColumn"),
            3,
        )
        .function(
            guarded(paged_insert_header_row),
            js_string!("insertHeaderRow"),
            2,
        )
        .function(
            guarded(paged_remove_header_row),
            js_string!("removeHeaderRow"),
            2,
        )
        .function(
            guarded(paged_insert_footer_row),
            js_string!("insertFooterRow"),
            2,
        )
        .function(
            guarded(paged_remove_footer_row),
            js_string!("removeFooterRow"),
            2,
        )
        .function(guarded(paged_set_cell_span), js_string!("setCellSpan"), 6)
        // style CRUD
        .function(
            guarded(paged_create_paragraph_style),
            js_string!("createParagraphStyle"),
            1,
        )
        .function(
            guarded(paged_rename_paragraph_style),
            js_string!("renameParagraphStyle"),
            2,
        )
        .function(
            guarded(paged_delete_paragraph_style),
            js_string!("deleteParagraphStyle"),
            1,
        )
        .function(
            guarded(paged_create_character_style),
            js_string!("createCharacterStyle"),
            1,
        )
        .function(
            guarded(paged_rename_character_style),
            js_string!("renameCharacterStyle"),
            2,
        )
        .function(
            guarded(paged_delete_character_style),
            js_string!("deleteCharacterStyle"),
            1,
        )
        .function(
            guarded(paged_create_object_style),
            js_string!("createObjectStyle"),
            1,
        )
        .function(
            guarded(paged_rename_object_style),
            js_string!("renameObjectStyle"),
            2,
        )
        .function(
            guarded(paged_delete_object_style),
            js_string!("deleteObjectStyle"),
            1,
        )
        .function(
            guarded(paged_create_cell_style),
            js_string!("createCellStyle"),
            1,
        )
        .function(
            guarded(paged_rename_cell_style),
            js_string!("renameCellStyle"),
            2,
        )
        .function(
            guarded(paged_delete_cell_style),
            js_string!("deleteCellStyle"),
            1,
        )
        .function(
            guarded(paged_create_table_style),
            js_string!("createTableStyle"),
            1,
        )
        .function(
            guarded(paged_rename_table_style),
            js_string!("renameTableStyle"),
            2,
        )
        .function(
            guarded(paged_delete_table_style),
            js_string!("deleteTableStyle"),
            1,
        )
        .function(
            guarded(paged_set_style_property),
            js_string!("setStyleProperty"),
            4,
        )
        // numbering lists
        .function(
            guarded(paged_create_numbering_list),
            js_string!("createNumberingList"),
            1,
        )
        .function(
            guarded(paged_edit_numbering_list),
            js_string!("editNumberingList"),
            2,
        )
        .function(
            guarded(paged_delete_numbering_list),
            js_string!("deleteNumberingList"),
            1,
        )
        // sections
        .function(
            guarded(paged_insert_section),
            js_string!("insertSection"),
            2,
        )
        .function(guarded(paged_edit_section), js_string!("editSection"), 2)
        .function(
            guarded(paged_delete_section),
            js_string!("deleteSection"),
            1,
        )
        // conditions
        .function(
            guarded(paged_set_condition_visible),
            js_string!("setConditionVisible"),
            2,
        )
        .function(
            guarded(paged_activate_condition_set),
            js_string!("activateConditionSet"),
            1,
        )
        // layers
        .function(guarded(paged_layer_insert), js_string!("layerInsert"), 2)
        .function(guarded(paged_layer_remove), js_string!("layerRemove"), 1)
        .function(guarded(paged_layer_move), js_string!("layerMove"), 2)
        // guides
        .function(guarded(paged_insert_guide), js_string!("insertGuide"), 4)
        .function(guarded(paged_move_guide), js_string!("moveGuide"), 2)
        .function(guarded(paged_delete_guide), js_string!("deleteGuide"), 1)
        // document defaults & colour management
        .function(
            guarded(paged_set_document_defaults),
            js_string!("setDocumentDefaults"),
            1,
        )
        .function(
            guarded(paged_set_color_settings),
            js_string!("setColorSettings"),
            1,
        )
        .function(
            guarded(paged_set_proof_setup),
            js_string!("setProofSetup"),
            1,
        )
        .function(
            guarded(paged_import_swatch_library),
            js_string!("importSwatchLibrary"),
            2,
        )
        .function(
            guarded(paged_set_ink_setting),
            js_string!("setInkSetting"),
            2,
        )
        .function(
            guarded(paged_set_use_standard_lab_for_spots),
            js_string!("setUseStandardLabForSpots"),
            1,
        )
        // plugin metadata & batch
        .function(
            guarded(paged_set_plugin_metadata),
            js_string!("setPluginMetadata"),
            4,
        )
        .function(guarded(paged_batch), js_string!("batch"), 1)
        // selection setters
        .function(
            guarded(paged_set_element_selection),
            js_string!("setElementSelection"),
            1,
        )
        .function(
            guarded(paged_clear_selection),
            js_string!("clearSelection"),
            0,
        )
        .function(
            guarded(paged_set_content_selection),
            js_string!("setContentSelection"),
            1,
        )
        .build();
    ctx.register_global_property(js_string!("paged"), paged, Attribute::all())?;

    let console = ObjectInitializer::new(ctx)
        .function(guarded(console_log), js_string!("log"), 0)
        .function(guarded(console_warn), js_string!("warn"), 0)
        .function(guarded(console_error), js_string!("error"), 0)
        .function(guarded(console_info), js_string!("info"), 0)
        .build();
    ctx.register_global_property(js_string!("console"), console, Attribute::all())?;

    Ok(())
}

// ---------------------------------------------------------------- paged.*

fn paged_set(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let path = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let value_arg = args.get_or_undefined(2).clone();

    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };

    // W1.20 (groups v2) — `paged.set("group:<id>", "groupTransform",
    // [a,b,c,d,tx,ty])` moves/scales/rotates a group as a UNIT (members'
    // effective transforms + the hit-test follow). This is distinct
    // from `frameTransform` on a group, which writes only the group's
    // own metadata transform (the v1 store-only arm). Routed to the
    // dedicated `SetGroupTransform` mutation rather than
    // `SetElementProperty`.
    if path == "groupTransform" {
        let paged_canvas::element_selection::ElementId::Group(group_id) = element_id else {
            return Ok(JsValue::from(false));
        };
        // `null` clears to identity; a 6-element array sets the matrix.
        let transform = if value_arg.is_null() {
            None
        } else {
            match js_value_to_wire(&value_arg, paged_mutate::PropertyPath::FrameTransform, ctx) {
                Some(paged_mutate::Value::Transform(t)) => t,
                _ => return Ok(JsValue::from(false)),
            }
        };
        let mutation = Mutation::SetGroupTransform {
            group_id,
            transform,
        };
        return Ok(JsValue::from(with_model(|m| {
            m.apply_mutation(&mutation).is_ok()
        })));
    }

    let Some(wire_path) = parse_property_path(&path) else {
        return Ok(JsValue::from(false));
    };
    let Some(wire_value) = js_value_to_wire(&value_arg, wire_path, ctx) else {
        return Ok(JsValue::from(false));
    };

    let mutation = Mutation::SetElementProperty {
        element_id,
        path: wire_path,
        value: wire_value,
    };
    Ok(JsValue::from(with_model(|m| {
        m.apply_mutation(&mutation).is_ok()
    })))
}

/// `paged.insertText(storyId, offset, text)` — insert plain text at a
/// story-local body offset (Stage 1 text authoring). `\n` splits into
/// paragraphs. Lands as `Mutation::InsertText` through the same apply
/// layer as the UI Type tool, so undo/redo work identically. Returns
/// `true` iff the insert applied. (Cell-local insertion is `None` for
/// now — the body-offset form covers ordinary story authoring.)
fn paged_insert_text(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let offset = args.get_or_undefined(1).to_number(ctx)? as u32;
    let text = args
        .get_or_undefined(2)
        .to_string(ctx)?
        .to_std_string_escaped();
    let mutation = Mutation::InsertText {
        story_id,
        offset,
        text,
        cell: None,
    };
    Ok(JsValue::from(with_model(|m| {
        m.apply_mutation(&mutation).is_ok()
    })))
}

/// `paged.deleteRange(storyId, start, end)` — delete the `[start, end)`
/// story-local body range (`Mutation::DeleteRange`). The complement of
/// `insertText`, for re-merge / replace flows. Returns `true` iff applied.
fn paged_delete_range(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let start = args.get_or_undefined(1).to_number(ctx)? as u32;
    let end = args.get_or_undefined(2).to_number(ctx)? as u32;
    let mutation = Mutation::DeleteRange {
        story_id,
        start,
        end,
        cell: None,
    };
    Ok(JsValue::from(with_model(|m| {
        m.apply_mutation(&mutation).is_ok()
    })))
}

/// Apply a structural insert, auto-select the freshly-created element
/// (so a following `paged.selection()` / `paged.set` addresses it), and
/// return its `kind:id` address as a JS string — or `null` on failure or
/// when the mutation mints no addressable element. Selection is set on the
/// worker model only (it doesn't push to the editor's React panels — a
/// protocol-bumping follow-up).
fn apply_insert(mutation: &Mutation) -> JsValue {
    let created = with_model(|m| match m.apply_mutation(mutation) {
        Ok(outcome) => {
            if let Some(id) = &outcome.created_id {
                m.element_selection.ids = vec![id.clone()];
            }
            outcome.created_id
        }
        Err(_) => None,
    });
    match created {
        Some(id) => JsValue::from(js_string!(element_id_to_address(&id))),
        None => JsValue::null(),
    }
}

/// `paged.insertTextFrame(pageId, [top, left, bottom, right])` — create
/// an empty, text-pourable frame on a page at page-local point bounds
/// (`Mutation::InsertTextFrame`; the model mints the ParentStory) and
/// select it. Returns the new frame's `textFrame:<id>` address (pass it to
/// `paged.insertText` / `paged.set`), or `null` if the insert failed or
/// `bounds` is not a 4-number array.
fn paged_insert_text_frame(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let page = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(bounds) = read_quad(args.get_or_undefined(1), ctx) else {
        return Ok(JsValue::null());
    };
    let mutation = Mutation::InsertTextFrame {
        page_id: PageId(page),
        bounds,
    };
    Ok(apply_insert(&mutation))
}

/// Read a JS 4-number array `[a, b, c, d]` into a tuple. Mirrors the
/// array-length idiom `js_value_to_wire` uses for `FrameBounds`.
fn read_quad(value: &JsValue, ctx: &mut Context) -> Option<(f32, f32, f32, f32)> {
    let obj = value.as_object()?;
    let len = obj.get(js_string!("length"), ctx).ok()?.as_number()? as usize;
    if len != 4 {
        return None;
    }
    let mut out = [0.0f32; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = obj.get(i as u32, ctx).ok()?.as_number()? as f32;
    }
    Some((out[0], out[1], out[2], out[3]))
}

// ------------------------------------------------- Stage 2: structural authoring

/// `paged.insertFrame(pageId, [t,l,b,r])` — create an empty graphic
/// (non-text) frame on a page (`Mutation::InsertFrame`); the usual
/// target for `placeImage`. Sibling of `insertTextFrame`.
fn paged_insert_frame(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let page = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(bounds) = read_quad(args.get_or_undefined(1), ctx) else {
        return Ok(JsValue::null());
    };
    let mutation = Mutation::InsertFrame {
        page_id: PageId(page),
        bounds,
    };
    Ok(apply_insert(&mutation))
}

/// `paged.insertPage(afterPageId?)` — append a page after `afterPageId`
/// (or at the document end when omitted), inheriting the default master
/// (`Mutation::InsertPage`).
fn paged_insert_page(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let after = args.get_or_undefined(0);
    let after_page_id = if after.is_undefined() || after.is_null() {
        None
    } else {
        let s = after.to_string(ctx)?.to_std_string_escaped();
        (!s.is_empty()).then_some(PageId(s))
    };
    let mutation = Mutation::InsertPage {
        after_page_id,
        master_id: None,
    };
    // A page has no `ElementId` variant (so `MutationOutcome.created_id` is
    // None for InsertPage); recover the minted page id by diffing the page
    // `selfId`s before/after. Pages are few, so the scan is free. The
    // returned `selfId` is reusable as the next `afterPageId`.
    let before: std::collections::HashSet<String> =
        with_model(|m| m.pages().into_iter().map(|p| p.self_id).collect());
    let ok = with_model(|m| m.apply_mutation(&mutation).is_ok());
    if !ok {
        return Ok(JsValue::null());
    }
    let new_id =
        with_model(|m| m.pages().into_iter().find(|p| !before.contains(&p.self_id))).map(|p| p.self_id);
    Ok(match new_id {
        Some(s) => JsValue::from(js_string!(s)),
        None => JsValue::null(),
    })
}

/// `paged.placeImage(frameId, uri, fit?)` — place an image into a frame
/// (`Mutation::PlaceImage`). `fit` is an optional InDesign fitting mode.
fn paged_place_image(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let element_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let uri = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let fit_arg = args.get_or_undefined(2);
    let fit = if fit_arg.is_undefined() || fit_arg.is_null() {
        None
    } else {
        Some(fit_arg.to_string(ctx)?.to_std_string_escaped())
    };
    let mutation = Mutation::PlaceImage {
        element_id,
        uri,
        fit,
    };
    Ok(JsValue::from(with_model(|m| {
        m.apply_mutation(&mutation).is_ok()
    })))
}

/// `paged.applyStyle(storyId, start, end, styleRef)` — apply a named
/// paragraph/character style to a story range (`Mutation::ApplyStyle`).
/// Scope is inferred from the ref prefix (`CharacterStyle/…` →
/// Character, else Paragraph).
fn paged_apply_style(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let start = args.get_or_undefined(1).to_number(ctx)? as u32;
    let end = args.get_or_undefined(2).to_number(ctx)? as u32;
    let style = args
        .get_or_undefined(3)
        .to_string(ctx)?
        .to_std_string_escaped();
    let scope = if style.starts_with("CharacterStyle") {
        paged_mutate::operation::StyleScope::Character
    } else {
        paged_mutate::operation::StyleScope::Paragraph
    };
    let mutation = Mutation::ApplyStyle {
        story_id,
        start,
        end,
        style,
        scope,
    };
    Ok(JsValue::from(with_model(|m| {
        m.apply_mutation(&mutation).is_ok()
    })))
}

/// `paged.createGroup([id, ...])` — group two-or-more elements
/// (`Mutation::CreateGroup`). Ids are parsed with the same resolver as
/// `paged.set`; unparseable entries are dropped, and fewer than two
/// valid members returns `false`.
fn paged_create_group(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let value = args.get_or_undefined(0);
    let Some(obj) = value.as_object() else {
        return Ok(JsValue::from(false));
    };
    let len = obj
        .get(js_string!("length"), ctx)
        .ok()
        .and_then(|v| v.as_number())
        .unwrap_or(0.0) as usize;
    let mut member_ids = Vec::with_capacity(len);
    for i in 0..len {
        let item = obj.get(i as u32, ctx)?;
        let s = item.to_string(ctx)?.to_std_string_escaped();
        if let Some(id) = parse_element_id(&s) {
            member_ids.push(id);
        }
    }
    if member_ids.len() < 2 {
        return Ok(JsValue::from(false));
    }
    let mutation = Mutation::CreateGroup { member_ids };
    Ok(JsValue::from(with_model(|m| {
        m.apply_mutation(&mutation).is_ok()
    })))
}

// ================================================================
// Complete mutation surface — one `paged.<name>` host fn per remaining
// `Mutation` variant. Additive over the Stage 1/2 authoring above.
//
// Three apply shapes recur, factored here:
//   * `apply_bool`        — fire-and-forget; returns the success boolean.
//   * `apply_insert`(above)— structural inserts that carry a `created_id`;
//     auto-selects + returns the `kind:id` address.
//   * `apply_new_self_id` — collection CRUD that mints an id the apply
//     layer does NOT surface via `created_id` (styles, numbering lists);
//     recovered by diffing the collection's `selfId`s — the same trick
//     `insertPage`/`duplicatePage` use for pages.
// ================================================================

// ---------------------------------------------------- shared apply helpers

/// Apply a mutation for its success boolean only.
fn apply_bool(mutation: &Mutation) -> JsValue {
    JsValue::from(with_model(|m| m.apply_mutation(mutation).is_ok()))
}

/// Resolve an element address (`kind:id`) — or an already-bare self id —
/// to the bare `Self` id the typed frame mutations
/// (`MoveFrame`/`ResizeFrame`/`DeleteFrame`/`LinkFrames`/`UnlinkFrames`/
/// `DissolveGroup`/`PlaceImage`/`ReplaceImageBytes`) match against
/// (`CanvasModel::resolve_frame_node_id` keys off the bare id).
fn bare_id(s: &str) -> String {
    parse_element_id(s)
        .map(|id| id.raw_id().to_string())
        .unwrap_or_else(|| s.to_string())
}

/// The `selfId` set of a typed document collection — the before/after
/// snapshot used to recover a freshly-minted id the apply layer does not
/// thread through `created_id`.
fn collection_self_ids(name: CollectionName) -> std::collections::HashSet<String> {
    with_model(|m| {
        serde_json::to_value(m.collection(name))
            .ok()
            .and_then(|v| {
                v.as_array().map(|a| {
                    a.iter()
                        .filter_map(|e| {
                            e.get("selfId").and_then(|s| s.as_str()).map(String::from)
                        })
                        .collect()
                })
            })
            .unwrap_or_default()
    })
}

/// Apply a collection-CRUD create and return the newly-minted `selfId`
/// (string) by diffing the collection, or `null` on failure / no new id.
fn apply_new_self_id(name: CollectionName, mutation: &Mutation) -> JsValue {
    let before = collection_self_ids(name);
    if !with_model(|m| m.apply_mutation(mutation).is_ok()) {
        return JsValue::null();
    }
    match collection_self_ids(name)
        .into_iter()
        .find(|id| !before.contains(id))
    {
        Some(id) => JsValue::from(js_string!(id)),
        None => JsValue::null(),
    }
}

/// Apply an `InsertTable` and return the minted `<Table>` `Self` id
/// (the id the plugin addresses cells with) — or `null` on failure.
fn apply_insert_table(mutation: &Mutation) -> JsValue {
    use paged_canvas::element_selection::ElementId;
    let created = with_model(|m| match m.apply_mutation(mutation) {
        Ok(o) => {
            if let Some(id) = &o.created_id {
                m.element_selection.ids = vec![id.clone()];
            }
            o.created_id
        }
        Err(_) => None,
    });
    match created {
        Some(ElementId::Table { table_id, .. }) => JsValue::from(js_string!(table_id)),
        Some(other) => JsValue::from(js_string!(other.raw_id().to_string())),
        None => JsValue::null(),
    }
}

// ---------------------------------------------------- shared arg readers

/// JS value → `serde_json::Value`, or `None` when it cannot be represented.
fn to_json_value(value: &JsValue, ctx: &mut Context) -> Option<serde_json::Value> {
    value.to_json(ctx).ok().flatten()
}

/// Deserialize a JS arg into a serde spec type (`PathAnchorSpec`,
/// `FieldKind`, `NumberingListSpec`, `Vec<Mutation>`, …). `None` on any
/// shape mismatch — callers translate that to `false`/`null`, never a
/// throw (the user-input-never-panics rule).
fn from_js<T: serde::de::DeserializeOwned>(value: &JsValue, ctx: &mut Context) -> Option<T> {
    serde_json::from_value(to_json_value(value, ctx)?).ok()
}

/// Read a JS 2-number array `[x, y]` (line endpoints, path-point set).
fn read_pair(value: &JsValue, ctx: &mut Context) -> Option<(f32, f32)> {
    let obj = value.as_object()?;
    let len = obj.get(js_string!("length"), ctx).ok()?.as_number()? as usize;
    if len != 2 {
        return None;
    }
    let x = obj.get(0u32, ctx).ok()?.as_number()? as f32;
    let y = obj.get(1u32, ctx).ok()?.as_number()? as f32;
    Some((x, y))
}

/// Read a fixed-length JS number array into `[f32; N]` (the 6-element
/// `MoveFrame` transform). Generalises [`read_quad`].
fn read_floats<const N: usize>(value: &JsValue, ctx: &mut Context) -> Option<[f32; N]> {
    let obj = value.as_object()?;
    let len = obj.get(js_string!("length"), ctx).ok()?.as_number()? as usize;
    if len != N {
        return None;
    }
    let mut out = [0.0f32; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = obj.get(i as u32, ctx).ok()?.as_number()? as f32;
    }
    Some(out)
}

/// An optional `f32` arg: `undefined`/`null`/non-number ⇒ `None`.
fn opt_f32(value: &JsValue) -> Option<f32> {
    value.as_number().map(|n| n as f32)
}

/// An optional non-empty `String` arg: `undefined`/`null`/empty ⇒ `None`.
fn opt_string(value: &JsValue, ctx: &mut Context) -> Option<String> {
    if value.is_undefined() || value.is_null() {
        return None;
    }
    let s = value.to_string(ctx).ok()?.to_std_string_escaped();
    (!s.is_empty()).then_some(s)
}

/// Optional non-empty string property of a JS object.
fn prop_string(
    obj: &boa_engine::object::JsObject,
    key: &str,
    ctx: &mut Context,
) -> Option<String> {
    let v = obj.get(js_string!(key), ctx).ok()?;
    opt_string(&v, ctx)
}

/// Optional `f32` property of a JS object.
fn prop_f32(obj: &boa_engine::object::JsObject, key: &str, ctx: &mut Context) -> Option<f32> {
    obj.get(js_string!(key), ctx)
        .ok()
        .and_then(|v| v.as_number())
        .map(|n| n as f32)
}

/// Optional `u32` property of a JS object.
fn prop_u32(obj: &boa_engine::object::JsObject, key: &str, ctx: &mut Context) -> Option<u32> {
    obj.get(js_string!(key), ctx)
        .ok()
        .and_then(|v| v.as_number())
        .map(|n| n as u32)
}

/// Parse a JS array of element-id address strings into `ElementId`s,
/// dropping unparseable entries (shared by `setElementSelection` +
/// `pathfinderBoolean`).
fn parse_element_id_array(
    value: &JsValue,
    ctx: &mut Context,
) -> Vec<paged_canvas::element_selection::ElementId> {
    let mut out = Vec::new();
    let Some(obj) = value.as_object() else {
        return out;
    };
    let len = obj
        .get(js_string!("length"), ctx)
        .ok()
        .and_then(|v| v.as_number())
        .unwrap_or(0.0) as usize;
    for i in 0..len {
        if let Ok(item) = obj.get(i as u32, ctx) {
            if let Ok(s) = item.to_string(ctx) {
                if let Some(id) = parse_element_id(&s.to_std_string_escaped()) {
                    out.push(id);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------- pages & masters

/// `paged.deletePage(pageId)` — remove a page (`Mutation::DeletePage`).
fn paged_delete_page(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let page = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DeletePage {
        page_id: PageId(page),
    }))
}

/// `paged.duplicatePage(pageId)` — duplicate a single-page spread after
/// the source (`Mutation::DuplicatePage`). Returns the new page `selfId`
/// (recovered by diffing `pages()`, like `insertPage`), or `null`.
fn paged_duplicate_page(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let page = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let mutation = Mutation::DuplicatePage {
        page: PageId(page),
    };
    let before: std::collections::HashSet<String> =
        with_model(|m| m.pages().into_iter().map(|p| p.self_id).collect());
    if !with_model(|m| m.apply_mutation(&mutation).is_ok()) {
        return Ok(JsValue::null());
    }
    let new_id = with_model(|m| m.pages().into_iter().find(|p| !before.contains(&p.self_id)))
        .map(|p| p.self_id);
    Ok(match new_id {
        Some(s) => JsValue::from(js_string!(s)),
        None => JsValue::null(),
    })
}

/// `paged.resizePage(pageId, [t,l,b,r])` — set the page's
/// `GeometricBounds` in page-inner points (`Mutation::ResizePage`).
fn paged_resize_page(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let page = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(bounds) = read_quad(args.get_or_undefined(1), ctx) else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::ResizePage {
        page_id: PageId(page),
        bounds,
    }))
}

/// `paged.applyMasterToPage(pageId, masterId?)` — set a page's applied
/// master (`masterId` omitted/null detaches; `Mutation::ApplyMasterToPage`).
fn paged_apply_master_to_page(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let page = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let master = opt_string(args.get_or_undefined(1), ctx);
    Ok(apply_bool(&Mutation::ApplyMasterToPage {
        page: PageId(page),
        master,
    }))
}

// ---------------------------------------------------- frames & groups

/// `paged.deleteElement(id)` — delete a page item (`Mutation::DeleteFrame`).
/// Accepts the `kind:id` address or a bare self id; groups are removed via
/// `dissolveGroup`, not here.
fn paged_delete_element(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DeleteFrame {
        frame_id: bare_id(&id),
    }))
}

/// `paged.dissolveGroup(groupId)` — ungroup; members return to the
/// group's paint slot (`Mutation::DissolveGroup`).
fn paged_dissolve_group(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DissolveGroup {
        group_id: bare_id(&id),
    }))
}

/// `paged.moveFrame(frameId, [a,b,c,d,tx,ty])` — set a frame's affine
/// placement transform (`Mutation::MoveFrame`).
fn paged_move_frame(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(transform) = read_floats::<6>(args.get_or_undefined(1), ctx) else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::MoveFrame {
        frame_id: bare_id(&id),
        transform,
    }))
}

/// `paged.resizeFrame(frameId, [t,l,b,r])` — set a frame's content-box
/// bounds (`Mutation::ResizeFrame`; this is the re-paginating resize).
fn paged_resize_frame(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(bounds) = read_quad(args.get_or_undefined(1), ctx) else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::ResizeFrame {
        frame_id: bare_id(&id),
        bounds,
    }))
}

/// `paged.linkFrames(fromId, toId)` — thread `from`'s overflow into the
/// empty frame `to` (`Mutation::LinkFrames`).
fn paged_link_frames(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let from = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let to = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::LinkFrames {
        from: bare_id(&from),
        to: bare_id(&to),
    }))
}

/// `paged.unlinkFrames(frameId)` — break the thread leaving `frame`
/// (`Mutation::UnlinkFrames`).
fn paged_unlink_frames(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let frame = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::UnlinkFrames {
        frame: bare_id(&frame),
    }))
}

// ---------------------------------------------------- shape inserts

/// `paged.insertLine(pageId, [x1,y1], [x2,y2])` — insert a two-anchor
/// open `GraphicLine` (`Mutation::InsertLine`). Returns the new
/// `graphicLine:<id>` address.
fn paged_insert_line(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let page = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(start) = read_pair(args.get_or_undefined(1), ctx) else {
        return Ok(JsValue::null());
    };
    let Some(end) = read_pair(args.get_or_undefined(2), ctx) else {
        return Ok(JsValue::null());
    };
    Ok(apply_insert(&Mutation::InsertLine {
        page_id: PageId(page),
        start,
        end,
    }))
}

/// `paged.insertOval(pageId, [t,l,b,r])` — insert an `Oval`
/// (`Mutation::InsertOval`). Returns the new `oval:<id>` address.
fn paged_insert_oval(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let page = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(bounds) = read_quad(args.get_or_undefined(1), ctx) else {
        return Ok(JsValue::null());
    };
    Ok(apply_insert(&Mutation::InsertOval {
        page_id: PageId(page),
        bounds,
    }))
}

/// `paged.insertPath(pageId, anchors, open, smooth?)` — insert an
/// arbitrary path (`Mutation::InsertPath`). `anchors` is a JS array of
/// `{ anchor:[x,y], left:[x,y], right:[x,y] }`. Returns the new address.
fn paged_insert_path(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let page = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(anchors) =
        from_js::<Vec<paged_mutate::operation::PathAnchorSpec>>(args.get_or_undefined(1), ctx)
    else {
        return Ok(JsValue::null());
    };
    let open = args.get_or_undefined(2).to_boolean();
    let smooth = args.get_or_undefined(3).to_boolean();
    Ok(apply_insert(&Mutation::InsertPath {
        page_id: PageId(page),
        anchors,
        open,
        smooth,
    }))
}

// ---------------------------------------------------- path-point editing

/// `paged.pathPointInsert(elemId, index, anchor, subpathStarts?)` — insert
/// an anchor into a path element's flat `PathPointArray` at `index`.
fn paged_path_point_insert(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let index = args.get_or_undefined(1).to_number(ctx)? as u32;
    let Some(anchor) =
        from_js::<paged_mutate::operation::PathAnchorSpec>(args.get_or_undefined(2), ctx)
    else {
        return Ok(JsValue::from(false));
    };
    let sub = args.get_or_undefined(3);
    let prev_subpath_starts = if sub.is_undefined() || sub.is_null() {
        None
    } else {
        from_js::<Vec<u32>>(sub, ctx)
    };
    Ok(apply_bool(&Mutation::PathPointInsert {
        element_id,
        index,
        anchor,
        prev_subpath_starts,
    }))
}

/// `paged.pathPointRemove(elemId, index)` — remove the anchor at flat
/// `index` (`Mutation::PathPointRemove`).
fn paged_path_point_remove(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let index = args.get_or_undefined(1).to_number(ctx)? as u32;
    Ok(apply_bool(&Mutation::PathPointRemove { element_id, index }))
}

/// `paged.pathPointCurveType(elemId, index, smooth)` — toggle an anchor
/// between corner and smooth (`Mutation::PathPointCurveType`).
fn paged_path_point_curve_type(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let index = args.get_or_undefined(1).to_number(ctx)? as u32;
    let smooth = args.get_or_undefined(2).to_boolean();
    Ok(apply_bool(&Mutation::PathPointCurveType {
        element_id,
        index,
        smooth,
    }))
}

/// `paged.pathPointSet(elemId, index, role, [x,y])` — write one Bezier
/// handle (`role` = `"anchor"|"left"|"right"`; `Mutation::PathPointSet`).
fn paged_path_point_set(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let index = args.get_or_undefined(1).to_number(ctx)? as u32;
    let Some(role) = from_js::<paged_mutate::PathPointRole>(args.get_or_undefined(2), ctx) else {
        return Ok(JsValue::from(false));
    };
    let Some((x, y)) = read_pair(args.get_or_undefined(3), ctx) else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::PathPointSet {
        element_id,
        index,
        role,
        position: [x, y],
    }))
}

/// `paged.pathOpenAt(elemId, index)` — cut the path at the anchor at flat
/// `index` (`Mutation::PathOpenAt`).
fn paged_path_open_at(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let index = args.get_or_undefined(1).to_number(ctx)? as u32;
    Ok(apply_bool(&Mutation::PathOpenAt { element_id, index }))
}

/// `paged.outlineStroke(elemId, width, cap, join, miter)` — replace the
/// path with its stroke-expansion outline (`Mutation::OutlineStroke`).
fn paged_outline_stroke(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let width = args.get_or_undefined(1).to_number(ctx)? as f32;
    let cap = args
        .get_or_undefined(2)
        .to_string(ctx)?
        .to_std_string_escaped();
    let join = args
        .get_or_undefined(3)
        .to_string(ctx)?
        .to_std_string_escaped();
    let miter_limit = args.get_or_undefined(4).to_number(ctx)? as f32;
    Ok(apply_bool(&Mutation::OutlineStroke {
        element_id,
        width,
        cap,
        join,
        miter_limit,
    }))
}

/// `paged.offsetPath(elemId, delta, join, miter)` — inset/outset a single
/// closed contour (`Mutation::OffsetPath`).
fn paged_offset_path(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let delta = args.get_or_undefined(1).to_number(ctx)? as f32;
    let join = args
        .get_or_undefined(2)
        .to_string(ctx)?
        .to_std_string_escaped();
    let miter_limit = args.get_or_undefined(3).to_number(ctx)? as f32;
    Ok(apply_bool(&Mutation::OffsetPath {
        element_id,
        delta,
        join,
        miter_limit,
    }))
}

/// `paged.simplifyPath(elemId, tolerance)` — re-express the path with
/// fewer anchors within `tolerance` pt (`Mutation::SimplifyPath`).
fn paged_simplify_path(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let tolerance = args.get_or_undefined(1).to_number(ctx)? as f32;
    Ok(apply_bool(&Mutation::SimplifyPath {
        element_id,
        tolerance,
    }))
}

/// `paged.pathfinderBoolean(keptId, [otherIds], kind)` — Pathfinder
/// boolean op (`kind` = `"union"|"intersect"|"subtract"|"exclude"`;
/// `Mutation::PathfinderBoolean`).
fn paged_pathfinder_boolean(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(kept) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let others = parse_element_id_array(args.get_or_undefined(1), ctx);
    let Some(kind) = from_js::<paged_mutate::PathfinderKind>(args.get_or_undefined(2), ctx) else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::PathfinderBoolean { kept, others, kind }))
}

// ---------------------------------------------------- fields & images

/// `paged.insertField(storyId, offset, fieldKind)` — insert a field marker
/// at a story offset. `fieldKind` = `"pageNumber"` | `"nextPageNumber"` |
/// `{ placeholder: { plugin, key, value? } }` (`Mutation::InsertField`).
fn paged_insert_field(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let offset = args.get_or_undefined(1).to_number(ctx)? as u32;
    let Some(field) = from_js::<paged_mutate::operation::FieldKind>(args.get_or_undefined(2), ctx)
    else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::InsertField {
        story_id,
        offset,
        field,
    }))
}

/// `paged.setFieldValue(storyId, offset, value?)` — update a placeholder
/// field's cached display value (`null` ⇒ unresolved `<key>`;
/// `Mutation::SetFieldValue`).
fn paged_set_field_value(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let offset = args.get_or_undefined(1).to_number(ctx)? as u32;
    let value = opt_string(args.get_or_undefined(2), ctx);
    Ok(apply_bool(&Mutation::SetFieldValue {
        story_id,
        offset,
        value,
    }))
}

/// `paged.replaceImageBytes(frameId, bytes?)` — commit inline image bytes
/// on a graphic frame (`bytes` = a JS `number[]` of u8, or `null` to
/// clear; `Mutation::ReplaceImageBytes`). The `ByteBuf` field is filled by
/// deserializing the whole mutation so the bridge needs no `serde_bytes`
/// dependency.
fn paged_replace_image_bytes(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let bytes_arg = args.get_or_undefined(1);
    let bytes_json = if bytes_arg.is_undefined() || bytes_arg.is_null() {
        serde_json::Value::Null
    } else {
        match to_json_value(bytes_arg, ctx) {
            Some(v) => v,
            None => return Ok(JsValue::from(false)),
        }
    };
    let m_json = serde_json::json!({
        "op": "replaceImageBytes",
        "args": { "elementId": bare_id(&id), "bytes": bytes_json },
    });
    match serde_json::from_value::<Mutation>(m_json) {
        Ok(mutation) => Ok(apply_bool(&mutation)),
        Err(_) => Ok(JsValue::from(false)),
    }
}

// ---------------------------------------------------- tables

/// `paged.insertTable(storyId, spec)` — create a `<Table>` at the end of a
/// story (`Mutation::InsertTable`). `spec` = `{ rows, cols, headerRows?,
/// footerRows?, columnWidths?, rowHeights? }`. Returns the minted table id.
fn paged_insert_table(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    #[derive(Default, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    struct Spec {
        rows: u32,
        cols: u32,
        header_rows: u32,
        footer_rows: u32,
        column_widths: Vec<f32>,
        row_heights: Vec<f32>,
    }
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(spec) = from_js::<Spec>(args.get_or_undefined(1), ctx) else {
        return Ok(JsValue::null());
    };
    Ok(apply_insert_table(&Mutation::InsertTable {
        story_id,
        rows: spec.rows,
        cols: spec.cols,
        header_rows: spec.header_rows,
        footer_rows: spec.footer_rows,
        column_widths: spec.column_widths,
        row_heights: spec.row_heights,
    }))
}

/// `paged.setRowHeight(storyId, tableId, row, height?)` — set/clear a row
/// height in pt (`Mutation::SetRowHeight`).
fn paged_set_row_height(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let row = args.get_or_undefined(2).to_number(ctx)? as u32;
    let height = opt_f32(args.get_or_undefined(3));
    Ok(apply_bool(&Mutation::SetRowHeight {
        story_id,
        table_id,
        row,
        height,
    }))
}

/// `paged.setColumnWidth(storyId, tableId, col, width?)` — set/clear a
/// column width in pt (`Mutation::SetColumnWidth`).
fn paged_set_column_width(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let col = args.get_or_undefined(2).to_number(ctx)? as u32;
    let width = opt_f32(args.get_or_undefined(3));
    Ok(apply_bool(&Mutation::SetColumnWidth {
        story_id,
        table_id,
        col,
        width,
    }))
}

/// `paged.insertTableRow(storyId, tableId, at)` — insert an empty body row
/// (`Mutation::InsertTableRow`).
fn paged_insert_table_row(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let at = args.get_or_undefined(2).to_number(ctx)? as u32;
    Ok(apply_bool(&Mutation::InsertTableRow {
        story_id,
        table_id,
        at,
    }))
}

/// `paged.deleteTableRow(storyId, tableId, at)` — delete the row at `at`
/// (`Mutation::DeleteTableRow`).
fn paged_delete_table_row(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let at = args.get_or_undefined(2).to_number(ctx)? as u32;
    Ok(apply_bool(&Mutation::DeleteTableRow {
        story_id,
        table_id,
        at,
    }))
}

/// `paged.insertTableColumn(storyId, tableId, at)` — insert an empty
/// column (`Mutation::InsertTableColumn`).
fn paged_insert_table_column(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let at = args.get_or_undefined(2).to_number(ctx)? as u32;
    Ok(apply_bool(&Mutation::InsertTableColumn {
        story_id,
        table_id,
        at,
    }))
}

/// `paged.deleteTableColumn(storyId, tableId, at)` — delete the column at
/// `at` (`Mutation::DeleteTableColumn`).
fn paged_delete_table_column(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let at = args.get_or_undefined(2).to_number(ctx)? as u32;
    Ok(apply_bool(&Mutation::DeleteTableColumn {
        story_id,
        table_id,
        at,
    }))
}

/// `paged.insertHeaderRow(storyId, tableId)` — insert a header-band row
/// (`Mutation::InsertHeaderRow`).
fn paged_insert_header_row(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::InsertHeaderRow { story_id, table_id }))
}

/// `paged.removeHeaderRow(storyId, tableId)` — remove the first header row
/// (`Mutation::RemoveHeaderRow`).
fn paged_remove_header_row(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::RemoveHeaderRow { story_id, table_id }))
}

/// `paged.insertFooterRow(storyId, tableId)` — insert a footer-band row
/// (`Mutation::InsertFooterRow`).
fn paged_insert_footer_row(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::InsertFooterRow { story_id, table_id }))
}

/// `paged.removeFooterRow(storyId, tableId)` — remove the last footer row
/// (`Mutation::RemoveFooterRow`).
fn paged_remove_footer_row(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::RemoveFooterRow { story_id, table_id }))
}

/// `paged.setCellSpan(storyId, tableId, row, col, rowSpan, columnSpan)` —
/// set a cell's row/column span (`Mutation::SetCellSpan`).
fn paged_set_cell_span(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let story_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let table_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let row = args.get_or_undefined(2).to_number(ctx)? as u32;
    let col = args.get_or_undefined(3).to_number(ctx)? as u32;
    let row_span = args.get_or_undefined(4).to_number(ctx)? as u32;
    let column_span = args.get_or_undefined(5).to_number(ctx)? as u32;
    Ok(apply_bool(&Mutation::SetCellSpan {
        story_id,
        table_id,
        row,
        col,
        row_span,
        column_span,
    }))
}

// ---------------------------------------------------- style CRUD

/// Read a `{ id?, name?, basedOn? }` style-create spec object.
fn read_style_spec(
    value: &JsValue,
    ctx: &mut Context,
) -> (Option<String>, Option<String>, Option<String>) {
    match value.as_object() {
        Some(obj) => (
            prop_string(&obj, "id", ctx),
            prop_string(&obj, "name", ctx),
            prop_string(&obj, "basedOn", ctx),
        ),
        None => (None, None, None),
    }
}

/// `paged.createParagraphStyle({id?,name?,basedOn?})` — returns the new
/// style id (`Mutation::CreateParagraphStyle`).
fn paged_create_paragraph_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let (self_id, name, based_on) = read_style_spec(args.get_or_undefined(0), ctx);
    Ok(apply_new_self_id(
        CollectionName::ParagraphStyles,
        &Mutation::CreateParagraphStyle {
            self_id,
            name,
            based_on,
        },
    ))
}

/// `paged.renameParagraphStyle(styleId, name)` (`Mutation::RenameParagraphStyle`).
fn paged_rename_paragraph_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let style_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let name = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::RenameParagraphStyle { style_id, name }))
}

/// `paged.deleteParagraphStyle(styleId)` (`Mutation::DeleteParagraphStyle`).
fn paged_delete_paragraph_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let style_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DeleteParagraphStyle { style_id }))
}

/// `paged.createCharacterStyle({id?,name?,basedOn?})` — returns the new id.
fn paged_create_character_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let (self_id, name, based_on) = read_style_spec(args.get_or_undefined(0), ctx);
    Ok(apply_new_self_id(
        CollectionName::CharacterStyles,
        &Mutation::CreateCharacterStyle {
            self_id,
            name,
            based_on,
        },
    ))
}

/// `paged.renameCharacterStyle(styleId, name)`.
fn paged_rename_character_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let style_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let name = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::RenameCharacterStyle { style_id, name }))
}

/// `paged.deleteCharacterStyle(styleId)`.
fn paged_delete_character_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let style_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DeleteCharacterStyle { style_id }))
}

/// `paged.createObjectStyle({id?,name?,basedOn?})` — returns the new id.
fn paged_create_object_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let (self_id, name, based_on) = read_style_spec(args.get_or_undefined(0), ctx);
    Ok(apply_new_self_id(
        CollectionName::ObjectStyles,
        &Mutation::CreateObjectStyle {
            self_id,
            name,
            based_on,
        },
    ))
}

/// `paged.renameObjectStyle(styleId, name)`.
fn paged_rename_object_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let style_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let name = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::RenameObjectStyle { style_id, name }))
}

/// `paged.deleteObjectStyle(styleId)`.
fn paged_delete_object_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let style_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DeleteObjectStyle { style_id }))
}

/// `paged.createCellStyle({id?,name?,basedOn?})` — returns the new id.
fn paged_create_cell_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let (self_id, name, based_on) = read_style_spec(args.get_or_undefined(0), ctx);
    Ok(apply_new_self_id(
        CollectionName::CellStyles,
        &Mutation::CreateCellStyle {
            self_id,
            name,
            based_on,
        },
    ))
}

/// `paged.renameCellStyle(styleId, name)`.
fn paged_rename_cell_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let style_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let name = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::RenameCellStyle { style_id, name }))
}

/// `paged.deleteCellStyle(styleId)`.
fn paged_delete_cell_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let style_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DeleteCellStyle { style_id }))
}

/// `paged.createTableStyle({id?,name?,basedOn?})` — returns the new id.
fn paged_create_table_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let (self_id, name, based_on) = read_style_spec(args.get_or_undefined(0), ctx);
    Ok(apply_new_self_id(
        CollectionName::TableStyles,
        &Mutation::CreateTableStyle {
            self_id,
            name,
            based_on,
        },
    ))
}

/// `paged.renameTableStyle(styleId, name)`.
fn paged_rename_table_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let style_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let name = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::RenameTableStyle { style_id, name }))
}

/// `paged.deleteTableStyle(styleId)`.
fn paged_delete_table_style(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let style_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DeleteTableStyle { style_id }))
}

/// `paged.setStyleProperty(collection, styleId, path, value)` — set one
/// property on a style definition (`collection` =
/// `"paragraph"|"character"|"object"|"cell"|"table"`, `path` a settable
/// path name; `Mutation::SetStyleProperty`).
fn paged_set_style_property(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let Some(collection) =
        from_js::<paged_mutate::StyleCollection>(args.get_or_undefined(0), ctx)
    else {
        return Ok(JsValue::from(false));
    };
    let style_id = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let path_str = args
        .get_or_undefined(2)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(path) = parse_property_path(&path_str) else {
        return Ok(JsValue::from(false));
    };
    let value_arg = args.get_or_undefined(3).clone();
    let Some(value) = js_value_to_wire(&value_arg, path, ctx) else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::SetStyleProperty {
        collection,
        style_id,
        path,
        value,
    }))
}

// ---------------------------------------------------- numbering lists

/// `paged.createNumberingList(spec)` — create a `<NumberingList>`
/// (`spec` = a `NumberingListSpec`; `Mutation::CreateNumberingList`).
/// Returns the new list id.
fn paged_create_numbering_list(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let Some(spec) =
        from_js::<paged_mutate::NumberingListSpec>(args.get_or_undefined(0), ctx)
    else {
        return Ok(JsValue::null());
    };
    Ok(apply_new_self_id(
        CollectionName::NumberingLists,
        &Mutation::CreateNumberingList { spec },
    ))
}

/// `paged.editNumberingList(listId, spec)` (`Mutation::EditNumberingList`).
fn paged_edit_numbering_list(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let list_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(spec) =
        from_js::<paged_mutate::NumberingListSpec>(args.get_or_undefined(1), ctx)
    else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::EditNumberingList { list_id, spec }))
}

/// `paged.deleteNumberingList(listId)` (`Mutation::DeleteNumberingList`).
fn paged_delete_numbering_list(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let list_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DeleteNumberingList { list_id }))
}

// ---------------------------------------------------- sections

/// `paged.insertSection(pageId, {prefix?,style?,start?})` — anchor a
/// `<Section>` at a page (`Mutation::InsertSection`).
fn paged_insert_section(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let page = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let (prefix, numbering_style, start_at) = match args.get_or_undefined(1).as_object() {
        Some(obj) => (
            prop_string(&obj, "prefix", ctx),
            prop_string(&obj, "style", ctx),
            prop_u32(&obj, "start", ctx),
        ),
        None => (None, None, None),
    };
    Ok(apply_bool(&Mutation::InsertSection {
        at_page: PageId(page),
        prefix,
        numbering_style,
        start_at,
    }))
}

/// `paged.editSection(sectionId, {prefix?,style?,start?})` — edit a
/// `<Section>`. `prefix`/`start` are tri-state: omit a key to leave it,
/// pass `null` to clear it (`Mutation::EditSection`).
fn paged_edit_section(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let section_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(obj) = args.get_or_undefined(1).as_object() else {
        return Ok(JsValue::from(false));
    };
    // tri-state: absent ⇒ leave; null ⇒ clear; value ⇒ set.
    let prefix = {
        let v = obj.get(js_string!("prefix"), ctx)?;
        if v.is_undefined() {
            None
        } else if v.is_null() {
            Some(None)
        } else {
            Some(Some(v.to_string(ctx)?.to_std_string_escaped()))
        }
    };
    let start_at = {
        let v = obj.get(js_string!("start"), ctx)?;
        if v.is_undefined() {
            None
        } else if v.is_null() {
            Some(None)
        } else {
            Some(Some(v.to_number(ctx)? as u32))
        }
    };
    let numbering_style = prop_string(&obj, "style", ctx);
    Ok(apply_bool(&Mutation::EditSection {
        section_id,
        prefix,
        numbering_style,
        start_at,
    }))
}

/// `paged.deleteSection(sectionId)` (`Mutation::DeleteSection`).
fn paged_delete_section(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let section_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DeleteSection { section_id }))
}

// ---------------------------------------------------- conditions

/// `paged.setConditionVisible(conditionId, visible)`
/// (`Mutation::SetConditionVisible`).
fn paged_set_condition_visible(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let condition = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let visible = args.get_or_undefined(1).to_boolean();
    Ok(apply_bool(&Mutation::SetConditionVisible { condition, visible }))
}

/// `paged.activateConditionSet(setId)` (`Mutation::ActivateConditionSet`).
fn paged_activate_condition_set(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let set = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::ActivateConditionSet { set }))
}

// ---------------------------------------------------- layers

/// `paged.layerInsert(position, name)` — append a layer at the given
/// zero-based stacking index (`Mutation::LayerInsert`).
fn paged_layer_insert(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let position = args.get_or_undefined(0).to_number(ctx)? as u32;
    let name = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::LayerInsert { position, name }))
}

/// `paged.layerRemove(layerId)` (`Mutation::LayerRemove`).
fn paged_layer_remove(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let layer_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::LayerRemove { layer_id }))
}

/// `paged.layerMove(layerId, newIndex)` — reorder a layer
/// (`Mutation::LayerMove`).
fn paged_layer_move(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let layer_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let new_index = args.get_or_undefined(1).to_number(ctx)? as u32;
    Ok(apply_bool(&Mutation::LayerMove {
        layer_id,
        new_index,
    }))
}

// ---------------------------------------------------- guides

/// `paged.insertGuide(spreadId, orientation, position, pageIndex?)` —
/// insert a ruler guide (`orientation` = `"vertical"|"horizontal"`;
/// `Mutation::InsertGuide`).
fn paged_insert_guide(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let spread_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(orientation) =
        from_js::<paged_mutate::operation::GuideOrientationSpec>(args.get_or_undefined(1), ctx)
    else {
        return Ok(JsValue::from(false));
    };
    let position = args.get_or_undefined(2).to_number(ctx)? as f32;
    let page_index = opt_f32(args.get_or_undefined(3)).map_or(0, |n| n as u32);
    Ok(apply_bool(&Mutation::InsertGuide {
        spread_id,
        orientation,
        position,
        page_index,
    }))
}

/// `paged.moveGuide(guideId, position)` (`Mutation::MoveGuide`).
fn paged_move_guide(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let guide_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let position = args.get_or_undefined(1).to_number(ctx)? as f32;
    Ok(apply_bool(&Mutation::MoveGuide { guide_id, position }))
}

/// `paged.deleteGuide(guideId)` (`Mutation::DeleteGuide`).
fn paged_delete_guide(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let guide_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    Ok(apply_bool(&Mutation::DeleteGuide { guide_id }))
}

// ---------------------------------------------------- document defaults & colour

/// `paged.setDocumentDefaults({fill?,stroke?,weight?})` — set the
/// new-object fill/stroke/weight defaults (`Mutation::SetDocumentDefaults`;
/// whole-triple semantics — omitted fields become "no fill"/"no stroke"/
/// engine-default weight).
fn paged_set_document_defaults(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let (fill_color, stroke_color, stroke_weight) = match args.get_or_undefined(0).as_object() {
        Some(obj) => (
            prop_string(&obj, "fill", ctx),
            prop_string(&obj, "stroke", ctx),
            prop_f32(&obj, "weight", ctx),
        ),
        None => (None, None, None),
    };
    Ok(apply_bool(&Mutation::SetDocumentDefaults {
        fill_color,
        stroke_color,
        stroke_weight,
    }))
}

/// `paged.setColorSettings({cmykProfileName?,rgbPolicy?,intent?,bpc?})` —
/// replace the document colour-management settings
/// (`Mutation::SetColorSettings`).
fn paged_set_color_settings(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    #[derive(Default, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    struct Spec {
        cmyk_profile_name: Option<String>,
        rgb_policy: Option<String>,
        intent: Option<String>,
        bpc: Option<bool>,
    }
    let Some(spec) = from_js::<Spec>(args.get_or_undefined(0), ctx) else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::SetColorSettings {
        cmyk_profile_name: spec.cmyk_profile_name,
        rgb_policy: spec.rgb_policy,
        intent: spec.intent,
        bpc: spec.bpc,
    }))
}

/// `paged.setProofSetup({profileName?,simulatePaperWhite?,intent?})` —
/// soft-proofing configuration (`Mutation::SetProofSetup`).
fn paged_set_proof_setup(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    #[derive(Default, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    struct Spec {
        profile_name: Option<String>,
        simulate_paper_white: bool,
        intent: Option<String>,
    }
    let Some(spec) = from_js::<Spec>(args.get_or_undefined(0), ctx) else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::SetProofSetup {
        profile_name: spec.profile_name,
        simulate_paper_white: spec.simulate_paper_white,
        intent: spec.intent,
    }))
}

/// `paged.importSwatchLibrary(bytes, groupName?)` — import an `.ase`
/// library (`bytes` = a JS `number[]`; `Mutation::ImportSwatchLibrary`).
/// The `ByteBuf` field is filled by deserializing the whole mutation.
fn paged_import_swatch_library(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let Some(bytes_json) = to_json_value(args.get_or_undefined(0), ctx) else {
        return Ok(JsValue::from(false));
    };
    let group_name = opt_string(args.get_or_undefined(1), ctx);
    let m_json = serde_json::json!({
        "op": "importSwatchLibrary",
        "args": { "bytes": bytes_json, "groupName": group_name },
    });
    match serde_json::from_value::<Mutation>(m_json) {
        Ok(mutation) => Ok(apply_bool(&mutation)),
        Err(_) => Ok(JsValue::from(false)),
    }
}

/// `paged.setInkSetting(spotId, {convertToProcess?,aliasTo?})` — replace
/// one ink's output-time settings (`Mutation::SetInkSetting`).
fn paged_set_ink_setting(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    #[derive(Default, serde::Deserialize)]
    #[serde(rename_all = "camelCase", default)]
    struct Spec {
        convert_to_process: bool,
        alias_to: Option<String>,
    }
    let spot_id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let spec = from_js::<Spec>(args.get_or_undefined(1), ctx).unwrap_or_default();
    Ok(apply_bool(&Mutation::SetInkSetting {
        spot_id,
        convert_to_process: spec.convert_to_process,
        alias_to: spec.alias_to,
    }))
}

/// `paged.setUseStandardLabForSpots(enabled)`
/// (`Mutation::SetUseStandardLabForSpots`).
fn paged_set_use_standard_lab_for_spots(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> JsResult<JsValue> {
    let enabled = args.get_or_undefined(0).to_boolean();
    Ok(apply_bool(&Mutation::SetUseStandardLabForSpots { enabled }))
}

// ---------------------------------------------------- plugin metadata & batch

/// `paged.setPluginMetadata(elemId, key, value?, caller?)` — write one
/// `Label` key/value pair on a leaf page item (`value` null deletes;
/// `Mutation::SetPluginMetadata`).
fn paged_set_plugin_metadata(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::from(false));
    };
    let key = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let value = opt_string(args.get_or_undefined(2), ctx);
    let caller = opt_string(args.get_or_undefined(3), ctx);
    Ok(apply_bool(&Mutation::SetPluginMetadata {
        element_id,
        key,
        value,
        caller,
    }))
}

/// `paged.batch([mutations])` — apply an array of `{ op, args }` mutation
/// objects as ONE undoable step (`Mutation::Batch`). An unparseable array
/// returns `false`.
fn paged_batch(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(ops) = from_js::<Vec<Mutation>>(args.get_or_undefined(0), ctx) else {
        return Ok(JsValue::from(false));
    };
    Ok(apply_bool(&Mutation::Batch { ops }))
}

// ---------------------------------------------------- selection setters
//
// These set the worker model's selection state directly — they are
// application state, NOT undoable document mutations, so they do not go
// through `apply_mutation`.

/// `paged.setElementSelection([id, ...])` — replace the element selection
/// with the parseable ids. Always returns `true`.
fn paged_set_element_selection(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let ids = parse_element_id_array(args.get_or_undefined(0), ctx);
    with_model(|m| m.element_selection.ids = ids);
    Ok(JsValue::from(true))
}

/// `paged.clearSelection()` — clear the element selection.
fn paged_clear_selection(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> JsResult<JsValue> {
    with_model(|m| m.element_selection.clear());
    Ok(JsValue::from(true))
}

/// `paged.setContentSelection({storyId,start,end} | null)` — set or clear
/// the text caret/range. Returns `false` if a non-null arg is not a valid
/// `ContentSelection` shape.
fn paged_set_content_selection(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let value = args.get_or_undefined(0);
    if value.is_undefined() || value.is_null() {
        with_model(|m| m.current_selection = None);
        return Ok(JsValue::from(true));
    }
    let Some(sel) = from_js::<paged_canvas::selection::ContentSelection>(value, ctx) else {
        return Ok(JsValue::from(false));
    };
    with_model(|m| m.current_selection = Some(sel));
    Ok(JsValue::from(true))
}

fn paged_get(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let path = args
        .get_or_undefined(1)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::null());
    };
    let entry = with_model(|m| {
        m.element_properties(&element_id).and_then(|props| {
            props
                .entries
                .into_iter()
                .find(|e| property_path_label(e.path) == path)
        })
    });
    match entry {
        Some(e) => Ok(JsValue::from(js_string!(
            serde_json::to_string(&e.value).unwrap_or_default()
        ))),
        None => Ok(JsValue::null()),
    }
}

fn paged_undo(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(with_model(|m| m.undo().is_some())))
}

fn paged_redo(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(with_model(|m| m.redo().is_some())))
}

fn paged_inspect(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(element_id) = parse_element_id(&id) else {
        return Ok(JsValue::null());
    };
    let payload = with_model(|m| {
        m.element_properties(&element_id)
            .map(|p| serde_json::to_string(&p).unwrap_or_default())
    });
    match payload {
        Some(s) => Ok(JsValue::from(js_string!(s))),
        None => Ok(JsValue::null()),
    }
}

fn paged_layers(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.layers()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

fn paged_tree(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.scene_tree()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// SDK Phase 3 — returns the loaded document's stories as a JSON-
/// encoded `StorySummary[]`. Each entry carries `selfId`,
/// `characterCount`, `paragraphCount`. Scripts use this to pick
/// valid `StoryRange` addresses; tests use it to populate the
/// content selection programmatically.
fn paged_stories(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.stories()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// `paged.pages()` — the loaded document's pages as a JSON-encoded
/// `PageSummary[]` (each carries `selfId`, a 1-based `index`, and
/// `sizePt`). `selfId` is the page id accepted by `insertFrame`,
/// `insertTextFrame`, and `insertPage` (and the `afterPageId` of
/// `insertPage`). Pages are not addressable elements (they carry
/// `id:null` in `paged.tree()`), so this is the only way a script can
/// obtain a usable page id.
fn paged_pages(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.pages()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// SDK Phase 3 — returns the loaded document's swatch palette as
/// a JSON-encoded `SwatchSummary[]`. First implementation of the
/// `documentCollection` read kind per
/// `docs/paged/panel-catalog-and-sdk-extension.md` §5.1; the same
/// shape the (future) UI `paged.collection("swatches")` consumes.
fn paged_swatches(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.swatches()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

fn paged_paragraph_styles(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.paragraph_styles()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

fn paged_character_styles(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.character_styles()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// SDK Phase 5 (v1 sweep) — `paged.objectStyles()` legacy-shape
/// alias for `paged.collection("objectStyles")`. The generic
/// `paged.collection(name)` is the canonical entry point; this
/// alias mirrors the existing `paragraphStyles` / `characterStyles`
/// style and stays for back-compat with scripts that learn the
/// per-collection name.
fn paged_object_styles(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.object_styles()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// SDK Phase 5 (v1 sweep) — `paged.links()` legacy-shape alias for
/// `paged.collection("links")`.
fn paged_links(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.links()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// SDK Phase 5 (v1 sweep) — `paged.conditions()` legacy-shape
/// alias for `paged.collection("conditions")`.
fn paged_conditions(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.conditions()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// SDK Phase 5 (v1 sweep) — `paged.conditionSets()` alias.
fn paged_condition_sets(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.condition_sets()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// SDK Phase 5 (v1 sweep) — `paged.colorGroups()` alias.
fn paged_color_groups(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.color_groups()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

fn paged_gradients(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.gradients()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// SDK Phase 5 (D1) — generic typed-collection read. Backs the
/// `documentCollection:<name>` ReadSpec end of the §5 binding
/// ceiling and the matching `client.collection(name)` TS API.
///
/// Returns a JSON-encoded array matching the typed `*Summary[]`
/// for the requested collection — `SwatchSummary[]` for
/// `"swatches"`, `ParagraphStyleSummary[]` for
/// `"paragraphStyles"`, etc. Unknown collection names return
/// `"[]"` (NOT `null`) with a console warning, so consumers'
/// typed arrays stay valid; this matches the model dispatcher's
/// empty-for-unwired semantics. The convergence thesis (sdk.md
/// §11.1) means this fn and the UI's `useCollection` hook reach
/// the same `CanvasModel::collection(name)` source — one truth
/// for both call sites.
fn paged_collection(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let name_str = args
        .get_or_undefined(0)
        .to_string(ctx)?
        .to_std_string_escaped();
    let Some(name) = paged_canvas::channel::CollectionName::from_str(&name_str) else {
        push_output(format!(
            "[warn] paged.collection(\"{name_str}\") — unknown collection; returning []"
        ));
        return Ok(JsValue::from(js_string!("[]")));
    };
    let s = with_model(|m| serde_json::to_string(&m.collection(name)).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// SDK Phase 5 (D1) — singleton document-state snapshot. Backs
/// the `documentMeta:<key>` ReadSpec form and the
/// `client.documentMeta()` TS API. Returns a JSON-encoded
/// `DocumentMeta` (the six §5.6 fields).
fn paged_document_meta(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.document_meta()).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// Returns the current element-selection set as a JSON-encoded
/// `ElementId[]`. Empty selection yields `"[]"` — never `null` —
/// to mirror the always-present array shape the UI consumes via
/// `useElementSelection()`. Application state, not document state:
/// reads do not enter the Operation log, and the caller is expected
/// to re-poll on `mutationApplied` if it wants to react to changes.
fn paged_selection(_this: &JsValue, _args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let s = with_model(|m| serde_json::to_string(&m.element_selection.ids).unwrap_or_default());
    Ok(JsValue::from(js_string!(s)))
}

/// Returns the current text-side selection (caret or range) as a
/// JSON-encoded `ContentSelection`, or JS `null` when there is none.
/// Same shape `client.setSelection` accepts on the way in.
fn paged_content_selection(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> JsResult<JsValue> {
    let payload = with_model(|m| {
        m.current_selection
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_default())
    });
    match payload {
        Some(s) => Ok(JsValue::from(js_string!(s))),
        None => Ok(JsValue::null()),
    }
}

// ---------------------------------------------------------------- console.*

fn console_log(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    emit_console("log", args, ctx);
    Ok(JsValue::undefined())
}
fn console_warn(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    emit_console("warn", args, ctx);
    Ok(JsValue::undefined())
}
fn console_error(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    emit_console("error", args, ctx);
    Ok(JsValue::undefined())
}
fn console_info(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    emit_console("info", args, ctx);
    Ok(JsValue::undefined())
}

fn emit_console(level: &str, args: &[JsValue], ctx: &mut Context) {
    let parts: Vec<String> = args.iter().map(|v| format_value(v, ctx)).collect();
    push_output(format!("[{level}] {}", parts.join(" ")));
}

// ---------------------------------------------------------------- parsing

fn parse_element_id(s: &str) -> Option<paged_canvas::element_selection::ElementId> {
    use paged_canvas::element_selection::ElementId;
    let (kind, id) = s.split_once(':')?;
    if id.is_empty() {
        return None;
    }
    // SDK Phase 3 — `storyRange:Story/u1@0..6` addresses a character
    // range. The id payload is `<story_id>@<start>..<end>` where
    // start + end are unsigned character offsets and end > start.
    if kind == "storyRange" || kind == "storyrange" {
        let (story_id, range) = id.split_once('@')?;
        if story_id.is_empty() {
            return None;
        }
        let (start_s, end_s) = range.split_once("..")?;
        let start: u32 = start_s.parse().ok()?;
        let end: u32 = end_s.parse().ok()?;
        if end <= start {
            return None;
        }
        return Some(ElementId::StoryRange {
            story_id: story_id.to_string(),
            start,
            end,
        });
    }
    let id = id.to_string();
    Some(match kind {
        "textFrame" | "textframe" => ElementId::TextFrame(id),
        "rectangle" | "rect" => ElementId::Rectangle(id),
        "oval" => ElementId::Oval(id),
        "polygon" => ElementId::Polygon(id),
        "graphicLine" | "graphicline" => ElementId::GraphicLine(id),
        "group" => ElementId::Group(id),
        _ => return None,
    })
}

/// Inverse of `parse_element_id` for page-item variants: render an
/// `ElementId` as the `kind:id` address a subsequent `paged.set` /
/// `paged.inspect` accepts. The structural insert fns only ever mint a
/// page item (TextFrame/Rectangle/Oval/Polygon/GraphicLine/Group), so the
/// non-page-item variants fall back to the bare `raw_id` (not a round-trip
/// address, but never produced here).
fn element_id_to_address(id: &paged_canvas::element_selection::ElementId) -> String {
    use paged_canvas::element_selection::ElementId::*;
    match id {
        TextFrame(i) => format!("textFrame:{i}"),
        Rectangle(i) => format!("rectangle:{i}"),
        Oval(i) => format!("oval:{i}"),
        Polygon(i) => format!("polygon:{i}"),
        GraphicLine(i) => format!("graphicLine:{i}"),
        Group(i) => format!("group:{i}"),
        other => other.raw_id().to_string(),
    }
}

fn parse_property_path(s: &str) -> Option<paged_mutate::PropertyPath> {
    // Single source of truth: the JS-name -> PropertyPath table owned by
    // `paged-introspect`, which also backs `api_catalog().settable_paths`
    // (ADR 019). No second list.
    paged_introspect::lookup_path(s)
}

fn property_path_label(path: paged_mutate::PropertyPath) -> &'static str {
    use paged_mutate::PropertyPath::*;
    match path {
        FrameBounds => "frameBounds",
        FrameFillColor => "frameFillColor",
        FrameStrokeColor => "frameStrokeColor",
        FrameStrokeWeight => "frameStrokeWeight",
        FrameOpacity => "frameOpacity",
        FrameTransform => "frameTransform",
        ImageContentTransform => "imageContentTransform",
        FramePathPoint => "framePathPoint",
        PathPointInsert => "pathPointInsert",
        PathPointRemove => "pathPointRemove",
        PathPointCurveType => "pathPointCurveType",
        LayerVisible => "layerVisible",
        LayerLocked => "layerLocked",
        LayerPrintable => "layerPrintable",
        LayerName => "layerName",
        CharacterFontSize => "characterFontSize",
        CharacterLeading => "characterLeading",
        CharacterTracking => "characterTracking",
        CharacterFillColor => "characterFillColor",
        ParagraphSpaceBefore => "paragraphSpaceBefore",
        ParagraphSpaceAfter => "paragraphSpaceAfter",
        ParagraphFirstLineIndent => "paragraphFirstLineIndent",
        AppliedParagraphStyle => "appliedParagraphStyle",
        AppliedCharacterStyle => "appliedCharacterStyle",
        AppliedObjectStyle => "appliedObjectStyle",
        AppliedCellStyle => "appliedCellStyle",
        AppliedTableStyle => "appliedTableStyle",
        AppliedConditions => "appliedConditions",
        FrameInsetSpacing => "frameInsetSpacing",
        ParagraphJustification => "paragraphJustification",
        ParagraphStyleNextStyle => "paragraphStyleNextStyle",
        ParagraphAppliedNumberingList => "paragraphAppliedNumberingList",
        FrameStrokeEndCap => "frameStrokeEndCap",
        FrameStrokeStartArrowhead => "frameStrokeStartArrowhead",
        FrameStrokeEndArrowhead => "frameStrokeEndArrowhead",
        FrameTextWrapMode => "frameTextWrapMode",
        FrameTextWrapOffsets => "frameTextWrapOffsets",
        FrameTextWrapContourType => "frameTextWrapContourType",
        FrameTextWrapContourIncludeInside => "frameTextWrapContourIncludeInside",
        FrameFittingCrops => "frameFittingCrops",
        FrameFittingType => "frameFittingType",
        FrameDropShadow => "frameDropShadow",
        FrameDropShadowMode => "frameDropShadowMode",
        FrameDropShadowXOffset => "frameDropShadowXOffset",
        FrameDropShadowYOffset => "frameDropShadowYOffset",
        FrameDropShadowSize => "frameDropShadowSize",
        FrameDropShadowOpacity => "frameDropShadowOpacity",
        FrameDropShadowColor => "frameDropShadowColor",
        FramePath => "framePath",
        FrameFillTint => "frameFillTint",
        FrameNonprinting => "frameNonprinting",
        FrameGradientFillAngle => "frameGradientFillAngle",
        FrameGradientFillLength => "frameGradientFillLength",
        FrameGradientStrokeAngle => "frameGradientStrokeAngle",
        FrameGradientStrokeLength => "frameGradientStrokeLength",
        PathOpenAt => "pathOpenAt",
        OutlineStroke => "outlineStroke",
        OutlineStrokeVariable => "outlineStrokeVariable",
        OffsetPath => "offsetPath",
        SimplifyPath => "simplifyPath",
        PageBounds => "pageBounds",
        FrameGradientFeather => "frameGradientFeather",
        CharacterFontFamily => "characterFontFamily",
        CharacterFontStyle => "characterFontStyle",
        CharacterKerningMethod => "characterKerningMethod",
        CharacterCase => "characterCase",
        CharacterPosition => "characterPosition",
        CharacterLanguage => "characterLanguage",
        CharacterBaselineShift => "characterBaselineShift",
        CharacterHorizontalScale => "characterHorizontalScale",
        CharacterVerticalScale => "characterVerticalScale",
        CharacterSkew => "characterSkew",
        CharacterUnderline => "characterUnderline",
        CharacterStrikethru => "characterStrikethru",
        CharacterLigatures => "characterLigatures",
        CharacterOtfFeatures => "characterOtfFeatures",
        ParagraphLeftIndent => "paragraphLeftIndent",
        ParagraphRightIndent => "paragraphRightIndent",
        ParagraphDropCapCharacters => "paragraphDropCapCharacters",
        ParagraphDropCapLines => "paragraphDropCapLines",
        ParagraphHyphenation => "paragraphHyphenation",
        ParagraphKeepLinesTogether => "paragraphKeepLinesTogether",
        ParagraphKeepWithNext => "paragraphKeepWithNext",
        ParagraphRuleAbove => "paragraphRuleAbove",
        ParagraphRuleBelow => "paragraphRuleBelow",
        ParagraphTabStops => "paragraphTabStops",
        ParagraphListType => "paragraphListType",
        ParagraphBulletCharacter => "paragraphBulletCharacter",
        ParagraphNumberingFormat => "paragraphNumberingFormat",
        // W0.3.
        TextFrameColumnCount => "textFrameColumnCount",
        TextFrameColumnGutter => "textFrameColumnGutter",
        TextFrameColumnBalance => "textFrameColumnBalance",
        TextFrameVerticalJustification => "textFrameVerticalJustification",
        TextFrameAutoSizing => "textFrameAutoSizing",
        TextFrameFirstBaseline => "textFrameFirstBaseline",
        TextWrapInvert => "frameTextWrapInvert",
        FrameFittingReferencePoint => "frameFittingReferencePoint",
        FrameAutoFit => "frameAutoFit",
        FrameStrokeType => "frameStrokeType",
        FrameStrokeJoin => "frameStrokeJoin",
        FrameStrokeMiterLimit => "frameStrokeMiterLimit",
        FrameStrokeAlignment => "frameStrokeAlignment",
        FrameStrokeGapColor => "frameStrokeGapColor",
        FrameStrokeGapTint => "frameStrokeGapTint",
        FrameStrokeDashArray => "frameStrokeDashArray",
        FrameCornerOptionTopLeft => "frameCornerOptionTopLeft",
        FrameCornerOptionTopRight => "frameCornerOptionTopRight",
        FrameCornerOptionBottomLeft => "frameCornerOptionBottomLeft",
        FrameCornerOptionBottomRight => "frameCornerOptionBottomRight",
        FrameCornerRadiusTopLeft => "frameCornerRadiusTopLeft",
        FrameCornerRadiusTopRight => "frameCornerRadiusTopRight",
        FrameCornerRadiusBottomLeft => "frameCornerRadiusBottomLeft",
        FrameCornerRadiusBottomRight => "frameCornerRadiusBottomRight",
        FrameRotationAngle => "frameRotationAngle",
        FrameScaleX => "frameScaleX",
        FrameScaleY => "frameScaleY",
        FrameFlipH => "frameFlipH",
        FrameFlipV => "frameFlipV",
        FrameOverprintFill => "frameOverprintFill",
        FrameOverprintStroke => "frameOverprintStroke",
        // W0.4 — transparency effects.
        FrameInnerShadowEnabled => "frameInnerShadow",
        FrameInnerShadowBlendMode => "frameInnerShadowBlendMode",
        FrameInnerShadowColor => "frameInnerShadowColor",
        FrameInnerShadowOpacity => "frameInnerShadowOpacity",
        FrameInnerShadowAngle => "frameInnerShadowAngle",
        FrameInnerShadowDistance => "frameInnerShadowDistance",
        FrameInnerShadowSize => "frameInnerShadowSize",
        FrameInnerShadowChoke => "frameInnerShadowChoke",
        FrameInnerShadowNoise => "frameInnerShadowNoise",
        FrameOuterGlowEnabled => "frameOuterGlow",
        FrameOuterGlowBlendMode => "frameOuterGlowBlendMode",
        FrameOuterGlowColor => "frameOuterGlowColor",
        FrameOuterGlowOpacity => "frameOuterGlowOpacity",
        FrameOuterGlowSpread => "frameOuterGlowSpread",
        FrameOuterGlowSize => "frameOuterGlowSize",
        FrameOuterGlowNoise => "frameOuterGlowNoise",
        FrameInnerGlowEnabled => "frameInnerGlow",
        FrameInnerGlowBlendMode => "frameInnerGlowBlendMode",
        FrameInnerGlowColor => "frameInnerGlowColor",
        FrameInnerGlowOpacity => "frameInnerGlowOpacity",
        FrameInnerGlowChoke => "frameInnerGlowChoke",
        FrameInnerGlowSize => "frameInnerGlowSize",
        FrameInnerGlowSource => "frameInnerGlowSource",
        FrameInnerGlowNoise => "frameInnerGlowNoise",
        FrameBevelEnabled => "frameBevel",
        FrameBevelStyle => "frameBevelStyle",
        FrameBevelTechnique => "frameBevelTechnique",
        FrameBevelDepth => "frameBevelDepth",
        FrameBevelDirection => "frameBevelDirection",
        FrameBevelSize => "frameBevelSize",
        FrameBevelSoften => "frameBevelSoften",
        FrameBevelAngle => "frameBevelAngle",
        FrameBevelAltitude => "frameBevelAltitude",
        FrameBevelHighlightColor => "frameBevelHighlightColor",
        FrameBevelShadowColor => "frameBevelShadowColor",
        FrameBevelHighlightOpacity => "frameBevelHighlightOpacity",
        FrameBevelShadowOpacity => "frameBevelShadowOpacity",
        FrameSatinEnabled => "frameSatin",
        FrameSatinBlendMode => "frameSatinBlendMode",
        FrameSatinColor => "frameSatinColor",
        FrameSatinOpacity => "frameSatinOpacity",
        FrameSatinAngle => "frameSatinAngle",
        FrameSatinDistance => "frameSatinDistance",
        FrameSatinSize => "frameSatinSize",
        FrameSatinInvert => "frameSatinInvert",
        FrameFeatherEnabled => "frameFeather",
        FrameFeatherWidth => "frameFeatherWidth",
        FrameFeatherCornerType => "frameFeatherCornerType",
        FrameFeatherNoise => "frameFeatherNoise",
        FrameFeatherChoke => "frameFeatherChoke",
        FrameDirectionalFeatherEnabled => "frameDirectionalFeather",
        FrameDirectionalFeatherLeftWidth => "frameDirectionalFeatherLeftWidth",
        FrameDirectionalFeatherRightWidth => "frameDirectionalFeatherRightWidth",
        FrameDirectionalFeatherTopWidth => "frameDirectionalFeatherTopWidth",
        FrameDirectionalFeatherBottomWidth => "frameDirectionalFeatherBottomWidth",
        FrameDirectionalFeatherAngle => "frameDirectionalFeatherAngle",
        FrameDirectionalFeatherNoise => "frameDirectionalFeatherNoise",
        FrameDirectionalFeatherChoke => "frameDirectionalFeatherChoke",
        FrameBlendMode => "frameBlendMode",
        // W3.A0 — text-frame thread chain (read-only paths).
        NextTextFrame => "nextTextFrame",
        PreviousTextFrame => "previousTextFrame",
        // W3.A1 — table cell properties.
        CellFillColor => "cellFillColor",
        CellFillTint => "cellFillTint",
        CellInsetTop => "cellInsetTop",
        CellInsetLeft => "cellInsetLeft",
        CellInsetBottom => "cellInsetBottom",
        CellInsetRight => "cellInsetRight",
        CellVerticalJustification => "cellVerticalJustification",
        CellTopEdgeStrokeColor => "cellTopEdgeStrokeColor",
        CellTopEdgeStrokeWeight => "cellTopEdgeStrokeWeight",
        CellTopEdgeStrokeTint => "cellTopEdgeStrokeTint",
        CellBottomEdgeStrokeColor => "cellBottomEdgeStrokeColor",
        CellBottomEdgeStrokeWeight => "cellBottomEdgeStrokeWeight",
        CellBottomEdgeStrokeTint => "cellBottomEdgeStrokeTint",
        CellLeftEdgeStrokeColor => "cellLeftEdgeStrokeColor",
        CellLeftEdgeStrokeWeight => "cellLeftEdgeStrokeWeight",
        CellLeftEdgeStrokeTint => "cellLeftEdgeStrokeTint",
        CellRightEdgeStrokeColor => "cellRightEdgeStrokeColor",
        CellRightEdgeStrokeWeight => "cellRightEdgeStrokeWeight",
        CellRightEdgeStrokeTint => "cellRightEdgeStrokeTint",
        // Aftercare-A — table dimensions (read-only).
        TableRowCount => "tableRowCount",
        TableColumnCount => "tableColumnCount",
        PluginMetadata => "pluginMetadata",
        // W1.16 — anchored-object settings.
        AnchoredPosition => "anchoredPosition",
        AnchorPoint => "anchorPoint",
        AnchoredXOffset => "anchoredXOffset",
        AnchoredYOffset => "anchoredYOffset",
        AnchoredHorizontalReference => "anchoredHorizontalReference",
        AnchoredVerticalReference => "anchoredVerticalReference",
        AnchoredHorizontalAlignment => "anchoredHorizontalAlignment",
        AnchoredVerticalAlignment => "anchoredVerticalAlignment",
        AnchoredSpineRelative => "anchoredSpineRelative",
        AnchoredLockPosition => "anchoredLockPosition",
        // W2.5 — element-level visibility / lock.
        ElementVisible => "elementVisible",
        ElementLocked => "elementLocked",
    }
}

fn js_value_to_wire(
    value: &JsValue,
    path: paged_mutate::PropertyPath,
    ctx: &mut Context,
) -> Option<paged_mutate::Value> {
    use paged_mutate::PropertyPath as P;
    use paged_mutate::Value as W;
    if let Some(b) = value.as_boolean() {
        return Some(W::Bool(b));
    }
    if let Some(n) = value.as_number() {
        return Some(W::Length(Some(n as f32)));
    }
    if value.is_null() {
        return Some(W::Length(None));
    }
    if let Some(s) = value.as_string() {
        let s = s.to_std_string_escaped();
        // Path-aware string encoding. The wire `Value` enum has two
        // string-ish variants (`Text`, `ColorRef`); the apply layer
        // raises `TypeMismatch` if we pick wrong, so the bridge has
        // to know the path's expected variant. We default to
        // `ColorRef` for back-compat with pre-Track-A scripts that
        // wrote `paged.set(id, "frameFillColor", "Color/Red")`;
        // explicit Text-bearing paths get the matching variant.
        return Some(match path {
            // Applied-entity refs (D3 paths) — Value::Text payload.
            P::AppliedParagraphStyle
            | P::AppliedCharacterStyle
            | P::AppliedObjectStyle
            | P::AppliedCellStyle
            | P::AppliedTableStyle
            | P::AppliedConditions
            | P::LayerName
            // Enum strings (v1 sweep).
            | P::ParagraphJustification
            | P::FrameStrokeEndCap
            | P::FrameStrokeStartArrowhead
            | P::FrameStrokeEndArrowhead
            | P::FrameTextWrapMode
            | P::FrameTextWrapContourType
            | P::FrameFittingType
            | P::FrameDropShadowMode
            // W0.1 — character text / enum-string paths.
            | P::CharacterFontFamily
            | P::CharacterFontStyle
            | P::CharacterKerningMethod
            | P::CharacterCase
            | P::CharacterPosition
            | P::CharacterLanguage
            | P::CharacterOtfFeatures
            // W0.2 — paragraph text / enum-string paths. The
            // whole-struct rule paths and the tab-stop list arrive as
            // `{ type, value }` objects, handled by the JSON
            // round-trip branch below — not here.
            | P::ParagraphListType
            | P::ParagraphBulletCharacter
            | P::ParagraphNumberingFormat
            // W1.22 — applied numbering-list ref + next-style ref.
            | P::ParagraphAppliedNumberingList
            | P::ParagraphStyleNextStyle
            // W0.3 — enum-string / text frame-scope paths.
            | P::TextFrameVerticalJustification
            | P::TextFrameAutoSizing
            | P::TextFrameFirstBaseline
            | P::FrameFittingReferencePoint
            | P::FrameStrokeType
            | P::FrameStrokeJoin
            | P::FrameStrokeAlignment
            | P::FrameCornerOptionTopLeft
            | P::FrameCornerOptionTopRight
            | P::FrameCornerOptionBottomLeft
            | P::FrameCornerOptionBottomRight
            // W0.4 — transparency-effect enum / blend-mode strings.
            | P::FrameInnerShadowBlendMode
            | P::FrameOuterGlowBlendMode
            | P::FrameInnerGlowBlendMode
            | P::FrameInnerGlowSource
            | P::FrameBevelStyle
            | P::FrameBevelTechnique
            | P::FrameBevelDirection
            | P::FrameSatinBlendMode
            | P::FrameFeatherCornerType
            | P::FrameBlendMode
            // W3.A1 — cell vertical-justify enum string.
            | P::CellVerticalJustification
            // W1.16 — anchored-object enum-string settings.
            | P::AnchoredPosition
            | P::AnchorPoint
            | P::AnchoredHorizontalReference
            | P::AnchoredVerticalReference
            | P::AnchoredHorizontalAlignment
            | P::AnchoredVerticalAlignment => W::Text(s),
            // Color-ref paths.
            P::FrameFillColor
            | P::FrameStrokeColor
            | P::CharacterFillColor
            | P::FrameDropShadowColor
            // W3.A1 — cell fill colour ref.
            | P::CellFillColor
            // W1.11b — per-cell edge-stroke colour refs (weight / tint
            // arrive as numbers, handled by the `as_number` branch above).
            | P::CellTopEdgeStrokeColor
            | P::CellBottomEdgeStrokeColor
            | P::CellLeftEdgeStrokeColor
            | P::CellRightEdgeStrokeColor
            // W0.3 — stroke gap colour.
            | P::FrameStrokeGapColor
            // W0.4 — transparency-effect colour refs.
            | P::FrameInnerShadowColor
            | P::FrameOuterGlowColor
            | P::FrameInnerGlowColor
            | P::FrameBevelHighlightColor
            | P::FrameBevelShadowColor
            | P::FrameSatinColor => W::ColorRef(Some(s)),
            // Anything else gets ColorRef as the legacy default —
            // callers passing other typed strings should use the
            // explicit `{ type, value }` wrapper which the
            // `to_json`-round-trip branch below handles.
            _ => W::ColorRef(Some(s)),
        });
    }
    if let Some(obj) = value.as_object() {
        if obj.is_array() {
            let len = obj
                .get(js_string!("length"), ctx)
                .ok()?
                .to_length(ctx)
                .ok()? as usize;
            if len == 4 {
                let mut out = [0.0f32; 4];
                for (i, slot) in out.iter_mut().enumerate() {
                    let v = obj.get(i as u32, ctx).ok()?;
                    *slot = v.as_number()? as f32;
                }
                return Some(W::Bounds(out));
            }
            if len == 6 {
                let mut out = [0.0f32; 6];
                for (i, slot) in out.iter_mut().enumerate() {
                    let v = obj.get(i as u32, ctx).ok()?;
                    *slot = v.as_number()? as f32;
                }
                return Some(W::Transform(Some(out)));
            }
        }
        // `{ type, value }` shape — round-trip via JSON.
        let json = value.to_json(ctx).ok()??;
        return serde_json::from_value::<paged_mutate::Value>(json).ok();
    }
    None
}

// ---------------------------------------------------------------- formatting

/// If `obj` is an `ElementId`-shaped object (`{kind: string, id: string}`),
/// return its `kind:id` address string. Used to pretty-print parsed element
/// ids in `console.log`.
fn element_id_address_of(obj: &boa_engine::object::JsObject, ctx: &mut Context) -> Option<String> {
    let kind = obj.get(js_string!("kind"), ctx).ok()?;
    let id = obj.get(js_string!("id"), ctx).ok()?;
    let kind = kind.as_string()?.to_std_string_escaped();
    let id = id.as_string()?.to_std_string_escaped();
    if kind.is_empty() || id.is_empty() {
        return None;
    }
    Some(format!("{kind}:{id}"))
}

/// Render a JS array as `[addr, addr, …]` iff every element is
/// `ElementId`-shaped; otherwise `None` (so the caller falls back to JSON
/// and non-element arrays keep their compact-JSON formatting).
fn render_element_id_array(obj: &boa_engine::object::JsObject, ctx: &mut Context) -> Option<String> {
    let len = obj.get(js_string!("length"), ctx).ok()?.as_number()? as usize;
    if len == 0 {
        return None;
    }
    let mut parts = Vec::with_capacity(len);
    for i in 0..len {
        let el = obj.get(i as u32, ctx).ok()?;
        let el_obj = el.as_object()?;
        parts.push(element_id_address_of(&el_obj, ctx)?);
    }
    Some(format!("[{}]", parts.join(", ")))
}

fn format_value(value: &JsValue, ctx: &mut Context) -> String {
    if value.is_undefined() {
        return "undefined".to_string();
    }
    if value.is_null() {
        return "null".to_string();
    }
    if let Some(b) = value.as_boolean() {
        return b.to_string();
    }
    if let Some(n) = value.as_number() {
        if n.is_finite() && n == n.trunc() && n.abs() < 1e15 {
            return (n as i64).to_string();
        }
        return n.to_string();
    }
    if let Some(s) = value.as_string() {
        return s.to_std_string_escaped();
    }
    if let Some(obj) = value.as_object() {
        // An `ElementId`-shaped object `{kind, id}` — what
        // `JSON.parse(paged.selection())[0]` yields — reads far better as its
        // `kind:id` address than as raw JSON, so `console.log('sel', sel[0])`
        // prints `sel textFrame:u123`.
        if let Some(addr) = element_id_address_of(&obj, ctx) {
            return addr;
        }
        // An array of `ElementId`s (e.g. the parsed selection) prints as
        // `[textFrame:u1, rectangle:u2]`. Only arrays whose every element is
        // ElementId-shaped take this path; all other arrays fall through to
        // JSON so numbers/strings are unaffected.
        if obj.is_array() {
            if let Some(rendered) = render_element_id_array(&obj, ctx) {
                return rendered;
            }
        }
        // Error-shaped objects: pull `.name`, `.message`, `.stack`.
        let name_v = obj.get(js_string!("name"), ctx).ok();
        let msg_v = obj.get(js_string!("message"), ctx).ok();
        let has_msg = msg_v
            .as_ref()
            .is_some_and(|v| !v.is_undefined() && !v.is_null());
        if has_msg {
            let mut head = String::new();
            if let Some(nv) = name_v {
                if !nv.is_undefined() && !nv.is_null() {
                    head.push_str(&format_value(&nv, ctx));
                }
            }
            let msg_s = format_value(msg_v.as_ref().unwrap(), ctx);
            if !head.is_empty() {
                head.push_str(": ");
            }
            head.push_str(&msg_s);
            if let Ok(stack) = obj.get(js_string!("stack"), ctx) {
                if !stack.is_undefined() && !stack.is_null() {
                    return format!("{head}\n{}", format_value(&stack, ctx));
                }
            }
            return head;
        }
    }
    // Fallback: JSON.stringify equivalent via Boa's to_json.
    if let Ok(Some(json)) = value.to_json(ctx) {
        return serde_json::to_string(&json).unwrap_or_else(|_| "[opaque]".to_string());
    }
    "[opaque]".to_string()
}

fn format_error(err: &boa_engine::JsError, ctx: &mut Context) -> String {
    // B-09 — a tripped RuntimeLimit is a NATIVE error that Boa
    // refuses to convert to an opaque JS value (`to_opaque` PANICS on
    // it — which would abort the wasm worker, the exact hang-class
    // the budget exists to prevent). Surface it as a plain string
    // before touching the opaque path.
    if let Some(native) = err.as_native() {
        if native.is_runtime_limit() {
            return format!("runtime budget exceeded: {native}");
        }
    }
    // Boa exposes the thrown value via `to_opaque(ctx)`; reuse the
    // value formatter so Error objects come out as "Name: message".
    let opaque = err.to_opaque(ctx);
    format_value(&opaque, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `parse_property_path` now delegates to the single `paged_introspect::catalog::PROPERTY_PATHS`
    /// table, so every table entry resolves through the public parser to the
    /// table's own variant — the parser and the catalog cannot disagree.
    #[test]
    fn parser_matches_the_catalog_table() {
        for (name, path) in paged_introspect::catalog::PROPERTY_PATHS {
            assert_eq!(
                parse_property_path(name),
                Some(*path),
                "parse_property_path({name:?}) disagreed with the catalog table"
            );
        }
    }

    /// The single table lists no duplicate JS names.
    #[test]
    fn settable_paths_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (name, _) in paged_introspect::catalog::PROPERTY_PATHS {
            assert!(seen.insert(*name), "duplicate catalog path {name:?}");
        }
    }

    /// Spot-check the non-obvious short-name → `*Enabled` (and renamed) mappings
    /// the table transcription had to get right — a guard against a future
    /// hand-edit silently breaking them.
    #[test]
    fn known_mappings_are_stable() {
        use paged_mutate::PropertyPath as P;
        assert_eq!(parse_property_path("characterFontSize"), Some(P::CharacterFontSize));
        assert_eq!(parse_property_path("frameBevel"), Some(P::FrameBevelEnabled));
        assert_eq!(parse_property_path("frameTextWrapInvert"), Some(P::TextWrapInvert));
        assert_eq!(parse_property_path("frameInnerGlow"), Some(P::FrameInnerGlowEnabled));
        assert_eq!(parse_property_path("notARealPath"), None);
    }
}
