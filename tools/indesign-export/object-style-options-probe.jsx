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


// Ask InDesign how an object style's text-wrap, anchored-object and
// frame-fitting options reach the items that apply it
// (object-style-options-probe.sh drives it).
//
// 1. "authored": in a new document, styles setting each option category
//    (and one based on the wrap style) are applied to rectangles, a text
//    frame and two anchored rectangles; some items then override one
//    setting. Reports each item's effective settings and exports
//    InDesign's own IDML of it (PROBE_RESAVE), which the shell reads to
//    see what InDesign writes on the item.
// 2. "read" (when PROBE_IDML is given): opens that IDML and reports, for
//    every item whose name starts with "case", its effective settings and
//    where it is; for every text frame named "case… text", where its first
//    lines start and end. Also each object style's Enable flags as read.
//
// Inputs: PROBE_OUT (JSON written), PROBE_RESAVE, PROBE_IDML (optional).
(function () {
    var OUT = $.global.PROBE_OUT, RESAVE = $.global.PROBE_RESAVE, IDML = $.global.PROBE_IDML;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;

    function q(s) {
        return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"';
    }
    function safe(f) {
        try {
            var v = f();
            if (v === null || v === undefined) {
                return "null";
            }
            if (v instanceof Array) {
                var parts = [];
                for (var i = 0; i < v.length; i++) {
                    parts.push(typeof v[i] == "number" ? String(Math.round(v[i] * 1000) / 1000) : q(v[i]));
                }
                return "[" + parts.join(", ") + "]";
            }
            if (typeof v == "number") {
                return String(Math.round(v * 1000) / 1000);
            }
            if (typeof v == "boolean") {
                return String(v);
            }
            return q(v);
        } catch (e) {
            return q("ERR " + e);
        }
    }
    function wrapOf(it) {
        var w;
        try { w = it.textWrapPreferences; } catch (e) { return "null"; }
        return '{"mode": ' + safe(function () { return String(w.textWrapMode); }) +
            ', "offset": ' + safe(function () { return w.textWrapOffset; }) +
            ', "side": ' + safe(function () { return String(w.textWrapSide); }) +
            ', "inverse": ' + safe(function () { return w.inverse; }) + "}";
    }
    function anchorOf(it) {
        var a;
        try { a = it.anchoredObjectSettings; } catch (e) { return "null"; }
        return '{"position": ' + safe(function () { return String(a.anchoredPosition); }) +
            ', "anchorPoint": ' + safe(function () { return String(a.anchorPoint); }) +
            ', "hRef": ' + safe(function () { return String(a.horizontalReferencePoint); }) +
            ', "hAlign": ' + safe(function () { return String(a.horizontalAlignment); }) +
            ', "vRef": ' + safe(function () { return String(a.verticalReferencePoint); }) +
            ', "vAlign": ' + safe(function () { return String(a.verticalAlignment); }) +
            ', "x": ' + safe(function () { return a.anchorXoffset; }) +
            ', "y": ' + safe(function () { return a.anchorYoffset; }) +
            ', "spineRelative": ' + safe(function () { return a.spineRelative; }) +
            ', "lockPosition": ' + safe(function () { return a.lockPosition; }) +
            ', "pinPosition": ' + safe(function () { return a.pinPosition; }) + "}";
    }
    function fitOf(it) {
        var f;
        try { f = it.frameFittingOptions; } catch (e) { return "null"; }
        return '{"fitting": ' + safe(function () { return String(f.fittingOnEmptyFrame); }) +
            ', "alignment": ' + safe(function () { return String(f.fittingAlignment); }) +
            ', "autoFit": ' + safe(function () { return f.autoFit; }) +
            ', "crops": ' + safe(function () { return [f.topCrop, f.leftCrop, f.bottomCrop, f.rightCrop]; }) + "}";
    }
    function flags(s) {
        return '{"wrap": ' + safe(function () { return s.enableTextWrapAndOthers; }) +
            ', "anchored": ' + safe(function () { return s.enableAnchoredObjectOptions; }) +
            ', "fitting": ' + safe(function () { return s.enableFrameFittingOptions; }) + "}";
    }
    function item(it) {
        return '    {"name": ' + q(it.name) +
            ', "type": ' + q(it.constructor.name) +
            ', "style": ' + safe(function () { return it.appliedObjectStyle.name; }) +
            ', "bounds": ' + safe(function () { return it.geometricBounds; }) +
            ', "wrap": ' + wrapOf(it) + ', "anchored": ' + anchorOf(it) + ', "fitting": ' + fitOf(it) + "}";
    }
    function style(s) {
        return '    {"name": ' + q(s.name) +
            ', "basedOn": ' + safe(function () { return s.basedOn.name; }) +
            ', "enable": ' + flags(s) + ', "wrap": ' + wrapOf(s) +
            ', "anchored": ' + anchorOf(s) + ', "fitting": ' + fitOf(s) + "}";
    }
    function lines(tf) {
        var out = [];
        var n = Math.min(tf.lines.length, 6);
        for (var i = 0; i < n; i++) {
            var l = tf.lines[i];
            out.push("[" + safe(function () { return l.horizontalOffset; }) + ", " +
                safe(function () { return l.endHorizontalOffset; }) + ", " +
                safe(function () { return l.baseline; }) + "]");
        }
        return '    {"name": ' + q(tf.name) + ', "lines": [' + out.join(", ") + "]}";
    }

    // An earlier run that stopped half-way leaves its scratch document open.
    for (var o = app.documents.length - 1; o >= 0; o--) {
        var left = app.documents[o];
        if (!left.saved && left.objectStyles.itemByName("W BBox").isValid) {
            left.close(SaveOptions.NO);
        }
    }

    // 1. authored
    var doc = app.documents.add(false);
    doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
    doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
    var page = doc.pages[0];
    var none = doc.objectStyles.itemByName("$ID/[None]");

    var wrap = doc.objectStyles.add({ name: "W BBox" });
    wrap.enableTextWrapAndOthers = true;
    wrap.textWrapPreferences.textWrapMode = TextWrapModes.BOUNDING_BOX_TEXT_WRAP;
    wrap.textWrapPreferences.textWrapOffset = [6, 9, 12, 15];
    var child = doc.objectStyles.add({ name: "W Child", basedOn: wrap });

    var anc = doc.objectStyles.add({ name: "A Right" });
    anc.enableAnchoredObjectOptions = true;
    var s = anc.anchoredObjectSettings;
    s.anchoredPosition = AnchorPosition.ANCHORED;
    s.anchorPoint = AnchorPoint.TOP_RIGHT_ANCHOR;
    s.horizontalReferencePoint = AnchoredRelativeTo.TEXT_FRAME;
    s.horizontalAlignment = HorizontalAlignment.RIGHT_ALIGN;
    s.verticalReferencePoint = VerticallyRelativeTo.LINE_BASELINE;
    s.anchorYoffset = 4;

    var fit = doc.objectStyles.add({ name: "F Fill" });
    fit.enableFrameFittingOptions = true;
    fit.frameFittingOptions.fittingOnEmptyFrame = EmptyFrameFittingOptions.FILL_PROPORTIONALLY;
    fit.frameFittingOptions.fittingAlignment = AnchorPoint.TOP_LEFT_ANCHOR;
    fit.frameFittingOptions.autoFit = true;
    fit.frameFittingOptions.leftCrop = 3;

    function rect(name, y, st) {
        var r = page.rectangles.add({ geometricBounds: [y, 20, y + 40, 80] });
        r.applyObjectStyle(st, true, false);
        r.name = name;
        return r;
    }
    rect("wrap-plain", 20, wrap);
    rect("wrap-offsets", 70, wrap).textWrapPreferences.textWrapOffset = [1, 1, 1, 1];
    rect("wrap-mode-none", 120, wrap).textWrapPreferences.textWrapMode = TextWrapModes.NONE;
    rect("wrap-child", 170, child);
    rect("fit-plain", 220, fit);
    rect("fit-override", 270, fit).frameFittingOptions.autoFit = false;
    var own = rect("wrap-own", 320, none);
    own.textWrapPreferences.textWrapMode = TextWrapModes.BOUNDING_BOX_TEXT_WRAP;
    var tw = page.textFrames.add({ geometricBounds: [380, 20, 420, 200] });
    tw.applyObjectStyle(wrap, true, false);
    tw.name = "wrap-textframe";
    var host = page.textFrames.add({ geometricBounds: [440, 20, 600, 300] });
    host.applyObjectStyle(none, true, false);
    host.name = "host";
    host.contents = "Anchor here and here and here, then some more words.";
    var a1 = host.insertionPoints[7].rectangles.add({ geometricBounds: [0, 0, 24, 36] });
    a1.applyObjectStyle(anc, true, false);
    a1.name = "anchored-plain";
    var a2 = host.insertionPoints[16].rectangles.add({ geometricBounds: [0, 0, 24, 36] });
    a2.applyObjectStyle(anc, true, false);
    a2.anchoredObjectSettings.anchorXoffset = 20;
    a2.name = "anchored-x";
    var a3 = host.insertionPoints[25].rectangles.add({ geometricBounds: [0, 0, 24, 36] });
    a3.applyObjectStyle(none, true, false);
    a3.name = "anchored-none";

    var authored = [], styles = [];
    var all = doc.allPageItems;
    for (var i = 0; i < all.length; i++) {
        authored.push(item(all[i]));
    }
    var os = [wrap, child, anc, fit, none];
    for (var j = 0; j < os.length; j++) {
        styles.push(style(os[j]));
    }
    doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(RESAVE));
    doc.close(SaveOptions.NO);

    // 2. read
    var read = [], readStyles = [], readLines = [];
    if (IDML) {
        var d = app.open(File(IDML), false);
        var items = d.allPageItems;
        for (var k = 0; k < items.length; k++) {
            var it = items[k];
            if (String(it.name).indexOf("case") != 0) {
                continue;
            }
            read.push(item(it));
            if (it.constructor.name == "TextFrame" && String(it.name).indexOf("text") > 0) {
                readLines.push(lines(it));
            }
        }
        for (var m = 0; m < d.objectStyles.length; m++) {
            readStyles.push(style(d.objectStyles[m]));
        }
        d.close(SaveOptions.NO);
    }

    var f = File(OUT);
    f.encoding = "UTF-8";
    f.lineFeed = "Unix";
    f.open("w");
    f.write('{\n  "indesign": "' + app.version + '",\n  "authored": [\n' + authored.join(",\n") +
        '\n  ],\n  "authored_styles": [\n' + styles.join(",\n") +
        '\n  ],\n  "read": [\n' + read.join(",\n") +
        '\n  ],\n  "read_lines": [\n' + readLines.join(",\n") +
        '\n  ],\n  "read_styles": [\n' + readStyles.join(",\n") + "\n  ]\n}\n");
    f.close();
})();
