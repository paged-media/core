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

use std::collections::HashMap;

/// Resolve assets referenced by an IDML document.
///
/// Implementations should be cheap to clone or share — the pipeline
/// holds an `&dyn AssetResolver` and may call methods many times per
/// render (once per distinct font, image URI, or ICC profile name).
pub trait AssetResolver: Send + Sync {
    /// Resolve a font by family + style. `style` is IDML's
    /// `FontStyle` attribute (e.g. "Bold", "Italic", "Bold Italic")
    /// or `None` when the run carries no style.
    fn resolve_font(&self, family: &str, style: Option<&str>) -> Option<Vec<u8>>;

    /// Resolve a placed image by URI. Returns the raw bytes (PNG /
    /// JPEG / TIFF / etc.); decoding is the renderer's job.
    fn resolve_image(&self, uri: &str) -> Option<Vec<u8>>;

    /// Resolve an ICC profile by name. Used by `idml-color` for
    /// CMYK → linear-RGB conversion when the document specifies a
    /// non-default working space.
    fn resolve_icc(&self, name: &str) -> Option<Vec<u8>>;
}

/// In-memory `AssetResolver` backed by `HashMap`s. Useful for tests,
/// for embedding fonts in the binary, and as a building block for
/// hosts that want to pre-load assets before rendering.
#[derive(Debug, Default)]
pub struct BytesResolver {
    pub fonts: HashMap<String, Vec<u8>>,
    pub images: HashMap<String, Vec<u8>>,
    pub icc: HashMap<String, Vec<u8>>,
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
    pub fn add_font(&mut self, family: &str, style: Option<&str>, bytes: Vec<u8>) {
        self.fonts.insert(font_key(family, style), bytes);
    }

    pub fn add_image(&mut self, uri: impl Into<String>, bytes: Vec<u8>) {
        self.images.insert(uri.into(), bytes);
    }

    pub fn add_icc(&mut self, name: impl Into<String>, bytes: Vec<u8>) {
        self.icc.insert(name.into(), bytes);
    }
}

impl AssetResolver for BytesResolver {
    fn resolve_font(&self, family: &str, style: Option<&str>) -> Option<Vec<u8>> {
        self.fonts
            .get(&font_key(family, style))
            .cloned()
            .or_else(|| {
                // Fall through to the bare-family entry when the styled
                // variant isn't registered.
                self.fonts.get(family).cloned()
            })
    }

    fn resolve_image(&self, uri: &str) -> Option<Vec<u8>> {
        self.images.get(uri).cloned()
    }

    fn resolve_icc(&self, name: &str) -> Option<Vec<u8>> {
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
        assert_eq!(bytes, b"FONTBYTES");
    }

    #[test]
    fn missing_style_falls_back_to_bare_family() {
        let mut r = BytesResolver::new();
        r.add_font("Minion Pro", None, b"REG".to_vec());
        let bytes = r.resolve_font("Minion Pro", Some("Bold")).unwrap();
        assert_eq!(bytes, b"REG");
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
}
