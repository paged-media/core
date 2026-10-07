# `rect-shadows` sets its text frames' "Hi" in Inter 48 pt
# (`samples/rect_shadows.rs` TEXT_FONT), the face InDesign exported it
# with; without it the text shadow would be measured against a substitute.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
