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

// Ask InDesign how it writes a page number in every PageNumberStyle
// (numbering-styles-probe.sh drives it). For each style its DOM knows
// (read by reflection, so none is missed), four one-page sections start
// at 1, 12, 199 and 2026; each page's name is the label InDesign prints.
// Reports {style, start, label} for every page, and the document's own
// IDML (so the attribute spelling of each style can be read off it).
//
// Inputs: PROBE_OUT (the JSON written), PROBE_RESAVE (InDesign's IDML).
(function () {
    var OUT = $.global.PROBE_OUT, RESAVE = $.global.PROBE_RESAVE;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = app.documents.add(false);
    doc.documentPreferences.facingPages = false;
    var styles = [];
    var props = PageNumberStyle.reflect.properties;
    for (var i = 0; i < props.length; i++) {
        var n = props[i].name;
        if (n === "__proto__" || n === "reflect" || n === "toString" || n === "valueOf") continue;
        styles.push(n);
    }
    // PROBE_PLAN (optional): {style: [starts]} asks particular numbers of
    // particular styles instead of 1, 12, 199, 2026 of every style.
    var plan = $.global.PROBE_PLAN;
    if (plan) {
        styles = [];
        for (var key in plan) styles.push(key);
    }
    var rows = [];
    var page = 0;
    for (var s = 0; s < styles.length; s++) {
        var starts = plan ? plan[styles[s]] : [1, 12, 199, 2026];
        for (var k = 0; k < starts.length; k++) {
            if (page > 0) doc.pages.add();
            var p = doc.pages[page];
            var sec = page === 0 ? doc.sections[0] : doc.sections.add(p);
            try {
                sec.continueNumbering = false;
                sec.pageNumberStart = starts[k];
                sec.pageNumberStyle = PageNumberStyle[styles[s]];
            } catch (e) {
                rows.push('    {"style": "' + styles[s] + '", "start": ' + starts[k] + ', "error": "' + String(e).replace(/"/g, "'") + '"}');
                page++;
                continue;
            }
            rows.push('    {"style": "' + styles[s] + '", "start": ' + starts[k] + ', "label": "' + p.name.replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"}');
            page++;
        }
    }
    doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(RESAVE));
    doc.close(SaveOptions.NO);
    var json = "{\n  \"indesign\": \"" + app.version + "\",\n  \"pages\": [\n" + rows.join(",\n") + "\n  ]\n}\n";
    var f = File(OUT);
    f.encoding = "UTF-8";
    f.lineFeed = "Unix";
    f.open("w");
    f.write(json);
    f.close();
})();
