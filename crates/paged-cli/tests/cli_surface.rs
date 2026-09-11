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

//! Which of the engine's 62 message kinds this CLI can reach.
//!
//! The second column of the capability × surface matrix, after
//! `paged-script`'s. The CLI began at **11 of 62** kinds and one
//! `Mutate` of its own. That number was a survey finding; here it is a
//! gate, with a sentence on every kind it does not send.
//!
//! The distinction the reasons are for: "a command line has no pointer"
//! is a property of the surface and will never change, while "a
//! diagnostic read with no subcommand yet" is work someone can do. Both
//! were the same silence before this file — a kind the CLI never named,
//! for reasons nobody had written down. Twenty-one of the second kind
//! are now closed (`paged read`, `paged parts`, and `ExecuteScript`,
//! whose budget became a wire parameter at v63 instead of a reason to
//! reach past the dispatcher), taking this to **32 of 62**; what
//! remains is almost entirely the first kind.
//!
//! **What this gate cannot see.** It counts message KINDS, and
//! `Mutate` has been one of them since the colour-profile activation in
//! `options.rs`. So a subcommand that reaches a mutation the CLI could
//! not express before — `paged place` and `ReplaceImageBytes`, added
//! 2026-09-11 — moves nothing here. The second axis (which of the 117
//! mutation ops each surface can send) is the capability matrix's, not
//! this file's; a reader who wants "can the CLI place an image" must
//! ask there.
//!
//! **Why there is no `paged wire <json>`.** It would reach every
//! remaining kind at a stroke, and it would be a second general door:
//! `paged session` already speaks the whole protocol, one message per
//! line, from the same `WorkerCore::dispatch`. A typed subcommand earns
//! its place by being the ergonomic form of a question worth asking; a
//! raw-JSON escape hatch beside a raw-JSON session is duplication, and
//! it would also not move this gate, which counts the kinds the CLI's
//! own source NAMES.
//!
//! The population is the deserializer's, not a list kept here: serde
//! names every `kind` it accepts in its "unknown variant" message, so a
//! kind added to the engine appears here on the next run whether or not
//! anyone remembered this file.

use std::collections::BTreeSet;

/// Message kinds the CLI does not send, each with why.
///
/// **This list may only SHRINK.** Reaching one means deleting its line
/// and lowering `UNREACHED_COUNT` in the same commit.
const UNREACHED: &[(&str, &str)] = &[
    ("BeginGesture",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("UpdateGesture",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("CommitGesture",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("CancelGesture",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("HitTest",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("SetSelection",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("SetElementSelection",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("RequestMarqueeHits",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("RequestSelectionGeometry",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("RequestCaretGeometry",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("RequestCaretNav",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("RequestLineBounds",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("RequestWordBounds",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("RequestParagraphBounds",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("RequestNearestPathPoint",
     "interaction: a command line has no pointer and no caret, and these answer a live canvas's questions about where one is"),
    ("RequestDocumentMeta",
     "answered through a different door: `paged inspect` and the NDJSON `inspect` read these off `Session::model`, which is the sanctioned path for a pure read with no state to consume"),
    ("RequestSceneTree",
     "answered through a different door: `paged inspect` and the NDJSON `inspect` read these off `Session::model`, which is the sanctioned path for a pure read with no state to consume"),
    ("RequestPage",
     "answered through a different door: `paged inspect` and the NDJSON `inspect` read these off `Session::model`, which is the sanctioned path for a pure read with no state to consume"),
    ("Undo",
     "reachable inside a script through `paged.undo` / `paged.redo`; there is no CLI verb, because a subcommand is one process and has nothing to undo across invocations"),
    ("Redo",
     "reachable inside a script through `paged.undo` / `paged.redo`; there is no CLI verb, because a subcommand is one process and has nothing to undo across invocations"),
    ("SubmitSceneLayer",
     "plugin-host lanes: scene layers, pixel layers and image resources are submitted by a bundle, and the CLI has no bundle host to submit them"),
    ("ClearSceneLayer",
     "plugin-host lanes: scene layers, pixel layers and image resources are submitted by a bundle, and the CLI has no bundle host to submit them"),
    ("SubmitPixelLayer",
     "plugin-host lanes: scene layers, pixel layers and image resources are submitted by a bundle, and the CLI has no bundle host to submit them"),
    ("ClearPixelLayer",
     "plugin-host lanes: scene layers, pixel layers and image resources are submitted by a bundle, and the CLI has no bundle host to submit them"),
    ("ClaimImageResource",
     "plugin-host lanes: scene layers, pixel layers and image resources are submitted by a bundle, and the CLI has no bundle host to submit them"),
    ("ReleaseImageResource",
     "plugin-host lanes: scene layers, pixel layers and image resources are submitted by a bundle, and the CLI has no bundle host to submit them"),
    ("SubmitResourceTiles",
     "plugin-host lanes: scene layers, pixel layers and image resources are submitted by a bundle, and the CLI has no bundle host to submit them"),
    ("Hello",
     "the worker shell's handshake; the NDJSON session emits its own ready line carrying the same protocol number, so there is no second one to send"),
    ("ExportPdfCancel",
     "only an interactive export dialog cancels a PDF mid-run; a subcommand either finishes or exits"),
    ("ClearFontRegistry",
     "no CLI verb: every subcommand builds a fresh `Session`, so a new process IS the reset this kind performs"),
];

/// Pinned so the list cannot grow quietly.
const UNREACHED_COUNT: usize = 30;

/// Every `kind` the wire accepts, read out of serde's own error rather
/// than kept as a list here — the same trick `wire_vocabulary.rs` uses
/// for the op tags, and for the same reason: a roster maintained beside
/// the thing it describes is a roster that drifts from it.
fn every_kind() -> BTreeSet<String> {
    let err = serde_json::from_str::<paged_canvas::channel::MainToWorkerKind>(
        r#"{"kind":"__not_a_kind__"}"#,
    )
    .expect_err("an unknown kind must not deserialize")
    .to_string();
    let list = err
        .split_once("expected one of ")
        .expect("serde names the kinds it expects")
        .1;
    let mut out = BTreeSet::new();
    let mut rest = list;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else { break };
        out.insert(after[..close].to_string());
        rest = &after[close + 1..];
    }
    out
}

/// PascalCase → the camelCase tag serde writes, so the source scan and
/// the deserializer's vocabulary can be compared at all.
fn tag(name: &str) -> String {
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The kinds this crate sends, derived from its own source: a
/// `MainToWorkerKind::X` in `src/` is the evidence that some subcommand
/// can ask for X.
fn kinds_the_cli_sends() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(&dir).expect("read src/") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read source");
        for (idx, _) in src.match_indices("MainToWorkerKind::") {
            let rest = &src[idx + "MainToWorkerKind::".len()..];
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric()).collect();
            if !name.is_empty() {
                out.insert(tag(&name));
            }
        }
    }
    out
}

#[test]
fn every_message_kind_is_reached_or_says_why_not() {
    let all = every_kind();
    let sent = kinds_the_cli_sends();
    let excused: BTreeSet<String> = UNREACHED.iter().map(|(k, _)| tag(k)).collect();

    assert!(
        all.len() > 50,
        "only {} kinds parsed out of serde's message — the reader lost its footing",
        all.len()
    );
    assert!(
        !sent.is_empty(),
        "the source scan found no `MainToWorkerKind::` at all, which cannot be right"
    );

    let phantom: Vec<&String> = sent.difference(&all).collect();
    assert!(
        phantom.is_empty(),
        "the CLI names kinds the wire does not accept: {phantom:?}"
    );

    let unclassified: Vec<&String> = all
        .difference(&sent)
        .filter(|k| !excused.contains(*k))
        .collect();
    assert!(
        unclassified.is_empty(),
        "these message kinds are unreachable from the CLI with no recorded reason — \
         add a subcommand, or add them to UNREACHED with a sentence: {unclassified:?}"
    );

    // The half that rots: a reason kept after the gap was closed.
    let stale: Vec<&String> = excused.intersection(&sent).collect();
    assert!(
        stale.is_empty(),
        "these are excused and the CLI now sends them — delete the lines and lower \
         UNREACHED_COUNT: {stale:?}"
    );

    let unknown: Vec<&String> = excused.difference(&all).collect();
    assert!(
        unknown.is_empty(),
        "UNREACHED names kinds the wire does not have: {unknown:?}"
    );

    assert_eq!(
        excused.len(),
        UNREACHED_COUNT,
        "the unreached list may only shrink"
    );
    assert_eq!(
        sent.len() + excused.len(),
        all.len(),
        "every kind is reached or excused, never both, never neither"
    );
}

#[test]
fn every_reason_is_a_reason() {
    for (kind, reason) in UNREACHED {
        assert!(
            reason.len() >= 80,
            "{kind}: say why in a sentence, got {reason:?}"
        );
        assert!(
            !reason.to_ascii_lowercase().contains("todo"),
            "{kind}: a TODO is not a reason"
        );
    }
}

/// The headline in the module doc, pinned against the code.
#[test]
fn the_cli_reaches_32_of_62() {
    assert_eq!(kinds_the_cli_sends().len(), 32);
    assert_eq!(every_kind().len(), 62);
}
