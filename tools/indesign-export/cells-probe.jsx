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

// Ask InDesign where it sets the text of every table cell
// (cells-probe.sh drives it): per table, its page and every cell's name
// ("column:row"), contents, applied cell style, the first-baseline
// settings InDesign resolved for it, and each of its lines' baselines in
// points with the ruler at the page origin; then every text frame holding
// no table, with its first line's baseline. The answers for
// `table-cell-baseline` are pinned in
// crates/paged-renderer/tests/cell_first_baseline.rs.
(function () {
    var IDML = $.global.PROBE_IDML, OUT = $.global.PROBE_OUT;
    function q(s) {
        s = String(s);
        return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/[\r\n\t]/g, " ") + '"';
    }
    function r3(v) { return Math.round(v * 1000) / 1000; }
    // A number rounded, a style by its name, an enumeration as InDesign
    // prints it (`CAP_HEIGHT`).
    function g(o, k) {
        var v;
        try { v = o[k]; } catch (e) { return "null"; }
        if (typeof v === "number") return String(r3(v));
        try { if (v.constructor.name === "CellStyle") return q(v.name); } catch (e) {}
        return q(String(v));
    }
    var out = ['"indesign_version":' + q(app.version)];
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = null;
    try {
        doc = app.open(File(IDML), false);
        doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.rulerOrigin = RulerOrigin.PAGE_ORIGIN;
        doc.zeroPoint = [0, 0];
        var tables = [];
        for (var s = 0; s < doc.stories.length; s++) {
            var st = doc.stories[s];
            for (var t = 0; t < st.tables.length; t++) {
                var tb = st.tables[t];
                var page = "null";
                try { page = q(tb.parent.parentPage.name); } catch (e) {}
                var cells = [];
                for (var c = 0; c < tb.cells.length; c++) {
                    var cell = tb.cells[c];
                    var lines = [];
                    for (var l = 0; l < cell.lines.length; l++) lines.push(String(r3(cell.lines[l].baseline)));
                    cells.push("{" + '"name":' + q(cell.name) + ',"contents":' + q(cell.contents) +
                        ',"style":' + g(cell, "appliedCellStyle") +
                        ',"first_baseline_offset":' + g(cell, "firstBaselineOffset") +
                        ',"minimum_first_baseline_offset":' + g(cell, "minimumFirstBaselineOffset") +
                        ',"top_inset":' + g(cell, "topInset") +
                        ',"vertical_justification":' + g(cell, "verticalJustification") +
                        ',"height":' + g(cell, "height") +
                        ',"baselines":[' + lines.join(",") + "]}");
                }
                tables.push("{" + '"page":' + page + ',"cells":[' + cells.join(",") + "]}");
            }
        }
        out.push('"tables":[' + tables.join(",") + "]");
        // Every text frame holding no table: where its first line sits.
        var frames = [];
        for (var p = 0; p < doc.pages.length; p++) {
            var tfs = doc.pages[p].textFrames;
            for (var f = 0; f < tfs.length; f++) {
                var tf = tfs[f];
                if (tf.tables.length || !tf.lines.length) continue;
                frames.push("{" + '"page":' + q(doc.pages[p].name) + ',"bounds":[' + tf.geometricBounds.join(",") +
                    '],"first_baseline_offset":' + g(tf.textFramePreferences, "firstBaselineOffset") +
                    ',"font":' + q(tf.lines[0].appliedFont.name) + ',"contents":' + q(tf.lines[0].contents) +
                    ',"baseline":' + r3(tf.lines[0].baseline) + "}");
            }
        }
        out.push('"frames":[' + frames.join(",") + "]");
    } catch (e) {
        out.push('"error":' + q(e.message) + ',"line":' + e.line);
    }
    if (doc !== null) {
        try { doc.close(SaveOptions.NO); } catch (e) {}
    }
    var o = File(OUT);
    o.encoding = "UTF-8";
    o.open("w");
    o.write("{" + out.join(",") + "}\n");
    o.close();
})();
