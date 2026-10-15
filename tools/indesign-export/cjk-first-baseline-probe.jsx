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

// Ask InDesign where it sets the first baseline of a frame of CJK text
// (cjk-first-baseline-probe.sh drives it). One frame per font and
// composer, 30 pt, "漢字かなAx" (Latin faces "Hxg"), the frame's first
// baseline offset left at InDesign's default (Ascent) and then set to Em
// Box Height. Writes cjk-first-baseline.json into PROBE_OUT: per frame its
// top, its first line's baseline, and the ascent and descent InDesign
// reports for the line's first character.
(function () {
    var OUT = Folder(String($.global.PROBE_OUT));
    var result = { version: app.version, frames: [], errors: [] };
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
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = app.documents.add(false);
    try {
        doc.documentPreferences.pageWidth = 595.276;
        doc.documentPreferences.pageHeight = 841.89;
        doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.rulerOrigin = RulerOrigin.PAGE_ORIGIN;
        doc.zeroPoint = [0, 0];
        // CID-keyed OpenType (Kozuka, Hiragino, Adobe Song), TrueType CJK
        // (Noto Sans JP, Songti SC, AppleMyungjo, Arial Unicode MS) and two
        // Latin faces, applied by InDesign's own "family<tab>style" name.
        var FONTS = [
            ["Kozuka Mincho Pr6N", "R"], ["Kozuka Gothic Pr6N", "R"], ["Hiragino Mincho ProN", "W3"],
            ["Adobe Song Std", "L"], ["Noto Sans JP", "Regular"], ["Songti SC", "Regular"],
            ["AppleMyungjo", "Regular"], ["Arial Unicode MS", "Regular"],
            ["Minion Pro", "Regular"], ["Open Sans", "Regular"]
        ];
        var CASES = [
            ["$ID/HL Composer", FirstBaseline.ASCENT_OFFSET],
            ["$ID/HL Composer J", FirstBaseline.ASCENT_OFFSET],
            ["$ID/HL Composer J", FirstBaseline.EMBOX_HEIGHT]
        ];
        var page = doc.pages[0], slot = 0;
        for (var f = 0; f < FONTS.length; f++) {
            for (var c = 0; c < CASES.length; c++) {
                if (slot > 0 && slot % 12 === 0) page = doc.pages.add();
                var top = 30 + (slot % 12) * 65, left = 40;
                slot++;
                var tf = page.textFrames.add({ geometricBounds: [top, left, top + 55, left + 400] });
                var latin = FONTS[f][0] === "Minion Pro" || FONTS[f][0] === "Open Sans";
                // Kozuka, Hiragino and Adobe Song are CID-keyed; the rest are not.
                tf.contents = latin ? "Hxg" : "漢字かなAx";
                var t = tf.parentStory.texts[0], rec = { font: FONTS[f][0], style: FONTS[f][1],
                    composer: CASES[c][0], firstBaseline: String(CASES[c][1]), top: top };
                try {
                    var font = app.fonts.itemByName(FONTS[f][0] + "\t" + FONTS[f][1]);
                    t.appliedFont = font;
                    rec.fontType = String(font.fontType);
                } catch (e) { rec.error = String(e); }
                t.pointSize = 30;
                tf.parentStory.paragraphs[0].composer = CASES[c][0];
                tf.textFramePreferences.firstBaselineOffset = CASES[c][1];
                rec.fontStatus = String(t.appliedFont.status);
                rec.baseline = tf.parentStory.lines[0].baseline;
                rec.ascent = tf.parentStory.characters[0].ascent;
                rec.descent = tf.parentStory.characters[0].descent;
                result.frames.push(rec);
            }
        }
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(OUT.fsName + "/cjk-first-baseline.idml"));
    } catch (e) {
        result.errors.push(String(e) + " @" + e.line);
    }
    doc.close(SaveOptions.NO);
    var file = File(OUT.fsName + "/cjk-first-baseline.json");
    file.encoding = "UTF-8";
    file.open("w");
    file.write(json(result));
    file.close();
})();
