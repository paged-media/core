//! [`Value<T>`] — scaffolding for future binding support.
//!
//! Today every property in the scene graph is a literal value: a
//! `Color`, an `f32`, a `String`. Bindings are deferred (see
//! `docs/inspector.md` Track B). But the *shape* of a property value
//! should already be an enum that *can* gain non-literal variants
//! without rewriting every property type in the scene graph.
//!
//! `Value::Literal(T)` is the only constructible variant right now.
//! When binding support arrives, a `Computed { source, query, ... }`
//! variant will slot in alongside, and the existing literal sites
//! stay untouched.
//!
//! The enum is the *inspector API's* property type, not the parse
//! types — parse types stay concrete (`Color`, `f32`, …). The
//! introspection layer lifts them into `Value<T>` at the boundary.
//! This keeps the 10k-call-site refactor of `paged-parse` off the
//! table.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Value<T> {
    Literal(T),
}

impl<T> Value<T> {
    pub fn as_literal(&self) -> Option<&T> {
        match self {
            Value::Literal(v) => Some(v),
        }
    }

    /// Unwrap the literal. Panics if a non-literal variant is ever
    /// added and constructed and this path is hit; until then the
    /// match is exhaustive and `Literal` is the only arm.
    pub fn expect_literal(&self) -> &T {
        match self {
            Value::Literal(v) => v,
        }
    }
}

impl<T> From<T> for Value<T> {
    fn from(value: T) -> Self {
        Value::Literal(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_round_trip() {
        let v: Value<i32> = 42.into();
        assert_eq!(v.as_literal(), Some(&42));
        assert_eq!(*v.expect_literal(), 42);
    }
}
