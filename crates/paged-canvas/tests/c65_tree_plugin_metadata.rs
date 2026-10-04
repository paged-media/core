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

//! C-65 — one scene-tree read carries every item's plugin metadata, so a
//! plugin finds its links without a property read per leaf.

use std::io::Write;

use paged_canvas::{channel::Mutation, element_selection::ElementId, CanvasModel, CanvasOptions};
use paged_mutate::{PropertyPath, Value};

fn idml() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/vnd.adobe.indesign-idml-package")
            .unwrap();
        zip.start_file("designmap.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Document DOMVersion="20.0" Self="d1">
<idPkg:Spread src="Spreads/Spread_s1.xml" xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging"/>
</Document>"#,
        )
        .unwrap();
        zip.start_file("Spreads/Spread_s1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<idPkg:Spread xmlns:idPkg="http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging" DOMVersion="20.0">
<Spread Self="s1" PageCount="1">
<Page Self="p1" Name="1" GeometricBounds="0 0 792 612" ItemTransform="1 0 0 1 0 0"/>
<Polygon Self="pen" ItemTransform="1 0 0 1 0 0" StrokeColor="Color/Black" StrokeWeight="4" EndCap="RoundEndCap" LeftLineEnd="CircleSolidArrowHead" RightLineEnd="None"><Properties><PathGeometry><GeometryPathType PathOpen="true"><PathPointArray><PathPointType Anchor="100 100" LeftDirection="100 100" RightDirection="100 100"/><PathPointType Anchor="200 160" LeftDirection="200 160" RightDirection="200 160"/></PathPointArray></GeometryPathType></PathGeometry></Properties></Polygon>
<Oval Self="ov" GeometricBounds="400 100 460 160" ItemTransform="1 0 0 1 0 0" StrokeColor="Color/Black" StrokeWeight="2" EndCap="ButtEndCap"/>
</Spread></idPkg:Spread>"#,
        )
        .unwrap();
        zip.finish().unwrap();
    }
    buf
}

fn model() -> CanvasModel {
    CanvasModel::load("d1", &idml(), CanvasOptions::default()).expect("load")
}

fn node<'a>(
    nodes: &'a [paged_canvas::channel::SceneTreeNode],
    want: &ElementId,
) -> Option<&'a paged_canvas::channel::SceneTreeNode> {
    for n in nodes {
        if n.id.as_ref() == Some(want) {
            return Some(n);
        }
        if let Some(hit) = node(&n.children, want) {
            return Some(hit);
        }
    }
    None
}

#[test]
fn the_tree_carries_plugin_metadata_and_only_the_reserved_namespace() {
    let mut m = model();
    let pen = ElementId::Polygon("pen".into());
    let ov = ElementId::Oval("ov".into());
    let tree = m.scene_tree();
    assert!(node(&tree, &pen).expect("pen").plugin_metadata.is_empty());

    m.apply_mutation(&Mutation::SetPluginMetadata {
        element_id: pen.clone(),
        key: "x-paged:media.paged.draw".into(),
        value: Some(r#"{"v":1,"data":{"blendStep":{"blend":"b1","index":1,"t":0.5}}}"#.into()),
        caller: None,
    })
    .expect("set metadata");

    let tree = m.scene_tree();
    let entries = &node(&tree, &pen).expect("pen").plugin_metadata;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "x-paged:media.paged.draw");
    assert!(entries[0].value.contains("blendStep"));
    assert!(node(&tree, &ov).expect("ov").plugin_metadata.is_empty());
    // Spread / page rows never carry any.
    assert!(tree.iter().all(|s| s.plugin_metadata.is_empty()));

    // The tree and the per-element read agree.
    let props = m.element_properties(&pen).expect("props");
    let per_element = props
        .entries
        .iter()
        .filter(|e| e.path == PropertyPath::PluginMetadata)
        .count();
    assert_eq!(per_element, entries.len());

    // Omitted from the wire when empty: an old reader sees no new field.
    let json = serde_json::to_string(node(&tree, &ov).unwrap()).unwrap();
    assert!(!json.contains("pluginMetadata"), "{json}");
}
