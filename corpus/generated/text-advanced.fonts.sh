# Per-sample font registrations for text-advanced.idml.
#
# The IDML's [No paragraph style] declares AppliedFont="Open Sans" and
# the reference PDF now renders it: re-exported 2026-09-06 on a host
# that HAS Open Sans installed (`PAGED_EXPORT_ONLY=text-advanced bash
# tools/indesign-export/run-export.sh`), so both sides set the face the
# fixture asks for.
#
# The file it replaces was exported on a host WITHOUT Open Sans, so
# InDesign baked its bundled Minion Pro into the reference and this
# script routed our renderer through CormorantGaramond to match — a
# fixture that measured a substitution against a substitution. The
# thresholds sized to that mismatch have been recalibrated.
FONT_FLAGS=(
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Open Sans/Italic=$FONTS/OpenSans-Italic.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
