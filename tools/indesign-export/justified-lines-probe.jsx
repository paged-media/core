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


// Ask InDesign where its Paragraph Composer breaks JUSTIFIED text
// (justified-lines-probe.sh drives it): ten paragraphs, each set
// LeftJustified in Open Sans 10/12 (English: USA) in frames 120 to 260 pt
// wide, with hyphenation on and off; every line's contents are recorded.
// The paragraphs are paged-gen's `samples/justified_probe.rs` TEXTS; keep
// the two in step.
(function () {
    var OUT = $.global.PROBE_OUT;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var EN = null;
    var langs = app.languagesWithVendors;
    for (var li = 0; li < langs.length; li++) {
        if (langs[li].icuLocaleName == "en_US") { EN = langs[li]; break; }
    }
    if (EN == null) throw new Error("no en_US language");
    var doc = app.documents.add(false);
    doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
    doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
    var page = doc.pages[0];
    var TEXTS = ["Printing from movable type changed how knowledge travelled, because a page set once could be pressed hundreds of times before the forme was broken up and the letters returned to their cases. Compositors learned to judge a line by eye, adding thin spaces between words or squeezing them tighter until every line filled its measure.", "A justified column asks more of its words than a ragged one. Each line must reach both margins, so the spaces stretch and shrink, and long words near the end of a line may be divided. Good composition balances these adjustments across the whole paragraph instead of fixing each line in turn, and the reader never notices.", "Considerable experimentation showed that international typographic conventions, documentation requirements and characteristically unpredictable manuscripts demand extraordinarily careful hyphenation, particularly within narrow newspaper columns where consecutive hyphenated lines quickly become distracting.", "Short words fit anywhere: a cat, a dog, an owl and a fox sat in a row by the old red barn, and one by one they got up, ran off and hid in the tall wet grass at the far end of the yard until the sun went down and the moon came up.", "The harbour master kept a ledger of every vessel that entered the bay, noting its tonnage, its cargo and the name of its captain in a careful copperplate hand that grew smaller each winter as the light in his office failed earlier in the afternoon.", "When the committee finally published its recommendations, the newspapers reported them in very different ways: some praised the thoroughness of the investigation, others complained that the conclusions were vague, and a few simply printed the summary without any comment at all.", "Bread rises because yeast feeds on the sugars in the flour and releases carbon dioxide, which the stretched gluten traps in countless small pockets; bake it too early and the crumb is dense, too late and the loaf collapses as it cools on the rack.", "Mountain railways climb steep gradients with the help of a toothed rack laid between the rails, so that a cogwheel beneath the locomotive can grip it firmly; passengers often notice the gentle clicking long before they see the summit station appear above the clouds.", "In the archive, letters from three centuries lie side by side: invoices, love notes, legal summonses and recipes, each folded small and tied with ribbon, waiting for a reader patient enough to untangle the handwriting and the relationships it records.", "Typographers distinguish between the measure, the length of a line of text, and the column, the block of lines it forms; a comfortable measure holds between forty-five and seventy-five characters, and anything much longer tires the eye as it returns to the left margin."];
    var WIDTHS = [120, 140, 160, 180, 200, 220, 240, 260];
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"'; }
    var rows = [];
    for (var h = 0; h < 2; h++) {
        for (var t = 0; t < TEXTS.length; t++) {
            for (var w = 0; w < WIDTHS.length; w++) {
                var tf = page.textFrames.add({ geometricBounds: [20, 20, 780, 20 + WIDTHS[w]] });
                tf.textFramePreferences.insetSpacing = [0, 0, 0, 0];
                tf.contents = TEXTS[t];
                var p = tf.paragraphs[0];
                p.appliedFont = "Open Sans\tRegular";
                p.pointSize = 10;
                p.leading = 12;
                p.hyphenation = (h == 0);
                p.justification = Justification.LEFT_JUSTIFIED;
                p.composer = "$ID/HL Composer";
                p.appliedLanguage = EN;
                var lines = [];
                for (var i = 0; i < tf.lines.length; i++) lines.push(q(tf.lines[i].contents));
                rows.push('{"text": ' + t + ', "width": ' + WIDTHS[w] + ', "hyphenate": ' + (h == 0) + ', "overflows": ' + tf.overflows + ', "lines": [' + lines.join(", ") + ']}');
                tf.remove();
            }
        }
    }
    var json = '{"indesign": "' + app.version + '", "language": "' + EN.name + '", "rows": [\n' + rows.join(",\n") + '\n]}\n';
    var f = File(OUT); f.encoding = "UTF-8"; f.lineFeed = "Unix"; f.open("w"); f.write(json); f.close();
    doc.close(SaveOptions.NO);
})();
