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

// Ask InDesign which of an object style's corner attributes it reads
// (object-style-corners-probe.sh drives it, in two phases).
//
// PROBE_PHASE "author": a document with an object style "Corners" whose
// four corners are rounded 20 pt, applied to one rectangle, exported as
// object-style-corners.idml into PROBE_OUT. The driver then rewrites the
// style's corner attributes into variants and strips the rectangle's own.
//
// PROBE_PHASE "read": opens every PROBE_OUT/variants/*.idml and reports
// the style's and the rectangle's four corner options and radii, as
// InDesign's DOM reads them, into object-style-corners.json.
(function () {
    var OUT = Folder(String($.global.PROBE_OUT));
    var PHASE = String($.global.PROBE_PHASE);
    var result = { version: app.version, phase: PHASE, variants: [], errors: [] };
    function q(s) {
        s = String(s);
        return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/[\x00-\x1f\u2028\u2029]/g, " ") + '"';
    }
    function r3(v) { return Math.round(v * 1000) / 1000; }
    function json(v) {
        if (v === null || v === undefined) return "null";
        if (typeof v === "number") return String(r3(v));
        if (typeof v === "boolean") return String(v);
        if (typeof v === "string") return q(v);
        if (v instanceof Array) {
            var a = [];
            for (var i = 0; i < v.length; i++) a.push(json(v[i]));
            return "[" + a.join(",") + "]";
        }
        var p = [];
        for (var k in v) if (v.hasOwnProperty(k)) p.push(q(k) + ":" + json(v[k]));
        return "{" + p.join(",") + "}";
    }
    var SLOTS = ["topLeft", "topRight", "bottomLeft", "bottomRight"];
    function corners(o) {
        var c = {};
        for (var i = 0; i < SLOTS.length; i++) {
            try {
                c[SLOTS[i]] = [String(o[SLOTS[i] + "CornerOption"]), o[SLOTS[i] + "CornerRadius"]];
            } catch (e) { c[SLOTS[i]] = ["ERR " + e]; }
        }
        return c;
    }
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    if (PHASE === "author") {
        var doc = app.documents.add(false);
        try {
            doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
            doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
            var style = doc.objectStyles.add({ name: "Corners" });
            style.enableStrokeAndCornerOptions = true;
            for (var i = 0; i < SLOTS.length; i++) {
                style[SLOTS[i] + "CornerOption"] = CornerOptions.ROUNDED_CORNER;
                style[SLOTS[i] + "CornerRadius"] = 20;
            }
            var rect = doc.pages[0].rectangles.add({ geometricBounds: [100, 100, 300, 400] });
            rect.appliedObjectStyle = style;
            result.variants.push({ name: "authored", style: corners(style), rect: corners(rect) });
            doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(OUT.fsName + "/object-style-corners.idml"));
        } catch (e) {
            result.errors.push(String(e) + " @" + e.line);
        }
        doc.close(SaveOptions.NO);
    } else {
        var files = Folder(OUT.fsName + "/variants").getFiles("*.idml");
        files.sort(function (a, b) { return a.name < b.name ? -1 : 1; });
        for (var f = 0; f < files.length; f++) {
            var rec = { name: decodeURI(files[f].name).replace(/\.idml$/, "") };
            var d = null;
            try {
                d = app.open(files[f], false);
                rec.style = corners(d.objectStyles.itemByName("Corners"));
                var r = d.pages[0].rectangles[0];
                rec.rectStyle = String(r.appliedObjectStyle.name);
                rec.rect = corners(r);
            } catch (e) { rec.error = String(e) + " @" + e.line; }
            if (d) d.close(SaveOptions.NO);
            result.variants.push(rec);
        }
    }
    var file = File(OUT.fsName + "/object-style-corners-" + PHASE + ".json");
    file.encoding = "UTF-8";
    file.open("w");
    file.write(json(result));
    file.close();
})();
