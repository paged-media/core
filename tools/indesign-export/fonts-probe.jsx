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


// Ask InDesign which fonts a package's text resolves to (fonts-probe.sh
// drives it): opens PROBE_IN (an absolute .idml) and reports, per story,
// its first character's applied font and that font's status, and the
// document's font list with each font's status, to PROBE_OUT; exports a
// PDF to PROBE_PDF.
(function () {
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\t/g, "\\t") + '"'; }
    var doc = app.open(File($.global.PROBE_IN), true);
    try {
        doc.recompose();
        var stories = [];
        for (var s = 0; s < doc.stories.length; s++) {
            var st = doc.stories[s];
            if (!st.characters.length) continue;
            var f = st.characters[0].appliedFont;
            var name = f && f.isValid ? f.name : String(f);
            var status = f && f.isValid ? String(f.status) : "?";
            stories.push("[" + q(name) + ", " + q(status) + "]");
        }
        var fonts = [];
        for (var i = 0; i < doc.fonts.length; i++) fonts.push("[" + q(doc.fonts[i].name) + ", " + q(String(doc.fonts[i].status)) + "]");
        if ($.global.PROBE_PDF) doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
        var out = File($.global.PROBE_OUT); out.encoding = "UTF-8"; out.open("w");
        out.write('{"stories": [' + stories.join(", ") + '], "fonts": [' + fonts.join(", ") + "]}");
        out.close();
    } finally { doc.close(SaveOptions.NO); }
})();
