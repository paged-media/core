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

// Ask InDesign what it does with ruby WIDER than its base characters
// (ruby-parent-spacing-probe.sh drives it). One frame per case: Kozuka
// Mincho Pr6N R 30 pt under the Japanese Paragraph Composer, kana on both
// sides of the base, group ruby at half size (15 pt) whose width exceeds
// the base's: "かんじかんじ" (90 pt) over 漢字 (60 pt), or "かんじ" (45 pt)
// over one 漢 (30 pt). Each case changes one setting from InDesign's
// default: the parent spacing (RubyParentSpacing), letting the ruby
// overhang the kana beside it (RubyOverhang, RubyParentOverhangAmount), or
// fitting it by scaling (RubyAutoScaling, RubyParentScalingPercent).
// Writes ruby-parent-spacing.{pdf,idml,json} into PROBE_OUT; the glyphs are
// measured from the PDF (pdf-glyphs.py).
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
        var PS = RubyParentSpacing;
        // [name, text, ruby range [from, to], ruby string, setter]
        // Kana beside the base let the ruby overhang them; kanji do not,
        // so the parent spacing shows with kanji neighbours.
        var TWO = ["あい漢字うえ", [2, 3], "かんじかんじ"];
        var WIDE = ["あい漢字うえ", [2, 3], "かんじかんじかん"];
        var K2 = ["山川漢字木林", [2, 3], "かんじかんじ"];
        var K1 = ["山川漢木林", [2, 2], "かんじ"];
        var K3 = ["山川漢字花木林", [2, 4], "かんじかんじかんじ"];
        var OH = RubyOverhang;
        var CASES = [
            ["k2-121", K2, function (r) {}],
            ["k2-none", K2, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_NO_ADJUSTMENT; }],
            ["k2-both", K2, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_BOTH_SIDES; }],
            ["k2-equal", K2, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_EQUAL_AKI; }],
            ["k2-full", K2, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_FULL_JUSTIFY; }],
            ["k1-121", K1, function (r) {}],
            ["k1-none", K1, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_NO_ADJUSTMENT; }],
            ["k1-both", K1, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_BOTH_SIDES; }],
            ["k1-equal", K1, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_EQUAL_AKI; }],
            ["k1-full", K1, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_FULL_JUSTIFY; }],
            ["k3-121", K3, function (r) {}],
            ["k3-none", K3, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_NO_ADJUSTMENT; }],
            ["k3-both", K3, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_BOTH_SIDES; }],
            ["k3-equal", K3, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_EQUAL_AKI; }],
            ["k3-full", K3, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_FULL_JUSTIFY; }],
            ["two-default", TWO, function (r) {}],
            ["two-oh-none", TWO, function (r) { r.rubyParentOverhangAmount = OH.NONE; }],
            ["two-oh-none-full", TWO, function (r) { r.rubyParentOverhangAmount = OH.NONE; r.rubyParentSpacing = PS.RUBY_PARENT_FULL_JUSTIFY; }],
            ["two-oh-halfruby", TWO, function (r) { r.rubyParentOverhangAmount = OH.RUBY_OVERHANG_HALF_RUBY; }],
            ["two-oh-halfchar", TWO, function (r) { r.rubyParentOverhangAmount = OH.RUBY_OVERHANG_HALF_CHAR; }],
            ["two-flag-on", TWO, function (r) { r.rubyOverhang = true; }],
            ["two-flag-on-halfruby", TWO, function (r) { r.rubyOverhang = true; r.rubyParentOverhangAmount = OH.RUBY_OVERHANG_HALF_RUBY; }],
            ["wide-default", WIDE, function (r) {}],
            ["wide-onechar", WIDE, function (r) { r.rubyParentOverhangAmount = OH.RUBY_OVERHANG_ONE_CHAR; }],
            ["wide-halfchar", WIDE, function (r) { r.rubyParentOverhangAmount = OH.RUBY_OVERHANG_HALF_CHAR; }],
            ["wide-nolimit", WIDE, function (r) { r.rubyParentOverhangAmount = OH.RUBY_OVERHANG_NO_LIMIT; }],
            ["wide-none-spacing", WIDE, function (r) { r.rubyParentSpacing = PS.RUBY_PARENT_NO_ADJUSTMENT; }],
            ["k2-scale", K2, function (r) { r.rubyAutoScaling = true; }],
            ["k2-scale-80", K2, function (r) { r.rubyAutoScaling = true; r.rubyParentScalingPercent = 80; }],
            ["k2-scale-50", K2, function (r) { r.rubyAutoScaling = true; r.rubyParentScalingPercent = 50; }],
            ["k2-align-off", K2, function (r) { r.rubyAutoAlign = false; }],
            ["mix-kanji-kana", ["山川漢字うえ", [2, 3], "かんじかんじ"], function (r) {}],
            ["mix-kana-kanji", ["あい漢字木林", [2, 3], "かんじかんじ"], function (r) {}],
            ["line-start", ["漢字うえお", [0, 1], "かんじかんじ"], function (r) {}],
            ["line-end", ["あいう漢字", [3, 4], "かんじかんじ"], function (r) {}],
            ["katakana", ["アイ漢字ウエ", [2, 3], "かんじかんじ"], function (r) {}],
            ["brackets", ["あ「漢字」え", [2, 3], "かんじかんじ"], function (r) {}],
            ["latin", ["ab漢字cd", [2, 3], "かんじかんじ"], function (r) {}],
            ["two-scale", TWO, function (r) { r.rubyAutoScaling = true; }],
            ["wide-scale", WIDE, function (r) { r.rubyAutoScaling = true; }]
        ];
        var page = doc.pages[0], perPage = 11;
        for (var c = 0; c < CASES.length; c++) {
            if (c > 0 && c % perPage === 0) page = doc.pages.add();
            var top = 50 + (c % perPage) * 70;
            var tf = page.textFrames.add({ geometricBounds: [top, 60, top + 60, 500] });
            var spec = CASES[c][1];
            tf.contents = spec[0];
            var t = tf.parentStory.texts[0];
            t.appliedFont = app.fonts.itemByName("Kozuka Mincho Pr6N\tR");
            t.pointSize = 30;
            tf.parentStory.paragraphs[0].composer = "$ID/HL Composer J";
            try {
                var r = tf.parentStory.characters.itemByRange(spec[1][0], spec[1][1]);
                r.rubyFlag = true;
                r.rubyType = RubyTypes.GROUP_RUBY;
                r.rubyString = spec[2];
                CASES[c][2](r);
            } catch (e) { errors.push(CASES[c][0] + ": " + e); }
            names.push(CASES[c][0] + "@" + (doc.pages.length - 1) + ":" + top);
        }
        doc.exportFile(ExportFormat.PDF_TYPE, File(OUT.fsName + "/ruby-parent-spacing.pdf"));
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(OUT.fsName + "/ruby-parent-spacing.idml"));
    } catch (e) {
        errors.push(String(e) + " @" + e.line);
    }
    doc.close(SaveOptions.NO);
    var f = File(OUT.fsName + "/ruby-parent-spacing.json");
    f.encoding = "UTF-8"; f.open("w");
    f.write('{"version":"' + app.version + '","cases":["' + names.join('","') + '"],"errors":["' +
        errors.join('","').replace(/\\/g, "\\\\").replace(/"/g, "'") + '"]}');
    f.close();
})();
