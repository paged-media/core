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

// Ask InDesign how big, and where, it sets ruby and emphasis marks
// (kenten) by default (ruby-kenten-probe.sh drives it). One frame per
// case, Kozuka Mincho Pr6N R 30 pt under the Japanese Paragraph Composer,
// "漢字漢字漢字". Writes, into PROBE_OUT:
//
//   ruby-kenten.idml  the sample, as InDesign exports it
//   ruby-kenten.pdf   as InDesign prints it (where the marks are measured)
//   ruby-kenten.json  per frame, each character's insertion point and
//                     baseline, and the ruby and kenten settings InDesign
//                     reports for the marked characters (its defaults)
//
// Cases:
//   group      group ruby かんじ over the 5th and 6th characters
//   perchar    per-character ruby かん / じ over the same two
//   perchar-nospace  per-character ruby かんじ, no space to split it
//   wide       group ruby かんじかん (75 pt) over the third character
//              (30 pt): ruby wider than its base
//   sesame     sesame-dot kenten over the first two characters
//   circle     black-circle kenten over the first two characters
//   latin      Open Sans 30 pt "ABCDEF": group ruby "xy" over CD and
//              sesame kenten over AB (the rules on a Latin face)
//   kinds      one character of each kenten kind, in KentenGeneric's
//              glyph order
//   courier    the same in Courier New, whose cap height (0.571 em) is
//              far from Open Sans's (0.714 em)
(function () {
    var OUT = Folder(String($.global.PROBE_OUT));
    var result = { version: app.version, frames: {}, errors: [] };
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
    // A DOM value as data: numbers as they are, enumerations and objects
    // by name.
    function val(o, k) {
        try {
            var v = o[k];
            if (typeof v === "number" || typeof v === "boolean") return v;
            if (v && v.hasOwnProperty && v.hasOwnProperty("name")) return String(v.name);
            return String(v);
        } catch (e) { return "error: " + e; }
    }
    var RUBY = ["rubyFlag", "rubyString", "rubyType", "rubyFontSize", "rubyXScale", "rubyYScale",
        "rubyXOffset", "rubyYOffset", "rubyAlignment", "rubyPosition", "rubyFont", "rubyFontStyle",
        "rubyOverhang", "rubyParentSpacing", "rubyAutoAlign", "rubyAutoScaling",
        "rubyParentScalingPercent", "rubyParentOverhangAmount", "rubyOpenTypePro"];
    var KENTEN = ["kentenKind", "kentenFontSize", "kentenXScale", "kentenYScale", "kentenPlacement",
        "kentenAlignment", "kentenPosition", "kentenFont", "kentenFontStyle", "kentenCustomCharacter",
        "kentenCharacterSet", "kentenFillColor", "kentenTint"];

    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = app.documents.add(false);
    try {
        doc.documentPreferences.pageWidth = 595.276;
        doc.documentPreferences.pageHeight = 841.89;
        doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.rulerOrigin = RulerOrigin.PAGE_ORIGIN;
        doc.zeroPoint = [0, 0];
        var page = doc.pages[0];
        var frames = [];
        function frame(name, text, font, style) {
            var top = 50 + frames.length * 85;
            var tf = page.textFrames.add({ geometricBounds: [top, 60, top + 75, 500] });
            tf.contents = text;
            var t = tf.parentStory.texts[0];
            t.appliedFont = font;
            t.fontStyle = style;
            t.pointSize = 30;
            tf.parentStory.paragraphs[0].composer = "$ID/HL Composer J";
            frames.push([name, tf]);
            return tf.parentStory;
        }
        var KANJI = "漢字漢字漢字", KOZUKA = "Kozuka Mincho Pr6N";
        var s, r;
        s = frame("group", KANJI, KOZUKA, "R");
        r = s.characters.itemByRange(4, 5);
        r.rubyFlag = true; r.rubyType = RubyTypes.GROUP_RUBY; r.rubyString = "かんじ";
        s = frame("perchar", KANJI, KOZUKA, "R");
        r = s.characters.itemByRange(4, 5);
        r.rubyFlag = true; r.rubyType = RubyTypes.PER_CHARACTER_RUBY; r.rubyString = "かん じ";
        s = frame("perchar-nospace", KANJI, KOZUKA, "R");
        r = s.characters.itemByRange(4, 5);
        r.rubyFlag = true; r.rubyType = RubyTypes.PER_CHARACTER_RUBY; r.rubyString = "かんじ";
        s = frame("wide", KANJI, KOZUKA, "R");
        r = s.characters.itemByRange(2, 2);
        r.rubyFlag = true; r.rubyType = RubyTypes.GROUP_RUBY; r.rubyString = "かんじかん";
        s = frame("sesame", KANJI, KOZUKA, "R");
        s.characters.itemByRange(0, 1).kentenKind = KentenCharacter.KENTEN_SESAME_DOT;
        s = frame("circle", KANJI, KOZUKA, "R");
        s.characters.itemByRange(0, 1).kentenKind = KentenCharacter.KENTEN_BLACK_CIRCLE;
        s = frame("latin", "ABCDEF", "Open Sans", "Regular");
        r = s.characters.itemByRange(2, 3);
        r.rubyFlag = true; r.rubyType = RubyTypes.GROUP_RUBY; r.rubyString = "xy";
        s.characters.itemByRange(0, 1).kentenKind = KentenCharacter.KENTEN_SESAME_DOT;
        // Every kind, one character each, in this order.
        var KINDS = ["KENTEN_SESAME_DOT", "KENTEN_WHITE_SESAME_DOT", "KENTEN_FISHEYE",
            "KENTEN_BLACK_CIRCLE", "KENTEN_SMALL_BLACK_CIRCLE", "KENTEN_BULLSEYE",
            "KENTEN_BLACK_TRIANGLE", "KENTEN_WHITE_TRIANGLE", "KENTEN_WHITE_CIRCLE",
            "KENTEN_SMALL_WHITE_CIRCLE"];
        s = frame("kinds", "漢字漢字漢字漢字漢字", KOZUKA, "R");
        for (var kk = 0; kk < KINDS.length; kk++) {
            try { s.characters[kk].kentenKind = KentenCharacter[KINDS[kk]]; }
            catch (e) { result.errors.push(KINDS[kk] + ": " + e); }
        }
        s = frame("courier", "ABCDEF", "Courier New", "Regular");
        r = s.characters.itemByRange(2, 3);
        r.rubyFlag = true; r.rubyType = RubyTypes.GROUP_RUBY; r.rubyString = "xy";
        s.characters.itemByRange(0, 1).kentenKind = KentenCharacter.KENTEN_SESAME_DOT;

        doc.recompose();
        for (var f = 0; f < frames.length; f++) {
            var story = frames[f][1].parentStory, chars = [];
            for (var c = 0; c < story.characters.length; c++) {
                var ch = story.characters[c], rec = {
                    text: String(ch.contents), x: ch.horizontalOffset, baseline: ch.baseline,
                    endX: ch.endHorizontalOffset, ascent: ch.ascent, descent: ch.descent
                };
                if (ch.rubyFlag) { rec.ruby = {}; for (var k = 0; k < RUBY.length; k++) rec.ruby[RUBY[k]] = val(ch, RUBY[k]); }
                var kind = val(ch, "kentenKind");
                if (kind !== "NONE") { rec.kenten = {}; for (var k2 = 0; k2 < KENTEN.length; k2++) rec.kenten[KENTEN[k2]] = val(ch, KENTEN[k2]); }
                chars.push(rec);
            }
            result.frames[frames[f][0]] = { frame: frames[f][1].geometricBounds, chars: chars };
        }
        doc.exportFile(ExportFormat.PDF_TYPE, File(OUT.fsName + "/ruby-kenten.pdf"));
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(OUT.fsName + "/ruby-kenten.idml"));
    } catch (e) {
        result.errors.push(String(e) + " @" + e.line);
    }
    doc.close(SaveOptions.NO);
    var file = File(OUT.fsName + "/ruby-kenten.json");
    file.encoding = "UTF-8";
    file.open("w");
    file.write(json(result));
    file.close();
})();
