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

//! Which of the engine's wire ops a `paged.*` script can name.
//!
//! The system has one capability catalog and several surfaces that are
//! meant to be projections of it, and until this file nothing anywhere
//! asserted that a given capability is REACHABLE from a given surface.
//! The catalog proves an op exists; the editor's `capability-matrix`
//! spec proves the wasm wire accepts it; nothing said whether a script
//! could ask for it by name.
//!
//! It cannot, for 32 of the 117. That is not a small edge: it is the
//! whole of swatch, gradient and colour-group CRUD, six of the seven
//! pathfinder region verbs, both opacity-mask ops, text-on-a-path,
//! hyperlinks, anchored frames, z-order and every layer attribute. It
//! is also not hypothetical — the editor's own automation layer carries
//! an `editor.mutate({op, args})` escape hatch whose comment says it
//! exists "for content authoring on engines whose `paged.*` Boa lacks
//! the authoring fns". This is that list, with a reason on every line
//! and a ratchet that only lets it shrink.
//!
//! A script is not BLOCKED on any of them: `paged.batch` deserialises
//! raw wire ops, and `wire_vocabulary.rs` proves every one of the 117
//! tags is accepted there. The gap is between naming a capability and
//! having to know the wire spelling to reach it — which is the
//! difference between a surface and a hole in one.

use std::collections::BTreeSet;

/// Wire ops no `paged.*` function names, each with why.
///
/// **This list may only SHRINK.** Bridging one means deleting its line
/// and lowering `BATCH_ONLY_COUNT` in the same commit. A new entry means
/// an op shipped without a script surface and without a decision.
const BATCH_ONLY: &[(&str, &str)] = &[
    ("CreateSwatch",
     "colour-resource CRUD has no `paged.*` fn at all: the editor drives swatches, gradients and colour groups from its panels, and a script must reach them through `paged.batch` knowing the wire spelling"),
    ("EditSwatch",
     "colour-resource CRUD has no `paged.*` fn at all: the editor drives swatches, gradients and colour groups from its panels, and a script must reach them through `paged.batch` knowing the wire spelling"),
    ("DeleteSwatch",
     "colour-resource CRUD has no `paged.*` fn at all: the editor drives swatches, gradients and colour groups from its panels, and a script must reach them through `paged.batch` knowing the wire spelling"),
    ("CreateGradient",
     "colour-resource CRUD has no `paged.*` fn at all: the editor drives swatches, gradients and colour groups from its panels, and a script must reach them through `paged.batch` knowing the wire spelling"),
    ("EditGradient",
     "colour-resource CRUD has no `paged.*` fn at all: the editor drives swatches, gradients and colour groups from its panels, and a script must reach them through `paged.batch` knowing the wire spelling"),
    ("DeleteGradient",
     "colour-resource CRUD has no `paged.*` fn at all: the editor drives swatches, gradients and colour groups from its panels, and a script must reach them through `paged.batch` knowing the wire spelling"),
    ("CreateColorGroup",
     "colour-resource CRUD has no `paged.*` fn at all: the editor drives swatches, gradients and colour groups from its panels, and a script must reach them through `paged.batch` knowing the wire spelling"),
    ("EditColorGroup",
     "colour-resource CRUD has no `paged.*` fn at all: the editor drives swatches, gradients and colour groups from its panels, and a script must reach them through `paged.batch` knowing the wire spelling"),
    ("DeleteColorGroup",
     "colour-resource CRUD has no `paged.*` fn at all: the editor drives swatches, gradients and colour groups from its panels, and a script must reach them through `paged.batch` knowing the wire spelling"),
    ("PathfinderDivide",
     "only `pathfinderBoolean` is bridged; the seven REGION verbs never got a fn, so a script can boolean two paths but cannot divide, trim or take faces"),
    ("PathfinderTrim",
     "only `pathfinderBoolean` is bridged; the seven REGION verbs never got a fn, so a script can boolean two paths but cannot divide, trim or take faces"),
    ("PathfinderMerge",
     "only `pathfinderBoolean` is bridged; the seven REGION verbs never got a fn, so a script can boolean two paths but cannot divide, trim or take faces"),
    ("PathfinderCrop",
     "only `pathfinderBoolean` is bridged; the seven REGION verbs never got a fn, so a script can boolean two paths but cannot divide, trim or take faces"),
    ("PathfinderOutline",
     "only `pathfinderBoolean` is bridged; the seven REGION verbs never got a fn, so a script can boolean two paths but cannot divide, trim or take faces"),
    ("PathfinderMinusBack",
     "only `pathfinderBoolean` is bridged; the seven REGION verbs never got a fn, so a script can boolean two paths but cannot divide, trim or take faces"),
    ("PathfinderFaces",
     "only `pathfinderBoolean` is bridged; the seven REGION verbs never got a fn, so a script can boolean two paths but cannot divide, trim or take faces"),
    ("ApplyOpacityMask",
     "opacity masks are panel-only; no `paged.*` fn applies or releases one, though the wire op has been there since the mask lane shipped"),
    ("ReleaseOpacityMask",
     "opacity masks are panel-only; no `paged.*` fn applies or releases one, though the wire op has been there since the mask lane shipped"),
    ("AttachTextToPath",
     "text-on-a-path is panel-only; a script can create the path and the story but cannot marry them"),
    ("DetachTextFromPath",
     "text-on-a-path is panel-only; a script can create the path and the story but cannot marry them"),
    ("LayerSetVisible",
     "the layer ATTRIBUTE setters have no fn: a script can insert, remove and move a layer but cannot rename it or toggle its visible/lock/print flags — the same four the catalog cannot advertise as paths either, for a different reason"),
    ("LayerSetLocked",
     "the layer ATTRIBUTE setters have no fn: a script can insert, remove and move a layer but cannot rename it or toggle its visible/lock/print flags — the same four the catalog cannot advertise as paths either, for a different reason"),
    ("LayerSetPrintable",
     "the layer ATTRIBUTE setters have no fn: a script can insert, remove and move a layer but cannot rename it or toggle its visible/lock/print flags — the same four the catalog cannot advertise as paths either, for a different reason"),
    ("LayerSetName",
     "the layer ATTRIBUTE setters have no fn: a script can insert, remove and move a layer but cannot rename it or toggle its visible/lock/print flags — the same four the catalog cannot advertise as paths either, for a different reason"),
    ("InsertAnchoredFrame",
     "anchored frames are inserted from the editor's Anchored panel; no `paged.*` fn takes the anchor spec"),
    ("InsertHyperlink",
     "hyperlink insertion is panel-only; the read side (`paged.links`) exists, the write side does not"),
    ("ReorderElement",
     "z-order is gesture- and menu-driven in the editor; a script has no fn to raise or lower an element"),
    ("PasteInto",
     "paste-into and release are clipboard verbs the editor owns; a script cannot nest one item inside another"),
    ("ReleaseFrom",
     "paste-into and release are clipboard verbs the editor owns; a script cannot nest one item inside another"),
    ("ClosePath",
     "the two path-topology verbs are pen-tool gestures; the path-point fns (`insert`/`remove`/`curveType`) are bridged and these are not"),
    ("JoinPaths",
     "the two path-topology verbs are pen-tool gestures; the path-point fns (`insert`/`remove`/`curveType`) are bridged and these are not"),
    ("BindCreated",
     "an internal binding op the host issues after a create; a script never needs to name it, and this is the one entry here that is unlikely ever to want a fn"),];

/// Pinned so the list cannot grow quietly.
const BATCH_ONLY_COUNT: usize = 32;

/// The ops the Boa bridge names in its own source. Derived, not listed:
/// the bridge emits `Mutation::X` at the site that implements the fn,
/// so the variant appearing there IS the evidence a fn reaches it.
fn ops_the_bridge_names() -> BTreeSet<String> {
    let src = include_str!("../src/lib.rs");
    paged_wire::MUTATION_NAMES
        .iter()
        .filter(|name| src.contains(&format!("Mutation::{name}")))
        .map(|name| (*name).to_string())
        .collect()
}

#[test]
fn every_wire_op_is_named_by_a_script_fn_or_says_why_not() {
    let named = ops_the_bridge_names();
    let batch_only: BTreeSet<String> = BATCH_ONLY.iter().map(|(op, _)| op.to_string()).collect();
    let all: BTreeSet<String> = paged_wire::MUTATION_NAMES
        .iter()
        .map(|n| (*n).to_string())
        .collect();

    assert!(
        all.len() > 100,
        "only {} ops in the roster — the population may only grow",
        all.len()
    );

    // An op the bridge does not name and this file does not excuse.
    let unclassified: Vec<&String> = all
        .difference(&named)
        .filter(|op| !batch_only.contains(*op))
        .collect();
    assert!(
        unclassified.is_empty(),
        "these wire ops have no `paged.*` fn and no recorded reason — bridge them \
         or add them to BATCH_ONLY with a sentence saying why: {unclassified:?}"
    );

    // The other direction: an excuse that has outlived its gap. This is
    // the half that rots — a fn gets written and the note stays.
    let stale: Vec<&String> = batch_only.intersection(&named).collect();
    assert!(
        stale.is_empty(),
        "these are listed as batch-only and the bridge now names them — delete the \
         lines and lower BATCH_ONLY_COUNT: {stale:?}"
    );

    // And an excuse for an op that does not exist.
    let phantom: Vec<&String> = batch_only.difference(&all).collect();
    assert!(
        phantom.is_empty(),
        "BATCH_ONLY names ops the engine does not have: {phantom:?}"
    );

    assert_eq!(
        batch_only.len(),
        BATCH_ONLY_COUNT,
        "the batch-only list may only shrink; bridging an op means deleting its \
         line and lowering BATCH_ONLY_COUNT in the same commit"
    );
    assert_eq!(
        named.len() + batch_only.len(),
        all.len(),
        "every op is named or excused, never both, never neither"
    );
}

/// An exemption costs a real sentence. A bare "TODO" tells the next
/// reader nothing about whether the gap is deliberate.
#[test]
fn every_reason_is_a_reason() {
    for (op, reason) in BATCH_ONLY {
        assert!(
            reason.len() >= 80,
            "{op}: say why in a sentence, got {reason:?}"
        );
        assert!(
            !reason.to_ascii_lowercase().contains("todo"),
            "{op}: a TODO is not a reason"
        );
    }
}

/// The bridge's own count, so the headline number in the module doc
/// cannot drift from the code without a failure.
#[test]
fn the_script_surface_reaches_85_of_117() {
    assert_eq!(ops_the_bridge_names().len(), 85);
    assert_eq!(paged_wire::MUTATION_NAMES.len(), 117);
}
