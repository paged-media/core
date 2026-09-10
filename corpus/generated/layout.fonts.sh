# `layout` sets its body copy in Inter (`samples/layout.rs` BODY_FONT),
# and the harness default registers only Open Sans + Minion Pro — so
# every body run in this fixture fell back to SourceSerif4 while
# InDesign, which has Inter installed, exported it in Inter. A whole
# fixture measured against a substituted face.
#
# It stayed hidden while the fixture's pages carried a line or two of
# body copy each: pages 1-6 scored 0.4-0.9 mean deltaE and the
# substitution was a rounding error inside that. The column pages
# (7-10) are 44 lines of body copy apiece, and the same substitution
# put them at 2.8 — which read like a layout defect and was not one.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Open Sans/Italic=$FONTS/OpenSans-Italic.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
