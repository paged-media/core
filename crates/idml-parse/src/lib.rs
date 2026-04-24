//! IDML parser.
//!
//! Consumes an IDML ZIP archive and produces a typed AST. Schema coverage
//! is driven by the reference-reading week described in the development
//! plan (Scribus `importidmlplugin.cpp`, SimpleIDML, Adobe's IDML spec).
//!
//! This crate is a skeleton; see the development plan for scope.

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("not implemented yet")]
    NotImplemented,
}
