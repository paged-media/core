# `nested-styles` sets every body paragraph in Inter (`samples/nested_styles.rs`,
# the Nest Body style) and its labels in Open Sans, the faces InDesign
# exported it with. Without registering Inter the body copy would fall
# back to a substitute and the gate would measure a font swap instead of
# the nested ranges.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
