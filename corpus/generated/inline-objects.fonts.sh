# `inline-objects` sets its body copy in Inter 10 pt
# (`samples/inline_objects.rs` BODY_FONT), the face InDesign exported it
# with; the page labels keep the generator's default face.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
