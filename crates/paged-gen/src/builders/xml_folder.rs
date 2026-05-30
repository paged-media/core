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

//! `XML/*.xml` — InDesign expects these present even when empty.
//! Generated samples never use XML structure tagging, so all three
//! files are minimal stubs.

use crate::xml::XmlBuilder;

const PKG_NS: (&str, &str) = (
    "xmlns:idPkg",
    "http://ns.adobe.com/AdobeInDesign/idml/1.0/packaging",
);
const DOM_VERSION: (&str, &str) = ("DOMVersion", "20.0");

pub fn backing_story_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:BackingStory", &[PKG_NS, DOM_VERSION]);
    b.empty(
        "XmlStory",
        &[
            ("Self", "BackingStory"),
            ("AppliedXMLTag", "XMLTag/$ID/Root"),
        ],
    );
    b.end("idPkg:BackingStory");
    b.into_bytes()
}

pub fn tags_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.start("idPkg:Tags", &[PKG_NS, DOM_VERSION]);
    b.empty("XMLTag", &[("Self", "XMLTag/$ID/Root"), ("Name", "Root")]);
    b.end("idPkg:Tags");
    b.into_bytes()
}

pub fn mapping_xml() -> Vec<u8> {
    let mut b = XmlBuilder::new();
    b.write_decl();
    b.empty("idPkg:Mapping", &[PKG_NS, DOM_VERSION]);
    b.into_bytes()
}
