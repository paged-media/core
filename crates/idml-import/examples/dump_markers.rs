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

//! Tiny debug helper: scan stories in an IDML and print any auto-page-
//! number markers that surface from the parser. Used to verify the
//! ACE 18 / ACE 19 PI handler reaches downstream consumers.
use std::path::PathBuf;

fn main() {
    let path: PathBuf = std::env::args()
        .nth(1)
        .expect("usage: dump_markers <idml>")
        .into();
    let bytes = std::fs::read(&path).unwrap();
    let container = idml_import::open_source_archive(&bytes).unwrap();
    for (name, raw) in container.entries.iter() {
        if !name.starts_with("Stories/") || !name.ends_with(".xml") {
            continue;
        }
        let story = idml_import::parse_story(raw).unwrap();
        for p in &story.paragraphs {
            for r in &p.runs {
                if r.text.contains(idml_import::AUTO_PAGE_NUMBER_MARKER)
                    || r.text.contains(idml_import::NEXT_PAGE_NUMBER_MARKER)
                {
                    println!("{name}  text={:?}", r.text);
                }
            }
        }
    }
}
