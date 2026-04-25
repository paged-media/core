//! Tiny shared helpers used by every per-format parser in this crate.

/// Read an XML attribute by key. Returns `None` when absent or
/// non-UTF-8. Each parser submodule used to define its own copy of
/// this — `lib.rs` re-exports it here so they all share one.
pub(crate) fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| std::str::from_utf8(&a.value).ok().map(str::to_string))
}
