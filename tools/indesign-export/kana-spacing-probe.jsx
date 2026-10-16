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

// Ask InDesign how it spaces kana (kana-spacing-probe.sh drives it). One
// frame per case: Kozuka Mincho Pr6N R 30 pt under the Japanese Paragraph
// Composer, a line of hiragana, katakana, kanji and brackets, with the
// kerning method or proportional metrics changed from a new frame's
// default. Writes kana-spacing.{pdf,idml,json} into PROBE_OUT; the glyphs
// are measured from the PDF (pdf-glyphs.py).
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
        var TEXT = "あいうえおアイウエオ漢「字」。";
        var CASES = [
            ["default", function (t) {}],
            ["none", function (t) { t.kerningMethod = "$ID/None"; }],
            ["metrics", function (t) { t.kerningMethod = "$ID/Metrics"; }],
            ["optical", function (t) { t.kerningMethod = "$ID/Optical"; }],
            ["palt", function (t) { t.otfProportionalMetrics = true; }],
            ["palt-none", function (t) { t.otfProportionalMetrics = true; t.kerningMethod = "$ID/None"; }]
        ];
        var page = doc.pages[0];
        for (var c = 0; c < CASES.length; c++) {
            var top = 50 + c * 70;
            var tf = page.textFrames.add({ geometricBounds: [top, 60, top + 60, 560] });
            tf.contents = TEXT;
            var t = tf.parentStory.texts[0];
            t.appliedFont = app.fonts.itemByName("Kozuka Mincho Pr6N\tR");
            t.pointSize = 30;
            tf.parentStory.paragraphs[0].composer = "$ID/HL Composer J";
            try { CASES[c][1](t); } catch (e) { errors.push(CASES[c][0] + ": " + e); }
            names.push(CASES[c][0] + "@0:" + top);
        }
        doc.exportFile(ExportFormat.PDF_TYPE, File(OUT.fsName + "/kana-spacing.pdf"));
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(OUT.fsName + "/kana-spacing.idml"));
    } catch (e) {
        errors.push(String(e) + " @" + e.line);
    }
    doc.close(SaveOptions.NO);
    var f = File(OUT.fsName + "/kana-spacing.json");
    f.encoding = "UTF-8"; f.open("w");
    f.write('{"version":"' + app.version + '","cases":["' + names.join('","') + '"],"errors":["' +
        errors.join('","').replace(/\\/g, "\\\\").replace(/"/g, "'") + '"]}');
    f.close();
})();
