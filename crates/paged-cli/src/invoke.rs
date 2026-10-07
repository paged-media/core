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

//! `paged invoke` / `paged ops` — every wire operation as a verb.
//!
//! The CLI had typed verbs for a handful of operations (`set`, `place`,
//! `new`), and the other operations were reachable only by writing a
//! script that called `paged.batch`. `paged invoke <doc> <op> [args]`
//! sends any of them as the one `Mutation` it names, with its arguments
//! as the JSON the wire takes (`{"pageId":"u1","bounds":[0,0,100,100]}`).
//!
//! The op vocabulary is not a list kept here: it is the catalog's
//! `operations` (the roster `Mutation::discriminant` is generated from),
//! so an op added to the engine is a verb on the next build, and the
//! command line's own value check (`--help`, `cli.json`) lists them.
//! What the CLI does NOT invoke is [`NOT_INVOKABLE`], each with why, and
//! `every_catalog_op_is_invokable_or_says_why` keeps the two total.
//!
//! This is not a second general door like a raw `paged wire <json>`: it
//! sends one message kind (`Mutate`), refuses an op the catalog does not
//! list before anything is opened, and the arguments are checked by the
//! op's own type, so a missing or mistyped argument is an error.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};
use paged_canvas::channel::{MainToWorkerKind, Mutation, WorkerToMainKind};
use serde_json::{json, Value};

use crate::engine::Session;
use crate::export::PdfOptions;
use crate::options::DocumentOptions;

/// Operations `paged invoke` does not send on their own, each with why.
///
/// **This list may only shrink.** An op that becomes invokable leaves it.
pub const NOT_INVOKABLE: &[(&str, &str)] = &[(
    "bindCreated",
    "legal only as a child of a batch: it names the element the previous creating child \
     minted, so there is nothing for it to name on its own; invoke a `batch` whose ops \
     carry it",
)];

/// Every wire op tag, from the catalog.
pub fn catalog_ops() -> &'static [String] {
    static OPS: OnceLock<Vec<String>> = OnceLock::new();
    OPS.get_or_init(|| paged_introspect::catalog::api_catalog().operations)
}

/// Why `op` is not invokable, if it is not.
pub fn not_invokable(op: &str) -> Option<&'static str> {
    NOT_INVOKABLE
        .iter()
        .find(|(name, _)| *name == op)
        .map(|(_, why)| *why)
}

/// The ops `paged invoke` sends: the catalog's, minus [`NOT_INVOKABLE`].
pub fn invokable() -> Vec<&'static str> {
    catalog_ops()
        .iter()
        .map(String::as_str)
        .filter(|op| not_invokable(op).is_none())
        .collect()
}

/// The `<OP>` argument's parser: the invokable ops as clap's possible
/// values, so `--help` and the generated `cli.json` list them.
pub fn op_parser() -> clap::builder::PossibleValuesParser {
    clap::builder::PossibleValuesParser::new(invokable())
}

/// The arguments: inline JSON (`{…}`), `-` for stdin, a file path, or
/// nothing (`{}`).
fn read_args(spec: Option<&str>) -> Result<Value> {
    let text = match spec {
        None => return Ok(json!({})),
        Some("-") => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .context("read the arguments from stdin")?;
            s
        }
        Some(s) if s.trim_start().starts_with(['{', '[']) => s.to_string(),
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("read arguments {path}"))?
        }
    };
    serde_json::from_str(&text).context("the arguments are not JSON")
}

/// Build the mutation `op` names from its JSON arguments.
pub fn mutation(op: &str, args: Value) -> Result<Mutation> {
    if let Some(why) = not_invokable(op) {
        bail!("{op} is not invoked on its own: {why}");
    }
    if !catalog_ops().iter().any(|o| o == op) {
        bail!("{op:?} is not an operation (see `paged ops`)");
    }
    serde_json::from_value(json!({ "op": op, "args": args }))
        .map_err(|e| anyhow!("{op}: the arguments do not fit: {e}"))
}

pub fn run(
    doc: &Path,
    assets: &DocumentOptions,
    op: &str,
    args: Option<&str>,
    out: Option<&PathBuf>,
    pdf: &PdfOptions,
) -> Result<()> {
    let mutation = mutation(op, read_args(args)?)?;
    let mut session = Session::new();
    assets.open(&mut session, doc)?;
    let reply = session.send(MainToWorkerKind::Mutate(mutation))?;
    let applied = match reply {
        WorkerToMainKind::MutationApplied {
            applied_seq,
            created_id,
            minted,
            ..
        } => json!({
            "op": op,
            "appliedSeq": applied_seq,
            "createdId": created_id,
            "minted": minted,
        }),
        WorkerToMainKind::MutationFailed { error } => bail!("{op}: {error}"),
        other => bail!("{op}: the engine answered {other:?}"),
    };
    match out {
        None => eprintln!(
            "{op} applied to the loaded document. Nothing saved — pass -o to write the result."
        ),
        Some(dest) => {
            let format = crate::export::format_for(dest)?;
            let bytes = crate::export::export_bytes(&mut session, format, pdf)?;
            std::fs::write(dest, &bytes).with_context(|| format!("write {}", dest.display()))?;
        }
    }
    println!("{}", serde_json::to_string(&applied)?);
    Ok(())
}

/// `paged ops`: every catalog operation, and whether `paged invoke`
/// sends it.
pub fn list(json_out: bool) -> Result<()> {
    let rows: Vec<Value> = catalog_ops()
        .iter()
        .map(|op| match not_invokable(op) {
            None => json!({ "op": op, "invokable": true }),
            Some(why) => json!({ "op": op, "invokable": false, "reason": why }),
        })
        .collect();
    if json_out {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for op in catalog_ops() {
        match not_invokable(op) {
            None => println!("{op}"),
            Some(why) => println!("{op}  (not invoked on its own: {why})"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_op_is_invokable_or_says_why() {
        let ops = catalog_ops();
        assert!(ops.len() > 100, "only {} catalog ops", ops.len());
        for (op, why) in NOT_INVOKABLE {
            assert!(
                ops.iter().any(|o| o == op),
                "NOT_INVOKABLE names {op}, which the catalog does not have"
            );
            assert!(why.len() >= 80, "{op}: say why in a sentence");
        }
        assert_eq!(
            invokable().len() + NOT_INVOKABLE.len(),
            ops.len(),
            "every op is invokable or excused, never both"
        );
        // Pinned so the excused list cannot grow quietly.
        assert_eq!(NOT_INVOKABLE.len(), 1);
    }

    #[test]
    fn arguments_are_checked_by_the_ops_own_type() {
        let m = mutation(
            "insertTextFrame",
            json!({ "pageId": "u1", "bounds": [0, 0, 100, 100] }),
        )
        .expect("an insertTextFrame");
        assert_eq!(m.discriminant(), "InsertTextFrame");
        assert!(mutation("insertTextFrame", json!({ "pageId": "u1" })).is_err());
        assert!(mutation("notAnOp", json!({})).is_err());
        let err = mutation("bindCreated", json!({ "handle": "a" })).unwrap_err();
        assert!(err.to_string().contains("batch"), "{err}");
    }
}
