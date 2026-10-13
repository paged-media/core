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


// Ask InDesign what applying a condition set does (condition-sets-probe.sh
// drives it). In a new document: A shown and B hidden, set S made, then C
// added (so S does not list it); every condition is then flipped (A
// hidden, B and C shown) and S applied. Reports each step's visibilities,
// S's recorded pairs, and S as InDesign spells it in its own IDML.
//
// Inputs: PROBE_OUT (the JSON written), PROBE_RESAVE (InDesign's IDML).
(function () {
    var OUT = $.global.PROBE_OUT, RESAVE = $.global.PROBE_RESAVE;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = app.documents.add(false);
    var steps = [];
    function state(tag) {
        var parts = [];
        for (var i = 0; i < doc.conditions.length; i++) {
            parts.push('"' + doc.conditions[i].name + '": ' + doc.conditions[i].visible);
        }
        steps.push('    {"step": "' + tag + '", "visible": {' + parts.join(", ") + "}}");
    }
    function pairs(set) {
        var out = [], sc = set.setConditions;
        for (var i = 0; i < sc.length; i++) {
            out.push('{"condition": "' + sc[i][0].name + '", "visible": ' + sc[i][1] + "}");
        }
        return "[" + out.join(", ") + "]";
    }
    var a = doc.conditions.add({ name: "A", visible: true });
    var b = doc.conditions.add({ name: "B", visible: false });
    var s = doc.conditionSets.add({ name: "S" });
    state("set S made");
    var c = doc.conditions.add({ name: "C", visible: true });
    state("C added after S");
    a.visible = false;
    b.visible = true;
    c.visible = true;
    state("flipped");
    doc.conditionalTextPreferences.activeConditionSet = s;
    state("S applied");
    var pairs_cache = pairs(s);
    doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(RESAVE));
    doc.close(SaveOptions.NO);
    var json = "{\n  \"indesign\": \"" + app.version + "\",\n  \"pairs\": " + pairs_cache + ",\n  \"steps\": [\n" + steps.join(",\n") + "\n  ]\n}\n";
    var f = File(OUT);
    f.encoding = "UTF-8";
    f.lineFeed = "Unix";
    f.open("w");
    f.write(json);
    f.close();
})();
