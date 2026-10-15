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

// Ask InDesign how it reads a frame's footnote layout
// (footnote-columns-probe.sh drives it). Opens the IDML, reports the
// document's footnote options and every text frame's footnote
// preferences as the DOM sees them (so the defaults InDesign fills in for
// an override that does not state them are visible), and saves the
// document as IDML again, so InDesign's own spelling can be read.
//
// Inputs: PROBE_IDML, PROBE_OUT, PROBE_RESAVE.
(function () {
    var IDML = $.global.PROBE_IDML, OUT = $.global.PROBE_OUT, RESAVE = $.global.PROBE_RESAVE;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    function n(v) { return Math.round(v * 1000) / 1000; }
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"'; }
    function opt(o, k) {
        try { var v = o[k]; return typeof v === "number" ? n(v) : (typeof v === "boolean" ? String(v) : q(v)); }
        catch (e) { return "null"; }
    }
    var doc = app.open(File(IDML), false);
    var fo = doc.footnoteOptions;
    var out = "{\"document\":{";
    var keys = ["enableStraddling", "spaceBetween", "spacer"];
    for (var k = 0; k < keys.length; k++) out += (k ? "," : "") + q(keys[k]) + ":" + opt(fo, keys[k]);
    out += "},\"frames\":[";
    var frames = doc.textFrames;
    var fkeys = ["footnotesEnableOverrides", "footnotesSpanAcrossColumns", "footnotesSpaceBetween", "footnotesMinimumSpacing"];
    for (var i = 0; i < frames.length; i++) {
        var tfp = frames[i].textFramePreferences;
        out += (i ? "," : "") + "{\"name\":" + q(frames[i].parentStory.paragraphs[0].contents.substr(0, 2));
        for (var j = 0; j < fkeys.length; j++) out += "," + q(fkeys[j]) + ":" + opt(tfp, fkeys[j]);
        out += "}";
    }
    out += "]}";
    var f = File(OUT);
    f.encoding = "UTF-8";
    f.open("w");
    f.write(out);
    f.close();
    doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(RESAVE));
    doc.close(SaveOptions.NO);
})();
