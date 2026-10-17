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

// Ask InDesign how endnotes are made, numbered and removed
// (endnote-probe.sh drives it). Builds a two-page document whose story
// carries a footnote, adds two endnotes, and reports after each step the
// document's pages, its stories (endnote story or not, contents with
// specials as code points), the frames holding the endnote story and the
// endnote options; exports InDesign's IDML and PDF of that state. Then
// inserts an endnote BEFORE the first, deletes the middle one
// (`deleteEndnote`) and deletes every endnote, reporting what each
// leaves.
//
// With PROBE_IN (an absolute .idml), opens that document instead and
// reports the same state as InDesign reads it.
//
// Inputs: PROBE_IN (optional IDML to open), PROBE_OUT (JSON), PROBE_IDML,
// PROBE_PDF.
(function () {
    var OUT = $.global.PROBE_OUT;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"'; }
    function spell(s) {
        var o = "";
        for (var i = 0; i < s.length; i++) {
            var c = s.charCodeAt(i);
            o += (c < 32 || c > 126) ? "<U+" + ("000" + c.toString(16).toUpperCase()).slice(-4) + ">" : s.charAt(i);
        }
        return o;
    }
    var log = [];
    var IN = $.global.PROBE_IN;
    var doc = IN ? app.open(File(IN), true) : app.documents.add(true);
    try { if (IN) { doc.recompose(); state("opened"); write(); } else { build(); } } finally { doc.close(SaveOptions.NO); }
    function state(label) {
        var stories = [];
        for (var s = 0; s < doc.stories.length; s++) {
            var st = doc.stories[s], frames = [];
            for (var f = 0; f < st.textContainers.length; f++) {
                var c = st.textContainers[f], page = "";
                try { page = c.parentPage ? c.parentPage.name : "pasteboard"; } catch (e) { page = "?"; }
                frames.push(q(c.constructor.name + "@" + page));
            }
            stories.push('{"endnoteStory": ' + q(st.isEndnoteStory) + ', "frames": [' + frames.join(", ") +
                '], "endnotes": ' + st.endnotes.length + ', "footnotes": ' + st.footnotes.length +
                ', "contents": ' + q(spell(st.contents)) + '}');
        }
        log.push('"' + label + '": {"pages": ' + doc.pages.length + ', "stories": [' + stories.join(", ") + ']}');
    }
    function options() {
        var eo = doc.endnoteOptions, o = [];
        var keys = ["endnoteTitle", "startEndnoteNumberAt", "endnoteNumberingStyle", "restartEndnoteNumbering",
            "showEndnotePrefixSuffix", "endnotePrefix", "endnoteSuffix", "endnoteMarkerPositioning",
            "endnoteSeparatorText", "scopeValue", "frameCreateOption"];
        for (var k = 0; k < keys.length; k++) {
            var v;
            try { v = eo[keys[k]]; } catch (e) { v = "?"; }
            o.push(q(keys[k]) + ": " + q(spell(String(v))));
        }
        log.push('"options": {' + o.join(", ") + '}');
    }
    function write() {
        var f = File(OUT); f.encoding = "UTF-8"; f.open("w");
        f.write("{\n  " + log.join(",\n  ") + "\n}\n"); f.close();
    }
    function attempt(label, fn) {
        try { fn(); } catch (e) { log.push('"' + label + 'Error": ' + q(e)); }
        state(label);
    }
    function build() {
        doc.documentPreferences.facingPages = false;
        doc.documentPreferences.pageWidth = 400;
        doc.documentPreferences.pageHeight = 300;
        while (doc.pages.length < 2) doc.pages.add();
        var tf = doc.pages[0].textFrames.add({ geometricBounds: [20, 20, 280, 380] });
        tf.contents = "First paragraph with a note. Second sentence with another note. A footnote here.";
        var story = tf.parentStory;
        story.insertionPoints[-1].footnotes.add().texts[0].insertionPoints[-1].contents = "A footnote.";
        state("none");
        attempt("one", function () {
            story.insertionPoints[28].createEndnote().texts[0].insertionPoints[-1].contents = "The first endnote.";
        });
        attempt("two", function () {
            story.insertionPoints[63].createEndnote().texts[0].insertionPoints[-1].contents = "The second endnote.";
        });
        options();
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
        doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
        attempt("insertedFirst", function () {
            story.insertionPoints[5].createEndnote().texts[0].insertionPoints[-1].contents = "Inserted before.";
        });
        attempt("deletedMiddle", function () { story.endnotes[1].deleteEndnote(); });
        // InDesign's DOM has no conversion (a footnote has remove and
        // convertToText only; Convert Footnote and Endnote is a dialog).
        attempt("deletedAll", function () {
            while (story.endnotes.length) story.endnotes[0].deleteEndnote();
        });
        write();
    }
})();
