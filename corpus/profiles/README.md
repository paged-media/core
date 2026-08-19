# CMYK / RGB profiles for the colour tests

**Profiles are not committed.** Their licences vary — Adobe's
`CoatedFOGRA39.icc`, the one most likely to be installed on a designer's
Mac, is explicitly not redistributable — and this is a public repo. The
`.icc` files here are gitignored and fetched on demand:

```bash
scripts/fetch-profiles.sh          # pulls Ghostscript's default_cmyk.icc
                                   # from a pinned ghostpdl release tag
```

Tests find a profile through
`paged_color::test_profiles::find_cmyk_profile`, which tries, in order:

1. `$PAGED_CMYK_PROFILE` — point it at any `.icc` and this directory is
   irrelevant;
2. any `.icc` in this directory (sorted, so the choice is stable across
   machines — an unstable pick would make ΔE budgets unreproducible);
3. a local Adobe installation (fine locally, absent on every CI runner).

## Why this directory exists at all

The 2026-08-19 corpus audit found **zero `.icc` files anywhere in the
workspace**, while the qcms engine, CMYK export, PDF/X-4, ink coverage
and soft-proofing all need one. About eight tests printed "skipping" and
passed. Worse, three of them looked in three different places — one in
`corpus/calibration`, a directory that has only ever held JSON. Green,
and asserting nothing.
