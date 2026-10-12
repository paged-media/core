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

// Ask InDesign which conditional text it shows in an IDML
// (conditions-probe.sh drives it). Reports, straight after the open:
//
//   * every condition: name, `visible`, indicator method;
//   * every condition set with its (condition, visibility) pairs, and the
//     active set;
//   * every story's text as InDesign holds it, plus the text of each table
//     cell and footnote in it, and the anchored items it carries;
//   * what each text frame COMPOSES: its lines.
//
// PROBE_TOGGLE=true then flips every condition's visibility and back, and
// reports the same again under "toggled": that tells "InDesign reads the
// flag" apart from "InDesign applies it only when it changes".
// PROBE_EXPORT_IDML / PROBE_EXPORT_PDF write InDesign's own IDML and PDF
// of the final state, so its spelling of hidden text can be read back.
//
// Inputs: PROBE_IDML, PROBE_OUT, PROBE_TOGGLE, PROBE_EXPORT_IDML,
// PROBE_EXPORT_PDF.
(function () {
    var IDML = $.global.PROBE_IDML, OUT = $.global.PROBE_OUT;
    var TOGGLE = String($.global.PROBE_TOGGLE) === "true";
    var EXPORT_IDML = String($.global.PROBE_EXPORT_IDML || "");
    var EXPORT_PDF = String($.global.PROBE_EXPORT_PDF || "");
    function q(s) {
        s = String(s);
        return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"')
            .replace(/\r/g, "\u00b6").replace(/\u2029/g, "\u00b6").replace(/\u2028/g, "\u21b5")
            .replace(/\uFFFC/g, "[obj]").replace(/[\n\t]/g, " ") + '"';
    }
    function g(o, k) {
        try { return q(o[k]); } catch (e) { return "null"; }
    }
    function conditions(doc) {
        var out = [];
        var cs = doc.conditions;
        for (var i = 0; i < cs.length; i++) {
            out.push("{" + '"name":' + q(cs[i].name) + ',"visible":' + cs[i].visible +
                ',"indicator":' + g(cs[i], "indicatorMethod") + "}");
        }
        return "[" + out.join(",") + "]";
    }
    function sets(doc) {
        var out = [];
        var ss = doc.conditionSets;
        for (var i = 0; i < ss.length; i++) {
            var pairs = [];
            try {
                var sc = ss[i].setConditions;
                for (var k = 0; k < sc.length; k++) {
                    pairs.push("[" + q(sc[k][0].name) + "," + sc[k][1] + "]");
                }
            } catch (e) { pairs.push(q("error: " + e.message)); }
            out.push("{" + '"name":' + q(ss[i].name) + ',"pairs":[' + pairs.join(",") + "]}");
        }
        var active = "null";
        try {
            var a = doc.conditionalTextPreferences.activeConditionSet;
            active = (a && a.isValid) ? q(a.name) : q(String(a));
        } catch (e) { active = q("error: " + e.message); }
        return '"sets":[' + out.join(",") + '],"active_set":' + active;
    }
    function stories(doc) {
        var out = [];
        for (var s = 0; s < doc.stories.length; s++) {
            var st = doc.stories[s];
            var paras = [];
            for (var p = 0; p < st.paragraphs.length; p++) paras.push(q(st.paragraphs[p].contents));
            var cells = [];
            for (var t = 0; t < st.tables.length; t++) {
                var tc = st.tables[t].cells;
                for (var c = 0; c < tc.length; c++) cells.push(q(tc[c].contents));
            }
            var notes = [];
            for (var f = 0; f < st.footnotes.length; f++) notes.push(q(st.footnotes[f].contents));
            var items = [];
            var all = st.allPageItems;
            for (var k = 0; k < all.length; k++) {
                var gb = "null";
                try { gb = "[" + all[k].geometricBounds.join(",") + "]"; } catch (e) {}
                items.push("{" + '"type":' + q(all[k].constructor.name) + ',"bounds":' + gb + "}");
            }
            out.push("{" + '"contents":' + q(st.contents) + ',"paragraphs":[' + paras.join(",") +
                '],"cells":[' + cells.join(",") + '],"footnotes":[' + notes.join(",") +
                '],"anchored":[' + items.join(",") + "]}");
        }
        return "[" + out.join(",") + "]";
    }
    function frames(doc) {
        var out = [];
        for (var p = 0; p < doc.pages.length; p++) {
            var tfs = doc.pages[p].textFrames;
            for (var f = 0; f < tfs.length; f++) {
                var lines = [];
                for (var l = 0; l < tfs[f].lines.length; l++) lines.push(q(tfs[f].lines[l].contents));
                out.push("{" + '"page":' + q(doc.pages[p].name) + ',"lines":[' + lines.join(",") + "]}");
            }
        }
        return "[" + out.join(",") + "]";
    }
    function snapshot(doc, label) {
        return q(label) + ":{" + '"conditions":' + conditions(doc) + "," + sets(doc) +
            ',"stories":' + stories(doc) + ',"frames":' + frames(doc) + "}";
    }

    var out = ['"indesign_version":' + q(app.version)];
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = null;
    try {
        doc = app.open(File(IDML), false);
        out.push(snapshot(doc, "open"));
        if (TOGGLE) {
            var cs = doc.conditions;
            for (var i = 0; i < cs.length; i++) {
                var v = cs[i].visible;
                cs[i].visible = !v;
                cs[i].visible = v;
            }
            out.push(snapshot(doc, "toggled"));
        }
        if (EXPORT_IDML) doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(EXPORT_IDML), false);
        if (EXPORT_PDF) {
            var preset = app.pdfExportPresets.itemByName("[High Quality Print]");
            if (!preset.isValid) preset = app.pdfExportPresets.itemByName("[Qualitativ hochwertiger Druck]");
            if (!preset.isValid) preset = app.pdfExportPresets[0];
            doc.exportFile(ExportFormat.PDF_TYPE, File(EXPORT_PDF), false, preset);
        }
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
