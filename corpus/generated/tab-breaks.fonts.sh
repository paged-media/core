# `tab-breaks` sets every paragraph in Inter (`samples/tab_breaks.rs` BODY_FONT), the
# face InDesign exported it with. Without registering it the body copy
# would fall back to a substitute and the gate would measure
# a font swap instead of the tab breaks.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
