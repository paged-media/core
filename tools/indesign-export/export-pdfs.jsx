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

/*
 * tools/indesign-export/export-pdfs.jsx
 *
 * Iterates every .idml under the configured input directory, opens
 * it in InDesign without prompting, exports an unattended PDF using
 * the [High Quality Print] preset, then closes without saving.
 *
 * Per-export metadata (InDesign version, timestamp, preset name) is
 * written next to each PDF as <stem>.export.meta.json so the diff
 * harness can later report which exporter the reference came from.
 *
 * Usage from a shell:
 *   osascript -e 'tell application "Adobe InDesign 2024" to do script "<full path to this .jsx>" language javascript'
 *
 * Or set INPUT_DIR / PRESET_NAME below and double-click the file in
 * Finder while InDesign is running.
 */

(function () {
    // Verbose log next to the IDMLs so the harness can read back what
    // happened — `$.writeln` only reaches the ExtendScript Toolkit
    // console, which isn't accessible from `osascript`.
    var LOG_PATH = "/tmp/idml-export.log";
    var log = File(LOG_PATH);
    log.encoding = "UTF-8";
    log.open("w");
    function logln(msg) {
        log.writeln(msg);
        $.writeln(msg);
    }
    logln("# log start " + new Date());
    try {

    var INPUT_DIR = "~/idml/corpus/generated";
    // Preset name varies by locale: "[High Quality Print]" on en_US,
    // "[Qualitativ hochwertiger Druck]" on de_DE, etc. The first
    // entry that resolves wins. Localized lists captured from
    // shipping InDesign 20.x; extend as new locales appear.
    var PRESET_CANDIDATES = [
        "[High Quality Print]",
        "[Qualitativ hochwertiger Druck]",
        "[Calidad de impresión alta]",
        "[Qualité supérieure]",
        "[高品質印刷]"
    ];

    app.scriptPreferences.userInteractionLevel =
        UserInteractionLevels.NEVER_INTERACT;

    var inputFolder = Folder(INPUT_DIR);
    logln("input folder: " + inputFolder.fsName + " (exists=" + inputFolder.exists + ")");
    if (!inputFolder.exists) {
        return;
    }

    var idmlFiles = inputFolder.getFiles("*.idml");
    logln("found " + idmlFiles.length + " idml files");
    var preset = null;
    var presetName = null;
    for (var pi = 0; pi < PRESET_CANDIDATES.length; pi++) {
        var candidate = app.pdfExportPresets.itemByName(PRESET_CANDIDATES[pi]);
        if (candidate.isValid) {
            preset = candidate;
            presetName = PRESET_CANDIDATES[pi];
            break;
        }
    }
    if (preset === null) {
        logln(
            "no high-quality-print preset matched; tried: "
                + PRESET_CANDIDATES.join(", ")
        );
        return;
    }
    logln("using preset: " + presetName);

    var indesignVersion = app.version;
    var nowISO = new Date().toISOString
        ? new Date().toISOString()
        : (new Date()).toString();

    for (var i = 0; i < idmlFiles.length; i++) {
        var idml = idmlFiles[i];
        var stem = idml.name.replace(/\.idml$/i, "");
        var pdfPath = File(idml.path + "/" + stem + ".pdf");
        var metaPath = File(idml.path + "/" + stem + ".export.meta.json");
        logln("export " + idml.name + " → " + pdfPath.name);

        var doc = null;
        try {
            doc = app.open(idml, false); // false = don't show window
            doc.exportFile(ExportFormat.PDF_TYPE, pdfPath, false, preset);
        } catch (e) {
            logln("  failed: " + e);
            if (doc !== null) doc.close(SaveOptions.NO);
            continue;
        }
        doc.close(SaveOptions.NO);

        var meta =
            "{\n" +
            '  "idml": "' + idml.name + '",\n' +
            '  "pdf": "' + pdfPath.name + '",\n' +
            '  "indesign_version": "' + indesignVersion + '",\n' +
            '  "preset": "' + presetName + '",\n' +
            '  "exported_at": "' + nowISO + '"\n' +
            "}\n";
        metaPath.encoding = "UTF-8";
        metaPath.open("w");
        metaPath.write(meta);
        metaPath.close();
    }

    logln("done");
    } catch (outerErr) {
        logln("UNCAUGHT: " + outerErr);
    }
    log.close();
})();
