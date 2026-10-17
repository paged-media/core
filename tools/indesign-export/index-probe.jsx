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

// Ask InDesign what Generate Index writes (index-probe.sh drives it).
// With PROBE_IN, opens that IDML (core's `index` sample) and generates
// its index; otherwise builds a four-page document whose threaded story
// runs over pages 1-3
// and carries page references to topics at two levels, a topic with
// references on two pages, one with two on the same page, three on
// consecutive pages, and a See cross-reference; then generates the index
// with InDesign's default options into a frame on page 4. Reports the
// index options as the DOM sees them, the generated story paragraph by
// paragraph (style, character ranges with style and text, specials as
// code points), and exports InDesign's IDML and PDF.
//
// Inputs: PROBE_IN (optional IDML to open), PROBE_OUT (JSON), PROBE_IDML,
// PROBE_PDF.
(function () {
    var OUT = $.global.PROBE_OUT, IN = $.global.PROBE_IN;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"'; }
    function spell(s) {
        var o = "";
        for (var i = 0; i < s.length; i++) {
            var c = s.charCodeAt(i);
            o += (c < 32 || c > 126) ? "<U+" + ("000" + c.toString(16).toUpperCase()).slice(-4) + ">" : s.charAt(i);
        }
        return o;
    }
    var log = [];
    var doc = IN ? app.open(File(IN), true) : app.documents.add(true);
    try { build(); } finally { doc.close(SaveOptions.NO); }
    function build() {
        if (!IN) populate();
        generateAndDump();
    }
    function populate() {
        doc.documentPreferences.facingPages = false;
        doc.documentPreferences.pageWidth = 400;
        doc.documentPreferences.pageHeight = 300;
        while (doc.pages.length < 4) doc.pages.add();
        var frames = [];
        for (var p = 0; p < 3; p++) {
            frames.push(doc.pages[p].textFrames.add({ geometricBounds: [20, 20, 280, 380] }));
            if (p) frames[p - 1].nextTextFrame = frames[p];
        }
        var story = frames[0].parentStory;
        // One paragraph per page; each later one starts the next frame.
        var texts = ["Apples and cherries grow here. Apples again.", "Bananas and sour cherries. Plums too.", "Apples once more. Plums and plums."];
        for (var i = 0; i < texts.length; i++) {
            story.insertionPoints[-1].contents = texts[i] + (i < texts.length - 1 ? "\r" : "");
        }
        for (var k = 1; k < 3; k++) story.paragraphs[k].startParagraph = StartParagraph.NEXT_FRAME;
        var index = doc.indexes.length ? doc.indexes[0] : doc.indexes.add();
        var apples = index.topics.add("Apples");
        var bananas = index.topics.add("Bananas");
        var cherries = index.topics.add("Cherries");
        var sour = cherries.topics.add("Sour");
        var plums = index.topics.add("Plums");
        var fruit = index.topics.add("Fruit");
        function at(par, word) {
            var t = story.paragraphs[par].contents;
            return story.paragraphs[par].insertionPoints[t.indexOf(word)];
        }
        apples.pageReferences.add(at(0, "Apples"), PageReferenceType.CURRENT_PAGE);
        apples.pageReferences.add(at(0, "Apples again"), PageReferenceType.CURRENT_PAGE);
        apples.pageReferences.add(at(2, "Apples"), PageReferenceType.CURRENT_PAGE);
        cherries.pageReferences.add(at(0, "cherries"), PageReferenceType.CURRENT_PAGE);
        sour.pageReferences.add(at(1, "sour"), PageReferenceType.CURRENT_PAGE);
        bananas.pageReferences.add(at(1, "Bananas"), PageReferenceType.CURRENT_PAGE);
        plums.pageReferences.add(at(1, "Plums"), PageReferenceType.CURRENT_PAGE);
        plums.pageReferences.add(at(2, "Plums"), PageReferenceType.CURRENT_PAGE);
        try {
            fruit.crossReferences.add(apples, CrossReferenceType.SEE);
        } catch (e) { log.push("crossReference: " + e); }
    }
    function generateAndDump() {
        var index = doc.indexes[0];
        // An opened document is composed lazily: without this, Generate
        // Index finds no page for any reference and lists none.
        doc.recompose();

        var opts = "";
        try {
            var go = doc.indexGenerationOptions;
            var keys = ["title", "replaceExistingIndex", "includeSectionHeadings", "includeEmptyIndexSections",
                "followingTopicSeparator", "betweenPageNumbers", "betweenEntries", "beforeCrossReference",
                "pageRangeSeparator", "entryEndSeparator", "indexFormat"];
            for (var j = 0; j < keys.length; j++) {
                var v;
                try { v = go[keys[j]]; } catch (e) { v = "?"; }
                opts += (j ? "," : "") + q(keys[j]) + ":" + q(spell(String(v)));
            }
            var styles = ["titleStyle", "sectionHeadingStyle", "pageNumberStyle", "crossReferenceStyle", "level1Style", "level2Style"];
            for (var m = 0; m < styles.length; m++) {
                var nm;
                try { nm = go[styles[m]].name; } catch (e) { nm = "?"; }
                opts += "," + q(styles[m]) + ":" + q(nm);
            }
        } catch (e) { log.push("options: " + e); }

        var out = [];
        try {
            app.activeWindow.activePage = doc.pages[3];
            out = index.generate(doc.pages[3], [30, 30]);
        } catch (e) { log.push("generate: " + e); }
        var gen = null;
        for (var s = 0; s < doc.stories.length; s++) {
            if (doc.stories[s].storyType == StoryTypes.INDEXING_STORY) gen = doc.stories[s];
        }
        var res = "{\"log\":[";
        for (var l = 0; l < log.length; l++) res += (l ? "," : "") + q(log[l]);
        res += "],\"options\":{" + opts + "},\"index\":[";
        if (gen) {
            var ps = gen.paragraphs;
            for (var g = 0; g < ps.length; g++) {
                res += (g ? "," : "") + "{\"style\":" + q(ps[g].appliedParagraphStyle.name) + ",\"ranges\":[";
                var rs = ps[g].textStyleRanges;
                for (var r = 0; r < rs.length; r++) {
                    res += (r ? "," : "") + "[" + q(rs[r].appliedCharacterStyle.name) + "," + q(spell(rs[r].contents)) + "]";
                }
                res += "]}";
            }
        }
        res += "]}";
        var f = File(OUT);
        f.encoding = "UTF-8";
        f.open("w");
        f.write(res);
        f.close();
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
        doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
    }
})();
