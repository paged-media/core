# `justified-breaks` sets every paragraph in Open Sans
# (`samples/justified_breaks.rs` BODY_FONT), the face InDesign exported it
# with; registering it keeps the gate on the line breaks, not a font swap.
FONT_FLAGS=(
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
)
