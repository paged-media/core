# Set in Inter (`samples/footnote_columns.rs` BODY_FONT), the face
# InDesign exported it with; without it the engine would substitute and
# the gate would measure a font swap instead of the numbering.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
)
