# `styles-cascade` page 4 sets its OpenType-feature runs (fractions,
# ordinals, contextual alternates) in Inter, the face InDesign exported it
# with (its reference embeds Inter). The rest of the fixture is Open Sans.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
