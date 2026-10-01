# `list-marker-styles` sets every paragraph in Inter (`samples/list_marker_styles.rs`
# BODY_FONT) and one marker character style in JetBrains Mono, the faces
# InDesign exported it with. Without registering them the gate would
# measure a font swap instead of the marker styles.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "JetBrains Mono=$FONTS/JetBrainsMono-VF.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
