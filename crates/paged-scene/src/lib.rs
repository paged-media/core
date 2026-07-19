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

//! Resolved scene graph.
//!
//! `Document` is the single object consumers get from this crate. It
//! owns the parsed IDML container, the swatch palette, and pre-parsed
//! spreads and stories. Pipelines that used to open + parse everything
//! inline now accept a `&Document` and walk its fields.
//!
//! Style cascades (paragraph → character → local overrides), link
//! resolution, and master-spread inheritance will lean on this crate
//! as it matures. For the current slice it's a thin owning wrapper
//! that removes the duplicated parsing code from `paged-renderer`.

use std::collections::{BTreeMap, HashMap};

use paged_parse::{
    Bounds, CharacterRun, Container, Graphic, Paragraph, ParseError, Spread, Story, StoryRef,
    StyleSheet, TOCStyleDef, TextFrame,
};

pub mod anchors;
pub mod layer;
pub mod value;
pub use anchors::{Anchor, AnchorId, AnchorKind, Field, FieldKind};
pub use layer::{
    build_layer_locked_map, build_layer_render_map, layer_locked, layer_render_visible, layer_z,
    layer_z_index, lookup_layer_locked, lookup_layer_render_visible,
};
pub use value::Value;

/// Owned, parsed representation of an IDML document.
#[derive(Debug, Clone)]
pub struct Document {
    pub container: Container,
    pub palette: Graphic,
    pub spreads: Vec<ParsedSpread>,
    pub stories: Vec<ParsedStory>,
    /// Master spreads, indexed by their `Self` id (e.g.
    /// `MasterSpread/uad`). Pages reference these via
    /// `Page::applied_master`.
    pub master_spreads: HashMap<String, ParsedMasterSpread>,
    /// `TextFrame` indexed by its `ParentStory` id — built once so the
    /// pipeline doesn't have to scan every spread for each story.
    pub frame_for_story: HashMap<String, TextFrame>,
    /// `(spread_idx, frame_idx)` for each TextFrame keyed by its
    /// `Self` id. Built at open time so [`text_frame`] is O(1) and
    /// [`frame_chain`] walks long NextTextFrame chains in linear
    /// time rather than O(K × total_frames).
    pub text_frame_index: HashMap<String, (usize, usize)>,
    /// Paragraph + character style definitions loaded from
    /// `Resources/Styles.xml`. Empty when the archive has no styles
    /// resource (rare; typically only synthetic test docs).
    pub styles: StyleSheet,
    /// Anchor table — heading paragraphs detected at parse time
    /// (Phase G of the canvas plan). Other anchor kinds (footnotes,
    /// cross-ref targets, bookmarks) join in subsequent Phase 2
    /// work as the parser emits the corresponding markers.
    pub anchors: Vec<Anchor>,
}

/// A spread plus the path it came from in the container.
#[derive(Debug, Clone)]
pub struct ParsedSpread {
    pub src: String,
    pub spread: Spread,
}

/// A story plus its `Self` id (derived from the manifest src) and
/// source path.
#[derive(Debug, Clone)]
pub struct ParsedStory {
    pub src: String,
    pub self_id: String,
    pub story: Story,
}

/// A master spread plus the `Self` id pages reference it by. The
/// XML schema is identical to a regular `<Spread>`, so we reuse
/// `Spread` for the geometry payload.
#[derive(Debug, Clone)]
pub struct ParsedMasterSpread {
    pub src: String,
    pub self_id: String,
    pub spread: Spread,
}

/// Cap on the number of frames followed via `NextTextFrame`.
/// Real chains are 1–10 frames; the cap exists so a malformed
/// document with a missed cycle can't make the resolver loop.
const MAX_FRAME_CHAIN: usize = 256;

/// The IDML default gutter between text columns, in pt (used when a frame
/// declares a column count but no `TextColumnGutter`).
const DEFAULT_COLUMN_GUTTER_PT: f32 = 12.0;

/// A text frame's **content box** as a neutral [`paged_flow::RegionGeometry`].
/// Thin adapter over [`content_box_geometry`] reading the frame's fields.
fn text_frame_content_box(frame: &TextFrame) -> paged_flow::RegionGeometry {
    content_box_geometry(
        frame.bounds,
        frame.inset_spacing,
        frame.column_count,
        frame.column_gutter,
    )
}

/// The flow region geometry of a text frame from its raw geometry inputs.
///
/// - **`width`** is the content-box width (bounds width minus the left/right
///   text insets) — the width text is line-broken to.
/// - **`height`** is the **full bounds height**, *not* bounds-minus-insets:
///   it must model the renderer's vertical **overflow reference**, and the
///   story emitter overflows a frame at its full bounds height
///   (`build_engine.rs`: `frame_height_64 = bounds.height()`), applying the
///   per-frame footnote reservation on top at emit time (which this static
///   projection cannot know). So the flow height is the frame's full extent;
///   footnote reservation is layered by the emitter.
///
/// `InsetSpacing` is IDML order `[top, left, bottom, right]`. Sizes are
/// clamped non-negative. This is the frame's own *local* box — the
/// `ItemTransform`/spread placement is composition-positioning, resolved
/// downstream. Kept pure so the geometry math is unit-testable without
/// constructing a full `TextFrame`.
fn content_box_geometry(
    bounds: Bounds,
    inset_spacing: Option<[f32; 4]>,
    column_count: Option<u32>,
    column_gutter: Option<f32>,
) -> paged_flow::RegionGeometry {
    let [_inset_top, inset_left, _inset_bottom, inset_right] = inset_spacing.unwrap_or([0.0; 4]);
    let width = (bounds.width() - inset_left - inset_right).max(0.0);
    let height = bounds.height().max(0.0);
    paged_flow::RegionGeometry {
        width_pt: width,
        height_pt: height,
        columns: column_count.unwrap_or(1).max(1),
        column_gap_pt: column_gutter.unwrap_or(DEFAULT_COLUMN_GUTTER_PT),
    }
}

impl Document {
    /// Parse every resource the manifest points at. Missing spreads
    /// or stories produce an [`OpenError::MissingEntry`] — the parse
    /// layer's tolerant behaviour (skipping entries without an
    /// archive match) is lifted here to a structured error.
    pub fn open(bytes: &[u8]) -> Result<Self, OpenError> {
        let container = Container::open(bytes)?;
        let palette = match container.entry("Resources/Graphic.xml") {
            Some(raw) => Graphic::parse(raw)?,
            None => Graphic::default(),
        };
        let styles = match container.entry("Resources/Styles.xml") {
            Some(raw) => StyleSheet::parse(raw)?,
            None => StyleSheet::default(),
        };

        // Master spreads parse first so the page → master link is
        // available downstream. The IDML schema for a `<MasterSpread>`
        // is identical to a `<Spread>` (same Page / TextFrame /
        // Rectangle children), so we reuse `Spread::parse`.
        let mut master_spreads: HashMap<String, ParsedMasterSpread> = HashMap::new();
        for src in &container.designmap.master_spreads {
            let raw = container
                .entry(src)
                .ok_or_else(|| OpenError::MissingEntry(src.clone()))?;
            let parsed = Spread::parse(raw)?;
            let self_id = derive_master_id(src);
            master_spreads.insert(
                self_id.clone(),
                ParsedMasterSpread {
                    src: src.clone(),
                    self_id,
                    spread: parsed,
                },
            );
        }

        let mut spreads = Vec::with_capacity(container.designmap.spreads.len());
        let mut frame_for_story = HashMap::new();
        let mut text_frame_index: HashMap<String, (usize, usize)> = HashMap::new();
        for spread_ref in &container.designmap.spreads {
            let raw = container
                .entry(&spread_ref.src)
                .ok_or_else(|| OpenError::MissingEntry(spread_ref.src.clone()))?;
            let parsed = Spread::parse(raw)?;
            let spread_idx = spreads.len();
            for (frame_idx, frame) in parsed.text_frames.iter().enumerate() {
                if let Some(id) = frame.parent_story.clone() {
                    frame_for_story.insert(id, frame.clone());
                }
                if let Some(self_id) = frame.self_id.clone() {
                    text_frame_index.insert(self_id, (spread_idx, frame_idx));
                }
            }
            spreads.push(ParsedSpread {
                src: spread_ref.src.clone(),
                spread: parsed,
            });
        }

        let mut stories = Vec::with_capacity(container.designmap.stories.len());
        for story_ref in &container.designmap.stories {
            let raw = container
                .entry(&story_ref.src)
                .ok_or_else(|| OpenError::MissingEntry(story_ref.src.clone()))?;
            let parsed = Story::parse(raw)?;
            let self_id = derive_story_id(&story_ref.src);
            stories.push(ParsedStory {
                src: story_ref.src.clone(),
                self_id,
                story: parsed,
            });
        }

        // Build the heading-anchor table from every story. This is
        // Phase G of the canvas plan: heading paragraphs become
        // anchors so the Tier 3 resolver can populate the
        // numbering map for cross-references and TOC entries.
        let mut anchors: Vec<Anchor> = Vec::new();
        for parsed_story in &stories {
            for (paragraph_index, paragraph) in parsed_story.story.paragraphs.iter().enumerate() {
                let Some(style_name) = paragraph.paragraph_style.as_deref() else {
                    continue;
                };
                if !anchors::paragraph_style_is_heading(style_name) {
                    continue;
                }
                let level = anchors::heading_level_from_style(style_name);
                anchors.push(Anchor {
                    id: AnchorId::heading(&parsed_story.self_id, paragraph_index),
                    story_id: parsed_story.self_id.clone(),
                    paragraph_index,
                    kind: AnchorKind::HeadingParagraph { level },
                });
            }
        }

        Ok(Document {
            container,
            palette,
            spreads,
            stories,
            master_spreads,
            frame_for_story,
            text_frame_index,
            styles,
            anchors,
        })
    }

    /// Look up a master spread by its `Self` id (the suffix stripped
    /// from the manifest src) or by the full reference value used in
    /// `Page::applied_master` (e.g. `MasterSpread/uad`).
    pub fn master_spread(&self, reference: &str) -> Option<&ParsedMasterSpread> {
        if let Some(m) = self.master_spreads.get(reference) {
            return Some(m);
        }
        // `applied_master` is typically `MasterSpread/<id>`; our key is
        // the bare `<id>`. Strip the prefix when needed.
        let stripped = reference
            .rsplit_once('/')
            .map(|(_, id)| id)
            .unwrap_or(reference);
        self.master_spreads.get(stripped)
    }

    /// The frame that hosts a story, looked up by the story's
    /// `self_id`. `None` means the story is unplaced — permissible
    /// in IDML.
    pub fn frame_for(&self, story_id: &str) -> Option<&TextFrame> {
        self.frame_for_story.get(story_id)
    }

    /// Look up a `TextFrame` by its `Self` id (e.g. `frameA`).
    /// O(1) via the `text_frame_index` map built at open time.
    pub fn text_frame(&self, frame_self_id: &str) -> Option<&TextFrame> {
        let &(spread_idx, frame_idx) = self.text_frame_index.get(frame_self_id)?;
        self.spreads
            .get(spread_idx)
            .and_then(|s| s.spread.text_frames.get(frame_idx))
    }

    /// Frame chain for a story: starts at the frame that is the
    /// chain head (a frame hosting `story_id` whose `Self` id is
    /// not another frame's `NextTextFrame` target) and follows
    /// `NextTextFrame` links until exhaustion. Cycles are bounded
    /// by `MAX_FRAME_CHAIN` so a malformed document can't hang.
    /// Returns `Vec<&TextFrame>` borrowing from the document.
    pub fn frame_chain(&self, story_id: &str) -> Vec<&TextFrame> {
        // Collect every frame on this story (typically 1; can be N
        // when the story is threaded across multiple frames).
        let mut story_frames: Vec<&TextFrame> = Vec::new();
        for parsed in &self.spreads {
            for f in &parsed.spread.text_frames {
                if f.parent_story.as_deref() == Some(story_id) {
                    story_frames.push(f);
                }
            }
        }
        if story_frames.is_empty() {
            return Vec::new();
        }
        // The head is whichever frame on this story isn't another
        // frame's NextTextFrame target. If every frame is targeted
        // (i.e. a cycle), fall back to the first frame found.
        let targeted: std::collections::HashSet<&str> = story_frames
            .iter()
            .filter_map(|f| f.next_text_frame.as_deref())
            .collect();
        let head = story_frames
            .iter()
            .find(|f| match f.self_id.as_deref() {
                Some(id) => !targeted.contains(id),
                None => true,
            })
            .copied()
            .unwrap_or(story_frames[0]);

        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        out.push(head);
        if let Some(id) = head.self_id.as_deref() {
            seen.insert(id.to_string());
        }
        let mut cursor = head.next_text_frame.clone();
        for _ in 0..MAX_FRAME_CHAIN {
            let Some(id) = cursor else { break };
            if seen.contains(&id) {
                break;
            }
            let Some(next) = self.text_frame(&id) else {
                break;
            };
            out.push(next);
            seen.insert(id);
            cursor = next.next_text_frame.clone();
        }
        out
    }

    /// A story's frame chain, expressed as a **content-agnostic region chain**
    /// (`paged_flow::RegionChain`) — the composition-format §5 flow protocol.
    ///
    /// This is the seam ADR-021 makes explicit: the *region-chain is
    /// arrangement* (composition-owned), the *story is content* (part-owned).
    /// Today the two are entangled — the IDML `TextFrame` carries both its
    /// geometry (arrangement) and its `NextTextFrame` thread + `ParentStory`
    /// link (which stitch arrangement to content). This projects the
    /// arrangement half out into the neutral vocabulary: each frame in
    /// [`frame_chain`](Self::frame_chain) becomes a [`Region`] whose id is the
    /// frame's `Self` id and whose geometry is the frame's flow box (content-box
    /// width, full bounds height — the emitter's overflow reference; see
    /// [`content_box_geometry`]). The flow id is the `story_id`.
    ///
    /// The geometry here is the frame's *local* content box (its own bounds,
    /// pre-`ItemTransform`); transform-aware page geometry and footnote
    /// reservations remain the renderer's job (`StoryEmitter`). This method
    /// changes no render path — it is the additive bridge that lets the
    /// content-agnostic driver ([`paged_flow::run_flow`]) drive the same
    /// region ordering the renderer walks imperatively today.
    pub fn flow_chain(&self, story_id: &str) -> paged_flow::RegionChain {
        let regions = self
            .frame_chain(story_id)
            .into_iter()
            .enumerate()
            .map(|(i, frame)| {
                // A frame with no `Self` id is synthetic; give it a stable
                // positional id so the region chain stays addressable.
                let id = frame
                    .self_id
                    .clone()
                    .unwrap_or_else(|| format!("{story_id}#frame{i}"));
                paged_flow::Region::new(id, text_frame_content_box(frame))
            })
            .collect();
        paged_flow::RegionChain::new(paged_flow::FlowId::new(story_id), regions)
    }

    /// Project this IDML document's **arrangement** into a Paged-native
    /// [`paged_composition::Composition`] (`document.pgd`) — the IDML→
    /// composition adapter (composition-format §9). Maps spreads/pages → pages,
    /// and each story's frame chain → a `Flow` + text-frame `Region`s tagged
    /// with that flow (bound to `publishing:story/<id>`, geometry from the
    /// frame's content box). The story *content* stays in the publishing part;
    /// this projects only the **arrangement**, so
    /// `to_composition().flow_chain(FlowId(story)) == self.flow_chain(story)`.
    ///
    /// Slice-3 scope: text-frame flows + pages. Rectangles/ovals/groups/layers,
    /// master-spread templates, anchored objects, and resource references are
    /// later slices; region positions are best-effort (the frame's spread-space
    /// translation) pending the positioning solver.
    pub fn to_composition(&self) -> paged_composition::Composition {
        use paged_composition::{
            Bind, Composition, Flow, Node, Page, PartRef, Position, Region, Surface, SurfaceKind,
        };

        let mut comp = Composition::new(1);
        comp.capabilities = vec![
            "flow.regionChain@1".to_string(),
            "surface.print@1".to_string(),
        ];
        comp.surfaces = vec![Surface {
            id: "print".to_string(),
            kind: SurfaceKind::Print,
        }];

        // Pages from spreads.
        let mut first_page_id: Option<String> = None;
        for parsed in &self.spreads {
            let spread_id = parsed.spread.self_id.clone();
            for page in &parsed.spread.pages {
                let id = page
                    .self_id
                    .clone()
                    .unwrap_or_else(|| format!("page{}", comp.pages.len()));
                if first_page_id.is_none() {
                    first_page_id = Some(id.clone());
                }
                comp.pages.push(Page {
                    id,
                    size: [page.bounds.width(), page.bounds.height()],
                    spread: spread_id.clone(),
                });
            }
        }

        // One flow per threaded story; its frames become flow-tagged regions.
        for parsed in &self.stories {
            let chain = self.frame_chain(&parsed.self_id);
            if chain.is_empty() {
                continue;
            }
            let flow_id = paged_flow::FlowId::new(&parsed.self_id);
            let selector = format!("story/{}", parsed.self_id);
            comp.flows.push(Flow {
                id: flow_id.clone(),
                part: PartRef::new("publishing"),
                selector: selector.clone(),
            });
            for (i, frame) in chain.iter().enumerate() {
                let id = frame
                    .self_id
                    .clone()
                    .unwrap_or_else(|| format!("{}#frame{i}", parsed.self_id));
                // Best-effort page: the frame's hosting spread's first page.
                let page = frame
                    .self_id
                    .as_deref()
                    .and_then(|fid| self.text_frame_index.get(fid))
                    .and_then(|&(sp, _)| self.spreads.get(sp))
                    .and_then(|s| s.spread.pages.first())
                    .and_then(|p| p.self_id.clone())
                    .or_else(|| first_page_id.clone())
                    .unwrap_or_else(|| "page0".to_string());
                let at = match frame.item_transform {
                    Some(m) => [m[4], m[5]],
                    None => [frame.bounds.left, frame.bounds.top],
                };
                comp.nodes.push(Node::Region(Region {
                    id: paged_flow::RegionId::new(id),
                    bind: Bind::Part {
                        part: PartRef::new("publishing"),
                        selector: selector.clone(),
                    },
                    position: Position::PageRelative { page, at },
                    geometry: text_frame_content_box(frame),
                    layer: None,
                    flow: Some(flow_id.clone()),
                    visible_on: Vec::new(),
                }));
            }
        }
        comp
    }

    /// Bytes of a sub-resource in the underlying container (fonts,
    /// linked images, ICC profiles — anything the manifest or frames
    /// reference but that `Document` doesn't parse itself).
    pub fn entry(&self, path: &str) -> Option<&[u8]> {
        self.container.entry(path).map(|b| b.as_ref())
    }

    /// Resolve a run's effective character-level attributes by
    /// walking the cascade: direct on the run > applied character
    /// style > applied paragraph style. Each attribute falls
    /// through to the next layer only when unset above.
    pub fn resolved_run_attrs(
        &self,
        paragraph: &Paragraph,
        run: &CharacterRun,
    ) -> ResolvedRunAttrs {
        let mut acc = ResolvedRunAttrs::from_run(run);
        if let Some(id) = run.character_style.as_deref() {
            acc.merge_below_character(&self.styles.resolve_character(id));
        }
        if let Some(id) = paragraph.paragraph_style.as_deref() {
            acc.merge_below_paragraph(&self.styles.resolve_paragraph(id));
        }
        acc
    }

    /// Resolve a paragraph's effective paragraph-level attributes.
    /// The cascade is direct > applied paragraph style. Character
    /// styles don't carry paragraph attrs in IDML.
    pub fn resolved_paragraph_attrs(&self, paragraph: &Paragraph) -> ResolvedParagraphAttrs {
        let mut acc = ResolvedParagraphAttrs::from_paragraph(paragraph);
        if let Some(id) = paragraph.paragraph_style.as_deref() {
            acc.merge_below(&self.styles.resolve_paragraph(id));
        }
        acc
    }

    /// Manifest-advertised story metadata; a convenience for callers
    /// that need the original src paths without walking `stories`.
    pub fn story_refs(&self) -> &[StoryRef] {
        &self.container.designmap.stories
    }

    /// Stable 256-bit content hash of the editable parts of the
    /// document (stories, spreads' frame definitions, anchor table).
    /// Phase 3 Item 6 — drives determinism tests for the mutation
    /// pipeline: applying the same mutation log to the same source
    /// bytes twice must produce identical hashes (AC-E-7).
    ///
    /// Walks the document in **stable order** (parser-order Vecs;
    /// sorted-key HashMaps) and feeds bytes into blake3. Skips
    /// derived caches (`frame_for_story`, `text_frame_index`) since
    /// they're functions of the underlying data. The hash is *not*
    /// guaranteed equal across renderer versions — only across runs
    /// of the same binary.
    /// W1.13 — fold a run list's text + font + size into the canonical
    /// hash. Shared by the body-paragraph and table-cell-paragraph
    /// passes so both streams hash identically (a cell edit and the
    /// same body edit produce the same per-run contribution).
    fn hash_runs(h: &mut blake3::Hasher, runs: &[CharacterRun]) {
        for (ri, r) in runs.iter().enumerate() {
            h.update(b"\0r");
            h.update(&(ri as u32).to_le_bytes());
            h.update(b"\0t\0");
            h.update(r.text.as_bytes());
            if let Some(font) = r.font.as_deref() {
                h.update(b"\0f\0");
                h.update(font.as_bytes());
            }
            if let Some(size) = r.point_size {
                h.update(b"\0sz\0");
                h.update(&size.to_le_bytes());
            }
        }
    }

    pub fn canonical_hash(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        // v2 (W1.13): cell-paragraph text now folds into the hash so
        // table-cell edits + their undo are observable to determinism
        // replay and the canvas undo gate. Bumped from v1 to invalidate
        // any persisted v1 digests.
        h.update(b"paged-scene-canonical-v2");

        // Stories — text + style names, in document order.
        h.update(b"stories:");
        for s in &self.stories {
            h.update(b"\0story\0");
            h.update(s.self_id.as_bytes());
            for (pi, p) in s.story.paragraphs.iter().enumerate() {
                h.update(b"\0p");
                h.update(&(pi as u32).to_le_bytes());
                if let Some(style) = p.paragraph_style.as_deref() {
                    h.update(b"\0ps\0");
                    h.update(style.as_bytes());
                }
                Self::hash_runs(&mut h, &p.runs);
                // W1.13 — table-cell paragraph text. A paragraph hosting
                // a `<Table>` folds each cell's `(col,row)` + paragraph
                // text into the hash so cell edits change the digest
                // (and an undo restores it). Walks cells in document
                // order; the per-cell `Name` keys the address so two
                // cells with identical text stay distinguishable.
                if let Some(table) = p.table.as_ref() {
                    h.update(b"\0tbl\0");
                    if let Some(tid) = table.self_id.as_deref() {
                        h.update(tid.as_bytes());
                    }
                    for cell in &table.cells {
                        h.update(b"\0cell\0");
                        if let Some(name) = cell.name.as_deref() {
                            h.update(name.as_bytes());
                        }
                        for (cpi, cp) in cell.paragraphs.iter().enumerate() {
                            h.update(b"\0cp");
                            h.update(&(cpi as u32).to_le_bytes());
                            if let Some(style) = cp.paragraph_style.as_deref() {
                                h.update(b"\0cps\0");
                                h.update(style.as_bytes());
                            }
                            Self::hash_runs(&mut h, &cp.runs);
                        }
                    }
                }
            }
        }

        // Spreads — frame bounds + transforms, in document order.
        h.update(b"spreads:");
        for ps in &self.spreads {
            h.update(b"\0spread\0");
            h.update(ps.src.as_bytes());
            for f in &ps.spread.text_frames {
                h.update(b"\0tf\0");
                if let Some(id) = f.self_id.as_deref() {
                    h.update(id.as_bytes());
                }
                hash_bounds(&mut h, &f.bounds);
                hash_transform(&mut h, f.item_transform.as_ref());
            }
            for r in &ps.spread.rectangles {
                h.update(b"\0rc\0");
                if let Some(id) = r.self_id.as_deref() {
                    h.update(id.as_bytes());
                }
                hash_bounds(&mut h, &r.bounds);
                hash_transform(&mut h, r.item_transform.as_ref());
            }
        }

        // Anchors — count + ids in stable order.
        h.update(b"anchors:");
        for a in &self.anchors {
            h.update(b"\0a\0");
            h.update(a.id.as_str().as_bytes());
            h.update(b"\0s\0");
            h.update(a.story_id.as_bytes());
            h.update(&a.paragraph_index.to_le_bytes());
        }

        *h.finalize().as_bytes()
    }

    /// Phase 5 — resolve every `<PageReference>` / `<IndexEntry>` /
    /// `<Index>` marker in the document into a sorted, deduplicated
    /// index. Each `IndexEntry` carries a topic plus the (body)
    /// page-index list of every paragraph that anchors a marker for
    /// that topic.
    ///
    /// Topics are grouped case-insensitively by their `topic_name`
    /// (the field the parser populates from `TopicName` directly, or
    /// from `AppliedTopic` when only the topic id is given). Page
    /// lists are deduplicated then sorted ascending so the renderer
    /// can emit "Apple ............. 12, 23, 41" rows without
    /// further processing.
    ///
    /// Like `resolve_toc`, the page-index assignment uses the head
    /// frame of the paragraph's parent story. Threaded stories that
    /// break across multiple frames resolve every marker to the
    /// chain head — the conservative choice. Frames with no host
    /// (orphan stories) drop the marker silently.
    pub fn resolve_index(&self) -> Vec<IndexEntry> {
        let mut by_topic: BTreeMap<String, IndexEntry> = BTreeMap::new();
        let body_page_index = self.body_page_index_map();
        for parsed in &self.stories {
            let host_page = body_page_index.get(&parsed.self_id).copied();
            for paragraph in &parsed.story.paragraphs {
                Self::collect_index_markers_from_paragraph(paragraph, host_page, &mut by_topic);
                // Markers can also appear inside table cells.
                if let Some(table) = paragraph.table.as_ref() {
                    for cell in &table.cells {
                        for cell_para in &cell.paragraphs {
                            Self::collect_index_markers_from_paragraph(
                                cell_para,
                                host_page,
                                &mut by_topic,
                            );
                        }
                    }
                }
            }
        }
        // Sort each entry's page list + deduplicate. Topics are
        // already in sorted order via BTreeMap.
        let mut out: Vec<IndexEntry> = by_topic.into_values().collect();
        for entry in &mut out {
            entry.pages.sort();
            entry.pages.dedup();
        }
        out
    }

    /// Internal — walk one paragraph's `index_markers` and fold them
    /// into the topic accumulator. Pulled out so the table-cell
    /// loop can reuse it.
    fn collect_index_markers_from_paragraph(
        paragraph: &paged_parse::Paragraph,
        host_page: Option<usize>,
        by_topic: &mut BTreeMap<String, IndexEntry>,
    ) {
        for marker in &paragraph.index_markers {
            let key = marker.topic_name.to_lowercase();
            let entry = by_topic.entry(key).or_insert_with(|| IndexEntry {
                topic: marker.topic_name.clone(),
                sort_key: marker
                    .sort_order
                    .clone()
                    .unwrap_or_else(|| marker.topic_name.to_lowercase()),
                pages: Vec::new(),
            });
            if let Some(p) = host_page {
                entry.pages.push(p);
            }
        }
    }

    /// Resolve a `<TOCStyle>` into a flat ordered list of TOC entries.
    ///
    /// Walks every story's paragraphs in document order; whenever a
    /// paragraph's `AppliedParagraphStyle` matches an entry's
    /// `IncludeStyle`, we emit a [`TOCEntry`] carrying:
    ///
    /// - the entry's `level` / `separator` / page-number toggle,
    /// - the resolved entry text (concatenated runs, stripped of
    ///   placeholder markers),
    /// - the body page-index of the paragraph's host frame.
    ///
    /// The page-index assignment is conservative: it returns the page
    /// hosting the *head* frame of the paragraph's parent story. A
    /// long threaded story that breaks across many frames still
    /// resolves every TOC entry from its style to that head frame —
    /// the renderer's chain pass is what would distribute paragraphs
    /// across pages at emit time, and the scene layer doesn't run
    /// composition. Most real-world TOC fixtures use one frame per
    /// story per page anyway, so the heuristic matches InDesign's
    /// output in the common case.
    ///
    /// `None` for an entry's `page_number` means the document has no
    /// host frame for the paragraph (orphan story) — the renderer
    /// should suppress the page number for that row.
    pub fn resolve_toc(&self, toc_style: &TOCStyleDef) -> Vec<TOCEntry> {
        let mut out: Vec<TOCEntry> = Vec::new();
        let body_page_index = self.body_page_index_map();
        for parsed in &self.stories {
            for paragraph in &parsed.story.paragraphs {
                let Some(applied) = paragraph.paragraph_style.as_deref() else {
                    continue;
                };
                let Some(entry_def) = toc_style
                    .entries
                    .iter()
                    .find(|e| e.include_style.as_deref() == Some(applied))
                else {
                    continue;
                };
                let text = paragraph_plain_text(paragraph);
                if text.is_empty() {
                    continue;
                }
                let page_number = body_page_index.get(&parsed.self_id).copied();
                out.push(TOCEntry {
                    level: entry_def.level.unwrap_or(1),
                    text,
                    page_number,
                    separator: entry_def
                        .separator
                        .clone()
                        .unwrap_or_else(|| "^t".to_string()),
                    format_style: entry_def.format_style.clone(),
                    include_style: applied.to_string(),
                    page_number_visible: !matches!(
                        entry_def.page_number.as_deref(),
                        Some("Off") | Some("NoPageNumber")
                    ),
                });
            }
        }
        out
    }

    /// Map story-id → body page-index of the chain head frame.
    ///
    /// Body page indices are 0-based and match the renderer's page
    /// ordering: spreads are concatenated in manifest order, pages
    /// inside each spread land in document order. The map is keyed
    /// off the *parent* story (not threaded targets), so a chain
    /// hosted across multiple frames resolves to its head frame's
    /// page. Stories with no hosting frame are absent.
    fn body_page_index_map(&self) -> HashMap<String, usize> {
        let mut out: HashMap<String, usize> = HashMap::new();
        // Pre-compute per-spread page index offsets so we can map
        // (spread_idx, local_page_idx) → body page index in O(1).
        let mut spread_page_offsets = Vec::with_capacity(self.spreads.len() + 1);
        let mut running = 0usize;
        for parsed in &self.spreads {
            spread_page_offsets.push(running);
            running += parsed.spread.pages.len().max(1);
        }
        spread_page_offsets.push(running);

        for parsed_story in &self.stories {
            let chain = self.frame_chain(&parsed_story.self_id);
            let Some(head) = chain.first() else { continue };
            let Some(self_id) = head.self_id.as_deref() else {
                continue;
            };
            let Some(&(spread_idx, _frame_idx)) = self.text_frame_index.get(self_id) else {
                continue;
            };
            let spread = match self.spreads.get(spread_idx) {
                Some(s) => &s.spread,
                None => continue,
            };
            let local_page =
                page_index_for_bounds(&spread.pages, head.bounds, head.item_transform).unwrap_or(0);
            out.insert(
                parsed_story.self_id.clone(),
                spread_page_offsets[spread_idx] + local_page,
            );
        }
        out
    }
}

/// Find the page in a spread that contains a frame's centroid. Uses
/// the frame's `GeometricBounds` after applying its `ItemTransform`
/// (since bounds are stored in the frame's inner coords). Returns
/// `None` if no page contains the centroid — caller defaults to the
/// first page.
fn page_index_for_bounds(
    pages: &[paged_parse::Page],
    bounds: Bounds,
    item_transform: Option<[f32; 6]>,
) -> Option<usize> {
    let cx = (bounds.left + bounds.right) * 0.5;
    let cy = (bounds.top + bounds.bottom) * 0.5;
    let (cx, cy) = match item_transform {
        Some([a, b, c, d, tx, ty]) => (a * cx + c * cy + tx, b * cx + d * cy + ty),
        None => (cx, cy),
    };
    pages.iter().position(|p| {
        let (left, right, top, bottom) = match p.item_transform {
            // Real IDML page ItemTransforms are pure translation
            // (InDesign limits the field to dx/dy plus 0/90/180/270
            // rotation); rotation is rare for body pages so we treat
            // the matrix as translation-only for containment.
            Some([_, _, _, _, tx, ty]) => (
                p.bounds.left + tx,
                p.bounds.right + tx,
                p.bounds.top + ty,
                p.bounds.bottom + ty,
            ),
            None => (p.bounds.left, p.bounds.right, p.bounds.top, p.bounds.bottom),
        };
        cx >= left && cx <= right && cy >= top && cy <= bottom
    })
}

/// Concatenate a paragraph's `runs` text into a plain string,
/// dropping the IDML auto-page-number / next-page-number sentinel
/// characters (they'd appear as private-use code-points in the TOC
/// output otherwise).
fn paragraph_plain_text(p: &Paragraph) -> String {
    let mut buf = String::new();
    for run in &p.runs {
        for ch in run.text.chars() {
            if ch == paged_parse::AUTO_PAGE_NUMBER_MARKER
                || ch == paged_parse::NEXT_PAGE_NUMBER_MARKER
            {
                continue;
            }
            buf.push(ch);
        }
    }
    buf.trim().to_string()
}

/// One resolved row of a Table of Contents.
///
/// The renderer composes one paragraph per entry, applying the
/// referenced `format_style`, the entry's `text` followed by
/// `separator`, then the formatted page number (when
/// `page_number_visible`).
#[derive(Debug, Clone, PartialEq)]
pub struct TOCEntry {
    /// Outline depth, 1-based. Top-level entries are level 1.
    pub level: u32,
    /// The trimmed text of the source paragraph.
    pub text: String,
    /// Body page-index of the paragraph's host frame head, or `None`
    /// for orphan stories (no hosting frame).
    pub page_number: Option<usize>,
    /// Separator string between `text` and the page number. IDML
    /// serialises tabs as `^t`; the renderer expands them at use
    /// time. Default is `"^t"`.
    pub separator: String,
    /// `FormatStyle` reference from the matching `<TOCStyleEntry>`,
    /// or `None` when the entry didn't declare one.
    pub format_style: Option<String>,
    /// The `IncludeStyle` reference this entry matched against — kept
    /// for debugging / book-style cross-referencing.
    pub include_style: String,
    /// When `false`, the IDML entry asked to suppress the page
    /// number (`PageNumber="Off"` / `NoPageNumber"`). The renderer
    /// should drop the separator + number for these rows.
    pub page_number_visible: bool,
}

/// Phase 5 — one resolved row of a generated index.
///
/// Each entry represents a single topic with the (body) page-index
/// list of every paragraph that anchors a marker for that topic.
/// The renderer composes one paragraph per entry, applying the
/// referenced index style; the page list renders as a
/// comma-separated string (`"12, 23, 41"`) appended after the topic.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    /// The indexed term, in its display form (preserving the IDML's
    /// original casing). Comparison + grouping happen on the
    /// lowercase form via `sort_key`.
    pub topic: String,
    /// Sort key — either the explicit `SortOrder` IDML attribute
    /// from the first marker for this topic, or `topic.to_lowercase()`
    /// when none was declared.
    pub sort_key: String,
    /// Body-page indices of every paragraph that anchors a marker
    /// for this topic. Deduplicated + ascending. May be empty when
    /// every host story is orphan (no on-page frames).
    pub pages: Vec<usize>,
}

impl ResolvedRunAttrs {
    /// Capture a run's directly-set fields into a fresh
    /// `ResolvedRunAttrs`. Style-cascade fallbacks apply via
    /// `merge_below_character` / `merge_below_paragraph`.
    pub fn from_run(run: &CharacterRun) -> Self {
        Self {
            font: run.font.clone(),
            font_style: run.font_style.clone(),
            point_size: run.point_size,
            fill_color: run.fill_color.clone(),
            fill_tint: run.fill_tint,
            stroke_color: run.stroke_color.clone(),
            stroke_weight: run.stroke_weight,
            capitalization: run.capitalization.clone(),
            baseline_shift: run.baseline_shift,
            horizontal_scale: run.horizontal_scale,
            vertical_scale: run.vertical_scale,
            skew: run.skew,
            position: run.position.clone(),
            tracking: run.tracking,
            underline: run.underline,
            strikethru: run.strikethru,
            leading: run.leading,
            ruby_flag: run.ruby_flag,
            ruby_type: run.ruby_type.clone(),
            ruby_string: run.ruby_string.clone(),
            kenten_kind: run.kenten_kind.clone(),
            kenten_character: run.kenten_character.clone(),
            kenten_font_size: run.kenten_font_size,
            overprint_fill: run.overprint_fill,
            overprint_stroke: run.overprint_stroke,
            ligatures_on: run.ligatures_on,
            kerning_method: run.kerning_method.clone(),
            otf: run.otf.clone(),
        }
    }

    /// Fill any unset field from a resolved character style.
    pub fn merge_below_character(&mut self, c: &paged_parse::ResolvedCharacter) {
        if self.font.is_none() {
            self.font = c.font.clone();
        }
        if self.font_style.is_none() {
            self.font_style = c.font_style.clone();
        }
        self.point_size = self.point_size.or(c.point_size);
        if self.fill_color.is_none() {
            self.fill_color = c.fill_color.clone();
        }
        self.fill_tint = self.fill_tint.or(c.fill_tint);
        if self.stroke_color.is_none() {
            self.stroke_color = c.stroke_color.clone();
        }
        self.stroke_weight = self.stroke_weight.or(c.stroke_weight);
        if self.capitalization.is_none() {
            self.capitalization = c.capitalization.clone();
        }
        self.baseline_shift = self.baseline_shift.or(c.baseline_shift);
        self.horizontal_scale = self.horizontal_scale.or(c.horizontal_scale);
        self.vertical_scale = self.vertical_scale.or(c.vertical_scale);
        self.skew = self.skew.or(c.skew);
        if self.position.is_none() {
            self.position = c.position.clone();
        }
        self.tracking = self.tracking.or(c.tracking);
        self.underline = self.underline.or(c.underline);
        self.strikethru = self.strikethru.or(c.strikethru);
        self.ruby_flag = self.ruby_flag.or(c.ruby_flag);
        if self.ruby_type.is_none() {
            self.ruby_type = c.ruby_type.clone();
        }
        if self.ruby_string.is_none() {
            self.ruby_string = c.ruby_string.clone();
        }
        if self.kenten_kind.is_none() {
            self.kenten_kind = c.kenten_kind.clone();
        }
        if self.kenten_character.is_none() {
            self.kenten_character = c.kenten_character.clone();
        }
        self.kenten_font_size = self.kenten_font_size.or(c.kenten_font_size);
        self.overprint_fill = self.overprint_fill.or(c.overprint_fill);
        self.overprint_stroke = self.overprint_stroke.or(c.overprint_stroke);
        self.ligatures_on = self.ligatures_on.or(c.ligatures_on);
        if self.kerning_method.is_none() {
            self.kerning_method = c.kerning_method.clone();
        }
        // Discrete OTF toggles cascade per-field: any flag still unset
        // on the run inherits the character style's value.
        self.otf.merge_below(&c.otf);
    }

    /// Fill any unset field from a resolved paragraph style.
    /// Run-level can pull font / size / fill out of paragraph
    /// styles but not the paragraph-only knobs.
    pub fn merge_below_paragraph(&mut self, p: &paged_parse::ResolvedParagraph) {
        if self.font.is_none() {
            self.font = p.font.clone();
        }
        if self.font_style.is_none() {
            self.font_style = p.font_style.clone();
        }
        self.point_size = self.point_size.or(p.point_size);
        if self.fill_color.is_none() {
            self.fill_color = p.fill_color.clone();
        }
        self.fill_tint = self.fill_tint.or(p.fill_tint);
        if self.stroke_color.is_none() {
            self.stroke_color = p.stroke_color.clone();
        }
        self.stroke_weight = self.stroke_weight.or(p.stroke_weight);
        if self.capitalization.is_none() {
            self.capitalization = p.capitalization.clone();
        }
        self.baseline_shift = self.baseline_shift.or(p.baseline_shift);
        self.horizontal_scale = self.horizontal_scale.or(p.horizontal_scale);
        self.vertical_scale = self.vertical_scale.or(p.vertical_scale);
        self.skew = self.skew.or(p.skew);
        if self.position.is_none() {
            self.position = p.position.clone();
        }
        self.tracking = self.tracking.or(p.tracking);
        self.underline = self.underline.or(p.underline);
        self.strikethru = self.strikethru.or(p.strikethru);
        self.overprint_fill = self.overprint_fill.or(p.overprint_fill);
        self.overprint_stroke = self.overprint_stroke.or(p.overprint_stroke);
    }
}

impl ResolvedParagraphAttrs {
    /// Capture a paragraph's directly-set fields. Style cascade
    /// fallbacks apply via `merge_below`. Local override values
    /// (BulletsAndNumberingListType / BulletChar) carried directly on
    /// the ParagraphStyleRange win over the cascaded paragraph style.
    pub fn from_paragraph(paragraph: &Paragraph) -> Self {
        Self {
            justification: paragraph.justification,
            first_line_indent: paragraph.first_line_indent,
            // FINDING #7.2 — instance margin indents win over the style
            // cascade (filled by `merge_below` only when unset here).
            left_indent: paragraph.left_indent,
            right_indent: paragraph.right_indent,
            space_before: paragraph.space_before,
            space_after: paragraph.space_after,
            tab_list: paragraph.tab_list.clone(),
            bullets_list_type: paragraph.bullets_list_type.clone(),
            bullet_character: paragraph.bullet_character,
            bullets_text_after: None,
            numbering_format: None,
            bullets_character_style: None,
            bullets_and_numbering_digits_character_style: None,
            numbering_expression: None,
            numbering_start_at: None,
            numbering_continue: None,
            // W1.22 — instance override wins over the cascade.
            applied_numbering_list: paragraph.applied_numbering_list.clone(),
            // styles.next-style is style-level only (no inline form).
            next_style: None,
            hyphenation: None,
            hyphenation_zone: None,
            applied_language: None,
            minimum_word_spacing: None,
            desired_word_spacing: None,
            maximum_word_spacing: None,
            minimum_letter_spacing: None,
            desired_letter_spacing: None,
            maximum_letter_spacing: None,
            minimum_glyph_scaling: None,
            desired_glyph_scaling: None,
            maximum_glyph_scaling: None,
            drop_cap_characters: None,
            drop_cap_lines: None,
            drop_cap_detail: None,
            kinsoku_set: paragraph.kinsoku_set.clone(),
            kinsoku_type: paragraph.kinsoku_type.clone(),
            mojikumi_table: paragraph.mojikumi_table.clone(),
            mojikumi_set: paragraph.mojikumi_set.clone(),
            overprint_fill: paragraph.overprint_fill,
            overprint_stroke: paragraph.overprint_stroke,
            // Q-09: paragraph carries no per-paragraph shading
            // override today; the cascade pulls everything from the
            // applied paragraph style.
            shading: Default::default(),
            // FINDING #7.3 — capture the paragraph's INSTANCE rule
            // structs (set directly on the ParagraphStyleRange, e.g. by
            // the `ParagraphRuleAbove` / `Below` mutation). `merge_below`
            // then fills any unset per-field value from the style
            // cascade. Pre-fix these were `Default::default()`, so an
            // instance-only rule (no style rule) painted nothing.
            rule_above: paragraph.rule_above.clone(),
            rule_below: paragraph.rule_below.clone(),
            border: Default::default(),
            // Nested styles are not declared inline on a
            // ParagraphStyleRange in normal IDML; `merge_below` pulls
            // them from the applied ParagraphStyle.
            nested_styles: Vec::new(),
        }
    }

    /// Fill any unset field from a resolved paragraph style.
    pub fn merge_below(&mut self, p: &paged_parse::ResolvedParagraph) {
        self.justification = self.justification.or(p.justification);
        self.first_line_indent = self.first_line_indent.or(p.first_line_indent);
        // FINDING #7.2 — fall back to the cascaded style indents.
        self.left_indent = self.left_indent.or(p.left_indent);
        self.right_indent = self.right_indent.or(p.right_indent);
        self.space_before = self.space_before.or(p.space_before);
        self.space_after = self.space_after.or(p.space_after);
        if self.tab_list.is_empty() && !p.tab_list.is_empty() {
            self.tab_list = p.tab_list.clone();
        }
        if self.bullets_list_type.is_none() {
            self.bullets_list_type = p.bullets_list_type.clone();
        }
        self.bullet_character = self.bullet_character.or(p.bullet_character);
        if self.bullets_text_after.is_none() {
            self.bullets_text_after = p.bullets_text_after.clone();
        }
        if self.numbering_format.is_none() {
            self.numbering_format = p.numbering_format.clone();
        }
        if self.bullets_character_style.is_none() {
            self.bullets_character_style = p.bullets_character_style.clone();
        }
        if self.bullets_and_numbering_digits_character_style.is_none() {
            self.bullets_and_numbering_digits_character_style =
                p.bullets_and_numbering_digits_character_style.clone();
        }
        if self.numbering_expression.is_none() {
            self.numbering_expression = p.numbering_expression.clone();
        }
        self.numbering_start_at = self.numbering_start_at.or(p.numbering_start_at);
        self.numbering_continue = self.numbering_continue.or(p.numbering_continue);
        if self.applied_numbering_list.is_none() {
            self.applied_numbering_list = p.applied_numbering_list.clone();
        }
        if self.next_style.is_none() {
            self.next_style = p.next_style.clone();
        }
        self.hyphenation = self.hyphenation.or(p.hyphenation);
        self.hyphenation_zone = self.hyphenation_zone.or(p.hyphenation_zone);
        if self.applied_language.is_none() {
            self.applied_language = p.applied_language.clone();
        }
        self.minimum_word_spacing = self.minimum_word_spacing.or(p.minimum_word_spacing);
        self.desired_word_spacing = self.desired_word_spacing.or(p.desired_word_spacing);
        self.maximum_word_spacing = self.maximum_word_spacing.or(p.maximum_word_spacing);
        // Q-20: letter / glyph spacing per-field inheritance.
        self.minimum_letter_spacing = self.minimum_letter_spacing.or(p.minimum_letter_spacing);
        self.desired_letter_spacing = self.desired_letter_spacing.or(p.desired_letter_spacing);
        self.maximum_letter_spacing = self.maximum_letter_spacing.or(p.maximum_letter_spacing);
        self.minimum_glyph_scaling = self.minimum_glyph_scaling.or(p.minimum_glyph_scaling);
        self.desired_glyph_scaling = self.desired_glyph_scaling.or(p.desired_glyph_scaling);
        self.maximum_glyph_scaling = self.maximum_glyph_scaling.or(p.maximum_glyph_scaling);
        self.drop_cap_characters = self.drop_cap_characters.or(p.drop_cap_characters);
        self.drop_cap_lines = self.drop_cap_lines.or(p.drop_cap_lines);
        self.drop_cap_detail = self.drop_cap_detail.or(p.drop_cap_detail);
        if self.kinsoku_set.is_none() {
            self.kinsoku_set = p.kinsoku_set.clone();
        }
        if self.kinsoku_type.is_none() {
            self.kinsoku_type = p.kinsoku_type.clone();
        }
        if self.mojikumi_table.is_none() {
            self.mojikumi_table = p.mojikumi_table.clone();
        }
        if self.mojikumi_set.is_none() {
            self.mojikumi_set = p.mojikumi_set.clone();
        }
        self.overprint_fill = self.overprint_fill.or(p.overprint_fill);
        self.overprint_stroke = self.overprint_stroke.or(p.overprint_stroke);
        // Q-09: paragraph-shading per-field inheritance.
        let s = &mut self.shading;
        let ps = &p.shading;
        s.on = s.on.or(ps.on);
        if s.color.is_none() {
            s.color = ps.color.clone();
        }
        s.tint = s.tint.or(ps.tint);
        if s.width.is_none() {
            s.width = ps.width.clone();
        }
        s.offset_top = s.offset_top.or(ps.offset_top);
        s.offset_left = s.offset_left.or(ps.offset_left);
        s.offset_bottom = s.offset_bottom.or(ps.offset_bottom);
        s.offset_right = s.offset_right.or(ps.offset_right);
        if s.top_origin.is_none() {
            s.top_origin = ps.top_origin.clone();
        }
        if s.bottom_origin.is_none() {
            s.bottom_origin = ps.bottom_origin.clone();
        }
        s.clip_to_frame = s.clip_to_frame.or(ps.clip_to_frame);
        s.overprint = s.overprint.or(ps.overprint);
        s.suppress_printing = s.suppress_printing.or(ps.suppress_printing);
        // Q-09: per-field rule_above / rule_below inheritance.
        merge_rule_attrs(&mut self.rule_above, &p.rule_above);
        merge_rule_attrs(&mut self.rule_below, &p.rule_below);
        // Q-09: per-field paragraph-border inheritance.
        merge_border_attrs(&mut self.border, &p.border);
        // Phase 4 — nested styles replace as a list when the lower
        // attrs have none of their own (mirrors styles.rs cascade).
        if self.nested_styles.is_empty() && !p.nested_styles.is_empty() {
            self.nested_styles = p.nested_styles.clone();
        }
    }
}

fn merge_rule_attrs(c: &mut paged_parse::ParagraphRule, p: &paged_parse::ParagraphRule) {
    c.on = c.on.or(p.on);
    if c.color.is_none() {
        c.color = p.color.clone();
    }
    c.tint = c.tint.or(p.tint);
    c.weight = c.weight.or(p.weight);
    c.offset = c.offset.or(p.offset);
    c.left_indent = c.left_indent.or(p.left_indent);
    c.right_indent = c.right_indent.or(p.right_indent);
    if c.width.is_none() {
        c.width = p.width.clone();
    }
}

fn merge_border_attrs(c: &mut paged_parse::ParagraphBorder, p: &paged_parse::ParagraphBorder) {
    c.on = c.on.or(p.on);
    if c.color.is_none() {
        c.color = p.color.clone();
    }
    c.tint = c.tint.or(p.tint);
    c.weight = c.weight.or(p.weight);
    c.offset_top = c.offset_top.or(p.offset_top);
    c.offset_left = c.offset_left.or(p.offset_left);
    c.offset_bottom = c.offset_bottom.or(p.offset_bottom);
    c.offset_right = c.offset_right.or(p.offset_right);
    if c.width.is_none() {
        c.width = p.width.clone();
    }
    for i in 0..4 {
        c.corners[i].option = c.corners[i].option.or(p.corners[i].option);
        c.corners[i].radius = c.corners[i].radius.or(p.corners[i].radius);
    }
}

/// Derive a Story's `Self` id from its manifest src. Turns
/// "Stories/Story_uXX.xml" → "uXX"; returns the stem otherwise.
fn hash_bounds(h: &mut blake3::Hasher, b: &paged_parse::Bounds) {
    h.update(&b.top.to_le_bytes());
    h.update(&b.left.to_le_bytes());
    h.update(&b.bottom.to_le_bytes());
    h.update(&b.right.to_le_bytes());
}

fn hash_transform(h: &mut blake3::Hasher, m: Option<&[f32; 6]>) {
    if let Some(arr) = m {
        h.update(b"\0t1\0");
        for v in arr {
            h.update(&v.to_le_bytes());
        }
    } else {
        h.update(b"\0t0\0");
    }
}

pub fn derive_story_id(src: &str) -> String {
    let stem = src.rsplit_once('/').map(|(_, t)| t).unwrap_or(src);
    let without_ext = stem.strip_suffix(".xml").unwrap_or(stem);
    without_ext
        .strip_prefix("Story_")
        .map(|s| s.to_string())
        .unwrap_or_else(|| without_ext.to_string())
}

/// Derive a MasterSpread's `Self` id from its manifest src. Turns
/// "MasterSpreads/MasterSpread_uad.xml" → "uad".
pub fn derive_master_id(src: &str) -> String {
    let stem = src.rsplit_once('/').map(|(_, t)| t).unwrap_or(src);
    let without_ext = stem.strip_suffix(".xml").unwrap_or(stem);
    without_ext
        .strip_prefix("MasterSpread_")
        .map(|s| s.to_string())
        .unwrap_or_else(|| without_ext.to_string())
}

/// Effective character-level attributes after walking the cascade
/// (direct > applied character style > applied paragraph style).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedRunAttrs {
    pub font: Option<String>,
    pub font_style: Option<String>,
    pub point_size: Option<f32>,
    pub fill_color: Option<String>,
    /// `FillTint` percentage (0..=100). `None` ⇒ use the swatch
    /// at full strength. The renderer scales the resolved RGB toward
    /// paper white by `(1 - tint/100)` when `Some`.
    pub fill_tint: Option<f32>,
    /// Cascaded `StrokeColor` for text outline. `None` ⇒ no outline
    /// (run renders fill-only). Resolves through the same direct >
    /// character-style > paragraph-style chain as every other field.
    /// See [`CharacterRun::stroke_color`].
    pub stroke_color: Option<String>,
    /// Cascaded `StrokeWeight` in pt. The renderer falls back to
    /// 1pt at emit time when `stroke_color` resolves but
    /// `stroke_weight` is None — matching IDML's `<TextDefault>`
    /// default for new documents.
    pub stroke_weight: Option<f32>,
    /// `Capitalization` value (Normal/AllCaps/SmallCaps/CapToSmallCap).
    /// Renderer uppercases the input string before shaping for
    /// `AllCaps` / `SmallCaps` (the latter without OT smcp lookup is
    /// just AllCaps until proper font-feature support arrives).
    pub capitalization: Option<String>,
    /// `BaselineShift` in pt. Applied as a per-glyph y-offset.
    pub baseline_shift: Option<f32>,
    /// `HorizontalScale` percentage (100 = identity). Folded into
    /// glyph x-advance + glyph affine at shape/emit time (P-08).
    pub horizontal_scale: Option<f32>,
    /// `VerticalScale` percentage (100 = identity). Parsed; not yet
    /// honoured.
    pub vertical_scale: Option<f32>,
    /// `Skew` in degrees (positive = right-leaning). Folded into the
    /// glyph affine alongside `HorizontalScale` (P-08).
    pub skew: Option<f32>,
    /// `Position` value (Normal / Superscript / Subscript / etc).
    /// Parsed; not yet honoured.
    pub position: Option<String>,
    pub tracking: Option<f32>,
    pub underline: Option<bool>,
    pub strikethru: Option<bool>,
    /// Explicit `Leading` in pt. `None` ⇒ Auto leading
    /// (`point_size × 1.2`).
    pub leading: Option<f32>,
    /// Cascaded `RubyFlag` — true when this run carries ruby
    /// annotation. Parser/scene-only today; renderer integration
    /// queued under Tier 4 — CJK Stage 4.
    pub ruby_flag: Option<bool>,
    /// Cascaded `RubyType` — `PerCharacter` / `GroupRuby`.
    pub ruby_type: Option<String>,
    /// Cascaded `RubyString` — the annotation text.
    pub ruby_string: Option<String>,
    /// Cascaded `KentenKind` — emphasis-mark glyph kind.
    pub kenten_kind: Option<String>,
    /// Cascaded `KentenCharacter` — custom emphasis mark codepoint
    /// when `kenten_kind == "Custom"`.
    pub kenten_character: Option<String>,
    /// Cascaded `KentenFontSize` — % of base size.
    pub kenten_font_size: Option<f32>,
    /// Cascaded `OverprintFill` flag — `true` means this run's fill
    /// should composite with darken (per-channel `min(top, bottom)`)
    /// instead of knocking out the underlying ink. The renderer's
    /// glyph emitter consumes this once run-level overprint is wired
    /// (frame-level overprint already flows through the display list).
    pub overprint_fill: Option<bool>,
    /// Cascaded `OverprintStroke` flag (rare on text — only outlined
    /// text strokes).
    pub overprint_stroke: Option<bool>,
    /// Phase 4 typography — cascaded `Ligatures` flag. `None` ⇒
    /// inherits / the default (true) wins at the bottom of the
    /// cascade.
    pub ligatures_on: Option<bool>,
    /// Cascaded `KerningMethod` string. `None` ⇒ default
    /// (`"Metrics"`) at the bottom of the cascade.
    pub kerning_method: Option<String>,
    /// Cascaded discrete OpenType feature toggles (`OTFFraction`,
    /// `OTFOrdinal`, `OTFSwash`, `OTFDiscretionaryLigature`,
    /// `OTFFigureStyle`, `OTFStylisticSets`, …). Resolves through the
    /// direct > character-style chain; IDML records these at the
    /// character level only, so there is no paragraph-style fallback.
    /// The renderer maps the resolved bag to rustybuzz feature tags via
    /// `paged_text::ShapingFeatures`.
    pub otf: paged_parse::OtfFeatures,
}

/// Effective paragraph-level attributes after walking the cascade
/// (direct > applied paragraph style).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedParagraphAttrs {
    pub justification: Option<paged_parse::Justification>,
    pub first_line_indent: Option<f32>,
    /// FINDING #7.2 — `LeftIndent` / `RightIndent` in pt. The
    /// paragraph's left/right margin offsets: the renderer narrows the
    /// composed column by `left + right` and shifts the body right by
    /// `left`. Instance values (set directly on the ParagraphStyleRange,
    /// e.g. via the `ParagraphLeftIndent` mutation) win over the style
    /// cascade. `None` ⇒ no indent.
    pub left_indent: Option<f32>,
    pub right_indent: Option<f32>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    pub tab_list: Vec<paged_parse::TabStop>,
    /// `BulletsAndNumberingListType` (BulletList / NumberedList /
    /// NoList). The renderer only consumes BulletList today.
    pub bullets_list_type: Option<String>,
    /// Unicode codepoint of the bullet glyph (from `<BulletChar>`).
    pub bullet_character: Option<u32>,
    /// Separator string rendered between the bullet and the
    /// paragraph text. IDML serialises tabs as `^t`; the
    /// renderer expands them to `\t` at use time.
    pub bullets_text_after: Option<String>,
    /// `NumberingFormat` sample string from the cascaded paragraph
    /// style. Used by the renderer's `format_number` helper to pick
    /// Arabic / Roman / alpha / zero-padded formatting for
    /// `NumberedList` paragraphs.
    pub numbering_format: Option<String>,
    /// `CharacterStyle/<id>` ref styling the bullet marker (font,
    /// size, colour) independently of the paragraph body. `None` ⇒
    /// inherit the first run's formatting (the historical fallback).
    /// IDML applies this only to `BulletList` paragraphs.
    pub bullets_character_style: Option<String>,
    /// `CharacterStyle/<id>` ref styling the digits of a
    /// `NumberedList` paragraph's marker. The InDesign UI surfaces a
    /// single "Character Style" picker per paragraph style regardless
    /// of list kind, so the renderer also treats this as a fallback
    /// bullet style when `bullets_character_style` is absent.
    pub bullets_and_numbering_digits_character_style: Option<String>,
    /// `NumberingExpression` template (`^#`, `^.`, `^t` tokens plus
    /// literal characters). `None` ⇒ renderer applies the IDML
    /// default `^#.^t`.
    pub numbering_expression: Option<String>,
    /// `NumberingStartAt` explicit integer override; the renderer
    /// resets the story-level counter to this value on paragraph
    /// entry. `None` ⇒ inherit (auto-increment off prior state).
    pub numbering_start_at: Option<i32>,
    /// `NumberingContinue` flag. `Some(true)` suppresses the
    /// auto-reset that fires when the bulleting kind changes
    /// across paragraphs; `Some(false)` forces a reset on entry.
    /// `None` ⇒ renderer default (continue).
    pub numbering_continue: Option<bool>,
    /// W1.22 — resolved `AppliedNumberingList` ref (instance override
    /// over the style cascade). `None` ⇒ no named list. The renderer
    /// reads the list def's `continue_across_stories` flag off this to
    /// decide cross-story numbering continuity.
    pub applied_numbering_list: Option<String>,
    /// styles.next-style — resolved `NextStyle` ref. Carried for the
    /// editor's typing-time flow; the renderer does not act on it.
    pub next_style: Option<String>,
    /// `Hyphenation` boolean from the cascaded paragraph style.
    /// Drives whether the composer wires up a hyphenator.
    pub hyphenation: Option<bool>,
    /// `HyphenationZone` in pt from the cascaded paragraph style.
    /// Suppresses hyphenation for words that would otherwise start
    /// within this distance of the right margin. `None`/`0` ⇒ no zone
    /// restriction. See [`paged_parse::ResolvedParagraph::hyphenation_zone`].
    pub hyphenation_zone: Option<f32>,
    /// `AppliedLanguage` from the cascade — feeds dictionary picking
    /// for hyphenation. Strings like `"$ID/English: USA"`.
    pub applied_language: Option<String>,
    /// `MinimumWordSpacing` (% of normal). 100 = baseline.
    pub minimum_word_spacing: Option<f32>,
    /// `DesiredWordSpacing` (% of normal). 100 = baseline.
    pub desired_word_spacing: Option<f32>,
    /// `MaximumWordSpacing` (% of normal). 100 = baseline.
    pub maximum_word_spacing: Option<f32>,
    /// Q-20: cascaded `MinimumLetterSpacing` pt. Additive signed
    /// adjustment to the inter-glyph advance budget. None ⇒ 0 pt.
    pub minimum_letter_spacing: Option<f32>,
    pub desired_letter_spacing: Option<f32>,
    pub maximum_letter_spacing: Option<f32>,
    /// Q-20: cascaded `MinimumGlyphScaling` percent. 100 ⇒ identity.
    /// Allows the breaker to scale per-glyph x-advance for justification.
    pub minimum_glyph_scaling: Option<f32>,
    pub desired_glyph_scaling: Option<f32>,
    pub maximum_glyph_scaling: Option<f32>,
    /// `DropCapCharacters` from the cascaded paragraph style.
    /// Count of leading characters that drop down across
    /// `drop_cap_lines` lines. 0 / `None` ⇒ no drop cap.
    pub drop_cap_characters: Option<u32>,
    /// `DropCapLines` — vertical extent of the drop cap.
    pub drop_cap_lines: Option<u32>,
    /// `DropCapDetail` — InDesign's scaling integer.
    pub drop_cap_detail: Option<i32>,
    /// Cascaded `KinsokuSet` ref. Identifies the CJK line-break
    /// character set this paragraph follows. Parser/scene captures
    /// the ref; the composer uses a built-in "Hard Kinsoku" set when
    /// `kinsoku_type` triggers enforcement. See docs/plan.md Tier 4
    /// — CJK Stage 2.
    pub kinsoku_set: Option<String>,
    /// Cascaded `KinsokuType` flavour
    /// (`WordbreakWithJustification` / `PushIn` / `PushOut`). The
    /// composer keys "any value present" → "apply hard-kinsoku
    /// penalty" today.
    pub kinsoku_type: Option<String>,
    /// Cascaded `MojikumiTable` ref. Parser/scene-only; the
    /// renderer does not yet implement Mojikumi spacing
    /// adjustments. See docs/plan.md Tier 4 — CJK Stage 4.
    pub mojikumi_table: Option<String>,
    /// Cascaded `MojikumiSet` ref (older IDML attribute name).
    pub mojikumi_set: Option<String>,
    /// Cascaded `OverprintFill` flag from the paragraph cascade. Stage
    /// 3 frame-level overprint flows through `ResolvedFrame`; this
    /// surface lets the future glyph-level overprint path consume the
    /// paragraph cascade.
    pub overprint_fill: Option<bool>,
    /// Cascaded `OverprintStroke` flag.
    pub overprint_stroke: Option<bool>,
    /// Q-09: cascaded paragraph-shading parameters.
    pub shading: paged_parse::ParagraphShading,
    /// Q-09: cascaded horizontal rule above the first line.
    pub rule_above: paged_parse::ParagraphRule,
    /// Q-09: cascaded horizontal rule below the last line.
    pub rule_below: paged_parse::ParagraphRule,
    /// Q-09: cascaded rectangular paragraph border.
    pub border: paged_parse::ParagraphBorder,
    /// Phase 4 typography — cascaded `<NestedStyle>` entries from the
    /// applied paragraph style. The renderer walks the paragraph
    /// text against this list to override the character style on
    /// leading byte ranges.
    pub nested_styles: Vec<paged_parse::NestedStyle>,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("manifest lists {0} but archive has no such entry")]
    MissingEntry(String),
    #[error("parse: {0}")]
    Parse(#[from] ParseError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use paged_parse::{TOCStyleDef, TOCStyleEntryDef};
    use std::io::Write;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    #[test]
    fn story_id_strips_dir_and_prefix() {
        assert_eq!(derive_story_id("Stories/Story_u10.xml"), "u10");
        assert_eq!(derive_story_id("u10.xml"), "u10");
        assert_eq!(derive_story_id("Stories/custom_u10.xml"), "custom_u10");
    }

    // ---- FlowId / region-chain seam (composition-format §5) -------------

    fn bounds(top: f32, left: f32, bottom: f32, right: f32) -> Bounds {
        Bounds {
            top,
            left,
            bottom,
            right,
        }
    }

    #[test]
    fn content_box_subtracts_h_insets_from_width_keeps_full_height() {
        // 300×200 bounds, insets [top6 left8 bottom10 right12] →
        // width 300-8-12=280 (line-break width); height stays the FULL 200
        // (the emitter's overflow reference — top/bottom insets do not reduce
        // the flow height; footnote reservation is layered by the emitter).
        let g = content_box_geometry(
            bounds(0.0, 0.0, 200.0, 300.0),
            Some([6.0, 8.0, 10.0, 12.0]),
            None,
            None,
        );
        assert_eq!(g.width_pt, 280.0);
        assert_eq!(g.height_pt, 200.0);
        // Defaults: one column, 12pt IDML gutter.
        assert_eq!(g.columns, 1);
        assert_eq!(g.column_gap_pt, DEFAULT_COLUMN_GUTTER_PT);
    }

    #[test]
    fn content_box_no_insets_is_full_bounds() {
        let g = content_box_geometry(bounds(0.0, 0.0, 200.0, 300.0), None, Some(2), Some(10.0));
        assert_eq!(g.width_pt, 300.0);
        assert_eq!(g.height_pt, 200.0);
        assert_eq!(g.columns, 2);
        assert_eq!(g.column_gap_pt, 10.0);
    }

    #[test]
    fn content_box_clamps_degenerate_width_and_floors_columns() {
        // Horizontal insets larger than the box must not yield a negative
        // width. Height is the full bounds height (10), unaffected by insets.
        let g = content_box_geometry(
            bounds(0.0, 0.0, 10.0, 10.0),
            Some([50.0, 50.0, 50.0, 50.0]),
            Some(0),
            None,
        );
        assert_eq!(g.width_pt, 0.0);
        assert_eq!(g.height_pt, 10.0);
        // A declared 0 columns is floored to 1.
        assert_eq!(g.columns, 1);
    }

    /// Two frames threaded into one story (`frameA` → `frameB`), plus a
    /// third frame on a *different* story that must not leak into the chain.
    /// `frameA` carries insets + columns so the region geometry is exercised.
    fn pack_threaded_idml() -> Vec<u8> {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();

        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  <idPkg:Story src="Stories/Story_u10.xml"/>
  <idPkg:Story src="Stories/Story_u20.xml"/>
</Document>"#,
        )
        .unwrap();

        zip.start_file("Resources/Graphic.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Graphic xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Graphic/>
</idPkg:Graphic>"#,
        )
        .unwrap();

        // frameA (head, insets + 2 columns) → frameB (tail). frameC is on a
        // separate story (u20). GeometricBounds is `top left bottom right`.
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    <Page Self="p1" GeometricBounds="0 0 800 1200"/>
    <TextFrame Self="frameA" ParentStory="u10" NextTextFrame="frameB" GeometricBounds="0 0 200 300" StrokeWeight="0">
      <Properties/>
      <TextFramePreference InsetSpacing="6 8 10 12" TextColumnCount="2" TextColumnGutter="10"/>
    </TextFrame>
    <TextFrame Self="frameB" ParentStory="u10" GeometricBounds="0 320 200 620" StrokeWeight="0"/>
    <TextFrame Self="frameC" ParentStory="u20" GeometricBounds="0 640 200 940" StrokeWeight="0"/>
  </Spread>
</idPkg:Spread>"#,
        )
        .unwrap();

        for (sid, text) in [("u10", "Threaded story."), ("u20", "Other story.")] {
            let story = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="{sid}">
    <ParagraphStyleRange>
      <CharacterStyleRange><Content>{text}</Content></CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
            );
            zip.start_file(format!("Stories/Story_{sid}.xml"), deflated)
                .unwrap();
            zip.write_all(story.as_bytes()).unwrap();
        }

        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn flow_chain_projects_the_frame_chain() {
        let doc = Document::open(&pack_threaded_idml()).unwrap();

        // The flow chain is a faithful projection of the frame chain:
        // same order, ids = frame Self ids, flow id = story id.
        let frames = doc.frame_chain("u10");
        let flow = doc.flow_chain("u10");
        assert_eq!(flow.flow, paged_flow::FlowId::new("u10"));
        assert_eq!(flow.len(), frames.len());
        assert_eq!(flow.len(), 2);
        let ids: Vec<&str> = flow.regions.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["frameA", "frameB"]);

        // Region 0 (frameA): width = 300 minus h-insets 8+12 = 280; height =
        // the full bounds 200 (the emitter's overflow reference), with the
        // declared 2 columns / 10pt gutter.
        let r0 = &flow.regions[0].geometry;
        assert_eq!((r0.width_pt, r0.height_pt), (280.0, 200.0));
        assert_eq!(r0.columns, 2);
        assert_eq!(r0.column_gap_pt, 10.0);

        // Region 1 (frameB) has no insets → full 300×200, default 1 column.
        let r1 = &flow.regions[1].geometry;
        assert_eq!((r1.width_pt, r1.height_pt), (300.0, 200.0));
        assert_eq!(r1.columns, 1);
    }

    #[test]
    fn flow_chain_for_unknown_story_is_empty() {
        let doc = Document::open(&pack_threaded_idml()).unwrap();
        let flow = doc.flow_chain("does-not-exist");
        assert_eq!(flow.flow, paged_flow::FlowId::new("does-not-exist"));
        assert!(flow.is_empty());
    }

    #[test]
    fn to_composition_preserves_the_flow_arrangement() {
        let doc = Document::open(&pack_threaded_idml()).unwrap();
        let comp = doc.to_composition();

        // One print surface + the single page.
        assert_eq!(comp.surfaces.len(), 1);
        assert_eq!(comp.surfaces[0].kind, paged_composition::SurfaceKind::Print);
        assert_eq!(comp.pages.len(), 1);
        assert_eq!(comp.pages[0].id, "p1");

        // A flow per threaded story: u10 (frameA→frameB) and u20 (frameC).
        let flow_ids: Vec<&str> = comp.flows.iter().map(|f| f.id.as_str()).collect();
        assert!(flow_ids.contains(&"u10") && flow_ids.contains(&"u20"));

        // The native composition reproduces the IDML region-chain exactly —
        // the adapter preserves the flow arrangement.
        assert_eq!(
            comp.flow_chain(&paged_flow::FlowId::new("u10")),
            doc.flow_chain("u10")
        );
        let chain = comp.flow_chain(&paged_flow::FlowId::new("u10"));
        let ids: Vec<&str> = chain.regions.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["frameA", "frameB"]);

        // Regions carry the publishing bind + the flow tag.
        let region = comp
            .regions()
            .into_iter()
            .find(|r| r.id.as_str() == "frameA")
            .unwrap();
        assert_eq!(region.flow, Some(paged_flow::FlowId::new("u10")));
        assert!(matches!(&region.bind, paged_composition::Bind::Part { .. }));
    }

    /// Pack an IDML with three body pages on a single spread, three
    /// host frames (one per page, each with one story), where each
    /// paragraph carries an explicit `AppliedParagraphStyle` so the
    /// resolver can filter against it. The three frames are arranged
    /// vertically so per-page centroid containment lands each frame
    /// on a different page.
    fn pack_toc_idml(paragraphs: &[(&str, &str, &str)]) -> Vec<u8> {
        // paragraphs: (story_id, applied_style, text) — one per
        // paragraph; multiple entries with the same story_id stack
        // inside that story in order.
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();

        let mut story_ids: Vec<&str> = Vec::new();
        for (sid, _, _) in paragraphs {
            if !story_ids.contains(sid) {
                story_ids.push(sid);
            }
        }

        // designmap references all stories + the single spread.
        let mut designmap = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
"#,
        );
        for sid in &story_ids {
            designmap.push_str(&format!(
                "  <idPkg:Story src=\"Stories/Story_{sid}.xml\"/>\n"
            ));
        }
        designmap.push_str("</Document>");
        zip.start_file("designmap.xml", deflated).unwrap();
        zip.write_all(designmap.as_bytes()).unwrap();

        // One page per story, vertically stacked.
        let mut spread = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
"#,
        );
        for (i, _sid) in story_ids.iter().enumerate() {
            let top = (i as f32) * 100.0;
            let bottom = top + 100.0;
            spread.push_str(&format!(
                "    <Page Self=\"page{i}\" GeometricBounds=\"{top} 0 {bottom} 100\"/>\n"
            ));
        }
        for (i, sid) in story_ids.iter().enumerate() {
            let top = (i as f32) * 100.0;
            let bottom = top + 100.0;
            spread.push_str(&format!(
                "    <TextFrame Self=\"frame_{sid}\" ParentStory=\"{sid}\" GeometricBounds=\"{top} 0 {bottom} 100\"/>\n"
            ));
        }
        spread.push_str("  </Spread>\n</idPkg:Spread>");
        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(spread.as_bytes()).unwrap();

        for sid in &story_ids {
            let mut story = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story>
"#,
            );
            for (psid, style, text) in paragraphs {
                if psid != sid {
                    continue;
                }
                story.push_str(&format!(
                    "    <ParagraphStyleRange AppliedParagraphStyle=\"{style}\">
      <CharacterStyleRange><Content>{text}</Content></CharacterStyleRange>
    </ParagraphStyleRange>
"
                ));
            }
            story.push_str("  </Story>\n</idPkg:Story>");
            zip.start_file(format!("Stories/Story_{sid}.xml"), deflated)
                .unwrap();
            zip.write_all(story.as_bytes()).unwrap();
        }

        zip.finish().unwrap().into_inner()
    }

    fn toc_style_with_two_entries() -> TOCStyleDef {
        TOCStyleDef {
            self_id: "TOCStyle/Main".to_string(),
            name: Some("Main".to_string()),
            title: Some("Contents".to_string()),
            title_style: Some("ParagraphStyle/TocTitle".to_string()),
            include_book_documents: Some(false),
            include_hidden: Some(false),
            run_in: Some(false),
            entries: vec![
                TOCStyleEntryDef {
                    name: Some("H1".to_string()),
                    include_style: Some("ParagraphStyle/Heading1".to_string()),
                    format_style: Some("ParagraphStyle/TocFormat1".to_string()),
                    level: Some(1),
                    page_number: Some("On".to_string()),
                    separator: Some("^t".to_string()),
                },
                TOCStyleEntryDef {
                    name: Some("H2".to_string()),
                    include_style: Some("ParagraphStyle/Heading2".to_string()),
                    format_style: Some("ParagraphStyle/TocFormat2".to_string()),
                    level: Some(2),
                    page_number: Some("On".to_string()),
                    separator: Some(" -- ".to_string()),
                },
            ],
        }
    }

    #[test]
    fn resolve_toc_picks_paragraphs_in_document_order() {
        // Three stories on three pages. Story 'intro' (page 0) holds
        // H1 "Intro" + a Body paragraph (should be ignored) + H2
        // "Background". Story 'mid' (page 1) has H2 "Setup". Story
        // 'tail' (page 2) has H1 "Results".
        let bytes = pack_toc_idml(&[
            ("intro", "ParagraphStyle/Heading1", "Intro"),
            ("intro", "ParagraphStyle/Body", "Skip me"),
            ("intro", "ParagraphStyle/Heading2", "Background"),
            ("mid", "ParagraphStyle/Heading2", "Setup"),
            ("tail", "ParagraphStyle/Heading1", "Results"),
        ]);
        let doc = Document::open(&bytes).expect("open IDML");
        let toc = toc_style_with_two_entries();
        let entries = doc.resolve_toc(&toc);
        assert_eq!(entries.len(), 4, "{:?}", entries);
        assert_eq!(entries[0].text, "Intro");
        assert_eq!(entries[0].level, 1);
        assert_eq!(entries[0].page_number, Some(0));
        assert_eq!(
            entries[0].format_style.as_deref(),
            Some("ParagraphStyle/TocFormat1")
        );
        assert_eq!(entries[0].separator, "^t");

        assert_eq!(entries[1].text, "Background");
        assert_eq!(entries[1].level, 2);
        assert_eq!(entries[1].page_number, Some(0));
        assert_eq!(entries[1].separator, " -- ");

        assert_eq!(entries[2].text, "Setup");
        assert_eq!(entries[2].level, 2);
        assert_eq!(entries[2].page_number, Some(1));

        assert_eq!(entries[3].text, "Results");
        assert_eq!(entries[3].level, 1);
        assert_eq!(entries[3].page_number, Some(2));
    }

    #[test]
    fn resolve_toc_respects_page_number_off_flag() {
        let bytes = pack_toc_idml(&[("intro", "ParagraphStyle/Heading1", "Foreword")]);
        let doc = Document::open(&bytes).expect("open IDML");
        let mut toc = toc_style_with_two_entries();
        toc.entries[0].page_number = Some("NoPageNumber".to_string());
        let entries = doc.resolve_toc(&toc);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].page_number_visible);
    }

    #[test]
    fn resolve_toc_uses_default_separator_when_absent() {
        let bytes = pack_toc_idml(&[("intro", "ParagraphStyle/Heading1", "Foreword")]);
        let doc = Document::open(&bytes).expect("open IDML");
        let mut toc = toc_style_with_two_entries();
        toc.entries[0].separator = None;
        let entries = doc.resolve_toc(&toc);
        assert_eq!(entries[0].separator, "^t");
    }

    #[test]
    fn resolve_index_groups_by_topic_and_sorts() {
        // Build a small IDML: three body pages, each with a frame
        // hosting one story. Each story carries a single paragraph
        // with `<PageReference>` markers. The resolver should group
        // by topic, deduplicate pages, and return entries sorted
        // alphabetically by lowercase topic.
        let xml = pack_index_idml(&[
            ("apple-1", "Apple", 0),
            ("apple-2", "Apple", 1),
            ("banana", "Banana", 2),
            ("apple-3", "Apple", 0), // duplicate page → dedup
        ]);
        let doc = Document::open(&xml).expect("open IDML");
        let entries = doc.resolve_index();
        // Two topics; Apple sorts before Banana case-insensitively.
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].topic, "Apple");
        assert_eq!(entries[0].pages, vec![0, 1]); // 0 deduped
        assert_eq!(entries[1].topic, "Banana");
        assert_eq!(entries[1].pages, vec![2]);
    }

    /// Pack an IDML with a single spread, one story per (story_id,
    /// topic, body-page) triple. Pages are arranged vertically so
    /// each frame's centroid lands on a distinct page; the resolver's
    /// `body_page_index_map` then assigns each story to its own page.
    fn pack_index_idml(markers: &[(&str, &str, usize)]) -> Vec<u8> {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();

        // Pages are 200pt tall stacked top-to-bottom. Each frame
        // sits inside one page's bounds (10..190 vertical extent).
        let n_pages = markers.iter().map(|(_, _, p)| *p).max().unwrap_or(0) + 1;
        let mut pages_xml = String::new();
        for p in 0..n_pages {
            pages_xml.push_str(&format!(
                "<Page Self=\"p{p}\" GeometricBounds=\"{} 0 {} 200\"/>",
                p * 200,
                p * 200 + 200,
            ));
        }
        // Group stories by their host page so we get one frame per
        // page (whose centroid falls inside that page's bounds).
        let mut frames_xml = String::new();
        let mut by_page: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, m) in markers.iter().enumerate() {
            by_page.entry(m.2).or_default().push(i);
        }
        // One frame per (story_id, page) tuple. We use synthetic
        // story ids to avoid collisions.
        for (i, (sid, _topic, page)) in markers.iter().enumerate() {
            let top = page * 200 + 20;
            let bottom = page * 200 + 180;
            frames_xml.push_str(&format!(
                "<TextFrame Self=\"frame-{i}\" ParentStory=\"{sid}\" \
                 GeometricBounds=\"{top} 10 {bottom} 190\"/>",
            ));
        }

        zip.start_file("designmap.xml", deflated).unwrap();
        // Each marker becomes its own Story file.
        let mut story_refs = String::new();
        for (sid, _topic, _page) in markers {
            story_refs.push_str(&format!("<idPkg:Story src=\"Stories/Story_{sid}.xml\"/>"));
        }
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <idPkg:Spread src="Spreads/Spread_sp1.xml"/>
  {story_refs}
</Document>"#
            )
            .as_bytes(),
        )
        .unwrap();

        zip.start_file("Spreads/Spread_sp1.xml", deflated).unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Spread Self="sp1">
    {pages_xml}
    {frames_xml}
  </Spread>
</idPkg:Spread>"#
            )
            .as_bytes(),
        )
        .unwrap();

        for (sid, topic, _page) in markers {
            zip.start_file(format!("Stories/Story_{sid}.xml"), deflated)
                .unwrap();
            zip.write_all(
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Story xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging">
  <Story Self="{sid}">
    <ParagraphStyleRange>
      <CharacterStyleRange>
        <Content>body for {topic}</Content>
        <PageReference Self="PR-{sid}" TopicName="{topic}"/>
      </CharacterStyleRange>
    </ParagraphStyleRange>
  </Story>
</idPkg:Story>"#
                )
                .as_bytes(),
            )
            .unwrap();
        }

        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn character_style_fill_color_wins_over_paragraph_style() {
        use paged_parse::{CharacterStyleDef, ParagraphStyleDef, StyleSheet};

        let mut styles = StyleSheet::default();
        styles.paragraph_styles.insert(
            "ParagraphStyle/Body".to_string(),
            ParagraphStyleDef {
                self_id: "ParagraphStyle/Body".to_string(),
                fill_color: Some("Color/Black".to_string()),
                ..Default::default()
            },
        );
        styles.character_styles.insert(
            "CharacterStyle/Inverse".to_string(),
            CharacterStyleDef {
                self_id: "CharacterStyle/Inverse".to_string(),
                fill_color: Some("Color/Paper".to_string()),
                ..Default::default()
            },
        );

        let paragraph = Paragraph {
            paragraph_style: Some("ParagraphStyle/Body".to_string()),
            ..Default::default()
        };
        let run = CharacterRun {
            character_style: Some("CharacterStyle/Inverse".to_string()),
            ..Default::default()
        };

        let mut acc = ResolvedRunAttrs::from_run(&run);
        if let Some(id) = run.character_style.as_deref() {
            acc.merge_below_character(&styles.resolve_character(id));
        }
        if let Some(id) = paragraph.paragraph_style.as_deref() {
            acc.merge_below_paragraph(&styles.resolve_paragraph(id));
        }

        assert_eq!(acc.fill_color.as_deref(), Some("Color/Paper"));
    }
}
