# `table-inline-text` sets its body copy and cells in Inter 10 pt
# (`samples/table_inline_text.rs` BODY_FONT), the face InDesign exported
# it with; the case labels keep the generator's default face.
FONT_FLAGS=(
    --font-family "Inter=$FONTS/Inter.ttf"
    --font-family "Open Sans=$FONTS/OpenSans.ttf"
    --font-family "Minion Pro=$FONTS/CormorantGaramond.ttf"
)
