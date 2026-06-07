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

//! Render diagnostics — structured, collectible signals for lossy or
//! degraded renders.
//!
//! Several render outcomes are *correct-but-lossy*: text that overflows
//! the last frame in a chain is clipped to match InDesign's exported PDF,
//! a missing image link renders a grey placeholder, page numbering falls
//! back to computed section rules when a page carries no baked `Name`.
//! Historically these were only `tracing::warn!`-logged and therefore
//! invisible to programmatic callers. A [`RenderDiagnostics`] rides along
//! on [`crate::pipeline::BuiltDocument`] so tooling (paged-inspect, the
//! editor) can surface them without scraping logs. The `tracing::warn!`
//! calls stay — this is additive.

use std::collections::BTreeMap;

/// Severity of a render diagnostic. Ordered so callers can filter
/// (`>= Warning`) and so `counts()` can bucket cheaply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// Stable, machine-matchable diagnostic category. New variants append at
/// the end so existing `by_code` consumers don't break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum DiagnosticCode {
    /// Laid-out lines fell past the last frame in a story's chain and
    /// were dropped. Matches InDesign's clipped PDF, but the text is not
    /// present in the output (overset).
    OversetTextDropped,
    /// A placed image's link could not be resolved to bytes; a grey
    /// placeholder was drawn instead.
    ImageLinkMissing,
    /// Image bytes were resolved but could not be decoded.
    ImageDecodeFailed,
    /// A placed image declared a `<ClippingPathSettings>` clip the
    /// renderer could not apply from the IDML XML alone — a Photoshop
    /// 8BIM path / alpha channel / detect-edges type (needs the image
    /// binary or raster analysis), or a named path with no inline
    /// geometry. The image is rendered clipped to the frame outline
    /// only (unclipped by the detached path).
    ImageClippingPathDeferred,
    /// A footnote body did not fit the space reserved at the bottom of
    /// its host frame and was clipped.
    FootnoteOverflow,
    /// A page carried no baked `Name`, so its label was computed from
    /// the document's `<Section>` numbering rules (or a 1-based fallback
    /// when no section applies).
    SectionNumberingFallback,
}

impl DiagnosticCode {
    /// Severity a freshly-constructed [`Diagnostic`] gets for this code.
    pub fn default_severity(self) -> Severity {
        match self {
            DiagnosticCode::OversetTextDropped => Severity::Warning,
            DiagnosticCode::ImageLinkMissing => Severity::Warning,
            DiagnosticCode::ImageDecodeFailed => Severity::Warning,
            DiagnosticCode::ImageClippingPathDeferred => Severity::Info,
            DiagnosticCode::FootnoteOverflow => Severity::Warning,
            DiagnosticCode::SectionNumberingFallback => Severity::Info,
        }
    }
}

/// One render diagnostic. `message` is pre-rendered for humans; the
/// structured fields let tooling group / filter without parsing it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    /// Flat body-page index the diagnostic pertains to, when known.
    pub page_index: Option<usize>,
    /// `<TextFrame>` / `<Rectangle>` self-id, when applicable.
    pub frame_id: Option<String>,
    /// `<Story Self="...">` id, when applicable.
    pub story_id: Option<String>,
    /// Image link URI, for the image codes.
    pub uri: Option<String>,
}

impl Diagnostic {
    /// Construct with the code's [`DiagnosticCode::default_severity`] and
    /// no location context; chain the `with_*` setters to attach it.
    pub fn new(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            severity: code.default_severity(),
            code,
            message: message.into(),
            page_index: None,
            frame_id: None,
            story_id: None,
            uri: None,
        }
    }

    pub fn with_page(mut self, page_index: usize) -> Self {
        self.page_index = Some(page_index);
        self
    }

    pub fn with_frame(mut self, frame_id: impl Into<String>) -> Self {
        self.frame_id = Some(frame_id.into());
        self
    }

    pub fn with_story(mut self, story_id: impl Into<String>) -> Self {
        self.story_id = Some(story_id.into());
        self
    }

    pub fn with_uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = Some(uri.into());
        self
    }
}

/// Collected render diagnostics for one document build. Empty for a
/// fully-faithful render.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct RenderDiagnostics {
    pub items: Vec<Diagnostic>,
}

impl RenderDiagnostics {
    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// `(errors, warnings, infos)`.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut c = (0usize, 0usize, 0usize);
        for d in &self.items {
            match d.severity {
                Severity::Error => c.0 += 1,
                Severity::Warning => c.1 += 1,
                Severity::Info => c.2 += 1,
            }
        }
        c
    }

    /// Number of diagnostics per code, ordered by code. Drives the
    /// paged-inspect summary.
    pub fn by_code(&self) -> BTreeMap<DiagnosticCode, usize> {
        let mut m = BTreeMap::new();
        for d in &self.items {
            *m.entry(d.code).or_insert(0) += 1;
        }
        m
    }

    /// Distinct `<Story Self="...">` ids reported overset (text
    /// dropped past the last frame in their chain). The overset
    /// diagnostic is fired once per story at emit time (see
    /// `StoryEmitter::overset_reported`), so this is already
    /// deduplicated, but we collect into a `BTreeSet` for stable
    /// order regardless of build/page-visit order. Backs the editor
    /// Preflight + per-story overset flag (panels.md gap 1).
    pub fn overset_story_ids(&self) -> std::collections::BTreeSet<String> {
        self.items
            .iter()
            .filter(|d| d.code == DiagnosticCode::OversetTextDropped)
            .filter_map(|d| d.story_id.clone())
            .collect()
    }

    /// Distinct host-frame `Self` ids whose placed image could not be
    /// resolved or decoded at build time (`ImageLinkMissing` /
    /// `ImageDecodeFailed`). The build draws the grey missing-image
    /// placeholder for these, so they're exactly the links the Links
    /// panel should mark `"missing"` (panels.md gap 2).
    pub fn missing_image_frame_ids(&self) -> std::collections::BTreeSet<String> {
        self.items
            .iter()
            .filter(|d| {
                matches!(
                    d.code,
                    DiagnosticCode::ImageLinkMissing | DiagnosticCode::ImageDecodeFailed
                )
            })
            .filter_map(|d| d.frame_id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_by_code_bucket_correctly() {
        let mut diags = RenderDiagnostics::default();
        diags.push(Diagnostic::new(DiagnosticCode::OversetTextDropped, "a").with_page(0));
        diags.push(Diagnostic::new(DiagnosticCode::ImageLinkMissing, "b").with_uri("x.png"));
        diags.push(Diagnostic::new(
            DiagnosticCode::SectionNumberingFallback,
            "c",
        ));
        let (errors, warnings, infos) = diags.counts();
        assert_eq!((errors, warnings, infos), (0, 2, 1));
        let by_code = diags.by_code();
        assert_eq!(by_code[&DiagnosticCode::OversetTextDropped], 1);
        assert_eq!(by_code[&DiagnosticCode::ImageLinkMissing], 1);
        assert_eq!(by_code[&DiagnosticCode::SectionNumberingFallback], 1);
    }

    #[test]
    fn builder_attaches_context() {
        let d = Diagnostic::new(DiagnosticCode::ImageLinkMissing, "missing")
            .with_page(3)
            .with_frame("frame-7")
            .with_uri("links/photo.jpg");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.page_index, Some(3));
        assert_eq!(d.frame_id.as_deref(), Some("frame-7"));
        assert_eq!(d.uri.as_deref(), Some("links/photo.jpg"));
    }
}
