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


// Ask InDesign how it records tracked changes (track-changes-probe.sh
// drives it). Builds one page with a story "Alpha beta gamma delta.",
// turns Track Changes on, inserts " INSERTED" after "beta", deletes
// "gamma ", and reports each change (type, user, date, text) and what the
// story's contents read; exports IDML and PDF. Then accepts the first
// change and rejects the second, and reports the contents again.
//
// With PROBE_IN (an absolute .idml) it opens that package instead and
// reports its first story's changes and contents, and exports its PDF.
//
// Inputs: PROBE_OUT (JSON), PROBE_IDML, PROBE_PDF, PROBE_IN (optional).
(function () {
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/[\r\n\t]/g, " ") + '"'; }
    var log = [];
    var IN = $.global.PROBE_IN;
    var doc = IN ? app.open(File(IN), true) : app.documents.add(true);
    try { if (IN) read(); else build(); } finally { doc.close(SaveOptions.NO); }
    function read() {
        doc.recompose();
        var story = doc.stories[0];
        var out = File($.global.PROBE_OUT); out.encoding = "UTF-8"; out.open("w");
        out.write('{"changes": ' + changes(story) + ', "contents": ' + q(String(story.contents)) + "}");
        out.close();
        doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
    }
    function changes(story) {
        var out = [];
        for (var i = 0; i < story.changes.length; i++) {
            var c = story.changes[i], t = "";
            try { t = c.texts[0].contents; } catch (e) { t = "?" + e; }
            out.push("[" + q(String(c.changeType)) + ", " + q(c.userName) + ", " + q(String(c.date)) + ", " + q(t) + "]");
        }
        return "[" + out.join(", ") + "]";
    }
    function build() {
        doc.documentPreferences.facingPages = false;
        doc.documentPreferences.pageWidth = 400;
        doc.documentPreferences.pageHeight = 300;
        var f = doc.pages[0].textFrames.add({ geometricBounds: [20, 20, 280, 380] });
        var story = f.parentStory;
        story.contents = "Alpha beta gamma delta.";
        story.trackChanges = true;
        story.insertionPoints[10].contents = " INSERTED";
        var s = String(story.contents);
        var at = s.indexOf("gamma ");
        story.characters.itemByRange(at, at + 5).remove();
        doc.recompose();
        var before = changes(story), contents = String(story.contents);
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
        doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
        try { story.changes[0].accept(); } catch (e) { log.push("accept: " + e); }
        try { story.changes[0].reject(); } catch (e) { log.push("reject: " + e); }
        var after = String(story.contents);
        var out = File($.global.PROBE_OUT); out.encoding = "UTF-8"; out.open("w");
        out.write('{"log": [' + (log.length ? q(log.join("; ")) : "") + '], "changes": ' + before + ', "contents": ' + q(contents) + ', "afterAcceptReject": ' + q(after) + "}");
        out.close();
    }
})();
