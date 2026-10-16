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

// Ask InDesign how it sets vertical text (vertical-text-probe.sh drives
// it, in two phases). InDesign 20.0.1's scripting DOM has no story
// orientation, so the frames are authored horizontally and turned
// vertical in the IDML.
//
// PROBE_PHASE "author": one frame per case, 200 pt wide and 300 tall, one
// per page: Kozuka Mincho Pr6N R 20 pt under the Japanese Paragraph
// Composer, a paragraph of kanji, kana, punctuation, brackets, a
// long-vowel mark, Latin and digits long enough to fill several columns,
// one setting changed per case (leading, insets, columns, first baseline,
// alignment, kerning, ruby and kenten). Exports vertical-text-author.idml.
// The driver then sets every story's StoryOrientation="Vertical" but the
// horizontal-* controls'. (A kinsoku set cannot be measured here: this
// InDesign reads <KinsokuSet>HardKinsoku</KinsokuSet> as Nothing.)
//
// PROBE_PHASE "read": opens vertical-text.idml, reports each frame's
// bounds and its lines as the DOM gives them, and exports
// vertical-text.pdf; the glyphs are measured from it (pdf-glyphs.py).
(function () {
    var OUT = Folder(String($.global.PROBE_OUT));
    var PHASE = String($.global.PROBE_PHASE);
    var result = { version: app.version, phase: PHASE, cases: [], errors: [] };
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
    var KENTEN_KINDS = function () {
        return [KentenCharacter.KENTEN_SESAME_DOT, KentenCharacter.KENTEN_WHITE_SESAME_DOT,
            KentenCharacter.KENTEN_FISHEYE, KentenCharacter.KENTEN_BLACK_CIRCLE,
            KentenCharacter.KENTEN_SMALL_BLACK_CIRCLE, KentenCharacter.KENTEN_BULLSEYE,
            KentenCharacter.KENTEN_BLACK_TRIANGLE, KentenCharacter.KENTEN_WHITE_TRIANGLE,
            KentenCharacter.KENTEN_WHITE_CIRCLE, KentenCharacter.KENTEN_SMALL_WHITE_CIRCLE];
    };
    function rubyAndKenten(tf, below) {
        var r = tf.parentStory.characters.itemByRange(0, 1);
        r.rubyFlag = true;
        r.rubyType = RubyTypes.GROUP_RUBY;
        r.rubyString = "\u305F\u3066\u304C";
        var k = tf.parentStory.characters.itemByRange(4, 5);
        k.kentenKind = KentenCharacter.KENTEN_SESAME_DOT;
        if (below) {
            r.rubyPosition = RubyKentenPosition.BELOW_LEFT;
            k.kentenPosition = RubyKentenPosition.BELOW_LEFT;
        }
    }
    var CASES = [
        ["basic", function (tf, t) {}],
        ["leading30", function (tf, t) { t.leading = 30; }],
        ["insets", function (tf, t) { tf.textFramePreferences.insetSpacing = [10, 20, 30, 40]; }],
        ["columns2", function (tf, t) {
            tf.textFramePreferences.textColumnCount = 2;
            tf.textFramePreferences.textColumnGutter = 10;
        }],
        ["embox", function (tf, t) { tf.textFramePreferences.firstBaselineOffset = FirstBaseline.EMBOX_HEIGHT; }],
        ["leadingfirst", function (tf, t) { tf.textFramePreferences.firstBaselineOffset = FirstBaseline.LEADING_OFFSET; }],
        ["center", function (tf, t) { tf.parentStory.paragraphs[0].justification = Justification.CENTER_ALIGN; }],
        ["vjcenter", function (tf, t) { tf.textFramePreferences.verticalJustification = VerticalJustification.CENTER_ALIGN; }],
        ["kerningnone", function (tf, t) { t.kerningMethod = "$ID/None"; }],
        ["horizontal", function (tf, t) {}],
        ["ruby", function (tf, t) { rubyAndKenten(tf, false); }],
        ["ruby-below", function (tf, t) { rubyAndKenten(tf, true); }],
        // Which characters take a mark: kenten on the first 24 characters,
        // kanji, kana, the full stop, commas, brackets, the long-vowel
        // mark and Latin among them, set horizontally.
        ["horizontal-kenten-punct", function (tf, t) {
            tf.parentStory.characters.itemByRange(0, 23).kentenKind = KentenCharacter.KENTEN_BLACK_CIRCLE;
        }],
        ["kenten-punct", function (tf, t) {
            tf.parentStory.characters.itemByRange(0, 23).kentenKind = KentenCharacter.KENTEN_BLACK_CIRCLE;
        }],
        ["kenten-shapes", function (tf, t) {
            var kinds = KENTEN_KINDS();
            for (var i = 0; i < kinds.length; i++) tf.parentStory.characters[i].kentenKind = kinds[i];
        }]
    ];
    // The sample is escaped: ExtendScript reads a file with no non-ASCII
    // character near its top as Mac Roman.
    var TEXT = "\u7E26\u66F8\u304D\u306E\u6587\u7AE0\u3067\u3059\u3002\u6F22\u5B57\u3001\u304B\u306A\u3001\u30AB\u30BF\u30AB\u30CA\u300C\u62EC\u5F27\u300D\u3068\u30FC\u3092\u542B\u307F\u307E\u3059\u3002ABC\u3068123\u3082\u5165\u308C\u307E\u3059\u3002\u65E5\u672C\u8A9E\u306E\u7D44\u7248\u3092\u78BA\u304B\u3081\u308B\u3002";
    if (PHASE === "author") {
        var doc = app.documents.add(false);
        try {
            doc.documentPreferences.pageWidth = 595.276;
            doc.documentPreferences.pageHeight = 841.89;
            doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
            doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
            doc.viewPreferences.rulerOrigin = RulerOrigin.PAGE_ORIGIN;
            doc.zeroPoint = [0, 0];
            for (var c = 0; c < CASES.length; c++) {
                var page = c === 0 ? doc.pages[0] : doc.pages.add();
                var tf = page.textFrames.add({ geometricBounds: [100, 200, 400, 400] });
                tf.contents = TEXT;
                var t = tf.parentStory.texts[0];
                t.appliedFont = app.fonts.itemByName("Kozuka Mincho Pr6N\tR");
                t.pointSize = 20;
                tf.parentStory.paragraphs[0].composer = "$ID/HL Composer J";
                tf.label = CASES[c][0];
                try { CASES[c][1](tf, t); } catch (e) { result.errors.push(CASES[c][0] + ": " + e); }
            }
            doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(OUT.fsName + "/vertical-text-author.idml"));
        } catch (e) {
            result.errors.push(String(e) + " @" + e.line);
        }
        doc.close(SaveOptions.NO);
    } else {
        var doc = app.open(File(OUT.fsName + "/vertical-text.idml"), false);
        try {
            for (var p = 0; p < doc.pages.length; p++) {
                var tf = doc.pages[p].textFrames[0];
                var rec = { name: tf.label, page: p + 1, bounds: tf.geometricBounds };
                try {
                    rec.leading = String(tf.parentStory.texts[0].leading);
                    rec.lines = [];
                    var lines = tf.parentStory.lines;
                    for (var i = 0; i < lines.length; i++) {
                        var ln = lines[i];
                        rec.lines.push({
                            text: ln.contents, baseline: ln.baseline, x: ln.horizontalOffset,
                            endX: ln.endHorizontalOffset, endBaseline: ln.endBaseline
                        });
                    }
                } catch (e) { result.errors.push(rec.name + " lines: " + e); }
                result.cases.push(rec);
            }
            doc.exportFile(ExportFormat.PDF_TYPE, File(OUT.fsName + "/vertical-text.pdf"));
        } catch (e) {
            result.errors.push(String(e) + " @" + e.line);
        }
        doc.close(SaveOptions.NO);
    }
    var f = File(OUT.fsName + "/vertical-text-" + PHASE + ".json");
    f.encoding = "UTF-8";
    f.open("w");
    f.write(json(result));
    f.close();
})();
