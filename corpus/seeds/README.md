# Seed corpus

Ten hand-curated IDML documents plus their paired reference PDFs (exported
from InDesign at 300 DPI, sRGB). Populated during Phase 0.

Each seed exercises one subsystem cleanly:

1. `pure-text/`        — single paragraph, single font, no effects
2. `multi-column/`     — column balancing + justification
3. `pure-vector/`      — paths, strokes, fills, gradients; no text
4. `pure-raster/`      — JPEG placement with bicubic resample
5. `blend-modes/`      — each PDF 1.7 blend mode in isolation
6. `drop-shadow/`      — feather + offset + colour multiply
7. `transparency-group/` — isolated + knockout semantics
8. `spot-color/`       — Lab-defined spot vs process conversion
9. `cjk-mojikumi/`     — Japanese composition rules
10. `table-basic/`     — styled cells, row spans

Corpus files are licensed or self-authored only — see the development
plan's "Corpus licensing" note.
