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

// Ask InDesign whether Smart Text Reflow adds pages for footnotes
// (footnote-reflow-probe.sh drives it). Each case is a fresh document with
// Smart Text Reflow on (add pages at the end of the story, not limited to
// primary frames) and one 300 x 200 pt frame on page 1:
//
//   bodyFits    — a short body whose one footnote fits;
//   noteSplits  — a short body whose one footnote is 40 lines long and
//                 may split (the default);
//   noteNoSplit — the same note with NoSplitting;
//   bodyOverset — body text that oversets the frame on its own.
//
// Reports per case the page count after reflow, the pages the body's last
// character and the note's last character sit on, and whether the story
// or the note is overset.
//
// Inputs: PROBE_OUT (JSON), PROBE_IDML, PROBE_PDF.
(function () {
    var OUT = $.global.PROBE_OUT;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    var log = [];
    function pageOf(ip) {
        try { var tf = ip.parentTextFrames[0]; return tf && tf.parentPage ? tf.parentPage.name : "overset"; } catch (e) { return "overset"; }
    }
    function lines(n, word) { var s = []; for (var i = 0; i < n; i++) s.push(word + " " + (i + 1)); return s.join("\r"); }
    var cases = ["bodyFits", "noteSplits", "noteNoSplit", "bodyOverset"];
    for (var c = 0; c < cases.length; c++) {
        var doc = app.documents.add(true);
        try {
            doc.documentPreferences.facingPages = false;
            doc.documentPreferences.pageWidth = 400;
            doc.documentPreferences.pageHeight = 300;
            var tp = doc.textPreferences;
            tp.smartTextReflow = true;
            tp.addPages = AddPageOptions.END_OF_STORY;
            tp.limitToMasterTextFrames = false;
            tp.deleteEmptyPages = false;
            var b = doc.pages[0].bounds;
            var f = doc.pages[0].textFrames.add({ geometricBounds: [b[0] + 50, b[1] + 50, b[0] + 250, b[1] + 350] });
            doc.footnoteOptions.noSplitting = cases[c] == "noteNoSplit";
            var st = f.parentStory;
            st.contents = cases[c] == "bodyOverset" ? lines(40, "Body line") : "Body text with a note.";
            var fn = null;
            if (cases[c] != "bodyOverset") {
                fn = st.insertionPoints[5].footnotes.add();
                fn.insertionPoints[-1].contents = cases[c] == "bodyFits" ? "A short note." : lines(40, "Note line");
            }
            doc.recompose();
            // Smart Text Reflow runs on idle/recompose; give it a nudge.
            for (var k = 0; k < 3; k++) { doc.recompose(); }
            var entry = '"' + cases[c] + '": {"pages": ' + doc.pages.length +
                ', "bodyEndPage": "' + pageOf(st.insertionPoints[-1]) + '"' +
                ', "storyOverset": ' + st.overflows;
            if (fn) entry += ', "noteEndPage": "' + pageOf(fn.insertionPoints[-1]) + '"';
            log.push(entry + "}");
            if (c == 1) {
                doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
                doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
            }
        } finally { doc.close(SaveOptions.NO); }
    }
    var file = File(OUT); file.encoding = "UTF-8"; file.open("w");
    file.write("{\n  " + log.join(",\n  ") + "\n}\n"); file.close();
})();
