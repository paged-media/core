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

// Ask InDesign how it spells, and lays out, the attributes behind four
// Paged fixes (spelling-and-spacing-probe.sh drives it). The document is
// built here, through InDesign's own scripting, so nothing Paged wrote
// stands between the question and the answer. Writes, into PROBE_OUT:
//
//   spelling-and-spacing.idml  the sample, as InDesign exports it: its
//                              attribute spellings (DropcapDetail,
//                              RubyFlag, CapToSmallCap, TextAlignment)
//   spelling-and-spacing.pdf   the same sample as InDesign prints it
//   spelling-and-spacing.json  what InDesign laid out: per frame, the
//                              insertion-point x of every character and
//                              each line's baseline, in pt from the page's
//                              top-left corner
//
// Open Sans 20 pt throughout (type on a path: 24 pt), A4, ruler at the
// page origin. Frames (top, left, bottom, right in pt):
//
//   ls0, ls50       "HH HH" ragged, DesiredLetterSpacing 0 / 50
//   ls50band        the same at 0 with Min -20 / Max 50 (the band only)
//   lsright         RightAlign, DesiredLetterSpacing 50, five lines
//                   (letter spacing on paragraph styles, as documents
//                   carry it)
//   trackright      RightAlign, Tracking 100, five lines
//   super, sub      "HH" then "HH" raised / lowered
//   ruleabove/below a 4 pt rule above / below "Hxg", offset 10 pt
//   spellings       a drop cap with detail 2, ruby on 漢字, small caps
//   localspacing    word and letter spacing and glyph scaling set on the
//                   paragraph itself, not through a style
//   glyphscale80    glyph scaling 105 % on a run set at 80 % horizontal
//                   scale
//   path0..path5    type on a straight 400 pt line with a 6 pt Paper
//                   stroke: Baseline, Ascender, Descender, Center text
//                   alignment on the path's centre, then Baseline on its
//                   top and bottom edge
(function () {
    var OUT = Folder(String($.global.PROBE_OUT));
    var result = { version: app.version, frames: {}, errors: [] };
    function q(s) {
        s = String(s);
        return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/[\r\n\t]/g, " ") + '"';
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
        var page = doc.pages[0];
        var black = doc.swatches.itemByName("Black");
        var paper = doc.swatches.itemByName("Paper");

        // A frame across the column, its text in Open Sans 20 pt; the
        // measured ones are listed in `measured`.
        var measured = [];
        function frame(name, top, height, text) {
            var tf = page.textFrames.add({ geometricBounds: [top, 57.638, top + height, 537.638] });
            tf.contents = text;
            var t = tf.parentStory.texts[0];
            t.appliedFont = "Open Sans";
            t.fontStyle = "Regular";
            t.pointSize = 20;
            measured.push([name, tf]);
            return tf;
        }
        function record(name, tf) {
            var p = tf.parentStory.paragraphs[0];
            var ip = [], lines = [];
            for (var i = 0; i < p.insertionPoints.length; i++) ip.push(p.insertionPoints[i].horizontalOffset);
            for (var l = 0; l < p.lines.length; l++) {
                var ln = p.lines[l], ips = ln.insertionPoints, k = ips.length - 1;
                while (k > 0 && /\s/.test(String(ln.characters[k - 1].contents))) k--;
                lines.push({
                    baseline: ln.baseline,
                    end: ips[k].horizontalOffset,
                    lastAdvance: ips[k].horizontalOffset - ips[k - 1].horizontalOffset,
                    prevAdvance: ips[k - 1].horizontalOffset - ips[k - 2].horizontalOffset
                });
            }
            var chars = [];
            for (var c = 0; c < p.characters.length; c++) chars.push(p.characters[c].baseline);
            result.frames[name] = {
                text: p.contents, ip: ip, lines: lines, charBaselines: chars,
                frame: tf.geometricBounds
            };
        }

        var wrap = [];
        for (var w = 0; w < 30; w++) wrap.push("HHHH");
        wrap = wrap.join(" ");

        // Letter spacing through paragraph styles, as real documents carry
        // it; `localspacing` sets it on the paragraph itself.
        var face = { appliedFont: "Open Sans", fontStyle: "Regular", pointSize: 20 };
        function style(name, props) {
            var st = doc.paragraphStyles.add({ name: name });
            for (var k in face) st[k] = face[k];
            for (var k2 in props) st[k2] = props[k2];
            return st;
        }
        var ls50 = style("LS50", { maximumLetterSpacing: 50, desiredLetterSpacing: 50 });
        var band = style("LSBand", { maximumLetterSpacing: 50, minimumLetterSpacing: -20 });
        var lsRight = style("LS50Right", { basedOn: ls50, justification: Justification.RIGHT_ALIGN });
        var f;
        f = frame("ls0", 40, 30, "HH HH");
        f = frame("ls50", 80, 30, "HH HH");
        f.parentStory.paragraphs[0].applyParagraphStyle(ls50, false);
        f = frame("ls50band", 120, 30, "HH HH");
        f.parentStory.paragraphs[0].applyParagraphStyle(band, false);
        f = frame("lsright", 160, 130, wrap);
        f.parentStory.paragraphs[0].applyParagraphStyle(lsRight, false);
        f = frame("trackright", 300, 130, wrap);
        f.parentStory.paragraphs[0].justification = Justification.RIGHT_ALIGN;
        f.parentStory.texts[0].tracking = 100;
        f = frame("super", 440, 30, "HHHH"); f.parentStory.characters.itemByRange(2, 3).position = Position.SUPERSCRIPT;
        f = frame("sub", 480, 30, "HHHH"); f.parentStory.characters.itemByRange(2, 3).position = Position.SUBSCRIPT;
        f = frame("ruleabove", 530, 30, "Hxg");
        f.parentStory.paragraphs[0].ruleAbove = true;
        f.parentStory.paragraphs[0].ruleAboveLineWeight = 4;
        f.parentStory.paragraphs[0].ruleAboveOffset = 10;
        f.parentStory.paragraphs[0].ruleAboveColor = black;
        f = frame("rulebelow", 580, 30, "Hxg");
        f.parentStory.paragraphs[0].ruleBelow = true;
        f.parentStory.paragraphs[0].ruleBelowLineWeight = 4;
        f.parentStory.paragraphs[0].ruleBelowOffset = 10;
        f.parentStory.paragraphs[0].ruleBelowColor = black;
        // Spacing set on the paragraph itself, not through a style: InDesign
        // writes it on the <ParagraphStyleRange>.
        f = frame("localspacing", 700, 30, "Local spacing");
        var lp = f.parentStory.paragraphs[0];
        lp.maximumWordSpacing = 150; lp.desiredWordSpacing = 110; lp.minimumWordSpacing = 70;
        lp.maximumLetterSpacing = 30; lp.desiredLetterSpacing = 10; lp.minimumLetterSpacing = -10;
        lp.maximumGlyphScaling = 110; lp.desiredGlyphScaling = 105; lp.minimumGlyphScaling = 95;
        // Glyph scaling over a run's own horizontal scale: do they multiply?
        f = frame("glyphscale80", 740, 30, "Local spacing");
        f.parentStory.texts[0].horizontalScale = 80;
        lp = f.parentStory.paragraphs[0];
        lp.maximumGlyphScaling = 110; lp.desiredGlyphScaling = 105; lp.minimumGlyphScaling = 95;
        f = frame("spellings", 640, 50, "Drop cap 漢字 and SMALL caps");
        f.parentStory.paragraphs[0].dropCapCharacters = 1;
        f.parentStory.paragraphs[0].dropCapLines = 2;
        f.parentStory.paragraphs[0].dropcapDetail = 2;
        var kanji = f.parentStory.characters.itemByRange(9, 10);
        kanji.rubyFlag = true;
        kanji.rubyString = "かんじ";
        f.parentStory.characters.itemByRange(16, 20).capitalization = Capitalization.CAP_TO_SMALL_CAP;

        for (var n = 0; n < measured.length; n++) {
            if (measured[n][0] !== "spellings") record(measured[n][0], measured[n][1]);
        }

        // Type on a straight line, a second page: one row per alignment.
        var p2 = doc.pages.add();
        var rows = [
            ["BASELINE", "CENTER"], ["ASCENDER", "CENTER"], ["DESCENDER", "CENTER"],
            ["CENTER", "CENTER"], ["BASELINE", "TOP"], ["BASELINE", "BOTTOM"]
        ];
        result.paths = [];
        for (var i = 0; i < rows.length; i++) {
            var y = 120 + i * 100;
            var ln = p2.graphicLines.add();
            ln.paths[0].entirePath = [[72, y], [472, y]];
            ln.strokeWeight = 6;
            ln.strokeColor = paper;
            var tp = ln.textPaths.add();
            tp.contents = "HHHH";
            tp.texts[0].appliedFont = "Open Sans";
            tp.texts[0].fontStyle = "Regular";
            tp.texts[0].pointSize = 24;
            tp.textAlignment = TextTypeAlignments[rows[i][0] + "_TEXT_ALIGNMENT"];
            tp.pathAlignment = PathTypeAlignments[rows[i][1] + "_PATH_ALIGNMENT"];
            result.paths.push({ line: y, textAlignment: rows[i][0], pathAlignment: rows[i][1] });
        }

        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(OUT.fsName + "/spelling-and-spacing.idml"));
        doc.exportFile(ExportFormat.PDF_TYPE, File(OUT.fsName + "/spelling-and-spacing.pdf"));
    } catch (e) {
        result.errors.push(String(e) + (e.line ? " (line " + e.line + ")" : ""));
    }
    doc.close(SaveOptions.NO);
    var f2 = File(OUT.fsName + "/spelling-and-spacing.json");
    f2.encoding = "UTF-8";
    f2.open("w");
    f2.write(json(result));
    f2.close();
})();
