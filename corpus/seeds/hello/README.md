# Hello — first self-authored seed IDML

A two-page IDML, hand-written, that exercises the parts of the schema
the renderer supports today:

| Feature | Where |
|---|---|
| Master spread (`<MasterSpread>`) | `source/MasterSpreads/MasterSpread_uad.xml` |
| `AppliedMaster` on `<Page>` | both pages reference `MasterSpread/uad` |
| Multi-page (two `<Spread>`s, one `<Page>` each) | `source/Spreads/Spread_ua.xml`, `Spread_ub.xml` |
| `<Rectangle>` vector frames | hero, sidebar, master band + footer rule |
| `<TextFrame>` with `ParentStory` | both body frames |
| CMYK + RGB + Gray swatches | `source/Resources/Graphic.xml` |
| Paragraph alignment (Left / Center / Justify) | `source/Stories/*` |
| Per-run `FillColor` (mid-paragraph colour change) | `Story_u200.xml` |
| Multiple paragraphs per story | both stories |

The constituents live as plain XML so they're easy to read and diff.
The library test `corpus_seed_hello_renders` zips them into a valid
`.idml` at test time, opens the result through `idml_renderer::Document`,
runs `pipeline::render_document`, and asserts the output:

- 2 pages each at 612 × 792 pt
- master items present on every page (band at top, footer rule)
- page-level frames over the master items

This corpus entry doubles as the smoke test for multi-page output and
master-spread application.
