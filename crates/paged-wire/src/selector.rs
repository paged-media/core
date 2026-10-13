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

//! Selectors: a query over the containment tree that returns addresses
//! (ADR 131). This module only PARSES; evaluation against a document is
//! the `query` module's.
//!
//! ## Grammar
//!
//! ```text
//! selector   = chain *( "," chain )                 ; union, in first-seen order
//! chain      = step *( combinator step )
//! combinator = ">"   (with any whitespace around it) ; child
//!            / 1*WS                                 ; descendant
//! step       = head *filter / 1*filter
//! head       = "*" / kind / address / '"' address '"'
//!            / "plugin:" pluginId "/" kind          ; a plugin kind (answered by its plugin)
//! filter     = "[" key [ op value [ "=" value ] ] "]" / ":nth(" n ")"
//! key        = "name" / "label" / "label." labelKey / "kind" / "layer" / "style"
//! op         = "=" / "!=" / "^=" / "*="
//! value      = '"' chars '"' / "'" chars "'" / bare  ; bare stops at "]"
//! ```
//!
//! - A **kind** is any address prefix (`textFrame`, `page`, `style`, …,
//!   case-insensitive) or `frame`, which matches all six page-item kinds.
//! - A bare **address** token runs to whitespace, `,`, `[` or `:nth(`;
//!   quote an address that contains one of those
//!   (`"style:paragraph/ParagraphStyle/Body Text"`).
//! - `[name]` — has a name. `[name=v]`, `!=`, `^=` (prefix), `*=` (contains).
//! - `[label=k]` — carries the label key `k`; `[label=k=v]` and
//!   `[label.k=v]` — with value `v` (the other ops apply to the value).
//! - `[kind=frame]` is the same as the `frame` head.
//! - `[layer=…]` — a page item on the layer with that `Self` id or name.
//! - `[style=…]` — the applied object style (page items), by `Self` id or
//!   name; on a text frame or a story, also any paragraph or character
//!   style the story applies (the whole story, so every frame of a thread
//!   answers): `textFrame[style="ParagraphStyle/Price"]`,
//!   `textFrame[style=Price]`. `[style!=v]` holds when none of them is `v`.
//! - `:nth(n)` keeps the n-th match (1-based) under each parent.
//! - The containment tree: doc → spread → page → page items (groups and
//!   paste-into hosts nest); doc → master → master items; doc → layer →
//!   the items on it; doc → story → table → cell, story → field, story →
//!   the frames anchored in it (an anchored group's members nest); doc →
//!   styles, swatches, gradients and the other resources.
//!
//! Examples: `page:#3 > frame`, `frame[name="price"]`,
//! `textFrame[label.x-paged:oid="a1b2"]`, `layer:ub3 > rectangle`,
//! `style[name^=Head], swatch`.

use serde::{Deserialize, Serialize};

use crate::address::{Address, AddressKind, GrammarError};

/// A parsed selector: the union of its chains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    pub chains: Vec<Chain>,
}

/// `step (combinator step)*`. `steps[0]` has no combinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    pub steps: Vec<(Combinator, Step)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Combinator {
    /// The first step: anywhere in the document.
    Root,
    Child,
    Descendant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub head: Head,
    pub filters: Vec<Filter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Head {
    Any,
    /// A kind. `Frame` matches all six page-item kinds.
    Kind(AddressKind),
    /// `plugin:<pluginId>/<kind>` — objects of a plugin kind. Core owns
    /// none; the host registry fans the step out to the plugin.
    PluginKind {
        plugin_id: String,
        kind: String,
    },
    Address(Address),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    /// The key is present (no operator written).
    Exists,
    Eq,
    Ne,
    Prefix,
    Contains,
}

impl Op {
    /// Does `actual` satisfy `self` against `expected`? `None` = absent.
    pub fn test(self, actual: Option<&str>, expected: &str) -> bool {
        match (self, actual) {
            (Op::Exists, a) => a.is_some(),
            (Op::Eq, Some(a)) => a == expected,
            (Op::Ne, a) => a != Some(expected),
            (Op::Prefix, Some(a)) => a.starts_with(expected),
            (Op::Contains, Some(a)) => a.contains(expected),
            (_, None) => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Filter {
    Name(Op, String),
    /// A label: `key` must be present; `value` (when given) is tested
    /// with `op`.
    Label {
        key: String,
        op: Op,
        value: Option<String>,
    },
    Kind(AddressKind),
    Layer(Op, String),
    Style(Op, String),
    Nth(u32),
}

impl Selector {
    pub fn parse(s: &str) -> Result<Self, GrammarError> {
        Parser { src: s, pos: 0 }.selector()
    }
}

impl std::str::FromStr for Selector {
    type Err = GrammarError;
    fn from_str(s: &str) -> Result<Self, GrammarError> {
        Selector::parse(s)
    }
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

const TOKEN_STOP: &[char] = &[',', '[', ']', '"', '\''];

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }
    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, GrammarError> {
        Err(GrammarError::new(self.pos, msg))
    }
    fn skip_ws(&mut self) -> bool {
        let before = self.pos;
        while self.peek().is_some_and(char::is_whitespace) {
            self.pos += self.peek().map_or(0, char::len_utf8);
        }
        self.pos != before
    }

    fn selector(&mut self) -> Result<Selector, GrammarError> {
        let mut chains = Vec::new();
        loop {
            self.skip_ws();
            chains.push(self.chain()?);
            self.skip_ws();
            match self.peek() {
                None => break,
                Some(',') => self.pos += 1,
                Some(c) => {
                    return self.err(format!("unexpected {c:?}: expected `,`, `>` or the end"))
                }
            }
        }
        Ok(Selector { chains })
    }

    fn chain(&mut self) -> Result<Chain, GrammarError> {
        let mut steps = vec![(Combinator::Root, self.step()?)];
        loop {
            let had_ws = self.skip_ws();
            let comb = match self.peek() {
                Some('>') => {
                    self.pos += 1;
                    self.skip_ws();
                    Combinator::Child
                }
                None | Some(',') => break,
                Some(_) if had_ws => Combinator::Descendant,
                Some(c) => return self.err(format!("unexpected {c:?}")),
            };
            steps.push((comb, self.step()?));
        }
        Ok(Chain { steps })
    }

    fn quoted(&mut self) -> Result<String, GrammarError> {
        let q = self.peek().expect("caller saw a quote");
        let start = self.pos;
        self.pos += 1;
        let Some(end) = self.rest().find(q) else {
            self.pos = start;
            return self.err("unterminated quote");
        };
        let out = self.rest()[..end].to_string();
        self.pos += end + 1;
        Ok(out)
    }

    fn step(&mut self) -> Result<Step, GrammarError> {
        let at = self.pos;
        let head = match self.peek() {
            Some('*') => {
                self.pos += 1;
                Head::Any
            }
            Some('"') | Some('\'') => {
                let s = self.quoted()?;
                Head::Address(Address::parse(&s).map_err(|e| e.shifted(at + 1))?)
            }
            Some('[') | Some(':') => Head::Any,
            Some(c) if c.is_whitespace() || c == '>' || c == ',' => {
                return self.err("expected a step: `*`, a kind, an address or a `[filter]`")
            }
            None => return self.err("expected a step, found the end"),
            Some(_) => {
                let token = self.token();
                if token.is_empty() {
                    return self.err("expected a step");
                }
                self.head(token, at)?
            }
        };
        let mut filters = Vec::new();
        loop {
            match self.peek() {
                Some('[') => filters.push(self.filter()?),
                Some(':') if self.rest().starts_with(":nth(") => {
                    self.pos += ":nth(".len();
                    let close = self
                        .rest()
                        .find(')')
                        .ok_or_else(|| GrammarError::new(self.pos, ":nth( is not closed"))?;
                    let n: u32 = self.rest()[..close]
                        .trim()
                        .parse()
                        .map_err(|_| GrammarError::new(self.pos, ":nth(n) takes a whole number"))?;
                    if n == 0 {
                        return self.err(":nth(n) is 1-based");
                    }
                    self.pos += close + 1;
                    filters.push(Filter::Nth(n));
                }
                _ => break,
            }
        }
        if head == Head::Any && filters.is_empty() && self.src[at..self.pos] != *"*" {
            return Err(GrammarError::new(at, "an empty step"));
        }
        Ok(Step { head, filters })
    }

    /// A bare head token: up to whitespace, `,`, `[` or `:nth(`.
    fn token(&mut self) -> &'a str {
        let rest = self.rest();
        let mut end = rest.len();
        for (i, c) in rest.char_indices() {
            if c.is_whitespace() || TOKEN_STOP.contains(&c) || rest[i..].starts_with(":nth(") {
                end = i;
                break;
            }
        }
        self.pos += end;
        &rest[..end]
    }

    fn head(&self, token: &str, at: usize) -> Result<Head, GrammarError> {
        if let Some(rest) = token.strip_prefix("plugin:") {
            if rest.matches('/').count() == 1 {
                let (plugin_id, kind) = rest.split_once('/').expect("one slash");
                if plugin_id.is_empty() || kind.is_empty() {
                    return Err(GrammarError::new(
                        at,
                        "a plugin kind step is plugin:<pluginId>/<kind>",
                    ));
                }
                return Ok(Head::PluginKind {
                    plugin_id: plugin_id.to_string(),
                    kind: kind.to_string(),
                });
            }
        }
        if token.contains(':') {
            return Address::parse(token)
                .map(Head::Address)
                .map_err(|e| e.shifted(at));
        }
        AddressKind::from_name(token)
            .map(Head::Kind)
            .ok_or_else(|| GrammarError::new(at, format!("{token:?} is not a kind")))
    }

    fn value(&mut self) -> Result<String, GrammarError> {
        self.skip_ws();
        match self.peek() {
            Some('"') | Some('\'') => self.quoted(),
            _ => {
                let rest = self.rest();
                let end = rest.find([']', '=']).unwrap_or(rest.len());
                self.pos += end;
                Ok(rest[..end].trim().to_string())
            }
        }
    }

    fn op(&mut self) -> Option<Op> {
        self.skip_ws();
        for (lit, op) in [
            ("!=", Op::Ne),
            ("^=", Op::Prefix),
            ("*=", Op::Contains),
            ("=", Op::Eq),
        ] {
            if self.rest().starts_with(lit) {
                self.pos += lit.len();
                return Some(op);
            }
        }
        None
    }

    fn filter(&mut self) -> Result<Filter, GrammarError> {
        let open = self.pos;
        self.pos += 1;
        self.skip_ws();
        let key_at = self.pos;
        let rest = self.rest();
        let key_len = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '=' | '!' | '^' | '*' | ']'))
            .unwrap_or(rest.len());
        let key = &rest[..key_len];
        self.pos += key_len;
        let op = self.op().unwrap_or(Op::Exists);
        let value = if op == Op::Exists {
            None
        } else {
            Some(self.value()?)
        };
        // `[label=k=v]`: a second `=` names the label's value.
        let mut second = None;
        if key == "label" && op == Op::Eq && self.rest().starts_with('=') {
            self.pos += 1;
            second = Some(self.value()?);
        }
        self.skip_ws();
        if self.peek() != Some(']') {
            return Err(GrammarError::new(open, "a `[` filter is not closed"));
        }
        self.pos += 1;
        let need = |v: Option<String>, what: &str| {
            v.ok_or_else(|| GrammarError::new(key_at, format!("[{what}] needs a value")))
        };
        Ok(match key {
            "name" => Filter::Name(op, value.unwrap_or_default()),
            "label" => {
                let k = need(value, "label=<key>")?;
                match second {
                    Some(v) => Filter::Label {
                        key: k,
                        op: Op::Eq,
                        value: Some(v),
                    },
                    None => Filter::Label {
                        key: k,
                        op: Op::Exists,
                        value: None,
                    },
                }
            }
            k if k.starts_with("label.") && k.len() > "label.".len() => Filter::Label {
                key: k["label.".len()..].to_string(),
                op,
                value,
            },
            "kind" => {
                let v = need(value, "kind=<kind>")?;
                Filter::Kind(AddressKind::from_name(&v).ok_or_else(|| {
                    GrammarError::new(key_at, format!("{v:?} is not a kind"))
                })?)
            }
            "layer" => Filter::Layer(op, value.unwrap_or_default()),
            "style" => Filter::Style(op, value.unwrap_or_default()),
            "" => return Err(GrammarError::new(key_at, "a filter needs a key")),
            other => {
                return Err(GrammarError::new(
                    key_at,
                    format!(
                        "{other:?} is not a filter key: name | label | label.<key> | kind | layer | style \
                         (property-path filters arrive with the schema table, ADR 132)"
                    ),
                ))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Selector {
        Selector::parse(s).unwrap_or_else(|e| panic!("{s:?}: {e}"))
    }

    #[test]
    fn the_adr_examples_parse() {
        let s = p("page:#3 > frame");
        assert_eq!(s.chains[0].steps.len(), 2);
        assert_eq!(
            s.chains[0].steps[0].1.head,
            Head::Address(Address::PageNumber { n: 3 })
        );
        assert_eq!(s.chains[0].steps[1].0, Combinator::Child);
        assert_eq!(s.chains[0].steps[1].1.head, Head::Kind(AddressKind::Frame));

        let s = p(r#"frame[name="price"]"#);
        assert_eq!(
            s.chains[0].steps[0].1.filters,
            vec![Filter::Name(Op::Eq, "price".into())]
        );

        let s = p(r#"frame[label.x-paged:oid="a1b2"]"#);
        assert_eq!(
            s.chains[0].steps[0].1.filters,
            vec![Filter::Label {
                key: "x-paged:oid".into(),
                op: Op::Eq,
                value: Some("a1b2".into())
            }]
        );

        let s = p(r#"plugin:media.paged.image/layer[name="Logo"]"#);
        assert_eq!(
            s.chains[0].steps[0].1.head,
            Head::PluginKind {
                plugin_id: "media.paged.image".into(),
                kind: "layer".into()
            }
        );
    }

    #[test]
    fn label_forms_of_the_brief() {
        assert_eq!(
            p("[label=x-paged:web]").chains[0].steps[0].1.filters,
            vec![Filter::Label {
                key: "x-paged:web".into(),
                op: Op::Exists,
                value: None
            }]
        );
        assert_eq!(
            p("*[label=vendor=acme]").chains[0].steps[0].1.filters,
            vec![Filter::Label {
                key: "vendor".into(),
                op: Op::Eq,
                value: Some("acme".into())
            }]
        );
    }

    #[test]
    fn unions_descendants_and_quoted_addresses() {
        let s = p(r#"spread rectangle, "style:paragraph/ParagraphStyle/Body Text", swatch"#);
        assert_eq!(s.chains.len(), 3);
        assert_eq!(s.chains[0].steps[1].0, Combinator::Descendant);
        assert_eq!(
            s.chains[1].steps[0].1.head,
            Head::Address(Address::Style {
                family: crate::StyleFamily::Paragraph,
                id: "ParagraphStyle/Body Text".into()
            })
        );
        let s = p("master:ua9 > textFrame:nth(2)");
        assert_eq!(s.chains[0].steps[1].1.filters, vec![Filter::Nth(2)]);
        assert_eq!(p("*").chains[0].steps[0].1.head, Head::Any);
        assert_eq!(
            p("TEXTFRAME").chains[0].steps[0].1.head,
            Head::Kind(AddressKind::TextFrame)
        );
    }

    #[test]
    fn errors_carry_positions() {
        let cases = [
            ("frame[nmae=x]", 6),
            ("page > sprocket", 7),
            ("frame[name=x", 5),
            ("frame,", 6),
            ("page >", 6),
            ("storyRange:s@5..1", 13),
            ("frame:nth(0)", 10),
            ("frame[kind=blob]", 6),
            ("\"page:u1", 0),
        ];
        for (src, pos) in cases {
            let e = Selector::parse(src).expect_err(src);
            assert_eq!(e.position, pos, "{src:?}: {e}");
        }
    }

    #[test]
    fn ops() {
        assert!(Op::Prefix.test(Some("Headline"), "Head"));
        assert!(Op::Contains.test(Some("a-price-b"), "price"));
        assert!(Op::Ne.test(None, "x"));
        assert!(!Op::Eq.test(None, "x"));
        assert!(Op::Exists.test(Some(""), ""));
    }
}
