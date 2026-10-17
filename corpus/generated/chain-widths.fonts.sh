# Set in Inter (`samples/chain_widths.rs` BODY_FONT), the face
# InDesign exported it with; without it the engine would substitute and
# the gate would measure a font swap instead of the line breaks.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
)
