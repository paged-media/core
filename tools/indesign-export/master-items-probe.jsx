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

// Ask InDesign where ShowMasterItems lives (master-items-probe.sh drives
// it). PROBE_MODE "build": a one-page document whose master carries a
// black rectangle, exported as master-items.idml. PROBE_MODE "open": open
// each variant the shell script derived (the spread's and the page's
// ShowMasterItems disagreeing) and print it to <variant>.pdf.
(function () {
    var out = Folder(String($.global.PROBE_OUT));
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    if ($.global.PROBE_MODE === "build") {
        var doc = app.documents.add(false);
        doc.documentPreferences.facingPages = false;
        var r = doc.masterSpreads[0].pages[0].rectangles.add({ geometricBounds: [100, 100, 300, 400] });
        r.fillColor = doc.swatches.itemByName("Black");
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(out.fsName + "/master-items.idml"));
        doc.close(SaveOptions.NO);
        return;
    }
    var names = ["spread-true-page-false", "spread-false-page-true"];
    for (var i = 0; i < names.length; i++) {
        var d = app.open(File(out.fsName + "/" + names[i] + ".idml"), false);
        d.exportFile(ExportFormat.PDF_TYPE, File(out.fsName + "/" + names[i] + ".pdf"));
        d.close(SaveOptions.NO);
    }
})();
