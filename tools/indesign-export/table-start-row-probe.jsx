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

// Ask InDesign where a table row starts when its Start Row option asks for
// the next column, frame or page (table-start-row-probe.sh drives it).
// Builds four facing pages; a story threaded through a two-column frame on
// each page holds one table of twelve 20 pt body rows. Then, case by case
// on a fresh copy of the table, sets one row's startRow and reports, per
// row, the frame (by page and column) InDesign sets it in. Exports
// InDesign's IDML and PDF of the last case.
//
// With PROBE_IN (an absolute .idml), opens that document instead and
// reports where each row of its tables stands.
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
    try { if (IN) { doc.recompose(); report("opened"); write(); } else { build(); } } finally { doc.close(SaveOptions.NO); }
    // Where each row of each table stands: its first cell's first
    // insertion point's parent text container (frame) and that frame's
    // page, plus the column of the frame the row's baseline falls in.
    // An opened document's rows also name the frame: its index among
    // its story's text containers ("page/column/frame").
    function where(row) {
        try {
            var ip = row.cells[0].insertionPoints[0];
            var tc = ip.parentTextFrames[0];
            var page = tc.parentPage ? tc.parentPage.name : "pasteboard";
            var col = ip.horizontalOffset < tc.geometricBounds[1] + (tc.geometricBounds[3] - tc.geometricBounds[1]) / 2 ? 1 : 2;
            if (!IN) return page + "/" + col;
            var tcs = tc.parentStory.textContainers, k = 0;
            while (k < tcs.length && tcs[k].id != tc.id) k++;
            return page + "/" + col + "/" + k;
        } catch (e) { return "overset"; }
    }
    function report(label) {
        var rows = [];
        for (var s = 0; s < doc.stories.length; s++) {
            var st = doc.stories[s];
            for (var t = 0; t < st.tables.length; t++) {
                var tb = st.tables[t], one = [];
                for (var r = 0; r < tb.rows.length; r++) one.push(q(where(tb.rows[r])));
                if (IN) log.push(q(tb.id ? st.id + "/" + tb.id : String(s)) + ": [" + one.join(", ") + "]");
                rows = rows.concat(one);
            }
        }
        if (!IN) log.push('"' + label + '": [' + rows.join(", ") + ']');
    }
    function write() {
        var f = File(OUT); f.encoding = "UTF-8"; f.open("w");
        f.write("{\n  " + log.join(",\n  ") + "\n}\n"); f.close();
    }
    function build() {
        doc.documentPreferences.facingPages = true;
        doc.documentPreferences.pageWidth = 400;
        doc.documentPreferences.pageHeight = 300;
        while (doc.pages.length < 5) doc.pages.add();
        var frames = [];
        for (var p = 0; p < 5; p++) {
            var b = doc.pages[p].bounds; // [top, left, bottom, right] in spread coordinates
            var f = doc.pages[p].textFrames.add({ geometricBounds: [b[0] + 20, b[1] + 20, b[2] - 20, b[3] - 20] });
            f.textFramePreferences.textColumnCount = 2;
            f.textFramePreferences.textColumnGutter = 12;
            if (p) frames[p - 1].nextTextFrame = f;
            frames.push(f);
        }
        var story = frames[0].parentStory;
        var cases = [
            ["anywhere", StartParagraph.ANYWHERE],
            ["nextColumn", StartParagraph.NEXT_COLUMN],
            ["nextFrame", StartParagraph.NEXT_FRAME],
            ["nextPage", StartParagraph.NEXT_PAGE],
            ["nextOddPage", StartParagraph.NEXT_ODD_PAGE],
            ["nextEvenPage", StartParagraph.NEXT_EVEN_PAGE]
        ];
        for (var c = 0; c < cases.length; c++) {
            story.contents = "";
            var t = story.insertionPoints[-1].tables.add({ bodyRowCount: 12, columnCount: 2 });
            for (var r = 0; r < 12; r++) {
                t.rows[r].height = 20; t.rows[r].autoGrow = false;
                t.rows[r].cells[0].contents = "Row " + (r + 1);
            }
            // Row 4 of 12 asks to start elsewhere; rows 1-3 sit in column 1
            // of page 1.
            t.rows[3].startRow = cases[c][1];
            doc.recompose();
            report(cases[c][0]);
        }
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
        doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
        write();
    }
})();
