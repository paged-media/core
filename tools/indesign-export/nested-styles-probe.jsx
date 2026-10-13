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

// Ask InDesign how it spells and sets nested styles
// (nested-styles-probe.sh drives it). Builds a document in the DOM, with
// no IDML in between:
//
// - (a) one nested style per `NestedStyleDelimiters` value, so the export
//   shows each one's IDML spelling;
// - (b) a parent style with nested, line and GREP styles and children that
//   inherit, clear, replace or append to its lists (`EmptyNestedStyles`);
// - (f) one paragraph per rule case: a delimiter, a repetition, through or
//   up to, on a text chosen to show the edge;
// - End Nested Style Here, Repeat, a character style applied by hand
//   inside a nested range, nested against line and GREP styles, a drop
//   cap with and without a drop cap character style.
//
// It then reads every character's EFFECTIVE fill colour, underline and
// size back from the DOM (InDesign resolves nested styles into them),
// writes those as strings beside each case's text to
// nested-styles-probe.json, and exports the document as IDML and PDF. The
// answers are pinned in crates/paged-renderer/src/pipeline/tests.rs
// (`indesign_*`) and plugin-publish's idml-import `style_lists` tests.
(function () {
    var OUT = $.global.PROBE_OUT;
    function q(s) { s = String(s); return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/[\x00-\x1f\u2029\u2028\uFFFC\u00A0\u2002\u2003]/g, function (c) { return "\\u" + ("0000" + c.charCodeAt(0).toString(16)).slice(-4); }) + '"'; }
    var out = [], errs = [];
    function err(where, e) { errs.push(q(where + ": " + e)); }
    var doc = app.documents.add(false);
    try {
        doc.viewPreferences.horizontalMeasurementUnits = MeasurementUnits.POINTS;
        doc.viewPreferences.verticalMeasurementUnits = MeasurementUnits.POINTS;
        function col(n, v) { return doc.colors.add({ name: n, model: ColorModel.PROCESS, space: ColorSpace.CMYK, colorValue: v }); }
        var R = col("PR", [0, 100, 100, 0]), B = col("PB", [100, 60, 0, 0]), G = col("PG", [80, 0, 100, 0]), M = col("PM", [0, 100, 0, 0]);
        var csR = doc.characterStyles.add({ name: "R", fillColor: R });
        var csB = doc.characterStyles.add({ name: "B", fillColor: B });
        var csG = doc.characterStyles.add({ name: "G", fillColor: G });
        var csM = doc.characterStyles.add({ name: "M", fillColor: M });
        var csU = doc.characterStyles.add({ name: "U", underline: true });
        var csRbig = doc.characterStyles.add({ name: "Rbig", fillColor: R, pointSize: 20 });
        var code = { PR: "R", PB: "B", PG: "G", PM: "M", Black: "K" };

        // (a) every delimiter's spelling
        var dnames = ["ANY_CHARACTER", "ANY_WORD", "AUTO_PAGE_NUMBER", "DIGITS", "DROPCAP", "EM_SPACE",
            "END_NESTED_STYLE", "EN_SPACE", "FORCED_LINE_BREAK", "INDENT_HERE_TAB", "INLINE_GRAPHIC",
            "LETTERS", "NONBREAKING_SPACE", "REPEAT", "SECTION_MARKER", "SENTENCE", "TABS",
            "ANCHORED_OBJECT", "FOOTNOTE", "ANY_FOOTNOTE"];
        var all = doc.paragraphStyles.add({ name: "AllDelimiters" });
        var dn = [];
        for (var i = 0; i < dnames.length; i++) {
            var v;
            try { v = NestedStyleDelimiters[dnames[i]]; } catch (e) { v = undefined; }
            if (v === undefined) { dn.push(q(dnames[i]) + ':"undefined"'); continue; }
            try {
                all.nestedStyles.add({ appliedCharacterStyle: csR, delimiter: v, repetition: 1, inclusive: true });
                dn.push(q(dnames[i]) + ":" + q(String(v)));
            } catch (e) { err("delimiter " + dnames[i], e); }
        }
        out.push('"delimiters":{' + dn.join(",") + "}");

        // (b) cascade
        var parent = doc.paragraphStyles.add({ name: "Parent" });
        parent.nestedStyles.add({ appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 1, inclusive: true });
        parent.nestedLineStyles.add({ appliedCharacterStyle: csU, lineCount: 1 });
        parent.nestedGrepStyles.add({ appliedCharacterStyle: csG, grepExpression: "\\d+" });
        var inherit = doc.paragraphStyles.add({ name: "ChildInherits", basedOn: parent });
        var cleared = doc.paragraphStyles.add({ name: "ChildCleared", basedOn: parent });
        try { cleared.nestedStyles.everyItem().remove(); cleared.nestedLineStyles.everyItem().remove(); cleared.nestedGrepStyles.everyItem().remove(); } catch (e) { err("clear", e); }
        var replaced = doc.paragraphStyles.add({ name: "ChildReplaced", basedOn: parent });
        try { replaced.nestedStyles.everyItem().remove(); replaced.nestedStyles.add({ appliedCharacterStyle: csB, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 2, inclusive: true }); } catch (e) { err("replace", e); }
        var appended = doc.paragraphStyles.add({ name: "ChildAppended", basedOn: parent });
        try { appended.nestedStyles.add({ appliedCharacterStyle: csB, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 1, inclusive: true }); } catch (e) { err("append", e); }
        out.push('"cascade_counts":{"inherits":' + inherit.nestedStyles.length + ',"cleared":' + cleared.nestedStyles.length + ',"replaced":' + replaced.nestedStyles.length + ',"appended":' + appended.nestedStyles.length + "}");

        // (f) one style per rule case
        var cases = [
            ["W1i", ["ANY_WORD", 1, true], "Alpha, beta  gamma delta"],
            ["W1x", ["ANY_WORD", 1, false], "Alpha, beta  gamma delta"],
            ["W2i", ["ANY_WORD", 2, true], "Alpha  beta  gamma delta"],
            ["W2x", ["ANY_WORD", 2, false], "Alpha  beta  gamma delta"],
            ["W1lead", ["ANY_WORD", 1, true], "  Lead space word"],
            ["Whyph", ["ANY_WORD", 2, true], "Well-known x-ray tests"],
            ["C3i", ["ANY_CHARACTER", 3, true], "Alpha beta"],
            ["C3x", ["ANY_CHARACTER", 3, false], "Alpha beta"],
            ["L2i", ["LETTERS", 2, true], "12ab cd"],
            ["L2x", ["LETTERS", 2, false], "12ab cd"],
            ["D2i", ["DIGITS", 2, true], "ab12 3"],
            ["D2x", ["DIGITS", 2, false], "ab12 3"],
            ["S1i", ["SENTENCE", 1, true], "One two. Three four."],
            ["S1x", ["SENTENCE", 1, false], "One two. Three four."],
            ["S2i", ["SENTENCE", 2, true], "Wow! Next? Yes."],
            ["S1abbr", ["SENTENCE", 1, true], "Dr. Who said 3.5 is fine. Next."],
            ["Snone", ["SENTENCE", 1, true], "No end here"],
            ["T2i", ["TABS", 2, true], "a\tb\tc\td"],
            ["T2x", ["TABS", 2, false], "a\tb\tc\td"],
            ["CHi", [":", 1, true], "Key: value: more"],
            ["CHx", [":", 1, false], "Key: value: more"],
            ["CH2i", [":", 2, true], "Key: value: more"],
            ["MULTIi", [".,", 1, true], "One, two. three"],
            ["MULTI2i", [".,", 2, true], "One, two. three"],
            ["EMi", ["EM_SPACE", 1, true], "aa\u2003bb\u2003cc"],
            ["ENi", ["EN_SPACE", 1, true], "aa\u2002bb\u2002cc"],
            ["NBSPi", ["NONBREAKING_SPACE", 1, true], "aa\u00A0bb\u00A0cc"],
            ["FLBi", ["FORCED_LINE_BREAK", 1, true], "line one\nline two"],
            ["FLBx", ["FORCED_LINE_BREAK", 1, false], "line one\nline two"],
            ["S_pi_i", ["SENTENCE", 1, true], "Pi is 3.14 ok. Next."],
            ["S_pi_x", ["SENTENCE", 1, false], "Pi is 3.14 ok. Next."],
            ["S_nospace", ["SENTENCE", 1, true], "Ends with dot.Next word. Last"],
            ["S_quote_i", ["SENTENCE", 1, true], "He said \u201Cstop.\u201D Next one."],
            ["S_quote_x", ["SENTENCE", 1, false], "He said \u201Cstop.\u201D Next one."],
            ["S_ellipsis", ["SENTENCE", 1, true], "Wait... more. Done."],
            ["S_twospace_i", ["SENTENCE", 1, true], "One.  Two."],
            ["W_tab_i", ["ANY_WORD", 1, true], "One\ttwo three"],
            ["W_tab_x", ["ANY_WORD", 1, false], "One\ttwo three"],
            ["W_nbsp_i", ["ANY_WORD", 1, true], "One\u00A0two three"],
            ["W_thin_i", ["ANY_WORD", 1, true], "One\u2009two three"],
            ["W_em_i", ["ANY_WORD", 1, true], "One\u2003two three"],
            ["W_dash_i", ["ANY_WORD", 1, true], "one\u2014two three"],
            ["W_flb_i", ["ANY_WORD", 1, true], "One\ntwo three"],
            ["C1x", ["ANY_CHARACTER", 1, false], "Alpha beta"],
            ["C1i", ["ANY_CHARACTER", 1, true], "Alpha beta"],
            ["C_space_i", ["ANY_CHARACTER", 4, true], "a b c d e"],
            ["L_accent_i", ["LETTERS", 2, true], "12\u00E9b cd"],
            ["L_space_x", ["LETTERS", 3, false], "a b c d"],
            ["D_x1", ["DIGITS", 1, false], "ab12 3"],
            ["T_x1", ["TABS", 1, false], "\tlead tab"],
            ["CH_first_x", [":", 1, false], ":lead colon"],
            ["Rep0", ["ANY_WORD", 0, true], "zero rep words"]
        ];
        var page = doc.pages[0];
        // Tall enough that no case is overset: a line style needs its lines.
        var tf = page.textFrames.add({ geometricBounds: [36, 36, 6000, 560] });
        var texts = [];
        for (var c = 0; c < cases.length; c++) texts.push(cases[c][2]);
        // the end-nested-style, repeat, cascade and layering paragraphs
        texts.push("Styled~rest of it");                 // ENDi
        texts.push("Styled~rest of it");                 // ENDx
        texts.push("one two three four five six");       // REPEAT2
        texts.push("one two three four five six");       // REPEAT1
        texts.push("Inherit 12 words here");             // inherit
        texts.push("Cleared 12 words here");             // cleared
        texts.push("Replaced two 12 words here");        // replaced
        texts.push("Appended two 12 words here");        // appended
        texts.push("Manual underline only here");        // layering 1
        texts.push("Manual fill only here");             // layering 2
        texts.push("Line 12 wins or not");               // line vs nested vs grep
        texts.push("DropTwo characters here");           // drop cap without style
        texts.push("DropStyled characters here");        // drop cap with style
        texts.push("Line 12 wins or not");               // nest vs grep
        texts.push("Line 12 wins or not");               // line vs grep
        texts.push("No colon here at all");              // first not found
        texts.push("Two words then more words here");   // nested + line, line sets underline only
        texts.push("Local one two three four");          // the range's own list
        tf.contents = texts.join("\r");
        var pars = tf.paragraphs;
        var labels = [];
        for (var c = 0; c < cases.length; c++) {
            var cs = cases[c], spec = cs[1];
            var st = doc.paragraphStyles.add({ name: "Case " + cs[0] });
            var delim;
            try { delim = NestedStyleDelimiters[spec[0]]; } catch (e) { delim = undefined; }
            if (delim === undefined) delim = spec[0];
            try { st.nestedStyles.add({ appliedCharacterStyle: csR, delimiter: delim, repetition: spec[1], inclusive: spec[2] }); } catch (e) { err("case " + cs[0], e); }
            pars[c].appliedParagraphStyle = st;
            labels.push(cs[0]);
        }
        var k = cases.length;
        function nest(name, list) {
            var st = doc.paragraphStyles.add({ name: name });
            for (var i = 0; i < list.length; i++) {
                try { st.nestedStyles.add(list[i]); } catch (e) { err(name + " " + i, e); }
            }
            return st;
        }
        // END_NESTED_STYLE: replace ~ with the special character.
        function endMark(p) {
            try {
                var idx = String(p.contents).indexOf("~");
                p.characters[idx].contents = SpecialCharacters.END_NESTED_STYLE;
            } catch (e) { err("endmark", e); }
        }
        endMark(pars[k]); pars[k].appliedParagraphStyle = nest("Case ENDi", [{ appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.END_NESTED_STYLE, repetition: 1, inclusive: true }]); labels.push("ENDi"); k++;
        endMark(pars[k]); pars[k].appliedParagraphStyle = nest("Case ENDx", [{ appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.END_NESTED_STYLE, repetition: 1, inclusive: false }]); labels.push("ENDx"); k++;
        pars[k].appliedParagraphStyle = nest("Case REPEAT2", [
            { appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 1, inclusive: true },
            { appliedCharacterStyle: csB, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 1, inclusive: true },
            { appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.REPEAT, repetition: 2, inclusive: true }]); labels.push("REPEAT2"); k++;
        pars[k].appliedParagraphStyle = nest("Case REPEAT1", [
            { appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 1, inclusive: true },
            { appliedCharacterStyle: csB, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 1, inclusive: true },
            { appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.REPEAT, repetition: 1, inclusive: true }]); labels.push("REPEAT1"); k++;
        pars[k].appliedParagraphStyle = inherit; labels.push("inherit"); k++;
        pars[k].appliedParagraphStyle = cleared; labels.push("cleared"); k++;
        pars[k].appliedParagraphStyle = replaced; labels.push("replaced"); k++;
        pars[k].appliedParagraphStyle = appended; labels.push("appended"); k++;
        // layering: nested Rbig (fill R + 20pt) over the first 3 words;
        // a manual style with only underline / only fill on word 2.
        var lay = nest("Layering", [{ appliedCharacterStyle: csRbig, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 3, inclusive: true }]);
        pars[k].appliedParagraphStyle = lay; try { pars[k].words[1].appliedCharacterStyle = csU; } catch (e) { err("lay1", e); } labels.push("layer_manual_underline"); k++;
        pars[k].appliedParagraphStyle = lay; try { pars[k].words[1].appliedCharacterStyle = csB; } catch (e) { err("lay2", e); } labels.push("layer_manual_fill"); k++;
        // nested (R, 2 words) vs line style (B, line 1) vs GREP (G, digits)
        var lng = nest("LineNestGrep", [{ appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 2, inclusive: true }]);
        lng.nestedLineStyles.add({ appliedCharacterStyle: csB, lineCount: 1 });
        lng.nestedGrepStyles.add({ appliedCharacterStyle: csG, grepExpression: "\\d+|wins" });
        pars[k].appliedParagraphStyle = lng; labels.push("line_nest_grep"); k++;
        // drop cap 2 chars, no drop cap style; nested R 3 characters.
        var dc = nest("DropNoStyle", [{ appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.ANY_CHARACTER, repetition: 3, inclusive: true }]);
        dc.dropCapCharacters = 2; dc.dropCapLines = 2;
        pars[k].appliedParagraphStyle = dc; labels.push("drop_no_style"); k++;
        var dcs = nest("DropStyled", [{ appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.ANY_CHARACTER, repetition: 3, inclusive: true }]);
        dcs.dropCapCharacters = 2; dcs.dropCapLines = 2;
        try { dcs.dropCapStyle = csM; } catch (e) { err("dropCapStyle", e); }
        pars[k].appliedParagraphStyle = dcs; labels.push("drop_styled"); k++;
        var ng = nest("NestGrep", [{ appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 2, inclusive: true }]);
        ng.nestedGrepStyles.add({ appliedCharacterStyle: csG, grepExpression: "\\d+|wins" });
        pars[k].appliedParagraphStyle = ng; labels.push("nest_vs_grep"); k++;
        var lg = nest("LineGrep", []);
        lg.nestedLineStyles.add({ appliedCharacterStyle: csB, lineCount: 1 });
        lg.nestedGrepStyles.add({ appliedCharacterStyle: csG, grepExpression: "\\d+|wins" });
        pars[k].appliedParagraphStyle = lg; labels.push("line_vs_grep"); k++;
        pars[k].appliedParagraphStyle = nest("FirstNotFound", [
            { appliedCharacterStyle: csR, delimiter: ":", repetition: 1, inclusive: true },
            { appliedCharacterStyle: csB, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 1, inclusive: true }]); labels.push("first_not_found"); k++;
        var nl = nest("NestLineUnder", [{ appliedCharacterStyle: csR, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 2, inclusive: true }]);
        nl.nestedLineStyles.add({ appliedCharacterStyle: csU, lineCount: 1 });
        pars[k].appliedParagraphStyle = nl; labels.push("nest_line_underline"); k++;


        // A range's own list: the ParagraphStyleRange spelling.
        try { pars[k].nestedStyles.add({ appliedCharacterStyle: csB, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 3, inclusive: true }); }
        catch (e) { err("local nested", e); }
        labels.push("local_list"); k++;
        // The document's text defaults: the Preferences.xml spelling (for
        // new text only, so after every case is set).
        try { doc.textDefaults.nestedStyles.add({ appliedCharacterStyle: csB, delimiter: NestedStyleDelimiters.ANY_WORD, repetition: 1, inclusive: true }); }
        catch (e) { err("text defaults nested", e); }

        function dump(par) {
            var t = "", f = "", u = "", z = "";
            for (var c = 0; c < par.characters.length; c++) {
                var ch = par.characters[c];
                var s = String(ch.contents);
                if (s.length !== 1) s = "#";
                t += s;
                var n = "?";
                try { n = ch.fillColor.name; } catch (e) {}
                f += code[n] || "?";
                var ul = false;
                try { ul = ch.underline; } catch (e) {}
                u += ul ? "1" : "0";
                var ps = 0;
                try { ps = ch.pointSize; } catch (e) {}
                z += ps > 15 ? "L" : "s";
            }
            return "{" + '"text":' + q(t) + ',"fill":' + q(f) + ',"ul":' + q(u) + ',"size":' + q(z) + "}";
        }
        var res = [];
        for (var p = 0; p < labels.length; p++) res.push(q(labels[p]) + ":" + dump(pars[p]));
        out.push('"cases":{' + res.join(",\n") + "}");

        // (g) kerning method spellings the DOM accepts (it answers in the
        // UI language; the IDML spells $ID/Optical, $ID/None)
        var km = [];
        var kvals = ["$ID/Optical", "$ID/Metrics", "$ID/None"];
        for (var i = 0; i < kvals.length; i++) {
            var st = doc.paragraphStyles.add({ name: "Kern " + i });
            try { st.kerningMethod = kvals[i]; km.push(q(kvals[i]) + ":" + q(st.kerningMethod)); } catch (e) { err("kern " + kvals[i], e); }
        }
        out.push('"kerning":{' + km.join(",") + "}");

        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(OUT + "/nested-styles-probe.idml"));
        doc.exportFile(ExportFormat.PDF_TYPE, File(OUT + "/nested-styles-probe.pdf"));
    } catch (e) {
        err("top", e + " line " + e.line);
    } finally {
        try { doc.close(SaveOptions.NO); } catch (e) {}
    }
    out.push('"errors":[' + errs.join(",") + "]");
    var f = File(OUT + "/nested-styles-probe.json");
    f.encoding = "UTF-8";
    f.open("w");
    f.write("{" + out.join(",\n") + "}\n");
    f.close();
})();
