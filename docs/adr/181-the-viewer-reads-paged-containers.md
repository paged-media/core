# ADR 181 — The viewer opens a `.paged` container from its model part, as the editor does

- **Status:** Accepted 2026-10-17.
- **Scope:**
  - `paged-renderer`: `container::open_document`, which the canvas and the viewer share.
  - `paged-sdk`: `ViewerSession::load`.
  - `paged-canvas`: its load path calls the shared function.

## Context

A `.paged` file is an IDML package that may also carry the native model part
(`paged/core/model/document.pgm`, ADR-021). The editor's canvas opens such a container from that
part, because the part holds the edits not yet written back to the IDML parts. The read-only viewer
imported the IDML parts only. So a `.paged` saved by the editor showed its last IDML export in the
viewer, not the document.

The viewer must not link the mutation engine (`paged-sdk` is published without it, and the publish
workflow audits that). The canvas's load path lived in `paged-canvas`, which does link it.

## Decision

- `paged_renderer::container::open_document(bytes)` opens an IDML package or a `.paged` container:
  - a container with a model part this engine reads is reconstructed from it;
  - one with no part, or a part it cannot read (a stale or foreign format version), imports its
    IDML parts. A stale part is never a load error (ADR-022 Q2).
- A part written before styles carried leading adopts the leading from the package's
  `Styles.xml` (moved here from `paged-canvas`, unchanged).
- It returns the source archive and whether the model part was used.
- `paged-renderer` takes `paged-store`, which depends on the model crates only.
- `ViewerSession::load` opens through it. The canvas does too, and keeps its export-side step
  (cutting next/previous page-number runs) for a document from the model part.

## Consequences

- A `.paged` file opens in the viewer as it opens in the editor.
- `container_open` covers three cases: an IDML package; a container whose model part differs from
  its IDML parts (the part wins); a container with an unreadable part (the IDML parts serve).
- `cargo tree -p paged-sdk` still shows no `paged-mutate`.
