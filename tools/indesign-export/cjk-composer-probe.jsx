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

// Ask InDesign when it prints ruby and emphasis marks (kenten)
// (cjk-composer-probe.sh drives it). One frame per composer, the same
// Kozuka Mincho text in each, kenten on the first two characters and ruby
// on the fifth and sixth. Writes cjk-composer.pdf and cjk-composer.idml
// into PROBE_OUT. InDesign 20.0.1 prints ruby and kenten under its
// Japanese composers only (HL Composer J / HL Single J); the answer is
// pinned in crates/paged-renderer/src/pipeline/tests.rs.
(function () {
    var out = Folder(String($.global.PROBE_OUT));
    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    var doc = app.documents.add(false);
    var composers = ["$ID/HL Composer", "$ID/HL Composer Optyca", "$ID/HL Composer J", "$ID/HL Single J"];
    try {
        for (var c = 0; c < composers.length; c++) {
            var tf = doc.pages[0].textFrames.add({ geometricBounds: [80 + c * 140, 60, 180 + c * 140, 500] });
            tf.contents = "漢字漢字漢字";
            var t = tf.parentStory.texts[0];
            t.appliedFont = "Kozuka Mincho Pr6N";
            t.fontStyle = "R";
            t.pointSize = 30;
            tf.parentStory.paragraphs[0].composer = composers[c];
            tf.parentStory.characters.itemByRange(0, 1).kentenKind = KentenCharacter.KENTEN_SESAME_DOT;
            var r = tf.parentStory.characters.itemByRange(4, 5);
            r.rubyFlag = true;
            r.rubyString = "かんじ";
        }
        doc.exportFile(ExportFormat.PDF_TYPE, File(out.fsName + "/cjk-composer.pdf"));
        doc.exportFile(ExportFormat.INDESIGN_MARKUP, File(out.fsName + "/cjk-composer.idml"));
    } catch (e) {
        var f = File(out.fsName + "/cjk-composer.error.txt"); f.open("w"); f.write(String(e)); f.close();
    }
    doc.close(SaveOptions.NO);
})();
