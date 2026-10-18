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


// Ask InDesign which structure elements a paragraph style's PDF export
// tag makes (tagged-styles-probe.sh drives it). Builds one page: a story
// of Heading (PDF tag H1), Body (no tag), Sub (H2) and Body paragraphs,
// and a second frame in Folio (PDF tag Artifact). Exports InDesign's IDML
// and a tagged PDF with the application's PDF preferences; the PDF's
// structure is read with pdf-structure.py.
//
// Inputs: PROBE_OUT (JSON), PROBE_IDML, PROBE_PDF.
(function () {
    var OUT = $.global.PROBE_OUT;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"'; }
    var log = [];
    var doc = app.documents.add(true);
    try { build(); } finally { doc.close(SaveOptions.NO); }
    function tag(style, t) {
        try { style.styleExportTagMaps.add({ exportType: "PDF", exportTag: t, exportClass: "", exportAttributes: "" }); }
        catch (e) { log.push(style.name + ": " + e); }
    }
    function build() {
        doc.documentPreferences.facingPages = false;
        doc.documentPreferences.pageWidth = 400;
        doc.documentPreferences.pageHeight = 300;
        var heading = doc.paragraphStyles.add({ name: "Heading", pointSize: 16 });
        var sub = doc.paragraphStyles.add({ name: "Sub", pointSize: 13 });
        var body = doc.paragraphStyles.add({ name: "Body", pointSize: 10 });
        var folio = doc.paragraphStyles.add({ name: "Folio", pointSize: 8 });
        tag(heading, "H1");
        tag(sub, "H2");
        tag(folio, "Artifact");
        var page = doc.pages[0];
        var main = page.textFrames.add({ geometricBounds: [20, 20, 250, 380] });
        main.parentStory.contents = "A Heading\rBody one.\rA Sub\rBody two.";
        var styles = [heading, body, sub, body];
        for (var i = 0; i < 4; i++) main.parentStory.paragraphs[i].appliedParagraphStyle = styles[i];
        var foot = page.textFrames.add({ geometricBounds: [260, 20, 280, 380] });
        foot.parentStory.contents = "Folio 1";
        foot.parentStory.paragraphs[0].appliedParagraphStyle = folio;
        doc.recompose();
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
        var prefs = app.pdfExportPreferences;
        var was = prefs.includeStructure;
        prefs.includeStructure = true;
        try { doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF), false); }
        finally { prefs.includeStructure = was; }
        var out = "{\"log\":[";
        for (var l = 0; l < log.length; l++) out += (l ? "," : "") + q(log[l]);
        out += "]}";
        var f = File(OUT); f.encoding = "UTF-8"; f.open("w"); f.write(out); f.close();
    }
})();
