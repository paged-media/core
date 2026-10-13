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

// InDesign's view of a document, `paged.indesign-view/1`: what InDesign
// holds once it has opened a package, read back through its DOM. The
// InDesign round-trip lane (lane.jsx) records it for every scenario, and
// crates/paged-canvas/tests/indesign_lane.rs computes the same view from
// the engine's import and compares the fields a scenario declares.
//
//   document        page size, facing pages, bleed (top, bottom, inside,
//                   outside), page count
//   layers          name, visible, locked, in stacking order
//   swatches        names
//   paragraphStyles / characterStyles   name and the name it is based on
//   pages[].items   each top-level page item on the page: constructor,
//                   name, visible, locked, geometric bounds [top, left,
//                   bottom, right] in points from the page's top-left
//   stories         each story's paragraphs (style, text) and footnote
//                   texts; text without InDesign's special characters
//
// Usage: $.evalFile(this file); var json = pagedOracleView(doc);
var pagedOracleView = (function () {
    function str(s) {
        var t = String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"');
        return '"' + t.replace(/[\x00-\x1f\u2028\u2029]/g, function (c) {
            return "\\u" + ("000" + c.charCodeAt(0).toString(16)).slice(-4);
        }) + '"';
    }
    function json(v) {
        if (v === null || v === undefined) return "null";
        if (typeof v === "string") return str(v);
        if (typeof v === "number") return isFinite(v) ? String(Math.round(v * 1000) / 1000) : "null";
        if (typeof v === "boolean") return v ? "true" : "false";
        if (v instanceof Array) {
            var a = [];
            for (var i = 0; i < v.length; i++) a.push(json(v[i]));
            return "[" + a.join(",") + "]";
        }
        var o = [];
        for (var k in v) if (v.hasOwnProperty(k)) o.push(str(k) + ":" + json(v[k]));
        return "{" + o.join(",") + "}";
    }
    function get(o, k, dflt) {
        try { var v = o[k]; return v === undefined ? dflt : v; } catch (e) { return dflt; }
    }
    // A paragraph's text as the engine's view keeps it: no paragraph mark,
    // no special characters (footnote and anchor markers, table and
    // variable placeholders), whitespace collapsed.
    function plain(s) {
        return String(s).replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\uFEFF\uFFFC\uE000-\uF8FF]/g, "")
            .replace(/[\r\n\t\u2028\u2029\u00a0]+/g, " ").replace(/\s+/g, " ")
            .replace(/^\s+|\s+$/g, "");
    }
    // A style's name in InDesign's English spelling. The DOM gives the
    // built-in styles their names in the language of the install
    // ("[Kein Absatzformat]"), so they are named by what they are: the
    // first paragraph style is [No Paragraph Style], the second [Basic
    // Paragraph], the first character style [None]. A style based on
    // nothing reports a string for its basedOn.
    function styleName(s, builtins) {
        if (s === null || s === undefined) return null;
        if (typeof s === "string") return builtins.length ? builtins[0][1] : s;
        try {
            for (var b = 0; b < builtins.length; b++) if (s.id === builtins[b][0].id) return builtins[b][1];
            return String(s.name);
        } catch (e) { return null; }
    }
    function styles(collection, builtins) {
        var out = [];
        for (var i = 0; i < collection.length; i++) {
            var s = collection[i];
            out.push({ name: styleName(s, builtins), basedOn: styleName(get(s, "basedOn", null), builtins) });
        }
        return out;
    }
    function item(it) {
        // A page's pageItems are generic PageItems; their elements are the
        // real Rectangle, TextFrame, Group, ...
        it = it.getElements()[0];
        var g = it.geometricBounds;
        return {
            kind: String(it.constructor.name),
            name: String(get(it, "name", "")),
            visible: get(it, "visible", true) === true,
            locked: get(it, "locked", false) === true,
            bounds: [g[0], g[1], g[2], g[3]]
        };
    }
    function story(st, pBuiltins) {
        var paras = [];
        for (var p = 0; p < st.paragraphs.length; p++) {
            var para = st.paragraphs[p];
            paras.push({ style: styleName(para.appliedParagraphStyle, pBuiltins), text: plain(para.contents) });
        }
        var notes = [];
        for (var f = 0; f < st.footnotes.length; f++) notes.push(plain(st.footnotes[f].contents));
        return { text: plain(st.contents), paragraphs: paras, footnotes: notes, tables: st.tables.length };
    }
    return function (doc) {
        var vp = doc.viewPreferences;
        vp.horizontalMeasurementUnits = MeasurementUnits.POINTS;
        vp.verticalMeasurementUnits = MeasurementUnits.POINTS;
        vp.rulerOrigin = RulerOrigin.PAGE_ORIGIN;
        var dp = doc.documentPreferences;
        var pBuiltins = [[doc.paragraphStyles[0], "[No Paragraph Style]"], [doc.paragraphStyles[1], "[Basic Paragraph]"]];
        var cBuiltins = [[doc.characterStyles[0], "[None]"]];
        var first = doc.pages.length ? doc.pages[0].bounds : [0, 0, 0, 0];
        var view = {
            schema: "paged.indesign-view/1",
            document: {
                // The first page's size; the document's own page size (what
                // a new page gets) separately.
                pageWidth: first[3] - first[1],
                pageHeight: first[2] - first[0],
                preferencePageSize: [dp.pageWidth, dp.pageHeight],
                facingPages: dp.facingPages,
                bleed: [dp.documentBleedTopOffset, dp.documentBleedBottomOffset,
                    dp.documentBleedInsideOrLeftOffset, dp.documentBleedOutsideOrRightOffset],
                pages: doc.pages.length
            },
            layers: [],
            swatches: [],
            paragraphStyles: styles(doc.allParagraphStyles, pBuiltins),
            characterStyles: styles(doc.allCharacterStyles, cBuiltins),
            pages: [],
            stories: []
        };
        for (var l = 0; l < doc.layers.length; l++) {
            var layer = doc.layers[l];
            view.layers.push({ name: String(layer.name), visible: layer.visible, locked: layer.locked });
        }
        for (var s = 0; s < doc.swatches.length; s++) view.swatches.push(String(doc.swatches[s].name));
        for (var p = 0; p < doc.pages.length; p++) {
            var items = [];
            var pis = doc.pages[p].pageItems;
            for (var i = 0; i < pis.length; i++) items.push(item(pis[i]));
            view.pages.push({ items: items });
        }
        for (var t = 0; t < doc.stories.length; t++) view.stories.push(story(doc.stories[t], pBuiltins));
        return json(view);
    };
})();
