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

/// Like [`attr`], but XML-NORMALIZES the value (entity unescaping) (`&quot;` → `"`,
/// `&amp;` → `&`, …). Required for free-text carriers — Label
/// `KeyValuePair` values hold JSON, which InDesign serialises with
/// escaped quotes. The plain [`attr`] stays raw because the numeric /
/// enum attributes it reads never contain entities.
pub(crate) fn attr_unescaped(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| {
            // IDML is XML 1.0 (every part declares it).
            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

/// Parse an `f32` attribute by key. Returns `None` when the
/// attribute is absent, malformed, or non-finite. Convenience
/// wrapper used by the IDML effect parsers (XOffset, Size, Opacity,
/// Angle, etc.) to dedupe the `attr(...).and_then(|s| s.parse().ok())`
/// pattern that appeared 60+ times across the spread + styles
/// parsers.
pub(crate) fn parse_f(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<f32> {
    attr(e, key)?.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// Parse an IDML tint percentage attribute (FillTint, StrokeTint).
///
/// Convention:
///   * absent or `-1`  → `None` (no override; use the swatch as-is).
///   * `0..=100`       → `Some(pct)`; 100 = full strength.
///
/// Out-of-range values return `None` so a malformed document can't
/// silently distort the renderer's output.
pub(crate) fn parse_tint_attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<f32> {
    let raw = attr(e, key)?;
    let v: f32 = raw.parse().ok()?;
    if !(0.0..=100.0).contains(&v) {
        return None;
    }
    Some(v)
}
