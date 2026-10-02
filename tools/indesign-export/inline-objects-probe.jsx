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

// Ask InDesign where it sets inline and above-line anchored objects in
// `inline-objects.idml` (inline-objects-probe.sh drives it): every body
// line's baseline and every object's visible bounds, in points with the
// ruler at the page origin, then the same pages under variants that pin
// what the rules depend on (auto leading 150 %, space above, point size,
// fixed leading, the face). The answers are pinned in
// crates/paged-renderer/tests/inline_objects_pipeline.rs.
(function () {
    var IDML = $.global.PROBE_IDML, OUT = $.global.PROBE_OUT;
    function q(s) { s = String(s); return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/[\r\n\u2029\u2028\uFFFC]/g, "~") + '"'; }
    function r2(v) { return Math.round(v * 1000) / 1000; }
    function arr(a) { var o = []; for (var i = 0; i < a.length; i++) o.push(r2(a[i])); return "[" + o.join(",") + "]"; }
    function g(o, k) { try { var v = o[k]; return (typeof v === "number") ? r2(v) : q(v); } catch (e) { return "null"; } }
    function measureFrame(tf) {
        var lines = [];
        for (var l = 0; l < tf.lines.length; l++) {
            var ln = tf.lines[l];
            var c0 = ln.characters[0];
            lines.push("{" + '"baseline":' + g(ln, "baseline") + ',"x":' + g(ln, "horizontalOffset") +
                ',"end_x":' + g(ln, "endHorizontalOffset") + ',"ascent":' + g(ln, "ascent") + ',"descent":' + g(ln, "descent") +
                ',"leading":' + g(ln, "leading") + ',"index":' + g(c0, "index") + ',"text":' + q(String(ln.contents).substr(0, 40)) + "}");
        }
        var objs = [];
        var items = tf.parentStory.allPageItems;
        for (var k = 0; k < items.length; k++) {
            var it = items[k];
            var a = it.anchoredObjectSettings;
            var anchorIdx = -1, anchorBase = null;
            var ax = null, nx = null, px = null;
            try { anchorIdx = it.parent.index; anchorBase = it.parent.baseline; ax = it.parent.horizontalOffset; } catch (e) {}
            try { nx = tf.parentStory.characters[anchorIdx + 1].horizontalOffset; } catch (e) {}
            try { px = tf.parentStory.characters[anchorIdx - 1].horizontalOffset; } catch (e) {}
            var vb = null;
            try { vb = it.visibleBounds; } catch (e) {}
            var gb = null;
            try { gb = it.geometricBounds; } catch (e) {}
            objs.push("{" + '"bounds":' + (gb ? arr(gb) : "null") + ',"visible":' + (vb ? arr(vb) : "null") +
                ',"position":' + q(a.anchoredPosition) + ',"space_above":' + r2(a.anchorSpaceAbove) +
                ',"y_offset":' + r2(a.anchorYoffset) + ',"align":' + q(a.horizontalAlignment) + ',"anchor_index":' + anchorIdx +
                ',"anchor_x":' + (ax === null ? "null" : r2(ax)) + ',"next_x":' + (nx === null ? "null" : r2(nx)) + ',"prev_x":' + (px === null ? "null" : r2(px)) + ',"anchor_baseline":' + (anchorBase === null ? "null" : r2(anchorBase)) + "}");
        }
        return "{" + '"bounds":' + arr(tf.geometricBounds) + ',"overflows":' + tf.overflows +
            ',"lines":[' + lines.join(",") + '],"objects":[' + objs.join(",") + "]}";
    }
    function measurePage(page) {
        var fr = [];
        var tfs = page.textFrames;
        var list = [];
        for (var f = 0; f < tfs.length; f++) list.push(tfs[f]);
        list.sort(function (a, b) { return a.geometricBounds[0] - b.geometricBounds[0]; });
        for (var f = 0; f < list.length; f++) {
            if (list[f].geometricBounds[0] < 50) continue; // label
            fr.push(measureFrame(list[f]));
        }
        return "{" + '"name":' + q(page.name) + ',"frames":[' + fr.join(",") + "]}";
    }
    var out = [];
    try {
        var doc = app.open(File(IDML), true);
        doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.rulerOrigin = RulerOrigin.PAGE_ORIGIN;
        doc.zeroPoint = [0, 0];
        var pages = [];
        for (var p = 0; p < doc.pages.length; p++) pages.push(measurePage(doc.pages[p]));
        out.push('"pages":[' + pages.join(",") + "]");
        // Variant: page 1 with autoLeading 150 %.
        function bodyFrames(page) {
            var r = [];
            for (var f = 0; f < page.textFrames.length; f++) if (page.textFrames[f].geometricBounds[0] >= 50) r.push(page.textFrames[f]);
            return r;
        }
        var b1 = bodyFrames(doc.pages[0])[0];
        b1.parentStory.texts[0].autoLeading = 150;
        out.push('"page1_autoleading150":' + measurePage(doc.pages[0]));
        b1.parentStory.texts[0].autoLeading = 120;
        // Variant: page 6 objects with space above 10.
        var b6 = bodyFrames(doc.pages[5])[0];
        var items = b6.parentStory.allPageItems;
        for (var k = 0; k < items.length; k++) items[k].anchoredObjectSettings.anchorSpaceAbove = 10;
        out.push('"page6_space_above10":' + measurePage(doc.pages[5]));
        // Variant: page 7 objects with space above 10 (an above-line object at a frame's top).
        var fs7 = bodyFrames(doc.pages[6]);
        for (var gi = 0; gi < fs7.length; gi++) {
            var it7 = fs7[gi].parentStory.allPageItems;
            for (var k = 0; k < it7.length; k++) it7[k].anchoredObjectSettings.anchorSpaceAbove = 10;
        }
        out.push('"page7_space_above10":' + measurePage(doc.pages[6]));
        var s6 = bodyFrames(doc.pages[5])[0].parentStory;
        for (var k = 0; k < items.length; k++) items[k].anchoredObjectSettings.anchorSpaceAbove = 0;
        s6.texts[0].pointSize = 20;
        out.push('"page6_pt20":' + measurePage(doc.pages[5]));
        s6.texts[0].pointSize = 10;
        s6.texts[0].leading = 20;
        out.push('"page6_lead20":' + measurePage(doc.pages[5]));
        s6.texts[0].leading = 30;
        out.push('"page6_lead30":' + measurePage(doc.pages[5]));
        s6.texts[0].leading = Leading.AUTO;
        var fams = ["Open Sans", "Roboto", "Source Serif 4", "Lora"];
        for (var fi = 0; fi < fams.length; fi++) {
            try {
                s6.texts[0].appliedFont = app.fonts.itemByName(fams[fi] + "	Regular");
                out.push('"page6_font_' + fi + '":' + measurePage(doc.pages[5]));
            } catch (e) { out.push('"page6_font_' + fi + '_err":' + q(e.message)); }
        }
        var s1 = bodyFrames(doc.pages[0])[0].parentStory;
        s1.texts[0].pointSize = 20;
        out.push('"page1_pt20":' + measurePage(doc.pages[0]));
        s1.texts[0].pointSize = 10;
        s1.texts[0].leading = 20;
        out.push('"page1_lead20":' + measurePage(doc.pages[0]));
        // The leading model the measurements were taken under.
        out.push('"text_prefs":{"leading_model":' + q(doc.textPreferences.useParagraphLeading) + "}");
        doc.close(SaveOptions.NO);
    } catch (e) {
        out.push('"error":' + q(e.message) + ',"line":' + e.line);
    }
    var o = File(OUT);
    o.encoding = "UTF-8";
    o.open("w");
    o.write("{" + out.join(",") + "}\n");
    o.close();
})();
