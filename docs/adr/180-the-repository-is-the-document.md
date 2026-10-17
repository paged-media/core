# ADR 180 — The repository is the document; a file is a projection

- **Status:** Proposed 2026-10-17.
- **Scope:**
  - New crate `paged-repo`: content-addressed objects (blob, chunk list, tree, commit), refs,
    a hash-chained evidence log, the governance checks every backend shares, sync
    negotiation, and `.paged`/`.idml` import, projection and history bundles.
  - No wire or protocol change. `paged-store` (the model codec) and the `.paged` container
    (ADR 118) are unchanged; `paged-repo` stores what they write.
  - The format is specified in [`reference/paged-repo-format.md`](../reference/paged-repo-format.md).

## Context

A document is persisted today as one file: the editor asks the engine for `.paged` bytes and
writes them to a file handle or a download. A file has no history, says nothing about who
changed it or why, cannot be proposed against, and is rewritten whole on every save even when
one story changed. When an agent edits a document, nothing records that it did.

A governed store needs exactly the opposite: every change attributable, checked against a
policy, reviewable and reversible, and a store that holds only what is not derivable. The
smallest authoritative set that gives this is immutable content-addressed blobs, a commit
graph over them, a few movable refs, and a log of what was read, exported or refused.

The editor must keep working with no server at all. So whatever holds the authoritative state
must run in the browser, in the CLI and on a server with one implementation.

## Decision

1. **The authoritative state of a document is a repository**: blobs, chunk lists, trees and
   commits addressed by domain-separated BLAKE3; named refs; an append-only, hash-chained
   evidence log. Nothing else is authoritative.
2. **Files are projections.** A `.paged` file is the deterministic ZIP of one tree. An `.idml`
   file, a PDF and a thumbnail are engine projections of a tree. Each is rebuildable and is
   addressed by its recipe (source tree, recipe, engine version), not by its bytes.
3. **Opening a file is an import.** It produces a tree and a first commit that records
   `imported_from` (file name and SHA-256 of the file bytes). The file itself is not changed.
4. **Saving is a commit.** An explicit save commits the working state to the current branch.
   Crash recovery may keep a working state, which is scratch and never history.
5. **History travels in the file when asked.** A `.paged` projection may carry the repository
   under `paged/repo/` (objects, refs, evidence). InDesign ignores `paged/` (ADR 118), so the
   file stays a valid IDML package. The default projection is thin.
6. **A part is the unit of version.** A tree lists the container's parts, each hashed over its
   uncompressed bytes, so ZIP timestamps, entry order and deflate level do not change an
   address. A part above 256 KiB is split by content-defined chunking (FastCDC, 64 KiB
   average) into a chunk list, so an edit stores the changed chunks only. Images are already
   their own parts (pgm v4).
7. **Governance runs in the kernel, the same in every backend.** Refs have kinds: `main`,
   `drafts/<actor>/…`, `agent/<run>/…`, `releases/…`. A release is created once and never
   moves; "published" means a release. An agent commits only with provenance and only to
   `agent/…` unless a policy grants more; an actor never approves its own change; a missing or
   malformed policy refuses every write.
8. **The repository does no I/O.** Like the engine (ADR 109), `paged-repo` computes objects and
   checks; a host persists them. A host's store is a small synchronous trait; hosts whose
   storage is asynchronous persist the objects first and then commit the ref.

## Consequences

- Every backend (memory, the browser's private file system, a server) must produce identical
  addresses for the same input; a certification test enforces it.
- A server becomes an optional remote. The browser alone has history, branches, releases and
  evidence.
- Merge is per part. Two branches that changed different parts merge; two that changed the
  same part conflict and the user chooses. Semantic diff and merge need the model itself to be
  stored as an element tree with stable identities, and commits to carry mutation operations.
  That is a later decision for `paged-store`, not part of this one.
- Two writers can race on a ref. A ref moves only by compare-and-set, so one wins and the other
  gets a conflict to resolve.
