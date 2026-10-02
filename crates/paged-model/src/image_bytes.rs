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

//! How a page item's `image_bytes` are serialized.
//!
//! The derived serde shape of an `Option<Vec<u8>>` is a JSON array of
//! integers: one decimal number per byte. A `.paged` whose model held 29
//! photos carried a 211 MB `document.pgm` (67 MB zipped) that every load
//! parsed and every save wrote: 11.3 s to save, 1.9 s to load.
//!
//! This adapter writes the bytes one of two ways:
//!
//! * **as a blob reference**, `{"blob":"<name>"}`, while a collector is
//!   active ([`collect`]): the bytes go to the collector, which the
//!   container writer stores as parts of their own, and the model part
//!   holds only the name;
//! * **as a base64 string** otherwise, so a model serialized on its own
//!   stays self-contained.
//!
//! It reads all three shapes: a blob reference (resolved from the blobs
//! handed to [`resolve`]), a base64 string, and the old integer array, so
//! parts written before this change still load.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use base64::Engine as _;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserializer, Serializer};

const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

struct Collector {
    name: fn(&[u8]) -> String,
    blobs: BTreeMap<String, Vec<u8>>,
}

thread_local! {
    static COLLECTOR: RefCell<Option<Collector>> = const { RefCell::new(None) };
    static BLOBS: RefCell<Option<HashMap<String, Vec<u8>>>> = const { RefCell::new(None) };
}

/// Run `f` (a serialization) with image bytes collected as blobs instead
/// of written inline. `name` names a blob from its bytes — a content
/// hash, so an unchanged image keeps its name across saves and identical
/// images share one blob. Returns `f`'s result and the blobs by name.
pub fn collect<T>(
    name: fn(&[u8]) -> String,
    f: impl FnOnce() -> T,
) -> (T, BTreeMap<String, Vec<u8>>) {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            COLLECTOR.with(|c| c.borrow_mut().take());
        }
    }
    COLLECTOR.with(|c| {
        *c.borrow_mut() = Some(Collector {
            name,
            blobs: BTreeMap::new(),
        })
    });
    let guard = Clear;
    let out = f();
    let blobs = COLLECTOR
        .with(|c| c.borrow_mut().take())
        .map(|c| c.blobs)
        .unwrap_or_default();
    drop(guard);
    (out, blobs)
}

/// Run `f` (a deserialization) with `blobs` available to resolve blob
/// references by name. A reference to a blob that is not there fails the
/// deserialization rather than loading an image-less frame.
pub fn resolve<T>(blobs: HashMap<String, Vec<u8>>, f: impl FnOnce() -> T) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            BLOBS.with(|b| b.borrow_mut().take());
        }
    }
    BLOBS.with(|b| *b.borrow_mut() = Some(blobs));
    let _guard = Clear;
    f()
}

pub fn serialize<S: Serializer>(value: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error> {
    let Some(bytes) = value else {
        return serializer.serialize_none();
    };
    let name = COLLECTOR.with(|c| {
        c.borrow_mut().as_mut().map(|c| {
            let name = (c.name)(bytes);
            c.blobs.entry(name.clone()).or_insert_with(|| bytes.clone());
            name
        })
    });
    match name {
        Some(name) => {
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry("blob", &name)?;
            map.end()
        }
        None => serializer.serialize_str(&BASE64.encode(bytes)),
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<u8>>, D::Error> {
    struct Bytes;
    impl<'de> Visitor<'de> for Bytes {
        type Value = Option<Vec<u8>>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str(
                "image bytes: null, a base64 string, a blob reference, or an array of bytes",
            )
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_any(Bytes)
        }

        fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
            BASE64.decode(s).map(Some).map_err(E::custom)
        }

        // The shape written before this adapter: one number per byte.
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(b) = seq.next_element::<u8>()? {
                bytes.push(b);
            }
            Ok(Some(bytes))
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut name: Option<String> = None;
            while let Some(key) = map.next_key::<String>()? {
                if key == "blob" {
                    name = Some(map.next_value()?);
                } else {
                    map.next_value::<de::IgnoredAny>()?;
                }
            }
            let name = name.ok_or_else(|| de::Error::missing_field("blob"))?;
            BLOBS
                .with(|b| b.borrow().as_ref().and_then(|b| b.get(&name).cloned()))
                .map(Some)
                .ok_or_else(|| {
                    de::Error::custom(format!("image blob {name:?} is not in the container"))
                })
        }
    }
    deserializer.deserialize_any(Bytes)
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Item {
        #[serde(default, with = "super")]
        image_bytes: Option<Vec<u8>>,
    }

    fn item(bytes: &[u8]) -> Item {
        Item {
            image_bytes: Some(bytes.to_vec()),
        }
    }

    #[test]
    fn inline_bytes_are_base64_and_round_trip() {
        let json = serde_json::to_string(&item(&[1, 2, 3])).unwrap();
        assert_eq!(json, r#"{"image_bytes":"AQID"}"#);
        assert_eq!(
            serde_json::from_str::<Item>(&json).unwrap(),
            item(&[1, 2, 3])
        );
    }

    #[test]
    fn the_old_integer_array_still_loads() {
        let old: Item = serde_json::from_str(r#"{"image_bytes":[255,216,255]}"#).unwrap();
        assert_eq!(old, item(&[255, 216, 255]));
    }

    #[test]
    fn absent_and_null_are_none() {
        let none = Item { image_bytes: None };
        assert_eq!(
            serde_json::to_string(&none).unwrap(),
            r#"{"image_bytes":null}"#
        );
        assert_eq!(
            serde_json::from_str::<Item>(r#"{"image_bytes":null}"#).unwrap(),
            none
        );
        assert_eq!(serde_json::from_str::<Item>("{}").unwrap(), none);
    }

    #[test]
    fn collected_bytes_leave_only_a_name_and_identical_images_share_a_blob() {
        let items = vec![item(&[9, 9]), item(&[9, 9]), item(&[7])];
        let (json, blobs) = super::collect(
            |b| format!("n{}", b.iter().map(|x| *x as u32).sum::<u32>()),
            || serde_json::to_string(&items).unwrap(),
        );
        assert_eq!(
            json,
            r#"[{"image_bytes":{"blob":"n18"}},{"image_bytes":{"blob":"n18"}},{"image_bytes":{"blob":"n7"}}]"#
        );
        assert_eq!(blobs.len(), 2, "two identical images, one blob");

        let back: Vec<Item> = super::resolve(blobs.into_iter().collect(), || {
            serde_json::from_str(&json).unwrap()
        });
        assert_eq!(back, items);
        // The collector is gone once `collect` returns.
        assert_eq!(
            serde_json::to_string(&item(&[1, 2, 3])).unwrap(),
            r#"{"image_bytes":"AQID"}"#
        );
    }

    #[test]
    fn a_reference_to_a_missing_blob_is_an_error_not_an_empty_frame() {
        let err = serde_json::from_str::<Item>(r#"{"image_bytes":{"blob":"gone"}}"#).unwrap_err();
        assert!(err.to_string().contains("gone"), "{err}");
    }
}
