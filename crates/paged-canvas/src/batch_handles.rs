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

//! RFI C-15 — within-batch symbols ("handles").
//!
//! A `Mutation::Batch` child could not address an id minted by an
//! EARLIER child of the same batch, so every insert-then-style flow had
//! to be split across two batches — two undo steps for what the user did
//! once (paged.draw's Blend, compound-path RELEASE, the appearance bake
//! and the pattern bake all document exactly this floor).
//!
//! The v34 `$created` sentinel was the narrow precedent: it addressed
//! the MOST RECENT creating child, understood by exactly two mutation
//! kinds (`SetPluginMetadata` / `SetElementProperty`). Handles generalise
//! it in both directions — any number of live names, addressable from any
//! mutation kind:
//!
//! ```text
//! [ insertPath …,               // mints an id
//!   bindCreated { handle: "a" },// name it
//!   insertPath …,
//!   bindCreated { handle: "b" },
//!   setElementProperty { elementId: {kind:"polygon", id:"$h:a"}, … },
//!   createGroup { memberIds: [ {…,id:"$h:a"}, {…,id:"$h:b"} ] } ]
//! ```
//!
//! Resolution is a rewrite over the child's own wire encoding, which is
//! why it works for EVERY mutation kind — present and future — instead of
//! a hand-maintained list of arms. Two rules, and only these two:
//!
//! 1. an object that IS a serialised [`ElementId`] (exactly the keys
//!    `kind` + `id`) whose `id` is a reference string is replaced WHOLE,
//!    so the bound element's real kind wins over the placeholder's;
//! 2. a bare string in an ADDRESS position — a key ending in `Id` /
//!    `Ids` — is replaced with the bound element's raw id, except a
//!    `storyId` position, which takes the story the insert minted (a
//!    fresh text frame's `ParentStory`), so `insertTextFrame` +
//!    `insertText` can ride one batch.
//!
//! Everything else is left byte-identical: a `$h:` inside a `text`
//! payload is content, not an address, and is never rewritten.
//!
//! An unresolvable reference in an address position is an ERROR, never a
//! pass-through — the caller's batch fails as a whole (the applier rolls
//! back what landed) rather than half-applying against a literal `"$h:a"`
//! id that no element has.

use std::collections::HashMap;

use crate::channel::Mutation;
use crate::element_selection::ElementId;

/// The reference prefix. `$h:NAME` in an address position resolves to
/// whatever `BindCreated { handle: NAME }` bound earlier in the batch.
pub const HANDLE_PREFIX: &str = "$h:";

/// The v34 sentinel, still spelled exactly as it was. Inside a batch
/// that uses handles it resolves through the same rewrite (so it works
/// for every mutation kind and on the mixed-lane path, not just the two
/// kinds the v34 arm special-cased); a batch with no `bindCreated`
/// child keeps the v34 behaviour untouched.
pub const CREATED_SENTINEL: &str = "$created";

/// What a name is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundHandle {
    /// The element the creating child minted.
    pub element: ElementId,
    /// The story that same child minted (`insertTextFrame` mints a
    /// `ParentStory`; `insertTable` carries its parent story), so a
    /// `storyId` position can address it. `None` for kinds with no
    /// story — referencing one from a `storyId` position is then an
    /// error, not a silent miss.
    pub story_id: Option<String>,
}

/// Everything a rewrite can resolve against: the named handles plus the
/// implicit "most recent creating child" the `$created` sentinel names.
#[derive(Debug, Default, Clone)]
pub struct HandleScope {
    named: HashMap<String, BoundHandle>,
    created: Option<BoundHandle>,
}

impl HandleScope {
    /// Bind `name`, replacing any earlier binding (last write wins —
    /// a loop that reuses one name addresses its current iteration).
    pub fn bind(&mut self, name: impl Into<String>, bound: BoundHandle) {
        self.named.insert(name.into(), bound);
    }

    /// Record the id the most recent creating child minted (what
    /// `$created` names, and what the next `bindCreated` binds).
    pub fn set_created(&mut self, bound: Option<BoundHandle>) {
        self.created = bound;
    }

    /// The pending `$created` binding, if any creating child has run.
    pub fn created(&self) -> Option<&BoundHandle> {
        self.created.as_ref()
    }

    fn resolve(&self, reference: &str) -> Result<&BoundHandle, String> {
        if reference == CREATED_SENTINEL {
            return self.created.as_ref().ok_or_else(|| {
                format!("{CREATED_SENTINEL} addresses no element — no creating child ran before it")
            });
        }
        let name = reference.strip_prefix(HANDLE_PREFIX).ok_or_else(|| {
            // Unreachable through `reference_of`, kept as a guard so a
            // future caller cannot pass an arbitrary string.
            format!("{reference:?} is not a within-batch handle reference")
        })?;
        self.named.get(name).ok_or_else(|| {
            let mut known: Vec<&str> = self.named.keys().map(String::as_str).collect();
            known.sort_unstable();
            format!(
                "unknown batch handle {name:?} (bound in this batch: {}) — a handle must be bound \
                 by a bindCreated child EARLIER in the same batch",
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            )
        })
    }
}

/// Is this string a handle reference (`$h:NAME`) or the `$created`
/// sentinel? Plain ids answer `false` and are never touched.
fn is_reference(s: &str) -> bool {
    s == CREATED_SENTINEL || s.starts_with(HANDLE_PREFIX)
}

/// An ADDRESS position: `elementId`, `frameId`, `storyId`, `memberIds`,
/// … Anything else (`text`, `key`, `name`, numbers) is content and is
/// never rewritten, so a document may legitimately contain the literal
/// `"$h:…"`.
fn is_address_key(key: &str) -> bool {
    key.ends_with("Id") || key.ends_with("Ids")
}

/// Rewrite every handle reference in one batch child. Returns the child
/// unchanged (a clone) when it carries none.
///
/// The rewrite runs over the child's own wire encoding, so it covers
/// every mutation kind; the round-trip back into `Mutation` is what
/// proves the result is still a well-formed mutation.
pub fn substitute(child: &Mutation, scope: &HandleScope) -> Result<Mutation, String> {
    let mut json = serde_json::to_value(child)
        .map_err(|e| format!("cannot encode child for handle resolution: {e}"))?;
    let mut touched = false;
    walk(&mut json, None, scope, &mut touched)?;
    if !touched {
        return Ok(child.clone());
    }
    serde_json::from_value(json)
        .map_err(|e| format!("handle resolution produced an undecodable mutation: {e}"))
}

fn walk(
    value: &mut serde_json::Value,
    key: Option<&str>,
    scope: &HandleScope,
    touched: &mut bool,
) -> Result<(), String> {
    match value {
        serde_json::Value::Object(map) => {
            // Rule 1 — a whole `ElementId`. The placeholder's `kind` is
            // discarded: the caller cannot always know what kind the
            // insert minted, and guessing wrong would address nothing.
            let element_ref = (map.len() == 2)
                .then(|| match (map.get("kind"), map.get("id")) {
                    (Some(serde_json::Value::String(_)), Some(serde_json::Value::String(id)))
                        if is_reference(id) =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .flatten();
            if let Some(reference) = element_ref {
                let bound = scope.resolve(&reference)?;
                *value = serde_json::to_value(&bound.element)
                    .map_err(|e| format!("cannot encode resolved handle: {e}"))?;
                *touched = true;
                return Ok(());
            }
            for (k, v) in map.iter_mut() {
                walk(v, Some(k.as_str()), scope, touched)?;
            }
        }
        // Array items inherit their field's key, so `memberIds: [...]`
        // and `elementIds: [...]` are address positions per item.
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                walk(item, key, scope, touched)?;
            }
        }
        serde_json::Value::String(s) if is_reference(s) => {
            // Rule 2 — bare id string, and ONLY in an address position.
            let Some(key) = key.filter(|k| is_address_key(k)) else {
                return Ok(());
            };
            let bound = scope.resolve(s)?;
            let resolved = if key == "storyId" {
                bound.story_id.clone().ok_or_else(|| {
                    format!(
                        "{s} names a {} which has no story — a storyId position needs a handle \
                         bound to a text frame (or a table)",
                        bound.element.kind_label()
                    )
                })?
            } else {
                bound.element.raw_id().to_string()
            };
            *value = serde_json::Value::String(resolved);
            *touched = true;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_with(name: &str, element: ElementId, story: Option<&str>) -> HandleScope {
        let mut scope = HandleScope::default();
        scope.bind(
            name,
            BoundHandle {
                element,
                story_id: story.map(str::to_string),
            },
        );
        scope
    }

    #[test]
    fn element_address_takes_the_bound_kind_not_the_placeholder() {
        let scope = scope_with("a", ElementId::Polygon("u9".into()), None);
        // The caller wrote `rectangle`; the insert minted a polygon.
        let child: Mutation = serde_json::from_str(
            r#"{"op":"setElementProperty","args":{"elementId":{"kind":"rectangle","id":"$h:a"},
                "path":"frameFillColor","value":{"type":"colorRef","value":"Color/Black"}}}"#,
        )
        .expect("decode");
        let out = substitute(&child, &scope).expect("resolve");
        match out {
            Mutation::SetElementProperty { element_id, .. } => {
                assert_eq!(element_id, ElementId::Polygon("u9".into()));
            }
            other => panic!("expected SetElementProperty, got {other:?}"),
        }
    }

    #[test]
    fn story_position_takes_the_minted_story() {
        let scope = scope_with("f", ElementId::TextFrame("u4".into()), Some("Story/u7"));
        let child: Mutation = serde_json::from_str(
            r#"{"op":"insertText","args":{"storyId":"$h:f","offset":0,"text":"hi"}}"#,
        )
        .expect("decode");
        match substitute(&child, &scope).expect("resolve") {
            Mutation::InsertText { story_id, text, .. } => {
                assert_eq!(story_id, "Story/u7");
                assert_eq!(text, "hi");
            }
            other => panic!("expected InsertText, got {other:?}"),
        }
    }

    #[test]
    fn story_position_on_a_storyless_element_is_an_error() {
        let scope = scope_with("r", ElementId::Rectangle("u4".into()), None);
        let child: Mutation = serde_json::from_str(
            r#"{"op":"insertText","args":{"storyId":"$h:r","offset":0,"text":"hi"}}"#,
        )
        .expect("decode");
        let err = substitute(&child, &scope).expect_err("must not resolve");
        assert!(err.contains("no story"), "{err}");
    }

    #[test]
    fn a_reference_in_a_content_position_is_left_alone() {
        let scope = scope_with("a", ElementId::Polygon("u9".into()), Some("Story/u1"));
        let child: Mutation = serde_json::from_str(
            r#"{"op":"insertText","args":{"storyId":"story1","offset":0,"text":"$h:a"}}"#,
        )
        .expect("decode");
        match substitute(&child, &scope).expect("resolve") {
            // The text is a payload, not an address — it survives verbatim.
            Mutation::InsertText { text, .. } => assert_eq!(text, "$h:a"),
            other => panic!("expected InsertText, got {other:?}"),
        }
    }

    #[test]
    fn unknown_handle_names_itself_and_the_bound_set() {
        let scope = scope_with("a", ElementId::Polygon("u9".into()), None);
        let child: Mutation = serde_json::from_str(
            r#"{"op":"setPluginMetadata","args":{"elementId":{"kind":"polygon","id":"$h:zzz"},
                "key":"x-paged:draw/k","value":null}}"#,
        )
        .expect("decode");
        let err = substitute(&child, &scope).expect_err("must not resolve");
        assert!(err.contains("zzz"), "names the handle: {err}");
        assert!(err.contains('a'), "names what IS bound: {err}");
    }

    #[test]
    fn created_sentinel_resolves_through_the_same_rewrite() {
        let mut scope = HandleScope::default();
        scope.set_created(Some(BoundHandle {
            element: ElementId::Oval("u2".into()),
            story_id: None,
        }));
        let child: Mutation = serde_json::from_str(
            r#"{"op":"setElementProperty","args":{"elementId":{"kind":"rectangle","id":"$created"},
                "path":"frameFillTint","value":{"type":"length","value":50.0}}}"#,
        )
        .expect("decode");
        match substitute(&child, &scope).expect("resolve") {
            Mutation::SetElementProperty { element_id, .. } => {
                assert_eq!(element_id, ElementId::Oval("u2".into()));
            }
            other => panic!("expected SetElementProperty, got {other:?}"),
        }
    }

    #[test]
    fn every_member_of_a_list_address_resolves() {
        let mut scope = scope_with("a", ElementId::Polygon("u1".into()), None);
        scope.bind(
            "b",
            BoundHandle {
                element: ElementId::Polygon("u2".into()),
                story_id: None,
            },
        );
        let child: Mutation = serde_json::from_str(
            r#"{"op":"createGroup","args":{"memberIds":[{"kind":"polygon","id":"$h:a"},
                {"kind":"polygon","id":"$h:b"}]}}"#,
        )
        .expect("decode");
        match substitute(&child, &scope).expect("resolve") {
            Mutation::CreateGroup { member_ids } => {
                assert_eq!(
                    member_ids,
                    vec![
                        ElementId::Polygon("u1".into()),
                        ElementId::Polygon("u2".into())
                    ]
                );
            }
            other => panic!("expected CreateGroup, got {other:?}"),
        }
    }

    #[test]
    fn a_child_with_no_reference_round_trips_untouched() {
        let scope = scope_with("a", ElementId::Polygon("u9".into()), None);
        let child: Mutation = serde_json::from_str(
            r#"{"op":"resizeFrame","args":{"frameId":"tf1","bounds":[1.5,2.5,3.5,4.5]}}"#,
        )
        .expect("decode");
        let out = substitute(&child, &scope).expect("resolve");
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            serde_json::to_value(&child).unwrap()
        );
    }
}
