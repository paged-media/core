# tools/indesign-export

Automates the InDesign-side leg of the corpus pipeline. The Rust
generator emits `.idml` files into `corpus/generated/`; this script
opens each one in InDesign and writes a sibling `<stem>.pdf` reference
PDF using the `[High Quality Print]` preset.

## Prerequisites

- macOS with Adobe InDesign 2024 installed (override the default name
  via `INDESIGN_APP="Adobe InDesign 2025"` etc.).
- The fonts the generated samples reference must be installed in the
  system. Phase 0 only uses **Open Sans**; later phases will pin a
  larger fixture font set.

## Usage

```bash
# 1. Generate the IDMLs.
cargo run -p idml-gen -- emit --sample geometry --out corpus/generated

# 2. Run the InDesign export pass. The script activates InDesign,
#    iterates corpus/generated/*.idml, and writes corpus/generated/*.pdf.
bash tools/indesign-export/run-export.sh

# 3. Run the diff harness. corpus/samples/diff.sh now resolves either
#    corpus/samples/<name>.{idml,pdf} or corpus/generated/<name>.{idml,pdf}.
bash corpus/samples/diff.sh geometry
```

## Outputs

For every `corpus/generated/<stem>.idml` the export pass writes:

```
corpus/generated/<stem>.pdf
corpus/generated/<stem>.export.meta.json
```

The `meta.json` records:

```json
{
  "idml": "geometry.idml",
  "pdf": "geometry.pdf",
  "indesign_version": "20.x",
  "preset": "[High Quality Print]",
  "exported_at": "2026-04-29T..."
}
```

Pin the InDesign version when the corpus is committed and re-export
only on conscious upgrades — InDesign's PDF output is not
deterministic across point releases.

## Caveats

- The driver runs with `userInteractionLevel = NEVER_INTERACT`, which
  suppresses *most* dialogs but not all. Missing fonts, missing
  links, or permission prompts will still pop. Plan a one-time
  manual pass after each new sample lands.
- Every InDesign export advances the application's recently-opened-
  files list. There's no public API to suppress that.
- `ExportFormat.PDF_TYPE` corresponds to the print PDF path. For
  interactive samples (Phase 4+, if interactive PDF lands in scope)
  swap to `ExportFormat.INTERACTIVE_PDF` and a different preset.
