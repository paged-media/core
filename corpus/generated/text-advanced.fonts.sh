# Per-sample font registrations for text-advanced.idml.
#
# The IDML's [No paragraph style] declares AppliedFont="Open Sans",
# but the InDesign-exported reference PDF was rendered with a serif
# substitute (the export host evidently did not have Open Sans
# installed and InDesign fell back to its bundled serif). To match
# the reference, route Open Sans through CormorantGaramond — the
# same family the corpus uses for the "Minion Pro" mapping.
DEFAULT_FONT="$FONTS/CormorantGaramond.ttf"
FONT_FLAGS=(
    --font-family "Open Sans=$FONTS/CormorantGaramond.ttf"
    --font-family "Open Sans/Italic=$FONTS/OpenSans-Italic.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
