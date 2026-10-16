# Set in Inter (`samples/numbering_multilevel.rs` BODY_FONT), the face
# InDesign exported it with; without it the engine would substitute and
# the gate would measure a font swap instead of the list numbers.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
)
