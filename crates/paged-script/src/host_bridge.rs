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

//! The async HOST BRIDGE (ADR 323, protocol 71): how a script reaches a
//! plugin object.
//!
//! Plugin objects (`plugin:<pluginId>/<kind>/<id>`) live in the HOST's
//! object registry (`host.objects` — the editor's main thread, the
//! headless host, the Node CLI), not in this worker. Boa runs
//! synchronously inside one worker message, so a script cannot block on
//! the host. Instead:
//!
//! - `paged.get/set/batch/query/schema` with a `plugin:` address (or a
//!   selector naming one) return a PROMISE and queue a [`HostObjectCall`].
//!   Consecutive `paged.set`s fold into ONE `batch` call.
//! - When the script has run as far as it can (its synchronous body, then
//!   every runnable promise job), the queued calls leave as ONE
//!   `RequestHostObjects` round trip and the session SUSPENDS, keeping its
//!   Boa context.
//! - `HostObjectsResult` settles the promises in call order and the
//!   session runs again — until nothing is queued, which is the
//!   `ScriptResult`.
//!
//! So a script reads like a synchronous one with `await` where it needs an
//! answer: `paged.set(plugin, …)` without `await` is a batched, fire-and-
//! forget write whose refusal still fails the script (an unhandled
//! rejection is the script's error); `await paged.get(plugin, …)` is a
//! read. Top-level `await` works: a source that only parses with it runs
//! as the body of an async function (`return x` prints `x`).
//!
//! A refused answer is an EXCEPTION carrying the registry's `code`
//! (`err.code === "invalidValue"`); `paged.batch` instead resolves to its
//! per-op results. With no host (`ExecuteScript.hostObjects` absent: the
//! Rust CLI, an older editor) a plugin address throws `noHost`
//! synchronously.
//!
//! Undo: when a session wrote through the host, every undo record logged
//! since the script started is folded into one (`group_undo_since`), so
//! the script's core writes (applied here) and its plugin writes (which
//! the host committed as its own engine batch while the script waited)
//! undo as ONE step.

use std::cell::RefCell;

use boa_engine::{
    builtins::promise::{PromiseState, ResolvingFunctions},
    object::{builtins::JsPromise, JsObject},
    Context, JsError, JsNativeError, JsResult, JsString, JsValue,
};
use paged_canvas::channel::{HostObjectAnswer, HostObjectCall, HostObjectError};

/// How many host round trips one script may take before it is stopped —
/// the runaway guard for a loop that awaits the host (the wall clock is
/// per segment, so it cannot see one).
pub const MAX_HOST_ROUNDS: u32 = 1000;

/// What a waiting promise expects from its call's answer.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Shape {
    /// `paged.get` — resolves to the value's JSON text, or `null`.
    Get,
    /// `paged.query` / `paged.schema` — resolves to the answer's JSON text.
    Json,
    /// One `paged.set`, op `index` of a folded batch — resolves `true`.
    Set { index: usize },
    /// `paged.batch` of `n` ops — resolves to the per-op results' JSON.
    Batch { n: usize },
}

struct Waiter {
    promise: JsObject,
    resolvers: ResolvingFunctions,
    shape: Shape,
}

/// One call on its way to the host, and the promises that wait on it.
pub(crate) struct Queued {
    call: HostObjectCall,
    waiters: Vec<Waiter>,
    /// A batch built from `paged.set`s — later sets may join it.
    from_sets: bool,
}

#[derive(Default)]
struct HostQueue {
    queued: Vec<Queued>,
    writes: bool,
}

thread_local! {
    /// `Some` while a host-enabled segment runs; `None` = no host.
    static HOST: RefCell<Option<HostQueue>> = const { RefCell::new(None) };
}

pub(crate) fn open_queue(enabled: bool) {
    HOST.with(|h| *h.borrow_mut() = enabled.then(HostQueue::default));
}

/// Take what the segment queued (and whether any of it writes), closing
/// the queue.
pub(crate) fn close_queue() -> (Vec<Queued>, bool) {
    HOST.with(|h| {
        h.borrow_mut()
            .take()
            .map(|q| (q.queued, q.writes))
            .unwrap_or_default()
    })
}

pub(crate) fn calls_of(queued: &[Queued]) -> Vec<HostObjectCall> {
    queued.iter().map(|q| q.call.clone()).collect()
}

/// An `Error` whose message is `"<code>: <reason>"` and whose `code`
/// property is the registry's code — what a refused call throws.
pub(crate) fn coded_error(code: &str, reason: &str, ctx: &mut Context) -> JsValue {
    let err: JsError = JsNativeError::error()
        .with_message(format!("{code}: {reason}"))
        .into();
    let value = err.to_opaque(ctx);
    if let Some(obj) = value.as_object() {
        let _ = obj.set(
            JsString::from("code"),
            JsValue::from(JsString::from(code)),
            false,
            ctx,
        );
    }
    value
}

/// Queue `call` for the host and return the promise that waits on it.
/// No host → throws `noHost` synchronously.
pub(crate) fn enqueue(call: HostObjectCall, shape: Shape, ctx: &mut Context) -> JsResult<JsValue> {
    if HOST.with(|h| h.borrow().is_none()) {
        return Err(JsError::from_opaque(coded_error(
            "noHost",
            "this script host has no object registry: a plugin: address needs the editor, the headless host or the Node CLI",
            ctx,
        )));
    }
    let (promise, resolvers) = JsPromise::new_pending(ctx);
    let promise = JsObject::from(promise);
    track(&promise, ctx)?;
    let result = JsValue::from(promise.clone());
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let q = h.as_mut().expect("checked above");
        if matches!(call, HostObjectCall::Batch { .. }) {
            q.writes = true;
        }
        if let (Shape::Set { .. }, HostObjectCall::Batch { ops }) = (&shape, &call) {
            if let Some(last) = q.queued.last_mut().filter(|l| l.from_sets) {
                if let HostObjectCall::Batch { ops: folded } = &mut last.call {
                    let index = folded.len();
                    folded.extend(ops.iter().cloned());
                    last.waiters.push(Waiter {
                        promise,
                        resolvers,
                        shape: Shape::Set { index },
                    });
                    return;
                }
            }
            q.queued.push(Queued {
                call,
                waiters: vec![Waiter {
                    promise,
                    resolvers,
                    shape: Shape::Set { index: 0 },
                }],
                from_sets: true,
            });
            return;
        }
        q.queued.push(Queued {
            call,
            waiters: vec![Waiter {
                promise,
                resolvers,
                shape,
            }],
            from_sets: false,
        });
    });
    Ok(result)
}

/// The property a bridge promise's `then` sets once anything attaches a
/// handler (`await`, `.then`, `.catch`, `Promise.all`).
const HANDLED: &str = "__pagedHandled";

/// Make `promise` report when a handler attaches. Boa 0.21 never marks a
/// promise handled when the handler attached while it was PENDING (its
/// `PerformPromiseThen` sets `[[PromiseIsHandled]]` only on the rejected
/// branch), so its rejection tracker reports every awaited bridge
/// rejection as unhandled. Instead the promise gets an own `then` that
/// flags it, and an own `constructor` so `await` / `Promise.resolve` take
/// the thenable path through that `then` (`catch` and `finally` invoke
/// `then` anyway).
fn track(promise: &JsObject, ctx: &mut Context) -> JsResult<()> {
    let tracker = ctx
        .global_object()
        .get(JsString::from("__pagedTrack"), ctx)?;
    if let Some(f) = tracker.as_callable() {
        f.call(
            &JsValue::undefined(),
            &[JsValue::from(promise.clone())],
            ctx,
        )?;
    }
    Ok(())
}

/// The bootstrap half of [`track`].
pub(crate) const TRACK_BOOTSTRAP: &str = r#"
    Object.defineProperty(globalThis, '__pagedTrack', { value: function (p) {
        const then = Promise.prototype.then;
        Object.defineProperty(p, 'constructor', { value: Object });
        Object.defineProperty(p, 'then', { value: function (a, b) {
            p.__pagedHandled = true;
            return then.call(this, a, b);
        } });
    } });
"#;

/// The reason of the first rejected bridge promise no handler ever
/// attached to — a fire-and-forget `paged.set` on a plugin object that
/// the host refused is the script's error.
pub(crate) fn first_unhandled(rejected: &[JsObject], ctx: &mut Context) -> Option<JsValue> {
    rejected.iter().find_map(|p| {
        let handled = p
            .get(JsString::from(HANDLED), ctx)
            .is_ok_and(|v| v.to_boolean());
        if handled {
            return None;
        }
        match JsPromise::from_object(p.clone()).ok()?.state() {
            PromiseState::Rejected(v) => Some(v),
            _ => None,
        }
    })
}

fn json_text(value: &serde_json::Value) -> JsValue {
    JsValue::from(JsString::from(
        serde_json::to_string(value).unwrap_or_default(),
    ))
}

/// The per-op results of a batch: all `ok`, or the refused op carrying
/// the code and every other op `aborted` (a batch is all-or-nothing).
pub(crate) fn per_op_results(n: usize, error: Option<&HostObjectError>) -> serde_json::Value {
    use serde_json::json;
    let rows = (0..n)
        .map(|i| match error {
            None => json!({ "ok": true }),
            Some(e) if e.index.is_none() || e.index == Some(i as u32) => {
                json!({ "ok": false, "code": e.code, "reason": e.reason })
            }
            Some(e) => json!({
                "ok": false,
                "code": "aborted",
                "reason": format!("not applied: op {} in the same batch was refused ({})",
                    e.index.unwrap_or_default(), e.code),
            }),
        })
        .collect();
    serde_json::Value::Array(rows)
}

/// Settle every promise waiting on `queued` from the host's `answer`.
/// A missing answer is a `failed` refusal (the host broke the contract).
pub(crate) fn settle(
    queued: Queued,
    answer: Option<&HostObjectAnswer>,
    ctx: &mut Context,
    rejected: &mut Vec<JsObject>,
) {
    let missing = HostObjectAnswer {
        value: serde_json::Value::Null,
        error: Some(HostObjectError {
            code: "failed".into(),
            reason: "the host sent no answer for this call".into(),
            index: None,
        }),
    };
    let answer = answer.unwrap_or(&missing);
    let mut waiters = queued.waiters;
    // The refused op's promise rejects FIRST, so the unhandled-rejection
    // the script reports is the real refusal, not an `aborted` sibling.
    if let Some(e) = &answer.error {
        waiters.sort_by_key(|w| match w.shape {
            Shape::Set { index } => e.index.is_some() && e.index != Some(index as u32),
            _ => false,
        });
    }
    for w in waiters {
        let outcome: Result<JsValue, JsValue> = match (&answer.error, w.shape) {
            (Some(e), Shape::Batch { n }) => Ok(json_text(&per_op_results(n, Some(e)))),
            (Some(e), Shape::Set { index })
                if e.index.is_some() && e.index != Some(index as u32) =>
            {
                Err(coded_error(
                    "aborted",
                    &format!(
                        "not applied: a write folded into the same batch was refused ({}: {})",
                        e.code, e.reason
                    ),
                    ctx,
                ))
            }
            (Some(e), _) => Err(coded_error(&e.code, &e.reason, ctx)),
            (None, Shape::Get) => get_value(&answer.value, ctx),
            (None, Shape::Json) => Ok(json_text(&answer.value)),
            (None, Shape::Set { .. }) => Ok(JsValue::from(true)),
            (None, Shape::Batch { n }) => Ok(json_text(&per_op_results(n, None))),
        };
        let (f, v) = match outcome {
            Ok(v) => (&w.resolvers.resolve, v),
            Err(v) => {
                rejected.push(w.promise.clone());
                (&w.resolvers.reject, v)
            }
        };
        let _ = f.call(&JsValue::undefined(), &[v], ctx);
    }
}

/// An `ObjectValue` answer (`{ kind: "value", value }`, `absent`, `mixed`,
/// or a `refused` the host passed through) as `paged.get`'s result.
fn get_value(value: &serde_json::Value, ctx: &mut Context) -> Result<JsValue, JsValue> {
    match value.get("kind").and_then(serde_json::Value::as_str) {
        Some("value") => Ok(json_text(
            value.get("value").unwrap_or(&serde_json::Value::Null),
        )),
        Some("refused") => {
            let s = |k: &str| {
                value
                    .get(k)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            Err(coded_error(&s("code"), &s("reason"), ctx))
        }
        _ => Ok(JsValue::null()),
    }
}
