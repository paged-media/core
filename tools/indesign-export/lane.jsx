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

// The InDesign round-trip lane (lane.sh drives it). For each scenario
// directory under LANE_DIR it opens `out.idml` (the engine's export),
// records InDesign's view of it (lib/oracle.jsx), saves it as an .indd,
// reopens that and records the view again, and exports the reopened
// document as `resaved.idml`. Writes `<scenario>/indesign-view.json`:
//
//   { "indesign": version, "opened": view, "reopened": view }
//
// Inputs: LANE_DIR, LANE_SCENARIOS (comma-separated), LANE_ORACLE (the
// path of lib/oracle.jsx).
(function () {
    $.evalFile(File($.global.LANE_ORACLE));
    var prior = app.scriptPreferences.userInteractionLevel;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    try {
        var names = String($.global.LANE_SCENARIOS).split(",");
        for (var i = 0; i < names.length; i++) {
            var dir = $.global.LANE_DIR + "/" + names[i];
            var doc = app.open(File(dir + "/out.idml"), false);
            var opened = pagedOracleView(doc);
            var indd = File(dir + "/out.indd");
            doc.save(indd);
            doc.close(SaveOptions.NO);
            var again = app.open(indd, false);
            var reopened = pagedOracleView(again);
            again.exportFile(ExportFormat.INDESIGN_MARKUP, File(dir + "/resaved.idml"));
            again.close(SaveOptions.NO);
            indd.remove();
            var f = File(dir + "/indesign-view.json");
            f.encoding = "UTF-8";
            f.open("w");
            f.write('{"indesign":"' + app.version + '","opened":' + opened + ',"reopened":' + reopened + "}\n");
            f.close();
        }
    } finally {
        app.scriptPreferences.userInteractionLevel = prior;
    }
})();
