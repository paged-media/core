# Set in Inter (`samples/toc.rs` FONT), the face InDesign exported it
# with; without it the engine would substitute and the gate would measure
# a font swap instead of the stored table of contents.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
)
