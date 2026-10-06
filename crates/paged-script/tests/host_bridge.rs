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

//! The async host bridge (ADR 323, protocol 71): a script names a
//! `plugin:` address, the session suspends with ONE batched
//! `RequestHostObjects`, the host's answers resume it. Refusals are
//! exceptions carrying the registry's code; `paged.batch` resolves to
//! per-op results; a script that wrote through the host undoes as ONE step.

use paged_canvas::{CanvasModel, CanvasOptions};
use paged_script::{
    execute_script, start_script, HostObjectAnswer, HostObjectCall, HostObjectError, ScriptBudget,
    ScriptResult, ScriptSession, ScriptStep,
};
use serde_json::json;

fn blank() -> CanvasModel {
    CanvasModel::new_blank("hb", 612.0, 792.0, CanvasOptions::default()).expect("new_blank")
}

fn clock() -> f64 {
    0.0
}

fn start(model: &mut CanvasModel, src: &str) -> (ScriptStep, Option<ScriptSession>) {
    start_script(model, src, ScriptBudget::default(), &clock, true, 7)
}

fn suspended(step: (ScriptStep, Option<ScriptSession>)) -> (Vec<HostObjectCall>, ScriptSession) {
    match step {
        (ScriptStep::Suspended { calls }, Some(s)) => (calls, s),
        (other, _) => panic!("expected the script to suspend, got {other:?}"),
    }
}

fn done(step: (ScriptStep, Option<ScriptSession>)) -> ScriptResult {
    match step {
        (ScriptStep::Done(r), None) => r,
        (other, _) => panic!("expected the script to finish, got {other:?}"),
    }
}

fn ok(value: serde_json::Value) -> HostObjectAnswer {
    HostObjectAnswer { value, error: None }
}

fn refused(code: &str, index: Option<u32>) -> HostObjectAnswer {
    HostObjectAnswer {
        value: serde_json::Value::Null,
        error: Some(HostObjectError {
            code: code.into(),
            reason: format!("{code} for the test"),
            index,
        }),
    }
}

#[test]
fn without_a_host_a_plugin_address_throws_no_host() {
    let mut model = blank();
    let r = execute_script(
        &mut model,
        r#"
try { paged.set('plugin:paged.test/thing/1', 'name', 'A'); }
catch (e) { console.log('caught', e.code); }
paged.get('plugin:paged.test/thing/1', 'name');
"#,
    );
    assert_eq!(r.output, vec!["[log] caught noHost".to_string()]);
    assert!(
        r.error.as_deref().unwrap_or_default().contains("noHost"),
        "{r:?}"
    );
}

#[test]
fn consecutive_plugin_sets_fold_into_one_batched_round_trip() {
    let mut model = blank();
    let (calls, session) = suspended(start(
        &mut model,
        r#"
paged.set('plugin:paged.test/thing/1', 'name', 'A');
paged.set('plugin:paged.test/thing/2', 'size', 12);
console.log('core', paged.set('doc', 'documentBleedTop', '3mm'));
"#,
    ));
    assert_eq!(session.id(), 7);
    assert_eq!(
        calls,
        vec![HostObjectCall::Batch {
            ops: vec![
                json!({ "op": "set", "address": "plugin:paged.test/thing/1", "path": "name", "value": "A" }),
                json!({ "op": "set", "address": "plugin:paged.test/thing/2", "path": "size", "value": 12 }),
            ]
        }],
        "one round trip, one batch"
    );
    let r = done(session.resume(
        &mut model,
        &[ok(json!({ "applied": true, "undoSteps": 1 }))],
        &clock,
    ));
    assert_eq!(r.error, None);
    assert_eq!(r.output, vec!["[log] core true".to_string()]);
}

#[test]
fn a_refused_fire_and_forget_write_is_the_scripts_error_with_its_code() {
    let mut model = blank();
    let (_, session) = suspended(start(
        &mut model,
        r#"
paged.set('plugin:paged.test/thing/1', 'name', 'A');
paged.set('plugin:paged.test/thing/1', 'size', 'big');
"#,
    ));
    let r = done(session.resume(&mut model, &[refused("invalidValue", Some(1))], &clock));
    let err = r.error.expect("the refusal fails the script");
    assert!(err.contains("invalidValue"), "{err}");
    assert!(
        !err.contains("aborted"),
        "the real refusal is reported first: {err}"
    );
}

#[test]
fn an_awaited_refusal_is_an_exception_carrying_the_code() {
    let mut model = blank();
    let (_, session) = suspended(start(
        &mut model,
        r#"
try { await paged.set('plugin:paged.test/thing/1', 'size', -1); console.log('no'); }
catch (e) { console.log('code', e.code, e instanceof Error); }
"#,
    ));
    let r = done(session.resume(&mut model, &[refused("readOnly", Some(0))], &clock));
    assert_eq!(r.error, None);
    assert_eq!(r.output, vec!["[log] code readOnly true".to_string()]);
}

#[test]
fn an_awaited_get_reads_the_plugin_value_and_reads_batch_per_round() {
    let mut model = blank();
    let (calls, session) = suspended(start(
        &mut model,
        r#"
const [a, b] = await Promise.all([
  paged.get('plugin:paged.test/thing/1', 'name'),
  paged.get('plugin:paged.test/thing/2', 'name'),
]);
console.log(JSON.parse(a), b);
const found = JSON.parse(await paged.query('plugin:paged.test/thing'));
console.log(found.length);
return 'done';
"#,
    ));
    assert_eq!(
        calls.len(),
        2,
        "both reads leave in ONE round trip: {calls:?}"
    );
    assert_eq!(
        calls[0],
        HostObjectCall::Get {
            address: "plugin:paged.test/thing/1".into(),
            path: "name".into()
        }
    );
    let (calls, session) = suspended(session.resume(
        &mut model,
        &[
            ok(json!({ "kind": "value", "value": "Alpha" })),
            ok(json!({ "kind": "absent" })),
        ],
        &clock,
    ));
    assert_eq!(
        calls,
        vec![HostObjectCall::Query {
            selector: "plugin:paged.test/thing".into()
        }]
    );
    let r = done(session.resume(
        &mut model,
        &[ok(json!([
            "plugin:paged.test/thing/1",
            "plugin:paged.test/thing/2"
        ]))],
        &clock,
    ));
    assert_eq!(r.error, None);
    assert_eq!(r.output, vec!["[log] Alpha null", "[log] 2", "done"]);
}

#[test]
fn a_refused_get_throws_its_code() {
    let mut model = blank();
    let (_, session) = suspended(start(
        &mut model,
        "await paged.get('plugin:paged.test/thing/9', 'name');",
    ));
    let r = done(session.resume(
        &mut model,
        &[ok(
            json!({ "kind": "refused", "code": "unknownAddress", "reason": "no thing 9" }),
        )],
        &clock,
    ));
    assert!(r.error.unwrap_or_default().contains("unknownAddress"));
}

#[test]
fn paged_batch_resolves_to_per_op_results() {
    let mut model = blank();
    let (calls, session) = suspended(start(
        &mut model,
        r#"
const res = await paged.batch([
  { op: 'set', address: 'doc', path: 'documentBleedTop', value: '3mm' },
  { op: 'set', address: 'plugin:paged.test/thing/1', path: 'size', value: 'x' },
]);
console.log(res);
"#,
    ));
    let HostObjectCall::Batch { ops } = &calls[0] else {
        panic!("{calls:?}")
    };
    assert_eq!(ops.len(), 2, "a mixed batch goes to the host WHOLE");
    let r = done(session.resume(&mut model, &[refused("invalidValue", Some(1))], &clock));
    assert_eq!(r.error, None, "a batch reports, it does not throw");
    let rows: serde_json::Value =
        serde_json::from_str(r.output[0].trim_start_matches("[log] ")).unwrap();
    assert_eq!(rows[0]["code"], "aborted");
    assert_eq!(rows[1]["code"], "invalidValue");
    assert_eq!(rows[1]["ok"], false);
}

#[test]
fn a_core_only_object_batch_applies_here_as_one_undo_step() {
    let mut model = blank();
    let before = model.applied_log_len();
    let r = execute_script(
        &mut model,
        r#"
console.log(paged.batch([
  { op: 'set', address: 'doc', path: 'documentBleedTop', value: '3mm' },
  { op: 'set', address: 'doc', path: 'documentBleedBottom', value: '4mm' },
]));
console.log(paged.batch([{ op: 'set', address: 'doc', path: 'noSuchPath', value: 1 }]));
"#,
    );
    assert_eq!(r.error, None);
    assert_eq!(r.output[0], r#"[log] [{"ok":true},{"ok":true}]"#);
    assert!(r.output[1].contains(r#""ok":false"#), "{:?}", r.output);
    assert_eq!(model.applied_log_len(), before + 1);
}

#[test]
fn core_and_host_writes_of_one_script_undo_as_one_step() {
    let mut model = blank();
    let before = model.applied_log_len();
    let read = |model: &mut CanvasModel| {
        execute_script(
            model,
            "console.log(paged.get('doc','documentBleedTop'), paged.get('doc','documentBleedBottom'))",
        )
        .output
        .join("")
    };
    let pristine = read(&mut model);
    let (_, session) = suspended(start(
        &mut model,
        r#"
paged.set('doc', 'documentBleedTop', '3mm');
paged.set('plugin:paged.test/thing/1', 'name', 'A');
"#,
    ));
    // While the script waits, the HOST commits the plugin write as its own
    // engine batch (here: the label-hash write a plugin state change makes).
    let host = execute_script(&mut model, "paged.set('doc', 'documentBleedBottom', '4mm')");
    assert_eq!(host.error, None);
    assert_eq!(
        model.applied_log_len(),
        before + 2,
        "two records before the fold"
    );
    let r = done(session.resume(
        &mut model,
        &[ok(json!({ "applied": true, "undoSteps": 1 }))],
        &clock,
    ));
    assert_eq!(r.error, None);
    assert_eq!(model.applied_log_len(), before + 1, "ONE undo step");
    assert_ne!(read(&mut model), pristine);
    assert!(model.undo().is_some());
    assert_eq!(
        read(&mut model),
        pristine,
        "one undo reverts core AND host writes"
    );
    assert!(model.redo().is_some());
    assert_ne!(read(&mut model), pristine);
}

#[test]
fn a_script_that_throws_does_not_flush_its_host_calls() {
    let mut model = blank();
    let r = done(start(
        &mut model,
        "paged.set('plugin:paged.test/thing/1', 'name', 'A'); throw new Error('boom');",
    ));
    assert!(r.error.unwrap_or_default().contains("boom"));
}

#[test]
fn a_script_with_no_plugin_calls_never_suspends() {
    let mut model = blank();
    let r = done(start(&mut model, "1 + 1"));
    assert_eq!(r.output, vec!["2".to_string()]);
}

#[test]
fn a_core_schema_is_read_here_and_a_plugin_schema_asks_the_host() {
    let mut model = blank();
    let (calls, session) = suspended(start(
        &mut model,
        r#"
const core = JSON.parse(paged.schema('doc'));
console.log(core.some(r => r.path === 'documentBleedTop'));
console.log(JSON.parse(await paged.schema('plugin:paged.test/thing'))[0].path);
"#,
    ));
    assert_eq!(
        calls,
        vec![HostObjectCall::Schema {
            target: "plugin:paged.test/thing".into()
        }]
    );
    let r = done(session.resume(
        &mut model,
        &[ok(json!([{ "path": "name", "type": { "kind": "text" } }]))],
        &clock,
    ));
    assert_eq!(r.error, None);
    assert_eq!(r.output, vec!["[log] true", "[log] name"]);
}

#[test]
fn the_bridge_messages_have_their_wire_shape() {
    use paged_canvas::channel::{MainToWorker, WorkerToMain, WorkerToMainKind};
    // ExecuteScript without hostObjects still decodes (old hosts).
    let old: MainToWorker = serde_json::from_value(json!({
        "seq": 1, "protocol": 71, "kind": "executeScript", "payload": { "source": "1" }
    }))
    .unwrap();
    let paged_canvas::channel::MainToWorkerKind::ExecuteScript { host_objects, .. } = old.kind
    else {
        panic!()
    };
    assert!(!host_objects);
    let answer: MainToWorker = serde_json::from_value(json!({
        "seq": 2, "protocol": 71, "kind": "hostObjectsResult",
        "payload": { "session": 3, "answers": [
            { "value": { "kind": "value", "value": 1 } },
            { "error": { "code": "invalidValue", "reason": "r", "index": 0 } },
            {}
        ] }
    }))
    .unwrap();
    let paged_canvas::channel::MainToWorkerKind::HostObjectsResult { session, answers } =
        answer.kind
    else {
        panic!()
    };
    assert_eq!(session, 3);
    assert_eq!(answers[1].error.as_ref().unwrap().index, Some(0));
    assert_eq!(answers[2], HostObjectAnswer::default());
    let req = WorkerToMain {
        seq: Some(2),
        protocol: paged_canvas::channel::PROTOCOL_VERSION,
        kind: WorkerToMainKind::RequestHostObjects {
            session: 3,
            calls: vec![
                HostObjectCall::Get {
                    address: "plugin:a/b/c".into(),
                    path: "p".into(),
                },
                HostObjectCall::Batch {
                    ops: vec![json!({ "op": "set" })],
                },
                HostObjectCall::Query {
                    selector: "plugin:a/b".into(),
                },
                HostObjectCall::Schema {
                    target: "plugin:a/b".into(),
                },
            ],
        },
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["kind"], "requestHostObjects");
    assert_eq!(
        v["payload"],
        json!({ "session": 3, "calls": [
            { "op": "get", "address": "plugin:a/b/c", "path": "p" },
            { "op": "batch", "ops": [{ "op": "set" }] },
            { "op": "query", "selector": "plugin:a/b" },
            { "op": "schema", "target": "plugin:a/b" },
        ] })
    );
}
