# ADR 004 — Path geometry kernel: kurbo (booleans stay flo_curves)

**2026-06-07 · decision record · status: ACCEPTED (records a resolved Open
Question).**

**Sources:** `crates/paged-mutate/Cargo.toml:27` (`kurbo = "0.11.3"`),
`:28` (`flo_curves = "=0.8.0"` retained); `crates/paged-mutate/src/kurbo_kernel.rs`
(the kernel: `kurbo::stroke(path, &style, &StrokeOpts::default(),
TOLERANCE)` at `:279`); `crates/paged-gpu/src/vello_rs.rs` (paths flattened
into `kurbo::BezPath` for Vello); an internal plugin-draw gap log (the
geometry-kernel entry, B-05 in the code comments · RESOLVED 2026-06-06); open
question 8 of the plugin-draw concept paper (`plugin-draw: docs/concept.md`).

## The decision

**kurbo is the path-geometry kernel for stroke outlining, offsetting, path
simplification, and Bézier flattening; flo_curves stays for boolean ops and
curve fitting.** This resolves plugin-draw Open Q#8 ("evaluate kurbo vs port a
boolean kernel") — answered **kurbo**, not a hand-rolled kernel.

## Why (reconstructed rationale)

The Open Q#8 resolution is recorded as a one-line "leaning kurbo" in an internal
review note + the gap-log entry; the fuller rationale is **reconstructed** from
where kurbo is wired:

- **kurbo is already the renderer's path type.** `crates/paged-gpu/src/vello_rs.rs` builds
  every path command into `kurbo::BezPath` for Vello (Linebender's own curve
  library, same author as Vello). Adopting it for the *mutation* kernel keeps
  one path representation across mutate→render, instead of converting between a
  hand-rolled type and kurbo at the boundary.
- **Stroke outlining is exactly kurbo's `stroke()` job** — production-grade
  expansion with dash/join/cap handling. Hand-rolling outline-stroke / offset to
  the tolerance a faithful renderer needs is a deep, well-trodden problem; kurbo
  solves it. The wire ops `outlineStroke` / `offsetPath` / `simplifyPath`
  (protocol v30) are thin shells over the kurbo kernel.
- **flo_curves kept for what kurbo doesn't do well:** boolean path ops
  (union/intersect/subtract) and curve fitting. Two libraries, each for its
  strength, beat porting a boolean kernel.

## Consequences

- The kernel gates the Tier-A draw rows (outline-stroke / offset). v1 scopes are
  recorded in `kurbo_kernel.rs` docs: offset = single closed contour with
  bevel-ish gap joins; outlineStroke keeps kurbo's raw expansion (correct under
  nonzero fill); paint transfer composes as a caller `Batch`, not in the kernel.
- Undo for these ops is snapshot-inverse (geometry-only), not algebraic.
- Two geometry libraries is a deliberate, accepted cost; consolidating onto one
  was not worth re-implementing the side each library uniquely covers.

## Amendment — 2026-10-02

Checked against the code at `9f933f1`. The decision stands: `paged-mutate` depends on kurbo
0.11.3 (`crates/paged-mutate/Cargo.toml:27`) and on flo_curves, stroke outlining is still
`kurbo::stroke` at `crates/paged-mutate/src/kurbo_kernel.rs:279`, and the kernel operations
are still undone from a snapshot of the previous anchors
(`crates/paged-mutate/src/apply/path_topology.rs:344-349`, `:542-608`). The text above is
imprecise or out of date in four places.

**1. The flo_curves pin.** **Sources** cites `flo_curves = "=0.8.0"`. The requirement is now
`flo_curves = "0.8"` (`crates/paged-mutate/Cargo.toml:28-32`), and the workspace patches the
crate to a fork pinned by revision that carries an upstream fix not yet in a crates.io
release (`Cargo.toml:84-99`; resolved in `Cargo.lock:1101-1103`).

**2. Which library does which job.** The Decision assigns offsetting and simplification to
kurbo. In `crates/paged-mutate/src/kurbo_kernel.rs` the split is finer:

- Stroke outlining — kurbo (`:261-291`). A variable-width outline was added later: it flattens
  the centreline with `kurbo::flatten` (`:333`) and builds the outline contour itself
  (`:311-398`).
- Offset of a closed contour — flo_curves. The parallel curves come from
  `flo_curves::bezier::offset` (`:547`, `:575`) and the crossings are resolved by
  `path_remove_interior_points` (`:546`, `:625-626`); kurbo supplies the enclosed-area measure
  that picks the inset or outset candidate (`:560-562`, `:639`). The function comment
  describes this construction (`:523-537`); the module comment at `:26-31` describes a
  different one (stroke the boundary, then a boolean against the original) and is stale.
- Offset of an open path or of several subpaths — the apply arm sends it to the kurbo stroke
  outline at twice the distance, which yields a closed band
  (`crates/paged-mutate/src/apply/path_topology.rs:496-513`). This supersedes "offset = single
  closed contour" in Consequences for those inputs; the module comment's "open-path offset is
  deliberately deferred" (`kurbo_kernel.rs:30-31`) is stale.
- Simplification — a Ramer–Douglas–Peucker pass written in the crate drops near-collinear
  anchors first (`:403-497`), then kurbo's `simplify_bezpath` fits curves through the rest
  (`:510-515`).
- Nearest point on a path — kurbo (`:685-709`), called from the canvas model
  (`crates/paged-canvas/src/model.rs:8423`).
- Booleans and curve fitting — flo_curves, as decided
  (`crates/paged-mutate/src/pathfinder.rs:32`, `crates/paged-mutate/src/planar.rs:111-116`,
  `crates/paged-mutate/src/path_math.rs:174`).

**3. The renderer and the kernel do not share a path type.** The first rationale bullet says
adopting kurbo "keeps one path representation across mutate→render". In the code each side
converts at its own boundary:

- `crates/paged-mutate/src/kurbo_kernel.rs:36-39` — every kernel function takes and returns the
  model's anchor tables (`PathAnchor`, `subpath_starts`, `subpath_open`); `BezPath` is built
  inside (`:130`) and converted back (`:173`).
- `crates/paged-gpu/src/vello_rs.rs:2544-2574` — the Vello backend builds its `BezPath` from the
  display list's `PathData`, using the kurbo that Vello re-exports (`:114`).
- `crates/paged-mutate/Cargo.toml:27` is the only manifest of a workspace member that names
  kurbo; `paged-gpu` declares Vello 0.10.0 (`crates/paged-gpu/Cargo.toml:38`). `Cargo.lock`
  resolves two kurbo versions: 0.11.3 for `paged-mutate` (`Cargo.lock:2717`) and 0.13.1 under
  Vello 0.10.0 through peniko 0.6.1 (`Cargo.lock:4336`, `:2934`).

**4. The renderer's own stroke geometry does not use the kernel.** "Bézier flattening" in the
Decision holds inside `paged-mutate`. The striped and wavy stroke styles and the inside/outside
stroke alignment are computed in `crates/paged-renderer/src/pipeline/stroke_geom.rs`, which
flattens curves to polylines at a fixed step count (`:58`, `:68`) and offsets them vertex by
vertex (`:212`, `:391`). The comment there calls the polyline "a deliberate approximation"
(`:21-26`) and states the limit of the per-vertex offset (`:381-388`). Its callers are in
`crates/paged-renderer/src/pipeline/shapes.rs:1009-1026` (alignment), `:1395-1403` (striped)
and `:1430` (wavy).
