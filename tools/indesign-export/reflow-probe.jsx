// Ask InDesign how Smart Text Reflow grows a document (thoughts ADR 026).
//
// Smart Text Reflow is an IDLE task: InDesign runs it after an edit, when it
// gets idle time, never while a script is running. So the probe runs as two
// scripts (reflow-probe.sh drives both):
//   PAGED_REFLOW_PHASE=prepare  open the IDML, record "before", switch the
//                               reflow preferences on, make the edit, RETURN
//   PAGED_REFLOW_PHASE=report   measure the still-open document ("after"),
//                               write the JSON, export the PDF, close
//
// Report: every page (name, applied master, bounds) and every text frame on
// it (bounds, story, first/last paragraph, line count), plus the story's
// overset state, in points.
//
// Inputs: PAGED_REFLOW_IDML, PAGED_REFLOW_JSON, PAGED_REFLOW_PDF,
// PAGED_REFLOW_LIMIT ("true" = limit to primary text frames),
// PAGED_REFLOW_EDIT ("grow" | "shrink"), PAGED_REFLOW_PHASE.

(function () {
    function q(s) {
        s = String(s);
        return '"' + s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/[\r\n\u2029\u2028]/g, " ") + '"';
    }
    function arr(a) {
        var r = [];
        for (var i = 0; i < a.length; i++) r.push(String(Math.round(a[i] * 100) / 100));
        return "[" + r.join(",") + "]";
    }
    function firstLast(frame) {
        var ps = frame.paragraphs;
        if (ps.length === 0) return '"first":null,"last":null,"lines":0';
        var first = String(ps[0].contents).substr(0, 24);
        var last = String(ps[ps.length - 1].contents).substr(0, 24);
        return '"first":' + q(first) + ',"last":' + q(last) + ',"lines":' + frame.lines.length;
    }
    function bodyStory(doc) {
        for (var s = 0; s < doc.stories.length; s++) {
            if (String(doc.stories[s].contents).indexOf("Paragraph 01") === 0) return doc.stories[s];
        }
        return null;
    }
    function report(doc, story, label) {
        var pages = [];
        for (var p = 0; p < doc.pages.length; p++) {
            var page = doc.pages[p];
            var frames = [];
            for (var f = 0; f < page.textFrames.length; f++) {
                var tf = page.textFrames[f];
                frames.push("{" +
                    '"bounds":' + arr(tf.geometricBounds) + "," +
                    '"story":' + (story && tf.parentStory.id === story.id ? '"body"' : '"other"') + "," +
                    firstLast(tf) + "}");
            }
            pages.push("{" +
                '"name":' + q(page.name) + "," +
                '"master":' + q(page.appliedMaster ? page.appliedMaster.name : "") + "," +
                '"bounds":' + arr(page.bounds) + "," +
                '"frames":[' + frames.join(",") + "]}");
        }
        return '"' + label + '":{"pages":' + doc.pages.length +
            ',"overflows":' + (story ? story.overflows : "null") + ',"page_list":[' + pages.join(",") + "]}";
    }
    function fail(e) {
        var o = File($.global.PAGED_REFLOW_JSON);
        o.encoding = "UTF-8";
        o.open("w");
        o.write('{"error":' + q(e.message) + ',"line":' + e.line + "}\n");
        o.close();
        while (app.documents.length) app.documents[0].close(SaveOptions.NO);
    }

    function prepare() {
        while (app.documents.length) app.documents[0].close(SaveOptions.NO);
        var limit = String($.global.PAGED_REFLOW_LIMIT) === "true";
        var edit = String($.global.PAGED_REFLOW_EDIT || "grow");
        var doc = app.open(File($.global.PAGED_REFLOW_IDML), true);
        var body = bodyStory(doc);
        // "geometry": page 2's frame differs from the master frame, to see
        // which one a generated page copies. [y1, x1, y2, x2] in points.
        if (String($.global.PAGED_REFLOW_VARIANT) === "geometry") {
            doc.pages[1].textFrames[0].geometricBounds = [60, 60, 360, 260];
        }
        // "master": the master frame differs from the margin box, to see
        // whether a generated frame follows the master frame or the margins.
        if (String($.global.PAGED_REFLOW_VARIANT) === "master") {
            doc.masterSpreads[0].pages[0].textFrames[0].geometricBounds = [100, 100, 300, 250];
        }
        var before = report(doc, body, "before");

        var tp = doc.textPreferences;
        tp.smartTextReflow = true;
        tp.addPages = AddPageOptions.END_OF_STORY;
        tp.limitToMasterTextFrames = limit;
        tp.deleteEmptyPages = true;
        tp.preserveFacingPageSpreads = false;

        if (edit === "shrink") {
            var n = body.paragraphs.length;
            body.paragraphs.itemByRange(n - 50, n - 1).remove();
        } else {
            body.insertionPoints[-1].contents = " ";
            body.characters[-1].remove();
        }
        // Stash the "before" half for the report phase (same InDesign session).
        app.insertLabel("paged.reflow.before",
            '"limit_to_master_text_frames":' + limit + ',"edit":' + q(edit) + "," + before);
    }

    function reportPhase() {
        var doc = app.activeDocument;
        var body = bodyStory(doc);
        var parts = [app.extractLabel("paged.reflow.before")];
        parts.push(report(doc, body, "after"));
        parts.push('"indesign_version":' + q(app.version));
        var out = File($.global.PAGED_REFLOW_JSON);
        out.encoding = "UTF-8";
        out.open("w");
        out.write("{" + parts.join(",") + "}\n");
        out.close();
        var preset = app.pdfExportPresets.itemByName("[High Quality Print]");
        if (!preset.isValid) preset = app.pdfExportPresets[0];
        doc.exportFile(ExportFormat.PDF_TYPE, File($.global.PAGED_REFLOW_PDF), false, preset);
        doc.close(SaveOptions.NO);
    }

    app.scriptPreferences.userInteractionLevel = UserInteractionLevels.NEVER_INTERACT;
    app.scriptPreferences.measurementUnit = MeasurementUnits.POINTS;
    try {
        if (String($.global.PAGED_REFLOW_PHASE) === "report") reportPhase();
        else prepare();
    } catch (e) {
        fail(e);
    }
})();
