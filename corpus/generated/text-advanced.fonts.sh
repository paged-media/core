# Per-sample font registrations for text-advanced.idml.
#
# The IDML's [No paragraph style] declares AppliedFont="Open Sans" but
# the InDesign-exported reference PDF was rendered with a serif
# substitute — the export host did not have Open Sans installed and
# InDesign baked its bundled serif (Minion Pro) into the PDF. CLAUDE.md
# offers two reconciliations for this class of mismatch: re-export the
# PDF on a host that has the font, or substitute in the renderer to
# match the PDF. Until the PDF is re-exported, we pick the substitute
# path: route Open Sans through CormorantGaramond (the same family the
# corpus already uses for the "Minion Pro" mapping) so the fidelity
# gate compares apples to apples. The Italic style still points at the
# real Open Sans Italic — page 1 (the drop-cap variant) and most other
# pages render upright body text, so the Italic mapping currently
# affects no rendered glyphs; it is kept here so a future variant that
# *does* use italic does not silently fall back to the regular face.
#
# When the reference PDF gets re-exported on a host that has Open Sans
# installed, swap the "Open Sans=..." line back to OpenSans.ttf, drop
# this multi-paragraph comment, and recalibrate thresholds.
DEFAULT_FONT="$FONTS/CormorantGaramond.ttf"
FONT_FLAGS=(
    --font-family "Open Sans=$FONTS/CormorantGaramond.ttf"
    --font-family "Open Sans/Italic=$FONTS/OpenSans-Italic.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
