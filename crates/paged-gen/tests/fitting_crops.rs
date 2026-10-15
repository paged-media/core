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

//! `<FrameFittingOption>` crops are written in InDesign's convention.
//!
//! The builder holds crops as the model does: frame-space points from
//! the frame's edge to the picture's, positive shrinking the picture. It
//! wrote that number straight into `LeftCrop` and the others. InDesign
//! measures the other way — from the picture's edge, in the picture's
//! own units, positive where the frame cuts in — so every generated
//! fixture spelled its crops with the wrong sign (and, under a scaled
//! picture, the wrong size) for InDesign. The importer reads InDesign's
//! convention into the model's, so a generated crop must read back as
//! the number the builder was given.

/// Every `<Rectangle>` in the images sample with its `FrameFittingOption`
/// crops as written and its picture transform, in document order.
fn written(bytes: &[u8]) -> Vec<([f32; 4], [f32; 6])> {
    let container = idml_import::open_source_archive(bytes).expect("archive");
    let mut out = Vec::new();
    for (name, part) in container.entries.iter() {
        if !name.starts_with("Spreads/") {
            continue;
        }
        let xml = String::from_utf8_lossy(part);
        for rect in xml.split("<Rectangle ").skip(1) {
            let Some(ffo) = rect.split("<FrameFittingOption ").nth(1) else {
                continue;
            };
            let num = |s: &str, k: &str| -> f32 {
                let at = s.find(&format!(" {k}=\"")).expect(k) + k.len() + 3;
                s[at..at + s[at..].find('"').unwrap()].parse().unwrap()
            };
            let ffo = &format!(" {}", &ffo[..ffo.find("/>").unwrap()]);
            let crops = ["LeftCrop", "TopCrop", "RightCrop", "BottomCrop"].map(|k| num(ffo, k));
            let image = rect.split("<Image ").nth(1).expect("an image");
            let image = &format!(" {}", &image[..image.find('>').unwrap()]);
            let at = image.find(" ItemTransform=\"").unwrap() + 16;
            let t: Vec<f32> = image[at..at + image[at..].find('"').unwrap()]
                .split_whitespace()
                .map(|v| v.parse().unwrap())
                .collect();
            out.push((crops, [t[0], t[1], t[2], t[3], t[4], t[5]]));
        }
    }
    out
}

#[test]
fn the_cover_overflow_is_a_positive_indesign_crop() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::images::build()).unwrap();
    let frames = written(&bytes);
    assert!(!frames.is_empty());
    // The FillProportionally variant: the model grows the picture 24 pt
    // past each side of the frame under an identity picture transform —
    // the frame cuts 24 units off each side, as InDesign spells it.
    assert!(
        frames
            .iter()
            .any(|(c, t)| *c == [24.0, 0.0, 24.0, 0.0] && *t == [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        "{frames:?}"
    );
    // No frame keeps the model's spelling of that overflow.
    assert!(
        !frames.iter().any(|(c, _)| c[0] == -24.0 && c[2] == -24.0),
        "{frames:?}"
    );
}

/// What the builder was given is what the importer reads back.
#[test]
fn generated_crops_read_back_as_given() {
    let bytes = paged_gen::write_idml(&paged_gen::samples::images::build()).unwrap();
    let doc = idml_import::import_idml_doc(&bytes).expect("import");
    let read: Vec<[f32; 4]> = doc
        .spreads
        .iter()
        .flat_map(|s| s.spread.rectangles.iter())
        .filter_map(|r| r.frame_fitting.as_ref())
        .map(|f| [f.left_crop, f.top_crop, f.right_crop, f.bottom_crop].map(|v| v.unwrap_or(0.0)))
        .collect();
    assert!(
        read.iter().any(|c| *c == [-24.0, 0.0, -24.0, 0.0]),
        "the cover's model crops: {read:?}"
    );
    let centred = (200.0 - 128.0) * 0.5;
    assert!(
        read.iter().any(|c| *c == [centred; 4]),
        "CenterContent's model crops: {read:?}"
    );
}
