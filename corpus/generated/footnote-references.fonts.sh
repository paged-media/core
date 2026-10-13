# `footnote-references` sets its body copy and notes in Inter
# (`samples/footnote_references.rs` BODY_FONT; one reference in Inter
# Bold, which the variable `Inter.ttf` covers), the face InDesign
# exported it with; the label keeps the generator's default face.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
