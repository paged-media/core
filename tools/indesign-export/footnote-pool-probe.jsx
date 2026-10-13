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

// Ask InDesign where it puts a frame's footnote area
// (footnote-pool-probe.sh drives it). For each case the IDML is opened
// afresh, one setting is changed through the DOM, and the probe reports
// the frame's bounds and the baseline of every body line and every note
// line, in points from the page top. The cases vary what can move the
// area: the minimum space before the first note (`spacer`), the
// separator rule, the body's leading, the notes' size and first-baseline
// offset, and a note short enough for the area to fit.
//
// Inputs: PROBE_IDML, PROBE_OUT.
(function () {
    var IDML = $.global.PROBE_IDML, OUT = $.global.PROBE_OUT;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    var cases = [
        { name: "as-authored", apply: function (d, s) {} },
        { name: "spacer-0", apply: function (d, s) { d.footnoteOptions.spacer = 0; } },
        { name: "spacer-12", apply: function (d, s) { d.footnoteOptions.spacer = 12; } },
        { name: "rule-off", apply: function (d, s) { d.footnoteOptions.ruleOn = false; } },
        { name: "rule-offset-10", apply: function (d, s) { d.footnoteOptions.ruleOffset = 10; } },
        { name: "rule-weight-4", apply: function (d, s) { d.footnoteOptions.ruleLineWeight = 4; } },
        { name: "body-leading-20", apply: function (d, s) { s.texts[0].leading = 20; } },
        { name: "notes-8pt", apply: function (d, s) {
            for (var i = 0; i < s.footnotes.length; i++) s.footnotes[i].texts[0].pointSize = 8;
        } },
        { name: "first-baseline-leading", apply: function (d, s) {
            d.footnoteOptions.footnoteFirstBaselineOffset = FootnoteFirstBaseline.LEADING_OFFSET;
        } },
        { name: "first-baseline-cap", apply: function (d, s) {
            d.footnoteOptions.footnoteFirstBaselineOffset = FootnoteFirstBaseline.CAP_HEIGHT;
        } },
        { name: "short-third-note", apply: function (d, s) {
            var t = s.footnotes[s.footnotes.length - 1].texts[0];
            t.contents = "Third, short.";
        } }
    ];
    function n(v) { return Math.round(v * 1000) / 1000; }
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/[\r\n\t\u2029\u2028]/g, " ") + '"'; }
    function opt(o, k) { try { var v = o[k]; return typeof v === "number" ? n(v) : q(v); } catch (e) { return "null"; } }
    function lines(texts) {
        var out = [];
        for (var i = 0; i < texts.lines.length; i++) {
            var l = texts.lines[i];
            out.push("{\"baseline\":" + opt(l, "baseline") + ",\"pointSize\":" + opt(l, "pointSize")
                + ",\"leading\":" + opt(l, "leading") + ",\"text\":" + opt(l, "contents") + "}");
        }
        return "[" + out.join(",") + "]";
    }
    var results = [];
    for (var c = 0; c < cases.length; c++) {
        var doc = app.open(File(IDML), false);
        try {
            var frame = null, story = null;
            for (var si = 0; si < doc.stories.length && !story; si++) {
                if (doc.stories[si].footnotes.length > 0) story = doc.stories[si];
            }
            frame = story.textContainers[0];
            var err = "null";
            try { cases[c].apply(doc, story); } catch (e) { err = q(e); }
            doc.recompose();
            var notes = [];
            for (var f = 0; f < story.footnotes.length; f++) {
                notes.push(lines(story.footnotes[f].texts[0]));
            }
            var o = doc.footnoteOptions;
            var gb = frame.geometricBounds;
            var inset = [0, 0, 0, 0];
            try { inset = frame.textFramePreferences.insetSpacing; } catch (e) {}
            results.push("{\"case\":" + q(cases[c].name) + ",\"error\":" + err
                + ",\"frame\":{\"top\":" + n(gb[0]) + ",\"bottom\":" + n(gb[2])
                + ",\"insetTop\":" + n(inset[0]) + ",\"insetBottom\":" + n(inset[2])
                + ",\"overflows\":" + opt(frame, "overflows") + "}"
                + ",\"options\":{\"spacer\":" + opt(o, "spacer") + ",\"spaceBetween\":" + opt(o, "spaceBetween")
                + ",\"ruleOn\":" + opt(o, "ruleOn") + ",\"ruleOffset\":" + opt(o, "ruleOffset")
                + ",\"ruleLineWeight\":" + opt(o, "ruleLineWeight")
                + ",\"firstBaseline\":" + opt(o, "footnoteFirstBaselineOffset")
                + ",\"minimumFirstBaseline\":" + opt(o, "footnoteMinimumFirstBaselineOffset") + "}"
                + ",\"body\":" + lines(story.texts[0])
                + ",\"notes\":[" + notes.join(",") + "]}");
        } finally {
            doc.close(SaveOptions.NO);
        }
    }
    var out = File(OUT);
    out.encoding = "UTF-8";
    out.open("w");
    out.write("{\"probe\":\"footnote-pool\",\"indesign\":" + q(app.version) + ",\"cases\":[\n"
        + results.join(",\n") + "\n]}\n");
    out.close();
})();
