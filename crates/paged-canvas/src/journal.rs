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

//! The journal — engine-side entries for the editor's local flight
//! recorder (ADR 025).
//!
//! # Why this is not `RenderDiagnostics`
//!
//! [`crate::channel`]'s neighbour module [`paged_renderer::diagnostics`]
//! answers "what is wrong with the DOCUMENT" — overset text, a missing
//! link, a substituted font — and those are user-actionable facts keyed to
//! a place in the user's content. This answers "what did the PROGRAM do",
//! keyed to a moment in time, and it is developer-actionable. Diagnostics
//! ride on `BuiltDocument` and reach a Problems panel; journal entries ride
//! on the reply envelope and reach a ring buffer. The two are RELATED, not
//! merged: [`RenderDiagnostics`] projects into the journal as counts by
//! code, and its `message` / `uri` / `story_id` deliberately do not cross.
//!
//! # Why there is no message field
//!
//! A free-text field is where PII leaks in every telemetry system ever
//! built. The `code` IS the message; the human sentence lives in a static
//! table on the viewer side. A variable goes in `data` as a number, a bool,
//! or an IDENTIFIER — and [`JournalValue::ident`] is the ONLY way to build
//! the string case, so the privacy contract is a type-level fact rather
//! than a review convention. A file path, a sentence, a font family or a
//! story's text cannot be represented here at all.
//!
//! The TypeScript side (`editor/packages/client/src/journal/entry.ts`)
//! mirrors this predicate exactly, and both sides carry the SAME
//! accept/reject vectors in their own tests — the Rust ones below, the TS
//! ones in `editor/scripts/journal.test.mjs`.
//!
//! They are duplicated rather than shared, and that is a real (small) gap:
//! the editor consumes this crate as a published npm package, so there is no
//! file both test suites can read today. The fix, when the engine-side door
//! actually ships, is to publish the vectors WITH the wasm package and have
//! the TS test read them from `node_modules`. Until then, a change to
//! `is_ident` here must be mirrored there by hand.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Severity of a journal entry. Ordered so a consumer can filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalSeverity {
    Debug,
    /// The default: `debug` is gated off by consumers, `info` and above
    /// always record, because a flight recorder that is off when the bug
    /// happens is worthless.
    #[default]
    Info,
    Warn,
    Error,
}

/// The only value shapes `data` may carry.
///
/// Flat and scalar BY DESIGN: it makes an entry trivially cloneable across
/// the structured-clone boundary on the JS side, and leaves nowhere for a
/// nested blob of user content to hide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JournalValue {
    Num(f64),
    Bool(bool),
    Ident(String),
}

impl JournalValue {
    /// The ONLY constructor for a string value.
    ///
    /// Returns `None` for anything that is not a lowercase identifier —
    /// which is to say, for anything that could be user content. Callers
    /// that hold a genuinely machine-authored id (a wire message kind, a
    /// diagnostic code) get `Some`; callers holding a URI, a font family or
    /// a story fragment get `None` and must drop it.
    ///
    /// Rejected values are DROPPED by the caller, never truncated: a
    /// truncated path is still a path.
    pub fn ident(s: &str) -> Option<Self> {
        if is_ident(s) {
            Some(Self::Ident(s.to_string()))
        } else {
            None
        }
    }

    /// Normalise a MACHINE identifier — a wire kind, a diagnostic code —
    /// to the identifier rule.
    ///
    /// These are code-authored constants from a closed set, never user
    /// input, and many are CamelCase on the wire (`LoadDocument`).
    /// Lowercasing is lossless for grouping and keeps `data` uniformly
    /// safe, so the emit sites that carry such an id route it through here
    /// rather than widening the predicate — which would also start
    /// admitting single-word font families.
    pub fn machine_ident(s: &str) -> Option<Self> {
        Self::ident(&s.to_lowercase())
    }
}

/// `^[a-z0-9][a-z0-9._:-]{0,63}$`, hand-rolled to avoid a regex dependency
/// in the engine's hot crate.
///
/// Deliberately narrow enough that a path (`/Users/...`, capitals and `/`),
/// a sentence (spaces), a font family (`Helvetica Neue`) or a URI (`://`)
/// cannot pass.
pub fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    if s.len() > 64 {
        return false;
    }
    s.chars()
        .skip(1)
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | ':' | '-'))
}

/// One engine-side journal entry.
///
/// Rides on [`crate::channel::WorkerToMain`] as an additive
/// `#[serde(default)]` field — governance rule 1, so shipping this costs no
/// `PROTOCOL_VERSION` bump (`channel.rs` has the same precedent for the
/// W1.24 `RebuildStats` breakdown).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    /// Dotted, stable, machine-matchable: `engine.<thing>.<outcome>`.
    /// APPEND-ONLY, exactly like `DiagnosticCode`.
    pub code: String,
    #[serde(default)]
    pub severity: JournalSeverity,
    /// Milliseconds, when the entry describes something that took time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur_ms: Option<f64>,
    /// Correlation scalar — the wire `seq` this entry was produced under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corr: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub data: BTreeMap<String, JournalValue>,
}

impl JournalEntry {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity: JournalSeverity::Info,
            dur_ms: None,
            corr: None,
            data: BTreeMap::new(),
        }
    }

    pub fn severity(mut self, severity: JournalSeverity) -> Self {
        self.severity = severity;
        self
    }

    pub fn dur_ms(mut self, ms: f64) -> Self {
        self.dur_ms = Some(ms);
        self
    }

    pub fn corr(mut self, seq: u64) -> Self {
        self.corr = Some(seq);
        self
    }

    pub fn num(mut self, key: &str, value: impl Into<f64>) -> Self {
        self.data
            .insert(key.to_string(), JournalValue::Num(value.into()));
        self
    }

    pub fn bool(mut self, key: &str, value: bool) -> Self {
        self.data.insert(key.to_string(), JournalValue::Bool(value));
        self
    }

    /// Insert a string value, DROPPING it when it is not a safe identifier.
    ///
    /// Dropping rather than escaping is the point: there is no encoding of
    /// a document's text that belongs in this file.
    pub fn ident(mut self, key: &str, value: &str) -> Self {
        if let Some(v) = JournalValue::ident(value) {
            self.data.insert(key.to_string(), v);
        }
        self
    }

    /// Insert a MACHINE identifier (a wire kind, a diagnostic code),
    /// lowercased. See [`JournalValue::machine_ident`].
    pub fn machine(mut self, key: &str, value: &str) -> Self {
        if let Some(v) = JournalValue::machine_ident(value) {
            self.data.insert(key.to_string(), v);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_anything_a_user_could_have_typed() {
        for bad in [
            "the quick brown fox",                // prose
            "/Users/alice/Documents/secret.idml", // absolute path
            "C:\\Users\\alice\\secret.idml",      // windows path
            "https://example.com/x?token=abc",    // URI
            "Helvetica Neue",                     // font family
            "alice@example.com",                  // email
            "Chapter 1: The Beginning",           // document text
            "UPPERCASE",
            "",
        ] {
            assert!(!is_ident(bad), "must reject: {bad}");
            assert!(JournalValue::ident(bad).is_none(), "must not build: {bad}");
        }
        // Over the length cap.
        assert!(!is_ident(&"a".repeat(65)));
    }

    #[test]
    fn accepts_the_identifiers_we_actually_emit() {
        for good in [
            "load-document",
            "engine.load.parse",
            "overset_text_dropped",
            "media.paged.draw",
            "k3f9a2b",
            "a",
        ] {
            assert!(is_ident(good), "must accept: {good}");
        }
    }

    #[test]
    fn machine_ident_lowercases_wire_kinds_but_is_no_back_door() {
        assert_eq!(
            JournalValue::machine_ident("LoadDocument"),
            Some(JournalValue::Ident("loaddocument".into()))
        );
        // Still not a way to smuggle prose in.
        assert!(JournalValue::machine_ident("The Quick Brown Fox").is_none());
        assert!(JournalValue::machine_ident("/Users/Alice/x.idml").is_none());
    }

    #[test]
    fn unsafe_values_are_dropped_not_stored() {
        let e = JournalEntry::new("engine.test")
            .ident("safe", "paged.pen")
            .ident("unsafe", "/Users/alice/secret.idml")
            .num("n", 3.0);
        assert!(e.data.contains_key("safe"));
        assert!(
            !e.data.contains_key("unsafe"),
            "a path must never be stored"
        );
        assert_eq!(e.data.len(), 2);
    }

    #[test]
    fn serialises_without_empty_noise() {
        let json = serde_json::to_string(&JournalEntry::new("engine.load.parse")).unwrap();
        // No `data: {}`, no `durMs: null` — an entry is mostly empty and the
        // wire should not pay for that on every reply.
        assert_eq!(json, r#"{"code":"engine.load.parse","severity":"info"}"#);
    }

    #[test]
    fn round_trips_through_json() {
        let e = JournalEntry::new("engine.dispatch")
            .dur_ms(1.5)
            .corr(42)
            .machine("kind", "LoadDocument")
            .num("n", 2.0)
            .bool("gpu", true);
        let json = serde_json::to_string(&e).unwrap();
        let back: JournalEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
