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

// Ask InDesign what Update Table of Contents writes (toc-probe.sh drives
// it). Builds a four-page document whose threaded story holds headings
// at two levels on pages 1-3, a TOC style with a title and three entry
// rules (page number after the entry behind a tab, before it, none),
// then lets InDesign generate the table of contents into a frame on
// page 4. Reports the generated story paragraph by paragraph (style,
// contents with every special character spelled as its code point) and
// the document's hyperlink, destination and bookmark counts before and
// after, then exports InDesign's IDML and PDF so the spelling can be
// read.
//
// Inputs: PROBE_OUT (JSON), PROBE_IDML (IDML to write), PROBE_PDF.
(function () {
    var OUT = $.global.PROBE_OUT, IDML = $.global.PROBE_IDML, PDF = $.global.PROBE_PDF;
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
    var doc = app.documents.add(true);
    try { build(); } finally { doc.close(SaveOptions.NO); }
    function build() {
    doc.documentPreferences.facingPages = false;
    doc.documentPreferences.pageWidth = 400;
    doc.documentPreferences.pageHeight = 300;
    while (doc.pages.length < 4) doc.pages.add();
    var font = "Minion Pro\tRegular";
    var body = doc.paragraphStyles.add({ name: "Body", pointSize: 10, leading: 12 });
    var h1 = doc.paragraphStyles.add({ name: "H1", pointSize: 16, leading: 20, basedOn: body });
    var h2 = doc.paragraphStyles.add({ name: "H2", pointSize: 13, leading: 16, basedOn: body });
    var title = doc.paragraphStyles.add({ name: "TOC Title", pointSize: 14, leading: 18, basedOn: body });
    var t1 = doc.paragraphStyles.add({ name: "TOC 1", basedOn: body });
    var t2 = doc.paragraphStyles.add({ name: "TOC 2", basedOn: body, leftIndent: 12 });
    var t3 = doc.paragraphStyles.add({ name: "TOC 3", basedOn: body, leftIndent: 24 });
    var h3 = doc.paragraphStyles.add({ name: "H3", basedOn: body });
    var pageNum = doc.characterStyles.add({ name: "TOC Page" , fontStyle: "Bold"});

    // One threaded story over pages 1-3: H1 p1, H2 p1, H1 p2, H3 p2, H2 p3.
    var frames = [];
    for (var p = 0; p < 3; p++) {
        frames.push(doc.pages[p].textFrames.add({ geometricBounds: [20, 20, 280, 380] }));
        if (p) frames[p - 1].nextTextFrame = frames[p];
    }
    var story = frames[0].parentStory;
    var heads = [
        ["Getting Started", h1], ["First Steps", h2], ["Going Further", h1],
        ["Aside", h3], ["Last Words", h2]
    ];
    for (var i = 0; i < heads.length; i++) {
        story.insertionPoints[-1].contents = heads[i][0] + "\r";
        story.paragraphs[-1].appliedParagraphStyle = heads[i][1];
        story.insertionPoints[-1].contents = "Body text.\r";
        story.paragraphs[-1].appliedParagraphStyle = body;
        // Push the next heading onto the next page for the two page breaks.
        if (i === 1 || i === 3) {
            story.paragraphs[-1].startParagraph = StartParagraph.NEXT_PAGE;
        }
    }

    var ts = doc.tocStyles.add({ name: "Contents", title: "Contents", titleStyle: title });
    ts.tocStyleEntries.add({ name: "H1", formatStyle: t1, pageNumberPosition: PageNumberPosition.AFTER_ENTRY, separator: "^t", pageNumberStyle: pageNum, level: 1 });
    ts.tocStyleEntries.add({ name: "H2", formatStyle: t2, pageNumberPosition: PageNumberPosition.BEFORE_ENTRY, separator: " ", level: 2 });
    ts.tocStyleEntries.add({ name: "H3", formatStyle: t3, pageNumberPosition: PageNumberPosition.NONE, level: 3 });

    var before = [doc.hyperlinks.length, doc.hyperlinkTextDestinations.length, doc.bookmarks.length];
    var tocStories = [];
    try {
        app.activeWindow.activePage = doc.pages[3];
        tocStories = doc.createTOC(ts, false, undefined, [30, 30]);
    } catch (e) { log.push("createTOC: " + e); }
    var after = [doc.hyperlinks.length, doc.hyperlinkTextDestinations.length, doc.bookmarks.length];

    var out = "{\"log\":[";
    for (var l = 0; l < log.length; l++) out += (l ? "," : "") + q(log[l]);
    out += "],\"before\":[" + before.join(",") + "],\"after\":[" + after.join(",") + "],\"toc\":[";
    if (tocStories && tocStories.length) {
        var ps = tocStories[0].paragraphs;
        for (var k = 0; k < ps.length; k++) {
            out += (k ? "," : "") + "{\"style\":" + q(ps[k].appliedParagraphStyle.name)
                + ",\"contents\":" + q(spell(ps[k].contents)) + ",\"ranges\":[";
            var rs = ps[k].textStyleRanges;
            for (var r = 0; r < rs.length; r++) {
                out += (r ? "," : "") + "{\"char\":" + q(rs[r].appliedCharacterStyle.name)
                    + ",\"text\":" + q(spell(rs[r].contents)) + "}";
            }
            out += "]}";
        }
        var page = tocStories[0].textContainers[0].parentPage;
        out += "],\"tocPage\":" + q(page ? page.name : "none");
    } else {
        out += "]";
    }
    out += "}";
    var f = File(OUT);
    f.encoding = "UTF-8";
    f.open("w");
    f.write(out);
    f.close();
    doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(IDML));
    doc.exportFile(ExportFormat.PDF_TYPE, File(PDF));
    }
})();
