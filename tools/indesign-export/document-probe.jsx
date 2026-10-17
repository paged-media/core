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

// Ask InDesign how it reads a document's setup (document-probe.sh drives
// it): opens PROBE_IN (an absolute .idml) and reports its document
// preferences (page size, facing pages, page count, bleed, columns), its
// masters and pages, its swatches and its paragraph styles to PROBE_OUT.
// Without PROBE_IN it reports a NEW document's (InDesign's defaults) and
// exports that blank document as IDML to PROBE_IDML.
//
// Inputs: PROBE_IN (optional), PROBE_OUT (JSON), PROBE_IDML, PROBE_PDF.
(function () {
    var OUT = $.global.PROBE_OUT;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"'; }
    var IN = $.global.PROBE_IN;
    var doc = IN ? app.open(File(IN), true) : app.documents.add(true);
    try {
        var dp = doc.documentPreferences, names = [];
        var masters = [];
        for (var m = 0; m < doc.masterSpreads.length; m++) masters.push(q(doc.masterSpreads[m].name));
        var pages = [];
        for (var p = 0; p < doc.pages.length; p++) {
            var pg = doc.pages[p], b = pg.bounds;
            pages.push("[" + q(pg.name) + ", " + q(pg.appliedMaster ? pg.appliedMaster.name : "none") + ", " + (b[3] - b[1]) + ", " + (b[2] - b[0]) + "]");
        }
        for (var s = 0; s < doc.swatches.length; s++) names.push(q(doc.swatches[s].name));
        var styles = [];
        for (var k = 0; k < doc.allParagraphStyles.length; k++) styles.push(q(doc.allParagraphStyles[k].name));
        var out = '{\n  "pageWidth": ' + dp.pageWidth + ', "pageHeight": ' + dp.pageHeight +
            ', "facingPages": ' + dp.facingPages + ', "pagesPerDocument": ' + dp.pagesPerDocument +
            ', "bleed": [' + [dp.documentBleedTopOffset, dp.documentBleedInsideOrLeftOffset, dp.documentBleedBottomOffset, dp.documentBleedOutsideOrRightOffset].join(", ") + ']' +
            ',\n  "masters": [' + masters.join(", ") + '],\n  "pages": [' + pages.join(", ") +
            '],\n  "swatches": [' + names.join(", ") + '],\n  "paragraphStyles": [' + styles.join(", ") + ']\n}\n';
        var f = File(OUT); f.encoding = "UTF-8"; f.open("w"); f.write(out); f.close();
        if (!IN) doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
    } finally { doc.close(SaveOptions.NO); }
})();
