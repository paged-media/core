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


// Ask InDesign what a cross-reference's chapter number prints
// (xref-chapter-probe.sh drives it). Builds a two-page document whose
// chapter number is 7 in Roman numerals, a heading on page 2 and, on
// page 1, a reference in a format "Chapter <chapter number>, page <page
// number>". Reports the reference's text and exports IDML and PDF; then
// sets the chapter number to 9 and exports a second PDF WITHOUT updating
// the reference, to ask whether the number is live.
//
// Inputs: PROBE_OUT (JSON), PROBE_IDML, PROBE_PDF, PROBE_PDF_CHANGED.
(function () {
    var OUT = $.global.PROBE_OUT;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"'; }
    var log = [];
    var doc = app.documents.add(true);
    try { build(); } finally { doc.close(SaveOptions.NO); }
    function build() {
        doc.documentPreferences.facingPages = false;
        doc.documentPreferences.pageWidth = 400;
        doc.documentPreferences.pageHeight = 300;
        while (doc.pages.length < 2) doc.pages.add();
        var cn = doc.chapterNumberPreferences;
        cn.chapterNumberSource = ChapterNumberSources.USER_DEFINED;
        cn.chapterNumber = 7;
        try { cn.chapterNumberFormat = "I, II, III, IV..."; } catch (e) { log.push("format: " + e); }
        var f1 = doc.pages[0].textFrames.add({ geometricBounds: [20, 20, 280, 380] });
        var f2 = doc.pages[1].textFrames.add({ geometricBounds: [20, 20, 280, 380] });
        f1.nextTextFrame = f2;
        var story = f1.parentStory;
        story.contents = "See: \rHeading Two";
        story.paragraphs[1].startParagraph = StartParagraph.NEXT_FRAME;
        var dest = doc.paragraphDestinations.add(story.paragraphs[1].insertionPoints[0]);
        var fmt = doc.crossReferenceFormats.add({ name: "Chapter and Page" });
        var bb = fmt.buildingBlocks;
        bb.add(BuildingBlockTypes.CUSTOM_STRING_BUILDING_BLOCK, { customText: "Chapter " });
        bb.add(BuildingBlockTypes.CHAPTER_NUMBER_BUILDING_BLOCK);
        bb.add(BuildingBlockTypes.CUSTOM_STRING_BUILDING_BLOCK, { customText: ", page " });
        bb.add(BuildingBlockTypes.PAGE_NUMBER_BUILDING_BLOCK);
        var src = doc.crossReferenceSources.add(story.paragraphs[0].insertionPoints[-2], fmt);
        doc.hyperlinks.add(src, dest);
        doc.recompose();
        var before = String(src.sourceText.contents);
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
        doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
        cn.chapterNumber = 9;
        doc.recompose();
        var changed = String(src.sourceText.contents);
        if ($.global.PROBE_PDF_CHANGED) doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF_CHANGED));
        var out = "{\"log\":[";
        for (var l = 0; l < log.length; l++) out += (l ? "," : "") + q(log[l]);
        out += "],\"before\":" + q(before) + ",\"afterChange\":" + q(changed)
            + ",\"chapterFormat\":" + q(cn.chapterNumberFormat) + "}";
        var f = File(OUT);
        f.encoding = "UTF-8";
        f.open("w");
        f.write(out);
        f.close();
    }
})();
