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

// Ask InDesign what a cross-reference says, and what Update
// Cross-References writes (xref-probe.sh drives it). Builds a three-page
// document: page 1 holds one reference per default cross-reference format
// to a numbered heading on page 2 and one to a text anchor; then reports
// each source's text, moves the heading on to page 3, updates every
// reference, and reports them again. Exports InDesign's IDML (before the
// move) and PDF.
//
// Inputs: PROBE_OUT (JSON), PROBE_IDML, PROBE_PDF, and PROBE_PDF_MOVED
// (optional: the PDF after the move, before any update).
(function () {
    var OUT = $.global.PROBE_OUT;
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
        while (doc.pages.length < 3) doc.pages.add();
        var frames = [];
        for (var p = 0; p < 3; p++) {
            frames.push(doc.pages[p].textFrames.add({ geometricBounds: [20, 20, 280, 380] }));
            if (p) frames[p - 1].nextTextFrame = frames[p];
        }
        var story = frames[0].parentStory;
        var heading = doc.paragraphStyles.add({ name: "Heading", pointSize: 14 });
        heading.bulletsAndNumberingListType = ListType.NUMBERED_LIST;
        var formats = [];
        for (var f = 0; f < doc.crossReferenceFormats.length; f++) formats.push(doc.crossReferenceFormats[f]);
        // Page 1: one line per format, the reference at its end.
        var lines = [];
        for (var k = 0; k < formats.length; k++) lines.push(formats[k].name + ": ");
        lines.push("Anchor: ");
        story.contents = lines.join("\r") + "\rBody on page one.";
        // Page 2: a heading, then text.
        story.insertionPoints[-1].contents = "\r";
        story.paragraphs[-1].startParagraph = StartParagraph.NEXT_FRAME;
        story.insertionPoints[-1].contents = "Where Things Are";
        story.paragraphs[-1].appliedParagraphStyle = heading;
        story.insertionPoints[-1].contents = "\rThe anchored word sits here.";
        var target = story.paragraphs[lines.length + 1];
        var dest = doc.paragraphDestinations.add(target.insertionPoints[0]);
        var anchorPara = story.paragraphs[lines.length + 2];
        var anchorWord = anchorPara.words[1];
        var textDest = doc.hyperlinkTextDestinations.add(anchorWord.insertionPoints[0], { name: "the anchor" });
        var sources = [];
        for (var m = 0; m < formats.length; m++) {
            var at = story.paragraphs[m].insertionPoints[-2];
            try {
                var src = doc.crossReferenceSources.add(at, formats[m]);
                doc.hyperlinks.add(src, dest);
                sources.push([formats[m].name, src]);
            } catch (e) { log.push(formats[m].name + ": " + e); }
        }
        try {
            var last = story.paragraphs[formats.length].insertionPoints[-2];
            var src2 = doc.crossReferenceSources.add(last, doc.crossReferenceFormats.itemByName("Text Anchor Name & Page Number"));
            doc.hyperlinks.add(src2, textDest);
            sources.push(["anchor: Text Anchor Name & Page Number", src2]);
        } catch (e) { log.push("anchor: " + e); }
        doc.recompose();
        function dump() {
            var o = "[";
            for (var i = 0; i < sources.length; i++) {
                var t;
                try { t = sources[i][1].sourceText.contents; } catch (e) { t = "?" + e; }
                o += (i ? "," : "") + "[" + q(sources[i][0]) + "," + q(spell(String(t))) + "]";
            }
            return o + "]";
        }
        var formatsOut = "[";
        for (var n = 0; n < formats.length; n++) {
            var bb = formats[n].buildingBlocks, parts = [];
            for (var b = 0; b < bb.length; b++) parts.push(String(bb[b].blockType) + "|" + spell(String(bb[b].customText)));
            formatsOut += (n ? "," : "") + "[" + q(formats[n].name) + "," + q(parts.join(" + ")) + "]";
        }
        formatsOut += "]";
        var before = dump();
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
        doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
        // Move the heading on a page; the stored texts stay until updated.
        story.paragraphs[lines.length + 1].startParagraph = StartParagraph.NEXT_PAGE;
        var blank = story.paragraphs[lines.length];
        blank.insertionPoints[-2].contents = SpecialCharacters.FRAME_BREAK;
        doc.recompose();
        var moved = dump();
        // What prints after the move, before any update: is the page
        // number live?
        if ($.global.PROBE_PDF_MOVED) doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF_MOVED));
        try { doc.crossReferenceSources.everyItem().update(); } catch (e) { log.push("update: " + e); }
        doc.recompose();
        var updated = dump();
        var out = "{\"log\":[";
        for (var l = 0; l < log.length; l++) out += (l ? "," : "") + q(log[l]);
        out += "],\"formats\":" + formatsOut + ",\"before\":" + before + ",\"moved\":" + moved + ",\"updated\":" + updated
            + ",\"headingPage\":" + q(story.paragraphs[lines.length + 1].parentTextFrames[0].parentPage.name) + "}";
        var f2 = File(OUT);
        f2.encoding = "UTF-8";
        f2.open("w");
        f2.write(out);
        f2.close();
    }
})();
