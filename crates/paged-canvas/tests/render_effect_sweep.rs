/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! **The render-effect sweep** — does an op that APPLIES actually
//! render?
//!
//! ## The defect class
//!
//! `LinkFrames` threaded nothing for its whole life. It set the source
//! frame's `next_text_frame` and stopped, but the composer reaches a
//! frame by walking the STORY's chain and `InsertTextFrame` mints every
//! frame with a story of its own — so the target was never reached
//! however correct the pointer was. The op returned `mutationApplied`,
//! the model changed, and not one pixel moved. Fixed in `59c98b5`, found
//! by accident.
//!
//! Unit tests are structurally blind to that: they assert the model
//! changed, and the model *did* change. What nothing asked was whether
//! the RENDERER reads the field that changed. This file asks exactly
//! that, once per mutation, across the whole vocabulary.
//!
//! ## Why the digest is the oracle
//!
//! `DisplayList::digest()` is a stable FNV-1a over every draw command,
//! interned path, gradient / spot pool and image payload — the project's
//! keystone "same code, same scene" tripwire (`paged-sdk`'s native
//! equivalence test rides it, and `paged-run`'s header states the
//! doctrine: digest first, pixels second). It is deterministic, needs no
//! GPU, and costs a build rather than a rasterisation. A moved digest
//! means the page paints differently; an unmoved digest means it paints
//! *identically*, which is a far stronger statement than any pixel-diff
//! threshold can make — and "identical" is precisely the claim under
//! test.
//!
//! The one thing a display list does not carry is the page ENVELOPE, so
//! the fingerprint below folds in each page's id and size. A page that
//! changes size renders differently even when every command is
//! unchanged, and `ResizePage` would otherwise read as a false finding.
//!
//! ## Two fingerprints, because there are two ways to render nothing
//!
//! * **cold** — `CanvasModel::build_for_export`, a full build from the
//!   post-mutation scene (no splice caches, no layout-cache
//!   short-circuit, same fonts and colour settings as the live one).
//!   Answers *does the renderer read this change at all?* An unmoved
//!   cold print is the `LinkFrames` shape: the model holds a field the
//!   composer never consults.
//! * **live** — `CanvasModel::built`, the incrementally rebuilt document
//!   the canvas is actually painting. Answers *did the canvas repaint?*
//!   Cold moved + live unmoved is the other half of the class: a correct
//!   write that failed to invalidate, so the user stares at stale pixels
//!   until something unrelated forces a rebuild.
//!
//! Live-moved-and-cold-unmoved is the third, benign combination: the
//! export build deliberately ignores soft-proof simulation ("export
//! always renders the WORKING space"), so `SetProofSetup` lands there
//! and is still painting. Measuring only one of the two would have
//! filed it as a defect.
//!
//! ## Expectations, not a blanket assertion
//!
//! Many ops legitimately paint nothing. `CreateSwatch` defines a colour
//! nothing references yet; `LayerSetLocked` is an editing affordance the
//! renderer never reads; `SetPluginMetadata` is a label. So every case
//! declares an [`Expect`] and the sweep asserts against THAT:
//!
//! * `Paints` — the digest must move.
//! * `PaintsNothing(reason)` — the digest must NOT move, and the reason
//!   says why the renderer is right not to care.
//! * `PaintsWhenUsed(reason)` — the digest must not move, AND a
//!   follow-up that references the new thing must move it. This is the
//!   stronger form for the create-a-resource ops: it proves the resource
//!   is real rather than inert, which a bare "nothing happened" cannot.
//!
//! The `PaintsNothing` / `PaintsWhenUsed` reasons are as much the point
//! of this file as the failures are. They are the written-down answer to
//! "should this have moved?" — the question nobody had asked when
//! `LinkFrames` shipped.
//!
//! **Do not relax an expectation to clear a red.** An op declared
//! `Paints` whose digest does not move is a finding; work out whether it
//! is a real defect or a bad setup, and fix the one it actually is. Most
//! of the first run's reds WERE bad setups, and each one is now a
//! comment at the case that had it — a black rectangle recoloured black,
//! `NextTextFrame="n"` mistaken for a thread, a paragraph style outranked
//! by the runs' own point size. Those are the ways a sweep like this
//! lies to you, and they are worth more written down than fixed
//! silently.
//!
//! ## The findings ratchet
//!
//! The ops that really do apply-and-render-nothing live in [`KNOWN`]
//! with a diagnosis apiece — what the op writes, what the renderer
//! reads, and why they miss each other — so the sweep runs green in CI
//! while nothing is swept under it. The list is enforced in BOTH
//! directions: an undiagnosed red fails, and so does a diagnosed one
//! that starts passing (someone fixed it; the entry must go). Adding an
//! entry without a real diagnosis is the one way to make this file
//! worthless.

use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use paged_canvas::channel::{ByteBuf, Mutation};
use paged_canvas::element_selection::ElementId;
use paged_canvas::selection::TextCellAddr;
use paged_canvas::{CanvasModel, CanvasOptions, ColorProfileEntry, PageId};
use paged_mutate::operation::PathAnchorSpec;
use paged_mutate::{
    ColorGroupSpec, FaceSelectMode, FieldKind, GradientSpec, GradientStopSpec,
    GuideOrientationSpec, NumberingListSpec, PathPointRole, PathfinderKind, PropertyPath,
    StyleCollection, StyleScope, SwatchSpec, Value, ZOrderTarget,
};
use paged_renderer::BuiltDocument;

// ── expectations ────────────────────────────────────────────────────

/// What a case claims the op does to the page.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// The op must move the fingerprint. A red here is the defect class
    /// this file exists to find.
    Paints,
    /// The op legitimately leaves the page identical, for the stated
    /// reason. The reason must say why the RENDERER is right not to
    /// care — not merely that it doesn't.
    PaintsNothing(&'static str),
    /// The op creates something inert; referencing it is what paints.
    /// The case supplies the reference step, and both halves are
    /// asserted.
    PaintsWhenUsed(&'static str),
}

/// One op under test.
struct Case {
    /// The `Mutation` variant name, verbatim — matched against
    /// `Mutation::discriminant`'s arms by the completeness guard.
    op: &'static str,
    /// A `paged-gen` sample name (`paged_gen::samples::SAMPLES`).
    fixture: &'static str,
    expect: Expect,
    /// Runs BEFORE the baseline fingerprint is taken, so whatever an op
    /// needs to be legal (a link to unlink, a table to edit, two
    /// overlapping shapes to combine) is not counted as the op's own
    /// effect. Returns the mutation to measure.
    prepare: fn(&mut CanvasModel) -> Mutation,
    /// `PaintsWhenUsed` only — reference the thing the op created. Runs
    /// after the no-paint half has been asserted.
    use_it: Option<fn(&mut CanvasModel)>,
}

fn paints(
    op: &'static str,
    fixture: &'static str,
    prepare: fn(&mut CanvasModel) -> Mutation,
) -> Case {
    Case {
        op,
        fixture,
        expect: Expect::Paints,
        prepare,
        use_it: None,
    }
}

fn inert(
    op: &'static str,
    fixture: &'static str,
    why: &'static str,
    prepare: fn(&mut CanvasModel) -> Mutation,
) -> Case {
    Case {
        op,
        fixture,
        expect: Expect::PaintsNothing(why),
        prepare,
        use_it: None,
    }
}

fn when_used(
    op: &'static str,
    fixture: &'static str,
    why: &'static str,
    prepare: fn(&mut CanvasModel) -> Mutation,
    use_it: fn(&mut CanvasModel),
) -> Case {
    Case {
        op,
        fixture,
        expect: Expect::PaintsWhenUsed(why),
        prepare,
        use_it: Some(use_it),
    }
}

/// What actually happened.
#[derive(Debug)]
enum Observed {
    /// Both fingerprints moved — the renderer reads the change and the
    /// canvas repainted.
    Moved,
    /// The full rebuild sees the change and the canvas does not. A
    /// correct write that failed to invalidate: the user stares at
    /// stale pixels until something unrelated forces a rebuild.
    MovedColdOnly,
    /// The canvas repainted but the export build did not. Legitimate
    /// for the VIEW-only settings — `build_for_export` deliberately
    /// ignores soft-proof simulation ("export always renders the
    /// WORKING space"), so an op that only changes the proof transform
    /// lands here and is still painting.
    MovedLiveOnly,
    /// Applied cleanly, page identical.
    Unmoved,
    /// Inert on its own, and referencing it painted — `PaintsWhenUsed`
    /// satisfied in both halves.
    UnmovedThenMoved,
    /// Inert on its own, and referencing it ALSO painted nothing. The
    /// resource is not merely unused, it is unusable.
    UnmovedThenStillUnmoved,
    /// `apply_mutation` returned `Err` — the op never ran, so the case
    /// measured nothing about rendering.
    Rejected(String),
    /// The setup itself failed. A harness artefact, not an engine
    /// result, but still a red: an unmeasured op is an unswept op.
    SetupFailed(String),
}

// ── the oracle ──────────────────────────────────────────────────────

/// A page's full render identity: which page it is, how big it is, and
/// the display-list digest of everything drawn on it.
fn page_prints(doc: &BuiltDocument) -> Vec<(String, u32, u32, u64)> {
    doc.pages
        .iter()
        .map(|p| {
            (
                p.id.0.clone(),
                p.width_pt.to_bits(),
                p.height_pt.to_bits(),
                p.list.digest(),
            )
        })
        .collect()
}

/// A FULL build from the current scene. `build_for_export` is the only
/// public entry point that rebuilds everything — no splice caches, no
/// layout-cache short-circuit — with the model's own fonts and colour
/// settings, which is what makes it the "does the renderer read this at
/// all" oracle.
fn cold(m: &CanvasModel) -> Vec<(String, u32, u32, u64)> {
    page_prints(&m.build_for_export().expect("full build"))
}

/// The document the canvas is currently painting.
fn live(m: &CanvasModel) -> Vec<(String, u32, u32, u64)> {
    page_prints(m.built())
}

// ── fixtures ────────────────────────────────────────────────────────

/// The corpus face every `paged-gen` text sample pins via
/// `AppliedFont="Inter"`. Without it text shapes through a fallback and
/// the digests would still move — but the layout would not be the
/// fixture's, and the overset cases depend on exactly how much text
/// fits.
fn inter() -> Vec<u8> {
    read_corpus("fonts/Inter.ttf")
}

fn read_corpus(rel: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// The name the sweep registers `corpus/profiles/default_cmyk.icc`
/// under. No fixture designmap names it, so it is inert at load and
/// `SetColorSettings` switching TO it is a real working-space change.
const SWEEP_CMYK: &str = "Sweep CMYK";

/// Load a `paged-gen` sample as a `CanvasModel`. Generated in memory
/// rather than read from `corpus/generated/` — those files are
/// gitignored and regenerate from exactly this source, so a sweep that
/// read them would be a fixture-presence test as much as a render test.
fn load_sample(name: &str) -> CanvasModel {
    let sample = paged_gen::samples::build(name)
        .unwrap_or_else(|| panic!("unknown paged-gen sample {name:?}"));
    let bytes = paged_gen::write_idml(&sample).unwrap_or_else(|e| panic!("gen {name}: {e}"));
    let opts = CanvasOptions {
        fonts: vec![inter()],
        color_profiles: vec![ColorProfileEntry {
            name: SWEEP_CMYK.to_string(),
            bytes: read_corpus("profiles/default_cmyk.icc"),
        }],
        ..CanvasOptions::default()
    };
    CanvasModel::load("sweep", &bytes, opts).unwrap_or_else(|e| panic!("load {name}: {e:?}"))
}

// ── discovery helpers ───────────────────────────────────────────────
//
// Every case addresses its target by SEARCHING the loaded model rather
// than by hard-coded id. A fixture regeneration that renames ids then
// fails loudly in one place instead of silently turning a case into a
// no-op against an element that is not there.

fn first_page(m: &CanvasModel) -> PageId {
    page_at(m, 0)
}

fn page_at(m: &CanvasModel, index: usize) -> PageId {
    PageId(
        m.pages()
            .get(index)
            .unwrap_or_else(|| panic!("fixture has no page {index}"))
            .self_id
            .clone(),
    )
}

fn rect_ids(m: &CanvasModel) -> Vec<String> {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .filter_map(|r| r.self_id.clone())
        .collect()
}

/// `(polygon_id, story_id)` of the first path carrying a `<TextPath>`.
fn text_path_host(m: &CanvasModel) -> (String, String) {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.polygons.iter())
        .find_map(|p| {
            let tp = p.text_paths.first()?;
            Some((p.self_id.clone()?, tp.parent_story.clone()))
        })
        .expect("text-on-path fixture carries a polygon with a TextPath")
}

fn first_spread_id(m: &CanvasModel) -> String {
    m.scene().spreads[0]
        .spread
        .self_id
        .clone()
        .expect("spread carries a Self id")
}

/// The story with the most characters — the one whose text edits are
/// most likely to move a line break, and never an empty story whose
/// edits would be invisible for an honest reason.
fn biggest_story(m: &CanvasModel) -> (String, u32) {
    let mut s = m.stories();
    s.sort_by_key(|s| std::cmp::Reverse(s.character_count));
    let top = s.first().expect("fixture contributes a story").clone();
    assert!(
        top.character_count > 0,
        "fixture's largest story is empty — nothing to edit"
    );
    (top.self_id, top.character_count)
}

/// `text-overset` page 1: one frame, decisively overset. The chain-length
/// filter separates it from page 2's two-frame chain unambiguously.
fn lone_overset_frame(m: &CanvasModel) -> String {
    for s in m.stories() {
        if !s.overset {
            continue;
        }
        let frames = frames_on_story(m, &s.self_id);
        if frames.len() == 1 {
            return frames[0].clone();
        }
    }
    panic!("fixture has no single-frame overset story");
}

fn frames_on_story(m: &CanvasModel, story: &str) -> Vec<String> {
    m.scene()
        .spreads
        .iter()
        .flat_map(|sp| sp.spread.text_frames.iter())
        .filter(|f| f.parent_story.as_deref() == Some(story))
        .filter_map(|f| f.self_id.clone())
        .collect()
}

/// The head of the first REAL threaded chain the fixture carries.
///
/// IDML writes `NextTextFrame="n"` for "no next frame" — a literal `n`,
/// not an id — so `next_text_frame.is_some()` is true for every
/// unthreaded frame in the document. Written naively this helper picked
/// a label frame whose thread does not exist, and reported
/// `unlinkFrames` as rendering nothing; the target must be a frame that
/// is actually there.
fn chain_head(m: &CanvasModel) -> String {
    let frames: Vec<(String, Option<String>)> = m
        .scene()
        .spreads
        .iter()
        .flat_map(|sp| sp.spread.text_frames.iter())
        .filter_map(|f| f.self_id.clone().map(|id| (id, f.next_text_frame.clone())))
        .collect();
    let ids: BTreeSet<&str> = frames.iter().map(|(id, _)| id.as_str()).collect();
    frames
        .iter()
        .find(|(_, next)| next.as_deref().is_some_and(|n| ids.contains(n)))
        .map(|(id, _)| id.clone())
        .expect("fixture has no threaded frame chain")
}

/// `(story_id, table_id)` of the first `<Table>` the fixture carries.
fn first_table(m: &CanvasModel) -> (String, String) {
    for s in &m.scene().stories {
        for p in &s.story.paragraphs {
            if let Some(t) = &p.table {
                if let Some(id) = &t.self_id {
                    return (s.self_id.clone(), id.clone());
                }
            }
        }
    }
    panic!("fixture carries no table");
}

/// A `Color/…` id that exists in the fixture's palette and is neither
/// None nor Paper — those two paint nothing by definition and would make
/// a fill write a no-op in disguise.
fn some_color(m: &CanvasModel) -> String {
    m.scene()
        .palette
        .colors
        .keys()
        .find(|k| !k.contains("None") && !k.contains("Paper"))
        .cloned()
        .expect("fixture palette carries a colour")
}

/// Mint a swatch guaranteed to differ from anything the fixture
/// already paints with, and return its id.
///
/// Needed more often than it looks: `geometry`'s whole palette is
/// `Color/Black` + `Color/Paper`, and every rectangle in it is already
/// black — so "set the fill to a palette colour" was a no-op in
/// disguise, and read as `setElementProperty` rendering nothing.
fn fresh_color(m: &mut CanvasModel) -> String {
    const ID: &str = "Color/sweep-fresh";
    m.apply_mutation(&Mutation::CreateSwatch {
        spec: SwatchSpec {
            self_id: Some(ID.into()),
            name: Some("Sweep Fresh".into()),
            space: "RGB".into(),
            value: vec![240.0, 20.0, 140.0],
            model: None,
            alternate_space: None,
            alternate_value: Vec::new(),
            tint: None,
            alpha: None,
        },
    })
    .expect("mint a distinct swatch");
    ID.to_string()
}

/// A colour some page item actually paints with — the one to edit or
/// delete when the case needs the change to be visible.
fn color_in_use(m: &CanvasModel) -> String {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .filter_map(|r| r.fill_color.clone())
        .find(|c| !c.contains("None") && !c.contains("Paper"))
        .expect("fixture has a filled rectangle")
}

fn layer_ids(m: &CanvasModel) -> Vec<String> {
    m.scene()
        .designmap
        .layers
        .iter()
        .map(|l| l.self_id.clone())
        .collect()
}

/// Insert an empty text frame and hand back its minted id.
fn add_text_frame(m: &mut CanvasModel, bounds: (f32, f32, f32, f32)) -> String {
    let page = first_page(m);
    let out = m
        .apply_mutation(&Mutation::InsertTextFrame {
            page_id: page,
            bounds,
        })
        .expect("insert text frame");
    match out.created_id.expect("created id") {
        ElementId::TextFrame(id) => id,
        other => panic!("expected a TextFrame, got {other:?}"),
    }
}

/// The story `insertTextFrame` minted for a frame.
fn story_of(m: &CanvasModel, frame: &str) -> String {
    m.scene()
        .spreads
        .iter()
        .flat_map(|s| s.spread.text_frames.iter())
        .find(|f| f.self_id.as_deref() == Some(frame))
        .and_then(|f| f.parent_story.clone())
        .expect("frame carries a parent story")
}

/// Insert a FILLED rectangle. The fill matters: a shape with no paint is
/// a legitimate no-op, which would make every case built on it
/// untrustworthy.
fn add_rect(m: &mut CanvasModel, bounds: (f32, f32, f32, f32)) -> String {
    let page = first_page(m);
    let color = some_color(m);
    let out = m
        .apply_mutation(&Mutation::InsertFrame {
            page_id: page,
            bounds,
        })
        .expect("insert frame");
    let id = match out.created_id.expect("created id") {
        ElementId::Rectangle(id) => id,
        other => panic!("expected a Rectangle, got {other:?}"),
    };
    fill(m, &ElementId::Rectangle(id.clone()), &color);
    id
}

fn fill(m: &mut CanvasModel, id: &ElementId, color: &str) {
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path: PropertyPath::FrameFillColor,
        value: Value::ColorRef(Some(color.to_string())),
    })
    .expect("fill");
}

fn corner(p: [f32; 2]) -> PathAnchorSpec {
    PathAnchorSpec {
        anchor: p,
        left: p,
        right: p,
    }
}

/// Insert a filled quad as a `Polygon` and hand back its id. The
/// pathfinder / path-edit cases all want real path geometry rather than
/// a Rectangle's derived box.
fn add_quad(m: &mut CanvasModel, x: f32, y: f32, w: f32, h: f32) -> String {
    let page = first_page(m);
    let color = some_color(m);
    let out = m
        .apply_mutation(&Mutation::InsertPath {
            page_id: page,
            anchors: vec![
                corner([x, y]),
                corner([x + w, y]),
                corner([x + w, y + h]),
                corner([x, y + h]),
            ],
            open: false,
            smooth: false,
        })
        .expect("insert path");
    let id = match out.created_id.expect("created id") {
        ElementId::Polygon(id) => id,
        other => panic!("expected a Polygon, got {other:?}"),
    };
    fill(m, &ElementId::Polygon(id.clone()), &color);
    id
}

/// An OPEN polyline with a visible stroke — the input `ClosePath` /
/// `JoinPaths` / `OutlineStroke` need.
fn add_open_path(m: &mut CanvasModel, x: f32, y: f32, w: f32) -> String {
    let page = first_page(m);
    let color = some_color(m);
    let out = m
        .apply_mutation(&Mutation::InsertPath {
            page_id: page,
            anchors: vec![
                corner([x, y]),
                corner([x + w, y + w]),
                corner([x + 2.0 * w, y]),
            ],
            open: true,
            smooth: false,
        })
        .expect("insert open path");
    let id = match out.created_id.expect("created id") {
        ElementId::Polygon(id) => id,
        other => panic!("expected a Polygon, got {other:?}"),
    };
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::Polygon(id.clone()),
        path: PropertyPath::FrameStrokeColor,
        value: Value::ColorRef(Some(color)),
    })
    .expect("stroke");
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: ElementId::Polygon(id.clone()),
        path: PropertyPath::FrameStrokeWeight,
        value: Value::Length(Some(4.0)),
    })
    .expect("stroke weight");
    id
}

/// A small PNG for the inline-image lane.
fn tiny_png() -> Vec<u8> {
    let mut img = image::RgbaImage::new(8, 8);
    for (x, y, p) in img.enumerate_pixels_mut() {
        *p = image::Rgba([(x * 30) as u8, (y * 30) as u8, 200, 255]);
    }
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode png");
    bytes
}

/// A `.ase` library with two entries — the `ImportSwatchLibrary` input.
fn sample_ase() -> Vec<u8> {
    use paged_color::ase::{AseEntry, AseGroup, AseKind, AseLibrary, AseSpace};
    paged_color::ase::write_ase(&AseLibrary {
        groups: vec![AseGroup {
            name: "Sweep Atlas".into(),
            entries: vec![AseEntry {
                name: "Sweep Lab".into(),
                space: AseSpace::Lab,
                value: vec![55.0, 12.0, -30.0],
                kind: AseKind::Global,
            }],
        }],
        loose: vec![AseEntry {
            name: "Sweep CMYK entry".into(),
            space: AseSpace::Cmyk,
            value: vec![10.0, 20.0, 30.0, 0.0],
            kind: AseKind::Process,
        }],
    })
}

/// Insert a ruler guide and return its positional id. Guides carry no
/// `Self` in the parse struct — the apply layer addresses them as
/// `Guide/<spread self id>/<index>` (see `paged-mutate`'s
/// `guide_id_for`), so the id is derived rather than reported.
fn seed_guide(m: &mut CanvasModel) -> String {
    let spread_id = first_spread_id(m);
    m.apply_mutation(&Mutation::InsertGuide {
        spread_id: spread_id.clone(),
        orientation: GuideOrientationSpec::Horizontal,
        position: 100.0,
        page_index: 0,
    })
    .expect("seed a guide");
    let index = m.scene().spreads[0].spread.guides.len() - 1;
    format!("Guide/{spread_id}/{index}")
}

/// Put a live page-number marker into the biggest story, so the section
/// cases have something whose RENDERED value can change.
fn seed_page_number(m: &mut CanvasModel) {
    let (story_id, _) = biggest_story(m);
    m.apply_mutation(&Mutation::InsertField {
        story_id,
        offset: 0,
        field: FieldKind::PageNumber,
    })
    .expect("seed a page-number marker");
}

// ── the sweep ───────────────────────────────────────────────────────

fn run_case(case: &Case) -> Observed {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut m = load_sample(case.fixture);
        let mutation = (case.prepare)(&mut m);

        let cold_before = cold(&m);
        let live_before = live(&m);

        if let Err(e) = m.apply_mutation(&mutation) {
            return Observed::Rejected(format!("{e:?}"));
        }

        let cold_after = cold(&m);
        let live_moved = live(&m) != live_before;
        match (cold_after != cold_before, live_moved) {
            (true, true) => return Observed::Moved,
            (true, false) => return Observed::MovedColdOnly,
            (false, true) => return Observed::MovedLiveOnly,
            (false, false) => {}
        }
        match case.use_it {
            None => Observed::Unmoved,
            Some(use_it) => {
                use_it(&mut m);
                if cold(&m) != cold_after {
                    Observed::UnmovedThenMoved
                } else {
                    Observed::UnmovedThenStillUnmoved
                }
            }
        }
    }));
    match result {
        Ok(o) => o,
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_else(|| "panic".to_string());
            Observed::SetupFailed(msg)
        }
    }
}

/// Whether the observation satisfies the expectation.
fn satisfied(expect: Expect, obs: &Observed) -> bool {
    matches!(
        (expect, obs),
        (Expect::Paints, Observed::Moved | Observed::MovedLiveOnly)
            | (Expect::PaintsNothing(_), Observed::Unmoved)
            | (Expect::PaintsWhenUsed(_), Observed::UnmovedThenMoved)
    )
}

/// A one-line verdict per case.
fn verdict(case: &Case, obs: &Observed) -> String {
    match (case.expect, obs) {
        // These two come FIRST. Written after the expectation arms, an
        // op the engine REJECTED was reported as "expected inert but the
        // page changed" — a false finding that hid a bad payload behind
        // a plausible-sounding verdict.
        (_, Observed::Rejected(e)) => format!("REJECTED · {e}"),
        (_, Observed::SetupFailed(e)) => format!("SETUP FAILED · {e}"),
        (Expect::Paints, Observed::Moved) => "ok · paints".into(),
        (Expect::Paints, Observed::MovedLiveOnly) => {
            "ok · paints on the canvas (the export build deliberately ignores this)".into()
        }
        (Expect::Paints, Observed::MovedColdOnly) => {
            "the renderer reads it, but the canvas was never invalidated".into()
        }
        (Expect::Paints, Observed::Unmoved) => "applied cleanly, rendered nothing".into(),
        (Expect::PaintsNothing(why), Observed::Unmoved) => format!("ok · inert: {why}"),
        (Expect::PaintsWhenUsed(why), Observed::UnmovedThenMoved) => {
            format!("ok · inert until referenced: {why}")
        }
        (Expect::PaintsWhenUsed(_), Observed::UnmovedThenStillUnmoved) => {
            "created, then referencing it STILL rendered nothing".into()
        }
        (Expect::PaintsWhenUsed(why), _) => {
            format!("expected inert until referenced ({why}) but creating it painted")
        }
        (Expect::PaintsNothing(why), _) => {
            format!("expected inert ({why}) but the page changed")
        }
        (_, other) => format!("expectation/observation mismatch: {other:?}"),
    }
}

// ── the findings ratchet ────────────────────────────────────────────

/// Why an op is allowed to be red.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The engine really does apply-and-render-nothing. Diagnosed, not
    /// excused.
    Defect,
    /// The op's effect is real but lies outside what a headless
    /// `CanvasModel` can render — a limit of this harness, not of the
    /// engine.
    HarnessLimit,
}

/// A red the sweep has already diagnosed.
///
/// This is a RATCHET, not a suppression list. It exists so the sweep can
/// run green in CI while every known defect stays written down and
/// un-loseable, and it is enforced in both directions: an undiagnosed
/// red fails the test, and so does a *diagnosed* one that starts passing
/// (that means someone fixed it, and the entry must go). Nothing may be
/// added here without a diagnosis that names what the op writes, what
/// the renderer reads, and why they miss each other.
struct Known {
    op: &'static str,
    kind: Kind,
    diagnosis: &'static str,
}

const KNOWN: &[Known] = &[
    Known {
        op: "InsertSection",
        kind: Kind::Defect,
        diagnosis: "\
`SectionWalk::next_label` computes the section-derived number correctly — \
reseeding from `start_at`, applying the numbering style and the prefix — and \
then throws it away: `if let Some(name) = page_name { return name.to_string() }` \
runs FIRST, so any page carrying a `Name` attribute keeps its baked label. \
InDesign writes `Page@Name` on every page, so on real documents a section edit \
is invisible by construction. Proven by stripping `page.name` in the scene and \
re-running: the same `editSection` then moves the page.",
    },
    Known {
        op: "EditSection",
        kind: Kind::Defect,
        diagnosis: "\
Same cause as `InsertSection`: the walk recomputes the label from the edited \
`start_at` / prefix / numbering style and `next_label` returns the page's \
baked `Name` instead. Editing a section that exists is as invisible as \
creating one, and for the same single line of code.",
    },
    Known {
        op: "DeleteSection",
        kind: Kind::Defect,
        diagnosis: "\
Same cause as `InsertSection`. Deleting a section returns the walk to implicit \
1-based numbering, which the page never displayed in the first place — its \
baked `Name` was winning throughout — so the removal is invisible for exactly \
the reason the creation was.",
    },
    Known {
        op: "SetUseStandardLabForSpots",
        kind: Kind::Defect,
        diagnosis: "\
The flag never leaves `paged-canvas`. `Mutation::SetUseStandardLabForSpots` \
sets `CanvasModel::use_standard_lab_for_spots`, which is read by \
`color_preview` / `working_color_of_with` / `DocumentMeta` and by nothing \
else — it is not on `PipelineOptions`, so `paged-renderer` and `paged-compose` \
never see it. The Swatches panel chip changes and the page does not, which is \
also exactly what the existing `standard_lab_for_spots_prefers_the_lab_primary` \
test asserts (it compares `color_preview`, not a render). The `swatches` fixture \
paints with a spot whose PRIMARY is Lab and whose alternate is CMYK, so there is \
a real difference to show.",
    },
    Known {
        op: "InsertAnchoredFrame",
        kind: Kind::Defect,
        diagnosis: "\
The minted-invisible-object class, the same shape as `InsertTable` — NOT the \
invalidation defect this entry claimed until the two builds were compared \
command by command. The op emits ZERO draw commands, in the cold build and \
the live one alike, and page 0's command stream is byte-identical across it: \
`apply_insert_anchored_frame` mints an `AnchoredFrame` with \
`fill_color: None`, `stroke_color: None` and — for the `image_uri: None` \
shape — no image, and `emit_anchored_rect_via_pipeline` hands that to the \
same `emit_rectangle_into` a spread Rectangle uses, where `fill_paint_module` \
returns early on a transparent fill and the stroke module on a zero weight. \
Nor does the text move around it: the engine places anchored frames at the \
paragraph origin and gives them no inline advance at all (the standing \
`TODO(anchored-position)` in `anchored.rs` — the composer surfaces no \
anchor-character position, and an IDML anchored object IS a character \
position), so the InDesign behaviour that makes an empty anchored box \
visible — it displaces the text it sits in — is a larger engine gap this \
door cannot close on its own. What made the row read `MovedColdOnly` was an \
oracle artefact, now fixed: `emit_rectangle_into` interned the UNIT_RECT path \
unconditionally for its effects stamp, so an invisible rect grew the page's \
path pool, which `DisplayList::digest` folds — while the LIVE build happened \
to hold that pool entry already (the A5 substituted-font highlight is a \
rect), so only the cold print moved. With the intern made conditional the row \
reads honestly: applied cleanly, rendered nothing. Two ways out, both a \
decision rather than a bug fix: give the composer real inline \
anchored-object metrics, or reclassify the row `PaintsWhenUsed` — which \
needs a third fix first, because `find_rectangle_mut` scans only the spreads, \
so the `createdId` this op hands back cannot be given a fill by any \
`setElementProperty` on the wire.",
    },
    Known {
        op: "PlaceImage",
        kind: Kind::HarnessLimit,
        diagnosis: "\
Not an engine finding. `CanvasOptions` has no image lane — `build_font_resolver` \
fills only `BytesResolver::fonts`, so NO `LinkResourceURI` resolves through a \
headless `CanvasModel`, and the `images` fixture's own frames report \
`ImageLinkMissing` too. The engine documents that an unreachable uri leaves the \
frame rendering as before (honest miss, no badge), and the missing-image \
placeholder is additionally gated on `has_image_element`, which `placeImage` \
does not set. Exercising this properly needs the C-6 resource-tile provider \
handshake (`submit_resource_tiles`), which is out of scope for a vocabulary \
sweep. The inline-bytes lane IS covered here, by `ReplaceImageBytes`.",
    },
];

fn known(op: &str) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.op == op)
}

#[test]
fn every_mutation_renders_what_it_claims_to() {
    let cases = cases();
    let mut rows = Vec::new();
    let mut new_findings = Vec::new();
    let mut fixed = Vec::new();

    for case in &cases {
        let obs = run_case(case);
        let ok = satisfied(case.expect, &obs);
        let v = verdict(case, &obs);
        let tag = match (ok, known(case.op)) {
            (true, None) => "ok",
            (true, Some(k)) => {
                fixed.push(k.op);
                "FIXED — remove its KNOWN entry"
            }
            (false, Some(k)) if k.kind == Kind::Defect => "known defect",
            (false, Some(_)) => "harness limit",
            (false, None) => {
                new_findings.push(format!("{}: {v}", case.op));
                "NEW FINDING"
            }
        };
        rows.push(format!(
            "{:<26} {:<14} {:<10} {:<28} {}",
            case.op,
            case.fixture,
            match case.expect {
                Expect::Paints => "paints",
                Expect::PaintsNothing(_) => "inert",
                Expect::PaintsWhenUsed(_) => "when-used",
            },
            tag,
            v
        ));
    }

    let table = rows.join("\n");
    println!(
        "\n── render-effect sweep · {} ops, {} known findings ──\n{table}\n",
        cases.len(),
        KNOWN.len()
    );
    assert!(
        fixed.is_empty(),
        "\nthese ops now render what they claim — delete their KNOWN entries \
         so the ratchet cannot slip back: {fixed:?}\n\n{table}\n"
    );
    assert!(
        new_findings.is_empty(),
        "\n{} op(s) apply cleanly and render nothing, with no diagnosis on \
         file:\n  {}\n\n{table}\n",
        new_findings.len(),
        new_findings.join("\n  ")
    );
}

/// Every diagnosis must name a real mutation, and must actually say
/// something. A `KNOWN` entry whose op was renamed away would otherwise
/// sit there forever describing a defect nobody can find.
#[test]
fn every_known_finding_names_a_swept_op_and_carries_a_diagnosis() {
    let swept: BTreeSet<&str> = cases().iter().map(|c| c.op).collect();
    for k in KNOWN {
        assert!(
            swept.contains(k.op),
            "KNOWN names {:?}, which the sweep does not cover",
            k.op
        );
        assert!(
            k.diagnosis.len() > 120,
            "{}'s diagnosis is too thin to be a diagnosis",
            k.op
        );
    }
}

/// The sweep must not silently lose an op.
///
/// This repo has been bitten by hand-maintained parallel lists before —
/// `regen-fixtures.sh` kept its own copy of the sample names and quietly
/// dropped two, and nothing noticed because the guard only ever fired on
/// names that did not exist, never on names left OUT. So the guard here
/// walks the vocabulary itself: `Mutation::discriminant`'s match arms are
/// the one list, read from source, and every arm must appear in the
/// sweep.
#[test]
fn the_sweep_covers_the_whole_mutation_vocabulary() {
    let src =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/channel.rs"))
            .expect("read channel.rs");
    let start = src
        .find("pub fn discriminant(&self)")
        .expect("Mutation::discriminant still exists");
    let body = &src[start..];
    let end = body.find("\n    }\n").expect("discriminant body ends");
    let mut declared = BTreeSet::new();
    for line in body[..end].lines() {
        if let Some(rest) = line.trim().strip_prefix("Self::") {
            if let Some(name) = rest.split([' ', '{', '(']).next() {
                declared.insert(name.to_string());
            }
        }
    }
    assert!(
        declared.len() > 100,
        "parsed only {} discriminants — the scraper lost its footing",
        declared.len()
    );

    let swept: BTreeSet<String> = cases().iter().map(|c| c.op.to_string()).collect();
    let missing: Vec<&String> = declared.difference(&swept).collect();
    let unknown: Vec<&String> = swept.difference(&declared).collect();
    assert!(
        unknown.is_empty(),
        "sweep names mutations that do not exist: {unknown:?}"
    );
    assert!(
        missing.is_empty(),
        "{} of {} mutations are unswept: {missing:?}",
        missing.len(),
        declared.len()
    );
}

/// The harness has to be able to see the bug it was built for — and,
/// now that the bug is fixed, to hold the fix down at the exact seam
/// that carried it.
///
/// Three measurements on ONE document, in order:
///
/// 1. `linkFrames` between an overset frame and a wire-created target
///    MOVES the page: the story pours into the second frame. That is the
///    `LinkFrames` row of the sweep, green since `paged_mutate::apply`
///    began refreshing `Document`'s derived indices.
/// 2. The cause, proven by inverting it: clear `text_frame_index` and
///    nothing else — not one model field — and the threaded render goes
///    away. `frame_chain` collects a story's frames by scanning the
///    spreads (always fresh) but follows `next_text_frame` through
///    `Document::text_frame`, an O(1) lookup in that index, so an index
///    missing the wire-created frame ends the walk one frame short.
///    Rebuilding restores the threaded render exactly.
/// 3. With that fresh index, reverting the half `59c98b5` added (target
///    back on its own story, forward pointer kept) renders IDENTICALLY
///    to the linked shape — `frame_chain` never checks a continuation
///    frame's `parent_story`, so that write, right as it is for every
///    other reader, is not the half that was blocking the pixels.
///
/// If step 2 ever stops changing the page, the oracle has gone blind
/// and every green row in the sweep is worthless.
#[test]
fn a_thread_threads_because_the_index_is_fresh_not_because_of_the_story_write() {
    let mut m = load_sample("text-overset");
    let from = lone_overset_frame(&m);
    let to = add_text_frame(&mut m, (500.0, 60.0, 800.0, 500.0));
    let own_story = story_of(&m, &to);

    let unlinked = cold(&m);
    m.apply_mutation(&Mutation::LinkFrames {
        from: from.clone(),
        to: to.clone(),
    })
    .expect("link");

    // 1 — the fix: the thread renders.
    let threaded = cold(&m);
    assert_ne!(
        threaded, unlinked,
        "linkFrames into a wire-created frame must pour the overset text \
         into the target — if this is byte-identical again, the derived \
         indices have gone stale after a mutation and the whole class is \
         back"
    );

    // 2 — the cause, isolated by inverting it: stale JUST
    // `text_frame_index` and the threaded render goes away again, with
    // every model field still saying "threaded". Not asserted equal to
    // `unlinked` — `frame_for_story` is a separate cache and stays
    // fresh here, so the page is not bit-for-bit the pre-link one; the
    // claim under test is narrower and exact: the pour depends on the
    // frame index, so the index is where the fix belongs.
    m.scene_mut().text_frame_index.clear();
    assert_ne!(
        cold(&m),
        threaded,
        "a chain walk that cannot resolve the target stops one frame \
         short and the overset text stays overset — which is what the op \
         did on every document before the fix"
    );
    m.scene_mut().rebuild_indexes();
    assert_eq!(cold(&m), threaded, "and rebuilding brings the thread back");

    // 3 — and the sharp edge: with a FRESH index, the pre-`59c98b5`
    // shape (drop the target back onto its own story, keep the forward
    // pointer) renders identically. `frame_chain` never checks a
    // continuation frame's `parent_story`, so the story half of
    // `59c98b5` — correct as it is for `frame_for_story`, overset
    // accounting and IDML export — is not what was standing between the
    // op and the pixels.
    let idx = m
        .scene()
        .spreads
        .iter()
        .enumerate()
        .flat_map(|(si, sp)| {
            sp.spread
                .text_frames
                .iter()
                .enumerate()
                .map(move |(fi, f)| (si, fi, f.self_id.clone()))
        })
        .find(|(_, _, id)| id.as_deref() == Some(to.as_str()))
        .map(|(si, fi, _)| (si, fi))
        .expect("target frame");
    m.scene_mut().spreads[idx.0].spread.text_frames[idx.1].parent_story = Some(own_story);
    m.scene_mut().rebuild_indexes();
    assert_eq!(
        cold(&m),
        threaded,
        "a fresh index threads on the forward pointer alone — if this now \
         differs, `frame_chain` has learned to check the continuation \
         frame's story"
    );
}

// ── cases ───────────────────────────────────────────────────────────

fn cases() -> Vec<Case> {
    let mut c = Vec::new();
    text_cases(&mut c);
    frame_cases(&mut c);
    page_cases(&mut c);
    path_cases(&mut c);
    pathfinder_cases(&mut c);
    structure_cases(&mut c);
    layer_cases(&mut c);
    palette_cases(&mut c);
    style_cases(&mut c);
    table_cases(&mut c);
    document_cases(&mut c);
    c
}

// ── text ────────────────────────────────────────────────────────────

fn text_cases(c: &mut Vec<Case>) {
    c.push(paints("InsertText", "text", |m| {
        let (story_id, _) = biggest_story(m);
        Mutation::InsertText {
            story_id,
            offset: 0,
            text: "SWEEP ".into(),
            cell: None,
        }
    }));
    c.push(paints("DeleteRange", "text", |m| {
        let (story_id, chars) = biggest_story(m);
        assert!(chars > 10, "story too short to delete a visible range from");
        Mutation::DeleteRange {
            story_id,
            start: 0,
            end: 10,
            cell: None,
        }
    }));
    c.push(paints("ApplyStyle", "styles-cascade", |m| {
        let (story_id, chars) = biggest_story(m);
        // A style with real overrides — a freshly-minted empty one would
        // inherit everything and legitimately paint nothing.
        let style = m
            .scene()
            .styles
            .paragraph_styles
            .iter()
            .find(|(id, def)| !id.contains("NoParagraphStyle") && def.point_size.is_some())
            .map(|(id, _)| id.clone())
            .expect("fixture carries a paragraph style with a point size");
        Mutation::ApplyStyle {
            story_id,
            start: 0,
            end: chars.min(40),
            style,
            scope: StyleScope::Paragraph,
            cell: None,
        }
    }));
    c.push(paints("InsertField", "text", |m| {
        let (story_id, _) = biggest_story(m);
        Mutation::InsertField {
            story_id,
            offset: 0,
            field: FieldKind::PageNumber,
        }
    }));
    c.push(paints("InsertAnchoredFrame", "text", |m| {
        let (story_id, _) = biggest_story(m);
        Mutation::InsertAnchoredFrame {
            story_id,
            offset: 0,
            width: 60.0,
            height: 60.0,
            image_uri: None,
        }
    }));
    // Written expecting INERT — a link is a REGION, not a mark: it
    // restyles no glyph, and the display list carries no link command
    // (the clickable area rides `collect_link_regions` onto `BuiltPage`,
    // beside the display list rather than inside it). The digest moves
    // anyway, and for a reason worth recording: tagging `[start, end)`
    // SPLITS the run at the link boundary, so what was one shaped run
    // becomes two and the glyph stream changes even though the same
    // characters are drawn. That is a real display-list change, so the
    // expectation is `Paints` — but note it is a shaping artefact of the
    // split, not a mark the op added.
    c.push(paints("InsertHyperlink", "text", |m| {
        let (story_id, chars) = biggest_story(m);
        Mutation::InsertHyperlink {
            story_id,
            start: 0,
            end: chars.min(12),
            url: "https://paged.media".into(),
        }
    }));
    c.push(paints("SetFieldValue", "text", |m| {
        let (story_id, _) = biggest_story(m);
        m.apply_mutation(&Mutation::InsertField {
            story_id: story_id.clone(),
            offset: 0,
            field: FieldKind::Placeholder {
                plugin: "paged.data".into(),
                key: "title".into(),
                value: Some("BEFORE".into()),
            },
        })
        .expect("seed a placeholder");
        Mutation::SetFieldValue {
            story_id,
            offset: 0,
            value: Some("AFTER — a visibly different string".into()),
        }
    }));
    c.push(paints("LinkFrames", "text-overset", |m| {
        // The op this whole file was written for. It rendered NOTHING
        // for its entire life, in two layers: `59c98b5` fixed the model
        // half (the target stayed on the story `insertTextFrame` minted,
        // so the composer walked a chain of one), and the pixels still
        // did not move because `Document::frame_chain` resolves
        // `next_text_frame` through `text_frame_index` — a derived cache
        // nothing refreshed after a mutation. `paged_mutate::apply` now
        // rebuilds it, and this row is the pixel-level proof.
        let from = lone_overset_frame(m);
        let to = add_text_frame(m, (500.0, 60.0, 800.0, 500.0));
        Mutation::LinkFrames { from, to }
    }));
    c.push(paints("UnlinkFrames", "text-overset", |m| {
        Mutation::UnlinkFrames {
            frame: chain_head(m),
        }
    }));
    c.push(when_used(
        "InsertTextFrame",
        "text",
        "the model mints it with no fill and no stroke on purpose — \
         `insertTextFrame` is the empty threading / Type-tool target, and \
         an empty frame has nothing to draw until text arrives",
        |m| Mutation::InsertTextFrame {
            page_id: first_page(m),
            bounds: (400.0, 60.0, 700.0, 500.0),
        },
        |m| {
            let frame = m
                .scene()
                .spreads
                .iter()
                .flat_map(|s| s.spread.text_frames.iter())
                .last()
                .and_then(|f| f.self_id.clone())
                .expect("the inserted frame");
            let story = story_of(m, &frame);
            m.apply_mutation(&Mutation::InsertText {
                story_id: story,
                offset: 0,
                text: "Poured into the new frame.".into(),
                cell: None,
            })
            .expect("pour text");
        },
    ));
}

// ── frames, images, transforms ──────────────────────────────────────

fn frame_cases(c: &mut Vec<Case>) {
    c.push(paints("MoveFrame", "geometry", |m| Mutation::MoveFrame {
        frame_id: rect_ids(m)[0].clone(),
        transform: [1.0, 0.0, 0.0, 1.0, 37.0, 23.0],
    }));
    c.push(paints("ResizeFrame", "geometry", |m| {
        let id = rect_ids(m)[0].clone();
        let r = m
            .scene()
            .spreads
            .iter()
            .flat_map(|s| s.spread.rectangles.iter())
            .find(|r| r.self_id.as_deref() == Some(id.as_str()))
            .expect("rect");
        let b = (
            r.bounds.top,
            r.bounds.left,
            r.bounds.bottom + 40.0,
            r.bounds.right + 40.0,
        );
        Mutation::ResizeFrame {
            frame_id: id,
            bounds: b,
        }
    }));
    c.push(paints("InsertFrame", "geometry", |m| {
        // A bare insert inherits the document defaults, and an unfilled,
        // unstroked rectangle would legitimately paint nothing — so the
        // defaults are set in SETUP and the op under test is the insert.
        let color = some_color(m);
        m.apply_mutation(&Mutation::SetDocumentDefaults {
            fill_color: Some(color),
            stroke_color: None,
            stroke_weight: None,
        })
        .expect("set defaults");
        Mutation::InsertFrame {
            page_id: first_page(m),
            bounds: (40.0, 40.0, 240.0, 240.0),
        }
    }));
    c.push(paints("InsertOval", "geometry", |m| {
        let color = some_color(m);
        m.apply_mutation(&Mutation::SetDocumentDefaults {
            fill_color: Some(color),
            stroke_color: None,
            stroke_weight: None,
        })
        .expect("set defaults");
        Mutation::InsertOval {
            page_id: first_page(m),
            bounds: (300.0, 40.0, 460.0, 240.0),
        }
    }));
    c.push(paints("InsertLine", "geometry", |m| {
        let color = some_color(m);
        m.apply_mutation(&Mutation::SetDocumentDefaults {
            fill_color: None,
            stroke_color: Some(color),
            stroke_weight: Some(6.0),
        })
        .expect("set defaults");
        Mutation::InsertLine {
            page_id: first_page(m),
            start: (40.0, 500.0),
            end: (400.0, 620.0),
        }
    }));
    c.push(paints("InsertPath", "geometry", |m| {
        let color = some_color(m);
        m.apply_mutation(&Mutation::SetDocumentDefaults {
            fill_color: Some(color),
            stroke_color: None,
            stroke_weight: None,
        })
        .expect("set defaults");
        Mutation::InsertPath {
            page_id: first_page(m),
            anchors: vec![
                corner([60.0, 60.0]),
                corner([240.0, 90.0]),
                corner([200.0, 260.0]),
                corner([70.0, 220.0]),
            ],
            open: false,
            smooth: false,
        }
    }));
    c.push(paints("DeleteFrame", "geometry", |m| {
        Mutation::DeleteFrame {
            frame_id: rect_ids(m)[0].clone(),
        }
    }));
    c.push(paints("SetElementProperty", "geometry", |m| {
        let color = fresh_color(m);
        Mutation::SetElementProperty {
            element_id: ElementId::Rectangle(rect_ids(m)[0].clone()),
            path: PropertyPath::FrameFillColor,
            value: Value::ColorRef(Some(color)),
        }
    }));
    c.push(paints("PlaceImage", "images", |m| {
        // Reuse a URI the fixture already packages, so the asset resolver
        // can actually serve it — an unreachable uri is documented to
        // leave the frame rendering as before, which would make this case
        // measure the resolver rather than the op.
        let uri = m
            .scene()
            .spreads
            .iter()
            .flat_map(|s| s.spread.rectangles.iter())
            .find_map(|r| r.image_link.clone())
            .expect("images fixture links an image");
        let host = add_rect(m, (40.0, 40.0, 240.0, 240.0));
        Mutation::PlaceImage {
            element_id: host,
            uri,
            fit: Some("FitContentToFrame".into()),
        }
    }));
    c.push(paints("ReplaceImageBytes", "geometry", |m| {
        let host = add_rect(m, (40.0, 40.0, 240.0, 240.0));
        Mutation::ReplaceImageBytes {
            element_id: host,
            bytes: Some(ByteBuf(tiny_png())),
        }
    }));
    c.push(paints("PasteInto", "geometry", |m| {
        // The child sticks out of the container, so the clip is visible.
        let container = add_rect(m, (100.0, 100.0, 300.0, 300.0));
        let child = add_rect(m, (150.0, 150.0, 500.0, 500.0));
        Mutation::PasteInto {
            container_id: ElementId::Rectangle(container),
            child_id: ElementId::Rectangle(child),
        }
    }));
    c.push(paints("ReleaseFrom", "geometry", |m| {
        let container = add_rect(m, (100.0, 100.0, 300.0, 300.0));
        let child = add_rect(m, (150.0, 150.0, 500.0, 500.0));
        m.apply_mutation(&Mutation::PasteInto {
            container_id: ElementId::Rectangle(container),
            child_id: ElementId::Rectangle(child.clone()),
        })
        .expect("paste into");
        Mutation::ReleaseFrom {
            child_id: ElementId::Rectangle(child),
        }
    }));
    c.push(paints("ReorderElement", "geometry", |m| {
        // Two overlapping filled rects: restacking inverts what occludes
        // what.
        let back = add_rect(m, (100.0, 100.0, 300.0, 300.0));
        let front = add_rect(m, (150.0, 150.0, 350.0, 350.0));
        // The two must differ in colour or restacking them is invisible.
        let color = fresh_color(m);
        fill(m, &ElementId::Rectangle(front.clone()), &color);
        let _ = back;
        Mutation::ReorderElement {
            element_id: ElementId::Rectangle(front),
            to: ZOrderTarget::Back,
        }
    }));
    c.push(paints("ApplyOpacityMask", "geometry", |m| {
        let target = add_rect(m, (100.0, 100.0, 340.0, 340.0));
        let mask = add_rect(m, (140.0, 140.0, 300.0, 300.0));
        Mutation::ApplyOpacityMask {
            target_id: ElementId::Rectangle(target),
            mask_id: ElementId::Rectangle(mask),
            mask_type: Some("luminosity".into()),
            invert: None,
        }
    }));
    c.push(paints("ReleaseOpacityMask", "geometry", |m| {
        let target = add_rect(m, (100.0, 100.0, 340.0, 340.0));
        let mask = add_rect(m, (140.0, 140.0, 300.0, 300.0));
        m.apply_mutation(&Mutation::ApplyOpacityMask {
            target_id: ElementId::Rectangle(target.clone()),
            mask_id: ElementId::Rectangle(mask),
            mask_type: Some("luminosity".into()),
            invert: None,
        })
        .expect("apply mask");
        Mutation::ReleaseOpacityMask {
            target_id: ElementId::Rectangle(target),
        }
    }));
    c.push(paints("AttachTextToPath", "text-on-path", |m| {
        // Detach first, so attaching is measured against a document in
        // which the text is NOT on the path.
        let (host, story) = text_path_host(m);
        m.apply_mutation(&Mutation::DetachTextFromPath {
            element_id: ElementId::Polygon(host.clone()),
        })
        .expect("detach first");
        Mutation::AttachTextToPath {
            element_id: ElementId::Polygon(host),
            story_id: story,
            path_type_alignment: None,
            flip_path_effect: None,
            start_bracket: None,
            end_bracket: None,
        }
    }));
    c.push(paints("DetachTextFromPath", "text-on-path", |m| {
        let (host, _) = text_path_host(m);
        Mutation::DetachTextFromPath {
            element_id: ElementId::Polygon(host),
        }
    }));
    c.push(when_used(
        "CreateGroup",
        "geometry",
        "grouping is z-order-neutral by construction — the group takes its \
         topmost member's paint slot and the members keep their order, so \
         the page is byte-identical; a group becomes visible only when \
         something is applied to it AS A UNIT",
        |m| {
            let a = add_rect(m, (100.0, 100.0, 300.0, 300.0));
            let b = add_rect(m, (150.0, 150.0, 350.0, 350.0));
            Mutation::CreateGroup {
                member_ids: vec![ElementId::Rectangle(a), ElementId::Rectangle(b)],
            }
        },
        |m| {
            let g = m
                .scene()
                .spreads
                .iter()
                .flat_map(|s| s.spread.groups.iter())
                .last()
                .and_then(|g| g.self_id.clone())
                .expect("the created group");
            m.apply_mutation(&Mutation::SetGroupTransform {
                group_id: g,
                transform: Some([1.0, 0.0, 0.0, 1.0, 90.0, 70.0]),
            })
            .expect("move the group");
        },
    ));
    c.push(inert(
        "DissolveGroup",
        "geometry",
        "the exact inverse of a z-order-neutral grouping: members return to \
         the group's paint slot in stored order, and a group carrying no \
         transform / opacity / shadow of its own contributed nothing to \
         lose",
        |m| {
            let a = add_rect(m, (100.0, 100.0, 300.0, 300.0));
            let b = add_rect(m, (150.0, 150.0, 350.0, 350.0));
            let out = m
                .apply_mutation(&Mutation::CreateGroup {
                    member_ids: vec![ElementId::Rectangle(a), ElementId::Rectangle(b)],
                })
                .expect("group");
            let id = match out.created_id.expect("created id") {
                ElementId::Group(id) => id,
                other => panic!("expected a Group, got {other:?}"),
            };
            Mutation::DissolveGroup { group_id: id }
        },
    ));
    c.push(paints("SetGroupTransform", "geometry", |m| {
        let a = add_rect(m, (100.0, 100.0, 300.0, 300.0));
        let b = add_rect(m, (150.0, 150.0, 350.0, 350.0));
        let out = m
            .apply_mutation(&Mutation::CreateGroup {
                member_ids: vec![ElementId::Rectangle(a), ElementId::Rectangle(b)],
            })
            .expect("group");
        let id = match out.created_id.expect("created id") {
            ElementId::Group(id) => id,
            other => panic!("expected a Group, got {other:?}"),
        };
        Mutation::SetGroupTransform {
            group_id: id,
            transform: Some([1.0, 0.0, 0.0, 1.0, 90.0, 70.0]),
        }
    }));
    c.push(inert(
        "SetPluginMetadata",
        "geometry",
        "one Label KeyValuePair on a page item — a carrier for plugin state \
         with no paint of its own; the renderer never reads Label",
        // The engine gates the key namespace: `x-paged:<plugin>` is the
        // only shape it accepts.
        // The engine gates both halves: the key must live in the reserved
        // `x-paged:<plugin>` namespace, and the value must be the JSON
        // envelope `{ v: <int >= 1>, data: {…} }`. A rejected mutation
        // measures nothing, so both have to be right for this case to say
        // anything about rendering.
        |m| Mutation::SetPluginMetadata {
            element_id: ElementId::Rectangle(rect_ids(m)[0].clone()),
            key: "x-paged:sweep".into(),
            value: Some(r#"{"v":1,"data":{"swept":true}}"#.into()),
            caller: None,
        },
    ));
    c.push(paints("Batch", "geometry", |m| {
        let id = rect_ids(m)[0].clone();
        let color = some_color(m);
        Mutation::Batch {
            ops: vec![
                Mutation::MoveFrame {
                    frame_id: id.clone(),
                    transform: [1.0, 0.0, 0.0, 1.0, 25.0, 25.0],
                },
                Mutation::SetElementProperty {
                    element_id: ElementId::Rectangle(id),
                    path: PropertyPath::FrameFillColor,
                    value: Value::ColorRef(Some(color)),
                },
            ],
        }
    }));
    c.push(paints("BindCreated", "geometry", |m| {
        // `bindCreated` contributes no operation of its own; what it must
        // do is let a LATER child of the same batch address what an
        // earlier one minted. If the handle failed to resolve, the fill
        // would land nowhere and the page would not move.
        let color = some_color(m);
        Mutation::Batch {
            ops: vec![
                Mutation::InsertFrame {
                    page_id: first_page(m),
                    bounds: (60.0, 60.0, 260.0, 260.0),
                },
                Mutation::BindCreated {
                    handle: "box".into(),
                },
                Mutation::SetElementProperty {
                    element_id: ElementId::Rectangle("$h:box".into()),
                    path: PropertyPath::FrameFillColor,
                    value: Value::ColorRef(Some(color)),
                },
            ],
        }
    }));
}

// ── pages ───────────────────────────────────────────────────────────

fn page_cases(c: &mut Vec<Case>) {
    c.push(paints("InsertPage", "layout", |m| {
        let _ = m;
        Mutation::InsertPage {
            after_page_id: None,
            master_id: None,
        }
    }));
    c.push(paints("DeletePage", "layout", |m| Mutation::DeletePage {
        page_id: first_page(m),
    }));
    c.push(paints("ResizePage", "layout", |m| Mutation::ResizePage {
        page_id: first_page(m),
        bounds: (0.0, 0.0, 500.0, 400.0),
    }));
    c.push(paints("DuplicatePage", "layout", |m| {
        Mutation::DuplicatePage {
            page: first_page(m),
        }
    }));
    c.push(paints("ApplyMasterToPage", "masters", |m| {
        // Detaching is the unambiguous direction: whatever the master was
        // contributing must stop appearing.
        Mutation::ApplyMasterToPage {
            page: first_page(m),
            master: None,
        }
    }));
    c.push(paints("InsertSection", "text", |m| {
        seed_page_number(m);
        Mutation::InsertSection {
            at_page: first_page(m),
            prefix: Some("X-".into()),
            numbering_style: None,
            start_at: Some(42),
        }
    }));
    c.push(paints("EditSection", "text", |m| {
        seed_page_number(m);
        let out = m
            .apply_mutation(&Mutation::InsertSection {
                at_page: first_page(m),
                prefix: None,
                numbering_style: None,
                start_at: Some(5),
            })
            .expect("seed a section");
        let _ = out;
        let id = m
            .scene()
            .designmap
            .sections
            .last()
            .map(|s| s.self_id.clone())
            .expect("the seeded section");
        Mutation::EditSection {
            section_id: id,
            prefix: None,
            numbering_style: None,
            start_at: Some(Some(99)),
        }
    }));
    c.push(paints("DeleteSection", "text", |m| {
        seed_page_number(m);
        m.apply_mutation(&Mutation::InsertSection {
            at_page: first_page(m),
            prefix: None,
            numbering_style: None,
            start_at: Some(77),
        })
        .expect("seed a section");
        let id = m
            .scene()
            .designmap
            .sections
            .last()
            .map(|s| s.self_id.clone())
            .expect("the seeded section");
        Mutation::DeleteSection { section_id: id }
    }));
    c.push(inert(
        "InsertGuide",
        "layout",
        "a ruler guide is editor chrome: the renderer emits no command for \
         `Spread::guides` at all — the host paints them in its own overlay, \
         above the display list",
        |m| Mutation::InsertGuide {
            spread_id: first_spread_id(m),
            orientation: GuideOrientationSpec::Vertical,
            position: 120.0,
            page_index: 0,
        },
    ));
    c.push(inert(
        "MoveGuide",
        "layout",
        "same as InsertGuide — guides never enter the display list, so \
         moving one cannot change it",
        |m| Mutation::MoveGuide {
            guide_id: seed_guide(m),
            position: 300.0,
        },
    ));
    c.push(inert(
        "DeleteGuide",
        "layout",
        "same as InsertGuide — a guide contributes no command, so removing \
         one removes nothing",
        |m| Mutation::DeleteGuide {
            guide_id: seed_guide(m),
        },
    ));
}

// ── path editing ────────────────────────────────────────────────────

fn path_cases(c: &mut Vec<Case>) {
    c.push(paints("PathPointInsert", "geometry", |m| {
        let id = add_quad(m, 80.0, 80.0, 200.0, 200.0);
        // Off the straight edge, so the outline genuinely changes.
        Mutation::PathPointInsert {
            element_id: ElementId::Polygon(id),
            index: 1,
            anchor: corner([330.0, 130.0]),
            prev_subpath_starts: None,
        }
    }));
    c.push(paints("PathPointRemove", "geometry", |m| {
        let id = add_quad(m, 80.0, 80.0, 200.0, 200.0);
        Mutation::PathPointRemove {
            element_id: ElementId::Polygon(id),
            index: 2,
        }
    }));
    c.push(paints("PathPointSet", "geometry", |m| {
        let id = add_quad(m, 80.0, 80.0, 200.0, 200.0);
        Mutation::PathPointSet {
            element_id: ElementId::Polygon(id),
            index: 0,
            role: PathPointRole::Anchor,
            position: [20.0, 20.0],
        }
    }));
    c.push(paints("PathPointCurveType", "geometry", |m| {
        let id = add_quad(m, 80.0, 80.0, 200.0, 200.0);
        Mutation::PathPointCurveType {
            element_id: ElementId::Polygon(id),
            index: 1,
            smooth: true,
        }
    }));
    c.push(paints("PathOpenAt", "geometry", |m| {
        let id = add_quad(m, 80.0, 80.0, 200.0, 200.0);
        Mutation::PathOpenAt {
            element_id: ElementId::Polygon(id),
            index: 1,
        }
    }));
    c.push(paints("ClosePath", "geometry", |m| {
        let id = add_open_path(m, 80.0, 80.0, 120.0);
        Mutation::ClosePath {
            element_id: ElementId::Polygon(id),
            subpath: None,
        }
    }));
    c.push(paints("JoinPaths", "geometry", |m| {
        let a = add_open_path(m, 80.0, 80.0, 120.0);
        let b = add_open_path(m, 400.0, 80.0, 120.0);
        Mutation::JoinPaths {
            element_id: ElementId::Polygon(a),
            other_id: ElementId::Polygon(b),
        }
    }));
    c.push(paints("OutlineStroke", "geometry", |m| {
        let id = add_open_path(m, 80.0, 80.0, 120.0);
        Mutation::OutlineStroke {
            element_id: ElementId::Polygon(id),
            width: 12.0,
            cap: "butt".into(),
            join: "miter".into(),
            miter_limit: 4.0,
        }
    }));
    c.push(paints("OffsetPath", "geometry", |m| {
        let id = add_quad(m, 80.0, 80.0, 200.0, 200.0);
        Mutation::OffsetPath {
            element_id: ElementId::Polygon(id),
            delta: 24.0,
            join: "miter".into(),
            miter_limit: 4.0,
        }
    }));
    c.push(paints("SimplifyPath", "geometry", |m| {
        // A deliberately over-sampled octagon: a four-anchor quad is
        // already minimal, so simplifying it would legitimately change
        // nothing and the case would prove the opposite of what it means.
        let page = first_page(m);
        let color = some_color(m);
        let anchors: Vec<PathAnchorSpec> = (0..16)
            .map(|i| {
                let t = i as f32 / 16.0 * std::f32::consts::TAU;
                corner([200.0 + 90.0 * t.cos(), 300.0 + 90.0 * t.sin()])
            })
            .collect();
        let out = m
            .apply_mutation(&Mutation::InsertPath {
                page_id: page,
                anchors,
                open: false,
                smooth: false,
            })
            .expect("insert dense path");
        let id = out.created_id.expect("created id").raw_id().to_string();
        fill(m, &ElementId::Polygon(id.clone()), &color);
        Mutation::SimplifyPath {
            element_id: ElementId::Polygon(id),
            tolerance: 40.0,
        }
    }));
}

// ── pathfinder ──────────────────────────────────────────────────────

/// Two overlapping filled quads — the input every Pathfinder verb needs.
/// Returned top-to-bottom, the order `element_ids` documents.
fn two_overlapping(m: &mut CanvasModel) -> (String, String) {
    let back = add_quad(m, 80.0, 80.0, 200.0, 200.0);
    let front = add_quad(m, 180.0, 180.0, 200.0, 200.0);
    (front, back)
}

/// The same pair as `ElementId`s, top-to-bottom.
fn overlapping_ids(m: &mut CanvasModel) -> Vec<ElementId> {
    let (front, back) = two_overlapping(m);
    vec![ElementId::Polygon(front), ElementId::Polygon(back)]
}

fn pathfinder_cases(c: &mut Vec<Case>) {
    c.push(paints("PathfinderBoolean", "geometry", |m| {
        let (front, back) = two_overlapping(m);
        Mutation::PathfinderBoolean {
            kept: ElementId::Polygon(front),
            others: vec![ElementId::Polygon(back)],
            kind: PathfinderKind::Union,
        }
    }));
    c.push(paints("PathfinderDivide", "geometry", |m| {
        Mutation::PathfinderDivide {
            element_ids: overlapping_ids(m),
        }
    }));
    c.push(paints("PathfinderTrim", "geometry", |m| {
        Mutation::PathfinderTrim {
            element_ids: overlapping_ids(m),
        }
    }));
    c.push(paints("PathfinderMerge", "geometry", |m| {
        Mutation::PathfinderMerge {
            element_ids: overlapping_ids(m),
        }
    }));
    c.push(paints("PathfinderCrop", "geometry", |m| {
        Mutation::PathfinderCrop {
            element_ids: overlapping_ids(m),
        }
    }));
    c.push(paints("PathfinderOutline", "geometry", |m| {
        Mutation::PathfinderOutline {
            element_ids: overlapping_ids(m),
        }
    }));
    c.push(paints("PathfinderMinusBack", "geometry", |m| {
        Mutation::PathfinderMinusBack {
            element_ids: overlapping_ids(m),
        }
    }));
    c.push(paints("PathfinderFaces", "geometry", |m| {
        let ids = overlapping_ids(m);
        let regions = m.planar_regions(&ids, None);
        assert!(regions.found, "planar regions: {:?}", regions.reason);
        // Remove the overlap face — the one both inputs cover.
        let overlap = regions
            .faces
            .iter()
            .find(|f| f.signature.len() > 1)
            .map(|f| f.id.clone())
            .expect("the two quads overlap");
        Mutation::PathfinderFaces {
            element_ids: ids,
            faces: vec![overlap],
            mode: FaceSelectMode::Remove,
        }
    }));
}

// ── conditions, masters, structure ──────────────────────────────────

fn structure_cases(c: &mut Vec<Case>) {
    c.push(paints("SetConditionVisible", "conditions", |m| {
        // The fixture declares one condition that is ALREADY hidden;
        // hiding it again is an honest no-op, so pick a visible one.
        let cond = m
            .scene()
            .styles
            .conditions
            .iter()
            .find(|(_, d)| d.visible != Some(false))
            .map(|(k, _)| k.clone())
            .expect("conditions fixture declares a visible condition");
        Mutation::SetConditionVisible {
            condition: cond,
            visible: false,
        }
    }));
    c.push(paints("ActivateConditionSet", "conditions", |m| {
        let set = m
            .scene()
            .styles
            .condition_sets
            .keys()
            .next()
            .cloned()
            .expect("conditions fixture declares a condition set");
        Mutation::ActivateConditionSet { set }
    }));
}

// ── layers ──────────────────────────────────────────────────────────

fn layer_cases(c: &mut Vec<Case>) {
    c.push(paints("LayerSetVisible", "layers-z", |m| {
        Mutation::LayerSetVisible {
            layer_id: layer_ids(m)[0].clone(),
            visible: false,
        }
    }));
    c.push(inert(
        "LayerSetLocked",
        "layers-z",
        "lock is an editing affordance — it gates selection in the host. \
         Unlike Visible, which the renderer resolves through ancestors, \
         nothing in the pipeline consults it",
        |m| Mutation::LayerSetLocked {
            layer_id: layer_ids(m)[0].clone(),
            locked: true,
        },
    ));
    // This case was written expecting INERT — "printable is an
    // output-time flag; a non-printing layer still shows on screen",
    // which is InDesign's behaviour. The engine disagrees, and the
    // engine is what this file measures: `build_layer_render_map` folds
    // printable into the same predicate as visible ("any item whose
    // ItemLayer points at a hidden or non-printable layer is
    // suppressed"), so items on the layer vanish from the canvas. The
    // expectation is corrected to match the observation; whether the
    // canvas SHOULD hide a non-printing layer is a fidelity question,
    // not a renders-nothing one.
    c.push(paints("LayerSetPrintable", "layers-z", |m| {
        Mutation::LayerSetPrintable {
            layer_id: layer_ids(m)[0].clone(),
            printable: false,
        }
    }));
    c.push(inert(
        "LayerSetName",
        "layers-z",
        "a layer's NAME is panel text; page items bind to a layer by Self \
         id, so renaming rebinds nothing",
        |m| Mutation::LayerSetName {
            layer_id: layer_ids(m)[0].clone(),
            name: "Renamed by the sweep".into(),
        },
    ));
    c.push(paints("LayerMove", "layers-z", |m| {
        // `layers-z` overlaps a rect on each of two layers, and the
        // renderer sorts `frames_in_order` by layer BEFORE the z table
        // (Q-10) — so reordering the layers inverts the occlusion.
        let ids = layer_ids(m);
        assert!(ids.len() > 1, "fixture must declare two layers");
        Mutation::LayerMove {
            layer_id: ids[0].clone(),
            new_index: (ids.len() - 1) as u32,
        }
    }));
    c.push(paints("LayerRemove", "layers-z", |m| {
        Mutation::LayerRemove {
            layer_id: layer_ids(m)[0].clone(),
        }
    }));
    c.push(when_used(
        "LayerInsert",
        "layers-z",
        "a new layer is empty: it holds no page item, and the renderer \
         paints items, not layers",
        |_| Mutation::LayerInsert {
            position: 0,
            name: "Sweep layer".into(),
        },
        |m| {
            // Move an item onto it — the layer sort runs before the z
            // table, so the occlusion changes.
            let target = layer_ids(m)
                .into_iter()
                .find(|_| true)
                .expect("a layer to move onto");
            let rect = rect_ids(m)[0].clone();
            m.apply_mutation(&Mutation::SetElementProperty {
                element_id: ElementId::Rectangle(rect),
                path: PropertyPath::ItemLayer,
                value: Value::Text(target),
            })
            .expect("move the item onto the new layer");
        },
    ));
}

// ── palette ─────────────────────────────────────────────────────────

fn palette_cases(c: &mut Vec<Case>) {
    c.push(when_used(
        "CreateSwatch",
        "geometry",
        "a palette entry is a DEFINITION: the renderer paints from \
         references, and no page item names it yet",
        |_| Mutation::CreateSwatch {
            spec: SwatchSpec {
                self_id: Some("Color/sweep-new".into()),
                name: Some("Sweep New".into()),
                space: "RGB".into(),
                value: vec![10.0, 200.0, 30.0],
                model: None,
                alternate_space: None,
                alternate_value: Vec::new(),
                tint: None,
                alpha: None,
            },
        },
        |m| {
            let rect = rect_ids(m)[0].clone();
            fill(m, &ElementId::Rectangle(rect), "Color/sweep-new");
        },
    ));
    c.push(paints("EditSwatch", "geometry", |m| {
        // Edit a colour a page item actually paints with.
        let id = color_in_use(m);
        Mutation::EditSwatch {
            swatch_id: id.clone(),
            spec: SwatchSpec {
                self_id: Some(id),
                name: Some("Recoloured by the sweep".into()),
                space: "RGB".into(),
                value: vec![250.0, 5.0, 120.0],
                model: None,
                alternate_space: None,
                alternate_value: Vec::new(),
                tint: None,
                alpha: None,
            },
        }
    }));
    c.push(paints("DeleteSwatch", "geometry", |m| {
        Mutation::DeleteSwatch {
            swatch_id: color_in_use(m),
        }
    }));
    c.push(when_used(
        "CreateGradient",
        "geometry",
        "a gradient swatch is a DEFINITION, like a colour — inert until a \
         page item's fill names it",
        |m| {
            let a = some_color(m);
            let b = m
                .scene()
                .palette
                .colors
                .keys()
                .filter(|k| !k.contains("None") && !k.contains("Paper"))
                .nth(1)
                .cloned()
                .unwrap_or_else(|| a.clone());
            let _ = &b;
            Mutation::CreateGradient {
                spec: GradientSpec {
                    self_id: Some("Gradient/sweep-new".into()),
                    name: Some("Sweep Ramp".into()),
                    kind: "Linear".into(),
                    stops: vec![
                        GradientStopSpec {
                            stop_color: a,
                            location_pct: 0.0,
                            midpoint_pct: None,
                        },
                        GradientStopSpec {
                            stop_color: b,
                            location_pct: 100.0,
                            midpoint_pct: None,
                        },
                    ],
                },
            }
        },
        |m| {
            let rect = rect_ids(m)[0].clone();
            fill(m, &ElementId::Rectangle(rect), "Gradient/sweep-new");
        },
    ));
    c.push(paints("EditGradient", "gradients", |m| {
        let id = m
            .scene()
            .palette
            .gradients
            .keys()
            .next()
            .cloned()
            .expect("gradients fixture carries a gradient");
        let a = some_color(m);
        Mutation::EditGradient {
            gradient_id: id.clone(),
            spec: GradientSpec {
                self_id: Some(id),
                name: Some("Sweep-edited ramp".into()),
                kind: "Radial".into(),
                stops: vec![
                    GradientStopSpec {
                        stop_color: a.clone(),
                        location_pct: 0.0,
                        midpoint_pct: None,
                    },
                    GradientStopSpec {
                        stop_color: a,
                        location_pct: 100.0,
                        midpoint_pct: None,
                    },
                ],
            },
        }
    }));
    c.push(paints("DeleteGradient", "gradients", |m| {
        let id = m
            .scene()
            .palette
            .gradients
            .keys()
            .next()
            .cloned()
            .expect("gradients fixture carries a gradient");
        Mutation::DeleteGradient { gradient_id: id }
    }));
    c.push(inert(
        "CreateColorGroup",
        "geometry",
        "a colour group is ORGANISATIONAL — a named folder in the Swatches \
         panel. Nothing in the pipeline branches on group membership, so \
         it paints nothing whether or not it is 'used'",
        |m| {
            let a = some_color(m);
            Mutation::CreateColorGroup {
                spec: ColorGroupSpec {
                    self_id: Some("ColorGroup/sweep".into()),
                    name: Some("Sweep group".into()),
                    members: vec![a],
                },
            }
        },
    ));
    c.push(inert(
        "EditColorGroup",
        "geometry",
        "see CreateColorGroup — regrouping swatches moves no paint",
        |m| {
            let a = some_color(m);
            m.apply_mutation(&Mutation::CreateColorGroup {
                spec: ColorGroupSpec {
                    self_id: Some("ColorGroup/sweep".into()),
                    name: Some("Sweep group".into()),
                    members: vec![a.clone()],
                },
            })
            .expect("seed a group");
            Mutation::EditColorGroup {
                group_id: "ColorGroup/sweep".into(),
                spec: ColorGroupSpec {
                    self_id: Some("ColorGroup/sweep".into()),
                    name: Some("Sweep group, renamed".into()),
                    members: vec![a],
                },
            }
        },
    ));
    c.push(inert(
        "DeleteColorGroup",
        "geometry",
        "see CreateColorGroup — deleting the folder leaves the swatches, so \
         nothing that paints is touched",
        |m| {
            let a = some_color(m);
            m.apply_mutation(&Mutation::CreateColorGroup {
                spec: ColorGroupSpec {
                    self_id: Some("ColorGroup/sweep".into()),
                    name: Some("Sweep group".into()),
                    members: vec![a],
                },
            })
            .expect("seed a group");
            Mutation::DeleteColorGroup {
                group_id: "ColorGroup/sweep".into(),
            }
        },
    ));
    c.push(inert(
        "ImportSwatchLibrary",
        "geometry",
        "an .ase import is a bulk CreateSwatch + CreateColorGroup: every \
         entry arrives as a definition nothing references yet",
        |_| Mutation::ImportSwatchLibrary {
            bytes: ByteBuf(sample_ase()),
            group_name: Some("Imported by the sweep".into()),
        },
    ));
    c.push(inert(
        "SetInkSetting",
        "swatches",
        "output-time ink routing (convert-to-process, plate aliasing) that \
         SEPARATIONS consume at export; the canvas shows the composite \
         preview, which the alternate-space colour already determines",
        |m| {
            let spot = m
                .inks()
                .first()
                .map(|i| i.spot_id.clone())
                .expect("swatches fixture carries a spot ink");
            Mutation::SetInkSetting {
                spot_id: spot,
                convert_to_process: true,
                alias_to: None,
            }
        },
    ));
    c.push(paints("SetUseStandardLabForSpots", "swatches", |m| {
        let _ = m;
        Mutation::SetUseStandardLabForSpots { enabled: true }
    }));
}

// ── styles ──────────────────────────────────────────────────────────

fn style_cases(c: &mut Vec<Case>) {
    c.push(when_used(
        "CreateParagraphStyle",
        "text",
        "a fresh style carries no overrides — it resolves to exactly what \
         the cascade already gave the text, so defining it changes nothing \
         until a property is set on it and it is applied",
        |_| Mutation::CreateParagraphStyle {
            self_id: Some("ParagraphStyle/sweep".into()),
            name: Some("Sweep Para".into()),
            based_on: None,
        },
        |m| {
            // Deliberately a FILL, not a point size. The fixture's runs
            // pin `PointSize` themselves, and a run's own attribute
            // outranks the paragraph style's, so a 30 pt style over 12 pt
            // runs is invisible — correct cascade behaviour that would
            // have read as "the style does nothing".
            let color = fresh_color(m);
            m.apply_mutation(&Mutation::SetStyleProperty {
                collection: StyleCollection::Paragraph,
                style_id: "ParagraphStyle/sweep".into(),
                path: PropertyPath::CharacterFillColor,
                value: Value::ColorRef(Some(color)),
            })
            .expect("give the style a fill");
            let (story_id, chars) = biggest_story(m);
            m.apply_mutation(&Mutation::ApplyStyle {
                story_id,
                start: 0,
                end: chars.min(40),
                style: "ParagraphStyle/sweep".into(),
                scope: StyleScope::Paragraph,
                cell: None,
            })
            .expect("apply the style");
        },
    ));
    c.push(inert(
        "RenameParagraphStyle",
        "styles-cascade",
        "paragraphs bind to a style by Self id; the Name is panel text, and \
         renaming rebinds nothing",
        |m| {
            let id = named_style(m, StyleCollection::Paragraph);
            Mutation::RenameParagraphStyle {
                style_id: id,
                name: "Renamed by the sweep".into(),
            }
        },
    ));
    c.push(paints("DeleteParagraphStyle", "styles-cascade", |m| {
        let id = applied_paragraph_style(m);
        Mutation::DeleteParagraphStyle { style_id: id }
    }));
    c.push(when_used(
        "CreateCharacterStyle",
        "text",
        "see CreateParagraphStyle — an override-free character style is \
         indistinguishable from the cascade it inherits",
        |_| Mutation::CreateCharacterStyle {
            self_id: Some("CharacterStyle/sweep".into()),
            name: Some("Sweep Char".into()),
            based_on: None,
        },
        |m| {
            // A fill, for the same reason as CreateParagraphStyle: the
            // runs pin their own PointSize and outrank the style.
            let color = fresh_color(m);
            m.apply_mutation(&Mutation::SetStyleProperty {
                collection: StyleCollection::Character,
                style_id: "CharacterStyle/sweep".into(),
                path: PropertyPath::CharacterFillColor,
                value: Value::ColorRef(Some(color)),
            })
            .expect("give the style a fill");
            let (story_id, chars) = biggest_story(m);
            m.apply_mutation(&Mutation::ApplyStyle {
                story_id,
                start: 0,
                end: chars.min(20),
                style: "CharacterStyle/sweep".into(),
                scope: StyleScope::Character,
                cell: None,
            })
            .expect("apply the style");
        },
    ));
    c.push(inert(
        "RenameCharacterStyle",
        "text",
        "see RenameParagraphStyle — runs bind by Self id",
        |m| Mutation::RenameCharacterStyle {
            style_id: seed_applied_character_style(m),
            name: "Renamed by the sweep".into(),
        },
    ));
    c.push(paints("DeleteCharacterStyle", "text", |m| {
        Mutation::DeleteCharacterStyle {
            style_id: seed_applied_character_style(m),
        }
    }));
    c.push(inert(
        "CreateObjectStyle",
        "geometry",
        "an object style is a DEFINITION; nothing carries \
         AppliedObjectStyle pointing at it yet",
        |_| Mutation::CreateObjectStyle {
            self_id: Some("ObjectStyle/sweep".into()),
            name: Some("Sweep Object".into()),
            based_on: None,
        },
    ));
    c.push(inert(
        "RenameObjectStyle",
        "geometry",
        "page items bind by Self id; the Name is panel text",
        |m| {
            m.apply_mutation(&Mutation::CreateObjectStyle {
                self_id: Some("ObjectStyle/sweep".into()),
                name: Some("Sweep Object".into()),
                based_on: None,
            })
            .expect("seed a style");
            Mutation::RenameObjectStyle {
                style_id: "ObjectStyle/sweep".into(),
                name: "Renamed by the sweep".into(),
            }
        },
    ));
    c.push(inert(
        "DeleteObjectStyle",
        "geometry",
        "deleting a style nothing applies leaves every page item resolving \
         exactly as before",
        |m| {
            m.apply_mutation(&Mutation::CreateObjectStyle {
                self_id: Some("ObjectStyle/sweep".into()),
                name: Some("Sweep Object".into()),
                based_on: None,
            })
            .expect("seed a style");
            Mutation::DeleteObjectStyle {
                style_id: "ObjectStyle/sweep".into(),
            }
        },
    ));
    c.push(inert(
        "CreateCellStyle",
        "tables",
        "a cell style is a DEFINITION; no cell carries AppliedCellStyle \
         pointing at it yet",
        |_| Mutation::CreateCellStyle {
            self_id: Some("CellStyle/sweep".into()),
            name: Some("Sweep Cell".into()),
            based_on: None,
        },
    ));
    c.push(inert(
        "RenameCellStyle",
        "tables",
        "cells bind by Self id; the Name is panel text",
        |m| {
            m.apply_mutation(&Mutation::CreateCellStyle {
                self_id: Some("CellStyle/sweep".into()),
                name: Some("Sweep Cell".into()),
                based_on: None,
            })
            .expect("seed a style");
            Mutation::RenameCellStyle {
                style_id: "CellStyle/sweep".into(),
                name: "Renamed by the sweep".into(),
            }
        },
    ));
    c.push(inert(
        "DeleteCellStyle",
        "tables",
        "deleting a style no cell applies leaves every cell resolving as \
         before",
        |m| {
            m.apply_mutation(&Mutation::CreateCellStyle {
                self_id: Some("CellStyle/sweep".into()),
                name: Some("Sweep Cell".into()),
                based_on: None,
            })
            .expect("seed a style");
            Mutation::DeleteCellStyle {
                style_id: "CellStyle/sweep".into(),
            }
        },
    ));
    c.push(inert(
        "CreateTableStyle",
        "tables",
        "a table style is a DEFINITION; no table carries AppliedTableStyle \
         pointing at it yet",
        |_| Mutation::CreateTableStyle {
            self_id: Some("TableStyle/sweep".into()),
            name: Some("Sweep Table".into()),
            based_on: None,
        },
    ));
    c.push(inert(
        "RenameTableStyle",
        "tables",
        "tables bind by Self id; the Name is panel text",
        |m| {
            m.apply_mutation(&Mutation::CreateTableStyle {
                self_id: Some("TableStyle/sweep".into()),
                name: Some("Sweep Table".into()),
                based_on: None,
            })
            .expect("seed a style");
            Mutation::RenameTableStyle {
                style_id: "TableStyle/sweep".into(),
                name: "Renamed by the sweep".into(),
            }
        },
    ));
    c.push(inert(
        "DeleteTableStyle",
        "tables",
        "deleting a style no table applies leaves every table resolving as \
         before",
        |m| {
            m.apply_mutation(&Mutation::CreateTableStyle {
                self_id: Some("TableStyle/sweep".into()),
                name: Some("Sweep Table".into()),
                based_on: None,
            })
            .expect("seed a style");
            Mutation::DeleteTableStyle {
                style_id: "TableStyle/sweep".into(),
            }
        },
    ));
    c.push(paints("SetStyleProperty", "styles-cascade", |m| {
        let id = applied_paragraph_style(m);
        Mutation::SetStyleProperty {
            collection: StyleCollection::Paragraph,
            style_id: id,
            path: PropertyPath::CharacterFontSize,
            value: Value::Length(Some(31.0)),
        }
    }));
    c.push(inert(
        "CreateNumberingList",
        "numbering",
        "a <NumberingList> is a named COUNTER definition; a paragraph joins \
         it via AppliedNumberingList, and only that binding changes what a \
         bullet renders",
        |_| Mutation::CreateNumberingList {
            spec: NumberingListSpec {
                self_id: Some("NumberingList/sweep".into()),
                name: Some("Sweep list".into()),
                continue_across_stories: Some(true),
                continue_across_documents: None,
            },
        },
    ));
    c.push(inert(
        "EditNumberingList",
        "numbering",
        "see CreateNumberingList — the flags only matter to paragraphs that \
         have joined the list",
        |m| {
            m.apply_mutation(&Mutation::CreateNumberingList {
                spec: NumberingListSpec {
                    self_id: Some("NumberingList/sweep".into()),
                    name: Some("Sweep list".into()),
                    continue_across_stories: Some(false),
                    continue_across_documents: None,
                },
            })
            .expect("seed a list");
            Mutation::EditNumberingList {
                list_id: "NumberingList/sweep".into(),
                spec: NumberingListSpec {
                    self_id: Some("NumberingList/sweep".into()),
                    name: Some("Sweep list, edited".into()),
                    continue_across_stories: Some(true),
                    continue_across_documents: None,
                },
            }
        },
    ));
    c.push(inert(
        "DeleteNumberingList",
        "numbering",
        "see CreateNumberingList — removing a definition nothing has joined \
         changes no counter",
        |m| {
            m.apply_mutation(&Mutation::CreateNumberingList {
                spec: NumberingListSpec {
                    self_id: Some("NumberingList/sweep".into()),
                    name: Some("Sweep list".into()),
                    continue_across_stories: Some(true),
                    continue_across_documents: None,
                },
            })
            .expect("seed a list");
            Mutation::DeleteNumberingList {
                list_id: "NumberingList/sweep".into(),
            }
        },
    ));
}

/// A style id from `collection` that is not one of IDML's reserved
/// `$ID/[...]` entries — safe to rename.
///
/// The filter tests for the `$ID/[` marker rather than for the word
/// "No": written as `!k.contains("No")` it also excluded every style
/// whose NAME happens to contain those two letters, which is how this
/// helper first failed to find a style at all.
fn named_style(m: &CanvasModel, collection: StyleCollection) -> String {
    let keys: Vec<String> = match collection {
        StyleCollection::Paragraph => m.scene().styles.paragraph_styles.keys().cloned().collect(),
        StyleCollection::Character => m.scene().styles.character_styles.keys().cloned().collect(),
        StyleCollection::Object => m.scene().styles.object_styles.keys().cloned().collect(),
        StyleCollection::Cell => m.scene().styles.cell_styles.keys().cloned().collect(),
        StyleCollection::Table => m.scene().styles.table_styles.keys().cloned().collect(),
    };
    keys.into_iter()
        .find(|k| !k.contains("$ID/["))
        .expect("fixture carries a user-defined style")
}

/// A paragraph style some paragraph actually applies — so deleting or
/// editing it is visible.
fn applied_paragraph_style(m: &CanvasModel) -> String {
    m.scene()
        .stories
        .iter()
        .flat_map(|s| s.story.paragraphs.iter())
        .filter_map(|p| p.paragraph_style.clone())
        .find(|s| !s.contains("NoParagraphStyle"))
        .expect("fixture applies a paragraph style")
}

/// Mint a character style that visibly changes the text, apply it, and
/// return its id.
///
/// No `paged-gen` fixture defines a user character style — every one
/// ships only the reserved `CharacterStyle/$ID/[No character style]`, so
/// "find one the document applies" finds the reserved entry, and
/// deleting THAT correctly changes nothing. The cases that need a real
/// applied character style build one.
fn seed_applied_character_style(m: &mut CanvasModel) -> String {
    const ID: &str = "CharacterStyle/sweep-applied";
    m.apply_mutation(&Mutation::CreateCharacterStyle {
        self_id: Some(ID.into()),
        name: Some("Sweep Applied".into()),
        based_on: None,
    })
    .expect("create the style");
    let color = fresh_color(m);
    m.apply_mutation(&Mutation::SetStyleProperty {
        collection: StyleCollection::Character,
        style_id: ID.into(),
        path: PropertyPath::CharacterFillColor,
        value: Value::ColorRef(Some(color)),
    })
    .expect("give the style a fill");
    let (story_id, chars) = biggest_story(m);
    m.apply_mutation(&Mutation::ApplyStyle {
        story_id,
        start: 0,
        end: chars.min(20),
        style: ID.into(),
        scope: StyleScope::Character,
        cell: None,
    })
    .expect("apply the style");
    ID.to_string()
}

// ── tables ──────────────────────────────────────────────────────────

fn table_cases(c: &mut Vec<Case>) {
    c.push(paints("InsertTable", "text", |m| {
        let (story_id, _) = biggest_story(m);
        Mutation::InsertTable {
            story_id,
            rows: 3,
            cols: 3,
            header_rows: 0,
            footer_rows: 0,
            column_widths: vec![80.0, 80.0, 80.0],
            row_heights: vec![20.0, 20.0, 20.0],
        }
    }));
    c.push(paints("SetRowHeight", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        Mutation::SetRowHeight {
            story_id,
            table_id,
            row: 0,
            height: Some(90.0),
        }
    }));
    c.push(paints("SetColumnWidth", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        Mutation::SetColumnWidth {
            story_id,
            table_id,
            col: 0,
            width: Some(150.0),
        }
    }));
    c.push(paints("InsertTableRow", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        Mutation::InsertTableRow {
            story_id,
            table_id,
            at: 1,
        }
    }));
    c.push(paints("DeleteTableRow", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        Mutation::DeleteTableRow {
            story_id,
            table_id,
            at: 1,
        }
    }));
    c.push(paints("InsertTableColumn", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        Mutation::InsertTableColumn {
            story_id,
            table_id,
            at: 1,
        }
    }));
    c.push(paints("DeleteTableColumn", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        Mutation::DeleteTableColumn {
            story_id,
            table_id,
            at: 1,
        }
    }));
    c.push(paints("InsertHeaderRow", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        Mutation::InsertHeaderRow { story_id, table_id }
    }));
    c.push(paints("RemoveHeaderRow", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        m.apply_mutation(&Mutation::InsertHeaderRow {
            story_id: story_id.clone(),
            table_id: table_id.clone(),
        })
        .expect("seed a header row");
        // The seeded row now arrives with the reference row's height, so
        // this is no longer load-bearing — but pinning an unmistakable
        // 36pt keeps the case measuring the REMOVAL rather than whatever
        // the insert happened to inherit.
        m.apply_mutation(&Mutation::SetRowHeight {
            story_id: story_id.clone(),
            table_id: table_id.clone(),
            row: 0,
            height: Some(36.0),
        })
        .expect("make the seeded header row visible");
        Mutation::RemoveHeaderRow { story_id, table_id }
    }));
    c.push(paints("InsertFooterRow", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        Mutation::InsertFooterRow { story_id, table_id }
    }));
    c.push(paints("RemoveFooterRow", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        m.apply_mutation(&Mutation::InsertFooterRow {
            story_id: story_id.clone(),
            table_id: table_id.clone(),
        })
        .expect("seed a footer row");
        // Sizing the seeded row alone used not to be enough here, and the
        // difference is instructive: a HEADER row at index 0 displaces
        // every body row below it, so height alone makes it observable —
        // a FOOTER row appended below the last row displaces nothing, so
        // it has to draw something of its own. It now does (the minted
        // cells carry edges), but this case still puts TEXT in the footer
        // cell so the removal is unmistakable either way.
        let last = m
            .scene()
            .stories
            .iter()
            .flat_map(|s| s.story.paragraphs.iter())
            .find_map(|p| p.table.as_ref().map(|t| t.rows.len()))
            .expect("table rows")
            - 1;
        m.apply_mutation(&Mutation::SetRowHeight {
            story_id: story_id.clone(),
            table_id: table_id.clone(),
            row: last as u32,
            height: Some(36.0),
        })
        .expect("size the seeded footer row");
        m.apply_mutation(&Mutation::InsertText {
            story_id: story_id.clone(),
            offset: 0,
            text: "Total".into(),
            cell: Some(TextCellAddr {
                table_id: table_id.clone(),
                row: last as u32,
                col: 0,
            }),
        })
        .expect("give the seeded footer row something to draw");
        Mutation::RemoveFooterRow { story_id, table_id }
    }));
    c.push(paints("SetCellSpan", "tables", |m| {
        let (story_id, table_id) = first_table(m);
        Mutation::SetCellSpan {
            story_id,
            table_id,
            row: 0,
            col: 0,
            row_span: 1,
            column_span: 2,
        }
    }));
}

// ── document-level ──────────────────────────────────────────────────

fn document_cases(c: &mut Vec<Case>) {
    c.push(inert(
        "SetDocumentDefaults",
        "geometry",
        "the fill / stroke wells for objects NOT YET CREATED. The model \
         documents it as app-level state: no rebuild, no undo entry, no \
         pixel change — the next insert reads it",
        |m| {
            let color = some_color(m);
            Mutation::SetDocumentDefaults {
                fill_color: Some(color),
                stroke_color: None,
                stroke_weight: Some(9.0),
            }
        },
    ));
    c.push(paints("SetColorSettings", "geometry", |m| {
        let _ = m;
        Mutation::SetColorSettings {
            cmyk_profile_name: Some(SWEEP_CMYK.into()),
            rgb_policy: None,
            intent: Some("Perceptual".into()),
            bpc: Some(true),
        }
    }));
    c.push(paints("SetProofSetup", "geometry", |m| {
        let _ = m;
        Mutation::SetProofSetup {
            profile_name: Some(SWEEP_CMYK.into()),
            simulate_paper_white: true,
            intent: Some("AbsoluteColorimetric".into()),
        }
    }));
}
