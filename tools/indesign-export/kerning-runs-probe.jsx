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

// Ask InDesign whether the font's kerning applies across a style-run
// boundary (kerning-runs-probe.sh drives it). One frame per case: Minion
// Pro Regular 30 pt, Metrics kerning, "AVATAR To Wa" with every other
// character changed in one attribute — colour, underline, point size or
// font — beside the same text as one run and with kerning off. Writes kerning-runs.{pdf,idml,json} into PROBE_OUT; the
// glyphs are measured from the PDF (pdf-glyphs.py).
(function () {
    var OUT = Folder(String($.global.PROBE_OUT));
    var errors = [], names = [];
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = app.documents.add(false);
    try {
        doc.documentPreferences.pageWidth = 595.276;
        doc.documentPreferences.pageHeight = 841.89;
        doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.rulerOrigin = RulerOrigin.PAGE_ORIGIN;
        doc.zeroPoint = [0, 0];
        var red = doc.colors.add({ name: "Probe Red", model: ColorModel.PROCESS,
            space: ColorSpace.CMYK, colorValue: [0, 100, 100, 0] });
        var TEXT = "AVATAR To Wa";
        var everyOther = function (t, f) {
            for (var i = 1; i < t.characters.length; i += 2) {
                f(t.characters[i]);
            }
        };
        var CASES = [
            ["one-run", function (t) {}],
            ["none", function (t) { t.kerningMethod = "$ID/None"; }],
            ["colour", function (t) { everyOther(t, function (ch) { ch.fillColor = red; }); }],
            ["underline", function (t) { everyOther(t, function (ch) { ch.underline = true; }); }],
            ["size", function (t) { everyOther(t, function (ch) { ch.pointSize = 29; }); }],
            ["font", function (t) {
                everyOther(t, function (ch) { ch.appliedFont = app.fonts.itemByName("Myriad Pro\tRegular"); });
            }]
        ];
        var page = doc.pages[0];
        for (var c = 0; c < CASES.length; c++) {
            var top = 50 + c * 70;
            var tf = page.textFrames.add({ geometricBounds: [top, 60, top + 60, 560] });
            tf.contents = TEXT;
            var t = tf.parentStory.texts[0];
            t.appliedFont = app.fonts.itemByName("Minion Pro\tRegular");
            t.pointSize = 30;
            t.kerningMethod = "$ID/Metrics";
            try { CASES[c][1](t); } catch (e) { errors.push(CASES[c][0] + ": " + e); }
            names.push(CASES[c][0] + "@0:" + top);
        }
        doc.exportFile(ExportFormat.PDF_TYPE, File(OUT.fsName + "/kerning-runs.pdf"));
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(OUT.fsName + "/kerning-runs.idml"));
    } catch (e) {
        errors.push(String(e) + " @" + e.line);
    }
    doc.close(SaveOptions.NO);
    var f = File(OUT.fsName + "/kerning-runs.json");
    f.encoding = "UTF-8"; f.open("w");
    f.write('{"version":"' + app.version + '","cases":["' + names.join('","') + '"],"errors":["' +
        errors.join('","').replace(/\\/g, "\\\\").replace(/"/g, "'") + '"]}');
    f.close();
})();
