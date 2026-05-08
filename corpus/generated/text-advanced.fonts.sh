# Per-sample font registrations for text-advanced.idml.
#
# The IDML's [No paragraph style] declares AppliedFont="Open Sans".
# The reference PDF was originally exported on a host without Open
# Sans installed (InDesign fell back to a bundled serif), so the
# fixture used to route Open Sans through CormorantGaramond. We now
# render with the real Open Sans the IDML asks for; the reference
# PDF needs re-exporting on a host that has Open Sans installed for
# the fidelity gate to be meaningful again.
DEFAULT_FONT="$FONTS/OpenSans.ttf"
FONT_FLAGS=(
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Open Sans/Italic=$FONTS/OpenSans-Italic.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
