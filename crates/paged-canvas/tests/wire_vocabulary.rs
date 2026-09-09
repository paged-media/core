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

//! The wire op vocabulary is one list, and it is the one serde speaks.
//!
//! `MUTATION_NAMES` exists so the 117 ops can be ENUMERATED, not just
//! named one at a time by `discriminant`. Everything that needs the
//! population used to re-derive it: the render sweep scraped
//! `discriminant`'s match arms out of the source text, and `state`'s
//! `OP_FEATURE` hand-copied them into another repo, where the copy is
//! still pinned at "94 ops at v40" against the engine's 117.
//!
//! A roster is only worth having if it is the same roster the wire
//! answers to, so that is measured here rather than assumed — serde
//! names every variant it accepts in its own error message, and that is
//! what the tags are compared against.

use std::collections::BTreeSet;

use paged_canvas::channel::{wire_tag_of, Mutation, MUTATION_NAMES};

/// The vocabulary serde will actually accept, read out of its own
/// "unknown variant" message. Not a list we maintain — the deserializer's.
fn serde_vocabulary() -> BTreeSet<String> {
    let err = serde_json::from_str::<Mutation>(r#"{"op":"__not_an_op__"}"#)
        .expect_err("an unknown op must not deserialize")
        .to_string();
    let list = err
        .split_once("expected one of ")
        .expect("serde names the variants it expects")
        .1;
    // Backtick-delimited, because the message tails off into
    // " at line 1 column N" after the last one.
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

#[test]
fn the_roster_is_exactly_what_serde_accepts() {
    let ours: BTreeSet<String> = MUTATION_NAMES.iter().map(|n| wire_tag_of(n)).collect();
    let theirs = serde_vocabulary();
    let missing: Vec<_> = theirs.difference(&ours).collect();
    let phantom: Vec<_> = ours.difference(&theirs).collect();
    assert!(
        missing.is_empty() && phantom.is_empty(),
        "MUTATION_NAMES and the wire disagree.\n  the wire accepts, we do not list: {missing:?}\n  we list, the wire rejects: {phantom:?}"
    );
    assert_eq!(
        ours.len(),
        MUTATION_NAMES.len(),
        "two ops share a wire tag — the camelCase rule collided"
    );
}

/// The population is the whole point; guard it against silently
/// collapsing the way a scraper can when the source it reads moves.
#[test]
fn the_roster_is_whole() {
    assert!(
        MUTATION_NAMES.len() >= 117,
        "only {} ops in the roster — it may only grow",
        MUTATION_NAMES.len()
    );
    let unique: BTreeSet<&&str> = MUTATION_NAMES.iter().collect();
    assert_eq!(unique.len(), MUTATION_NAMES.len(), "a name is listed twice");
    for name in MUTATION_NAMES {
        let tag = wire_tag_of(name);
        assert!(
            name.starts_with(|c: char| c.is_ascii_uppercase()),
            "{name} is not PascalCase, so the wire tag rule does not apply to it"
        );
        assert_ne!(&tag, name, "{name}: the wire tag must differ in case");
    }
}

/// `discriminant` and the roster are generated from the same tokens, so
/// an instance's own name must be in it. One instance is enough to prove
/// the macro wired both outputs to the same list.
#[test]
fn an_instance_names_itself_from_the_roster() {
    let m = Mutation::Batch { ops: Vec::new() };
    assert!(
        MUTATION_NAMES.contains(&m.discriminant()),
        "{} is not in the roster it was generated with",
        m.discriminant()
    );
    assert_eq!(m.wire_tag(), "batch");
}
