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

// Ask InDesign what it lets a SCRIPT do to a locked page item, what
// deleting a container does to what was pasted into it, and what
// deleting every member of a group leaves behind
// (object-integrity-probe.sh drives it). The mutation engine's rules for
// the same three cases are pinned in crates/paged-mutate/tests/
// element_lock.rs, remove_node_nested.rs and remove_node_groups.rs.
//
// Works on a fresh, unsaved, windowless document and closes it without
// saving. Every case is one line, `<case>: OK <readback>` or
// `<case>: ERROR <number> <message>`. The message is in the language of
// the InDesign install; the number is the stable part (11274 is "the
// object is locked").
(function () {
    var OUT = $.global.PROBE_OUT;
    var lines = [];
    function q(s) { s = String(s); return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/[\r\n]/g, " ") + '"'; }
    function attempt(label, fn) {
        try {
            var r = fn();
            lines.push(q(label) + ":" + q("OK" + (r === undefined ? "" : " " + r)));
        } catch (e) {
            lines.push(q(label) + ":" + q("ERROR " + e.number + " " + e.message));
        }
    }
    function b(it) { var g = it.geometricBounds; return "[" + g.join(",") + "]"; }

    var step = "start";
    var prior = app.scriptPreferences.userInteractionLevel;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = app.documents.add(false);
    try {
        doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
        var page = doc.pages[0];
        var black = doc.swatches.itemByName("Black");
        function rect(bounds) {
            return page.rectangles.add({ geometricBounds: bounds, fillColor: black });
        }

        step = '1 —';
        // 1 — a locked rectangle: what moves, what changes, what goes.
        var r1 = rect([10, 10, 50, 50]);
        r1.locked = true;
        attempt("1a locked: move by", function () { r1.move(undefined, [5, 5]); return b(r1); });
        attempt("1b locked: set geometricBounds", function () { r1.geometricBounds = [20, 20, 60, 60]; return b(r1); });
        attempt("1c locked: set rotationAngle", function () { r1.rotationAngle = 30; return r1.rotationAngle; });
        attempt("1d locked: transform (scale)", function () {
            r1.transform(CoordinateSpaces.PASTEBOARD_COORDINATES, AnchorPoint.CENTER_ANCHOR,
                app.transformationMatrices.add({ horizontalScaleFactor: 2 }));
            return b(r1);
        });
        attempt("1e locked: move a path point", function () {
            var p = r1.paths[0].pathPoints[0];
            p.anchor = [0, 0];
            return "[" + p.anchor.join(",") + "]";
        });
        attempt("1f locked: set fillColor", function () { r1.fillColor = doc.swatches.itemByName("Paper"); return r1.fillColor.name; });
        attempt("1g locked: set strokeWeight", function () { r1.strokeWeight = 4; return r1.strokeWeight; });
        attempt("1h locked: set opacity", function () {
            r1.transparencySettings.blendingSettings.opacity = 40;
            return r1.transparencySettings.blendingSettings.opacity;
        });
        attempt("1i locked: set corner radius", function () {
            r1.topLeftCornerOption = CornerOptions.ROUNDED_CORNER;
            r1.topLeftCornerRadius = 6;
            return r1.topLeftCornerRadius;
        });
        attempt("1j locked: set nonprinting", function () { r1.nonprinting = true; return r1.nonprinting; });
        attempt("1k locked: bringToFront", function () { r1.bringToFront(); return r1.index; });
        attempt("1l locked: duplicate", function () { var d = r1.duplicate(); var s = "copy locked=" + d.locked; d.locked = false; d.remove(); return s; });
        attempt("1m locked: remove", function () { r1.remove(); return "removed, valid=" + r1.isValid; });
        attempt("1n unlock", function () { r1.locked = false; return "locked=" + r1.locked; });
        attempt("1o unlocked: move by", function () { r1.move(undefined, [5, 5]); return b(r1); });
        attempt("1p unlocked: remove", function () { r1.remove(); return "valid=" + r1.isValid; });

        step = '2 —';
        // 2 — a locked line and a locked text frame.
        var line = page.graphicLines.add({ geometricBounds: [100, 10, 100, 80] });
        line.locked = true;
        attempt("2a locked line: move by", function () { line.move(undefined, [5, 5]); return b(line); });
        attempt("2b locked line: remove", function () { line.remove(); return "valid=" + line.isValid; });
        var tf = page.textFrames.add({ geometricBounds: [120, 10, 160, 100], contents: "abc" });
        tf.locked = true;
        attempt("2c locked text frame: set contents", function () { tf.contents = "xyz"; return tf.contents; });
        attempt("2d locked text frame: insert text", function () { tf.insertionPoints[-1].contents = "!"; return tf.contents; });
        attempt("2e locked text frame: move by", function () { tf.move(undefined, [5, 5]); return b(tf); });
        attempt("2f locked text frame: set textFramePreferences.insetSpacing", function () {
            tf.textFramePreferences.insetSpacing = [2, 2, 2, 2];
            return tf.textFramePreferences.insetSpacing.join(",");
        });

        step = '3 —';
        // 3 — a group with one locked member.
        var ga = rect([200, 10, 220, 30]);
        var gb = rect([200, 40, 220, 60]);
        gb.locked = true;
        var grp = null;
        attempt("3a group a locked member", function () { grp = page.groups.add([ga, gb]); return "members=" + grp.pageItems.length; });
        if (grp) {
            attempt("3b move the group", function () { grp.move(undefined, [5, 5]); return "group " + b(grp) + " locked member " + b(gb); });
            attempt("3c remove the group", function () { grp.remove(); return "valid=" + grp.isValid + " member valid=" + gb.isValid; });
        }
        // A member locked on its own, inside an unlocked group.
        var gm1 = rect([280, 10, 290, 20]);
        var gm2 = rect([280, 40, 290, 50]);
        var grp3 = page.groups.add([gm1, gm2]);
        attempt("3g lock a member inside a group", function () { gm2.locked = true; return "locked=" + gm2.locked; });
        attempt("3h move the group holding a locked member", function () { grp3.move(undefined, [5, 5]); return "group " + b(grp3) + " member " + b(gm2); });
        attempt("3i ungroup the group holding a locked member", function () { grp3.ungroup(); return "member valid=" + gm2.isValid + " locked=" + gm2.locked; });
        step = 'A locked GROUP';
        // A locked GROUP: its members.
        var gc = rect([240, 10, 260, 30]);
        var gd = rect([240, 40, 260, 60]);
        var grp2 = page.groups.add([gc, gd]);
        grp2.locked = true;
        attempt("3d locked group: member locked?", function () { return "member locked=" + gc.locked; });
        attempt("3e locked group: move a member", function () { gc.move(undefined, [5, 5]); return b(gc); });
        attempt("3f locked group: remove a member", function () { gc.remove(); return "valid=" + gc.isValid; });

        step = '4 —';
        // 4 — a container with content pasted into it.
        var host = rect([300, 10, 360, 70]);
        var inner = host.rectangles.add({ geometricBounds: [310, 20, 350, 60], fillColor: black });
        attempt("4a container holds", function () { return "children=" + host.pageItems.length + " inner parent=" + inner.parent.constructor.name; });
        attempt("4b remove the container", function () { host.remove(); return "container valid=" + host.isValid + " inner valid=" + inner.isValid + " page items=" + page.allPageItems.length; });
        // Pasted-in content cannot be locked on its own; a locked
        // container keeps its content.
        var host2 = rect([380, 10, 440, 70]);
        var inner2 = host2.rectangles.add({ geometricBounds: [390, 20, 430, 60], fillColor: black });
        attempt("4c lock the pasted-in content", function () { inner2.locked = true; return "locked=" + inner2.locked; });
        host2.locked = true;
        attempt("4d locked container: content locked?", function () { return "content locked=" + inner2.locked; });
        attempt("4e locked container: move its content", function () { inner2.move(undefined, [5, 5]); return b(inner2); });
        attempt("4f locked container: remove", function () { host2.remove(); return "valid=" + host2.isValid; });

        step = '5 —';
        // 5 — deleting every member of a group.
        var m1 = rect([460, 10, 480, 30]);
        var m2 = rect([460, 40, 480, 60]);
        var m3 = rect([460, 70, 480, 90]);
        var g5 = page.groups.add([m1, m2, m3]);
        attempt("5a remove member 1 of 3", function () { m1.remove(); return "group valid=" + g5.isValid + " members=" + (g5.isValid ? g5.pageItems.length : "-"); });
        attempt("5b remove member 2 of 3", function () { m2.remove(); return "group valid=" + g5.isValid + " members=" + (g5.isValid ? g5.pageItems.length : "-") + " m3 parent=" + m3.parent.constructor.name; });
        attempt("5c remove the last member", function () { m3.remove(); return "group valid=" + g5.isValid; });
    } catch (e) {
        lines.push(q("setup failed after " + step) + ":" + q("ERROR " + e.number + " " + e.message + " line " + e.line));
    } finally {
        try { doc.close(SaveOptions.NO); } catch (e) {}
        app.scriptPreferences.userInteractionLevel = prior;
    }

    var f = File(OUT);
    f.encoding = "UTF-8";
    f.lineFeed = "Unix";
    f.open("w");
    f.write("{\n  \"indesign\": " + q(app.version) + ",\n  " + lines.join(",\n  ") + "\n}\n");
    f.close();
})();
