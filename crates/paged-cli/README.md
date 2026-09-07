# `paged` — the engine on the command line

One binary that opens IDML or `.paged`, authors through the script
bridge, renders what the editor renders, exports IDML / `.paged` / PDF,
and verifies the result.

```bash
cargo build --release -p paged-cli      # → target/release/paged
```

## Why it exists

Everything the engine can do used to be reachable only from a browser.
The older binaries each hold a slice and none holds two: `paged-inspect`
has fonts, ICC, links and every renderer diagnostic but parses IDML
directly, so it cannot see a `.paged` container's native model part;
`paged-run` is the only one that can, but constructs
`CanvasOptions::default()` — no fonts, no ICC — so its sessions shape no
glyphs at all.

`paged` closes that gap by driving **`WorkerCore::dispatch`**, the same
typed door the editor's worker uses. One code path, so the CLI cannot
apply a mutation or load with options the editor cannot express — and
fonts, colour management, PDF export, container parts, scripting and
undo come for free.

## Commands

```bash
paged render  <doc> [--page N|ID] [--all] [--dpi N] -o <file|dir>
paged inspect <doc> [--json]
paged script  <doc> <script.js> [-o out.paged|.idml|.pdf] [--render p.png]
paged export  <doc> --format idml|paged|pdf -o <file>
paged new     [--size letter|a4|WxH] [--format paged|idml] -o <file>
paged diff    <reference.png> <candidate.png> [--json] [--heatmap f.png]
paged gen     emit --sample NAME | emit-all [--out DIR]
paged session                       # the paged-run NDJSON protocol
```

Every document command takes the same asset flags:

```
--fonts <dir|file>                  repeatable; a directory is read one
                                    level deep and each face is asked
                                    for its own family name
--font-family "Name[/Style]=path"   repeatable; wins over --fonts
--font <file>                       fallback face for text that names
                                    no family
--cmyk-profile <name|path>          a path, or a name resolved against
                                    the host's installed profiles
```

### Fonts and colour: the ordering rule

It is the reason `options.rs` exists as a type rather than a set of
flags. Fonts are registered **before** `LoadDocument`, because the
registry seeds shaping at load and a later registration cannot
retroactively shape anything. A colour profile is registered and then
made the working space with `SetColorSettings` **after** load, because
the model binds its profile at load. Getting either backwards produces
a document that looks right in the log and wrong on the page.

When `--cmyk-profile` is absent the document's own declared profile name
is resolved and registered under that name.

**Core ships no fallback face.** Text whose family no registered font
answers for is not shaped, and an unshaped page renders white. The CLI
says so on stderr rather than letting a blank PNG look like a renderer
bug:

```
warning: 2 text run(s) shaped 0 glyphs — no registered font answered
         for them, so the text will not appear.
```

`paged inspect` reports the same counts (`content  1 frame(s), … 0
glyph(s)`).

### Parity

`paged render` reproduces the editor's own canvas render. The assertion
that keeps it honest, against the 134-page annual:

```bash
paged render showcase.paged --page 1 --dpi 163.2 \
  --fonts corpus/fonts \
  --cmyk-profile "Coated FOGRA39 (ISO 12647-2:2004)" -o p1.png
paged diff editor/…/showcase/pages/page-001.png p1.png
```

mean ΔE 0.494, SSIM 0.9996 — and all 134 pages render in ~16 s, against
four minutes for the Playwright round trip it replaces.

### Scripting budgets

`paged script` runs with a **60 s** wall clock (`--timeout`, or `none`
to disable the deadline) and `--max-loop-iterations`. The shipped
`ScriptBudget` default is 2 s, justified in `paged-script` as an
editor-REPL guard — the right rule for a REPL and the wrong one for a
batch CLI, where authoring a 134-page document is the job rather than a
hang.

Raising it *on the wire* would be protocol drift, so `ExecuteScript` and
`session`'s `run-script` both keep the 2 s default: the docs gate
validates its corpus against the shipped default, and quietly raising it
there would let an example that times out in the editor pass the gate.

There is deliberately no `paged.save` / `paged.render` in the bridge —
it holds only a `&mut CanvasModel` and runs identically inside a browser
worker, where a filesystem verb is meaningless. Compose at the CLI
instead:

```bash
paged script doc.paged build.js -o out.pdf --render p1.png
```

## What it is not

`paged-inspect` still exists and still should. It drives the raw
pipeline and can resolve an external `Links/photo.jpg` off the
filesystem; `paged` cannot, because `CanvasModel`'s asset resolver is
fonts-only and images reach it as bytes already in the container. **So
can the editor's canvas — which is the point.** That limit is the parity
contract, not a gap.
