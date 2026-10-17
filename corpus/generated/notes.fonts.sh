# Set in Inter (`samples/notes.rs` FONT), the face InDesign exported it
# with; without it the engine would substitute and the gate would measure
# a font swap instead of the notes printing nothing.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
)
