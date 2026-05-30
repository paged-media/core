//! Per-paragraph layout cache.
//!
//! Phase 4 step 1 — `layout_runs` / `layout_paragraph` are pure
//! functions of (text + styling + column geometry + font identity).
//! When a single mutation only touches one paragraph, the other ~N-1
//! paragraphs in the document re-arrive at the K-P engine byte-for-byte
//! identical. Caching their `LaidOutParagraph` results lets a mutation-
//! triggered rebuild skip Knuth-Plass for every unchanged paragraph,
//! collapsing the dominant rebuild cost from O(stories × paragraphs) to
//! O(touched paragraphs).
//!
//! The cache key is a 32-byte blake3 digest folded by [`LayoutKeyHasher`].
//! Callers add every input that affects the laid-out output in a fixed
//! order; salting with the constant tag string guards against accidental
//! collisions with other digests stored in the same workspace (the
//! scene-level `Document::canonical_hash` uses a different domain
//! prefix).
//!
//! The cache itself is a bounded `HashMap` keyed by digest. There's no
//! LRU yet — when the entry count exceeds `capacity` the cache clears
//! itself entirely on the next insert. The simpler policy is good
//! enough for the first cut because cache size is bounded by the count
//! of distinct paragraph shapes in the active document (typically
//! thousands, well below the default 10 000 cap), and a typing session
//! never grows the working set fast enough to thrash. LRU is queued for
//! the moment we see real cache pressure on a 500-page corpus.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::compose::ComposeOptions;
use crate::layout::{layout_runs, Alignment, LaidOutParagraph, LayoutOptions, StyledRun};
use crate::shape::{KerningMethod, ShapingFeatures};

/// Bounded per-paragraph layout cache.
#[derive(Debug)]
pub struct LayoutCache {
    entries: HashMap<[u8; 32], LaidOutParagraph>,
    capacity: usize,
    hits: u64,
    misses: u64,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub len: usize,
    pub capacity: usize,
}

impl LayoutCache {
    /// Build an empty cache capped at `capacity` entries. A capacity of
    /// 0 disables caching (every lookup misses, every insert is a
    /// no-op) — useful for diff-baseline benchmarking.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity,
            hits: 0,
            misses: 0,
        }
    }

    /// Lookup. Increments hits/misses; returns a clone of the entry on
    /// hit (cheap — `LaidOutParagraph` is glyphs + line metadata, no
    /// nested allocations of significance).
    pub fn get(&mut self, key: &[u8; 32]) -> Option<LaidOutParagraph> {
        if self.capacity == 0 {
            self.misses += 1;
            return None;
        }
        if let Some(p) = self.entries.get(key) {
            self.hits += 1;
            return Some(p.clone());
        }
        self.misses += 1;
        None
    }

    /// Insert. If the cache is full, drop everything before adding —
    /// see module docs on the no-LRU policy.
    pub fn insert(&mut self, key: [u8; 32], value: LaidOutParagraph) {
        if self.capacity == 0 {
            return;
        }
        if self.entries.len() >= self.capacity {
            self.entries.clear();
        }
        self.entries.insert(key, value);
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits,
            misses: self.misses,
            len: self.entries.len(),
            capacity: self.capacity,
        }
    }

    pub fn reset_stats(&mut self) {
        self.hits = 0;
        self.misses = 0;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.reset_stats();
    }
}

impl Default for LayoutCache {
    fn default() -> Self {
        // 10 000 entries × ~2 KB per LaidOutParagraph ≈ 20 MB ceiling.
        // Generous; tighten when measured.
        Self::new(10_000)
    }
}

/// Folds the inputs of a layout call into a 32-byte cache key. Add
/// every input that affects the output in a fixed order — order is
/// part of the key.
///
/// Typical usage:
/// ```ignore
/// let mut h = LayoutKeyHasher::new("layout_runs");
/// for r in runs {
///     h.add_str(r.text);
///     h.add_u32(r.font_id);
///     h.add_f32(r.point_size);
///     // …every field that matters…
/// }
/// h.add_layout_options(options);
/// let key = h.finalize();
/// ```
pub struct LayoutKeyHasher {
    hasher: blake3::Hasher,
}

impl LayoutKeyHasher {
    /// Initialise with a domain tag. Different tags produce different
    /// hashes for the same input so single-font and multi-font layouts
    /// don't accidentally share cache entries.
    pub fn new(tag: &str) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"paged-text/layout-cache/v1/");
        hasher.update(tag.as_bytes());
        hasher.update(b":");
        Self { hasher }
    }

    /// Hash a length-prefixed UTF-8 string. Length-prefix prevents
    /// adjacent strings from concatenating into the same digest.
    pub fn add_str(&mut self, s: &str) {
        self.add_u64(s.len() as u64);
        self.hasher.update(s.as_bytes());
    }

    /// Hash an opaque byte slice (length-prefixed).
    pub fn add_bytes(&mut self, b: &[u8]) {
        self.add_u64(b.len() as u64);
        self.hasher.update(b);
    }

    pub fn add_bool(&mut self, v: bool) {
        self.hasher.update(&[v as u8]);
    }

    pub fn add_i32(&mut self, v: i32) {
        self.hasher.update(&v.to_le_bytes());
    }

    pub fn add_u32(&mut self, v: u32) {
        self.hasher.update(&v.to_le_bytes());
    }

    pub fn add_u64(&mut self, v: u64) {
        self.hasher.update(&v.to_le_bytes());
    }

    /// Hash a `f32` via its bit representation. NaN inputs would produce
    /// unstable hashes; the layout pipeline never feeds NaNs (column
    /// width, point size, tracking are always finite) so this is fine.
    pub fn add_f32(&mut self, v: f32) {
        self.hasher.update(&v.to_bits().to_le_bytes());
    }

    pub fn add_optional_i32(&mut self, v: Option<i32>) {
        match v {
            Some(x) => {
                self.add_bool(true);
                self.add_i32(x);
            }
            None => self.add_bool(false),
        }
    }

    /// Field separator. Insert between logical groups so a longer
    /// `&str` field can't bleed into the next field's bytes.
    pub fn sep(&mut self) {
        self.hasher.update(b"|");
    }

    pub fn finalize(self) -> [u8; 32] {
        *self.hasher.finalize().as_bytes()
    }
}

// ────────────────────────────────────────────────────────────────────
// Thread-local install/take API.
//
// Pipeline call sites use `layout_runs_cached(...)`. When a cache is
// installed on the current thread it serves cache hits and writes
// back fresh results; when no cache is installed every call falls
// through to plain `layout_runs(...)` (zero-cost wrapper). The
// pipeline doesn't need to know whether caching is active.
//
// CanvasModel installs its cache once per rebuild via
// `with_layout_cache(cache, || pipeline::build_document(...))`. The
// helper takes the cache back at the end so the caller can read its
// stats; nesting is supported (an inner `with_layout_cache` shadows
// the outer for its scope).
// ────────────────────────────────────────────────────────────────────

thread_local! {
    static ACTIVE_CACHE: RefCell<Option<LayoutCache>> = const { RefCell::new(None) };
}

/// Install `cache` as the active layout cache for the current thread,
/// run `f`, and take the cache back so the caller can read its stats.
/// Re-entrant: an inner `with_layout_cache` saves the outer cache,
/// installs the inner cache for its scope, then restores.
pub fn with_layout_cache<R>(cache: LayoutCache, f: impl FnOnce() -> R) -> (R, LayoutCache) {
    let prev = ACTIVE_CACHE.with(|slot| slot.replace(Some(cache)));
    let result = f();
    let used = ACTIVE_CACHE.with(|slot| slot.replace(prev));
    (
        result,
        used.expect("active cache disappeared mid-`with_layout_cache`"),
    )
}

/// Internal — try the active cache; on miss compute via `f` and store.
/// When no cache is installed, just calls `f`.
fn cached<F: FnOnce() -> LaidOutParagraph>(key: [u8; 32], f: F) -> LaidOutParagraph {
    let hit = ACTIVE_CACHE.with(|slot| slot.borrow_mut().as_mut().and_then(|c| c.get(&key)));
    if let Some(p) = hit {
        return p;
    }
    let result = f();
    ACTIVE_CACHE.with(|slot| {
        if let Some(c) = slot.borrow_mut().as_mut() {
            c.insert(key, result.clone());
        }
    });
    result
}

/// Cached flavour of [`layout_runs`]. When a layout cache is installed
/// on the current thread, this short-circuits cached entries; otherwise
/// it behaves identically to the plain function. Pipeline call sites
/// use this so installing a cache is a one-line opt-in at the top.
pub fn layout_runs_cached(runs: &[StyledRun], options: &LayoutOptions) -> LaidOutParagraph {
    let key = layout_runs_key(runs, options);
    cached(key, || layout_runs(runs, options))
}

/// Fold the inputs of [`layout_runs`] into a 32-byte cache key.
///
/// Hashed inputs (in order):
/// - Per run: text, `font_id`, point size, tracking, underline,
///   strikethru, baseline_shift_pt, horizontal_scale_pct,
///   fallback-face count (not contents — see module docs).
/// - LayoutOptions: alignment, line_height, first_baseline,
///   leading_override.
/// - ComposeOptions: column_width, column_widths, tolerance, looseness,
///   stretch_ratio, shrink_ratio, desired_space_ratio,
///   hyphenator language id, hyphen_penalty, kinsoku_enforce.
///
/// **Caching contract assumption.** `font_id` is treated as a stable
/// per-font identifier within a render. If the caller maps different
/// fonts to the same id (a bug), the cache will alias them — same as
/// any cache keyed on a logical id. Fallback faces are not hashed by
/// content because their content depends only on the configured font
/// set, which is stable per render.
pub fn layout_runs_key(runs: &[StyledRun], options: &LayoutOptions) -> [u8; 32] {
    let mut h = LayoutKeyHasher::new("layout_runs");
    h.add_u32(runs.len() as u32);
    for r in runs {
        h.add_str(r.text);
        h.add_u32(r.font_id);
        h.add_f32(r.point_size);
        h.add_optional_f32(r.tracking);
        h.add_bool(r.underline);
        h.add_bool(r.strikethru);
        h.add_f32(r.baseline_shift_pt);
        h.add_f32(r.horizontal_scale_pct);
        h.add_u32(r.fallback_faces.len() as u32);
        h.add_bool(r.shaping_features.ligatures_on);
        h.add_u32(match r.shaping_features.kerning {
            KerningMethod::Metrics => 0,
            KerningMethod::Optical => 1,
            KerningMethod::Off => 2,
        });
        h.sep();
    }
    fold_layout_options(&mut h, options);
    h.finalize()
}

fn fold_layout_options(h: &mut LayoutKeyHasher, options: &LayoutOptions) {
    h.sep();
    h.add_i32(options.line_height);
    h.add_i32(options.first_baseline);
    h.add_optional_i32(options.leading_override);
    h.add_u32(alignment_tag(options.alignment));
    fold_compose_options(h, &options.compose);
}

fn fold_compose_options(h: &mut LayoutKeyHasher, options: &ComposeOptions) {
    h.sep();
    h.add_i32(options.column_width);
    match &options.column_widths {
        Some(ws) => {
            h.add_bool(true);
            h.add_u32(ws.len() as u32);
            for w in ws {
                h.add_i32(*w);
            }
        }
        None => h.add_bool(false),
    }
    h.add_f32(options.tolerance);
    h.add_i32(options.looseness);
    h.add_f32(options.stretch_ratio);
    h.add_f32(options.shrink_ratio);
    h.add_f32(options.desired_space_ratio);
    match options.hyphenator {
        Some(hyph) => {
            h.add_bool(true);
            h.add_u32(hyph.lang_id() as u32);
        }
        None => h.add_bool(false),
    }
    h.add_i32(options.hyphen_penalty);
    h.add_bool(options.kinsoku_enforce);
}

fn alignment_tag(a: Alignment) -> u32 {
    match a {
        Alignment::Left => 0,
        Alignment::Right => 1,
        Alignment::Center => 2,
        Alignment::Justify => 3,
    }
}

impl LayoutKeyHasher {
    pub fn add_optional_f32(&mut self, v: Option<f32>) {
        match v {
            Some(x) => {
                self.add_bool(true);
                self.add_f32(x);
            }
            None => self.add_bool(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cache_returns_none() {
        let mut c = LayoutCache::new(8);
        assert!(c.get(&[0; 32]).is_none());
        assert_eq!(c.stats().misses, 1);
        assert_eq!(c.stats().hits, 0);
    }

    #[test]
    fn round_trip_hits() {
        let mut c = LayoutCache::new(8);
        let key = [42u8; 32];
        let p = LaidOutParagraph { lines: Vec::new() };
        c.insert(key, p);
        assert!(c.get(&key).is_some());
        assert_eq!(c.stats().hits, 1);
    }

    #[test]
    fn full_cache_clears_on_overflow() {
        let mut c = LayoutCache::new(2);
        for i in 0..3u8 {
            let mut key = [0u8; 32];
            key[0] = i;
            c.insert(key, LaidOutParagraph { lines: Vec::new() });
        }
        // After the third insert, the cap was reached, the cache
        // cleared, and only the third entry remains.
        assert_eq!(c.stats().len, 1);
    }

    #[test]
    fn disabled_cache_never_stores() {
        let mut c = LayoutCache::new(0);
        c.insert([1; 32], LaidOutParagraph { lines: Vec::new() });
        assert_eq!(c.stats().len, 0);
        assert!(c.get(&[1; 32]).is_none());
    }

    #[test]
    fn key_hasher_differentiates_inputs() {
        let mut h1 = LayoutKeyHasher::new("layout_runs");
        h1.add_str("hello");
        h1.add_i32(42);
        let k1 = h1.finalize();

        let mut h2 = LayoutKeyHasher::new("layout_runs");
        h2.add_str("hello");
        h2.add_i32(43);
        let k2 = h2.finalize();

        assert_ne!(k1, k2);
    }

    #[test]
    fn key_hasher_str_length_prefix_prevents_collision() {
        // Without length-prefixing, "ab" + "cd" would equal "a" + "bcd".
        let mut h1 = LayoutKeyHasher::new("t");
        h1.add_str("ab");
        h1.add_str("cd");
        let k1 = h1.finalize();

        let mut h2 = LayoutKeyHasher::new("t");
        h2.add_str("a");
        h2.add_str("bcd");
        let k2 = h2.finalize();

        assert_ne!(k1, k2);
    }

    #[test]
    fn different_tags_produce_different_keys() {
        let h1 = LayoutKeyHasher::new("layout_runs").finalize();
        let h2 = LayoutKeyHasher::new("layout_paragraph").finalize();
        assert_ne!(h1, h2);
    }

    #[test]
    fn with_layout_cache_returns_cache_back() {
        let cache = LayoutCache::new(8);
        let (sum, taken) = with_layout_cache(cache, || 1 + 1);
        assert_eq!(sum, 2);
        assert_eq!(taken.stats().len, 0);
    }

    #[test]
    fn layout_runs_cached_no_active_cache_does_not_panic() {
        // No `with_layout_cache` wrap → call falls through to plain
        // `layout_runs` (the runs slice is empty so this is cheap).
        let empty: [StyledRun; 0] = [];
        let opts = LayoutOptions::new(400.0, 12.0);
        let p = layout_runs_cached(&empty, &opts);
        assert!(p.lines.is_empty());
    }
}
