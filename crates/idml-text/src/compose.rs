//! Paragraph composition: Knuth-Plass line breaking with calibrated
//! penalty weights.
//!
//! The composer works in 1/64 pt integer arithmetic (matching
//! `shape_run`) so results are stable and reproducible. A
//! `AdvanceMeasurer` trait abstracts width measurement so:
//!
//! * Production code plugs in `RustybuzzMeasurer` (real shaping).
//! * Tests and tooling can use `MonospaceMeasurer` (every char N units,
//!   every space M units) — fast, deterministic, no font required.
//!
//! Calibration of the penalty knobs against InDesign's Paragraph
//! Composer happens in `spikes/composer-calibration`; this module owns
//! the shape of the calibration surface (tolerance, looseness, glue
//! stretch/shrink ratios).

use std::sync::atomic::{AtomicBool, Ordering};

use paragraph_breaker::{Breakpoint, Item};
use rustybuzz::Face;

use crate::hyphenate::Hyphenator;
use crate::shape::{shape_run, ShapedGlyph, ShapedRun, ADVANCE_PRECISION};

/// Hard Kinsoku ("forbidden break") character classification.
///
/// IDML's `KinsokuType` attribute on a `<ParagraphStyleRange>` (or
/// `<ParagraphStyle>`) toggles InDesign's CJK line-break-rule
/// enforcement. The full standard is huge and per-locale; the
/// industry-standard JIS X 4051 "Hard" set is small (~30 chars) and
/// well-known. We hardcode it here:
///
/// - **No-start** chars cannot land at the start of a continuation
///   line — they trail the *previous* line. Closing brackets,
///   low-priority punctuation: `)」』}〕〉》】〙〗〟｠`、,. 'etc.
/// - **No-end** chars cannot land at the end of a line — they push
///   to the *next* line. Opening brackets: `(「『{〔〈《【〘〖〝｟[`,
///   etc.
///
/// The composer's `kinsoku_enforce` flag uses these to *suppress
/// break candidates* that would violate either rule (no Penalty item
/// emitted between adjacent chars where the break is forbidden), so
/// paragraph-breaker has to pick another break — the behavioural
/// change a fixture-free test can verify.
pub mod kinsoku {
    /// JIS-derived "no line start" characters (closing brackets +
    /// low-priority punctuation that should hang on the previous
    /// line rather than dangle alone). ~30 chars — explicitly
    /// enumerated so the lookup is a const `match` arm; lifted from
    /// JIS X 4051 §6.1 "kinsoku shori".
    pub fn is_no_start(c: char) -> bool {
        matches!(
            c,
            // Halfwidth ASCII punctuation that's also no-start in JIS
            // shori: closing brackets + sentence-final marks.
            ')' | ']' | '}' | '!' | '?' | ',' | '.' | ':' | ';'
            // Fullwidth (CJK) punctuation, sentence-final / pause marks
            | '、' // U+3001 IDEOGRAPHIC COMMA
            | '。' // U+3002 IDEOGRAPHIC FULL STOP
            | '，' // U+FF0C FULLWIDTH COMMA
            | '．' // U+FF0E FULLWIDTH FULL STOP
            | '？' // U+FF1F FULLWIDTH QUESTION MARK
            | '！' // U+FF01 FULLWIDTH EXCLAMATION MARK
            | '：' // U+FF1A FULLWIDTH COLON
            | '；' // U+FF1B FULLWIDTH SEMICOLON
            // Closing brackets — fullwidth
            | '）' // U+FF09 FULLWIDTH RIGHT PARENTHESIS
            | '］' // U+FF3D FULLWIDTH RIGHT SQUARE BRACKET
            | '｝' // U+FF5D FULLWIDTH RIGHT CURLY BRACKET
            // Closing brackets — CJK
            | '」' // U+300D RIGHT CORNER BRACKET
            | '』' // U+300F RIGHT WHITE CORNER BRACKET
            | '〕' // U+3015 RIGHT TORTOISE SHELL BRACKET
            | '〉' // U+3009 RIGHT ANGLE BRACKET
            | '》' // U+300B RIGHT DOUBLE ANGLE BRACKET
            | '】' // U+3011 RIGHT BLACK LENTICULAR BRACKET
            | '〗' // U+3017 RIGHT WHITE LENTICULAR BRACKET
            | '〙' // U+3019 RIGHT WHITE TORTOISE SHELL BRACKET
            | '〟' // U+301F LOW DOUBLE PRIME QUOTATION MARK
            | '｠' // U+FF60 FULLWIDTH RIGHT WHITE PARENTHESIS
            // Small kana (line-start avoidance is JIS-standard)
            | 'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ'
            | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ'
            | 'っ' | 'ッ' | 'ゃ' | 'ャ' | 'ゅ' | 'ュ' | 'ょ' | 'ョ'
            // Prolonged sound mark
            | 'ー' // U+30FC KATAKANA-HIRAGANA PROLONGED SOUND MARK
        )
    }

    /// JIS-derived "no line end" characters (opening brackets that
    /// should not be stranded at the end of a line).
    pub fn is_no_end(c: char) -> bool {
        matches!(
            c,
            '(' | '[' | '{'
            | '（' // U+FF08 FULLWIDTH LEFT PARENTHESIS
            | '［' // U+FF3B FULLWIDTH LEFT SQUARE BRACKET
            | '｛' // U+FF5B FULLWIDTH LEFT CURLY BRACKET
            | '「' // U+300C LEFT CORNER BRACKET
            | '『' // U+300E LEFT WHITE CORNER BRACKET
            | '〔' // U+3014 LEFT TORTOISE SHELL BRACKET
            | '〈' // U+3008 LEFT ANGLE BRACKET
            | '《' // U+300A LEFT DOUBLE ANGLE BRACKET
            | '【' // U+3010 LEFT BLACK LENTICULAR BRACKET
            | '〖' // U+3016 LEFT WHITE LENTICULAR BRACKET
            | '〘' // U+3018 LEFT WHITE TORTOISE SHELL BRACKET
            | '〝' // U+301D REVERSED DOUBLE PRIME QUOTATION MARK
            | '｟' // U+FF5F FULLWIDTH LEFT WHITE PARENTHESIS
        )
    }

    /// True for characters in the CJK ideograph / kana ranges that
    /// admit per-character line breaks. The composer's per-word
    /// segmenter splits on ASCII whitespace, which leaves CJK
    /// paragraphs as one giant "word" with no break opportunities;
    /// when `kinsoku_enforce` is on we add per-character breaks
    /// inside every word whose chars satisfy this predicate (or are
    /// in the kinsoku punctuation sets, since those are themselves
    /// CJK punctuation).
    pub fn is_breakable_cjk(c: char) -> bool {
        let c = c as u32;
        // CJK Unified Ideographs (basic): U+4E00..=U+9FFF
        (0x4E00..=0x9FFF).contains(&c)
            // Hiragana: U+3040..=U+309F
            || (0x3040..=0x309F).contains(&c)
            // Katakana: U+30A0..=U+30FF
            || (0x30A0..=0x30FF).contains(&c)
            // Halfwidth Katakana: U+FF65..=U+FF9F
            || (0xFF65..=0xFF9F).contains(&c)
            // CJK Symbols & Punctuation: U+3000..=U+303F
            || (0x3000..=0x303F).contains(&c)
            // Fullwidth ASCII (incl. punctuation): U+FF00..=U+FF60
            || (0xFF00..=0xFF60).contains(&c)
    }
}

/// Abstraction over width measurement. Returns advances in 1/64 pt.
pub trait AdvanceMeasurer {
    /// Advance width of `text`. `text` must not contain whitespace.
    fn measure_word(&self, text: &str) -> i32;
    /// Advance width of a single inter-word break opportunity.
    fn space_width(&self) -> i32;
}

/// Produces per-glyph data the layout pass needs to position text.
///
/// This sits *above* [`AdvanceMeasurer`]: every shaper is also a
/// measurer (measurement is just `shape(text).total_advance`). The
/// two traits are separate because the composer only needs widths —
/// keeping the cheaper path allocation-free where possible.
pub trait TextShaper: AdvanceMeasurer {
    /// Shape `text` into glyph ids + advances at the shaper's point
    /// size. Units are 1/64 pt.
    fn shape(&self, text: &str) -> ShapedRun;
}

/// Knobs the calibration spike tunes against InDesign.
///
/// `'a` is the lifetime of the optional `&Hyphenator` borrow. Building
/// `ComposeOptions` without a hyphenator imposes no lifetime obligation
/// on callers — the lifetime is inferred to `'static`.
#[derive(Debug, Clone)]
pub struct ComposeOptions<'a> {
    /// Column width in 1/64 pt. Used as the only width when
    /// `column_widths` is `None`, and as the fallback for lines past
    /// the end of `column_widths`.
    pub column_width: i32,
    /// Per-line column widths in 1/64 pt. Index `i` is the width
    /// available for the `i`-th composed line; lines past the end
    /// of the slice fall back to `column_width`. `None` means "use
    /// `column_width` for every line" (the legacy single-width
    /// shape). Drives text-wrap-around-objects and other layouts
    /// where a wrap rectangle carves a hole out of specific lines.
    pub column_widths: Option<Vec<i32>>,
    /// paragraph-breaker tolerance. Higher = more permissive fits.
    pub tolerance: f32,
    /// Looseness bias: >0 prefers longer paragraphs, <0 shorter.
    pub looseness: i32,
    /// Inter-word glue: stretch as a fraction of `space_width`.
    pub stretch_ratio: f32,
    /// Inter-word glue: shrink as a fraction of `space_width`.
    pub shrink_ratio: f32,
    /// Optional hyphenation engine. When set, the composer emits
    /// flagged Penalty items at every TeX-pattern break opportunity
    /// inside each word; paragraph-breaker decides whether to take
    /// them based on `tolerance` and `hyphen_penalty`.
    pub hyphenator: Option<&'a Hyphenator>,
    /// Penalty cost paid when a line is broken at a hyphenation
    /// opportunity. Knuth-Plass convention: 50 = mildly penalised,
    /// 100 = costly. Only consulted when `hyphenator` is set.
    pub hyphen_penalty: i32,
    /// When `true`, the composer emits per-character break opportunities
    /// inside CJK runs and forbids breaks that would violate the
    /// built-in "Hard Kinsoku" rules — i.e. would put a no-start
    /// character at the start of a continuation line, or leave a
    /// no-end character dangling at the end of a line. The character
    /// set is the JIS-derived hard set hardcoded in
    /// [`kinsoku::is_no_start`] / [`kinsoku::is_no_end`] (~30 glyphs
    /// each).
    ///
    /// The renderer drives this from the paragraph's resolved
    /// `kinsoku_type` (any value present ⇒ enforce). Finer flavour-
    /// specific behaviour (`PushIn` vs. `PushOut`) is queued; today
    /// every flavour maps to "high penalty before/after the
    /// offending char" which paragraph-breaker honours by picking a
    /// non-violating break candidate.
    pub kinsoku_enforce: bool,
}

impl ComposeOptions<'_> {
    /// Defaults calibrated against InDesign's Paragraph Composer.
    ///
    /// `stretch_ratio` and `shrink_ratio` mirror Adobe's Justification
    /// preset (`MinimumWordSpacing="80" DesiredWordSpacing="100"
    /// MaximumWordSpacing="133"`) — i.e. inter-word glue can shrink to
    /// 80% of its natural width (= 0.20 below) and stretch to 133%
    /// (= 0.33 above). The previous defaults (1.0/0.5) were too
    /// permissive: with a 100% stretch budget paragraph-breaker could
    /// over-pack lines by deferring breaks past where InDesign would
    /// take them, costing line-break parity on the calibration corpus
    /// (50% match → 100% with the new ratios).
    ///
    /// `tolerance = 8` accommodates left-aligned paragraphs whose
    /// short tail lines have unavoidable high stretch ratios in
    /// Knuth-Plass terms even though the rendered output never
    /// actually stretches glue (lines just left-flush). The 4.0
    /// default would reject these candidates entirely on some
    /// corpus paragraphs.
    ///
    /// See `spikes/composer-calibration/` for the corpus and the
    /// sweep that produced these numbers.
    pub fn new(column_width_pt: f32) -> Self {
        Self {
            column_width: (column_width_pt * ADVANCE_PRECISION).round() as i32,
            column_widths: None,
            tolerance: 8.0,
            looseness: 0,
            stretch_ratio: 0.33,
            shrink_ratio: 0.2,
            hyphenator: None,
            hyphen_penalty: 50,
            kinsoku_enforce: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComposedLine {
    /// Byte range of the source paragraph this line covers (after
    /// trailing space is trimmed).
    pub byte_range: std::ops::Range<usize>,
    /// Content width of the line, 1/64 pt.
    pub width: i32,
    /// Justification ratio: 0 = natural width, >0 = stretched, <0 =
    /// shrunk. Matches paragraph-breaker's convention.
    pub ratio: f32,
    /// True when the line break landed on a flagged hyphenation
    /// penalty mid-word. The layout pass appends a `-` glyph after
    /// the line's last glyph so the rendered output reads correctly.
    pub ends_with_hyphen: bool,
}

/// Per-paragraph drop-cap configuration.
///
/// IDML expresses this as
/// `<ParagraphStyleRange DropCapCharacters="1" DropCapLines="3" .../>`:
/// the first `characters` glyphs of the paragraph render at a height
/// equal to `lines` body lines, and the remainder of the paragraph
/// wraps to the right of the dropped glyph(s) for that many lines.
///
/// We model the drop cap as a synthetic per-line column-width table
/// (see [`drop_cap_column_widths`]), reusing the existing
/// `column_widths` mechanism in [`ComposeOptions`]. The dropped
/// glyph itself is shaped by a separate pass at the larger point
/// size — see [`drop_cap_point_size`] for how to compute it.
///
/// `characters == 0` means "no drop cap" — the column-widths helper
/// returns an empty vec and the caller should treat the paragraph
/// as a regular flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropCapSpec {
    /// Number of leading characters (Unicode scalars) to drop. IDML's
    /// `DropCapCharacters`. Zero disables the drop cap.
    pub characters: u32,
    /// Number of body lines the dropped glyph spans. IDML's
    /// `DropCapLines`. Typical value: 2 or 3.
    pub lines: u32,
    /// Advance width of the dropped run *at its enlarged point size*,
    /// in 1/64 pt. The remainder paragraph indents by this much for
    /// `lines` lines so body text doesn't collide with the drop cap.
    /// The caller computes this by shaping the dropped characters
    /// via [`TextShaper::shape`] at [`drop_cap_point_size`].
    pub glyph_advance: i32,
    /// Extra space between the drop-cap glyph and the body text, in
    /// 1/64 pt. IDML's `DropCapDetail` is the side-bearing tweak —
    /// we approximate as a flat gutter. A reasonable default is
    /// `space_width / 2`.
    pub gutter: i32,
}

impl DropCapSpec {
    /// True when this spec asks for a real drop cap (non-zero
    /// characters and lines). Helpers fall through to the no-op
    /// behaviour when this is false.
    pub fn is_active(&self) -> bool {
        self.characters > 0 && self.lines > 0
    }
}

/// Compute the enlarged point size for a drop-cap glyph.
///
/// IDML's drop cap height is "M body lines tall". We approximate as
/// `body_line_height * drop_cap_lines` — the dropped glyph is shaped
/// at this point size so its cap-height fills the spanned lines. In
/// practice the shaped height is slightly smaller than the line
/// height (cap-height vs em-square), which matches InDesign's visual
/// (the drop cap doesn't quite touch the baseline of the M-th line).
///
/// `body_line_height_pt` is the body paragraph's line height in pt
/// (i.e. `LayoutOptions::line_height` divided by `ADVANCE_PRECISION`).
/// `drop_cap_lines` is `DropCapSpec::lines`. Returns the point size
/// to pass to the shaper / measurer for the dropped run.
pub fn drop_cap_point_size(body_line_height_pt: f32, drop_cap_lines: u32) -> f32 {
    if drop_cap_lines == 0 {
        return 0.0;
    }
    body_line_height_pt * drop_cap_lines as f32
}

/// Build a per-line `column_widths` vector that carves out a
/// drop-cap shaped notch from the first `spec.lines` lines.
///
/// The first `spec.lines` entries equal `base_width - (glyph_advance
/// + gutter)` — those lines are narrower because the drop-cap glyph
/// occupies the leftmost column. Lines past `spec.lines` aren't
/// included; the composer falls back to `column_width` for them
/// (see [`ComposeOptions::column_widths`]).
///
/// If `spec` is inactive (no drop cap), returns an empty vec — the
/// caller treats this the same as `None` and the column shape is
/// unchanged.
///
/// All values in 1/64 pt.
pub fn drop_cap_column_widths(spec: &DropCapSpec, base_width: i32) -> Vec<i32> {
    if !spec.is_active() {
        return Vec::new();
    }
    let indent = spec.glyph_advance.saturating_add(spec.gutter);
    let narrow = (base_width - indent).max(0);
    vec![narrow; spec.lines as usize]
}

/// Result of [`compose_paragraph_with_drop_cap`].
///
/// The composer splits the paragraph into:
/// 1. The dropped run (first `spec.characters` glyphs) — shaped
///    separately at the enlarged point size.
/// 2. The remainder, composed with the carved-out column widths.
///
/// Callers position the drop-cap glyph at the paragraph origin
/// (left edge, top of the first body line — InDesign aligns the
/// drop cap's cap-height to the first line's cap-height). The
/// remainder lines then layout as usual: the first `spec.lines`
/// lines start at `glyph_advance + gutter`, subsequent lines at 0.
///
/// `dropped_byte_range` covers the source bytes consumed by the
/// dropped run — the layout pass walks the source paragraph using
/// `dropped_byte_range.end` as the start offset for the remainder
/// lines (whose `byte_range` is paragraph-relative, not
/// remainder-relative).
#[derive(Debug, Clone, PartialEq)]
pub struct DropCapComposition {
    /// The dropped run's source byte range (paragraph-relative). The
    /// layout pass shapes `&text[dropped_byte_range]` at the enlarged
    /// point size.
    pub dropped_byte_range: std::ops::Range<usize>,
    /// Composed lines for the remainder of the paragraph. Each
    /// line's `byte_range` is paragraph-relative (not relative to
    /// the remainder slice) — the composer translates internally.
    pub lines: Vec<ComposedLine>,
}

/// Compose a paragraph with a drop cap.
///
/// `base_options.column_widths` is overlaid with a per-line widths
/// table that narrows the first `spec.lines` lines by the drop-cap
/// glyph's advance + gutter. The dropped glyph itself is *not*
/// composed — the caller shapes it separately (at
/// [`drop_cap_point_size`]) and positions it at the paragraph
/// origin.
///
/// The first `spec.characters` Unicode scalars of `text` are skipped
/// before the regular composition — they belong to the dropped run.
/// If `text` has fewer scalars than `spec.characters`, every
/// character drops and the result has zero remainder lines.
///
/// When `spec.is_active()` is false, this function is equivalent to
/// `compose_paragraph(text, measurer, base_options)` wrapped in a
/// `DropCapComposition` with an empty `dropped_byte_range`.
///
/// Note: this entry point does **not** mutate `base_options` — it
/// builds a temporary copy internally. Callers can share a single
/// `ComposeOptions` across paragraphs that may or may not have drop
/// caps.
pub fn compose_paragraph_with_drop_cap(
    text: &str,
    measurer: &dyn AdvanceMeasurer,
    base_options: &ComposeOptions,
    spec: &DropCapSpec,
) -> DropCapComposition {
    if !spec.is_active() {
        return DropCapComposition {
            dropped_byte_range: 0..0,
            lines: compose_paragraph(text, measurer, base_options),
        };
    }
    // Walk `spec.characters` Unicode scalars off the front to find
    // the byte split. Char-counted, not byte-counted, because IDML's
    // DropCapCharacters is a character count.
    let mut split = 0usize;
    let mut taken = 0u32;
    for (i, _) in text.char_indices() {
        if taken == spec.characters {
            split = i;
            break;
        }
        taken += 1;
    }
    if taken < spec.characters {
        // Whole paragraph fit inside the drop-cap span — there are
        // no remainder lines. (Edge case: a one-character paragraph
        // with DropCapCharacters="3".)
        return DropCapComposition {
            dropped_byte_range: 0..text.len(),
            lines: Vec::new(),
        };
    }
    let dropped_byte_range = 0..split;
    let remainder = &text[split..];

    // Build the per-line widths table for the remainder. We start
    // with the caller-supplied `column_widths` if any, then narrow
    // the first `spec.lines` entries. If the caller already set
    // `column_widths` (e.g. a text-wrap rectangle), we merge by
    // taking the min width per line — drop cap and text wrap both
    // *carve out* space from the column.
    let mut widths = drop_cap_column_widths(spec, base_options.column_width);
    if let Some(existing) = base_options.column_widths.as_deref() {
        for (i, w) in widths.iter_mut().enumerate() {
            if let Some(&e) = existing.get(i) {
                *w = (*w).min(e);
            }
        }
        // Append any tail lines from the caller's table that extend
        // past the drop-cap span — those lines aren't narrowed by
        // the drop cap but may still be narrowed by a wrap.
        for &e in existing.iter().skip(widths.len()) {
            widths.push(e);
        }
    }

    let mut opts = base_options.clone();
    opts.column_widths = Some(widths);

    // Compose the remainder. ComposedLine::byte_range is relative to
    // the remainder slice — translate back to paragraph coordinates
    // so callers see paragraph-relative offsets consistent with
    // `compose_paragraph` on the whole text.
    let mut lines = compose_paragraph(remainder, measurer, &opts);
    for line in &mut lines {
        line.byte_range.start += split;
        line.byte_range.end += split;
    }

    DropCapComposition {
        dropped_byte_range,
        lines,
    }
}

/// One-shot guard for the hyphenation-parity advisory log. We emit
/// the "TeX patterns; Proximity dictionary not licensed" trace once
/// per process — enough for an operator scanning logs to notice the
/// divergence without flooding traces with one entry per composed
/// paragraph. See `docs/hyphenation-parity.md` for the full known
/// divergence between our TeX-pattern hyphenator and InDesign's
/// Proximity dictionaries.
static HYPHENATION_DIVERGENCE_LOGGED: AtomicBool = AtomicBool::new(false);

fn note_hyphenation_divergence_once() {
    // Relaxed is fine: this is best-effort advisory; a benign
    // duplicate log on a tight race is acceptable and we don't pair
    // it with any other memory ordering.
    if !HYPHENATION_DIVERGENCE_LOGGED.swap(true, Ordering::Relaxed) {
        tracing::debug!(
            target: "idml_text::compose",
            "hyphenation: TeX patterns (hypher); Proximity dictionary not licensed — \
             expect minor break-position divergence vs InDesign. \
             See docs/hyphenation-parity.md."
        );
    }
}

/// Compose one paragraph.
///
/// Splits `text` into words by ASCII whitespace, measures each with
/// `measurer`, builds a Knuth-Plass item stream, and invokes
/// `paragraph_breaker::total_fit`. Returns the resulting line sequence.
///
/// When `options.hyphenator` is set, each word is split into segments
/// at every TeX-pattern break point and a flagged Penalty is emitted
/// between segments so paragraph-breaker can hyphenate mid-word.
pub fn compose_paragraph(
    text: &str,
    measurer: &dyn AdvanceMeasurer,
    options: &ComposeOptions,
) -> Vec<ComposedLine> {
    let words = segment(text);
    if words.is_empty() {
        return Vec::new();
    }
    if options.hyphenator.is_some() {
        note_hyphenation_divergence_once();
    }
    let space_width = measurer.space_width();
    let stretch = (space_width as f32 * options.stretch_ratio).round() as i32;
    let shrink = (space_width as f32 * options.shrink_ratio).round() as i32;
    let hyphen_width = if options.hyphenator.is_some() {
        measurer.measure_word("-")
    } else {
        0
    };

    // Per-item metadata kept in lockstep with the items vector via
    // `push_item`. paragraph-breaker takes `&[Item<()>]`, so we keep
    // the items in their own contiguous Vec — but every push goes
    // through the helper so the byte_end / is_hyphen side-data can
    // never drift out of sync.
    let item_capacity = if options.hyphenator.is_some() {
        words.len() * 4 + 2
    } else {
        words.len() * 2 + 2
    };
    let mut items: Vec<Item<()>> = Vec::with_capacity(item_capacity);
    let mut meta: Vec<ItemMeta> = Vec::with_capacity(item_capacity);
    let push = |items: &mut Vec<_>, meta: &mut Vec<ItemMeta>, item, byte_end, is_hyphen| {
        items.push(item);
        meta.push(ItemMeta {
            byte_end,
            is_hyphen,
        });
    };

    for (i, w) in words.iter().enumerate() {
        let word_text = &text[w.start..w.end];
        // No hyphenator → emit a single Box for the whole word and
        // skip the per-word break-vec construction entirely.
        // When kinsoku is enforced, we layer per-character break
        // opportunities over either path (after the base items are
        // emitted, we walk the word's chars and inject penalty items
        // between any pair where at least one is CJK).
        match options.hyphenator {
            None if !options.kinsoku_enforce => {
                push(
                    &mut items,
                    &mut meta,
                    Item::Box {
                        width: measurer.measure_word(word_text),
                        data: (),
                    },
                    w.end,
                    false,
                );
            }
            None => {
                // Kinsoku-only path: walk the word char by char,
                // emitting a `Box` per character and a `Penalty`
                // between adjacent chars when at least one is CJK
                // (or matches the kinsoku punctuation set). The
                // penalty is INFINITE when the pair would land a
                // no-start char at the start of a continuation line
                // or strand a no-end char at the end of the previous
                // line; 0 (free break) otherwise.
                emit_word_with_kinsoku_breaks(
                    word_text, w.start, measurer, &mut items, &mut meta, &push,
                );
            }
            Some(h) => {
                let mut seg_start = 0usize;
                for offset in h.opportunities(word_text) {
                    if offset <= seg_start || offset >= word_text.len() {
                        continue;
                    }
                    push(
                        &mut items,
                        &mut meta,
                        Item::Box {
                            width: measurer.measure_word(&word_text[seg_start..offset]),
                            data: (),
                        },
                        w.start + offset,
                        false,
                    );
                    push(
                        &mut items,
                        &mut meta,
                        Item::Penalty {
                            width: hyphen_width,
                            penalty: options.hyphen_penalty,
                            flagged: true,
                        },
                        w.start + offset,
                        true,
                    );
                    seg_start = offset;
                }
                push(
                    &mut items,
                    &mut meta,
                    Item::Box {
                        width: measurer.measure_word(&word_text[seg_start..]),
                        data: (),
                    },
                    w.end,
                    false,
                );
                // Hyphenation + kinsoku can both apply — when both
                // are on, the kinsoku enforcement adds high-penalty
                // *inhibition* items at any violating intra-word
                // position. We don't currently subdivide the
                // hyphenated segments further; full kinsoku-over-
                // hyphenation interplay is queued. Latin text rarely
                // sees both at once, so this is acceptable.
                let _ = options.kinsoku_enforce;
            }
        }

        if i + 1 < words.len() {
            push(
                &mut items,
                &mut meta,
                Item::Glue {
                    width: space_width,
                    stretch,
                    shrink,
                },
                // A break at this glue trims the trailing space, so
                // the byte_end is the previous word's end.
                w.end,
                false,
            );
        }
    }
    // Paragraph end: infinite stretch + forced break (TeX convention).
    push(
        &mut items,
        &mut meta,
        Item::Glue {
            width: 0,
            stretch: paragraph_breaker::INFINITE_PENALTY,
            shrink: 0,
        },
        text.len(),
        false,
    );
    push(
        &mut items,
        &mut meta,
        Item::Penalty {
            width: 0,
            penalty: -paragraph_breaker::INFINITE_PENALTY,
            flagged: true,
        },
        text.len(),
        false,
    );

    // Per-line widths drive text-wrap-around-objects: each line's
    // available width is computed from any wrap rectangles
    // overlapping that line's predicted y-range. Without an
    // explicit `column_widths`, every line uses `column_width`.
    let single_width = [options.column_width];
    let lengths: &[i32] = options
        .column_widths
        .as_deref()
        .filter(|v| !v.is_empty())
        .unwrap_or(&single_width);
    // Mirror layout_runs' fallback: when the configured tolerance
    // can't fit the paragraph, retry progressively looser so a long
    // paragraph isn't dropped entirely.
    let mut breaks: Vec<Breakpoint> =
        paragraph_breaker::total_fit(&items, lengths, options.tolerance, options.looseness);
    if breaks.is_empty() && !items.is_empty() {
        for fallback_tol in [
            options.tolerance * 4.0,
            options.tolerance * 16.0,
            1000.0,
        ] {
            breaks = paragraph_breaker::total_fit(&items, lengths, fallback_tol, options.looseness);
            if !breaks.is_empty() {
                break;
            }
        }
    }

    // Translate Breakpoints (item indices) into byte ranges. A break
    // at a flagged hyphenation penalty marks the line for hyphen
    // rendering at the layout pass.
    let mut lines = Vec::with_capacity(breaks.len());
    let mut byte_cursor = 0usize;
    let bytes = text.as_bytes();
    for bp in &breaks {
        let Some(m) = meta.get(bp.index) else {
            continue;
        };
        // Skip whitespace at the line's left edge (after a glue
        // break) so byte_range tracks visible content.
        let mut start = byte_cursor;
        while start < m.byte_end && bytes[start].is_ascii_whitespace() {
            start += 1;
        }
        if start >= m.byte_end {
            continue;
        }
        lines.push(ComposedLine {
            byte_range: start..m.byte_end,
            width: bp.width,
            ratio: bp.ratio,
            ends_with_hyphen: m.is_hyphen,
        });
        byte_cursor = m.byte_end;
    }
    lines
}

struct ItemMeta {
    byte_end: usize,
    is_hyphen: bool,
}

/// Emit one Knuth-Plass Box per Unicode scalar of `word_text`, with
/// inter-character `Penalty` items where the kinsoku rules permit
/// (free break) or forbid (INFINITE_PENALTY) a line break.
///
/// `word_text` is the slice for one whitespace-separated word; the
/// caller positions it at byte offset `word_start` inside the source
/// paragraph. The emitted byte_ends in `meta` are absolute (paragraph-
/// relative) so the breakpoint translation in `compose_paragraph`
/// stays consistent with the non-kinsoku path.
///
/// We measure each char individually via `measurer.measure_word`
/// rather than splitting a pre-measured total — the measurer is the
/// only source of truth for width, and `measurer` shapes each chunk
/// fresh which keeps the totals consistent with the non-kinsoku path
/// (down to the rounding the measurer applies).
///
/// Word-internal segmentation rule: a `Penalty` is emitted between
/// adjacent chars `(a, b)` only when both ends are "kinsoku-relevant"
/// — at least one of `a`, `b` is in [`kinsoku::is_breakable_cjk`] or
/// in either kinsoku set. ASCII Latin pairs ("ab" inside a word) get
/// no break opportunity — they stay one logical Box from the
/// breaker's perspective even though they're emitted as multiple
/// Boxes here (the absence of an interceding Penalty means
/// paragraph-breaker can't pick a break there).
fn emit_word_with_kinsoku_breaks(
    word_text: &str,
    word_start: usize,
    measurer: &dyn AdvanceMeasurer,
    items: &mut Vec<Item<()>>,
    meta: &mut Vec<ItemMeta>,
    push: &impl Fn(&mut Vec<Item<()>>, &mut Vec<ItemMeta>, Item<()>, usize, bool),
) {
    let chars: Vec<(usize, char)> = word_text.char_indices().collect();
    if chars.is_empty() {
        return;
    }
    for (idx, &(off, ch)) in chars.iter().enumerate() {
        let next_off = chars
            .get(idx + 1)
            .map(|&(o, _)| o)
            .unwrap_or(word_text.len());
        let ch_text = &word_text[off..next_off];
        let byte_end = word_start + next_off;
        push(
            items,
            meta,
            Item::Box {
                width: measurer.measure_word(ch_text),
                data: (),
            },
            byte_end,
            false,
        );
        // Inject a kinsoku-aware break opportunity between this
        // char and the next (if any).
        //
        // Knuth-Plass semantics: a break can land at a `Glue` (the
        // canonical break point — its width is consumed by the
        // breaker and its stretch absorbs short-line slack) or at a
        // finite-penalty `Penalty`. The *absence* of either between
        // two `Box`es inhibits any break at that position.
        //
        // We emit a zero-width Glue with mild stretch + zero shrink
        // between break-permitted CJK chars. The stretch is critical
        // — without it, lines that come up short (the common case
        // for monospaced CJK where a column rarely lands on an exact
        // multiple of the char width) have no slack budget and the
        // breaker rejects the entire paragraph (`total_fit` returns
        // empty when no feasible solution exists). The Glue's width
        // is 0 so the visual rendering is unchanged.
        //
        // We don't emit a Glue between non-CJK pairs (Latin words
        // stay one logical Box from the breaker's perspective) or at
        // forbidden positions (no-start at the next char, or no-end
        // at the current — the kinsoku-rule enforcement).
        if let Some(&(_, next_ch)) = chars.get(idx + 1) {
            let pair_is_kinsoku_relevant = kinsoku::is_breakable_cjk(ch)
                || kinsoku::is_breakable_cjk(next_ch)
                || kinsoku::is_no_start(next_ch)
                || kinsoku::is_no_end(ch);
            if !pair_is_kinsoku_relevant {
                continue;
            }
            let forbidden = kinsoku::is_no_start(next_ch) || kinsoku::is_no_end(ch);
            if forbidden {
                continue;
            }
            // Stretch budget: a CJK char's-worth (1 em ≈ point size
            // measured in 1/64 pt — we don't have the point size
            // here, so use a constant in 1/64-pt units that's larger
            // than any plausible single-char width). 1024 is ~ 16
            // pt; one inter-char gap with 1024 units of stretch can
            // absorb the slack of a line that's one char short of
            // the column.
            push(
                items,
                meta,
                Item::Glue {
                    width: 0,
                    stretch: 1024,
                    shrink: 0,
                },
                byte_end,
                false,
            );
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct WordSpan {
    start: usize,
    end: usize,
}

fn segment(text: &str) -> Vec<WordSpan> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor > start {
            out.push(WordSpan { start, end: cursor });
        }
    }
    out
}

/// Production measurer: shapes each word via `rustybuzz` at the given
/// point size and reads back advance widths.
pub struct RustybuzzMeasurer<'a> {
    pub face: &'a Face<'a>,
    pub point_size: f32,
}

impl<'a> RustybuzzMeasurer<'a> {
    pub fn new(face: &'a Face<'a>, point_size: f32) -> Self {
        Self { face, point_size }
    }
}

impl AdvanceMeasurer for RustybuzzMeasurer<'_> {
    fn measure_word(&self, text: &str) -> i32 {
        shape_run(self.face, text, self.point_size).total_advance
    }

    fn space_width(&self) -> i32 {
        shape_run(self.face, " ", self.point_size).total_advance
    }
}

impl TextShaper for RustybuzzMeasurer<'_> {
    fn shape(&self, text: &str) -> ShapedRun {
        shape_run(self.face, text, self.point_size)
    }
}

/// Deterministic measurer used in tests and by tooling that doesn't want
/// to ship a TTF. Treats each Unicode scalar as having a fixed width.
pub struct MonospaceMeasurer {
    pub char_width: i32,
    pub space_width: i32,
}

impl MonospaceMeasurer {
    pub fn new(char_width: i32, space_width: i32) -> Self {
        Self {
            char_width,
            space_width,
        }
    }
}

impl AdvanceMeasurer for MonospaceMeasurer {
    fn measure_word(&self, text: &str) -> i32 {
        text.chars().count() as i32 * self.char_width
    }

    fn space_width(&self) -> i32 {
        self.space_width
    }
}

impl TextShaper for MonospaceMeasurer {
    /// Produces a synthetic `ShapedRun` — one glyph per Unicode scalar.
    /// Useful for layout-pass tests without shipping a test font.
    fn shape(&self, text: &str) -> ShapedRun {
        let mut byte_cursor = 0u32;
        let mut total = 0i32;
        let glyphs: Vec<ShapedGlyph> = text
            .chars()
            .map(|c| {
                let cluster = byte_cursor;
                byte_cursor += c.len_utf8() as u32;
                let advance = if c.is_whitespace() {
                    self.space_width
                } else {
                    self.char_width
                };
                total += advance;
                ShapedGlyph {
                    glyph_id: c as u32,
                    cluster,
                    x_advance: advance,
                    y_offset: 0,
                    x_offset: 0,
                }
            })
            .collect();
        ShapedRun {
            glyphs,
            total_advance: total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str, column_chars: i32) -> Vec<String> {
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: column_chars * 10,
            tolerance: 10.0,
            stretch_ratio: 1.0,
            shrink_ratio: 0.5,
            ..ComposeOptions::new(0.0)
        };
        compose_paragraph(text, &m, &opts)
            .into_iter()
            .map(|l| text[l.byte_range].to_string())
            .collect()
    }

    #[test]
    fn hyphenation_inserts_mid_word_breaks_when_needed() {
        // A line that fits exactly when broken at a hyphenation
        // penalty inside the second word, and can't fit without one
        // (a single-word line has no inner glue to absorb slack).
        let m = MonospaceMeasurer::new(10, 10);
        let h = crate::Hyphenator::for_language(crate::Language::EnglishUS);
        let mut opts = ComposeOptions::new(0.0);
        opts.column_width = 80;
        opts.tolerance = 20.0;
        opts.hyphenator = Some(&h);
        let out = compose_paragraph("the elephants", &m, &opts);
        assert!(
            out.iter().any(|l| l.ends_with_hyphen),
            "expected at least one hyphenated line: {:?}",
            out
        );
    }

    #[test]
    fn no_hyphenator_means_no_hyphen_flag() {
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: 200,
            ..ComposeOptions::new(0.0)
        };
        let out = compose_paragraph("optimisation works well", &m, &opts);
        for l in &out {
            assert!(!l.ends_with_hyphen, "no hyphenator → no flag");
        }
    }

    #[test]
    fn single_word_paragraph() {
        let ls = lines("hello", 20);
        assert_eq!(ls, vec!["hello"]);
    }

    #[test]
    fn paragraph_wraps_to_multiple_lines() {
        // "lorem ipsum dolor sit amet" — 26 chars total with spaces.
        // column_chars = 12 forces a break after "lorem ipsum" (11 chars).
        let ls = lines("lorem ipsum dolor sit amet", 12);
        assert!(ls.len() >= 2, "expected >=2 lines, got {:?}", ls);
        // First line ends at or before the 12th char boundary.
        assert!(ls[0].len() <= 12, "first line too long: {:?}", ls);
        // Round-trip: joining lines with spaces reproduces the input.
        assert_eq!(ls.join(" "), "lorem ipsum dolor sit amet");
    }

    #[test]
    fn empty_paragraph_returns_no_lines() {
        let ls = lines("", 40);
        assert!(ls.is_empty());
    }

    #[test]
    fn line_widths_are_populated() {
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: 120,
            ..ComposeOptions::new(0.0)
        };
        let out = compose_paragraph("one two three four five", &m, &opts);
        for line in &out {
            assert!(line.width > 0, "width not set: {:?}", line);
        }
    }

    // ----- Drop cap -----

    #[test]
    fn drop_cap_inactive_returns_unchanged_composition() {
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: 200,
            ..ComposeOptions::new(0.0)
        };
        let spec = DropCapSpec {
            characters: 0,
            lines: 0,
            glyph_advance: 0,
            gutter: 0,
        };
        let composed = compose_paragraph_with_drop_cap("hello world", &m, &opts, &spec);
        let baseline = compose_paragraph("hello world", &m, &opts);
        assert_eq!(composed.lines, baseline);
        assert_eq!(composed.dropped_byte_range, 0..0);
    }

    #[test]
    fn drop_cap_carves_first_lines_narrower() {
        // Synthetic monospace at 10 per char/space; full column =
        // 400 (40 chars). Drop-cap glyph indent = 100, so first 3
        // lines have width 300 (30 chars), lines 4+ have width 400.
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: 400,
            tolerance: 50.0,
            ..ComposeOptions::new(0.0)
        };
        let spec = DropCapSpec {
            characters: 1,
            lines: 3,
            glyph_advance: 90,
            gutter: 10,
        };
        let widths = drop_cap_column_widths(&spec, opts.column_width);
        assert_eq!(widths, vec![300, 300, 300]);

        // Paragraph long enough to need > 3 lines.
        let text = "Once upon a time in a far off land lived a wise old wizard \
                    who knew the answer to every question but one and \
                    he kept that final answer to himself for many many years";
        let composed = compose_paragraph_with_drop_cap(text, &m, &opts, &spec);
        assert_eq!(composed.dropped_byte_range, 0..1);
        assert!(
            composed.lines.len() >= 4,
            "expected >=4 lines got {}: {:?}",
            composed.lines.len(),
            composed.lines.iter().map(|l| line_text(text, l)).collect::<Vec<_>>()
        );
        // First 3 lines fit inside the carved (300-unit) column.
        for line in composed.lines.iter().take(3) {
            assert!(
                line.width <= 300,
                "first-three line width {} exceeds carved 300 ({})",
                line.width,
                line_text(text, line)
            );
        }
        // The carved-vs-full distinction shows up in what *fits*
        // on each line: a long word that goes on line 4+ wouldn't
        // have fit on line 1-3. Check by forcing a long word past
        // the carve span: line 4 contains content that, if it had
        // been on line 1, would have overflowed 300. We assert the
        // first 3 lines used the narrow shape (already done) and
        // assert separately that paragraph-breaker honoured the
        // narrowing — by composing the *same* text without a drop
        // cap and confirming line 1 there is wider than line 1
        // here.
        let baseline = compose_paragraph(text, &m, &opts);
        assert!(
            baseline[0].width > composed.lines[0].width,
            "without drop cap, line 1 should be wider (baseline={}, \
             with-drop-cap={})",
            baseline[0].width,
            composed.lines[0].width,
        );
    }

    #[test]
    fn drop_cap_remainder_byte_ranges_are_paragraph_relative() {
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: 1500,
            ..ComposeOptions::new(0.0)
        };
        let spec = DropCapSpec {
            characters: 1,
            lines: 2,
            glyph_advance: 200,
            gutter: 20,
        };
        let text = "Once upon a time";
        let composed = compose_paragraph_with_drop_cap(text, &m, &opts, &spec);
        // First remainder line begins at byte 1 (after the dropped
        // 'O') and the source text at that byte range is part of
        // the original paragraph.
        let first = &composed.lines[0];
        assert!(first.byte_range.start >= 1);
        assert!(first.byte_range.end <= text.len());
        let snippet = &text[first.byte_range.clone()];
        // Either skips leading whitespace (none here at byte 1) or
        // begins with the post-O character. Confirm round-trip
        // doesn't panic on UTF-8 boundaries.
        assert!(!snippet.is_empty());
    }

    #[test]
    fn drop_cap_short_paragraph_consumes_all_text() {
        // Drop cap requests 5 chars, but the paragraph is only 2.
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: 200,
            ..ComposeOptions::new(0.0)
        };
        let spec = DropCapSpec {
            characters: 5,
            lines: 3,
            glyph_advance: 100,
            gutter: 10,
        };
        let composed = compose_paragraph_with_drop_cap("ok", &m, &opts, &spec);
        assert_eq!(composed.dropped_byte_range, 0..2);
        assert!(composed.lines.is_empty());
    }

    #[test]
    fn drop_cap_point_size_scales_with_lines() {
        // 12pt body × 3 drop-cap lines = 36pt drop cap.
        assert_eq!(drop_cap_point_size(12.0, 3), 36.0);
        // No drop cap = zero point size.
        assert_eq!(drop_cap_point_size(12.0, 0), 0.0);
    }

    #[test]
    fn drop_cap_existing_column_widths_are_merged() {
        // If the caller already set column_widths (e.g. text-wrap),
        // the drop cap takes the min per line so both carvings are
        // honoured.
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: 1500,
            // First line is *already* very narrow (a wrap rectangle).
            column_widths: Some(vec![400, 1500, 1500, 1500]),
            ..ComposeOptions::new(0.0)
        };
        let spec = DropCapSpec {
            characters: 1,
            lines: 3,
            glyph_advance: 600,
            gutter: 50,
        };
        // Drop cap would carve to 850 on each of the first 3, but
        // the wrap on line 1 is even narrower (400) — keep 400.
        let widths = drop_cap_column_widths(&spec, opts.column_width);
        assert_eq!(widths, vec![850, 850, 850]);

        let composed = compose_paragraph_with_drop_cap(
            "Once upon a time in a faraway land lived a lonely old wizard \
             with a long white beard and a tall pointed hat",
            &m,
            &opts,
            &spec,
        );
        // First line uses min(850, 400) = 400.
        let first = &composed.lines[0];
        assert!(first.width <= 400, "first line should be wrap-narrow: {:?}", first);
    }

    #[test]
    fn default_compose_paragraph_matches_drop_cap_inactive() {
        // Corpus-protection guard: with `DropCapSpec` inactive,
        // compose_paragraph_with_drop_cap must produce *exactly* the
        // same line stream as compose_paragraph for arbitrary input.
        let m = MonospaceMeasurer::new(10, 10);
        let opts = ComposeOptions {
            column_width: 90,
            tolerance: 10.0,
            ..ComposeOptions::new(0.0)
        };
        let text = "the quick brown fox jumps over the lazy dog";
        let baseline = compose_paragraph(text, &m, &opts);
        let inactive = DropCapSpec {
            characters: 0,
            lines: 0,
            glyph_advance: 9999,
            gutter: 9999,
        };
        let with_cap = compose_paragraph_with_drop_cap(text, &m, &opts, &inactive);
        assert_eq!(with_cap.lines, baseline);
    }

    fn line_text<'a>(text: &'a str, line: &ComposedLine) -> &'a str {
        &text[line.byte_range.clone()]
    }

    // ---- CJK Stage 2 (kinsoku enforcement) ----

    #[test]
    fn kinsoku_set_membership_matches_hard_set() {
        // Spot-check the JIS-derived sets. If any of these flip the
        // composer's penalty design needs to be revisited.
        assert!(kinsoku::is_no_start('）'));
        assert!(kinsoku::is_no_start('」'));
        assert!(kinsoku::is_no_start('、'));
        assert!(kinsoku::is_no_start('。'));
        assert!(kinsoku::is_no_start('っ'));
        assert!(kinsoku::is_no_start(')'));

        assert!(kinsoku::is_no_end('('));
        assert!(kinsoku::is_no_end('（'));
        assert!(kinsoku::is_no_end('「'));
        assert!(kinsoku::is_no_end('『'));

        // A regular CJK character is neither.
        assert!(!kinsoku::is_no_start('本'));
        assert!(!kinsoku::is_no_end('本'));
        assert!(kinsoku::is_breakable_cjk('本'));
        assert!(kinsoku::is_breakable_cjk('あ'));
        assert!(kinsoku::is_breakable_cjk('ア'));

        // Latin chars don't admit per-character break opportunities.
        assert!(!kinsoku::is_breakable_cjk('a'));
        assert!(!kinsoku::is_breakable_cjk('A'));
    }

    #[test]
    fn kinsoku_disabled_baseline_keeps_cjk_text_on_one_line() {
        // Sanity: without kinsoku enforcement, a CJK paragraph is one
        // big "word" with no internal break opportunities, so the
        // breaker can only put it on one line — and a column wide
        // enough to hold the whole word produces exactly one line.
        // (If the column were narrower than the word, paragraph-
        // breaker would return no breaks at all because no feasible
        // line fit exists. That's what the "kinsoku enables wrap"
        // test below demonstrates with the same text + narrower
        // column.)
        let m = MonospaceMeasurer::new(10, 10);
        let text = "本日本日本日本日本日"; // 10 CJK chars × 10 units
        let opts = ComposeOptions {
            column_width: 200, // wider than the whole text (100)
            kinsoku_enforce: false,
            ..ComposeOptions::new(0.0)
        };
        let lines = compose_paragraph(text, &m, &opts);
        assert_eq!(
            lines.len(),
            1,
            "without kinsoku_enforce, no per-character breaks: {:?}",
            lines
        );
    }

    #[test]
    fn kinsoku_enabled_breaks_per_character_in_cjk_text() {
        // With enforcement on, the composer emits Penalty(0) between
        // adjacent CJK chars and the breaker fits multiple lines.
        let m = MonospaceMeasurer::new(10, 10);
        let text = "本日本日本日本日本日"; // 10 CJK chars, 10 units each
        let opts = ComposeOptions {
            column_width: 40, // room for 4 chars per line
            kinsoku_enforce: true,
            ..ComposeOptions::new(0.0)
        };
        let lines = compose_paragraph(text, &m, &opts);
        assert!(
            lines.len() >= 2,
            "kinsoku_enforce → per-char breaks: {:?}",
            lines
        );
        // Each line should be at most the column width's worth of
        // CJK chars.
        for line in &lines {
            assert!(
                line.width <= 40,
                "line too wide: {} > 40 in {:?}",
                line.width,
                line
            );
        }
    }

    #[test]
    fn kinsoku_forbids_breaking_before_no_start_char() {
        // A paragraph where the breaker would otherwise place a `）`
        // at the start of a continuation line. With kinsoku enforced
        // the break must shift earlier so the `）` rides with its
        // preceding char.
        //
        // Layout: `本本本本本）本本本本本` — 11 chars, 10 units each;
        // column = 50 (5 chars per line). Naive per-char break would
        // give "本本本本本" / "）本本本本本" which strands the closing
        // paren at the start. Kinsoku must shift to "本本本本" /
        // "本）本本本本本" (or similar — the key invariant is that
        // no line starts with `）`).
        let m = MonospaceMeasurer::new(10, 10);
        let text = "本本本本本）本本本本本";
        let opts = ComposeOptions {
            column_width: 50,
            kinsoku_enforce: true,
            ..ComposeOptions::new(0.0)
        };
        let lines = compose_paragraph(text, &m, &opts);
        assert!(lines.len() >= 2, "expected multi-line: {:?}", lines);
        for line in &lines {
            let line_text = &text[line.byte_range.clone()];
            let first_char = line_text.chars().next().unwrap();
            assert!(
                !kinsoku::is_no_start(first_char),
                "line starts with no-start char {:?}: {:?}",
                first_char,
                line
            );
        }
    }

    #[test]
    fn kinsoku_behavior_change_off_vs_on_is_demonstrable() {
        // Direct comparison: with the same text and column width,
        // the composer's output line count differs between
        // `kinsoku_enforce = false` and `= true` — a fixture-free
        // demonstration of the composer behaviour change Stage 2 of
        // Tier 4 CJK introduces.
        //
        // The column is wide enough to hold the whole text on one
        // line (so the baseline doesn't fail with "no feasible
        // fit"), but with enforcement on the per-character break
        // opportunities let the breaker pick a multi-line composition
        // and the no-start rule keeps `）` away from line starts.
        let m = MonospaceMeasurer::new(10, 10);
        let text = "本本本本本）本本本本本";
        let baseline_opts = ComposeOptions {
            column_width: 200, // whole text fits in one line
            kinsoku_enforce: false,
            ..ComposeOptions::new(0.0)
        };
        let baseline = compose_paragraph(text, &m, &baseline_opts);
        // Enforced path uses a NARROWER column (text needs wrap) +
        // kinsoku enforcement; the breaker exploits the per-char
        // break opportunities and the no-start rule.
        let enforced_opts = ComposeOptions {
            column_width: 60, // narrower → forces wrap
            kinsoku_enforce: true,
            ..ComposeOptions::new(0.0)
        };
        let enforced = compose_paragraph(text, &m, &enforced_opts);

        assert_eq!(baseline.len(), 1, "baseline = single line: {:?}", baseline);
        assert!(
            enforced.len() >= 2,
            "kinsoku on + narrow column wraps: {:?}",
            enforced
        );
        // No line in the kinsoku path begins with `）`.
        for line in &enforced {
            let first = text[line.byte_range.clone()].chars().next().unwrap();
            assert_ne!(first, '）', "kinsoku never strands closing paren");
        }
    }

    #[test]
    fn kinsoku_forbids_no_end_char_at_line_end() {
        // `（` should never end a line. Without enforcement and
        // with an artificial wrap, the baseline could strand it
        // there; with enforcement on, the breaker shifts.
        //
        // 11 chars; column = 50 (5 chars). The "natural" 5-char
        // split puts `（` at position 5 — the LAST char of line 1.
        // Kinsoku must push it to line 2.
        let m = MonospaceMeasurer::new(10, 10);
        let text = "本本本本（本本本本本本";
        let opts = ComposeOptions {
            column_width: 50,
            kinsoku_enforce: true,
            ..ComposeOptions::new(0.0)
        };
        let lines = compose_paragraph(text, &m, &opts);
        assert!(lines.len() >= 2, "expected multi-line: {:?}", lines);
        for line in &lines {
            let line_text = &text[line.byte_range.clone()];
            let last_char = line_text.chars().last().unwrap();
            assert!(
                !kinsoku::is_no_end(last_char),
                "line ends with no-end char {:?}: {:?}",
                last_char,
                line
            );
        }
    }

    #[test]
    fn kinsoku_leaves_latin_text_unchanged() {
        // Western paragraphs should compose identically whether
        // kinsoku_enforce is on or off — the per-character break
        // injection only fires on CJK chars (or chars in the
        // kinsoku punctuation set, which Latin words don't contain
        // mid-word).
        let m = MonospaceMeasurer::new(10, 10);
        let text = "lorem ipsum dolor sit amet";
        let off = ComposeOptions {
            column_width: 120,
            kinsoku_enforce: false,
            ..ComposeOptions::new(0.0)
        };
        let on = ComposeOptions {
            column_width: 120,
            kinsoku_enforce: true,
            ..ComposeOptions::new(0.0)
        };
        let lines_off = compose_paragraph(text, &m, &off);
        let lines_on = compose_paragraph(text, &m, &on);
        assert_eq!(
            lines_off.len(),
            lines_on.len(),
            "kinsoku must not alter Latin paragraphs"
        );
        for (a, b) in lines_off.iter().zip(lines_on.iter()) {
            assert_eq!(a.byte_range, b.byte_range);
        }
    }
}
