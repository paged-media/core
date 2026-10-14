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

// Ask InDesign how a footnote's own paragraph settings lay its lines out
// (footnote-paragraphs-probe.sh drives it). The document is built here,
// through InDesign's scripting, one frame per case: a line of body text
// carrying one or two notes, each note's paragraph set one way. Writes,
// into PROBE_OUT:
//
//   footnote-paragraphs.idml  the sample, as InDesign exports it
//   footnote-paragraphs.pdf   the same sample as InDesign prints it
//   footnote-paragraphs.json  per frame, its bounds and the baseline of
//                             every body line; per note and paragraph,
//                             every line's baseline and the x of each of
//                             its insertion points, in pt from the page's
//                             top-left corner
//
// Open Sans throughout: the body 12 pt, the notes 10 pt, frames 360 pt
// wide. Cases:
//
//   plain        nothing set (the reference)
//   center       CenterAlign
//   right        RightAlign
//   justify      LeftJustified
//   indents      LeftIndent 24, FirstLineIndent -12, RightIndent 36
//   leading      Leading 18
//   spacing      word spacing 80/150/200, letter spacing 0/10/20, glyph
//                scaling 85/90/100 (Min/Desired/Max)
//   spacebefore  one note, SpaceBefore 12
//   twonotes     two notes, each SpaceBefore 12 and SpaceAfter 9
//   twoparas     a note of two paragraphs, each SpaceBefore 12 and
//                SpaceAfter 9, then a plain note
//   styled       the settings through a paragraph style instead
//                (LeftIndent 24, RightAlign, Leading 15)
//   tabs         a tab stop at 20 pt (the number's separator is a tab)
//   bodyparas    no note: two body paragraphs, each SpaceBefore 12 and
//                SpaceAfter 9 (InDesign writes them as one range)
//   bodyempty    the same with an empty paragraph between them
//   bodyright,   no note: "1", a tab, then text, in body text, RightAlign
//   bodycenter   and CenterAlign
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

    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = app.documents.add(false);
    try {
        doc.documentPreferences.pageWidth = 595.276;
        doc.documentPreferences.pageHeight = 841.89;
        doc.documentPreferences.facingPages = false;
        doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.rulerOrigin = RulerOrigin.PAGE_ORIGIN;
        doc.zeroPoint = [0, 0];
        result.options = {
            separatorText: doc.footnoteOptions.separatorText,
            spacer: doc.footnoteOptions.spacer,
            spaceBetween: doc.footnoteOptions.spaceBetween,
            ruleOn: doc.footnoteOptions.ruleOn,
            ruleOffset: doc.footnoteOptions.ruleOffset,
            ruleLineWeight: doc.footnoteOptions.ruleLineWeight,
            firstBaseline: String(doc.footnoteOptions.footnoteFirstBaselineOffset)
        };

        var LONG = "Footnote text that runs long enough to wrap onto a second and a third line, "
            + "so that alignment, indents, leading and spacing all show where InDesign puts "
            + "each line of the note.";
        var SHORT = "A second, shorter note.";
        var cases = [];
        var slot = 0;
        // Frames four to a page, 160 pt tall.
        function frame(name) {
            var pageIdx = Math.floor(slot / 4), row = slot % 4;
            slot++;
            while (doc.pages.length <= pageIdx) doc.pages.add();
            var top = 40 + row * 195;
            var tf = doc.pages[pageIdx].textFrames.add({
                geometricBounds: [top, 57.638, top + 160, 417.638]
            });
            tf.contents = "Body line that carries the note.";
            var t = tf.parentStory.texts[0];
            t.appliedFont = "Open Sans";
            t.fontStyle = "Regular";
            t.pointSize = 12;
            cases.push([name, tf]);
            return tf;
        }
        // A note at body character `at` (-1: the end), its text set in
        // Open Sans 10 pt unless `keepStyle`.
        function note(tf, text, at, keepStyle) {
            var ips = tf.parentStory.insertionPoints;
            var ip = at < 0 ? ips[-1] : ips[at];
            var fn = ip.footnotes.add();
            fn.insertionPoints[-1].contents = text;
            if (!keepStyle) {
                fn.texts[0].appliedFont = "Open Sans";
                fn.texts[0].fontStyle = "Regular";
                fn.texts[0].pointSize = 10;
            }
            return fn;
        }
        function paras(fn, set) {
            for (var i = 0; i < fn.paragraphs.length; i++) set(fn.paragraphs[i]);
        }

        var f, n;
        f = frame("plain"); note(f, LONG, -1);
        f = frame("center"); n = note(f, LONG, -1);
        paras(n, function (p) { p.justification = Justification.CENTER_ALIGN; });
        f = frame("right"); n = note(f, LONG, -1);
        paras(n, function (p) { p.justification = Justification.RIGHT_ALIGN; });
        f = frame("justify"); n = note(f, LONG, -1);
        paras(n, function (p) { p.justification = Justification.LEFT_JUSTIFIED; });
        f = frame("indents"); n = note(f, LONG, -1);
        paras(n, function (p) { p.leftIndent = 24; p.firstLineIndent = -12; p.rightIndent = 36; });
        f = frame("leading"); n = note(f, LONG, -1);
        n.texts[0].leading = 18;
        f = frame("spacing"); n = note(f, LONG, -1);
        paras(n, function (p) {
            p.maximumWordSpacing = 200; p.desiredWordSpacing = 150; p.minimumWordSpacing = 80;
            p.maximumLetterSpacing = 20; p.desiredLetterSpacing = 10; p.minimumLetterSpacing = 0;
            p.minimumGlyphScaling = 85; p.desiredGlyphScaling = 90; p.maximumGlyphScaling = 100;
        });
        f = frame("spacebefore"); n = note(f, SHORT, -1);
        paras(n, function (p) { p.spaceBefore = 12; });
        f = frame("twonotes");
        n = note(f, SHORT, -1);
        paras(n, function (p) { p.spaceBefore = 12; p.spaceAfter = 9; });
        n = note(f, SHORT, 4);
        paras(n, function (p) { p.spaceBefore = 12; p.spaceAfter = 9; });
        f = frame("twoparas");
        n = note(f, SHORT, -1);
        n = note(f, "The first paragraph of a note.\rIts second paragraph.", 4);
        paras(n, function (p) { p.spaceBefore = 12; p.spaceAfter = 9; });
        f = frame("styled");
        var st = doc.paragraphStyles.add({
            name: "Note", appliedFont: "Open Sans", fontStyle: "Regular", pointSize: 10,
            leftIndent: 24, justification: Justification.RIGHT_ALIGN, leading: 15
        });
        n = note(f, LONG, -1, true);
        paras(n, function (p) { p.applyParagraphStyle(st, true); });
        f = frame("tabs"); n = note(f, LONG, -1);
        paras(n, function (p) { p.tabStops.add({ position: 20 }); });
        // Body text, no note: two paragraphs set alike, which InDesign
        // writes as one range with a <Br/> between them.
        f = frame("bodyparas");
        f.contents = "First body paragraph.\rSecond body paragraph.";
        f.parentStory.texts[0].appliedFont = "Open Sans";
        f.parentStory.texts[0].fontStyle = "Regular";
        f.parentStory.texts[0].pointSize = 12;
        paras(f.parentStory, function (p) { p.spaceBefore = 12; p.spaceAfter = 9; });
        // The same with an empty paragraph between the two.
        f = frame("bodyempty");
        f.contents = "First body paragraph.\r\rThird body paragraph.";
        f.parentStory.texts[0].appliedFont = "Open Sans";
        f.parentStory.texts[0].fontStyle = "Regular";
        f.parentStory.texts[0].pointSize = 12;
        paras(f.parentStory, function (p) { p.spaceBefore = 12; p.spaceAfter = 9; });
        // Body text, no note: "1", a tab, then text, flush right and centred.
        var aligned = [["bodyright", Justification.RIGHT_ALIGN], ["bodycenter", Justification.CENTER_ALIGN]];
        for (var al = 0; al < aligned.length; al++) {
            f = frame(aligned[al][0]);
            f.contents = "1\t" + LONG;
            f.parentStory.texts[0].appliedFont = "Open Sans";
            f.parentStory.texts[0].fontStyle = "Regular";
            f.parentStory.texts[0].pointSize = 10;
            f.parentStory.paragraphs[0].justification = aligned[al][1];
        }

        doc.recompose();
        for (var c = 0; c < cases.length; c++) {
            var name = cases[c][0], tf = cases[c][1], story = tf.parentStory;
            var body = [];
            for (var b = 0; b < story.lines.length; b++) body.push(story.lines[b].baseline);
            // Each body paragraph's first baseline, an empty one's too.
            var bodyLines = [];
            for (var bl = 0; bl < story.lines.length; bl++) {
                var bln = story.lines[bl], bip = [];
                for (var bx = 0; bx < bln.insertionPoints.length; bx++) {
                    bip.push(bln.insertionPoints[bx].horizontalOffset);
                }
                bodyLines.push({ baseline: bln.baseline, text: bln.contents, ip: bip });
            }
            var starts = [];
            for (var sp = 0; sp < story.paragraphs.length; sp++) {
                starts.push(story.paragraphs[sp].insertionPoints[0].baseline);
            }
            var notes = [];
            for (var k = 0; k < story.footnotes.length; k++) {
                var fn2 = story.footnotes[k], ps = [];
                for (var pi = 0; pi < fn2.paragraphs.length; pi++) {
                    var p = fn2.paragraphs[pi], lines = [];
                    for (var li = 0; li < p.lines.length; li++) {
                        var ln = p.lines[li], ip = [];
                        for (var x = 0; x < ln.insertionPoints.length; x++) {
                            ip.push(ln.insertionPoints[x].horizontalOffset);
                        }
                        var lead = typeof ln.leading === "number" ? ln.leading : String(ln.leading);
                        lines.push({ baseline: ln.baseline, leading: lead, text: ln.contents, ip: ip });
                    }
                    ps.push({ lines: lines });
                }
                // The note's order in the frame's area: by its reference.
                notes.push({ at: fn2.storyOffset.index, paragraphs: ps });
            }
            var inset = tf.textFramePreferences.insetSpacing;
            result.frames[name] = {
                frame: tf.geometricBounds, inset: inset, page: tf.parentPage.documentOffset,
                body: body, bodyLines: bodyLines, paragraphs: starts, notes: notes
            };
        }
        doc.exportFile(ExportFormat.PDF_TYPE, File(OUT.fsName + "/footnote-paragraphs.pdf"));
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(OUT.fsName + "/footnote-paragraphs.idml"));
    } catch (e) {
        result.errors.push(String(e) + " @" + e.line);
    }
    doc.close(SaveOptions.NO);
    var file = File(OUT.fsName + "/footnote-paragraphs.json");
    file.encoding = "UTF-8";
    file.open("w");
    file.write(json(result));
    file.close();
})();
