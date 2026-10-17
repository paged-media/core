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

// Ask InDesign how many table rows a text frame holds as its height nears
// the rows' sum (table-fit-probe.sh drives it). Each case is a frame of
// one height, threaded to a second frame, holding a table of eight rows of
// 20 pt with 1 pt strokes and the default 4 pt insets (each grows to its
// 12 pt label), or of fixed 20 pt rows. Reports per case the frame height,
// the rows' heights, the table's top in the frame and how many rows the
// first frame holds; also the frame's first-baseline and inset settings.
//
// With PROBE_IN (an absolute .idml), opens that document instead and
// reports, per table, its rows' heights, its first row's top below its
// frame's top and how many rows its first frame holds.
//
// Inputs: PROBE_IN (optional), PROBE_OUT (JSON), PROBE_IDML, PROBE_PDF.
(function () {
    var OUT = $.global.PROBE_OUT;
    var IN = $.global.PROBE_IN;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    var log = [];
    var doc = IN ? app.open(File(IN), true) : app.documents.add(true);
    try { if (IN) { opened(); } else { build(); } } finally { doc.close(SaveOptions.NO); }
    function opened() {
        doc.recompose();
        for (var s = 0; s < doc.stories.length; s++) {
            var st = doc.stories[s];
            for (var t = 0; t < st.tables.length; t++) {
                var tb = st.tables[t], rowH = [], first = null, inFirst = 0;
                for (var r = 0; r < tb.rows.length; r++) {
                    rowH.push(Math.round(tb.rows[r].height * 1000) / 1000);
                    try {
                        var tf = tb.rows[r].cells[0].insertionPoints[0].parentTextFrames[0];
                        if (first === null) first = tf;
                        if (tf.id == first.id) inFirst++;
                    } catch (e) {}
                }
                var frameH = first ? Math.round((first.geometricBounds[2] - first.geometricBounds[0]) * 1000) / 1000 : -1;
                log.push('{"table": "' + tb.id + '", "frame": ' + frameH + ', "rowsInFirstFrame": ' + inFirst + ', "rowHeights": [' + rowH.join(", ") + ']}');
            }
        }
        var file = File(OUT); file.encoding = "UTF-8"; file.open("w");
        file.write("[\n  " + log.join(",\n  ") + "\n]\n"); file.close();
    }
    function build() {
        doc.documentPreferences.facingPages = false;
        doc.documentPreferences.pageWidth = 400;
        doc.documentPreferences.pageHeight = 400;
        // Six 20 pt rows with 1 pt strokes: 120 pt of rows, 121 with the
        // outer strokes. Heights around both, plain and with a 4 pt bottom
        // inset or a 1 pt centred black frame stroke.
        var heights = [120, 120.4, 120.5, 120.6, 120.9, 121, 121.1, 121.2, 124.9, 125, 125.1, 125.4, 125.5, 125.6, 126];
        var fixed = [false];
        var variants = ["plain", "inset4", "stroke1"];
        while (doc.pages.length < heights.length * fixed.length * variants.length * 2) doc.pages.add();
        var p = 0;
        for (var v = 0; v < variants.length; v++)
        for (var f = 0; f < fixed.length; f++) {
            for (var h = 0; h < heights.length; h++) {
                var a = doc.pages[p].textFrames.add({ geometricBounds: [20, 20, 20 + heights[h], 220] });
                var b = doc.pages[p + 1].textFrames.add({ geometricBounds: [20, 20, 320, 220] });
                a.textFramePreferences.insetSpacing = variants[v] == "inset4" ? [0, 0, 4, 0] : [0, 0, 0, 0];
                if (variants[v] == "stroke1") {
                    a.strokeWeight = 1;
                    a.strokeColor = doc.swatches.itemByName("Black");
                    a.strokeAlignment = StrokeAlignment.CENTER_ALIGNMENT;
                }
                a.nextTextFrame = b;
                var t = a.parentStory.insertionPoints[-1].tables.add({ bodyRowCount: 8, columnCount: 1 });
                t.columns[0].width = 100;
                for (var r = 0; r < 8; r++) {
                    t.rows[r].height = 20;
                    t.rows[r].autoGrow = !fixed[f];
                    t.rows[r].cells[0].contents = "R" + (r + 1);
                }
                doc.recompose();
                var inA = 0, rowH = [];
                for (var r = 0; r < 8; r++) {
                    rowH.push(Math.round(t.rows[r].height * 1000) / 1000);
                    try { if (t.rows[r].cells[0].insertionPoints[0].parentTextFrames[0].id == a.id) inA++; } catch (e) {}
                }
                var top = Math.round((t.rows[0].cells[0].insertionPoints[0].baseline - 20) * 1000) / 1000;
                log.push('{"variant": "' + variants[v] + '", "fixed": ' + fixed[f] + ', "frame": ' + heights[h] + ', "rowsInFirst": ' + inA + ', "rowHeights": [' + rowH.join(", ") + '], "firstBaselineInFrame": ' + top + ', "firstBaselineOffset": "' + a.textFramePreferences.firstBaselineOffset + '"}');
                p += 2;
            }
        }
        var file = File(OUT); file.encoding = "UTF-8"; file.open("w");
        file.write("[\n  " + log.join(",\n  ") + "\n]\n"); file.close();
    }
})();
