//! Asset resolution interface (idea.md §11.1).
//!
//! Hosts pass document-relative font / image / ICC-profile lookups
//! to the renderer through this trait. The current pipeline only
//! consumes one font (passed via `PipelineOptions::font` for
//! back-compat) but the trait surface is here so the multi-font
//! refactor can land without an API rewrite.
//!
//! Today's surface is sync. The browser binding (idml-wasm) wraps
//! it with a JS Promise resolver at the language boundary; the
//! browser's `AssetResolver` interface in idea.md §11.1 stays
//! Promise-based externally.
//!
//! Returned bytes are `bytes::Bytes` so cloning is a refcount bump,
//! not a memcpy — fonts and image payloads are routinely megabytes.

use std::collections::HashMap;

use bytes::Bytes;

/// Resolve assets referenced by an IDML document.
///
/// Implementations should be cheap to clone or share — the pipeline
/// holds an `&dyn AssetResolver` and may call methods many times per
/// render (once per distinct font, image URI, or ICC profile name).
pub trait AssetResolver: Send + Sync {
    /// Resolve a font by family + style. `style` is IDML's
    /// `FontStyle` attribute (e.g. "Bold", "Italic", "Bold Italic")
    /// or `None` when the run carries no style.
    fn resolve_font(&self, family: &str, style: Option<&str>) -> Option<Bytes>;

    /// Resolve a placed image by URI.
    fn resolve_image(&self, uri: &str) -> Option<Bytes>;

    /// Resolve an ICC profile by name. Used by `idml-color` for
    /// CMYK → linear-RGB conversion when the document specifies a
    /// non-default working space.
    fn resolve_icc(&self, name: &str) -> Option<Bytes>;
}

/// In-memory `AssetResolver` backed by `HashMap`s. Useful for tests,
/// for embedding fonts in the binary, and as a building block for
/// hosts that want to pre-load assets before rendering.
#[derive(Debug, Default)]
pub struct BytesResolver {
    pub fonts: HashMap<String, Bytes>,
    pub images: HashMap<String, Bytes>,
    pub icc: HashMap<String, Bytes>,
    /// Returned when a font lookup misses both the styled and bare
    /// family entries. Useful for rendering documents whose fonts
    /// the host can't ship (Adobe-licensed Minion / Caslon / etc.) —
    /// callers register a permissively-licensed substitute and every
    /// run shapes through that. `None` ⇒ unresolved fonts return
    /// `None` and the renderer falls back to its `font` option.
    pub default_font: Option<Bytes>,
}

impl BytesResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a font under its family name. `style` is concatenated
    /// to the family with a single space when looking up: this
    /// matches IDML's "Helvetica Neue" + "Bold" → "Helvetica Neue Bold"
    /// convention used by AppliedFont without spaces between the
    /// two halves.
    pub fn add_font(&mut self, family: &str, style: Option<&str>, bytes: impl Into<Bytes>) {
        self.fonts.insert(font_key(family, style), bytes.into());
    }

    /// Set the catch-all font returned when a `(family, style)` lookup
    /// misses every registered entry. Builder-style for chaining.
    pub fn with_default_font(mut self, bytes: impl Into<Bytes>) -> Self {
        self.default_font = Some(bytes.into());
        self
    }

    pub fn add_image(&mut self, uri: impl Into<String>, bytes: impl Into<Bytes>) {
        self.images.insert(uri.into(), bytes.into());
    }

    pub fn add_icc(&mut self, name: impl Into<String>, bytes: impl Into<Bytes>) {
        self.icc.insert(name.into(), bytes.into());
    }
}

impl AssetResolver for BytesResolver {
    fn resolve_font(&self, family: &str, style: Option<&str>) -> Option<Bytes> {
        self.fonts
            .get(&font_key(family, style))
            .cloned()
            // Fall through to the bare-family entry when the styled
            // variant isn't registered.
            .or_else(|| self.fonts.get(family).cloned())
            // …then to the document-wide default font.
            .or_else(|| self.default_font.clone())
    }

    fn resolve_image(&self, uri: &str) -> Option<Bytes> {
        self.images.get(uri).cloned()
    }

    fn resolve_icc(&self, name: &str) -> Option<Bytes> {
        self.icc.get(name).cloned()
    }
}

fn font_key(family: &str, style: Option<&str>) -> String {
    match style {
        Some(s) if !s.is_empty() && s != "Regular" => format!("{family} {s}"),
        _ => family.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_font_round_trips() {
        let mut r = BytesResolver::new();
        r.add_font("Helvetica Neue", Some("Bold"), b"FONTBYTES".to_vec());
        let bytes = r.resolve_font("Helvetica Neue", Some("Bold")).unwrap();
        assert_eq!(&bytes[..], b"FONTBYTES");
    }

    #[test]
    fn missing_style_falls_back_to_bare_family() {
        let mut r = BytesResolver::new();
        r.add_font("Minion Pro", None, b"REG".to_vec());
        let bytes = r.resolve_font("Minion Pro", Some("Bold")).unwrap();
        assert_eq!(&bytes[..], b"REG");
    }

    #[test]
    fn regular_style_uses_bare_family_key() {
        let mut r = BytesResolver::new();
        r.add_font("Caslon", Some("Regular"), b"REG".to_vec());
        // "Regular" maps to the bare key; explicit Bold still falls back.
        assert!(r.resolve_font("Caslon", None).is_some());
        assert!(r.resolve_font("Caslon", Some("Bold")).is_some());
    }

    #[test]
    fn unknown_asset_returns_none() {
        let r = BytesResolver::new();
        assert!(r.resolve_font("Nope", None).is_none());
        assert!(r.resolve_image("missing.png").is_none());
        assert!(r.resolve_icc("nope").is_none());
    }

    #[test]
    fn default_font_serves_unknown_families() {
        let r = BytesResolver::new().with_default_font(b"FALLBACK".to_vec());
        let bytes = r.resolve_font("Minion Pro", Some("Bold")).unwrap();
        assert_eq!(&bytes[..], b"FALLBACK");
    }

    #[test]
    fn registered_family_wins_over_default_font() {
        let mut r = BytesResolver::new().with_default_font(b"DEFAULT".to_vec());
        r.add_font("Inter", None, b"INTER".to_vec());
        assert_eq!(&r.resolve_font("Inter", None).unwrap()[..], b"INTER");
        assert_eq!(&r.resolve_font("Other", None).unwrap()[..], b"DEFAULT");
    }
}
