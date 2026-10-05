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

//! v66 — `imageContentTransform` on every frame kind that holds an image.
//!
//! The renderer has drawn an oval's and a polygon's inner `<Image>`
//! transform and the IDML writer has written it back all along, but the
//! setter answered only for rectangles, so a plugin that changed an
//! image's pixel size could commit the new pixels to an ellipse and not
//! the transform that places them. Each kind here takes the write, reads
//! it back, MOVES the drawn image, and undoes.

use paged_canvas::{
    channel::{ByteBuf, Mutation},
    element_selection::ElementId,
    CanvasModel, CanvasOptions, PageId,
};
use paged_compose::DisplayCommand;
use paged_mutate::operation::PathAnchorSpec;
use paged_mutate::{PropertyPath, Value};

fn model() -> CanvasModel {
    let sample = paged_gen::samples::build("geometry").expect("geometry sample");
    let bytes = paged_gen::write_idml(&sample).expect("write idml");
    CanvasModel::load("v66-ict", &bytes, CanvasOptions::default()).expect("load")
}

fn page(m: &CanvasModel) -> PageId {
    PageId(m.pages()[0].self_id.clone())
}

fn minted(m: &mut CanvasModel, mutation: Mutation) -> ElementId {
    m.apply_mutation(&mutation)
        .expect("insert")
        .created_id
        .expect("the insert reports the element it created")
}

fn png_2x2() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

/// Every `Image` command's transform on the first page, in order.
fn image_transforms(m: &CanvasModel) -> Vec<[f32; 6]> {
    let id = page(m);
    m.display_list_for_page(&id)
        .expect("page display list")
        .commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::Image { transform, .. } => Some(transform.0),
            _ => None,
        })
        .collect()
}

fn read(m: &CanvasModel, id: &ElementId) -> Option<Value> {
    m.element_properties(id)?
        .entries
        .into_iter()
        .find(|e| e.path == PropertyPath::ImageContentTransform)
        .and_then(|e| e.value)
}

fn check(kind: &str, mut m: CanvasModel, id: ElementId) {
    m.apply_mutation(&Mutation::ReplaceImageBytes {
        element_id: id.raw_id().to_string(),
        bytes: Some(ByteBuf(png_2x2())),
    })
    .unwrap_or_else(|e| panic!("{kind}: image bytes: {e:?}"));
    let before = image_transforms(&m);
    assert_eq!(before.len(), 1, "{kind}: one image drawn: {before:?}");
    let before_read = read(&m, &id);
    assert!(
        before_read.is_some(),
        "{kind}: an image-bearing frame reads its content transform"
    );

    let t = [40.0, 0.0, 0.0, 40.0, 12.0, 8.0];
    m.apply_mutation(&Mutation::SetElementProperty {
        element_id: id.clone(),
        path: PropertyPath::ImageContentTransform,
        value: Value::Transform(Some(t)),
    })
    .unwrap_or_else(|e| panic!("{kind}: the write is refused: {e:?}"));
    assert_eq!(read(&m, &id), Some(Value::Transform(Some(t))), "{kind}");
    let after = image_transforms(&m);
    assert_eq!(after.len(), 1, "{kind}");
    assert_ne!(after, before, "{kind}: the drawn image must move");

    m.undo()
        .unwrap_or_else(|| panic!("{kind}: nothing to undo"));
    assert_eq!(read(&m, &id), before_read, "{kind}: undo restores");
    assert_eq!(
        image_transforms(&m),
        before,
        "{kind}: undo restores the draw"
    );
}

#[test]
fn rectangle_oval_and_polygon_take_the_image_content_transform() {
    let mut m = model();
    let page_id = page(&m);
    let rect = minted(
        &mut m,
        Mutation::InsertFrame {
            page_id,
            bounds: (100.0, 100.0, 260.0, 220.0),
        },
    );
    check("Rectangle", m, rect);

    let mut m = model();
    let page_id = page(&m);
    let oval = minted(
        &mut m,
        Mutation::InsertOval {
            page_id,
            bounds: (100.0, 300.0, 260.0, 420.0),
        },
    );
    check("Oval", m, oval);

    let mut m = model();
    let page_id = page(&m);
    let corner = |x: f32, y: f32| PathAnchorSpec {
        anchor: [x, y],
        left: [x, y],
        right: [x, y],
    };
    let poly = minted(
        &mut m,
        Mutation::InsertPath {
            page_id,
            anchors: vec![
                corner(300.0, 100.0),
                corner(460.0, 100.0),
                corner(460.0, 220.0),
                corner(300.0, 220.0),
            ],
            open: false,
            smooth: false,
        },
    );
    check("Polygon", m, poly);
}
