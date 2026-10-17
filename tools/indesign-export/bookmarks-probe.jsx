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

// Ask InDesign how bookmarks are made, nested, moved and deleted
// (bookmarks-probe.sh drives it). Builds a three-page document with a
// story, then adds a bookmark to page 2, one to a text position on page
// 1, and a child under the first; reports the bookmark tree and the
// document's destinations after each step, and exports InDesign's IDML.
// Then renames one, moves the child to the top level and back under the
// other, deletes a parent that has a child, and reports what each leaves:
// the tree, and which destinations remain.
//
// With PROBE_IN (an absolute .idml), opens that document instead and
// reports its bookmark tree as InDesign reads it.
//
// Inputs: PROBE_IN (optional IDML to open), PROBE_OUT (JSON), PROBE_IDML,
// PROBE_PDF.
(function () {
    var OUT = $.global.PROBE_OUT;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"'; }
    var log = [];
    var IN = $.global.PROBE_IN;
    var doc = IN ? app.open(File(IN), true) : app.documents.add(true);
    try { if (IN) { state("opened"); write(); } else { build(); } } finally { doc.close(SaveOptions.NO); }
    function tree(list) {
        var out = [];
        for (var i = 0; i < list.length; i++) {
            var b = list[i], dest = "";
            try { dest = b.destination.constructor.name + ":" + b.destination.name; } catch (e) { dest = "err " + e; }
            out.push('{"name": ' + q(b.name) + ', "indent": ' + q(b.indent) + ', "destination": ' + q(dest) +
                ', "children": [' + tree(b.bookmarks).join(", ") + ']}');
        }
        return out;
    }
    function state(label) {
        log.push('"' + label + '": {"bookmarks": [' + tree(doc.bookmarks).join(", ") +
            '], "pageDestinations": ' + doc.hyperlinkPageDestinations.length +
            ', "textDestinations": ' + doc.hyperlinkTextDestinations.length + '}');
    }
    function write() {
        var f = File(OUT); f.encoding = "UTF-8"; f.open("w");
        f.write("{\n  " + log.join(",\n  ") + "\n}\n"); f.close();
    }
    function build() {
        doc.documentPreferences.facingPages = false;
        while (doc.pages.length < 3) doc.pages.add();
        var tf = doc.pages[0].textFrames.add({ geometricBounds: [20, 20, 280, 380] });
        tf.contents = "Chapter one starts here. A second sentence.";
        state("none");
        var page = doc.bookmarks.add(doc.pages[1], { name: "Page two" });
        state("pageBookmark");
        var dest = doc.hyperlinkTextDestinations.add(tf.parentStory.insertionPoints[25], { name: "Second sentence" });
        var text = doc.bookmarks.add(dest, { name: "A sentence" });
        state("textBookmark");
        var child = page.bookmarks.add(doc.pages[2], { name: "Page three, under two" });
        state("child");
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
        doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
        text.name = "Renamed sentence";
        state("renamed");
        try { child.move(LocationOptions.AT_END, doc); } catch (e) { log.push('"moveTopError": ' + q(e)); }
        state("movedToTop");
        try { doc.bookmarks.itemByName("Page three, under two").move(LocationOptions.AT_END, text); } catch (e) { log.push('"moveUnderError": ' + q(e)); }
        state("movedUnderText");
        try { doc.bookmarks.itemByName("Page two").move(LocationOptions.AFTER, doc.bookmarks.itemByName("Renamed sentence")); } catch (e) { log.push('"moveAfterError": ' + q(e)); }
        state("reordered");
        try { doc.bookmarks.itemByName("Renamed sentence").remove(); } catch (e) { log.push('"removeError": ' + q(e)); }
        state("removedParent");
        write();
    }
})();
