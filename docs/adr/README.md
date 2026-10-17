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
| [130](130-plugin-faces-shadows-and-batch-handles.md) | Scene-scoped faces, a drop shadow on every shape, handles on every batch lane | Accepted 2026-10-06 |
| [131](131-one-address-grammar-and-selectors.md) | One address grammar for every object, with selectors | Accepted 2026-10-06 |
| [132](132-typed-property-schemas-and-virtual-paths.md) | Typed property schemas, virtual paths and one `Set { address, path, value }` | Accepted 2026-10-06 |
| [133](133-locks-and-deletes-follow-indesign.md) | Locks, container deletes and emptied groups follow InDesign | Accepted 2026-10-12 |
| [134](134-nested-styles-and-opentype-as-indesign-sets-them.md) | Nested styles and OpenType features as InDesign sets them | Accepted 2026-10-12 |
| [135](135-transparency-targets-and-effect-attributes.md) | Transparency per target, group isolation and knockout, and the effect attributes InDesign writes | Accepted 2026-10-13 |
| [136](136-tables-and-footnote-references-are-characters.md) | Tables and footnote references are characters of their line, and the footnote area sits as InDesign sets it | Accepted 2026-10-13 |
| [137](137-object-style-text-frame-options.md) | A text frame takes the options it does not state from its object style | Accepted 2026-10-13 |
| [138](138-positions-are-exact-sums.md) | A position is the exact sum of the advances before it, rounded once | Accepted 2026-10-13 |
| [139](139-jump-object-and-next-column-wraps.md) | Jump-object and next-column wraps, and how wrapped lines break | Accepted 2026-10-13 |
| [140](140-align-to-baseline-grid.md) | A paragraph aligned to the baseline grid stands each line on the next grid line | Accepted 2026-10-14 |
| [141](141-a-line-may-overrun-its-measure-by-a-sliver.md) | A line may overrun its measure by a sliver; the hyphenation zone does not widen the gaps | Accepted 2026-10-13 |
| [142](142-ragged-text-breaks-by-minimum-raggedness.md) | Ragged text breaks by minimum raggedness, and a line keeps its last word by a share of it | Accepted 2026-10-14 |
| [143](143-object-style-wrap-and-anchoring.md) | An item takes the text wrap and anchoring it leaves out from its object style | Accepted 2026-10-14 |
| [144](144-justified-text-prices-over-shrunk-lines.md) | Justified text prices an over-shrunk line instead of forbidding it | Accepted 2026-10-14 |
| [145](145-a-pages-number-comes-from-its-sections.md) | A page's number comes from its sections, not from `Page@Name` | Accepted 2026-10-14 |
| [146](146-the-gpu-lane-is-held-to-the-cpu-lane.md) | The GPU lane is held to the CPU lane | Accepted 2026-10-14 |
| [147](147-a-rotated-frames-gradient-default.md) | A rotated frame's gradient starts where InDesign puts it | Accepted 2026-10-14 |
| [148](148-dashes-are-laid-out-side-by-side.md) | Dashes are laid out side by side, as InDesign lays them out | Accepted 2026-10-14 |
| [149](149-corners-are-read-per-corner.md) | A frame's corners are read per corner, in InDesign's tokens | Accepted 2026-10-14 |
| [150](150-hashes-and-diamonds-are-marks-along-the-path.md) | Hashes and diamonds are marks along the path | Accepted 2026-10-14 |
| [151](151-which-side-text-wraps-to.md) | Which side text wraps to, and the contour it wraps round | Accepted 2026-10-14 |
| [152](152-a-cells-fill-tint.md) | A cell's fill tint, on the cell and from its style | Accepted 2026-10-14 |
| [153](153-a-paragraphs-line-setting.md) | A paragraph's line setting: auto leading, a lone word, the last line, balance, no break | Accepted 2026-10-14 |
| [154](154-a-placed-pdf-is-drawn-from-its-own-operations.md) | A placed PDF is drawn from its own drawing operations | Accepted 2026-10-14 |
| [155](155-column-rules.md) | Column rules: where InDesign draws the rule between two columns | Accepted 2026-10-14 |
| [156](156-how-indesign-numbers-footnotes.md) | How InDesign numbers footnotes: per story, restarted by page, spread or section | Accepted 2026-10-15 |
| [157](157-every-page-numbering-style.md) | Every page-numbering style InDesign offers, formatted as InDesign formats it | Accepted 2026-10-15 |
| [158](158-how-indesign-numbers-a-multi-level-list.md) | How InDesign numbers a multi-level list | Accepted 2026-10-15 |
| [159](159-what-a-committed-change-reports.md) | What a committed change reports, and where a page sits | Accepted 2026-10-15 |
| [160](160-footnotes-in-columns.md) | Footnotes in a frame with columns: under each column, or across the frame | Accepted 2026-10-15 |
| [161](161-a-note-too-tall-for-its-frame.md) | A note too tall for its frame: continued in the next, or its reference moved on | Accepted 2026-10-15 |
| [162](162-the-documents-footnote-options-are-editable.md) | The document's footnote options are editable, and saved as InDesign writes them | Accepted 2026-10-15 |
| [163](163-footnotes-inserted-and-deleted.md) | Footnotes inserted and deleted, over the wire and through a save | Accepted 2026-10-16 |
| [164](164-hyperlinks-deleted-and-edited.md) | Hyperlinks deleted and edited, whatever made them | Accepted 2026-10-16 |
| [165](165-update-table-of-contents.md) | Update Table of Contents, as InDesign writes it | Accepted 2026-10-16 |
| [166](166-a-ruby-wider-than-its-base.md) | A ruby wider than its base, and kana at their proportional widths | Accepted 2026-10-16 |
| [167](167-japanese-lines-and-vertical-text.md) | Japanese lines, and vertical text as horizontal text turned | Accepted 2026-10-16 |
| [168](168-generate-index.md) | Generate Index, as InDesign writes it | Accepted 2026-10-16 |
| [169](169-update-cross-references.md) | Cross-references: a live page number, and Update Cross-References | Accepted 2026-10-16 |
| [170](170-endnotes-as-indesign-writes-them.md) | Endnotes, read and numbered as InDesign writes them | Accepted 2026-10-16 |
| [171](171-pdf-bookmarks.md) | PDF bookmarks, as InDesign exports them | Accepted 2026-10-16 |
| [172](172-editorial-notes.md) | Editorial notes, kept and edited as InDesign keeps them | Accepted 2026-10-16 |
| [173](173-bookmarks-edited.md) | Bookmarks created, renamed, moved and deleted as InDesign does it | Accepted 2026-10-16 |
| [174](174-endnotes-inserted-and-deleted.md) | Endnotes inserted, deleted and converted from footnotes, as InDesign does it | Accepted 2026-10-17 |
| [175](175-table-row-start.md) | A table row starts where its Start Row option asks, as InDesign sets it | Accepted 2026-10-17 |
| [176](176-nested-tables.md) | A table nested in a cell is drawn as any table is, as InDesign draws it | Accepted 2026-10-17 |
| [177](177-table-rows-fit-the-text-area.md) | A frame holds the table rows its text area holds, strokes included | Accepted 2026-10-17 |
| [181](181-the-viewer-reads-paged-containers.md) | The viewer opens a `.paged` container from its model part, as the editor does | Accepted 2026-10-17 |

Decisions made in other repositories that this engine's code rests on are listed in
[`../README.md`](../README.md).
