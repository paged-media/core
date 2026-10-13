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

// Ask InDesign where it sets each line of an IDML, and which baseline
// grid it read (baseline-grid-probe.sh drives it). Reports, straight
// after the open, in points from the top-left of each page:
//
//   * the document grid: `baselineStart`, `baselineDivision`,
//     `baselineGridRelativeOption`;
//   * every page's top margin;
//   * every text frame: its bounds, insets and frame grid
//     (`baselineFrameGridOptions`), and each of its lines: the baseline,
//     the first characters, and its paragraph's `gridAlignment` and
//     `gridAlignFirstLineOnly`.
//
// Inputs: PROBE_IDML, PROBE_OUT.
(function () {
    var IDML = $.global.PROBE_IDML, OUT = $.global.PROBE_OUT;
    function q(s) {
        s = String(s);
        return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"')
            .replace(/[\r\u2029]/g, "\u00b6").replace(/\u2028/g, "\u21b5")
            .replace(/[\n\t]/g, " ") + '"';
    }
    function n(v) { return String(Math.round(v * 1000) / 1000); }
    function g(o, k) {
        try { return q(o[k]); } catch (e) { return "null"; }
    }
    function frameGrid(tf) {
        try {
            var o = tf.baselineFrameGridOptions;
            return "{" + '"use":' + o.useCustomBaselineFrameGrid +
                ',"offset":' + n(o.startingOffsetForBaselineFrameGrid) +
                ',"relative":' + q(o.baselineFrameGridRelativeOption) +
                ',"increment":' + n(o.baselineFrameGridIncrement) + "}";
        } catch (e) { return q("error: " + e.message); }
    }
    function lines(tf) {
        var out = [];
        for (var l = 0; l < tf.lines.length; l++) {
            var line = tf.lines[l];
            var p = line.paragraphs[0];
            out.push("{" + '"baseline":' + n(line.baseline) +
                ',"text":' + q(String(line.contents).substr(0, 24)) +
                ',"grid":' + g(p, "gridAlignment") +
                ',"first_only":' + g(p, "gridAlignFirstLineOnly") +
                ',"size":' + n(line.pointSize) + "}");
        }
        return "[" + out.join(",") + "]";
    }
    function frames(page) {
        var out = [];
        var tfs = page.textFrames;
        for (var f = 0; f < tfs.length; f++) {
            var tf = tfs[f];
            var inset = "null";
            try { inset = "[" + tf.textFramePreferences.insetSpacing.join(",") + "]"; } catch (e) {}
            out.push("{" + '"bounds":[' + tf.geometricBounds.join(",") + "]" +
                ',"inset":' + inset +
                ',"columns":' + tf.textFramePreferences.textColumnCount +
                ',"frame_grid":' + frameGrid(tf) +
                ',"lines":' + lines(tf) + "}");
        }
        return "[" + out.join(",") + "]";
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
        var gp = doc.gridPreferences;
        out.push('"grid":{"start":' + n(gp.baselineStart) + ',"division":' + n(gp.baselineDivision) +
            ',"relative":' + q(gp.baselineGridRelativeOption) + "}");
        var pages = [];
        for (var p = 0; p < doc.pages.length; p++) {
            var page = doc.pages[p];
            pages.push("{" + '"name":' + q(page.name) +
                ',"margin_top":' + n(page.marginPreferences.top) +
                ',"frames":' + frames(page) + "}");
        }
        out.push('"pages":[' + pages.join(",") + "]");
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
