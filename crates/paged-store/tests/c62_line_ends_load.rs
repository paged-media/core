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

//! C-62 — the end cap (Polygon / GraphicLine / Oval) and the line ends
//! (Polygon) are new model fields, and a `.pgm` written before them must
//! still open: with no cap, no line ends and arrowhead scales of 100 %,
//! never a derived `0.0` that would shrink every arrowhead to nothing.

use idml_import::{ArrowheadType, Bounds, GraphicLine, Oval, Polygon, Spread};
use paged_scene::{Document, ParsedSpread};

fn bounds() -> Bounds {
    Bounds {
        top: 10.0,
        left: 10.0,
        bottom: 60.0,
        right: 110.0,
    }
}

fn doc() -> Document {
    let mut spread = Spread::default();
    spread.polygons.push(Polygon {
        end_cap: Some("RoundEndCap".into()),
        start_arrow: ArrowheadType::CircleSolid,
        end_arrow: ArrowheadType::Triangle,
        start_arrow_scale: 150.0,
        ..Polygon::new("pen", bounds())
    });
    spread.graphic_lines.push(GraphicLine {
        end_cap: Some("ProjectingEndCap".into()),
        ..GraphicLine::new("line", bounds())
    });
    spread.ovals.push(Oval {
        end_cap: Some("ButtEndCap".into()),
        ..Oval::new("oval", bounds())
    });
    Document {
        spreads: vec![ParsedSpread {
            src: "Spreads/Spread_s1.xml".into(),
            spread,
        }],
        ..Default::default()
    }
}

#[test]
fn the_new_fields_round_trip_through_the_native_model() {
    let back = paged_store::from_bytes(&paged_store::to_bytes(&doc()).unwrap()).unwrap();
    let s = &back.spreads[0].spread;
    let p = &s.polygons[0];
    assert_eq!(p.end_cap.as_deref(), Some("RoundEndCap"));
    assert_eq!(
        (p.start_arrow, p.end_arrow),
        (ArrowheadType::CircleSolid, ArrowheadType::Triangle)
    );
    assert_eq!((p.start_arrow_scale, p.end_arrow_scale), (150.0, 100.0));
    assert_eq!(
        s.graphic_lines[0].end_cap.as_deref(),
        Some("ProjectingEndCap")
    );
    assert_eq!(s.ovals[0].end_cap.as_deref(), Some("ButtEndCap"));
}

/// A `.pgm` from before C-62: the same document with the new keys gone,
/// which is exactly what an older engine wrote.
#[test]
fn a_pgm_written_before_the_fields_existed_still_opens() {
    let mut json: serde_json::Value =
        serde_json::from_slice(&paged_store::to_bytes(&doc()).unwrap()).unwrap();
    let spread = &mut json["model"]["spreads"][0]["spread"];
    let strip = |item: &mut serde_json::Value, keys: &[&str]| {
        let obj = item.as_object_mut().expect("page item is an object");
        for k in keys {
            assert!(obj.remove(*k).is_some(), "{k} was serialised");
        }
    };
    strip(
        &mut spread["polygons"][0],
        &[
            "end_cap",
            "start_arrow",
            "end_arrow",
            "start_arrow_scale",
            "end_arrow_scale",
        ],
    );
    strip(&mut spread["graphic_lines"][0], &["end_cap"]);
    strip(&mut spread["ovals"][0], &["end_cap"]);

    let old = serde_json::to_vec(&json).unwrap();
    let back = paged_store::from_bytes(&old).expect("an older .pgm must still load");
    let s = &back.spreads[0].spread;
    let p = &s.polygons[0];
    assert_eq!(p.end_cap, None);
    assert_eq!(
        (p.start_arrow, p.end_arrow),
        (ArrowheadType::None, ArrowheadType::None)
    );
    assert_eq!(
        (p.start_arrow_scale, p.end_arrow_scale),
        (100.0, 100.0),
        "a missing scale is InDesign's 100 %, not 0"
    );
    assert_eq!(s.graphic_lines[0].end_cap, None);
    assert_eq!(s.ovals[0].end_cap, None);
    // Fields that were there before keep their values.
    assert!(p.visible);
    assert_eq!(p.self_id.as_deref(), Some("pen"));
}
