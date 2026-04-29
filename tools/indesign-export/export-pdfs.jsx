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
    var INPUT_DIR = "~/idml/corpus/generated";
    var PRESET_NAME = "[High Quality Print]";

    app.scriptPreferences.userInteractionLevel =
        UserInteractionLevels.NEVER_INTERACT;

    var inputFolder = Folder(INPUT_DIR);
    if (!inputFolder.exists) {
        $.writeln("input folder not found: " + inputFolder.fsName);
        return;
    }

    var idmlFiles = inputFolder.getFiles("*.idml");
    var preset = app.pdfExportPresets.itemByName(PRESET_NAME);
    if (!preset.isValid) {
        $.writeln("PDF preset not found: " + PRESET_NAME);
        return;
    }

    var indesignVersion = app.version;
    var nowISO = new Date().toISOString
        ? new Date().toISOString()
        : (new Date()).toString();

    for (var i = 0; i < idmlFiles.length; i++) {
        var idml = idmlFiles[i];
        var stem = idml.name.replace(/\.idml$/i, "");
        var pdfPath = File(idml.path + "/" + stem + ".pdf");
        var metaPath = File(idml.path + "/" + stem + ".export.meta.json");
        $.writeln("export " + idml.name + " → " + pdfPath.name);

        var doc = null;
        try {
            doc = app.open(idml, false); // false = don't show window
            doc.exportFile(ExportFormat.PDF_TYPE, pdfPath, false, preset);
        } catch (e) {
            $.writeln("  failed: " + e);
            if (doc !== null) doc.close(SaveOptions.NO);
            continue;
        }
        doc.close(SaveOptions.NO);

        var meta =
            "{\n" +
            '  "idml": "' + idml.name + '",\n' +
            '  "pdf": "' + pdfPath.name + '",\n' +
            '  "indesign_version": "' + indesignVersion + '",\n' +
            '  "preset": "' + PRESET_NAME + '",\n' +
            '  "exported_at": "' + nowISO + '"\n' +
            "}\n";
        metaPath.encoding = "UTF-8";
        metaPath.open("w");
        metaPath.write(meta);
        metaPath.close();
    }

    $.writeln("done");
})();
