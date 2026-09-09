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
//! It could not, for 32 of the 117 when this file was written — the
//! whole of swatch, gradient and colour-group CRUD, six of the seven
//! pathfinder region verbs, both opacity-mask ops, text-on-a-path,
//! hyperlinks, anchored frames, z-order and every layer attribute. That
//! was not hypothetical: the editor's own automation layer carries an
//! `editor.mutate({op, args})` escape hatch whose comment says it exists
//! "for content authoring on engines whose `paged.*` Boa lacks the
//! authoring fns".
//!
//! Thirty-one of those are now named. What remains is ONE, and it is
//! the only entry that was ever unlikely to want a fn: `BindCreated` is
//! legal only as a batch child, so a standalone call could not mean
//! anything. The distinction this file exists to keep is between a gap
//! someone can close and a property of the surface — and the list is
//! now entirely the second kind.
//!
//! A script was never BLOCKED on any of them: `paged.batch` deserialises
//! raw wire ops, and `wire_vocabulary.rs` proves every one of the 117
//! tags is accepted there. The gap was between naming a capability and
//! having to know the wire spelling to reach it — which is the
//! difference between a surface and a hole in one.

use std::collections::BTreeSet;

/// Wire ops no `paged.*` function names, each with why.
///
/// **This list may only SHRINK.** Bridging one means deleting its line
/// and lowering `BATCH_ONLY_COUNT` in the same commit. A new entry means
/// an op shipped without a script surface and without a decision.
const BATCH_ONLY: &[(&str, &str)] = &[
    ("BindCreated",
     "legal only as a BATCH CHILD by construction: it names an id the NEXT op in the same batch will mint, so a standalone `paged.bindCreated()` has nothing to bind and could not be given a meaning. The other thirty-one entries this list once held were gaps; this one is the shape of the surface."),
];

/// Pinned so the list cannot grow quietly.
const BATCH_ONLY_COUNT: usize = 1;

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
fn the_script_surface_reaches_116_of_117() {
    assert_eq!(ops_the_bridge_names().len(), 116);
    assert_eq!(paged_wire::MUTATION_NAMES.len(), 117);
}

/// The names the bridge actually installs, read out of `install_bridge`
/// itself. Every registration is one `.function(guarded(f),
/// js_string!("name"), n)` call, so the first string literal in each
/// `.function(` chunk IS the name — derived, never a second list.
fn functions_the_bridge_installs() -> BTreeSet<String> {
    let src = include_str!("../src/lib.rs");
    let start = src
        .find("fn install_bridge")
        .expect("install_bridge moved or was renamed");
    let end = src[start..]
        .find("\n}\n")
        .map(|i| start + i)
        .expect("install_bridge has no end");
    let body = &src[start..end];

    // The two objects are built in order: everything before the `paged`
    // global registration belongs to `paged.`, everything after to
    // `console.`.
    let split = body
        .find("register_global_property(js_string!(\"paged\")")
        .expect("the paged global is registered by name");

    let mut out = BTreeSet::new();
    for (offset, chunk) in body.match_indices(".function(") {
        let rest = &body[offset..];
        let Some(open) = rest.find("js_string!(\"") else {
            continue;
        };
        let after = &rest[open + "js_string!(\"".len()..];
        let Some(close) = after.find('"') else {
            continue;
        };
        let name = &after[..close];
        let prefix = if offset < split { "paged" } else { "console" };
        out.insert(format!("{prefix}.{name}"));
        let _ = chunk;
    }
    out
}

/// The catalog is the CONTRACT every generated consumer reads — the docs
/// site's scripting pages, plugin-sdk's vendored copy, the completeness
/// gate's `paged.*` roster. It carried a hand-written list of host
/// functions with nothing comparing it to the bridge, which is a fourth
/// vocabulary of the kind this campaign has been collapsing: the day
/// thirty-one fns were added it would have gone on advertising the old
/// hundred and nine, and every one of those consumers would have been
/// wrong together.
#[test]
fn the_catalog_names_exactly_the_functions_the_bridge_installs() {
    let installed = functions_the_bridge_installs();
    let catalogued: BTreeSet<String> = paged_introspect::api_catalog()
        .host_functions
        .iter()
        .map(|f| f.name.to_string())
        .collect();

    assert!(
        installed.len() > 100,
        "only {} functions extracted from install_bridge — the extractor is broken, \
         not the bridge",
        installed.len()
    );

    let missing: Vec<&String> = installed.difference(&catalogued).collect();
    assert!(
        missing.is_empty(),
        "installed but not in the catalog — a script can call these and no generated \
         consumer knows they exist: {missing:?}"
    );

    let phantom: Vec<&String> = catalogued.difference(&installed).collect();
    assert!(
        phantom.is_empty(),
        "advertised by the catalog and not installed — a documented call that throws: \
         {phantom:?}"
    );
}

/// The catalog PUBLISHES an id grammar — three worked examples that
/// docs.paged.media prints and a plugin author copies. Nothing checked
/// that the parser accepts them, which made the grammar a third
/// statement of the same rule alongside `ElementId::parse` and the
/// bridge's own doc comments. Two of the three would still be right by
/// luck; the point is that the next one added has to be.
#[test]
fn every_published_id_form_actually_parses() {
    use paged_wire::ElementId;

    let grammar = paged_introspect::api_catalog().id_grammar;
    assert!(
        grammar.len() >= 3,
        "the id grammar came back with {} forms — it is published documentation \
         and must not silently empty",
        grammar.len()
    );
    for form in grammar {
        let parsed = ElementId::parse(form.example);
        assert!(
            parsed.is_some(),
            "the catalog publishes {:?} as an example of {:?}, and the parser \
             rejects it",
            form.example,
            form.form
        );
        // And it is an address, not merely something that parses: it
        // must survive the round trip a caller does when it hands the
        // id back to `paged.set` / `paged.inspect`.
        let id = parsed.unwrap();
        assert_eq!(
            id.to_address().as_deref().and_then(ElementId::parse),
            Some(id.clone()),
            "{:?} does not round-trip",
            form.example
        );
    }
}
