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

//! Quick diagnostic: read a Story XML and print the anchored
//! frames it carries. Used during development to verify the
//! anchored-frame parser slice picks up FillColor / StrokeColor /
//! children on real corpus stories.

use std::env;
use std::fs;

fn main() {
    let path = env::args()
        .nth(1)
        .expect("usage: dump_anchored <story.xml>");
    let bytes = fs::read(&path).expect("read story");
    let s = idml_import::parse_story(&bytes).expect("parse story");
    println!("story: {} paragraphs", s.paragraphs.len());
    for (i, p) in s.paragraphs.iter().enumerate() {
        if p.anchored_frames.is_empty() {
            continue;
        }
        println!("  para {}: {} anchored", i, p.anchored_frames.len());
        for af in &p.anchored_frames {
            println!(
                "    kind={:?} self={:?} fill={:?} stroke={:?} stroke_weight={:?}",
                af.frame_kind, af.self_id, af.fill_color, af.stroke_color, af.stroke_weight
            );
            println!(
                "      bounds={:?} item_transform={:?} setting={:?}",
                af.bounds, af.item_transform, af.setting
            );
            println!("      children={}", af.children.len());
        }
    }
}
