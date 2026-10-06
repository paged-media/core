# Architecture decision records

An ADR records one load-bearing decision that has already been made: what was decided, what
in the code shows it, and what it obliges other code to do. It is a record, not a proposal.
When the code stops matching a record, the body is left as it is and a dated amendment is
added at the end.

ADR numbers are unique across the paged-media repositories, so a number names the same
record wherever it is cited. New records in this repository use 100–199. The records below
100 predate that scheme and keep their numbers; the numbers missing between them belong to
other repositories. Records 100–124 were written on 2026-10-02 from the code as it stood,
for decisions made earlier; their status says so. Number 122 is reserved for a record that
is not written yet.

| ADR | Title | Status |
|---|---|---|
| [001](001-boa-over-quickjs.md) | Scripting runtime: Boa, reversing the QuickJS decision | Accepted (amended 2026-10-02) |
| [003](003-lcms2-color.md) | Color CMM: lcms2 native, qcms on wasm | Accepted |
| [004](004-kurbo-geometry-kernel.md) | Path geometry kernel: kurbo (booleans stay flo_curves) | Accepted (amended 2026-10-02) |
| [005](005-wire-recipe.md) | The wire recipe: every operation self-describing and invertible | Accepted (amended 2026-10-02) |
| [006](006-protocol-coupled-versioning.md) | Protocol-coupled package versioning (`0.<protocol>.<patch>`) | Accepted (amended 2026-10-02) |
| [007](007-carry-through-rendering-honesty.md) | Rendering & save-back honesty: parse-don't-fake, carry-through | Accepted (amended 2026-10-02) |
| [008](008-read-surfaces-first-class-wire-collections.md) | Read surfaces as first-class wire collections | Accepted (amended 2026-10-02) |
| [013](013-in-frame-scenelayer.md) | In-frame plugin rendering via `SceneLayer` (C-1) | Accepted (amended 2026-10-02) |
| [018](018-stage-b-gpu-texture-defer-record-only.md) | C-1 Stage B (shared GPUDevice + plugin GPUTexture): record-only deferral | Accepted (amended 2026-10-02) |
| [019](019-capability-catalog-one-contract.md) | Capability catalog: one generated contract, projected to every surface | Accepted (amended 2026-10-02) |
| [021](021-paged-native-document-model-idml-as-format.md) | Paged-native document model; IDML becomes an import/export format | Accepted (amended 2026-10-02) |
| [026](026-auto-growing-region-chains.md) | Auto-growing region chains: pages grow in core, at composition time | Accepted (amended 2026-10-02) |
| [027](027-incremental-flow-invalidation.md) | Incremental flow invalidation: a complete key, an early stop, and a digest that proves it | Accepted 2026-10-02 (amended 2026-10-02); implemented in part |
| [028](028-pagination-rules-are-engine-owned.md) | Pagination rules are engine-owned and shared by every format | Accepted (amended 2026-10-02) |
| [100](100-two-rasterisers-one-trait.md) | Two rasterisers behind one trait: Vello/WebGPU is the forward surface, tiny-skia the path of record | Accepted, recorded retroactively 2026-10-02 |
| [101](101-display-list-single-intermediate.md) | The display list is the single intermediate; a digest proves two builds produce the same scene | Accepted, recorded retroactively 2026-10-02 |
| [102](102-text-stack.md) | The text stack: harfrust shaping, total-fit line breaking, composers measured against InDesign | Accepted, recorded retroactively 2026-10-02 |
| [103](103-hyphenation-sources.md) | Hyphenation sources and their precedence | Accepted, recorded retroactively 2026-10-02 |
| [104](104-effects-follow-indesign-parameters.md) | Object effects are modelled on InDesign's parameters over one shared mask pipeline | Accepted, recorded retroactively 2026-10-02 |
| [105](105-fidelity-gate.md) | The fidelity gate: CPU raster against a raster of InDesign's own PDF, thresholds that only tighten | Accepted, recorded retroactively 2026-10-02 |
| [106](106-colour-resolved-at-build-time.md) | Colour is resolved to linear RGB at build time; CMYK channels and spot inks ride along | Accepted, recorded retroactively 2026-10-02 |
| [107](107-whole-document-build.md) | Layout is a pure whole-document build; a batch pays for one rebuild | Accepted, recorded retroactively 2026-10-02 |
| [108](108-plugin-raster-tiles.md) | Plugin raster content enters as tiles through the ordinary image lane | Accepted, recorded retroactively 2026-10-02 |
| [109](109-engine-does-no-io.md) | The engine does no I/O and ships no assets; the host registers fonts and profiles | Accepted, recorded retroactively 2026-10-02 |
| [110](110-one-undo-timeline.md) | One undo timeline, by pre-captured inverses | Accepted, recorded retroactively 2026-10-02 |
| [111](111-wire-vocabulary-leaf-crate.md) | The wire vocabulary is one leaf crate, split by feature | Accepted, recorded retroactively 2026-10-02 |
| [112](112-viewer-sdk-is-a-sibling.md) | The read-only viewer SDK is a sibling of the editor wasm, enforced by a dependency audit | Accepted, recorded retroactively 2026-10-02 |
| [113](113-one-typed-door.md) | One typed door drives every surface: wasm, CLI, session and scripts | Accepted, recorded retroactively 2026-10-02 |
| [114](114-interaction-lives-in-the-engine.md) | Interaction lives in the engine: hit testing, selection, gestures, snapping | Accepted, recorded retroactively 2026-10-02 |
| [115](115-worker-boundary-transports.md) | The worker boundary uses three transports | Accepted, recorded retroactively 2026-10-02 |
| [116](116-mutations-lowered-onto-operations.md) | Wire mutations are lowered onto invertible operations | Accepted, recorded retroactively 2026-10-02 |
| [117](117-story-local-text-offsets.md) | Text is addressed by story-local offsets: bytes for edits, characters for ranges | Accepted, recorded retroactively 2026-10-02 |
| [118](118-paged-file-is-a-valid-idml-package.md) | A `.paged` file is a ZIP that stays a valid IDML package | Accepted, recorded retroactively 2026-10-02 |
| [119](119-pdf-export-backend.md) | PDF export is a second backend over the display list | Accepted, recorded retroactively 2026-10-02 |
| [120](120-indesign-is-the-oracle.md) | InDesign is the oracle: a generator authors, InDesign answers, a diff gates | Accepted, recorded retroactively 2026-10-02 |
| [121](121-public-repo-builds-alone.md) | The public engine repo builds and gates without any private repo | Accepted, recorded retroactively 2026-10-02 |
| 122 | Dual licence, per-file header, one contributor agreement | Reserved (not yet written) |
| [123](123-viewer-ships-from-core.md) | The viewer ships from core | Accepted 2026-06-07, recorded here 2026-10-02 |
| [124](124-opacity-masks-native.md) | Opacity masks are a native construct; loss is reported on IDML export | Accepted, recorded retroactively 2026-10-02 |
| [125](125-snapping-lives-in-the-engine.md) | Snapping lives in the engine | Accepted 2026-10-05 |
| [126](126-scene-text-in-its-own-face.md) | Scene-layer text draws in its own face | Accepted 2026-10-05 |
| [127](127-fields-and-document-labels-for-data.md) | Field offsets, typing at a field, document labels and delete undo | Accepted 2026-10-05 |
| [128](128-pages-a-merge-can-make.md) | Pages a merge can make: page handles, copied stories, margins | Accepted 2026-10-06 |
| [129](129-pages-and-masters-for-presentations.md) | Pages and masters for presentations: reorder, page labels, page links, snapshot masks, master editing | Accepted 2026-10-06 |

Decisions made in other repositories that this engine's code rests on are listed in
[`../README.md`](../README.md).
