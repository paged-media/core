# `soft-hyphens` sets every body paragraph in Inter (`samples/soft_hyphens.rs` BODY_FONT), the
# face InDesign exported it with. Without registering it the body copy
# would fall back to a substitute and the gate would measure
# a font swap instead of the soft hyphens.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
