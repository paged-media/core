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

// Ask InDesign which frame a handle scale works in (scale-frame-probe.sh
// drives it). The engine's Scale gesture follows the answers; its tests
// are crates/paged-canvas/tests/rotated_scale_gesture.rs.
//
// 1. The orientation of a SELECTION's transformation bounding box. The
//    box is not readable through the DOM, but its reference point is:
//    the "Rotate 90° Clockwise" menu action turns the selection about the
//    reference point the Control panel names ON THAT BOX. With the point
//    set to the top-left corner, the pivot the action turned about (solved
//    from where two points went) is the box's top-left corner — the
//    corner of the item's own rotated box if the box follows the
//    rotation, the corner of the axis-aligned box if it does not.
// 2. The orientation of a GROUP made of the same items.
// 3. What a non-uniform scale along a group's own axes does to its
//    members: whether they stay rectangles (no shear) at their angle.
//
// Works on a fresh, unsaved document (with a window: the menu action
// needs one) and closes it without saving. Every case is one line,
// `<case>: <readback>`.
(function () {
    var OUT = $.global.PROBE_OUT;
    var lines = [];
    function q(s) { s = String(s); return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/[\r\n]/g, " ") + '"'; }
    function r2(v) { return Math.round(v * 100) / 100; }
    function pt(p) { return "(" + r2(p[0]) + "," + r2(p[1]) + ")"; }
    function put(label, value) { lines.push(q(label) + ":" + q(value)); }

    var PB = CoordinateSpaces.PASTEBOARD_COORDINATES;
    var INNER = CoordinateSpaces.INNER_COORDINATES;
    var GEO = BoundingBoxLimits.GEOMETRIC_PATH_BOUNDS;
    function corner(it, anchor, space) {
        return it.resolve([anchor, GEO, space || INNER], PB)[0];
    }
    // An item's own box in pasteboard space: its top-left corner, its
    // edge lengths along its own axes, the angle its top edge makes and
    // the angle between its top and left edges (90 = a rectangle).
    function box(it) {
        var tl = corner(it, AnchorPoint.TOP_LEFT_ANCHOR);
        var tr = corner(it, AnchorPoint.TOP_RIGHT_ANCHOR);
        var bl = corner(it, AnchorPoint.BOTTOM_LEFT_ANCHOR);
        var ux = tr[0] - tl[0], uy = tr[1] - tl[1];
        var vx = bl[0] - tl[0], vy = bl[1] - tl[1];
        var w = Math.sqrt(ux * ux + uy * uy), h = Math.sqrt(vx * vx + vy * vy);
        var top = Math.atan2(uy, ux) * 180 / Math.PI;
        var cornerAngle = Math.acos((ux * vx + uy * vy) / (w * h)) * 180 / Math.PI;
        return "tl=" + pt(tl) + " w=" + r2(w) + " h=" + r2(h) + " topEdge=" + r2(top) +
            "deg corner=" + r2(cornerAngle) + "deg rotation=" + r2(it.absoluteRotationAngle) +
            " shear=" + r2(it.absoluteShearAngle);
    }
    function centre(it) { return it.resolve([AnchorPoint.CENTER_ANCHOR, GEO, INNER], PB)[0]; }
    // The fixed point of the rigid turn that took a→a2 and b→b2.
    function pivotOf(a, a2, b, b2) {
        var t = Math.atan2(b2[1] - a2[1], b2[0] - a2[0]) - Math.atan2(b[1] - a[1], b[0] - a[0]);
        var c = Math.cos(t), s = Math.sin(t);
        // a2 = P + R(a − P)  ⇒  (I − R)P = a2 − R·a
        var rx = a2[0] - (c * a[0] - s * a[1]);
        var ry = a2[1] - (s * a[0] + c * a[1]);
        var m00 = 1 - c, m01 = s, m10 = -s, m11 = 1 - c;
        var det = m00 * m11 - m01 * m10;
        return { p: [(m11 * rx - m01 * ry) / det, (-m10 * rx + m00 * ry) / det], deg: t * 180 / Math.PI };
    }
    // The axis-aligned box of the items' corners, and the box of the same
    // corners in a frame turned by `deg` (top-left of each, pasteboard).
    function unionTopLeft(items, deg) {
        var t = deg * Math.PI / 180, c = Math.cos(t), s = Math.sin(t);
        var minU = 1e9, minV = 1e9;
        for (var i = 0; i < items.length; i++) {
            var as = [AnchorPoint.TOP_LEFT_ANCHOR, AnchorPoint.TOP_RIGHT_ANCHOR,
                AnchorPoint.BOTTOM_LEFT_ANCHOR, AnchorPoint.BOTTOM_RIGHT_ANCHOR];
            for (var k = 0; k < 4; k++) {
                var p = corner(items[i], as[k]);
                var u = c * p[0] + s * p[1], v = -s * p[0] + c * p[1];
                if (u < minU) minU = u;
                if (v < minV) minV = v;
            }
        }
        return [c * minU - s * minV, s * minU + c * minV];
    }

    var prior = app.scriptPreferences.userInteractionLevel;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = app.documents.add(true);
    var step = "start";
    try {
        doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
        var page = doc.pages[0];
        var black = doc.swatches.itemByName("Black");
        function rect(bounds, deg) {
            var r = page.rectangles.add({ geometricBounds: bounds, fillColor: black, strokeWeight: 0 });
            if (deg) {
                r.transform(PB, AnchorPoint.CENTER_ANCHOR,
                    app.transformationMatrices.add({ counterclockwiseRotationAngle: deg }));
            }
            return r;
        }
        function clear() { while (page.pageItems.length) page.pageItems[0].remove(); }

        step = "menu action";
        // The action's name is in the install's language; its ID is not.
        var rot90 = null, names = [];
        var all = app.menuActions.everyItem().getElements();
        for (var i = 0; i < all.length; i++) {
            var n = all[i].name;
            if (/90/.test(n)) names.push(all[i].id + "=" + n);
        }
        put("0 menu actions naming 90", names.join(" | "));
        // "Rotate 90° Clockwise" is the first rotate-90 action with a
        // clockwise turn; resolved by its effect below rather than its name.
        function findRot90() {
            for (var i = 0; i < all.length; i++) {
                var n = all[i].name;
                if (/90/.test(n) && /(Clockwise|Uhrzeigersinn)/i.test(n) && !/(Counter|gegen)/i.test(n)) {
                    return all[i];
                }
            }
            return null;
        }
        rot90 = findRot90();
        put("0 rotate action", rot90 ? rot90.id + "=" + rot90.name : "NOT FOUND");

        function turnSelection(items, ref) {
            app.select(items);
            doc.layoutWindows[0].transformReferencePoint = ref;
            var a = corner(items[0], AnchorPoint.TOP_LEFT_ANCHOR);
            var b = corner(items[items.length - 1], AnchorPoint.BOTTOM_RIGHT_ANCHOR);
            rot90.invoke();
            var a2 = corner(items[0], AnchorPoint.TOP_LEFT_ANCHOR);
            var b2 = corner(items[items.length - 1], AnchorPoint.BOTTOM_RIGHT_ANCHOR);
            return pivotOf(a, a2, b, b2);
        }
        function selectionCase(label, specs) {
            clear();
            var items = [];
            for (var i = 0; i < specs.length; i++) items.push(rect(specs[i][0], specs[i][1]));
            var common = specs[0][1] || 0;
            var aabbTL = unionTopLeft(items, 0);
            var ownTL = unionTopLeft(items, -common);
            var turn = turnSelection(items, AnchorPoint.TOP_LEFT_ANCHOR);
            put(label, "turned " + r2(turn.deg) + "deg about " + pt(turn.p) +
                " | axis-aligned box top-left " + pt(aabbTL) +
                " | box turned with the items (" + common + "deg) top-left " + pt(ownTL));
        }

        if (rot90) {
            step = "1 selection";
            selectionCase("1a one rect at 0deg (sanity)", [[[100, 100, 160, 260], 0]]);
            selectionCase("1b one rect at 30deg", [[[100, 100, 160, 260], 30]]);
            selectionCase("1c two rects, both 30deg", [[[100, 100, 160, 260], 30], [[300, 150, 340, 230], 30]]);
            selectionCase("1d two rects, 30deg and 60deg", [[[100, 100, 160, 260], 30], [[300, 150, 340, 230], 60]]);
            selectionCase("1e two rects, 30deg and 210deg", [[[100, 100, 160, 260], 30], [[300, 150, 340, 230], 210]]);
        }

        step = "2 groups";
        function groupCase(label, specs) {
            clear();
            var items = [];
            for (var i = 0; i < specs.length; i++) items.push(rect(specs[i][0], specs[i][1]));
            var g = page.groups.add(items);
            put(label, "group " + box(g));
            return g;
        }
        var g1 = groupCase("2a group of two rects, both 30deg", [[[100, 100, 160, 260], 30], [[300, 150, 340, 230], 30]]);
        step = "3 scale same";
        // Scale the group 150% along its own x axis about its left-centre
        // handle — what a right-side handle drag does to a group.
        var m1 = g1.pageItems.everyItem().getElements();
        put("3a before: member 1", box(m1[0]));
        put("3a before: member 2", box(m1[1]));
        g1.transform(INNER, AnchorPoint.LEFT_CENTER_ANCHOR,
            app.transformationMatrices.add({ horizontalScaleFactor: 1.5 }));
        put("3a group 150% x in its own frame", "group " + box(g1));
        put("3a after: member 1", box(m1[0]));
        put("3a after: member 2", box(m1[1]));

        var g2 = groupCase("2b group of two rects, 30deg and 60deg", [[[100, 100, 160, 260], 30], [[300, 150, 340, 230], 60]]);
        step = "3 scale mixed";
        var m2 = g2.pageItems.everyItem().getElements();
        put("3b before: member 1", box(m2[0]));
        put("3b before: member 2", box(m2[1]));
        g2.transform(INNER, AnchorPoint.LEFT_CENTER_ANCHOR,
            app.transformationMatrices.add({ horizontalScaleFactor: 1.5 }));
        put("3b group 150% x in its own frame", "group " + box(g2));
        put("3b after: member 1", box(m2[0]));
        put("3b after: member 2", box(m2[1]));

        groupCase("2c group of two rects, 30deg and 210deg", [[[100, 100, 160, 260], 30], [[300, 150, 340, 230], 210]]);
        groupCase("2d group of two rects, 30deg and 120deg", [[[100, 100, 160, 260], 30], [[300, 150, 340, 230], 120]]);

        // A group that is itself rotated: two unrotated rects grouped, the
        // group turned 30deg. Its selection box (by the same pivot test as
        // 1), and what scaling it along its own x axis does to its members.
        step = "4 rotated group";
        function rotatedGroup() {
            clear();
            var g = page.groups.add([rect([100, 100, 160, 260], 0), rect([200, 100, 240, 180], 0)]);
            g.transform(PB, AnchorPoint.CENTER_ANCHOR,
                app.transformationMatrices.add({ counterclockwiseRotationAngle: 30 }));
            return g;
        }
        var g3 = rotatedGroup();
        put("4a group turned 30deg", "group " + box(g3));
        if (rot90) {
            var ownTL3 = corner(g3, AnchorPoint.TOP_LEFT_ANCHOR);
            var aabbTL3 = unionTopLeft([g3], 0);
            var turn3 = turnSelection([g3], AnchorPoint.TOP_LEFT_ANCHOR);
            put("4b selection box of the turned group", "turned " + r2(turn3.deg) + "deg about " + pt(turn3.p) +
                " | axis-aligned box top-left " + pt(aabbTL3) +
                " | the group's own box top-left " + pt(ownTL3));
            g3 = rotatedGroup();
        }
        var m3 = g3.pageItems.everyItem().getElements();
        put("4c before: member 1", box(m3[0]));
        put("4c before: member 2", box(m3[1]));
        g3.transform(INNER, AnchorPoint.LEFT_CENTER_ANCHOR,
            app.transformationMatrices.add({ horizontalScaleFactor: 1.5 }));
        put("4c group 150% x in its own frame", "group " + box(g3));
        put("4c after: member 1", box(m3[0]));
        put("4c after: member 2", box(m3[1]));
    } catch (e) {
        put("setup failed after " + step, "ERROR " + e.number + " " + e.message + " line " + e.line);
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
