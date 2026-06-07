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
use paged_canvas::channel::Mutation;
use paged_canvas::CanvasModel;
use serde::{Deserialize, Serialize};

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
        .function(guarded(paged_undo), js_string!("undo"), 0)
        .function(guarded(paged_redo), js_string!("redo"), 0)
        .function(guarded(paged_inspect), js_string!("inspect"), 1)
        .function(guarded(paged_layers), js_string!("layers"), 0)
        .function(guarded(paged_tree), js_string!("tree"), 0)
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

fn parse_property_path(s: &str) -> Option<paged_mutate::PropertyPath> {
    use paged_mutate::PropertyPath::*;
    Some(match s {
        "frameBounds" => FrameBounds,
        "frameFillColor" => FrameFillColor,
        "frameStrokeColor" => FrameStrokeColor,
        "frameStrokeWeight" => FrameStrokeWeight,
        "frameOpacity" => FrameOpacity,
        "frameTransform" => FrameTransform,
        "imageContentTransform" => ImageContentTransform,
        "framePathPoint" => FramePathPoint,
        "pathPointInsert" => PathPointInsert,
        "pathPointRemove" => PathPointRemove,
        "pathPointCurveType" => PathPointCurveType,
        "layerVisible" => LayerVisible,
        "layerLocked" => LayerLocked,
        "layerPrintable" => LayerPrintable,
        "layerName" => LayerName,
        "characterFontSize" => CharacterFontSize,
        "characterLeading" => CharacterLeading,
        "characterTracking" => CharacterTracking,
        "characterFillColor" => CharacterFillColor,
        "paragraphSpaceBefore" => ParagraphSpaceBefore,
        "paragraphSpaceAfter" => ParagraphSpaceAfter,
        "paragraphFirstLineIndent" => ParagraphFirstLineIndent,
        "appliedParagraphStyle" => AppliedParagraphStyle,
        "appliedCharacterStyle" => AppliedCharacterStyle,
        "appliedObjectStyle" => AppliedObjectStyle,
        "appliedCellStyle" => AppliedCellStyle,
        "appliedTableStyle" => AppliedTableStyle,
        "appliedConditions" => AppliedConditions,
        "frameInsetSpacing" => FrameInsetSpacing,
        "paragraphJustification" => ParagraphJustification,
        "paragraphStyleNextStyle" => ParagraphStyleNextStyle,
        "paragraphAppliedNumberingList" => ParagraphAppliedNumberingList,
        "frameStrokeEndCap" => FrameStrokeEndCap,
        "frameTextWrapMode" => FrameTextWrapMode,
        "frameTextWrapOffsets" => FrameTextWrapOffsets,
        "frameTextWrapContourType" => FrameTextWrapContourType,
        "frameTextWrapContourIncludeInside" => FrameTextWrapContourIncludeInside,
        "frameFittingCrops" => FrameFittingCrops,
        "frameFittingType" => FrameFittingType,
        "frameDropShadow" => FrameDropShadow,
        "frameDropShadowMode" => FrameDropShadowMode,
        "frameDropShadowXOffset" => FrameDropShadowXOffset,
        "frameDropShadowYOffset" => FrameDropShadowYOffset,
        "frameDropShadowSize" => FrameDropShadowSize,
        "frameDropShadowOpacity" => FrameDropShadowOpacity,
        "frameDropShadowColor" => FrameDropShadowColor,
        "framePath" => FramePath,
        "frameFillTint" => FrameFillTint,
        "frameNonprinting" => FrameNonprinting,
        "frameGradientFillAngle" => FrameGradientFillAngle,
        "frameGradientFillLength" => FrameGradientFillLength,
        "frameGradientStrokeAngle" => FrameGradientStrokeAngle,
        "frameGradientStrokeLength" => FrameGradientStrokeLength,
        // W0.3.
        "textFrameColumnCount" => TextFrameColumnCount,
        "textFrameColumnGutter" => TextFrameColumnGutter,
        "textFrameColumnBalance" => TextFrameColumnBalance,
        "textFrameVerticalJustification" => TextFrameVerticalJustification,
        "textFrameAutoSizing" => TextFrameAutoSizing,
        "textFrameFirstBaseline" => TextFrameFirstBaseline,
        "frameTextWrapInvert" => TextWrapInvert,
        "frameFittingReferencePoint" => FrameFittingReferencePoint,
        "frameAutoFit" => FrameAutoFit,
        "frameStrokeType" => FrameStrokeType,
        "frameStrokeJoin" => FrameStrokeJoin,
        "frameStrokeMiterLimit" => FrameStrokeMiterLimit,
        "frameStrokeAlignment" => FrameStrokeAlignment,
        "frameStrokeGapColor" => FrameStrokeGapColor,
        "frameStrokeGapTint" => FrameStrokeGapTint,
        "frameStrokeDashArray" => FrameStrokeDashArray,
        "frameCornerOptionTopLeft" => FrameCornerOptionTopLeft,
        "frameCornerOptionTopRight" => FrameCornerOptionTopRight,
        "frameCornerOptionBottomLeft" => FrameCornerOptionBottomLeft,
        "frameCornerOptionBottomRight" => FrameCornerOptionBottomRight,
        "frameCornerRadiusTopLeft" => FrameCornerRadiusTopLeft,
        "frameCornerRadiusTopRight" => FrameCornerRadiusTopRight,
        "frameCornerRadiusBottomLeft" => FrameCornerRadiusBottomLeft,
        "frameCornerRadiusBottomRight" => FrameCornerRadiusBottomRight,
        "frameRotationAngle" => FrameRotationAngle,
        "frameScaleX" => FrameScaleX,
        "frameScaleY" => FrameScaleY,
        "frameFlipH" => FrameFlipH,
        "frameFlipV" => FrameFlipV,
        "frameOverprintFill" => FrameOverprintFill,
        "frameOverprintStroke" => FrameOverprintStroke,
        // W0.4 — transparency effects.
        "frameInnerShadow" => FrameInnerShadowEnabled,
        "frameInnerShadowBlendMode" => FrameInnerShadowBlendMode,
        "frameInnerShadowColor" => FrameInnerShadowColor,
        "frameInnerShadowOpacity" => FrameInnerShadowOpacity,
        "frameInnerShadowAngle" => FrameInnerShadowAngle,
        "frameInnerShadowDistance" => FrameInnerShadowDistance,
        "frameInnerShadowSize" => FrameInnerShadowSize,
        "frameInnerShadowChoke" => FrameInnerShadowChoke,
        "frameInnerShadowNoise" => FrameInnerShadowNoise,
        "frameOuterGlow" => FrameOuterGlowEnabled,
        "frameOuterGlowBlendMode" => FrameOuterGlowBlendMode,
        "frameOuterGlowColor" => FrameOuterGlowColor,
        "frameOuterGlowOpacity" => FrameOuterGlowOpacity,
        "frameOuterGlowSpread" => FrameOuterGlowSpread,
        "frameOuterGlowSize" => FrameOuterGlowSize,
        "frameOuterGlowNoise" => FrameOuterGlowNoise,
        "frameInnerGlow" => FrameInnerGlowEnabled,
        "frameInnerGlowBlendMode" => FrameInnerGlowBlendMode,
        "frameInnerGlowColor" => FrameInnerGlowColor,
        "frameInnerGlowOpacity" => FrameInnerGlowOpacity,
        "frameInnerGlowChoke" => FrameInnerGlowChoke,
        "frameInnerGlowSize" => FrameInnerGlowSize,
        "frameInnerGlowSource" => FrameInnerGlowSource,
        "frameInnerGlowNoise" => FrameInnerGlowNoise,
        "frameBevel" => FrameBevelEnabled,
        "frameBevelStyle" => FrameBevelStyle,
        "frameBevelTechnique" => FrameBevelTechnique,
        "frameBevelDepth" => FrameBevelDepth,
        "frameBevelDirection" => FrameBevelDirection,
        "frameBevelSize" => FrameBevelSize,
        "frameBevelSoften" => FrameBevelSoften,
        "frameBevelAngle" => FrameBevelAngle,
        "frameBevelAltitude" => FrameBevelAltitude,
        "frameBevelHighlightColor" => FrameBevelHighlightColor,
        "frameBevelShadowColor" => FrameBevelShadowColor,
        "frameBevelHighlightOpacity" => FrameBevelHighlightOpacity,
        "frameBevelShadowOpacity" => FrameBevelShadowOpacity,
        "frameSatin" => FrameSatinEnabled,
        "frameSatinBlendMode" => FrameSatinBlendMode,
        "frameSatinColor" => FrameSatinColor,
        "frameSatinOpacity" => FrameSatinOpacity,
        "frameSatinAngle" => FrameSatinAngle,
        "frameSatinDistance" => FrameSatinDistance,
        "frameSatinSize" => FrameSatinSize,
        "frameSatinInvert" => FrameSatinInvert,
        "frameFeather" => FrameFeatherEnabled,
        "frameFeatherWidth" => FrameFeatherWidth,
        "frameFeatherCornerType" => FrameFeatherCornerType,
        "frameFeatherNoise" => FrameFeatherNoise,
        "frameFeatherChoke" => FrameFeatherChoke,
        "frameDirectionalFeather" => FrameDirectionalFeatherEnabled,
        "frameDirectionalFeatherLeftWidth" => FrameDirectionalFeatherLeftWidth,
        "frameDirectionalFeatherRightWidth" => FrameDirectionalFeatherRightWidth,
        "frameDirectionalFeatherTopWidth" => FrameDirectionalFeatherTopWidth,
        "frameDirectionalFeatherBottomWidth" => FrameDirectionalFeatherBottomWidth,
        "frameDirectionalFeatherAngle" => FrameDirectionalFeatherAngle,
        "frameDirectionalFeatherNoise" => FrameDirectionalFeatherNoise,
        "frameDirectionalFeatherChoke" => FrameDirectionalFeatherChoke,
        "frameBlendMode" => FrameBlendMode,
        // W3.A1 — table cell properties (writable).
        "cellFillColor" => CellFillColor,
        "cellFillTint" => CellFillTint,
        "cellInsetTop" => CellInsetTop,
        "cellInsetLeft" => CellInsetLeft,
        "cellInsetBottom" => CellInsetBottom,
        "cellInsetRight" => CellInsetRight,
        "cellVerticalJustification" => CellVerticalJustification,
        // W1.11b — per-cell edge strokes (writable).
        "cellTopEdgeStrokeColor" => CellTopEdgeStrokeColor,
        "cellTopEdgeStrokeWeight" => CellTopEdgeStrokeWeight,
        "cellTopEdgeStrokeTint" => CellTopEdgeStrokeTint,
        "cellBottomEdgeStrokeColor" => CellBottomEdgeStrokeColor,
        "cellBottomEdgeStrokeWeight" => CellBottomEdgeStrokeWeight,
        "cellBottomEdgeStrokeTint" => CellBottomEdgeStrokeTint,
        "cellLeftEdgeStrokeColor" => CellLeftEdgeStrokeColor,
        "cellLeftEdgeStrokeWeight" => CellLeftEdgeStrokeWeight,
        "cellLeftEdgeStrokeTint" => CellLeftEdgeStrokeTint,
        "cellRightEdgeStrokeColor" => CellRightEdgeStrokeColor,
        "cellRightEdgeStrokeWeight" => CellRightEdgeStrokeWeight,
        "cellRightEdgeStrokeTint" => CellRightEdgeStrokeTint,
        // Aftercare-A — table dimensions (read-only; resolvable by name
        // so scripts can read them, rejected on write by the apply layer).
        "tableRowCount" => TableRowCount,
        "tableColumnCount" => TableColumnCount,
        "pluginMetadata" => PluginMetadata,
        // W1.16 — anchored-object settings.
        "anchoredPosition" => AnchoredPosition,
        "anchorPoint" => AnchorPoint,
        "anchoredXOffset" => AnchoredXOffset,
        "anchoredYOffset" => AnchoredYOffset,
        "anchoredHorizontalReference" => AnchoredHorizontalReference,
        "anchoredVerticalReference" => AnchoredVerticalReference,
        "anchoredHorizontalAlignment" => AnchoredHorizontalAlignment,
        "anchoredVerticalAlignment" => AnchoredVerticalAlignment,
        "anchoredSpineRelative" => AnchoredSpineRelative,
        "anchoredLockPosition" => AnchoredLockPosition,
        // W2.5 — element-level visibility / lock.
        "elementVisible" => ElementVisible,
        "elementLocked" => ElementLocked,
        _ => return None,
    })
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
