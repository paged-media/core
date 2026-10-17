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

// Ask InDesign how editorial notes stand in a story (notes-probe.sh
// drives it). Builds a one-frame story, reports its length, then adds a
// note inside a word, a note of two paragraphs between words and one at
// the end of the story, and reports
// what each takes of the story: its length and contents (specials as
// code points), each note's offset, attributes and text. Exports
// InDesign's IDML and PDF of that document; then, on the same story,
// reports what Convert to Text leaves of each note in turn, and what
// deleting a note leaves.
//
// With PROBE_IN (an absolute .idml), opens that document instead and
// reports every story's notes as InDesign reads them.
//
// Inputs: PROBE_IN (optional IDML to open), PROBE_OUT (JSON), PROBE_IDML,
// PROBE_PDF.
(function () {
    var OUT = $.global.PROBE_OUT;
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    function q(s) { return '"' + String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"'; }
    function spell(s) {
        var o = "";
        for (var i = 0; i < s.length; i++) {
            var c = s.charCodeAt(i);
            o += (c < 32 || c > 126) ? "<U+" + ("000" + c.toString(16).toUpperCase()).slice(-4) + ">" : s.charAt(i);
        }
        return o;
    }
    var log = [];
    var IN = $.global.PROBE_IN;
    var doc = IN ? app.open(File(IN), true) : app.documents.add(true);
    try { if (IN) { report(); } else { build(); } } finally { doc.close(SaveOptions.NO); }
    // With PROBE_IN: every story's notes as InDesign reads the document.
    function report() {
        for (var s = 0; s < doc.stories.length; s++) {
            storyState(doc.stories[s], "story" + s);
        }
        var f = File(OUT); f.encoding = "UTF-8"; f.open("w");
        f.write("{\n  " + log.join(",\n  ") + "\n}\n"); f.close();
    }
    function storyState(story, label) {
        var notes = [];
        for (var i = 0; i < story.notes.length; i++) {
            var n = story.notes[i];
            var off = -1;
            try { off = n.storyOffset.index; } catch (e) { off = "err " + e; }
            notes.push('{"offset": ' + q(off) + ', "collapsed": ' + q(n.collapsed) +
                ', "userName": ' + q(n.userName) + ', "paragraphs": ' + n.paragraphs.length +
                ', "text": ' + q(spell(n.texts[0].contents)) + '}');
        }
        log.push('"' + label + '": {"length": ' + story.characters.length +
            ', "paragraphs": ' + story.paragraphs.length +
            ', "contents": ' + q(spell(story.contents)) +
            ', "notes": [' + notes.join(", ") + ']}');
    }
    function build() {
        doc.documentPreferences.facingPages = false;
        doc.documentPreferences.pageWidth = 400;
        doc.documentPreferences.pageHeight = 300;
        var tf = doc.pages[0].textFrames.add({ geometricBounds: [20, 20, 280, 380] });
        tf.contents = "Text with a note here and more text.";
        var story = tf.parentStory;
        storyState(story, "before");
        var n1 = story.insertionPoints[15].notes.add();
        n1.texts[0].contents = "Check this claim.";
        storyState(story, "oneNote");
        var n2 = story.insertionPoints[26].notes.add();
        n2.texts[0].contents = "First line of a note.\rIts second paragraph.";
        storyState(story, "twoNotes");
        var n3 = story.insertionPoints[-1].notes.add();
        n3.texts[0].contents = "Last one.";
        storyState(story, "threeNotes");
        // Text typed before a note's place moves it as a character.
        story.insertionPoints[0].contents = "AB ";
        storyState(story, "typedBefore");
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File($.global.PROBE_IDML));
        doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PROBE_PDF));
        var names = ["convertedOne", "convertedTwo", "convertedLast"];
        for (var c = 0; c < names.length; c++) {
            try { story.notes[0].convertToText(); } catch (e) { log.push('"' + names[c] + 'Error": ' + q(e)); }
            storyState(story, names[c]);
        }
        story.insertionPoints[3].notes.add().texts[0].contents = "Gone soon.";
        storyState(story, "addedFourth");
        try { story.notes[0].remove(); } catch (e) { log.push('"removeError": ' + q(e)); }
        storyState(story, "removed");
        var f = File(OUT); f.encoding = "UTF-8"; f.open("w");
        f.write("{\n  " + log.join(",\n  ") + "\n}\n"); f.close();
    }
})();
